//! Isolated fictional area UI acceptance; no model or source calls.
use relic_app::{
    config::Config,
    model::{Query, Scope, hash, ident, value},
    processing_ai::validate_output,
    processing_geo::GeoResolver,
    processing_store::prepare,
    store::Store,
};
use serde_json::{Value, json};
use std::{
    fs,
    sync::{Arc, atomic::AtomicBool},
};
fn query(s: &Store, scope: Scope) -> Value {
    s.query(Query {
        scope: value(scope),
        generation: 1,
        cursor: None,
        limit: Some(100),
    })
    .unwrap()
}
fn main() {
    let config = Config::load().unwrap();
    assert!(
        !config.db_path().exists(),
        "Use a new isolated data directory"
    );
    let directory = config.data.clone();
    let s = Store::open(config).unwrap();
    let tmp = directory.join("fictional-boundaries");
    fs::create_dir_all(&tmp).unwrap();
    let fixture: Value =
        serde_json::from_str(include_str!("../../examples/processing-areas.json")).unwrap();
    let mut paths = vec![];
    for e in fixture["editions"].as_array().unwrap() {
        let level = e["metadata"]["boundaryType"].as_str().unwrap();
        let dir = tmp.join(level);
        fs::create_dir(&dir).unwrap();
        let mut assets = json!({});
        for (key, file) in [
            ("metadata", "metadata.json"),
            ("full", "full.geojson"),
            ("display", "display.geojson"),
        ] {
            let bytes = serde_json::to_vec(&e[key]).unwrap();
            fs::write(dir.join(file), &bytes).unwrap();
            assets[file] = json!({"sha256":hash(&bytes),"bytes":bytes.len()});
        }
        fs::write(dir.join("manifest.json"),serde_json::to_vec(&json!({"schema_version":1,"status":"candidate","publisher":"geoBoundaries","collection":"gbOpen","country":"LBN","level":level,"boundary_id":e["metadata"]["boundaryID"],"features":1,"assets":assets,"fictional":true})).unwrap()).unwrap();
        paths.push(dir.to_str().unwrap().to_owned());
    }
    assert!(
        s.activate_boundaries(&paths, &AtomicBool::new(true))
            .is_err()
    );
    assert!(s.boundaries().unwrap().is_none());
    let active = s
        .activate_boundaries(&paths, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(active["features"], 2);
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    assert!(resolver.edition().is_some());
    let rows=fixture["cases"].as_array().unwrap().iter().map(|case|{let id=format!("fiction:{}",case["id"].as_str().unwrap());let text=case["original"].as_str().unwrap();json!({"id":id,"revision":"v1","source":"fiction:areas","text":text,"published_at":"2026-09-20T12:00:00Z","acquired_at":"2026-09-20T12:01:00Z","geometry":null,"location_precision":"unresolved","language":"en","origin_group":null,"fictional":true,"availability":"available","citation":{"evidence_id":id,"revision":"v1","start_character":0,"end_character":text.chars().count()}})}).collect::<Vec<_>>();
    let _imported=s.import_job(json!({"schema_version":1,"fictional":true,"sources":[{"id":"fiction:areas","label":"Fictional Area Wire","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
    let mut prepared = vec![];
    for case in fixture["cases"].as_array().unwrap() {
        let e = s
            .detail(&format!("fiction:{}", case["id"].as_str().unwrap()), "v1")
            .unwrap();
        let checked = validate_output(e["text"].as_str().unwrap(), &case["gold"]).unwrap();
        prepared.push(
            prepare(
                e["message_key"].as_str().unwrap().parse().unwrap(),
                &e,
                &checked,
                &resolver,
                "authored/gold",
            )
            .unwrap(),
        );
    }
    s.publish_processing(prepared, &ident("processing"), &AtomicBool::new(false))
        .unwrap();
    let all = query(&s, Scope::default());
    println!(
        "{}",
        json!({"fictional":true,"messages":all["total"],"located":all["located"],"areas":all["map_areas"]["features"].as_array().unwrap().len(),"model_calls":0})
    );
}
