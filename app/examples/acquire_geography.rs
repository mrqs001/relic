//! Explicit bounded reference job; no runtime startup, source or model calls.
use relic_app::reference_acquisition;
use serde_json::json;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[tokio::main]
async fn main() -> Result<(), String> {
    let a = std::env::args().collect::<Vec<_>>();
    if a.len() < 3 {
        return Err("Usage: acquire_geography NEW_PARENT PRODUCT [PRODUCT…]".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal.store(true, Ordering::Release);
    });
    for product in &a[2..] {
        let m = reference_acquisition::acquire(
            product,
            &Path::new(&a[1]).join(product),
            cancel.clone(),
        )
        .await
        .map_err(|e| e.message)?;
        println!(
            "{}",
            json!({"product":product,"places":m["place_count"],"aliases":m["indexed_aliases"],"status":m["status"]})
        );
    }
    Ok(())
}
