//! Offline only: stop the target workspace before installing a validated gate.
use relic_app::{config::Config, store::Store};
fn main() -> Result<(), String> {
    let path = std::env::var("RELIC_EVALUATION_REPORT")
        .map_err(|_| "Set RELIC_EVALUATION_REPORT to a reviewed evaluation report")?;
    if std::fs::metadata(&path)
        .map_err(|_| "Evaluation report unavailable")?
        .len()
        > 2 * 1024 * 1024
    {
        return Err("Evaluation report exceeds limit".into());
    }
    let report =
        serde_json::from_slice(&std::fs::read(path).map_err(|_| "Evaluation report unavailable")?)
            .map_err(|_| "Invalid evaluation report")?;
    let store = Store::open(Config::load().map_err(|e| e.message)?).map_err(|e| e.message)?;
    store
        .record_processing_evaluation(&report)
        .map_err(|e| e.message)?;
    println!("Validated reading evaluation activated for its explicit baseline/correction phase; no model calls made");
    Ok(())
}
