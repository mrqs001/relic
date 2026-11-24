//! Offline only: install a passed ship finder evaluation record into a
//! stopped workspace: `--record <evaluation.json>` plus the workspace's usual
//! arguments (`--data-dir`, `--env`). Relic holds an exclusive lock on its
//! data directory, so this refuses while that workspace runs. A failed record,
//! or one for another model profile, case set or thresholds, is refused. No
//! model call is made.
use relic_app::{config::Config, store::Store};
use serde_json::Value;

fn main() -> Result<(), String> {
    let mut rest = vec![];
    let mut path = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--record" {
            path = Some(args.next().ok_or("--record needs a file")?);
        } else {
            rest.push(arg);
        }
    }
    let path = path.ok_or("Use --record <evaluation.json> with the workspace's --data-dir or --env")?;
    if std::fs::metadata(&path).map_err(|_| "Evaluation record unavailable")?.len() > 4 * 1024 * 1024 {
        return Err("Evaluation record exceeds the size limit".into());
    }
    let record: Value = serde_json::from_slice(&std::fs::read(&path).map_err(|_| "Evaluation record unavailable")?).map_err(|_| "Invalid evaluation record")?;
    let store = Store::open(Config::from_args(rest).map_err(|e| e.message)?).map_err(|e| e.message)?;
    store.record_vessel_evaluation(&record).map_err(|e| e.message)?;
    println!(
        "Ship finder evaluation installed in {} for profile {} ({} cases, checked {}); no model calls made",
        store.config.data.display(),
        record["fingerprint_sha256"].as_str().unwrap_or_default(),
        record["cases"],
        record["checked_at"].as_str().unwrap_or_default()
    );
    Ok(())
}
