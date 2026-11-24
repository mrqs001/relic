//! Reproducible fictional dense-site query measurement; no environment or network.
use relic_app::{
    config::Config,
    model::{Query, ident},
    processing_ai::validate_output,
    processing_geo::GeoResolver,
    processing_store::prepare,
    store::Store,
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let reuse = std::env::args().nth(1).map(std::path::PathBuf::from);
    if let Some(data) = &reuse {
        assert!(
            data.join("relic.sqlite3").is_file(),
            "Missing fictional workspace"
        );
    }
    let store = Store::open(Config::local(
        root.clone(),
        reuse.clone().unwrap_or_else(|| temp.path().join("data")),
        "127.0.0.1:8796".parse()?,
    ))
    .map_err(|e| e.message)?;
    if reuse.is_none() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../examples/processing-multisite.json"))?;
        let case = &fixture["cases"][0];
        let text = case["original"].as_str().unwrap();
        let checked = validate_output(text, &case["gold"])?;
        let resolver = GeoResolver::load(&root).map_err(|e| e.message)?;
        let mut prepared = vec![];
        for batch in 0..20 {
            eprintln!("Seeding fictional batch {batch}/20");
            let rows = (0..500).map(|i| {
            let id = format!("fiction:load-{}",batch*500+i);
            json!({"id":id,"revision":"v1","source":"fiction:load","text":text,"published_at":"2026-09-12T12:00:00Z","acquired_at":"2026-09-12T12:01:00Z","geometry":null,"location_precision":"unresolved","origin_group":null,"language":"en","fictional":true,"availability":"available","citation":{"evidence_id":id,"revision":"v1","start_character":0,"end_character":text.chars().count()}})
        }).collect::<Vec<_>>();
            store.import_job(json!({"schema_version":1,"fictional":true,"title":"Fictional multi-site load","sources":[{"id":"fiction:load","label":"Fictional load","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).map_err(|e|e.message)?;
            for row in rows {
                let e = store
                    .detail(row["id"].as_str().unwrap(), "v1")
                    .map_err(|e| e.message)?;
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
                if prepared.len() == relic_app::processing_store::BATCH {
                    store
                        .publish_processing(
                            std::mem::take(&mut prepared),
                            &ident("processing"),
                            &AtomicBool::new(false),
                        )
                        .map_err(|e| e.message)?;
                }
            }
        }
        assert!(prepared.is_empty());
    }
    let (total,fictional):(i64,i64) = store.connect().map_err(|e|e.message)?.query_row("SELECT count(*),sum(source='fiction:load' AND json_extract(json,'$.fictional')=1) FROM evidence",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    assert_eq!(
        (total, fictional),
        (10000, 10000),
        "Only the owned fictional load seed is allowed"
    );
    let mut measurements = vec![];
    for (scope, expected_sites, truncated) in [
        (json!({}), 10000, true),
        (json!({"event_type":"fire"}), 10000, false),
        (
            json!({"bbox":[2.0,48.0,3.0,49.0],"include_unlocated":false}),
            10000,
            false,
        ),
    ] {
        eprintln!("Measuring {scope}");
        let mut times = vec![];
        for generation in 0..12 {
            let start = Instant::now();
            let q = store
                .query(Query {
                    scope: scope.clone(),
                    generation,
                    cursor: None,
                    limit: Some(100),
                })
                .map_err(|e| e.message)?;
            times.push(start.elapsed().as_secs_f64() * 1000.);
            eprintln!("Query {generation}: {:.0} ms", times.last().unwrap());
            assert_eq!(q["total"], 10000);
            assert_eq!(q["located"], 10000);
            assert_eq!(q["map_loaded"], expected_sites);
            assert_eq!(q["map_truncated"], truncated);
            assert_eq!(
                q["items"][0]["map_locations"].as_array().unwrap().len(),
                if truncated { 2 } else { 1 }
            );
            assert_eq!(
                q["timeline"]["buckets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|b| b["count"].as_u64().unwrap())
                    .sum::<u64>(),
                10000
            );
        }
        times.sort_by(f64::total_cmp);
        measurements.push(json!({"scope":scope,"queries":12,"p50_ms":times[6],"p95_ms":times[11],"sites":expected_sites,"map_truncated":truncated}));
    }
    println!(
        "{}",
        json!({"fictional":true,"messages":10000,"supported_sites":20000,"provider_requests":0,"model_requests":0,"measurements":measurements})
    );
    Ok(())
}
