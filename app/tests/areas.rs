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
#[test]
fn area_scope_uses_full_geometry_preserves_originals_and_restores() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let tmp = tempfile::Builder::new()
        .prefix("fictional-areas-")
        .tempdir_in(root.join("data"))
        .unwrap();
    let s = Store::open(Config::local(
        root,
        tmp.path().join("workspace"),
        "127.0.0.1:8794".parse().unwrap(),
    ))
    .unwrap();
    let fixture: Value =
        serde_json::from_str(include_str!("../../examples/processing-areas.json")).unwrap();
    let mut paths = vec![];
    for e in fixture["editions"].as_array().unwrap() {
        let level = e["metadata"]["boundaryType"].as_str().unwrap();
        let dir = tmp.path().join(level);
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
    let imported=s.import_job(json!({"schema_version":1,"fictional":true,"sources":[{"id":"fiction:areas","label":"Fictional Area Wire","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
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
    let policy = relic_app::processing_store::Policy {
        enabled: false,
        eligible_source_ids: vec!["fiction:areas".into()],
    };
    s.set_processing_policy(policy.clone()).unwrap();
    assert!(
        s.processing_pending(&policy).unwrap().is_empty(),
        "area edition must not queue fresh semantic calls after publication"
    );
    assert_eq!(s.processing_status(false).unwrap()["needs_upgrade"], 0);
    let all = query(&s, Scope::default());
    let choices: Vec<_> = all["map_areas"]["features"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|f| f["properties"]["choices"].as_array().unwrap())
        .collect();
    let choice = choices
        .iter()
        .find(|c| c["id"] == "fiction:province")
        .unwrap();
    assert_eq!(
        choice["title"],
        "Fictional: fire reported in Hollow Province."
    );
    assert_eq!(choice["published_at"], "2026-09-20T12:00:00.000Z");
    assert_eq!(choice["events"][0]["type"], "fire");
    assert_eq!(choice["events"][0]["state"], "reported");
    assert_eq!(choice["processed"], true);
    assert!(
        all["map_areas"]["features"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["properties"]["country_code"] == "LB")
    );
    assert_eq!(all["total"], 4);
    assert_eq!(all["located"], 2);
    assert_eq!(all["map_loaded"], 0);
    assert_eq!(all["map_areas"]["features"].as_array().unwrap().len(), 2);
    let province = s
        .detail_with_publication("fiction:province", "v1", None)
        .unwrap();
    assert!(province["geometry"].is_null());
    assert_eq!(province["map_areas"].as_array().unwrap().len(), 1);
    assert_eq!(province["text"], fixture["cases"][1]["original"]);
    assert_eq!(
        s.area_selection(&json!({"scope":{},"id":"fiction:province","revision":"v1"}))
            .unwrap()["ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        s.area_selection(
            &json!({"scope":{"event_type":"strike"},"id":"fiction:province","revision":"v1"})
        )
        .unwrap()["ids"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let reference =
        &province["interpretation"]["events"][0]["locations"][0]["candidates"][0]["reference"];
    assert_eq!(reference["record_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(reference["source_sha256"].as_str().unwrap().len(), 64);
    let scope = Scope {
        query: "Hollow Province".into(),
        include_unlocated: false,
        bbox: Some([35.2, 33.2, 35.8, 33.8]),
        ..Scope::default()
    };
    assert_eq!(
        query(&s, scope.clone())["total"],
        0,
        "display fills the hole, but analytical membership must not"
    );
    assert_eq!(
        query(
            &s,
            Scope {
                bbox: Some([34.1, 32.1, 34.5, 32.5]),
                ..scope.clone()
            }
        )["total"],
        1
    );
    assert_eq!(
        query(
            &s,
            Scope {
                event_type: Some("strike".into()),
                bbox: None,
                ..scope
            }
        )["total"],
        0
    );
    assert_eq!(
        query(
            &s,
            Scope {
                publication_id: Some(imported["publication_id"].as_str().unwrap().into()),
                ..Scope::default()
            }
        )["located"],
        0
    );
    fs::write(
        std::path::Path::new(&paths[0]).join("full.geojson"),
        b"corrupt",
    )
    .unwrap();
    assert!(
        s.activate_boundaries(&paths, &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(
        s.boundaries().unwrap().unwrap()["edition"],
        active["edition"]
    );
    let backup = s.backup().unwrap();
    let verified = s.verify_backup(backup["id"].as_str().unwrap()).unwrap();
    let mut config = s.config.clone();
    config.data = verified["restored_directory"].as_str().unwrap().into();
    let restored = Store::open(config).unwrap();
    assert_eq!(
        query(&restored, Scope::default())["map_areas"],
        all["map_areas"]
    );
}
