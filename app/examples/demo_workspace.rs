//! Builds the fictional demo workspace used for the README: invented reports
//! from `examples/demo-workspace.json`, read with authored outputs that pass
//! the same validation and geography as model output. No network, no model.
//!
//! Usage: cargo run --release --example demo_workspace -- data/demo
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
    let destination = PathBuf::from(std::env::args().nth(1).expect("Pass a new data directory, for example data/demo"));
    assert!(!destination.exists(), "Use a new, empty directory");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_owned();
    let s = Store::open(Config::local(root.clone(), destination, "127.0.0.1:8796".parse()?)).map_err(|e| e.message)?;
    let fixture: Value = serde_json::from_str(include_str!("../../examples/demo-workspace.json"))?;
    let cases = fixture["cases"].as_array().unwrap();
    let rows: Vec<Value> = cases
        .iter()
        .map(|c| {
            let id = format!("demo:{}", c["id"].as_str().unwrap());
            let text = c["original"].as_str().unwrap();
            json!({"id":id,"revision":"v1","source":c["source"],"published_at":c["published_at"],"acquired_at":c["published_at"],
                "text":text,"geometry":null,"location_precision":"unresolved","origin_group":null,"dependence":"unknown","language":"und",
                "fictional":true,"availability":"available",
                "citation":{"evidence_id":id,"revision":"v1","start_character":0,"end_character":text.chars().count()}})
        })
        .collect();
    s.import_job(
        json!({"schema_version":1,"fictional":true,"title":"Fictional demo workspace","sources":fixture["sources"],"evidence_revisions":rows}),
        ident("import"),
        Arc::new(AtomicBool::new(false)),
    )
    .map_err(|e| e.message)?;
    let resolver = GeoResolver::load(&root).map_err(|e| e.message)?;
    let mut candidates = vec![];
    for c in cases {
        let id = format!("demo:{}", c["id"].as_str().unwrap());
        let e = s.detail(&id, "v1").map_err(|e| e.message)?;
        let rowid = s.connect().map_err(|e| e.message)?.query_row("SELECT rowid FROM evidence WHERE id=?1", [&id], |r| r.get(0))?;
        let checked = validate_output(e["text"].as_str().unwrap(), &c["gold"]).map_err(|e| format!("{id}: {e}"))?;
        let mut candidate = processing_store::prepare(rowid, &e, &checked, &resolver, "fixture/authored-gold").map_err(|e| e.message)?;
        candidate.audit = json!({"fixture":true,"model_called":false,"authored_output":value(&checked.output)});
        candidates.push(candidate);
    }
    let run = ident("processing");
    let count = candidates.len();
    s.publish_processing(candidates, &run, &AtomicBool::new(false)).map_err(|e| e.message)?;
    s.save_job(&json!({"id":run,"kind":"processing","status":"completed","accepted":count,"total":count,"created_at":now(),
        "detail":"Fictional demo: authored readings passed exact support and geographic validation. No model or source was contacted."}))
        .map_err(|e| e.message)?;
    println!("Fictional demo workspace ready: {count} messages; no network requests.");
    Ok(())
}
