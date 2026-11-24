//! Offline re-index of an explicit retained release; never activates it.
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[tokio::main]
async fn main() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        return Err("Usage: rebuild_geography ACQUIRED_DIRECTORY NEW_DIRECTORY".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            flag.store(true, Ordering::Release);
        }
    });
    let report = tokio::task::spawn_blocking(move || {
        relic_app::geonames::rebuild(Path::new(&args[0]), Path::new(&args[1]), &cancel)
    })
    .await
    .map_err(|_| "Reference worker failed")?
    .map_err(|e| e.message)?;
    println!(
        "{}",
        serde_json::json!({"product":report["acquisition"]["product"],"places":report["place_count"],"possible_names":report["possible_name_count"],"index_bytes":report["assets"]["index.sqlite3"]["bytes"],"activated":false})
    );
    Ok(())
}
