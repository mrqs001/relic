#[tokio::main]
async fn main() {
    // The public build registers no provider adapters; Telegram is built in.
    if let Err(e) = relic_app::server::run(relic_app::sources::Sources::default()).await {
        eprintln!("Relic could not start: {}", e.message);
        std::process::exit(1)
    }
}
