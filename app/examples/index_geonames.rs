//! Build/inspect a GeoNames candidate from retained, verified publisher members.
//! No model or social-provider calls. Existing editions cannot be overwritten.
use relic_app::geonames::{self, Index};
use serde_json::json;
use std::{path::Path, sync::atomic::AtomicBool};
fn main() -> Result<(), String> {
    let a = std::env::args().collect::<Vec<_>>();
    if a.len() != 4 {
        return Err(
            "Usage: index_geonames PLACES.txt alternateNamesV2.txt NEW_EDITION_DIRECTORY".into(),
        );
    }
    let m=geonames::build(Path::new(&a[3]),Path::new(&a[1]),Path::new(&a[2]),json!({"method":"explicit retained publisher members; acquisition receipt must be retained separately","model_calls":0}),&AtomicBool::new(false)).map_err(|e|e.message)?;
    Index::inspect(Path::new(&a[3])).map_err(|e| e.message)?;
    println!(
        "{}",
        json!({"status":m["status"],"places":m["place_count"],"aliases":m["indexed_aliases"],"unjoined_aliases":m["unjoined_aliases"],"countries":m["countries"]})
    );
    Ok(())
}
