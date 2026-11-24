//! Offline maintenance: replay already-paid failed responses using the current
//! validator. No model, source or receipt requests. Stop the app before running;
//! Store::open performs startup recovery. Pass explicit --root/--env/--data-dir.
use relic_app::{config::Config, processing_store::BATCH, store::Store};
use serde_json::json;
use std::sync::atomic::AtomicBool;

fn main() -> Result<(), String> {
    let config = Config::load().map_err(|e| e.message)?;
    if !config.db_path().is_file() {
        return Err("An existing workspace is required".into());
    }
    let store = Store::open(config).map_err(|e| e.message)?;
    let mut examined = 0;
    let mut recovered = 0;
    loop {
        let result = store
            .recover_processing(&AtomicBool::new(false))
            .map_err(|e| e.message)?;
        let batch = result["examined"].as_u64().ok_or("Missing replay count")?;
        examined += batch;
        recovered += result["recovered"]
            .as_u64()
            .ok_or("Missing recovery count")?;
        if batch < BATCH as u64 {
            break;
        }
    }
    println!(
        "{}",
        json!({"examined":examined,"recovered":recovered,"still_rejected":examined-recovered,"model_calls":0})
    );
    Ok(())
}
