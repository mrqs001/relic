//! Reproducible offline UI fixture. Requires a new, isolated workspace.
use relic_app::{
    config::Config, model::ident, processing_ai::validate_output, processing_geo::GeoResolver,
    processing_store::prepare, store::Store,
};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().map_err(|e| e.message)?;
    if config.db_path().exists() {
        return Err("Use a new isolated --data-dir; never overwrite an existing workspace".into());
    }
    let resolver = GeoResolver::load(&config.root).map_err(|e| e.message)?;
    let store = Store::open(config).map_err(|e| e.message)?;
    let mut fixture: Value =
        serde_json::from_str(include_str!("../../examples/processing-events.json"))?;
    let taxonomy: Value =
        serde_json::from_str(include_str!("../../examples/processing-taxonomy.json"))?;
    fixture["cases"]
        .as_array_mut()
        .unwrap()
        .extend(taxonomy["cases"].as_array().unwrap().iter().cloned());
    let multisite: Value =
        serde_json::from_str(include_str!("../../examples/processing-multisite.json"))?;
    fixture["cases"]
        .as_array_mut()
        .unwrap()
        .extend(multisite["cases"].as_array().unwrap().iter().cloned());
    let cases = fixture["cases"].as_array().unwrap();
    let rows=cases.iter().map(|case| {
        let id=format!("fiction:{}",case["id"].as_str().unwrap());let text=case["original"].as_str().unwrap();
        json!({"id":id,"revision":"v1","source":"fiction:events","published_at":"2026-09-12T12:00:00Z","acquired_at":"2026-09-12T12:01:00Z","text":text,"geometry":case["source_geometry"],"location_precision":if case["source_geometry"].is_null(){"unresolved"}else{"source_supplied"},"origin_group":null,"dependence":"unknown","language":case["gold"]["language"],"fictional":true,"availability":"available","citation":{"evidence_id":id,"revision":"v1","start_character":0,"end_character":text.chars().count()}})
    }).collect::<Vec<_>>();
    store.import_job(json!({"schema_version":1,"fictional":true,"title":"Fictional event acceptance","sources":[{"id":"fiction:events","label":"Fictional Event Wire","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).map_err(|e| e.message)?;
    let mut prepared = vec![];
    for case in cases {
        let e = store
            .detail(&format!("fiction:{}", case["id"].as_str().unwrap()), "v1")
            .map_err(|e| e.message)?;
        let checked = validate_output(e["text"].as_str().unwrap(), &case["gold"])?;
        prepared.push(
            prepare(
                e["message_key"].as_str().unwrap().parse()?,
                &e,
                &checked,
                &resolver,
                "authored/gold",
            )
            .map_err(|e| e.message)?,
        );
    }
    store
        .publish_processing(prepared, &ident("processing"), &AtomicBool::new(false))
        .map_err(|e| e.message)?;
    println!(
        "{}",
        json!({"fictional":true,"messages":cases.len(),"source_calls":0,"model_calls":0})
    );
    Ok(())
}
