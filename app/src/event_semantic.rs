//! Semantic candidate retrieval for the relationship lane. Claim texts are
//! embedded once (keyed by prepared text, so unchanged re-readings reuse the
//! vector) and each claim's closest reports inside the publication window become
//! extra candidate pairs. Similarity never links reports: every candidate still
//! goes through the evaluated pair review with exact support.
use crate::{
    embeddings,
    error::{Error, Result},
    event_link_store::Member,
    event_links,
    model::*,
    store::Store,
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

/// Batches embedded per lane page; each batch is one bounded request.
pub const BATCHES_PER_PAGE: usize = 4;
/// Claims whose neighbours are scanned per page.
const SCAN_PAGE: usize = 64;

pub fn text_hash(text: &str) -> String {
    hash(string(&json!({"profile":embeddings::profile(),"text":text})).as_bytes())
}
fn encode(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|n| n.to_le_bytes()).collect()
}
fn decode(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() != embeddings::DIMENSIONS * 4 {
        return Err(Error::invalid("Stored claim vector has the wrong dimension"));
    }
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect())
}
/// One reserved embedding request: exact texts and the hashes they fill.
pub struct Batch {
    pub id: String,
    pub hashes: Vec<String>,
    pub texts: Vec<String>,
}
/// Claims of the current publication, from eligible present sources.
const CURRENT_CLAIMS: &str = "SELECT c.id,c.evidence_rowid,c.published_millis,c.json,t.text_hash FROM event_link_claims c JOIN event_claim_text t ON t.claim_id=c.id JOIN publication_interpretations p ON p.interpretation_id=c.interpretation_id AND p.publication_id=?1 JOIN evidence e ON e.rowid=c.evidence_rowid WHERE e.source IN(SELECT value FROM json_each(?2)) AND coalesce(json_extract(e.json,'$.upstream_state'),'present')!='removed' AND c.published_millis>=(unixepoch('now')-coalesce((SELECT CAST(value AS REAL) FROM metadata WHERE key='processing_max_age_days'),1e9)*86400)*1000";

