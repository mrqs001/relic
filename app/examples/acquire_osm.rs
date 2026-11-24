//! Explicit region and new candidate path; does not activate or open a workspace.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[tokio::main]
async fn main() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        return Err("Usage: acquire_osm REGION NEW_DIRECTORY".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            flag.store(true, Ordering::Release);
        }
    });
    let report =
        relic_app::osm_acquisition::acquire(&args[0], std::path::Path::new(&args[1]), cancel)
            .await
            .map_err(|e| e.message)?;
    println!(
        "{}",
        serde_json::json!({"region":args[0],"objects_scanned":report["objects_scanned"],"selected_features":report["selected_features"],"geometry_status":report["geometry_status"],"bytes":report["raw"]["bytes"],"activated":false})
    );
    Ok(())
}
