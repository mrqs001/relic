//! Offline fictional messages against explicitly retained public reference editions.
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
fn main() -> Result<(), String> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let temp = tempfile::Builder::new()
        .prefix("area-load-")
        .tempdir_in(root.join("data"))
        .map_err(|e| e.to_string())?;
    let reuse = std::env::args().nth(1).map(std::path::PathBuf::from);
    let store = Store::open(Config::local(
        root.clone(),
        reuse
            .clone()
            .unwrap_or_else(|| temp.path().join("workspace")),
        "127.0.0.1:8796".parse().unwrap(),
    ))
    .map_err(|e| e.message)?;
    let started = Instant::now();
    let active = if reuse.is_some() {
        store
            .boundaries()
            .map_err(|e| e.message)?
            .ok_or("No area edition")?
    } else {
        let paths = std::fs::read_dir(root.join("reference/candidates/boundaries-20260920"))
            .map_err(|e| e.to_string())?
            .filter_map(|d| d.ok())
            .filter(|d| d.file_name() != "IRN-ADM1")
            .map(|d| d.path().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        store
            .activate_boundaries(&paths, &AtomicBool::new(false))
            .map_err(|e| e.message)?
    };
    let activation_ms = started.elapsed().as_millis();
    if reuse.is_none() {
        let resolver = GeoResolver::load_for_store(&store).map_err(|e| e.message)?;
        let fixture: Value =
            serde_json::from_str(include_str!("../../examples/processing-areas.json")).unwrap();
        let mut case = fixture["cases"][0].clone();
        // Replace all exact supports together in the fictional author-provided gold.
        case = serde_json::from_str(
            &case
                .to_string()
                .replace("Lebanon", "Saudi Arabia")
                .replace("\"LB\"", "\"SA\""),
        )
        .unwrap();
        let text = case["original"].as_str().unwrap();
        let checked = validate_output(text, &case["gold"]).map_err(|e| e.to_string())?;
        for batch in 0..20 {
            let rows=(0..500).map(|i|{let id=format!("fiction:area-load-{}",batch*500+i);json!({"id":id,"revision":"v1","source":"fiction:area-load","text":text,"published_at":"2026-09-20T12:00:00Z","acquired_at":"2026-09-20T12:01:00Z","geometry":null,"location_precision":"unresolved","language":"en","origin_group":null,"fictional":true,"availability":"available","citation":{"evidence_id":id,"revision":"v1","start_character":0,"end_character":text.chars().count()}})}).collect::<Vec<_>>();
            store.import_job(json!({"schema_version":1,"fictional":true,"sources":[{"id":"fiction:area-load","label":"Fictional Area Load","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).map_err(|e|e.message)?;
            let mut prepared = vec![];
            for row in rows {
                let e = store
                    .detail(row["id"].as_str().unwrap(), "v1")
                    .map_err(|e| e.message)?;
                prepared.push(
                    prepare(
                        e["message_key"].as_str().unwrap().parse().unwrap(),
                        &e,
                        &checked,
                        &resolver,
                        "authored/gold",
                    )
                    .map_err(|e| e.message)?,
                );
                if prepared.len() == 50 {
                    store
                        .publish_processing(
                            std::mem::take(&mut prepared),
                            &ident("load"),
                            &AtomicBool::new(false),
                        )
                        .map_err(|e| e.message)?;
                }
            }
            eprintln!("Seeded {} fictional messages", (batch + 1) * 500);
        }
    }
    let mut measurements = vec![];
    for bounds in [json!(null), json!([38., 20., 48., 28.])] {
        let mut samples = vec![];
        for _ in 0..8 {
            let t = Instant::now();
            let result = store
                .query(Query {
                    scope: json!({"bbox":bounds,"include_unlocated":false}),
                    generation: 1,
                    cursor: None,
                    limit: Some(100),
                })
                .map_err(|e| e.message)?;
            samples.push(t.elapsed().as_secs_f64() * 1000.);
            assert!(
                result["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|i| i["fictional"] == true && i["source"] == "fiction:area-load")
            );
            assert_eq!(result["total"], 10000);
            assert_eq!(result["located"], 10000);
            assert_eq!(result["map_loaded"], 0);
            assert_eq!(result["map_areas"]["features"].as_array().unwrap().len(), 1);
            assert_eq!(
                result["map_areas"]["features"][0]["properties"]["choices"]
                    .as_array()
                    .unwrap()
                    .len(),
                100
            );
            let selected = store
                .area_selection(
                    &json!({"scope":result["scope"],"id":"fiction:area-load-0","revision":"v1"}),
                )
                .map_err(|e| e.message)?;
            assert_eq!(selected["ids"].as_array().unwrap().len(), 1);
        }
        let cold = samples[0];
        samples.sort_by(f64::total_cmp);
        measurements
            .push(json!({"bbox":bounds,"cold_ms":cold,"p95_ms":samples[7],"samples_ms":samples}));
    }
    println!(
        "{}",
        json!({"fictional_messages":10000,"stored_area_shapes":active["features"],"analytical_vertices_in_shown_area":78481,"activation_ms":activation_ms,"measurements":measurements,"model_calls":0,"provider_calls":0,"selection_beyond_first_100":true})
    );
    Ok(())
}
