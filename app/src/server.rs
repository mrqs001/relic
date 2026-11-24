//! The Relic server process: configuration, storage, collection and HTTP.
use crate::{api, collection::Runtime, config::Config, error::Result, sources::Sources, store::Store};

/// Runs Relic until Ctrl-C or SIGTERM with the given source adapters, then
/// lets model requests already sent finish (up to two minutes).
pub async fn run(sources: Sources) -> Result<()> {
    let config = Config::load()?;
    let address = config.address;
    // Store::open holds the data directory for this process until it exits.
    let store = Store::open(config)?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    let runtime = Runtime::with_sources(store.clone(), &sources);
    runtime.start();
    println!("Relic local workspace: http://{address} (only connected, activated sources collect)");
    let shutdown = store.clone();
    let drain = store.clone();
    axum::serve(listener, api::router_with_sources(store, runtime, sources))
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                if let Ok(mut terminate) =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                {
                    tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
                } else {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
            shutdown.begin_drain();
        })
        .await?;
    // Paid requests already sent finish and are recorded, within a bound.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut announced = false;
    loop {
        let open = drain.in_flight_model_requests().unwrap_or(0);
        if (open == 0 && drain.processing_jobs_running() == 0) || tokio::time::Instant::now() >= deadline {
            break;
        }
        if !announced {
            println!("Finishing {open} model requests already sent (up to 2 minutes). Press Ctrl-C again to stop now; interrupted requests are recorded with unknown cost.");
            announced = true;
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
        }
    }
    drain.begin_shutdown();
    // Let cancelled requests record their outcome before the runtime stops.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    Ok(())
}
