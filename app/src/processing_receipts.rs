//! Durable billing handles and bounded, read-only recovery. A receipt does not
//! authorize retrying a failed generation or publishing an incomplete response.
use crate::{
    error::{Error, Result},
    model::{now, string},
    processing_ai,
    store::Store,
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

pub struct ReceiptRequest {
    pub attempt_id: String,
    pub generation_id: String,
    pub model: String,
}
impl Store {
    pub fn save_processing_generation(
        &self,
        attempt: &str,
        generation: &str,
        model: &str,
    ) -> Result<()> {
        if !processing_ai::valid_generation_id(generation) {
            return Err(Error::invalid("Invalid generation identifier"));
        }
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let c = self.connect()?;
        let updated = c.execute("UPDATE processing_attempts SET json=json_patch(json,?2) WHERE id=?1 AND status='reserved' AND json_extract(json,'$.receipt_generation_id') IS NULL",
            params![attempt, string(&json!({"receipt_generation_id":generation,"receipt_model":model}))])?;
        if updated != 1 {
            return Err(Error::conflict(
                "Processing receipt was already saved or its attempt ended",
            ));
        }
        Ok(())
    }

    /// Called while holding the single processing worker. Claim at most one
    /// lookup, with five-minute persisted spacing and eight attempts total.
    pub fn claim_processing_receipt(&self) -> Result<Option<ReceiptRequest>> {
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let c = self.connect()?;
        let at = now();
        let mut stmt = c.prepare("SELECT id,json FROM processing_attempts WHERE cost_usd IS NULL AND coalesce(json_extract(json,'$.receipt_checks'),0)<8 AND coalesce(json_extract(json,'$.receipt_next_at'),'')<=?1 ORDER BY created_at DESC LIMIT 100")?;
        let rows = stmt
            .query_map([&at], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (id, raw) in rows {
            let record: Value = serde_json::from_str(&raw)?;
            let audit: Value = match record["audit_hash"].as_str() {
                Some(hash) => serde_json::from_slice(&self.original_object(hash)?)?,
                None => Value::Null,
            };
            let audit = audit.get("audit").unwrap_or(&audit);
            let header_id = record["receipt_generation_id"].as_str();
            let body_id = audit["generation_id"].as_str();
            if header_id.zip(body_id).is_some_and(|(a, b)| a != b) {
                continue;
            }
            let Some(generation) = header_id
                .or(body_id)
                .filter(|id| processing_ai::valid_generation_id(id))
            else {
                continue;
            };
            let Some(model) = record["receipt_model"]
                .as_str()
                .or(audit["requested_model"].as_str())
            else {
                continue;
            };
            let next = (chrono::Utc::now() + chrono::Duration::minutes(5))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            c.execute("UPDATE processing_attempts SET json=json_patch(json,?2) WHERE id=?1 AND cost_usd IS NULL", params![id, string(&json!({"receipt_generation_id":generation,"receipt_model":model,"receipt_checks":record["receipt_checks"].as_u64().unwrap_or(0)+1,"receipt_next_at":next}))])?;
            return Ok(Some(ReceiptRequest {
                attempt_id: id,
                generation_id: generation.to_owned(),
                model: model.to_owned(),
            }));
        }
        Ok(None)
    }

    pub fn apply_processing_receipt(
        &self,
        request: &ReceiptRequest,
        receipt: &Value,
    ) -> Result<()> {
        let cost = processing_ai::receipt_cost(receipt, &request.generation_id, &request.model)
            .map_err(|_| {
                Error::invalid("Billing receipt does not identify this completed generation")
            })?;
        let bytes = serde_json::to_vec(receipt)?;
        if bytes.len() > 16384 {
            return Err(Error::invalid("Billing receipt exceeds its byte bound"));
        }
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let row: Option<(Option<f64>, String)> = tx
            .query_row(
                "SELECT cost_usd,json FROM processing_attempts WHERE id=?1",
                [&request.attempt_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (prior, record) = row.ok_or_else(|| Error::missing("Processing attempt not found"))?;
        let record: Value = serde_json::from_str(&record)?;
        if record["receipt_generation_id"] != request.generation_id
            || record["receipt_model"] != request.model
        {
            return Err(Error::conflict("Billing receipt identity changed"));
        }
        if let Some(prior) = prior {
            if prior == cost {
                return Ok(());
            }
            return Err(Error::conflict(
                "Billing receipt cannot overwrite an accounted cost",
            ));
        }
        let hash = self.write_object(&bytes)?;
        self.sync_object_directories(
            std::iter::once(hash.clone()),
            &std::sync::atomic::AtomicBool::new(false),
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
            params![hash, bytes.len() as i64],
        )?;
        // Only billing changes. Failed/unknown outcomes never become successful
        // interpretations, and the original response/error remain intact.
        tx.execute("UPDATE processing_attempts SET cost_usd=?2,status=CASE WHEN status='reserved' THEN 'unknown' ELSE status END,json=json_patch(json,?3) WHERE id=?1",
            params![request.attempt_id,cost,string(&json!({"billing_receipt_hash":hash,"billing_reconciled_at":now()}))])?;
        tx.commit()?;
        Ok(())
    }
}
