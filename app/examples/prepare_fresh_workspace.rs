//! Explicit maintenance only: source remains read-only; destination must not exist.
use relic_app::{config::Config, fresh_workspace};
fn main() {
    let result = (|| {
        let config = Config::load()?;
        let source = std::env::var("RELIC_RESET_SOURCE").map_err(|_| {
            relic_app::error::Error::invalid(
                "RELIC_RESET_SOURCE must name the stopped source workspace",
            )
        })?;
        fresh_workspace::prepare(config, std::path::Path::new(&source))
    })();
    match result {
        Ok(report) => println!("{report}"),
        Err(error) => {
            eprintln!("{}", error.message);
            std::process::exit(1);
        }
    }
}
