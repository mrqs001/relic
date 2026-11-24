//! Rebuild an acquired snapshot offline; retain a self-contained candidate.
use relic_app::{model::required, osm};
use serde_json::Value;
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
        return Err("Usage: rebuild_osm ACQUIRED_DIRECTORY NEW_DIRECTORY".into());
    }
    let source = Path::new(&args[0]).to_owned();
    let destination = Path::new(&args[1]).to_owned();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            flag.store(true, Ordering::Release);
        }
    });
    let report = tokio::task::spawn_blocking(move || -> relic_app::error::Result<Value> {
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(source.join("manifest.json"))?)?;
        let staging = tempfile::tempdir_in(destination.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")))?;
        let candidate = staging.path().join("candidate");
        if destination.exists() {
            return Err(relic_app::error::Error::conflict("Destination exists"));
        }
        let report = osm::build(
            &source.join("source.osm.pbf"),
            &candidate,
            manifest["selection"].clone(),
            &cancel,
        )?;
        if report["raw"] != manifest["raw"] {
            return Err(relic_app::error::Error::invalid(
                "Retained source checksum changed",
            ));
        }
        for (name, expected) in [
            ("source.osm.pbf", &manifest["raw"]),
            ("index.json", &manifest["selection"]["index"]),
            ("source.md5", &manifest["selection"]["checksum"]),
        ] {
            let (hash, bytes) = osm::file_hash(&source.join(name), &cancel)?;
            if hash != required(expected, "sha256")? || Some(bytes) != expected["bytes"].as_u64() {
                return Err(relic_app::error::Error::invalid(
                    "Retained asset checksum changed",
                ));
            }
            std::fs::copy(source.join(name), candidate.join(name))?;
            std::fs::File::open(candidate.join(name))?.sync_all()?;
        }
        std::fs::File::open(&candidate)?.sync_all()?;
        if cancel.load(Ordering::Acquire) {
            return Err(relic_app::error::Error::cancelled());
        }
        std::fs::rename(candidate, &destination)?;
        std::fs::File::open(destination.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")))?.sync_all()?;
        Ok(report)
    })
    .await
    .map_err(|_| "Import worker failed")?
    .map_err(|e| e.message)?;
    println!(
        "{}",
        serde_json::json!({"objects_scanned":report["objects_scanned"],"selected_features":report["selected_features"],"geometry_status":report["geometry_status"],"activated":false})
    );
    Ok(())
}
