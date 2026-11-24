//! Offline activation. Stop the selected workspace before using Store::open.
fn main() -> Result<(), String> {
    let path = std::env::var("RELIC_MARINE_EDITION")
        .map_err(|_| "Set RELIC_MARINE_EDITION to the explicit candidate directory")?;
    let store =
        relic_app::store::Store::open(relic_app::config::Config::load().map_err(|e| e.message)?)
            .map_err(|e| e.message)?;
    let result = store
        .activate_marine(
            std::path::Path::new(&path),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .map_err(|e| e.message)?;
    println!("{result}");
    Ok(())
}
