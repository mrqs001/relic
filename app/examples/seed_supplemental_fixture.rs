//! Offline UI fixture using the already-retained fictional check receipt.
//! Arguments: ROOT NEW_DATA_DIRECTORY RECEIPT_DIRECTORY. Never opens live data.
use relic_app::{
    config::Config,
    model::{ident, value},
    processing_store::Policy,
    store::Store,
    supplemental::Work,
    supplemental_ai::Context,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 3 {
        return Err("ROOT NEW_DATA_DIRECTORY RECEIPT_DIRECTORY".into());
    }
    let data = PathBuf::from(&a[1]);
    if data.exists() {
        return Err("Use a new fixture directory".into());
    }
    let request: Value = serde_json::from_slice(&std::fs::read(
        PathBuf::from(&a[2]).join("fire.request.json"),
    )?)?;
    let context: Context = serde_json::from_value(request["state"].clone())?;
    let audit: Value =
        serde_json::from_slice(&std::fs::read(PathBuf::from(&a[2]).join("fire.json"))?)?;
    let text = context.message["text"].as_str().unwrap();
    let mut e = context.message.clone();
    e["acquired_at"] = json!("2026-09-21T10:01:00Z");
    e["published_at"] = json!("2026-09-21T10:00:00Z");
    e["fictional"] = json!(true);
    e["language"] = json!("en");
    e["geometry"] = Value::Null;
    e["location_precision"] = json!("unresolved");
    e["availability"] = json!("available");
    e["citation"] = json!({"evidence_id":e["id"],"revision":e["revision"],"start_character":0,"end_character":text.chars().count()});
    let s = Store::open(Config::local(
        PathBuf::from(&a[0]),
        data,
        "127.0.0.1:8795".parse()?,
    ))
    .map_err(|e| e.message)?;
    s.import_job(json!({"schema_version":1,"fictional":true,"title":"Jev supplementary fixture","sources":[{"id":"fiction:jev","label":"Fictional World Wire","family":"authored_report"}],"evidence_revisions":[e]}),ident("job"),Arc::new(AtomicBool::new(false))).map_err(|e|e.message)?;
    s.set_processing_policy(Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:jev".into()],
    })
    .map_err(|e| e.message)?;
    s.set_supplemental_enabled(true).map_err(|e| e.message)?;
    let rowid = s.connect().map_err(|e| e.message)?.query_row(
        "SELECT rowid FROM evidence WHERE id='fiction:fire'",
        [],
        |r| r.get(0),
    )?;
    let publication = Store::latest(&*s.connect().map_err(|e| e.message)?)
        .map_err(|e| e.message)?
        .unwrap();
    let work = Work {
        rowid,
        publication,
        phase: "initial".into(),
        context,
        request,
    };
    let id = s.reserve_supplemental(&work).map_err(|e| e.message)?;
    s.finish_supplemental(&id, &audit, None)
        .map_err(|e| e.message)?;
    s.set_processing_policy(Policy {
        enabled: false,
        eligible_source_ids: vec!["fiction:jev".into()],
    })
    .map_err(|e| e.message)?;
    println!(
        "{}",
        value(json!({"ready":true,"message_key":rowid,"model_calls":0}))
    );
    Ok(())
}