impl Store {
    /// Reserve up to `BATCHES_PER_PAGE` requests for claim texts without vectors.
    /// Admission is durable before any network call.
    pub fn reserve_claim_embeddings(&self, cancel: &AtomicBool) -> Result<Vec<Batch>> {
        let policy = self.processing_policy()?;
        if !policy.enabled || !self.event_links_ready()? || cancel.load(Ordering::Acquire) {
            return Ok(vec![]);
        }
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let cooling: bool = tx.query_row("SELECT coalesce((SELECT value FROM metadata WHERE key='event_embeddings_retry_at'),'')>?1", [now()], |r| r.get(0))?;
        let Some(pubid) = Self::latest(&tx)?.filter(|_| !cooling) else {
            return Ok(vec![]);
        };
        let rows = tx
            .prepare(&format!("SELECT x.text_hash,json_extract(x.json,'$.claim.text') FROM ({CURRENT_CLAIMS}) x WHERE NOT EXISTS(SELECT 1 FROM event_claim_vectors v WHERE v.text_hash=x.text_hash) GROUP BY x.text_hash ORDER BY min(x.published_millis) DESC LIMIT ?3"))?
            .query_map(
                params![
                    pubid,
                    string(&value(&policy.eligible_source_ids)),
                    (BATCHES_PER_PAGE * embeddings::BATCH) as i64
                ],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut batches = vec![];
        for chunk in rows.chunks(embeddings::BATCH) {
            let (hashes, texts): (Vec<_>, Vec<_>) = chunk.iter().cloned().unzip();
            let request = embeddings::request(&texts)?;
            let bytes = string(&request);
            let h = self.write_object(bytes.as_bytes())?;
            self.sync_object_directories(std::iter::once(h.clone()), &AtomicBool::new(false))?;
            tx.execute(
                "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
                params![h, bytes.len() as i64],
            )?;
            let id = ident("embedding");
            tx.execute(
                "INSERT INTO event_embedding_attempts VALUES(?1,'reserved',?2,NULL,?3,NULL,NULL)",
                params![id, now(), h],
            )?;
            batches.push(Batch { id, hashes, texts });
        }
        tx.commit()?;
        Ok(batches)
    }
    /// Record one embedding outcome with its exact receipt. Anything other than a
    /// decoded 200 stops the lane until the owner resumes it; nothing is retried.
    pub fn finish_claim_embeddings(
        &self,
        batch: &Batch,
        status: Option<u16>,
        bytes: &[u8],
        error: Option<(&str, &str)>,
    ) -> Result<()> {
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let response = serde_json::from_slice::<Value>(bytes).ok();
        let cost = response
            .as_ref()
            .and_then(|v| v["usage"]["cost"].as_f64())
            .filter(|v| v.is_finite() && *v >= 0.0);
        let not_sent = error.is_some_and(|e| e.0 == "model_not_sent");
        let credits = status == Some(402) && crate::processing_ai::credit_rejection(402, bytes);
        let decoded = (status == Some(200))
            .then(|| {
                response
                    .as_ref()
                    .ok_or_else(|| Error::invalid("Invalid embedding JSON"))
                    .and_then(|v| embeddings::decode(v, batch.texts.len()))
            })
            .transpose();
        let (state, detail, charge) = match (&decoded, not_sent) {
            (_, true) => ("not_sent", Some("Cancelled before dispatch".to_string()), Some(0.0)),
            _ if credits => ("rate_limited", Some(crate::event_link_store::CREDITS.to_string()), Some(0.0)),
            (Ok(Some(_)), _) => ("completed", None, cost),
            (Err(e), _) => ("failed", Some(e.message.clone()), cost),
            (Ok(None), _) => (
                if status.is_some() { "failed" } else { "unknown" },
                Some(match (status, crate::event_link_store::provider_message(response.as_ref())) {
                    (Some(code), Some(message)) => format!("Embedding provider HTTP {code}: {message}"),
                    (Some(code), None) => format!("Embedding provider HTTP {code}"),
                    (None, _) => error.map(|(_, m)| m.to_owned()).unwrap_or_else(|| "Embedding request failed without a response".into()),
                }),
                cost,
            ),
        };
        let body = self.write_object(bytes)?;
        let receipt = json!({"attempt":batch.id,"status":status,"raw_body_hash":body,"error":detail,"received_at":now()});
        let receipt = string(&receipt);
        let h = self.write_object(receipt.as_bytes())?;
        self.sync_object_directories([body.clone(), h.clone()].into_iter(), &AtomicBool::new(false))?;
        for (object, len) in [(&body, bytes.len()), (&h, receipt.len())] {
            tx.execute(
                "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
                params![object, len as i64],
            )?;
        }
        let changed = tx.execute(
            "UPDATE event_embedding_attempts SET status=?2,cost_usd=?3,receipt_hash=?4,error=?5 WHERE id=?1 AND status='reserved'",
            params![batch.id, state, charge, h, detail],
        )?;
        if changed != 1 {
            return Err(Error::conflict("Embedding attempt is no longer reserved"));
        }
        if let Ok(Some(vectors)) = decoded {
            for (text_hash, vector) in batch.hashes.iter().zip(vectors) {
                tx.execute(
                    "INSERT OR IGNORE INTO event_claim_vectors VALUES(?1,?2,?3)",
                    params![text_hash, batch.id, encode(&vector)],
                )?;
            }
        }
        if credits {
            let until = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            tx.execute("INSERT INTO metadata VALUES('event_embeddings_retry_at',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [until])?;
        } else if state != "completed" && state != "not_sent" || (state == "completed" && charge.is_none()) {
            tx.execute("INSERT INTO metadata VALUES('event_links_error',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[detail.unwrap_or_else(||"Embedding billing is unknown; resume processing after review".into())])?;
        }
        tx.commit()?;
        Ok(())
    }
    /// For claims with vectors that were not scanned yet, keep the closest
    /// `SEMANTIC_NEIGHBOURS` reviewable reports above the floor as candidates.
    pub fn scan_semantic_neighbours(&self, cancel: &AtomicBool) -> Result<usize> {
        let policy = self.processing_policy()?;
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let Some(pubid) = Self::latest(&tx)? else {
            return Ok(0);
        };
        let sources = string(&value(&policy.eligible_source_ids));
        let pending = tx
            .prepare(&format!("SELECT x.id,x.evidence_rowid,x.published_millis,x.json,v.vector FROM ({CURRENT_CLAIMS}) x JOIN event_claim_vectors v ON v.text_hash=x.text_hash WHERE NOT EXISTS(SELECT 1 FROM event_semantic_scanned s WHERE s.claim_id=x.id) ORDER BY x.published_millis LIMIT ?3"))?
            .query_map(params![pubid, sources, SCAN_PAGE as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, Vec<u8>>(4)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let window = event_links::SEMANTIC_WINDOW_SECONDS * 1000;
        let mut near = tx.prepare(&format!("SELECT x.id,x.evidence_rowid,x.published_millis,x.json,v.vector FROM ({CURRENT_CLAIMS}) x JOIN event_claim_vectors v ON v.text_hash=x.text_hash WHERE x.published_millis BETWEEN ?3 AND ?4 AND x.id!=?5"))?;
        let mut inserted = 0;
        for (id, rowid, millis, raw, vector) in &pending {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let own: Member = serde_json::from_str(raw)?;
            let own_vector = decode(vector)?;
            let peers = near
                .query_map(params![pubid, sources, millis - window, millis + window, id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, Vec<u8>>(4)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut ranked = vec![];
            for (peer_id, peer_rowid, peer_millis, peer_raw, peer_vector) in peers {
                if peer_rowid == *rowid {
                    continue;
                }
                let peer: Member = serde_json::from_str(&peer_raw)?;
                // Canonical pair order matches place admission: earlier first.
                let (a, b) = if (peer_millis, &peer_id) < (*millis, id) {
                    (&peer, &own)
                } else {
                    (&own, &peer)
                };
                if !event_links::eligible_semantic(&a.claim, &b.claim) {
                    continue;
                }
                let cosine = embeddings::cosine(&own_vector, &decode(&peer_vector)?)?;
                if cosine >= event_links::SEMANTIC_FLOOR {
                    ranked.push((cosine, a.id.clone(), b.id.clone()));
                }
            }
            ranked.sort_by(|x, y| y.0.total_cmp(&x.0).then_with(|| x.1.cmp(&y.1)).then_with(|| x.2.cmp(&y.2)));
            for (cosine, a, b) in ranked.into_iter().take(event_links::SEMANTIC_NEIGHBOURS) {
                inserted += tx.execute(
                    "INSERT OR IGNORE INTO event_semantic_candidates VALUES(?1,?2,?3)",
                    params![a, b, cosine],
                )?;
            }
            tx.execute("INSERT OR IGNORE INTO event_semantic_scanned VALUES(?1)", [id])?;
        }
        drop(near);
        tx.commit()?;
        Ok(inserted)
    }
    pub fn embedding_status(&self) -> Result<Value> {
        let c = self.connect()?;
        let (completed, failed, cost, unknown): (i64, i64, f64, i64) = c.query_row(
            "SELECT coalesce(sum(status='completed'),0),coalesce(sum(status IN ('failed','unknown')),0),coalesce(sum(cost_usd),0),coalesce(sum(cost_usd IS NULL AND status!='reserved'),0) FROM event_embedding_attempts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let (vectors, candidates): (i64, i64) = c.query_row(
            "SELECT (SELECT count(*) FROM event_claim_vectors),(SELECT count(*) FROM event_semantic_candidates)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let last: Option<String> = c
            .query_row(
                "SELECT error FROM event_embedding_attempts WHERE status IN ('failed','unknown') ORDER BY created_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(json!({"model":embeddings::MODEL,"requests":completed,"failed":failed,"vectors":vectors,"candidates":candidates,"known_cost_usd":cost,"unknown_cost_requests":unknown,"last_error":last}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vectors_round_trip_exactly_and_reject_wrong_dimensions() {
        let v = (0..embeddings::DIMENSIONS).map(|i| i as f32 / 7.0).collect::<Vec<_>>();
        assert_eq!(decode(&encode(&v)).unwrap(), v);
        assert!(decode(&[0; 12]).is_err());
        assert_ne!(text_hash("Fire at North Pier"), text_hash("Fire at North Pier."));
    }
}
