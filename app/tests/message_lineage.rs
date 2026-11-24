use relic_app::{
    config::Config,
    model::ident,
    processing_ai::validate_output,
    processing_geo::GeoResolver,
    processing_store::{self, Prepared},
    store::Store,
};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};

fn workspace() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let s = Store::open(Config::local(
        root,
        tmp.path().join("data"),
        "127.0.0.1:8794".parse().unwrap(),
    ))
    .unwrap();
    (tmp, s)
}
fn row(id: &str, revision: &str, text: &str) -> Value {
    json!({"id":id,"revision":revision,"source":"fiction:alpha","published_at":"2026-09-10T12:00:00Z","acquired_at":"2026-09-10T12:01:00Z","text":text,"geometry":null,"location_precision":"unresolved","origin_group":null,"dependence":"unknown","language":"en","fictional":true,"availability":"available","citation":{"evidence_id":id,"revision":revision,"start_character":0,"end_character":text.chars().count()}})
}
fn import(s: &Store, rows: Vec<Value>) {
    s.import_job(json!({"schema_version":1,"fictional":true,"title":"Fictional lineage test","sources":[{"id":"fiction:alpha","label":"Fictional Alpha","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
}
fn candidate(s: &Store, id: &str, revision: &str) -> Prepared {
    let e = s.detail(id, revision).unwrap();
    let text = e["text"].as_str().unwrap();
    let output = json!({"language":"en","translation":null,"headline":"Fictional source reports on London","kind":"event","claim_status":"reported","topic":"infrastructure","claims":[{"text":"Fictional claim","quote":text,"state":"reported"}],"locations":[{"mention":"London","quote":text,"role":"event","canonical_name":"London","country_code":null}]});
    let checked = validate_output(text, &output).unwrap();
    let resolver = GeoResolver::load(&s.config.root).unwrap();
    let rowid = s
        .connect()
        .unwrap()
        .query_row(
            "SELECT rowid FROM evidence WHERE id=?1 AND revision=?2",
            [id, revision],
            |r| r.get(0),
        )
        .unwrap();
    processing_store::prepare(rowid, &e, &checked, &resolver, "fixture/gold").unwrap()
}
fn kinds(v: &Value) -> Vec<(String, String)> {
    v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            (
                n["kind"].as_str().unwrap().to_string(),
                n["id"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}
fn has_edge(v: &Value, source: &str, target: &str, relationship: &str) -> bool {
    v["edges"].as_array().unwrap().iter().any(|e| {
        e["source"] == source && e["target"] == target && e["relationship"] == relationship
    })
}

#[test]
fn message_lineage_follows_one_message_without_expanding_shared_steps() {
    let (_tmp, s) = workspace();
    let texts = [
        "London crews reported that 2 roads were blocked.",
        "London officials reported a closed bridge.",
        "London reports a power cut in two districts.",
    ];
    import(
        &s,
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| row(&format!("fiction:{i}"), "v1", t))
            .collect(),
    );
    let mut edited = row("fiction:0", "v2", "London crews reported that 3 roads were blocked.");
    edited["supersedes"] = json!({"id":"fiction:0","revision":"v1"});
    import(&s, vec![edited]);
    // One processing run reads three messages: a shared step for this message.
    let prepared = vec![
        candidate(&s, "fiction:0", "v2"),
        candidate(&s, "fiction:1", "v1"),
        candidate(&s, "fiction:2", "v1"),
    ];
    s.publish_processing(prepared, &ident("processing"), &AtomicBool::new(false))
        .unwrap()
        .unwrap();

    let v = s.message_lineage("fiction:0@v2").unwrap();
    assert_eq!(v["root"], "fiction:0@v2");
    assert_eq!(v["hidden_nodes"], 0);
    let nodes = kinds(&v);
    let has = |kind: &str| nodes.iter().any(|(k, _)| k == kind);
    for kind in ["evidence_revision", "source_instance", "run_step", "interpretation", "publication"] {
        assert!(has(kind), "missing {kind} in {nodes:?}");
    }
    // The exact version chain is present in both directions.
    assert!(has_edge(&v, "fiction:0@v1", "fiction:0@v2", "superseded_by"));
    assert!(has_edge(&v, "fiction:alpha", "fiction:0@v2", "acquired_from"));

    // Only this revision's reading appears; the shared run's other outputs do not.
    let readings = v["readings"].as_array().unwrap();
    assert_eq!(readings.len(), 1, "{readings:?}");
    let reading = &readings[0];
    assert_eq!(reading["evidence_id"], "fiction:0");
    assert_eq!(reading["revision"], "v2");
    assert_eq!(reading["claims"], 1);
    assert!(reading["created_at"].is_string());
    assert!(reading["model"].is_string());
    let reading_id = reading["id"].as_str().unwrap();
    assert!(has_edge(&v, "fiction:0@v2", reading_id, "supports"));
    let interpretations: Vec<_> = nodes.iter().filter(|(k, _)| k == "interpretation").collect();
    assert_eq!(interpretations.len(), 1, "{interpretations:?}");
    for (_, id) in &nodes {
        assert!(
            !id.starts_with("fiction:1@") && !id.starts_with("fiction:2@"),
            "expanded a shared step into another message: {id}"
        );
    }
    // Every returned edge has both endpoints present.
    let ids: Vec<&str> = nodes.iter().map(|(_, id)| id.as_str()).collect();
    for e in v["edges"].as_array().unwrap() {
        assert!(ids.contains(&e["source"].as_str().unwrap()));
        assert!(ids.contains(&e["target"].as_str().unwrap()));
    }
    // An unread revision still reaches the publication of the run that retained it.
    let unread = s.message_lineage("fiction:0@v1").unwrap();
    assert!(unread["readings"].as_array().unwrap().is_empty());
    assert!(unread["edges"].as_array().unwrap().iter().any(|e| {
        e["relationship"] == "published" && unread["nodes"].as_array().unwrap().iter()
            .any(|n| n["id"] == e["source"] && n["kind"] == "run_step")
    }));
    assert!(has_edge(&unread, "fiction:0@v1", "fiction:0@v2", "superseded_by"));
}

#[test]
fn message_lineage_rejects_missing_and_non_message_roots() {
    let (_tmp, s) = workspace();
    import(&s, vec![row("fiction:x", "v1", "London reports a road closure.")]);
    assert!(s.message_lineage("fiction:x@v9").is_err());
    let err = s.message_lineage("fiction:alpha").unwrap_err();
    assert!(err.message.contains("exact message revision"), "{}", err.message);
}
