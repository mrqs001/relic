//! Explicit fictional UI fixture using authored gold outputs, with no network.
use relic_app::{
    config::Config,
    model::{ident, now, value},
    processing_ai::validate_output,
    processing_geo::GeoResolver,
    processing_store,
    store::Store,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let destination = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("Pass a new temporary data directory"),
    );
    assert!(!destination.exists(), "Use an empty fixture directory");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let s = Store::open(Config::local(
        root.clone(),
        destination,
        "127.0.0.1:8796".parse()?,
    ))
    .map_err(|e| e.message)?;
    let fixture: Value = serde_json::from_str(include_str!("../../examples/processing-ai.json"))?;
    let mut cases = fixture["cases"].as_array().unwrap().clone();
    if let Some(mut copied) = cases
        .iter()
        .find(|c| c["id"].as_str().is_some_and(|id| id.contains("persian")))
        .cloned()
    {
        copied["id"] = json!("copied-persian-report");
        cases.push(copied);
    }
    let rows:Vec<Value>=cases.iter().enumerate().map(|(n,c)|{let id=format!("fiction:{}",c["id"].as_str().unwrap());let text=c["original"].as_str().unwrap();json!({"id":id,"revision":"v1","source":if n%2==0{"fiction:wire"}else{"fiction:local"},"published_at":format!("2026-09-11T10:{n:02}:00Z"),"acquired_at":format!("2026-09-11T10:{n:02}:30Z"),"text":text,"geometry":null,"location_precision":"unresolved","origin_group":null,"dependence":"unknown","language":"und","fictional":true,"availability":"available","citation":{"evidence_id":id,"revision":"v1","start_character":0,"end_character":text.chars().count()}})}).collect();
    s.import_job(json!({"schema_version":1,"fictional":true,"title":"Fictional multilingual processing review","sources":[{"id":"fiction:wire","label":"Fictional World Wire","family":"authored_report"},{"id":"fiction:local","label":"Fictional Local Bulletin","family":"authored_report"}],"evidence_revisions":rows}),ident("import"),Arc::new(AtomicBool::new(false))).map_err(|e|e.message)?;
    let resolver = GeoResolver::load(&root).map_err(|e| e.message)?;
    let mut candidates = vec![];
    for c in &cases {
        let id = format!("fiction:{}", c["id"].as_str().unwrap());
        let e = s.detail(&id, "v1").map_err(|e| e.message)?;
        let rowid = s.connect().map_err(|e| e.message)?.query_row(
            "SELECT rowid FROM evidence WHERE id=?1",
            [&id],
            |r| r.get(0),
        )?;
        let output = c
            .get("gold")
            .or_else(|| c.get("output"))
            .expect("Fixture gold output");
        let checked = validate_output(e["text"].as_str().unwrap(), output)?;
        let mut candidate =
            processing_store::prepare(rowid, &e, &checked, &resolver, "fixture/authored-gold")
                .map_err(|e| e.message)?;
        candidate.audit =
            json!({"fixture":true,"model_called":false,"authored_output":value(&checked.output)});
        candidates.push(candidate);
    }
    let run = ident("processing");
    let count = candidates.len();
    s.publish_processing(candidates, &run, &AtomicBool::new(false))
        .map_err(|e| e.message)?;
    s.save_job(&json!({"id":run,"kind":"processing","status":"completed","accepted":count,"total":count,"created_at":now(),"detail":"Fictional interface fixture: authored gold outputs passed exact support and geographic validation. No model or source was contacted."})).map_err(|e|e.message)?;
    println!("Fictional processed-reading fixture ready: {count} messages; no network requests.");
    Ok(())
}
