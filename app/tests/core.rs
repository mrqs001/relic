use relic_app::{
    config::Config,
    model::{Query, Scope, ident, value},
    store::Store,
};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};

fn store() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let config = Config::local(
        std::env::current_dir().unwrap(),
        tmp.path().join("workspace"),
        "127.0.0.1:8792".parse().unwrap(),
    );
    let s = Store::open(config).unwrap();
    (tmp, s)
}
fn fixture() -> Value {
    serde_json::from_str(include_str!("../../examples/port-investigation.json")).unwrap()
}
fn import(s: &Store, v: Value) -> relic_app::error::Result<Value> {
    s.import_job(v, ident("job"), Arc::new(AtomicBool::new(false)))
}
fn query(s: &Store, scope: Scope) -> Value {
    s.query(Query {
        scope: value(scope),
        generation: 42,
        cursor: None,
        limit: Some(100),
    })
    .unwrap()
}
fn row(id: &str, revision: &str, text: &str, coords: Option<[f64; 2]>) -> Value {
    json!({"id":id,"revision":revision,"source":"test:source","published_at":"2026-01-01T00:00:00Z","acquired_at":"2026-01-02T00:00:00Z","text":text,"geometry":coords.map(|p|json!({"type":"Point","coordinates":p})),"location_precision":if coords.is_some(){"approximate_locality"}else{"unresolved"},"origin_group":null,"dependence":"unknown","language":"und","fictional":true,"availability":"available","citation":{"evidence_id":id,"revision":revision,"start_character":0,"end_character":text.chars().count()}})
}
fn bundle(rows: Vec<Value>) -> Value {
    json!({"schema_version":1,"fictional":true,"title":"Bounded test evidence","sources":[{"id":"test:source","label":"Fictional source","family":"authored_report"}],"evidence_revisions":rows})
}
fn timeline_count(q: &Value) -> u64 {
    q["timeline"]["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["count"].as_u64().unwrap())
        .sum::<u64>()
        + q["timeline"]["undated"].as_u64().unwrap()
}
#[test]
fn timeline_counts_complete_scope_across_pages_and_time_bases() {
    let (_tmp, s) = store();
    let mut rows = Vec::new();
    for i in 0..125 {
        let mut r = row(
            &format!("time:{i}"),
            "v1",
            "Fictional timeline message",
            Some([10., 20.]),
        );
        r["published_at"] = json!(format!("2026-01-01T00:00:00.{i:03}Z"));
        rows.push(r);
    }
    let mut unknown = row("time:unknown", "v1", "Fictional undated message", None);
    unknown["published_at"] = Value::Null;
    rows.push(unknown);
    let mut fixture = bundle(rows);
    fixture["sources"].as_array_mut().unwrap().push(
        json!({"id":"empty:source","label":"Fictional empty source","family":"authored_report"}),
    );
    let publication = import(&s, fixture).unwrap()["publication_id"].clone();
    let q = query(&s, Scope::default());
    assert_eq!(q["total"], 126);
    assert_eq!(q["loaded"], 100);
    assert_eq!(q["timeline"]["undated"], 1);
    assert_eq!(timeline_count(&q), 126);
    assert!(q["timeline"]["buckets"].as_array().unwrap().len() <= 36);
    let page = s
        .query(Query {
            scope: json!({}),
            generation: 42,
            cursor: Some(q["next_cursor"].as_str().unwrap().into()),
            limit: Some(100),
        })
        .unwrap();
    assert_eq!(page["timeline"], q["timeline"]);
    let range = Scope {
        from: Some("2026-01-01T00:00:00.010Z".into()),
        to: Some("2026-01-01T00:00:00.020Z".into()),
        ..Scope::default()
    };
    let bounded = query(&s, range.clone());
    assert_eq!(bounded["total"], 11);
    assert_eq!(timeline_count(&bounded), 11);
    assert_eq!(bounded["timeline"]["undated"], 0);
    assert_eq!(bounded["map_loaded"], 11);
    // Every bucket can be replayed as its exact interval, including boundary milliseconds.
    for b in bounded["timeline"]["buckets"].as_array().unwrap() {
        let selected = query(
            &s,
            Scope {
                from: Some(b["from"].as_str().unwrap().into()),
                to: Some(b["to"].as_str().unwrap().into()),
                ..Scope::default()
            },
        );
        assert_eq!(selected["total"], b["count"]);
    }
    let received = query(
        &s,
        Scope {
            time_basis: "acquisition_time".into(),
            ..range
        },
    );
    assert_eq!(received["total"], 0);
    assert_eq!(timeline_count(&received), 0);
    let received = query(
        &s,
        Scope {
            time_basis: "acquisition_time".into(),
            ..Scope::default()
        },
    );
    assert_eq!(timeline_count(&received), 126);
    assert_eq!(received["timeline"]["undated"], 0);
    import(
        &s,
        bundle(vec![row(
            "time:later",
            "v1",
            "Fictional later publication",
            None,
        )]),
    )
    .unwrap();
    let frozen = query(
        &s,
        Scope {
            publication_id: Some(publication.as_str().unwrap().into()),
            ..Scope::default()
        },
    );
    assert_eq!(frozen["timeline"], q["timeline"]);
    for scope in [
        Scope {
            source_ids: vec!["empty:source".into()],
            ..Scope::default()
        },
        Scope {
            bbox: Some([90., 0., 100., 5.]),
            include_unlocated: false,
            ..Scope::default()
        },
        Scope {
            query: "no matching text".into(),
            ..Scope::default()
        },
    ] {
        let filtered = query(&s, scope);
        assert_eq!(filtered["total"], 0);
        assert_eq!(timeline_count(&filtered), 0);
    }
}

#[test]
fn timeline_all_undated_one_sided_empty_and_reversed_bounds() {
    let (_tmp, s) = store();
    let mut unknown = row("time:none", "v1", "Fictional undated", None);
    unknown["published_at"] = Value::Null;
    import(&s, bundle(vec![unknown])).unwrap();
    let q = query(&s, Scope::default());
    assert_eq!(q["timeline"]["from"], Value::Null);
    assert_eq!(q["timeline"]["buckets"], json!([]));
    assert_eq!(timeline_count(&q), 1);
    for scope in [
        Scope {
            from: Some("2026-01-01T00:00:00Z".into()),
            ..Scope::default()
        },
        Scope {
            to: Some("2026-01-01T00:00:00Z".into()),
            ..Scope::default()
        },
    ] {
        assert_eq!(timeline_count(&query(&s, scope)), 0);
    }
    assert!(
        s.query(Query {
            scope: json!({"from":"2026-01-02T00:00:00Z","to":"2026-01-01T00:00:00Z"}),
            generation: 42,
            cursor: None,
            limit: Some(1)
        })
        .is_err()
    );
}
#[test]
fn frozen_scope_and_immutable_revision_survive_reimport() {
    let (_tmp, s) = store();
    assert_eq!(s.workspace().unwrap()["empty"], true);
    let p = import(&s, fixture()).unwrap();
    assert_eq!(p["accepted"], 5);
    let repeated = import(&s, fixture()).unwrap();
    assert_eq!(repeated["publication_id"], p["publication_id"]);
    assert_eq!(repeated["accepted"], 0);
    let q = query(&s, Scope::default());
    assert_eq!(
        (
            q["total"].as_u64(),
            q["located"].as_u64(),
            q["unlocated"].as_u64()
        ),
        (Some(4), Some(2), Some(2))
    );
    assert_eq!(q["origin_groups"], 2);
    assert_eq!(q["unknown_origins"], 1);
    let baseline = s.brief("demo:brief-1", Some("v1")).unwrap();
    let scope = Scope::parse(&baseline["scope"]).unwrap();
    let frozen = query(&s, scope);
    assert_eq!(frozen["total"], 3);
    assert!(
        !frozen["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == "demo:report-3")
    );
    assert_eq!(s.detail("demo:report-1", "v1").unwrap()["revision"], "v1");
    let mut bad = fixture();
    bad["evidence_revisions"][0]["text"] = json!("Untruthful replacement");
    bad["evidence_revisions"][0]["citation"]["end_character"] = json!(21);
    assert!(import(&s, bad).is_err());
    assert_eq!(
        query(&s, Scope::default())["publication_id"],
        p["publication_id"]
    );
}
#[test]
fn unicode_scalar_citations_and_search_are_exact() {
    let (_tmp, s) = store();
    let text = "ÉTÉ 🛰️ café — شاهد عربي";
    import(&s, bundle(vec![row("test:unicode", "v1", text, None)])).unwrap();
    let scope = Scope {
        query: "été".into(),
        ..Scope::default()
    };
    assert_eq!(query(&s, scope)["total"], 1);
    assert_eq!(
        query(
            &s,
            Scope {
                query: "شاهد".into(),
                ..Scope::default()
            }
        )["total"],
        1
    );
    let c = s.connect().unwrap();
    let citation = relic_app::model::Citation {
        evidence_id: "test:unicode".into(),
        revision: "v1".into(),
        start_character: 4,
        end_character: 6,
    };
    let support = Store::check_citations(&c, &[citation], None).unwrap();
    assert_eq!(support[0]["quote"], "🛰️");
    let mut invalid = row("test:invalid", "v1", "é", None);
    invalid["citation"]["end_character"] = json!(2);
    assert!(import(&s, bundle(vec![invalid])).is_err());
}
#[test]
fn dateline_scope_counts_and_cursor_remain_coherent() {
    let (_tmp, s) = store();
    import(
        &s,
        bundle(vec![
            row("test:east", "v1", "east", Some([179., 10.])),
            row("test:west", "v1", "west", Some([-179., 10.])),
            row("test:outside", "v1", "outside", Some([0., 10.])),
            row("test:unplaced", "v1", "unplaced", None),
        ]),
    )
    .unwrap();
    let scope = Scope {
        bbox: Some([170., 0., -170., 20.]),
        ..Scope::default()
    };
    let q = query(&s, scope.clone());
    assert_eq!(q["total"], 3);
    assert_eq!(q["map_loaded"], 2);
    let page = s
        .query(Query {
            scope: value(&scope),
            generation: 7,
            cursor: None,
            limit: Some(1),
        })
        .unwrap();
    let cursor = page["next_cursor"].as_str().unwrap().to_string();
    let next = s
        .query(Query {
            scope: value(&scope),
            generation: 8,
            cursor: Some(cursor.clone()),
            limit: Some(1),
        })
        .unwrap();
    assert_ne!(page["items"][0]["id"], next["items"][0]["id"]);
    assert!(
        s.query(Query {
            scope: value(Scope::default()),
            generation: 8,
            cursor: Some(cursor),
            limit: Some(1)
        })
        .is_err()
    );
}
#[test]
fn revisions_cannot_change_source_or_mix_fiction_with_real() {
    let (_tmp, s) = store();
    import(&s, bundle(vec![row("test:evidence", "v1", "one", None)])).unwrap();
    let mut changed = bundle(vec![row("test:evidence", "v2", "two", None)]);
    changed["sources"][0]["id"] = json!("other:source");
    changed["evidence_revisions"][0]["source"] = json!("other:source");
    assert_eq!(import(&s, changed).unwrap_err().code, "conflict");
    let mut real = bundle(vec![row("real:evidence", "v1", "real", None)]);
    real["fictional"] = json!(false);
    real["evidence_revisions"][0]["fictional"] = json!(false);
    assert!(import(&s, real).is_err());
    let mut expired = bundle(vec![row("test:expired", "v1", "expired", None)]);
    expired["evidence_revisions"][0]["availability"] = json!("user-purged");
    assert!(import(&s, expired).is_err());
    assert_eq!(query(&s, Scope::default())["total"], 1);
}
#[test]
fn cancelled_candidate_preserves_last_publication_and_job_state() {
    let (_tmp, s) = store();
    let published = import(&s, fixture()).unwrap();
    let job = ident("job");
    let cancel = Arc::new(AtomicBool::new(true));
    let result = s.import_job(
        bundle(vec![row("test:cancelled", "v1", "never published", None)]),
        job.clone(),
        cancel,
    );
    assert_eq!(result.unwrap_err().code, "cancelled");
    assert_eq!(
        query(&s, Scope::default())["publication_id"],
        published["publication_id"]
    );
    let jobs = s.list("jobs").unwrap();
    assert_eq!(
        jobs["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == job)
            .unwrap()["status"],
        "cancelled"
    );
    assert!(s.detail("test:cancelled", "v1").is_err());
}
#[test]
fn restart_keeps_analyst_work_and_marks_interrupted_jobs_failed() {
    let (_tmp, s) = store();
    import(&s, fixture()).unwrap();
    s.update_investigation(
        "demo:investigation-1",
        json!({"expected_version":1,"notes":"An analyst-owned alternative remains durable."}),
        false,
    )
    .unwrap();
    let job = json!({"id":"job:crash","kind":"import","status":"running","accepted":0,"total":100,"created_at":"2026-01-01T00:00:00Z","detail":"in progress"});
    s.save_job(&job).unwrap();
    let config = s.config.clone();
    drop(s);
    let reopened = Store::open(config).unwrap();
    assert_eq!(
        reopened.workspace().unwrap()["investigations"][0]["notes"],
        "An analyst-owned alternative remains durable."
    );
    assert!(reopened.detail("demo:report-1", "v1").is_ok());
    let jobs = reopened.list("jobs").unwrap();
    assert_eq!(
        jobs["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == "job:crash")
            .unwrap()["status"],
        "failed"
    );
}
#[test]
fn backup_restores_exact_support_and_rejects_corruption() {
    let (_tmp, s) = store();
    import(&s, fixture()).unwrap();
    let backup = s.backup().unwrap();
    let id = backup["id"].as_str().unwrap();
    let verified = s.verify_backup(id).unwrap();
    assert_eq!(verified["verified"], true);
    assert_eq!(verified["briefs"], 1);
    let path = s.config.data.join("backups").join(id).join("relic.sqlite3");
    std::fs::write(path, b"corrupted backup").unwrap();
    assert!(s.verify_backup(id).is_err());
    assert!(s.detail("demo:report-1", "v1").is_ok());
}
#[test]
fn old_source_revision_acquired_late_does_not_replace_correction() {
    let (_tmp, s) = store();
    let mut newer = row("test:item", "v2", "correction", None);
    newer["source_edited_at"] = json!("2026-01-03T00:00:00Z");
    newer["acquired_at"] = json!("2026-01-03T00:01:00Z");
    import(&s, bundle(vec![newer])).unwrap();
    let mut older = row("test:item", "v1", "old body acquired late", None);
    older["acquired_at"] = json!("2026-01-04T00:00:00Z");
    import(&s, bundle(vec![older])).unwrap();
    assert_eq!(query(&s, Scope::default())["items"][0]["revision"], "v2");
    assert!(s.detail("test:item", "v1").is_ok());
}
#[test]
fn unsupported_watch_condition_is_explicitly_rejected() {
    let (_tmp, s) = store();
    import(&s, fixture()).unwrap();
    let error=s.watch_preview(json!({"title":"Unsupported","question":"Find contradictions","scope":Scope::default(),"conditions":["contradiction"]})).unwrap_err();
    assert_eq!(error.code, "invalid_input");
    assert_eq!(s.workspace().unwrap()["watches"], json!([]));
}
#[test]
fn new_material_alert_restores_review_without_rewriting_assessment() {
    let (_tmp, s) = store();
    import(&s, fixture()).unwrap();
    let w=s.create_watch(json!({"title":"Revision review","question":"What changed?","scope":Scope::default(),"conditions":["source correction"],"baseline_policy":"fresh"})).unwrap();
    let mut f = fixture();
    f.as_object_mut().unwrap().remove("baseline_brief");
    let base = f["evidence_revisions"][3].clone();
    let revision = |n: u32| {
        let mut row = base.clone();
        row["revision"] = json!(format!("v{n}"));
        row["citation"]["revision"] = row["revision"].clone();
        row["source_edited_at"] = json!(format!("2026-01-10T10:0{n}:00Z"));
        row["acquired_at"] = row["source_edited_at"].clone();
        row["supersedes"]["revision"] = json!(format!("v{}", n - 1));
        row
    };
    f["evidence_revisions"] = json!([revision(3)]);
    import(&s, f.clone()).unwrap();
    let first = s.list("alerts").unwrap()["items"][0].clone();
    assert_eq!(first["watch_id"], w["id"]);
    s.update_alert(
        first["id"].as_str().unwrap(),
        json!({"review_status":"acknowledged"}),
    )
    .unwrap();
    f["evidence_revisions"] = json!([revision(4)]);
    import(&s, f).unwrap();
    let next = s.list("alerts").unwrap()["items"][0].clone();
    assert_eq!(next["id"], first["id"]);
    assert_eq!(next["review_status"], "new");
    assert_eq!(next["review_history"][0]["review_status"], "acknowledged");
    assert_eq!(next["evidence"].as_array().unwrap().len(), 2);
    assert!(
        s.brief("demo:brief-1", Some("v1")).unwrap()["finding"]
            .as_str()
            .unwrap()
            .contains("One originating account")
    );
}
#[test]
fn active_worker_cancellation_stops_before_publish() {
    let (_tmp, s) = store();
    import(&s, fixture()).unwrap();
    let prior = query(&s, Scope::default())["publication_id"].clone();
    let rows = (0..1000)
        .map(|i| {
            row(
                &format!("test:cancel-{i}"),
                "v1",
                "bounded cancellable import",
                None,
            )
        })
        .collect();
    let job = ident("job");
    let cancel = Arc::new(AtomicBool::new(false));
    s.running
        .lock()
        .unwrap()
        .insert(job.clone(), cancel.clone());
    let worker = s.clone();
    let id = job.clone();
    let t = std::thread::spawn(move || worker.import_job(bundle(rows), id, cancel));
    let ledger_wait = std::time::Instant::now();
    while !s.list("jobs").unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|j| j["id"] == job)
    {
        assert!(ledger_wait.elapsed() < std::time::Duration::from_secs(2));
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let start = std::time::Instant::now();
    let acknowledgement = s.cancel_job(&job).unwrap();
    assert_eq!(acknowledgement["status"], "cancelling");
    assert_eq!(acknowledgement["total"], 1000);
    assert_eq!(t.join().unwrap().unwrap_err().code, "cancelled");
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(query(&s, Scope::default())["publication_id"], prior);
    assert!(s.detail("test:cancel-0", "v1").is_err());
}
#[test]
fn malformed_fixture_interval_rejects_without_poisoning_writer() {
    let (_tmp, s) = store();
    let mut bad = fixture();
    bad["investigation"]["scope"]["time_interval"] = json!([]);
    assert_eq!(import(&s, bad).unwrap_err().code, "invalid_input");
    assert_eq!(s.workspace().unwrap()["empty"], true);
    assert!(import(&s, fixture()).is_ok());
    assert!(s.update_investigation("demo:investigation-1",json!({"expected_version":1,"notes":"Writer is usable after rejected malformed import"}),false).is_ok());
}
#[test]
fn queried_brief_support_survives_restore_and_assessed_claim_is_immutable() {
    let (_tmp, s) = store();
    import(&s, fixture()).unwrap();
    let q = query(
        &s,
        Scope {
            query: "Correction".into(),
            ..Scope::default()
        },
    );
    let c = q["items"][0]["citation"].clone();
    let scope = q["scope"].clone();
    let inv=s.update_investigation("demo:investigation-1",json!({"expected_version":1,"claim_id":"demo:claim-access","status":"contested","rationale":"Exact correction addresses closure","citations":[c],"scope":scope,"publication_id":q["publication_id"]}),true).unwrap();
    let mut claims = inv["claims"].clone();
    claims[0]["text"] = json!("An entirely different proposition");
    assert_eq!(
        s.update_investigation(
            "demo:investigation-1",
            json!({"expected_version":2,"claims":claims}),
            false
        )
        .unwrap_err()
        .code,
        "conflict"
    );
    s.save_brief(json!({"title":"Query-scoped assessment","question":"What changed?","investigation_id":"demo:investigation-1","finding":"Correction describes planned inspection.","assessment":"contested","limitations":[],"citations":[c],"scope":scope,"publication_id":q["publication_id"]})).unwrap();
    let backup = s.backup().unwrap();
    assert_eq!(
        s.verify_backup(backup["id"].as_str().unwrap()).unwrap()["verified"],
        true
    );
}
#[test]
fn copies_attach_to_origin_episode_without_reopening_acknowledged_review() {
    let (_tmp, s) = store();
    import(&s, fixture()).unwrap();
    s.create_watch(json!({"title":"New origins","question":"Any new origin?","scope":Scope::default(),"conditions":["new observed origin"],"baseline_policy":"fresh"})).unwrap();
    let mut evidence = row("test:first", "v1", "An original report", None);
    evidence["origin_group"] = json!("test:origin");
    import(&s, bundle(vec![evidence])).unwrap();
    let first = s.list("alerts").unwrap()["items"][0].clone();
    s.update_alert(
        first["id"].as_str().unwrap(),
        json!({"review_status":"acknowledged"}),
    )
    .unwrap();
    let mut copy = row("test:copy", "v1", "Repeating that original", None);
    copy["origin_group"] = json!("test:origin");
    copy["copies_from"] = json!({"id":"test:first","revision":"v1"});
    import(&s, bundle(vec![copy])).unwrap();
    let a = s.list("alerts").unwrap()["items"][0].clone();
    assert_eq!(a["id"], first["id"]);
    assert_eq!(a["evidence"].as_array().unwrap().len(), 2);
    assert_eq!(a["review_status"], "acknowledged");
}
#[test]
fn semantically_equal_uploads_retain_their_actual_original_bytes() {
    let (_tmp, s) = store();
    let b = bundle(vec![row("test:original", "v1", "Évidence 🛰", None)]);
    let pretty = serde_json::to_string_pretty(&b).unwrap() + "\n";
    let compact = serde_json::to_vec(&b).unwrap();
    let a = s
        .import_job_with_raw(
            b.clone(),
            ident("job"),
            Arc::new(AtomicBool::new(false)),
            Some(pretty.as_bytes().to_vec()),
        )
        .unwrap();
    let second = s
        .import_job_with_raw(
            b,
            ident("job"),
            Arc::new(AtomicBool::new(false)),
            Some(compact.clone()),
        )
        .unwrap();
    assert_eq!(a["publication_id"], second["publication_id"]);
    assert_ne!(a["original_input_hash"], second["original_input_hash"]);
    assert_eq!(
        s.original_object(a["original_input_hash"].as_str().unwrap())
            .unwrap(),
        pretty.as_bytes()
    );
    assert_eq!(
        s.original_object(second["original_input_hash"].as_str().unwrap())
            .unwrap(),
        compact
    );
    assert_eq!(
        s.detail("test:original", "v1").unwrap()["original_input_hashes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn publication_intervals_grow_with_changes_and_restore_frozen_branches() {
    let (_temp, s) = store();
    let first = import(
        &s,
        bundle(vec![row(
            "test:steady",
            "v1",
            "Unchanged exact original",
            None,
        )]),
    )
    .unwrap();
    for n in 0..25 {
        import(
            &s,
            bundle(vec![row(
                &format!("test:{n}"),
                "v1",
                "Additional message",
                None,
            )]),
        )
        .unwrap();
    }
    let c = s.connect().unwrap();
    let physical: i64 = c
        .query_row("SELECT count(*) FROM membership_intervals", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(physical, 26);
    assert_eq!(query(&s, Scope::default())["total"], 26);
    assert_eq!(
        query(
            &s,
            Scope {
                publication_id: first["publication_id"].as_str().map(str::to_owned),
                ..Scope::default()
            }
        )["total"],
        1
    );
    import(&s, fixture()).unwrap(); // inserts an independent frozen snapshot
    import(
        &s,
        bundle(vec![row("test:after-freeze", "v1", "After branch", None)]),
    )
    .unwrap();
    assert_eq!(query(&s, Scope::default())["total"], 31);
    let backup = s.backup().unwrap();
    assert_eq!(
        s.verify_backup(backup["id"].as_str().unwrap()).unwrap()["verified"],
        true
    );
}

#[test]
fn all_timeline_outlier_is_explicit_and_does_not_remove_scope_members() {
    let (_temp, s) = store();
    let mut rows: Vec<_> = (0..99)
        .map(|i| row(&format!("time:{i}"), "v1", "Recent message", None))
        .collect();
    let mut old = row("time:old", "v1", "Pinned older message", None);
    old["published_at"] = json!("2023-01-01T00:00:00Z");
    rows.push(old);
    import(&s, bundle(rows)).unwrap();
    let all = query(&s, Scope::default());
    assert_eq!(all["total"], 100);
    assert_eq!(all["timeline"]["earlier"], 1);
    assert_eq!(timeline_count(&all), 99);
    assert_eq!(all["timeline"]["earliest"], "2023-01-01T00:00:00.000Z");
    let explicit = query(
        &s,
        Scope {
            from: Some("2023-01-01T00:00:00Z".into()),
            ..Scope::default()
        },
    );
    assert_eq!(explicit["timeline"]["earlier"], 0);
    assert_eq!(timeline_count(&explicit), 100);
}

#[test]
fn backup_does_not_take_application_writer_lock() {
    let (_temp, s) = store();
    import(
        &s,
        bundle(vec![row(
            "test:backup",
            "v1",
            "Exact retained original",
            None,
        )]),
    )
    .unwrap();
    let lock = s.writer.lock().unwrap();
    let clone = s.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || tx.send(clone.backup().is_ok()).unwrap());
    let result = rx.recv_timeout(std::time::Duration::from_secs(3));
    drop(lock);
    worker.join().unwrap();
    assert!(result.unwrap());
}

#[test]
fn verification_only_cleans_its_temporary_restore_and_failure_scratch() {
    let (_temp, s) = store();
    let backup = s.backup().unwrap();
    let id = backup["id"].as_str().unwrap();
    let checked = s.check_backup(id).unwrap();
    assert_eq!(checked["verified"], true);
    assert!(checked["restored_directory"].is_null());
    let scratch = s.config.data.join("restore-verifications");
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
    let restored = s.verify_backup(id).unwrap();
    assert!(std::path::Path::new(restored["restored_directory"].as_str().unwrap()).exists());
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 1);
}

#[test]
fn continuation_page_keeps_original_publication_during_live_updates() {
    let (_tmp, s) = store();
    import(
        &s,
        bundle(
            (0..5)
                .map(|n| row(&format!("page:{n}"), "v1", "Fictional old page", None))
                .collect(),
        ),
    )
    .unwrap();
    let first = s
        .query(Query {
            scope: json!({}),
            generation: 8,
            cursor: None,
            limit: Some(2),
        })
        .unwrap();
    import(
        &s,
        bundle(vec![row(
            "page:new",
            "v1",
            "Fictional newly published message",
            None,
        )]),
    )
    .unwrap();
    let request = || Query {
        scope: first["scope"].clone(),
        generation: 8,
        cursor: Some(first["next_cursor"].as_str().unwrap().into()),
        limit: Some(2),
    };
    let next = s.query_page_preview(request()).unwrap();
    let full = s.query_preview(request()).unwrap();
    assert_eq!(next["items"], full["items"]);
    assert_eq!(next["publication_id"], first["publication_id"]);
    assert_eq!(next["scope_revision"], first["scope_revision"]);
    assert!(next.get("map").is_none());
    assert!(next.get("map_areas").is_none());
    assert!(next.get("timeline").is_none());
    assert!(
        s.query_page(Query {
            scope: json!({}),
            ..request()
        })
        .is_err()
    );
    assert!(
        s.query_page(Query {
            scope: json!({"publication_id":first["publication_id"],"query":"different"}),
            ..request()
        })
        .is_err()
    );
}

#[test]
fn incremental_backups_share_only_backup_objects_and_survive_live_corruption() {
    let (_tmp, s) = store();
    import(&s, fixture()).unwrap();
    let first = s.backup().unwrap();
    let second = s.backup().unwrap();
    assert_eq!(second["reused_backup_objects"], first["objects"]);
    let hash: String = s
        .connect()
        .unwrap()
        .query_row("SELECT hash FROM objects LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let path = |backup: &Value| {
        s.config
            .data
            .join("backups")
            .join(backup["id"].as_str().unwrap())
            .join("objects")
            .join(&hash[..2])
            .join(&hash)
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(path(&first)).unwrap().ino(),
            std::fs::metadata(path(&second)).unwrap().ino()
        );
        assert_ne!(
            std::fs::metadata(path(&first)).unwrap().ino(),
            std::fs::metadata(s.config.object_path(&hash))
                .unwrap()
                .ino()
        );
    }
    std::fs::write(s.config.object_path(&hash), b"fictional live corruption").unwrap();
    std::fs::remove_dir_all(
        s.config
            .data
            .join("backups")
            .join(first["id"].as_str().unwrap()),
    )
    .unwrap();
    assert_eq!(
        s.check_backup(second["id"].as_str().unwrap()).unwrap()["verified"],
        true
    );
}

/// Startup maintenance marks in-flight requests unknown, so a second app
/// process must never take a workspace another one is using.
#[test]
fn a_second_process_cannot_take_a_workspace_in_use() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("workspace");
    let first = relic_app::store::lock_workspace(&data).unwrap();
    let refused = relic_app::store::lock_workspace(&data).expect_err("a second process must be refused");
    assert_eq!(refused.code, "conflict");
    assert!(refused.message.contains("already running"), "{}", refused.message);
    drop(first);
    relic_app::store::lock_workspace(&data).expect("the workspace is free once the first process is gone");
}

/// Every opener (the app, examples, tools) takes the workspace lock in
/// `Store::open`, before startup maintenance touches another process's
/// reserved requests. The same process may reopen its own workspace.
#[test]
fn opening_a_store_holds_the_workspace_for_this_process() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let config = |name: &str| Config::local(root.clone(), tmp.path().join(name), "127.0.0.1:8790".parse().unwrap());
    let s = Store::open(config("workspace")).unwrap();
    // Reopening in the same process, as tests do to simulate a restart.
    let again = Store::open(config("workspace")).unwrap();
    drop(again);
    // Another process (a separate lock on the same file) is refused while
    // this process holds the workspace, even after its stores are dropped.
    let refused = relic_app::store::lock_workspace(&s.config.data).expect_err("another process must be refused");
    assert_eq!(refused.code, "conflict");
    drop(s);
    assert!(relic_app::store::lock_workspace(&tmp.path().join("workspace")).is_err());
    // A workspace another process holds is not opened, and nothing in it is touched.
    let held = relic_app::store::lock_workspace(&tmp.path().join("other")).unwrap();
    let refused = Store::open(config("other")).err().expect("a workspace in use must be refused");
    assert_eq!(refused.code, "conflict");
    assert!(refused.message.contains("already running"), "{}", refused.message);
    assert!(!tmp.path().join("other/relic.sqlite3").exists());
    drop(held);
    Store::open(config("other")).expect("free once the other process is gone");
}

/// Run as a child process by the test below; does nothing on its own.
#[test]
fn workspace_lock_child_probe() {
    let Some(data) = std::env::var_os("RELIC_LOCK_PROBE_DATA") else {
        return;
    };
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    std::process::exit(match Store::open(Config::local(root, data.into(), "127.0.0.1:8790".parse().unwrap())) {
        Err(e) if e.code == "conflict" => 17,
        Err(_) => 1,
        Ok(_) => 0,
    });
}
#[test]
fn another_process_cannot_open_a_workspace_this_process_holds() {
    let (_tmp, s) = store();
    let probe = |data: &std::path::Path| {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "workspace_lock_child_probe", "--test-threads", "1", "--quiet"])
            .env("RELIC_LOCK_PROBE_DATA", data)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap()
            .code()
    };
    assert_eq!(probe(&s.config.data), Some(17), "the child must get the conflict error");
    let free = tempfile::tempdir().unwrap();
    assert_eq!(probe(&free.path().join("workspace")), Some(0), "a free workspace opens");
}
