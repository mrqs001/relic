//! Durable relationship admission and publication. Reading/evidence identity is
//! authoritative; links are optional context, never replacements or incidents.
use crate::{
    error::{Error, Result},
    event_links::{self, Review},
    event_similarity::Claim,
    model::*,
    store::{Store, edge, node, predicate},
};
use rusqlite::{OptionalExtension, params, types::Value as SqlValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicBool, Ordering},
};
pub const PAGE: usize = 32;
pub(crate) const CREDITS: &str = "OpenRouter credits are exhausted; comparisons resume after credits are added";
/// The provider's error message, bounded for display.
pub(crate) fn provider_message(response: Option<&Value>) -> Option<String> {
    let message = response?["error"]["message"].as_str()?.trim();
    (!message.is_empty()).then(|| message.chars().take(240).collect())
}
/// Claim projection version: v2 keeps claims without a single resolved place.
const PROJECTION: &str = "relic.event-claim-projection.v2";
pub const CONCURRENCY: usize = 8;
fn retry_deadline(header: Option<&str>) -> chrono::DateTime<chrono::Utc> {
    let fallback = chrono::Utc::now() + chrono::Duration::minutes(5);
    let parsed = header.and_then(|s| {
        if let Ok(seconds) = s.parse::<i64>() {
            Some(
                chrono::Duration::try_seconds(seconds)
                    .and_then(|d| chrono::Utc::now().checked_add_signed(d))
                    .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC),
            )
        } else {
            chrono::DateTime::parse_from_rfc2822(s)
                .ok()
                .map(|d| d.with_timezone(&chrono::Utc))
        }
    });
    parsed.map(|d| d.max(fallback)).unwrap_or(fallback)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Member {
    pub id: String,
    pub evidence_rowid: i64,
    pub evidence_id: String,
    pub revision: String,
    pub interpretation_id: String,
    pub citation: Citation,
    pub source_quote: String,
    pub claim: Claim,
}
#[derive(Clone)]
pub struct Pair {
    pub a: Member,
    pub b: Member,
    pub key: String,
    pub request: Value,
}
pub fn profile_key() -> String {
    hash(string(&event_links::profile()).as_bytes())
}
pub fn admission_key() -> String {
    hash(
        string(&json!({"review":profile_key(),"retrieval":event_links::retrieval_profile()}))
            .as_bytes(),
    )
}
/// Gold is unchanged: a safe abstention is explicitly acceptable only for a
/// negative case. Positive relationship labels remain exact requirements.
pub fn acceptance() -> Result<Value> {
    let fixture: Value =
        serde_json::from_str(include_str!("../../examples/event-link-evaluation.json"))?;
    let replies: Value =
        serde_json::from_str(include_str!("../../examples/event-link-responses.json"))?;
    let mut exact = 0;
    let mut accepted = 0;
    for case in fixture["cases"].as_array().ok_or_else(Error::internal)? {
        let a: Claim = serde_json::from_value(case["a"].clone())?;
        let b: Claim = serde_json::from_value(case["b"].clone())?;
        if !event_links::reviewable(&a, &b) {
            if case["related"] == false {
                exact += 1;
                accepted += 1;
            }
            continue;
        }
        let row = replies["responses"]
            .as_array()
            .ok_or_else(Error::internal)?
            .iter()
            .find(|r| r["case_id"] == case["id"])
            .ok_or_else(Error::internal)?;
        if row["request_sha256"] != hash(string(&event_links::request(&a, &b)?).as_bytes()) {
            return Err(Error::invalid(
                "Relationship request profile needs evaluation",
            ));
        }
        let review = event_links::decode(&a, &b, &row["response"])?;
        let actual = value(&review.decision.relation);
        if actual == case["expected"] {
            exact += 1;
            accepted += 1;
        } else if case["related"] == false && !review.decision.relation.is_link() {
            accepted += 1;
        }
    }
    let total = fixture["cases"].as_array().unwrap().len();
    Ok(
        json!({"passed":accepted==total,"accepted":accepted,"strict_exact":exact,"total":total,"profile":profile_key(),"fixture_sha256":hash(include_bytes!("../../examples/event-link-evaluation.json")),"criterion":"exact positive labels; negatives remain non-links; source support mandatory"}),
    )
}
fn project(rowid: i64, e: &Value, i: &Value) -> Result<Vec<Member>> {
    let text = required(e, "text")?;
    if i["evidence_id"] != e["id"]
        || i["revision"] != e["revision"]
        || i["source_input_hash"] != hash(text.as_bytes())
    {
        return Err(Error::invalid(
            "Relationship reading does not match exact evidence",
        ));
    }
    let mut result = vec![];
    let Some(events) = i["events"].as_array() else {
        return Ok(result);
    };
    if events.len() > 5 {
        return Err(Error::invalid(
            "Relationship claim count exceeds reading contract",
        ));
    }
    for event in events {
        if !["reported", "denied"].contains(&event["state"].as_str().unwrap_or("")) {
            continue;
        }
        let places: BTreeSet<&str> = event["locations"]
            .as_array()
            .ok_or_else(Error::internal)?
            .iter()
            .filter(|l| {
                l["role"] == "event"
                    && ["resolved", "resolved_area", "context_only"]
                        .contains(&l["status"].as_str().unwrap_or(""))
            })
            .filter_map(|l| l["selected_candidate_id"].as_str())
            .collect();
        // Without exactly one resolved place a claim is kept for semantic
        // retrieval only; place-based candidates need that single identity.
        let claim = Claim {
            text: required(event, "text")?.into(),
            event_type: required(event, "type")?.into(),
            state: required(event, "state")?.into(),
            place: (places.len() == 1).then(|| places.first().unwrap().to_string()),
            published_at: e["published_at"].as_str().map(str::to_owned),
            time_text: event["time_text"].as_str().map(str::to_owned),
        };
        if !event_links::indexable(&claim) {
            continue;
        }
        let citation: Citation = serde_json::from_value(event["citation"].clone())?;
        let quote = required(event, "quote")?;
        if citation.end_character > text.chars().count()
            || citation.start_character >= citation.end_character
            || citation.evidence_id != e["id"]
            || citation.revision != e["revision"]
            || citation.end_character < citation.start_character
            || text
                .chars()
                .skip(citation.start_character)
                .take(citation.end_character - citation.start_character)
                .collect::<String>()
                != quote
        {
            return Err(Error::invalid("Relationship claim citation is not exact"));
        }
        let reading = required(i, "id")?;
        let id = required(event, "id")?;
        if !id.starts_with(&format!("{reading}:event:")) {
            return Err(Error::invalid("Claim does not belong to reading"));
        }
        result.push(Member {
            id: id.into(),
            evidence_rowid: rowid,
            evidence_id: required(e, "id")?.into(),
            revision: required(e, "revision")?.into(),
            interpretation_id: reading.into(),
            citation,
            source_quote: quote.into(),
            claim,
        });
    }
    Ok(result)
}
fn save_object(s: &Store, c: &rusqlite::Connection, bytes: &[u8]) -> Result<String> {
    let h = s.write_object(bytes)?;
    s.sync_object_directories(std::iter::once(h.clone()), &AtomicBool::new(false))?;
    c.execute(
        "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
        params![h, bytes.len() as i64],
    )?;
    Ok(h)
}
impl Store {
    /// Cheap idle watermark. A changed publication/policy/projection or a due
    /// known-unsent retry wakes exhaustive candidate scanning.
    pub fn event_link_work_version(&self) -> Result<String> {
        let c = self.connect()?;
        let version: String = c.query_row("SELECT json_array((SELECT value FROM metadata WHERE key='current_publication'),(SELECT count(*) FROM event_link_indexed),(SELECT count(*) FROM event_link_pairs),(SELECT count(*) FROM event_link_receipts),(SELECT count(*) FROM event_claim_vectors),(SELECT count(*) FROM event_semantic_scanned),(SELECT count(*) FROM event_semantic_candidates),(SELECT count(*) FROM event_embedding_attempts),coalesce((SELECT value FROM metadata WHERE key='event_embeddings_retry_at'),'')>?2,EXISTS(SELECT 1 FROM event_link_attempts WHERE profile=?1 AND status='rate_limited' AND retry_at>?2),EXISTS(SELECT 1 FROM event_link_attempts WHERE profile=?1 AND status IN('not_sent','rate_limited') AND (retry_at IS NULL OR retry_at<=?2)))",params![profile_key(),now()],|r|r.get(0))?;
        Ok(format!(
            "{}:{}:{}",
            version,
            string(&value(self.processing_policy()?)),
            admission_key()
        ))
    }
    pub fn event_links_ready(&self) -> Result<bool> {
        Ok(self.event_links_enabled()?
            && self.processing_policy()?.enabled
            && !self.connect()?.query_row(
                "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='event_links_error')",
                [],
                |r| r.get::<_, bool>(0),
            )?)
    }
    pub fn event_links_enabled(&self) -> Result<bool> {
        Ok(self
            .connect()?
            .query_row(
                "SELECT value FROM metadata WHERE key='event_links_enabled'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .as_deref()
            == Some(&admission_key()))
    }
    pub fn set_event_links_enabled(&self, enabled: bool) -> Result<Value> {
        if enabled && acceptance()?["passed"] != true {
            return Err(Error::invalid("Event relationship evaluation failed"));
        }
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        self.connect()?.execute("INSERT INTO metadata VALUES('event_links_enabled',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[if enabled{admission_key()}else{String::new()}])?;
        self.connect()?
            .execute("DELETE FROM metadata WHERE key='event_links_error'", [])?;
        if !enabled {
            self.cancel_event_links();
        }
        drop(_lock);
        self.event_link_status()
    }
    /// Stop the lane with a visible reason until the owner resumes it.
    pub fn record_event_link_error(&self, message: &str) -> Result<()> {
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        self.connect()?.execute("INSERT INTO metadata VALUES('event_links_error',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[message])?;
        Ok(())
    }
    pub fn cancel_event_links(&self) {
        if let Ok(r) = self.running.lock() {
            for (id, flag) in r.iter() {
                if id.starts_with("processing:relationships:") {
                    flag.store(true, Ordering::Release);
                }
            }
        }
    }
    pub fn event_link_status(&self) -> Result<Value> {
        let mut connection = self.connect()?;
        // One snapshot, so the latest completion read here bounds `last` below.
        let c = connection.transaction()?;
        let (completed,pending,failed,cost,unknown,last_completed):(i64,i64,i64,f64,i64,Option<String>)=c.query_row("SELECT coalesce(sum(status='completed'),0),coalesce(sum(status='reserved'),0),coalesce(sum(status IN ('failed','unknown')),0),coalesce(sum(cost_usd),0),coalesce(sum(cost_usd IS NULL AND status!='reserved'),0),max(CASE WHEN status='completed' THEN created_at END) FROM event_link_attempts WHERE profile=?1",[profile_key()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
        // Links between current readings; older reading versions keep theirs for history.
        // The current readings are listed once: joining the view directly made
        // SQLite index every retained reading interval on each status poll.
        let links: i64 = c.query_row("WITH readings AS MATERIALIZED (SELECT interpretation_id FROM publication_interpretations WHERE publication_id=(SELECT value FROM metadata WHERE key='current_publication')) SELECT count(*) FROM event_links l JOIN event_link_claims ca ON ca.id=l.a JOIN readings pa ON pa.interpretation_id=ca.interpretation_id JOIN event_link_claims cb ON cb.id=l.b JOIN readings pb ON pb.interpretation_id=cb.interpretation_id", [], |r| r.get(0))?;
        let enabled_profile: Option<String> = c
            .query_row(
                "SELECT value FROM metadata WHERE key='event_links_enabled'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let activation_required =
            enabled_profile.is_some_and(|v| !v.is_empty() && v != admission_key());
        // A failure already followed by a completed comparison is history, not the
        // lane's current state; `failed` still counts it.
        let last:Option<String>=c.query_row("SELECT error FROM event_link_attempts WHERE profile=?1 AND status IN ('failed','unknown','rate_limited') AND created_at>coalesce(?2,'') ORDER BY created_at DESC LIMIT 1",params![profile_key(),last_completed],|r|r.get(0)).optional()?.flatten();
        let last = c
            .query_row(
                "SELECT value FROM metadata WHERE key='event_links_error'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .or(last);
        // Costs include the retrieval embeddings; `semantic` breaks them out.
        let semantic = self.embedding_status()?;
        let cost = cost + semantic["known_cost_usd"].as_f64().unwrap_or(0.0);
        let unknown = unknown + semantic["unknown_cost_requests"].as_i64().unwrap_or(0);
        Ok(
            json!({"enabled":self.event_links_enabled()?,"activation_required":activation_required,"completed":completed,"in_flight":pending,"failed":failed,"links":links,"known_cost_usd":cost,"unknown_cost_requests":unknown,"last_error":last,"stopped":c.query_row("SELECT EXISTS(SELECT 1 FROM metadata WHERE key='event_links_error')",[],|r|r.get::<_,bool>(0))?,"semantic":semantic,"projection_errors":c.query_row("SELECT count(*) FROM event_link_projection_errors",[],|r|r.get::<_,i64>(0))?,"running":self.running.lock().is_ok_and(|r|r.keys().any(|id|id.starts_with("processing:relationships:")))}),
        )
    }
    pub fn index_event_claims(&self, cancel: &AtomicBool) -> Result<usize> {
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        // A projection change re-reads every current reading once. Existing claim
        // rows are immutable and kept (INSERT OR IGNORE); only markers reset.
        let projection: Option<String> = tx.query_row("SELECT value FROM metadata WHERE key='event_link_projection'", [], |r| r.get(0)).optional()?;
        if projection.as_deref() != Some(PROJECTION) {
            tx.execute("DELETE FROM event_link_indexed", [])?;
            tx.execute("INSERT INTO metadata VALUES('event_link_projection',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [PROJECTION])?;
        }
        let Some(pubid) = Self::latest(&tx)? else {
            return Ok(0);
        };
        let rows=tx.prepare("SELECT e.rowid,e.json,i.json FROM publication_interpretations p JOIN evidence e ON e.rowid=p.evidence_rowid JOIN interpretations i ON i.id=p.interpretation_id WHERE p.publication_id=?1 AND NOT EXISTS(SELECT 1 FROM event_link_indexed x WHERE x.interpretation_id=i.id) LIMIT 100")?.query_map([pubid],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        for (rowid, raw, reading) in &rows {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let i: Value = serde_json::from_str(reading)?;
            let members = match project(*rowid, &serde_json::from_str(raw)?, &i) {
                Ok(members) => members,
                Err(error) => {
                    tx.execute(
                        "INSERT OR IGNORE INTO event_link_projection_errors VALUES(?1,?2)",
                        params![required(&i, "id")?, error.message],
                    )?;
                    vec![]
                }
            };
            for m in members {
                let seconds =
                    chrono::DateTime::parse_from_rfc3339(m.claim.published_at.as_deref().unwrap())
                        .map_err(|_| Error::internal())?
                        .timestamp_millis();
                tx.execute(
                    "INSERT OR IGNORE INTO event_link_claims VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        m.id,
                        rowid,
                        m.interpretation_id,
                        m.claim.place.clone().unwrap_or_default(),
                        m.claim.event_type,
                        seconds,
                        string(&value(&m))
                    ],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO event_claim_text VALUES(?1,?2)",
                    params![m.id, crate::event_semantic::text_hash(&m.claim.text)],
                )?;
            }
            tx.execute(
                "INSERT OR IGNORE INTO event_link_indexed VALUES(?1)",
                [required(&i, "id")?],
            )?;
        }
        tx.commit()?;
        Ok(rows.len())
    }
    pub fn admit_event_pairs(&self, cancel: &AtomicBool) -> Result<Vec<Pair>> {
        let policy = self.processing_policy()?;
        if !policy.enabled || !self.event_links_ready()? || cancel.load(Ordering::Acquire) {
            return Ok(vec![]);
        }
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let cooling:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM event_link_attempts WHERE profile=?2 AND retry_at> ?1 AND status='rate_limited')",params![now(),profile_key()],|r|r.get(0))?;
        if cooling {
            return Ok(vec![]);
        }
        let Some(pubid) = Self::latest(&tx)? else {
            return Ok(vec![]);
        };
        let profile = profile_key();
        let admission = admission_key();
        let rows=tx.prepare("SELECT a.json,b.json FROM event_link_claims a JOIN event_link_claims b ON a.place!='' AND b.place=a.place AND b.published_millis BETWEEN a.published_millis AND a.published_millis+21600000 AND (b.published_millis>a.published_millis OR b.id>a.id) AND b.evidence_rowid!=a.evidence_rowid JOIN publication_interpretations pa ON pa.interpretation_id=a.interpretation_id AND pa.publication_id=?1 JOIN publication_interpretations pb ON pb.interpretation_id=b.interpretation_id AND pb.publication_id=?1 JOIN evidence ea ON ea.rowid=a.evidence_rowid JOIN evidence eb ON eb.rowid=b.evidence_rowid WHERE ea.id!=eb.id AND a.published_millis>=(unixepoch('now')-coalesce((SELECT CAST(value AS REAL) FROM metadata WHERE key='processing_max_age_days'),1e9)*86400)*1000 AND b.published_millis>=(unixepoch('now')-coalesce((SELECT CAST(value AS REAL) FROM metadata WHERE key='processing_max_age_days'),1e9)*86400)*1000 AND ea.source IN(SELECT value FROM json_each(?2)) AND eb.source IN(SELECT value FROM json_each(?2)) AND coalesce(json_extract(ea.json,'$.upstream_state'),'present')!='removed' AND coalesce(json_extract(eb.json,'$.upstream_state'),'present')!='removed' AND NOT EXISTS(SELECT 1 FROM event_link_pairs p WHERE p.a=a.id AND p.b=b.id AND p.profile=?3) ORDER BY a.published_millis,a.id,b.id LIMIT 32")?.query_map(params![pubid,string(&value(&policy.eligible_source_ids)),admission],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut rows = rows.into_iter().map(|(a, b)| (a, b, false)).collect::<Vec<_>>();
        if rows.len() < PAGE {
            let semantic=tx.prepare("SELECT a.json,b.json FROM event_semantic_candidates s JOIN event_link_claims a ON a.id=s.a JOIN event_link_claims b ON b.id=s.b JOIN publication_interpretations pa ON pa.interpretation_id=a.interpretation_id AND pa.publication_id=?1 JOIN publication_interpretations pb ON pb.interpretation_id=b.interpretation_id AND pb.publication_id=?1 JOIN evidence ea ON ea.rowid=a.evidence_rowid JOIN evidence eb ON eb.rowid=b.evidence_rowid WHERE ea.id!=eb.id AND a.published_millis>=(unixepoch('now')-coalesce((SELECT CAST(value AS REAL) FROM metadata WHERE key='processing_max_age_days'),1e9)*86400)*1000 AND b.published_millis>=(unixepoch('now')-coalesce((SELECT CAST(value AS REAL) FROM metadata WHERE key='processing_max_age_days'),1e9)*86400)*1000 AND ea.source IN(SELECT value FROM json_each(?2)) AND eb.source IN(SELECT value FROM json_each(?2)) AND coalesce(json_extract(ea.json,'$.upstream_state'),'present')!='removed' AND coalesce(json_extract(eb.json,'$.upstream_state'),'present')!='removed' AND NOT EXISTS(SELECT 1 FROM event_link_pairs p WHERE p.a=a.id AND p.b=b.id AND p.profile=?3) ORDER BY s.cosine DESC,a.id,b.id LIMIT ?4")?.query_map(params![pubid,string(&value(&policy.eligible_source_ids)),admission,(PAGE-rows.len()) as i64],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            rows.extend(semantic.into_iter().map(|(a, b)| (a, b, true)));
        }
        let mut work = vec![];
        for (ar, br, semantic) in rows {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let a: Member = serde_json::from_str(&ar)?;
            let b: Member = serde_json::from_str(&br)?;
            let eligible = if semantic {
                event_links::eligible_semantic(&a.claim, &b.claim)
            } else {
                event_links::eligible(&a.claim, &b.claim)
            };
            if !eligible {
                tx.execute(
                    "INSERT OR IGNORE INTO event_link_pairs VALUES(?1,?2,?3,NULL,'ineligible')",
                    params![a.id, b.id, admission],
                )?;
                continue;
            }
            let request = event_links::request(&a.claim, &b.claim)?;
            let key = hash(format!("{}:{}", profile, string(&request)).as_bytes());
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM event_link_attempts WHERE id=?1)",
                [&key],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO event_link_pairs VALUES(?1,?2,?3,NULL,'pending')",
                params![a.id, b.id, admission],
            )?;
            if !exists {
                let h = save_object(self, &tx, string(&request).as_bytes())?;
                tx.execute("INSERT INTO event_link_attempts VALUES(?1,?2,'reserved',?3,NULL,?4,NULL,NULL,NULL,NULL)",params![key,profile,now(),h])?;
                work.push(Pair {
                    a: a.clone(),
                    b: b.clone(),
                    key: key.clone(),
                    request,
                });
            }
            tx.execute(
                "UPDATE event_link_pairs SET attempt_id=?4 WHERE a=?1 AND b=?2 AND profile=?3",
                params![a.id, b.id, admission, key],
            )?;
        }
        // Only explicit pre-generation 429 or known-unsent cancellation may be retried.
        let retries=tx.prepare("SELECT x.id,a.json,b.json FROM event_link_attempts x JOIN event_link_pairs p ON p.attempt_id=x.id JOIN event_link_claims a ON a.id=p.a JOIN event_link_claims b ON b.id=p.b JOIN publication_interpretations pa ON pa.interpretation_id=a.interpretation_id AND pa.publication_id=?1 JOIN publication_interpretations pb ON pb.interpretation_id=b.interpretation_id AND pb.publication_id=?1 JOIN evidence ea ON ea.rowid=a.evidence_rowid JOIN evidence eb ON eb.rowid=b.evidence_rowid WHERE x.profile=?4 AND coalesce(json_extract(ea.json,'$.upstream_state'),'present')!='removed' AND coalesce(json_extract(eb.json,'$.upstream_state'),'present')!='removed' AND x.status IN('not_sent','rate_limited') AND (x.retry_at IS NULL OR x.retry_at<=?2) AND ea.source IN(SELECT value FROM json_each(?3)) AND eb.source IN(SELECT value FROM json_each(?3)) GROUP BY x.id LIMIT 8")?.query_map(params![pubid,now(),string(&value(&policy.eligible_source_ids)),profile],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        for (key, a, b) in retries {
            let a: Member = serde_json::from_str(&a)?;
            let b: Member = serde_json::from_str(&b)?;
            tx.execute("UPDATE event_link_attempts SET status='reserved',cost_usd=NULL,error=NULL,retry_at=NULL WHERE id=?1",[&key])?;
            work.push(Pair {
                request: event_links::request(&a.claim, &b.claim)?,
                a,
                b,
                key,
            });
        }
        tx.commit()?;
        Ok(work)
    }
    pub fn finish_event_pair(
        &self,
        pair: &Pair,
        status: Option<u16>,
        bytes: &[u8],
        error: Option<(&str, &str)>,
        retry_after: Option<&str>,
    ) -> Result<()> {
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let response = serde_json::from_slice::<Value>(bytes).ok();
        let cost = response
            .as_ref()
            .and_then(|v| v["usage"]["cost"].as_f64())
            .filter(|v| v.is_finite() && *v >= 0.0);
        let mut retry = None;
        let mut review = None;
        let mut charge = cost;
        let state;
        let detail;
        if error.is_some_and(|e| e.0 == "model_not_sent") {
            state = "not_sent";
            detail = Some("Cancelled before dispatch".to_string());
            charge = Some(0.0);
        } else if status == Some(402) && crate::processing_ai::credit_rejection(402, bytes) {
            // Pre-flight credit rejection: nothing generated or billed. Cool down
            // and retry; the lane continues by itself once credits are added.
            state = "rate_limited";
            detail = Some(CREDITS.into());
            charge = Some(0.0);
            retry = Some(
                retry_deadline(retry_after).to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            );
        } else if status == Some(429)
            && !error.is_some_and(|e| e.0 == "generation_id")
            && cost.is_none_or(|c| c == 0.0)
            && response.as_ref().is_some_and(|v| {
                v["error"]["code"] == 429
                    && v.get("id").is_none()
                    && v["error"]
                        .get("metadata")
                        .and_then(|m| m.get("generation_id"))
                        .is_none()
            })
        {
            state = "rate_limited";
            detail = Some("Provider rate limit; retry after cooldown".into());
            charge = Some(0.0);
            retry = Some(
                retry_deadline(retry_after).to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            );
        } else if status == Some(200) {
            match response
                .as_ref()
                .ok_or_else(|| Error::invalid("Invalid model JSON"))
                .and_then(|v| event_links::decode(&pair.a.claim, &pair.b.claim, v))
            {
                Ok(r) => {
                    state = "completed";
                    detail = None;
                    review = Some(string(&value(r)));
                }
                Err(e) => {
                    state = "failed";
                    detail = Some(e.message);
                }
            }
        } else {
            state = if status.is_some() {
                "failed"
            } else {
                "unknown"
            };
            // Name the provider's own error; a generation id is not a reason.
            detail = Some(match (status, provider_message(response.as_ref())) {
                (Some(code), Some(message)) => format!("Relationship provider HTTP {code}: {message}"),
                (Some(code), None) => format!("Relationship provider HTTP {code}"),
                (None, _) => error
                    .filter(|e| e.0 != "generation_id")
                    .map(|(_, m)| m.to_owned())
                    .unwrap_or_else(|| "Relationship request failed without a response".into()),
            });
        }
        let body_hash = save_object(self, &tx, bytes)?;
        let receipt = json!({"request_key":pair.key,"status":status,"raw_body_hash":body_hash,"error":detail,"retry_after":retry_after,"received_at":now()});
        let h = save_object(self, &tx, string(&receipt).as_bytes())?;
        let changed=tx.execute("UPDATE event_link_attempts SET status=?2,cost_usd=?3,receipt_hash=?4,review_json=?5,error=?6,retry_at=?7 WHERE id=?1 AND status='reserved'",params![pair.key,state,charge,h,review,detail,retry])?;
        if changed != 1 {
            return Err(Error::conflict(
                "Relationship attempt is no longer reserved",
            ));
        }
        tx.execute(
            "INSERT OR IGNORE INTO event_link_receipts VALUES(?1,?2)",
            params![pair.key, h],
        )?;
        if (status.is_none() && state != "not_sent")
            || status.is_some_and(|v| v != 200 && state != "rate_limited")
            || (status == Some(200) && charge.is_none())
        {
            tx.execute("INSERT INTO metadata VALUES('event_links_error',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[detail.clone().unwrap_or_else(||"Relationship billing is unknown; resume processing after review".into())])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn publish_event_links(&self, cancel: &AtomicBool) -> Result<usize> {
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let rows=tx.prepare("SELECT p.a,p.b,x.id,x.review_json,a.json,b.json FROM event_link_pairs p JOIN event_link_attempts x ON x.id=p.attempt_id JOIN event_link_claims a ON a.id=p.a JOIN event_link_claims b ON b.id=p.b WHERE p.profile=?1 AND p.state='pending' AND x.status='completed' LIMIT 64")?.query_map([admission_key()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        if rows.is_empty() {
            return Ok(0);
        }
        let prior = Self::latest(&tx)?.ok_or_else(Error::internal)?;
        let mut publication: Option<String> = None;
        let mut count = 0;
        for (a_id, b_id, key, raw, ar, br) in rows {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let review: Review = serde_json::from_str(&raw)?;
            let a: Member = serde_json::from_str(&ar)?;
            let b: Member = serde_json::from_str(&br)?;
            let checked = event_links::validate(&a.claim, &b.claim, &value(&review.decision))?;
            if checked.input_sha256 != review.input_sha256
                || checked.profile_sha256 != review.profile_sha256
            {
                return Err(Error::invalid(
                    "Stored relationship profile or input changed",
                ));
            }
            if review.decision.relation.is_link() {
                let id = format!(
                    "event-link:{}",
                    hash(format!("{a_id}:{b_id}:{key}").as_bytes())
                );
                // A retrieval-policy change may revisit a paid pair. Its exact
                // immutable link already belongs to history; do not publish an
                // empty duplicate generation merely to mark the new scan done.
                if tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM event_links WHERE id=?1)",
                    [&id],
                    |r| r.get::<_, bool>(0),
                )? {
                    tx.execute("UPDATE event_link_pairs SET state='complete' WHERE a=?1 AND b=?2 AND profile=?3",params![a_id,b_id,admission_key()])?;
                    continue;
                }
                let pubid = if let Some(p) = &publication {
                    p.clone()
                } else {
                    let p = ident("publication");
                    tx.execute(
                        "INSERT INTO publications VALUES(?1,'Related messages',?2,'complete',NULL)",
                        params![p, now()],
                    )?;
                    crate::publication_history::inherit(&tx, &p, Some(&prior))?;
                    node(
                        &tx,
                        &p,
                        "publication",
                        "Related messages",
                        &json!({"profile":profile_key(),"admission":admission_key(),"retrieval":event_links::retrieval_profile()}),
                    )?;
                    publication = Some(p.clone());
                    p
                };
                let record = json!({"id":id,"a":a,"b":b,"review":review,"attempt":key,"retrieval":event_links::retrieval_profile(),"meaning":"model-suggested relationship, not a verified incident or independent confirmation"});
                tx.execute(
                    "INSERT OR IGNORE INTO event_links VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        id,
                        a_id,
                        b_id,
                        key,
                        pubid,
                        string(&value(&review.decision.relation)).trim_matches('"'),
                        string(&record)
                    ],
                )?;
                node(&tx, &id, "event_relationship", "Related messages", &record)?;
                edge(
                    &tx,
                    &a.interpretation_id,
                    &id,
                    "relationship_input",
                    &value(&a.citation),
                )?;
                edge(
                    &tx,
                    &b.interpretation_id,
                    &id,
                    "relationship_input",
                    &value(&b.citation),
                )?;
                edge(&tx, &id, &pubid, "published", &json!({"attempt":key}))?;
                count += 1;
            }
            tx.execute(
                "UPDATE event_link_pairs SET state='complete' WHERE a=?1 AND b=?2 AND profile=?3",
                params![a_id, b_id, admission_key()],
            )?;
        }
        let ev = if let Some(pubid) = publication {
            tx.execute(
                "UPDATE metadata SET value=?1 WHERE key='current_publication'",
                [&pubid],
            )?;
            Some(Self::event(&tx, "processing", Some(&pubid), vec![])?)
        } else {
            None
        };
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        tx.commit()?;
        if let Some(ev) = ev {
            let _ = self.events.send(ev);
        }
        Ok(count)
    }
    /// The anchor message's event groups, one per event it reports: messages
    /// connected to that event's claim through published links visible at the
    /// selected publication. Only in-scope messages with their exact current
    /// reading are members or bridges, so a filtered-out message never joins two
    /// visible reports, and a message's other events never merge groups.
    pub fn event_group(&self, input: Value) -> Result<Value> {
        let mut scope = Scope::parse(&input["scope"])?;
        let mut connection = self.connect()?;
        let c = connection.transaction()?;
        let pubid = Self::resolve_scope(&c, &mut scope)?;
        let (pred, vals) = predicate(&scope);
        let mut anchor_vals = vals.clone();
        anchor_vals.push(SqlValue::Text(required(&input, "id")?.into()));
        anchor_vals.push(SqlValue::Text(required(&input, "revision")?.into()));
        let anchor:Option<(i64,Option<String>)>=c.query_row(&format!("SELECT e.rowid,json_extract(e.json,'$.interpretation.id') FROM reading_evidence e WHERE {pred} AND e.id=? AND e.revision=?"),rusqlite::params_from_iter(anchor_vals),|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((rowid, Some(reading))) = anchor else {
            return Ok(json!({"publication_id":pubid,"groups":[]}));
        };
        let seq: i64 = c.query_row("SELECT seq FROM publication_order WHERE id=?1", [&pubid], |r| r.get(0))?;
        let own = c
            .prepare("SELECT id,json FROM event_link_claims WHERE interpretation_id=?1 AND evidence_rowid=?2 ORDER BY id")?
            .query_map(params![reading, rowid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut groups = vec![];
        let mut covered = BTreeSet::new();
        for (own_id, raw) in own {
            if covered.contains(&own_id) {
                continue;
            }
            let start: Member = serde_json::from_str(&raw)?;
            let (claims, edges, truncated) = walk(&c, seq, &pred, &vals, own_id.clone(), start)?;
            covered.extend(claims.keys().cloned());
            if edges.is_empty() {
                continue;
            }
            let rowids = claims.values().map(|m| m.evidence_rowid).collect::<BTreeSet<_>>();
            let mut member_vals = vals.clone();
            member_vals.push(SqlValue::Text(string(&json!(rowids))));
            let rows = c
                .prepare(&format!("SELECT e.rowid,e.json FROM reading_evidence e WHERE {pred} AND e.rowid IN (SELECT value FROM json_each(?)) ORDER BY coalesce(e.published_at,e.acquired_at),e.rowid"))?
                .query_map(rusqlite::params_from_iter(member_vals), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            // Members carry the same map sites and areas as feed rows, so the
            // reader can show where these reports place the event.
            let mut evidence = rows
                .iter()
                .map(|(row, raw)| {
                    let mut e: Value = serde_json::from_str(raw)?;
                    e["message_key"] = json!(row.to_string());
                    Ok(e)
                })
                .collect::<Result<Vec<_>>>()?;
            let mut site_vals = vals.clone();
            site_vals.push(SqlValue::Text(string(&json!(rowids))));
            let sites = crate::geographic_view::sites(&c, &scope, &format!("{pred} AND e.rowid IN (SELECT value FROM json_each(?))"), &site_vals, GROUP_LIMIT * 25)?;
            for e in &mut evidence {
                crate::geographic_view::attach_sites(e, &sites);
            }
            crate::area_view::attach(&c, &scope, &mut evidence)?;
            let mut items = vec![];
            for ((row, _), e) in rows.iter().zip(evidence) {
                let own_claims = claims
                    .values()
                    .filter(|m| m.evidence_rowid == *row)
                    .map(|m| json!({"id":m.id,"text":m.claim.text,"state":m.claim.state,"type":m.claim.event_type}))
                    .collect::<Vec<_>>();
                items.push(json!({"evidence":e,"anchor":*row==rowid,"claims":own_claims}));
            }
            let links = edges
                .into_iter()
                .map(|(a, b, relation, a_state, b_state)| json!({"a":a,"b":b,"relation":relation,"a_state":a_state,"b_state":b_state}))
                .collect::<Vec<_>>();
            let claim = &claims[&own_id];
            groups.push(json!({"claim":{"id":own_id,"text":claim.claim.text,"type":claim.claim.event_type},"members":items,"links":links,"truncated":truncated}));
        }
        Ok(json!({"publication_id":pubid,"groups":groups}))
    }
    pub fn query_event_links(&self, input: Value) -> Result<Value> {
        let mut scope = Scope::parse(&input["scope"])?;
        let mut connection = self.connect()?;
        let c = connection.transaction()?;
        let pubid = Self::resolve_scope(&c, &mut scope)?;
        let (pred, vals) = predicate(&scope);
        let id = required(&input, "id")?;
        let revision = required(&input, "revision")?;
        let mut anchor_vals = vals.clone();
        anchor_vals.push(SqlValue::Text(id.into()));
        anchor_vals.push(SqlValue::Text(revision.into()));
        let anchor:Option<(i64,Option<String>)>=c.query_row(&format!("SELECT e.rowid,json_extract(e.json,'$.interpretation.id') FROM reading_evidence e WHERE {pred} AND e.id=? AND e.revision=?"),rusqlite::params_from_iter(anchor_vals),|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((rowid, Some(reading))) = anchor else {
            return Ok(json!({"publication_id":pubid,"total":0,"items":[],"next_offset":null}));
        };
        let seq: i64 = c.query_row(
            "SELECT seq FROM publication_order WHERE id=?1",
            [&pubid],
            |r| r.get(0),
        )?;
        let offset = input["offset"].as_u64().unwrap_or(0).min(100_000) as i64;
        // Both ends must have their exact reading in the selected publication. Scope
        // includes the anchor and every peer, so counts never leak excluded messages.
        let sql = format!(
            "WITH related AS (SELECT peer.evidence_rowid,peer.interpretation_id,l.relation,l.id,CASE WHEN own.published_millis=peer.published_millis THEN 'contemporary' WHEN l.b=peer.id THEN 'later' ELSE 'earlier' END direction,json_extract(peer.json,'$.claim.state') peer_state,json_extract(peer.json,'$.claim.text') peer_claim FROM event_link_claims own JOIN event_links l ON (l.a=own.id OR l.b=own.id) JOIN event_link_claims peer ON peer.id=CASE WHEN l.a=own.id THEN l.b ELSE l.a END JOIN publication_order lp ON lp.id=l.publication_id WHERE own.evidence_rowid={rowid} AND own.interpretation_id=? AND lp.seq<={seq}) SELECT e.json,json_group_array(json_object('id',r.id,'relation',r.relation,'direction',r.direction,'peer_state',r.peer_state,'peer_claim',r.peer_claim)) FROM related r JOIN reading_evidence e ON e.rowid=r.evidence_rowid WHERE {pred} AND r.interpretation_id=json_extract(e.json,'$.interpretation.id') GROUP BY e.rowid ORDER BY coalesce(e.published_at,e.acquired_at) DESC,e.rowid DESC"
        );
        let mut args = vec![SqlValue::Text(reading)];
        args.extend(vals);
        let total: i64 = c.query_row(
            &format!("SELECT count(*) FROM ({sql})"),
            rusqlite::params_from_iter(&args),
            |r| r.get(0),
        )?;
        args.push(SqlValue::Integer(offset));
        let rows = c
            .prepare(&format!("{sql} LIMIT 20 OFFSET ?"))?
            .query_map(rusqlite::params_from_iter(args), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut items = vec![];
        for (e, links) in rows {
            items.push(json!({"evidence":serde_json::from_str::<Value>(&e)?,"links":serde_json::from_str::<Value>(&links)?}));
        }
        Ok(
            json!({"publication_id":pubid,"total":total,"next_offset":if offset+20<total{Some(offset+20)}else{None},"items":items}),
        )
    }
}
/// Largest event group returned to the reader.
pub const GROUP_LIMIT: usize = 60;
type Edge = (String, String, String, String, String);
/// Claims reachable from `start` through visible links, in scope, up to
/// `GROUP_LIMIT` messages; the edges between them (by evidence id).
fn walk(
    c: &rusqlite::Connection,
    seq: i64,
    pred: &str,
    vals: &[SqlValue],
    start_id: String,
    start: Member,
) -> Result<(BTreeMap<String, Member>, BTreeSet<Edge>, bool)> {
    let mut claims: BTreeMap<String, Member> = BTreeMap::from([(start_id.clone(), start)]);
    let mut frontier = vec![start_id];
    let mut edges = BTreeSet::new();
    let mut truncated = false;
    let messages = |claims: &BTreeMap<String, Member>| {
        claims.values().map(|m| m.evidence_rowid).collect::<BTreeSet<_>>().len()
    };
    while !frontier.is_empty() {
        let links = visible_links(c, seq, &frontier)?;
        let unseen = links
            .iter()
            .flat_map(|(a, b, _)| [a, b])
            .filter(|id| !claims.contains_key(*id))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        frontier = vec![];
        for (id, member) in visible_claims(c, pred, vals, &unseen)? {
            if messages(&claims) >= GROUP_LIMIT
                && !claims.values().any(|m| m.evidence_rowid == member.evidence_rowid)
            {
                truncated = true;
                continue;
            }
            claims.insert(id.clone(), member);
            frontier.push(id);
        }
        for (a, b, relation) in links {
            if let (Some(a), Some(b)) = (claims.get(&a), claims.get(&b))
                && a.evidence_rowid != b.evidence_rowid
            {
                edges.insert((a.evidence_id.clone(), b.evidence_id.clone(), relation, a.claim.state.clone(), b.claim.state.clone()));
            }
        }
    }
    Ok((claims, edges, truncated))
}
/// Links touching `claims` that were published at or before `seq`.
fn visible_links(c: &rusqlite::Connection, seq: i64, claims: &[String]) -> Result<Vec<(String, String, String)>> {
    if claims.is_empty() {
        return Ok(vec![]);
    }
    let ids = string(&json!(claims));
    Ok(c.prepare("SELECT l.a,l.b,l.relation FROM event_links l JOIN publication_order lp ON lp.id=l.publication_id WHERE lp.seq<=?1 AND l.a IN (SELECT value FROM json_each(?2)) UNION SELECT l.a,l.b,l.relation FROM event_links l JOIN publication_order lp ON lp.id=l.publication_id WHERE lp.seq<=?1 AND l.b IN (SELECT value FROM json_each(?2))")?
        .query_map(params![seq, ids], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?)
}
/// Claims whose exact reading is the current reading of an in-scope message.
fn visible_claims(c: &rusqlite::Connection, pred: &str, vals: &[SqlValue], claims: &[String]) -> Result<Vec<(String, Member)>> {
    if claims.is_empty() {
        return Ok(vec![]);
    }
    let mut args = vals.to_vec();
    args.push(SqlValue::Text(string(&json!(claims))));
    let rows = c
        .prepare(&format!("SELECT c.id,c.json FROM event_link_claims c JOIN reading_evidence e ON e.rowid=c.evidence_rowid WHERE {pred} AND c.id IN (SELECT value FROM json_each(?)) AND c.interpretation_id=json_extract(e.json,'$.interpretation.id')"))?
        .query_map(rusqlite::params_from_iter(args), |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, raw)| Ok((id, serde_json::from_str(&raw)?)))
        .collect()
}
/// Kinds of event joined as one occurrence by the grouping rule: the "attack"
/// family (Recall folded explosion, airstrike and attack the same way). Other
/// types join only their own type.
const ATTACK_FAMILY: [&str; 6] = ["strike", "armed_attack", "explosion", "weapons_launch", "air_defense", "structural_damage"];
/// Reports chain into one event while each is within this gap of the previous…
const RULE_GAP_MS: i64 = 3 * 3600 * 1000;
/// …and the chain spans no more than this, so a campaign is not one event.
const RULE_SPAN_MS: i64 = 12 * 3600 * 1000;
type ScopeClaim = (String, i64, String, String, i64, Option<String>);
/// Reports per (place, event family): (published millis, claim id, message row).
type RuleBuckets<'a> = std::collections::HashMap<(&'a str, &'a str), Vec<(i64, &'a str, i64)>>;

/// Rule-based links (owner, 26 October 2026, after reviewed pair links left
/// obvious repeats apart): reported claims of the same event family sharing a
/// place (an exact site, or a province, sea or other sub-country area; never a
/// country alone) within [`RULE_GAP_MS`] of each other, chains capped at
/// [`RULE_SPAN_MS`]. A reviewed "distinct" decision vetoes a pair. Claims of
/// one message are not linked to each other. Not independent confirmation.
fn rule_links(c: &rusqlite::Connection, claims: &[ScopeClaim]) -> Result<Vec<(String, String)>> {
    use std::collections::{HashMap, HashSet};
    let reported = claims.iter().filter(|r| r.5.as_deref() == Some("reported")).collect::<Vec<_>>();
    if reported.len() < 2 {
        return Ok(vec![]);
    }
    let interpretations = string(&json!(reported.iter().map(|r| r.2.as_str()).collect::<BTreeSet<_>>()));
    let mut keys: HashMap<(String, String), BTreeSet<String>> = HashMap::new();
    let mut stmt = c.prepare("SELECT interpretation_id,event_type,site_key FROM interpretation_sites WHERE interpretation_id IN (SELECT value FROM json_each(?1)) UNION ALL SELECT a.interpretation_id,a.event_type,a.area_id FROM interpretation_areas a JOIN boundary_shapes b ON b.id=a.area_id WHERE a.interpretation_id IN (SELECT value FROM json_each(?1)) AND b.level!='ADM0'")?;
    for row in stmt.query_map([&interpretations], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
        let (interpretation, kind, place) = row?;
        keys.entry((interpretation, kind)).or_default().insert(place);
    }
    fn family(kind: &str) -> &str {
        if ATTACK_FAMILY.contains(&kind) { "attack" } else { kind }
    }
    let mut buckets: RuleBuckets = HashMap::new();
    for claim in &reported {
        for place in keys.get(&(claim.2.clone(), claim.3.clone())).into_iter().flatten() {
            buckets.entry((place.as_str(), family(&claim.3))).or_default().push((claim.4, claim.0.as_str(), claim.1));
        }
    }
    let mut edges = BTreeSet::new();
    for mut reports in buckets.into_values() {
        reports.sort();
        let mut start = i64::MIN;
        let mut previous: Option<(i64, &str, i64)> = None;
        for report in reports {
            match previous {
                Some(p) if report.0 - p.0 <= RULE_GAP_MS && report.0 - start <= RULE_SPAN_MS => {
                    if report.2 != p.2 {
                        edges.insert((p.1.to_owned(), report.1.to_owned()));
                    }
                }
                _ => start = report.0,
            }
            previous = Some(report);
        }
    }
    if edges.is_empty() {
        return Ok(vec![]);
    }
    let involved = string(&json!(edges.iter().flat_map(|(a, b)| [a, b]).collect::<BTreeSet<_>>()));
    let distinct = c
        .prepare("SELECT p.a,p.b FROM event_link_pairs p JOIN event_link_attempts t ON t.id=p.attempt_id WHERE p.a IN (SELECT value FROM json_each(?1)) AND p.b IN (SELECT value FROM json_each(?1)) AND json_extract(t.review_json,'$.decision.relation')='distinct'")?
        .query_map([&involved], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<HashSet<_>, _>>()?;
    Ok(edges.into_iter().filter(|(a, b)| !distinct.contains(&(a.clone(), b.clone())) && !distinct.contains(&(b.clone(), a.clone()))).collect())
}

/// Event groups in one scope: connected components of the links visible at this
/// publication between claims of current readings, restricted to in-scope
/// messages, together with [`rule_links`]. Claims, not messages, are joined,
/// because one message can report several events. Each message maps to its
/// largest group: (group id, reports).
pub(crate) fn scope_groups(
    c: &rusqlite::Connection,
    pubid: &str,
    pred: &str,
    vals: &[SqlValue],
) -> Result<std::collections::HashMap<i64, (String, usize)>> {
    use std::collections::HashMap;
    // A claim counts when it belongs to the reading published for its message
    // here. That reading's identity is its interval's interpretation (a stored
    // reading's id is its document's "id"), so the merged message document is
    // built only for messages without a published reading.
    let claims: Vec<ScopeClaim> = c
        .prepare(&format!("SELECT c.id,c.evidence_rowid,c.interpretation_id,c.event_type,c.published_millis,json_extract(c.json,'$.claim.state') FROM event_link_claims c JOIN reading_evidence e ON e.rowid=c.evidence_rowid WHERE {pred} AND c.interpretation_id=coalesce((SELECT pi.interpretation_id FROM publication_interpretations pi WHERE pi.publication_id=e.publication_id AND pi.evidence_rowid=e.rowid),json_extract(e.json,'$.interpretation.id'))"))?
        .query_map(rusqlite::params_from_iter(vals.iter()), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
        .collect::<std::result::Result<_, _>>()?;
    if claims.is_empty() {
        return Ok(HashMap::new());
    }
    let in_scope: HashMap<String, i64> = claims.iter().map(|claim| (claim.0.clone(), claim.1)).collect();
    let seq: i64 = c.query_row("SELECT seq FROM publication_order WHERE id=?1", [pubid], |r| r.get(0))?;
    let mut links = c
        .prepare("SELECT l.a,l.b FROM event_links l JOIN publication_order o ON o.id=l.publication_id JOIN event_link_claims ca ON ca.id=l.a JOIN publication_interpretations pa ON pa.interpretation_id=ca.interpretation_id AND pa.publication_id=?1 JOIN event_link_claims cb ON cb.id=l.b JOIN publication_interpretations pb ON pb.interpretation_id=cb.interpretation_id AND pb.publication_id=?1 WHERE o.seq<=?2")?
        .query_map(params![pubid, seq], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    links.extend(rule_links(c, &claims)?);
    if links.is_empty() {
        return Ok(HashMap::new());
    }
    let mut parent: HashMap<String, String> = HashMap::new();
    fn find(parent: &mut HashMap<String, String>, x: &str) -> String {
        let mut root = x.to_owned();
        while let Some(p) = parent.get(&root).filter(|p| **p != root) {
            root = p.clone();
        }
        let mut node = x.to_owned();
        while node != root {
            let next = parent.insert(node.clone(), root.clone()).unwrap_or_else(|| root.clone());
            node = next;
        }
        root
    }
    for (a, b) in &links {
        if !in_scope.contains_key(a) || !in_scope.contains_key(b) {
            continue;
        }
        parent.entry(a.clone()).or_insert_with(|| a.clone());
        parent.entry(b.clone()).or_insert_with(|| b.clone());
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        // The smaller claim id is the stable root, so group ids do not depend on order.
        if ra != rb {
            let (keep, other) = if ra < rb { (ra, rb) } else { (rb, ra) };
            parent.insert(other, keep);
        }
    }
    let claims = parent.keys().cloned().collect::<Vec<_>>();
    let mut groups: HashMap<String, BTreeSet<i64>> = HashMap::new();
    let mut member: Vec<(String, i64)> = vec![];
    for claim in claims {
        let root = find(&mut parent, &claim);
        let row = in_scope[&claim];
        groups.entry(root.clone()).or_default().insert(row);
        member.push((root, row));
    }
    let mut out: HashMap<i64, (String, usize)> = HashMap::new();
    for (root, row) in member {
        let reports = groups[&root].len();
        let id = format!("event:{}", &hash(root.as_bytes())[..16]);
        let better = out.get(&row).is_none_or(|(old, n)| reports > *n || (reports == *n && id < *old));
        if better {
            out.insert(row, (id, reports));
        }
    }
    Ok(out)
}
/// Adds `event_group` {id, reports} and `event_links` (other reports of that
/// event in this scope) to each feed row that belongs to an event group.
pub(crate) fn attach_event_links(
    groups: &std::collections::HashMap<i64, (String, usize)>,
    items: &mut [Value],
) {
    for item in items {
        if let Some((id, reports)) = item["message_key"]
            .as_str()
            .and_then(|k| k.parse::<i64>().ok())
            .and_then(|k| groups.get(&k))
        {
            item["event_group"] = json!({"id":id,"reports":reports});
            item["event_links"] = json!(reports - 1);
        }
    }
}
