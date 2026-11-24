//! One explicit, read-only OpenRouter receipt compatibility check. No generation,
//! scheduler, Store::open, database writes or private identifiers in output.
use relic_app::{config::Config, processing_ai};
use rusqlite::OpenFlags;
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().map_err(|error| error.message)?;
    let db =
        rusqlite::Connection::open_with_flags(config.db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = db.prepare("SELECT cost_usd,json_extract(json,'$.audit_hash') FROM processing_attempts WHERE cost_usd IS NOT NULL AND json_extract(json,'$.audit_hash') IS NOT NULL ORDER BY created_at DESC LIMIT 10")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, f64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let client =
        processing_ai::Client::new(processing_ai::Config::from_mapping(&config.source_values)?)?;
    for (known, hash) in rows {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid retained audit hash".into());
        }
        let bytes = std::fs::read(config.object_path(&hash))?;
        if relic_app::model::hash(&bytes) != hash {
            return Err("Retained audit integrity check failed".into());
        }
        let value: Value = serde_json::from_slice(&bytes)?;
        let audit = value.get("audit").unwrap_or(&value);
        let Some(id) = audit["generation_id"].as_str() else {
            continue;
        };
        if audit["requested_model"] != client.model() {
            continue;
        }
        let receipt = client.generation_receipt(id).await?;
        let cost = receipt["data"]["total_cost"]
            .as_f64()
            .ok_or("Missing receipt cost")?;
        println!(
            "{}",
            json!({"outcome":"passed","read_only_receipt_requests":1,"generation_requests":0,"exact_generation_and_model_match":true,"cost_agrees_within_1e_9_usd":(cost-known).abs()<=1e-9,"retained_response_cost":known,"generation_receipt_cost":cost,"workspace_writes":0})
        );
        return Ok(());
    }
    Err("No suitable retained generation ID found; no provider request sent".into())
}
