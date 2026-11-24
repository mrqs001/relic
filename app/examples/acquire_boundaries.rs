//! Explicit official gbOpen ADM0/ADM1 acquisition; no Relic secrets or social data.
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[tokio::main]
async fn main() -> Result<(), String> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() < 3 {
        return Err("Usage: acquire_boundaries NEW_PARENT ISO3 [ISO3…]".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        stop.store(true, Ordering::Release);
    });
    for iso in &args[2..] {
        for tier in ["ADM0", "ADM1"] {
            let m = relic_app::boundary_reference::acquire(
                iso,
                tier,
                &Path::new(&args[1]).join(format!("{iso}-{tier}")),
                cancel.clone(),
            )
            .await
            .map_err(|e| e.message)?;
            println!(
                "{}",
                serde_json::json!({"country":iso,"level":tier,"features":m["features"],"vertices":m["vertices"],"status":m["status"]})
            );
        }
    }
    Ok(())
}
