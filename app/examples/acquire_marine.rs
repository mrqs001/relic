//! Bounded official acquisition. This does not activate a release or use secrets.
#[tokio::main]
async fn main() -> Result<(), String> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() != 2 {
        return Err("Usage: acquire_marine NEW_DESTINATION".into());
    }
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        stop.store(true, std::sync::atomic::Ordering::Release);
    });
    let report = relic_app::marine::acquire(std::path::Path::new(&args[1]), &cancel)
        .await
        .map_err(|e| e.message)?;
    println!(
        "{}",
        serde_json::json!({"product":report["product"],"status":report["status"]})
    );
    Ok(())
}
