use relic_app::{
    config::Config,
    event_link_store::{self, Pair},
    model::{Scope, ident, value},
    processing_ai::validate_output,
    processing_geo::GeoResolver,
    processing_store::{self, Policy},
    store::Store,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
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
fn row(id: &str, text: &str, source: &str) -> Value {
    let published = format!(
        "2026-09-23T12:00:00.00{}Z",
        match id {
            "fiction:a" => 1,
            "fiction:b" => 2,
            "fiction:c" => 3,
            _ => 4,
        }
    );
    json!({"id":id,"revision":"v1","source":source,"published_at":published,"acquired_at":"2026-09-23T12:01:00Z","text":text,"geometry":null,"location_precision":"unresolved","origin_group":null,"dependence":"unknown","language":"en","fictional":true,"availability":"available","citation":{"evidence_id":id,"revision":"v1","start_character":0,"end_character":text.chars().count()}})
}
fn seed(s: &Store) -> String {
    let rows = vec![
        row(
            "fiction:a",
            "A fire broke out at the North Pier grain warehouse in London.",
            "fiction:alpha",
        ),
        row(
            "fiction:b",
            "Firefighters contained the fire at the North Pier grain warehouse in London.",
            "fiction:beta",
        ),
        row(
            "fiction:c",
            "A fire broke out at the East Park school in London.",
            "fiction:beta",
        ),
        row(
            "fiction:d",
            "Authorities deny reports of a fire at the North Pier grain warehouse in London.",
            "fiction:alpha",
        ),
    ];
    s.import_job(json!({"schema_version":1,"fictional":true,"title":"Fictional related messages","sources":[{"id":"fiction:alpha","label":"Fictional Alpha","family":"authored_report"},{"id":"fiction:beta","label":"Fictional Beta","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
    let pubid = publish_readings(s);
    s.set_processing_policy(Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into(), "fiction:beta".into()],
    })
    .unwrap();
    s.set_event_links_enabled(true).unwrap();
    pubid
}
fn publish_readings(s: &Store) -> String {
    let resolver = GeoResolver::load(&s.config.root).unwrap();
    let mut candidates = vec![];
    for id in ["fiction:a", "fiction:b", "fiction:c", "fiction:d"] {
        let e = s.detail(id, "v1").unwrap();
        let text = e["text"].as_str().unwrap();
        let state = if id == "fiction:d" {
            "denied"
        } else {
            "reported"
        };
        let checked=validate_output(text,&json!({"language":"en","translation":null,"headline":text,"kind":"event","claim_status":state,"topic":"safety","claims":[{"text":text,"quote":text,"state":state,"event_type":"fire","time_text":null,"location_indices":[0]}],"locations":[{"mention":"London","quote":text,"role":"event","canonical_name":"London","country_code":"GB"}]})).unwrap();
        let rowid = s
            .connect()
            .unwrap()
            .query_row("SELECT rowid FROM evidence WHERE id=?1", [id], |r| r.get(0))
            .unwrap();
        let mut candidate =
            processing_store::prepare(rowid, &e, &checked, &resolver, "fixture/gold").unwrap();
        let generation: i64 = s
            .connect()
            .unwrap()
            .query_row("SELECT count(*) FROM interpretations", [], |r| r.get(0))
            .unwrap();
        candidate.interpretation["recipe"] = json!(format!("fixture/gold-{generation}"));
        candidates.push(candidate);
    }
    s.publish_processing(candidates, &ident("processing"), &AtomicBool::new(false))
        .unwrap()
        .unwrap()
}
fn finish(s: &Store, p: &Pair) {
    let relation = if p.a.evidence_id == "fiction:c" || p.b.evidence_id == "fiction:c" {
        "distinct"
    } else if p.a.claim.state != p.b.claim.state {
        "denial"
    } else {
        "update"
    };
    let response = json!({"model":"openai/gpt-6-luna","provider":"OpenAI","usage":{"cost":0.001},"choices":[{"finish_reason":"stop","message":{"content":json!({"relation":relation,"a_quote":p.a.claim.text,"b_quote":p.b.claim.text,"reason":"Fictional same warehouse context or explicitly different venue."}).to_string()}}]});
    s.finish_event_pair(
        p,
        Some(200),
        &serde_json::to_vec(&response).unwrap(),
        None,
        None,
    )
    .unwrap();
}
fn query(s: &Store, id: &str, scope: Scope) -> Value {
    s.query_event_links(json!({"id":id,"revision":"v1","scope":value(scope)}))
        .unwrap()
}
#[test]
fn new_retrieval_reuses_paid_decisions_and_requires_explicit_activation() {
    let (_tmp, s) = workspace();
    seed(&s);
    let flag = AtomicBool::new(false);
    s.index_event_claims(&flag).unwrap();
    for pair in s.admit_event_pairs(&flag).unwrap() {
        finish(&s, &pair);
    }
    s.publish_event_links(&flag).unwrap();
    let before = query(&s, "fiction:a", Scope::default());
    let c = s.connect().unwrap();
    // Simulate the prior deployment: same paid profile, old scan/activation
    // identity. Changing retrieval must neither authorize work nor rebill it.
    c.execute(
        "UPDATE event_link_pairs SET profile=?1",
        [event_link_store::profile_key()],
    )
    .unwrap();
    c.execute(
        "UPDATE metadata SET value=?1 WHERE key='event_links_enabled'",
        [event_link_store::profile_key()],
    )
    .unwrap();
    assert!(!s.event_links_enabled().unwrap());
    assert_eq!(s.event_link_status().unwrap()["activation_required"], true);
    assert!(s.admit_event_pairs(&flag).unwrap().is_empty());
    s.set_event_links_enabled(true).unwrap();
    assert!(s.admit_event_pairs(&flag).unwrap().is_empty());
    assert_eq!(s.publish_event_links(&flag).unwrap(), 0);
    assert_eq!(query(&s, "fiction:a", Scope::default()), before);
    assert_eq!(
        c.query_row("SELECT count(*) FROM event_link_attempts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        6
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM event_link_pairs WHERE state='complete' AND profile=?1",
            [event_link_store::admission_key()],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        6
    );
    // A new cross-type update is now considered beside those reused pairs.
    let text = "Crews have reopened the North Pier grain warehouse in London after today's fire.";
    let mut e = row("fiction:e", text, "fiction:alpha");
    e["published_at"] = json!("2026-09-23T12:02:00Z");
    s.import_job(json!({"schema_version":1,"fictional":true,"title":"Fictional update","sources":[{"id":"fiction:alpha","label":"Fictional Alpha","family":"authored_report"}],"evidence_revisions":[e]}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
    let rowid: i64 = c
        .query_row("SELECT rowid FROM evidence WHERE id='fiction:e'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let checked = validate_output(text, &json!({"language":"en","translation":null,"headline":text,"kind":"event","claim_status":"reported","topic":"safety","claims":[{"text":text,"quote":text,"state":"reported","event_type":"other","time_text":"today","location_indices":[0]}],"locations":[{"mention":"London","quote":text,"role":"event","canonical_name":"London","country_code":"GB"}]})).unwrap();
    let reading = processing_store::prepare(
        rowid,
        &e,
        &checked,
        &GeoResolver::load(&s.config.root).unwrap(),
        "fixture/gold",
    )
    .unwrap();
    s.publish_processing(vec![reading], &ident("processing"), &flag)
        .unwrap();
    s.index_event_claims(&flag).unwrap();
    // Earlier local rejection has no paid attempt and must not block v2.
    c.execute("INSERT INTO event_link_pairs SELECT a.id,b.id,?1,NULL,'ineligible' FROM event_link_claims a JOIN event_link_claims b WHERE json_extract(a.json,'$.evidence_id')='fiction:a' AND json_extract(b.json,'$.evidence_id')='fiction:e'", [event_link_store::profile_key()]).unwrap();
    let fresh = s.admit_event_pairs(&flag).unwrap();
    assert_eq!(fresh.len(), 4);
    assert!(fresh.iter().all(|pair| pair.b.evidence_id == "fiction:e"
        && pair.a.claim.event_type != pair.b.claim.event_type));
    assert_eq!(
        c.query_row("SELECT count(*) FROM event_link_attempts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        10
    );
    for pair in &fresh {
        finish(&s, pair);
    }
    s.publish_event_links(&flag).unwrap();
    assert!(s.admit_event_pairs(&flag).unwrap().is_empty());
    assert_eq!(query(&s, "fiction:a", Scope::default())["total"], 3);
}

#[test]
fn exact_links_preserve_scope_history_originals_and_restart() {
    let (tmp, s) = workspace();
    let before = seed(&s);
    let flag = AtomicBool::new(false);
    assert_eq!(event_link_store::acceptance().unwrap()["accepted"], 62);
    assert_eq!(event_link_store::acceptance().unwrap()["strict_exact"], 59);
    s.index_event_claims(&flag).unwrap();
    let pairs = s.admit_event_pairs(&flag).unwrap();
    assert_eq!(pairs.len(), 6);
    for p in &pairs {
        finish(&s, p)
    }
    assert_eq!(s.publish_event_links(&flag).unwrap(), 3);
    let all = query(&s, "fiction:a", Scope::default());
    assert_eq!(all["total"], 2);
    let b = query(&s, "fiction:b", Scope::default());
    let original = b["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["evidence"]["id"] == "fiction:a")
        .unwrap();
    assert_eq!(original["links"][0]["direction"], "earlier");
    assert_eq!(original["links"][0]["relation"], "update");
    let backup = s.backup().unwrap();
    let verified = s.verify_backup(backup["id"].as_str().unwrap()).unwrap();
    let mut restored_config = s.config.clone();
    restored_config.data = verified["restored_directory"].as_str().unwrap().into();
    let restored = Store::open(restored_config).unwrap();
    assert_eq!(query(&restored, "fiction:a", Scope::default()), all);
    let linked_pub = all["publication_id"].as_str().unwrap().to_owned();
    publish_readings(&s); // new exact reading IDs invalidate current links
    assert_eq!(query(&s, "fiction:a", Scope::default())["total"], 0);
    assert_eq!(
        query(
            &s,
            "fiction:a",
            Scope {
                publication_id: Some(linked_pub),
                ..Scope::default()
            }
        )["total"],
        2
    );
    s.index_event_claims(&flag).unwrap();
    assert!(s.admit_event_pairs(&flag).unwrap().is_empty()); // identical requests reuse retained reviews
    assert_eq!(s.publish_event_links(&flag).unwrap(), 3);
    assert_eq!(query(&s, "fiction:a", Scope::default())["total"], 2);
    assert_eq!(s.event_link_status().unwrap()["completed"], 6);
    assert_eq!(query(&s, "fiction:c", Scope::default())["total"], 0);
    assert_eq!(
        query(
            &s,
            "fiction:a",
            Scope {
                publication_id: Some(before),
                ..Scope::default()
            }
        )["total"],
        0
    );
    assert_eq!(
        query(
            &s,
            "fiction:a",
            Scope {
                source_ids: vec!["fiction:alpha".into()],
                ..Scope::default()
            }
        )["total"],
        1
    );
    assert_eq!(
        query(
            &s,
            "fiction:b",
            Scope {
                source_ids: vec!["fiction:alpha".into()],
                ..Scope::default()
            }
        )["total"],
        0
    );
    assert_eq!(
        query(
            &s,
            "fiction:a",
            Scope {
                content_view: "events".into(),
                ..Scope::default()
            }
        )["total"],
        1
    );
    assert_eq!(
        s.detail("fiction:a", "v1").unwrap()["text"],
        "A fire broke out at the North Pier grain warehouse in London."
    );
    assert!(s.admit_event_pairs(&flag).unwrap().is_empty());
    assert_eq!(s.publish_event_links(&flag).unwrap(), 0);
    let config = s.config.clone();
    drop(s);
    let reopened = Store::open(config).unwrap();
    assert_eq!(query(&reopened, "fiction:a", Scope::default())["total"], 2);
    assert!(reopened.admit_event_pairs(&flag).unwrap().is_empty());
    if std::env::var_os("RELIC_KEEP_LINK_FIXTURE").is_some() {
        println!("fixture workspace {}", tmp.keep().join("data").display());
    }
}
#[test]
fn pause_cancellation_failed_requests_and_restart_do_not_rebill() {
    let (_tmp, s) = workspace();
    seed(&s);
    let flag = AtomicBool::new(false);
    s.index_event_claims(&flag).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    s.running
        .lock()
        .unwrap()
        .insert("processing:relationships:fixture".into(), cancel.clone());
    s.set_processing_policy(Policy {
        enabled: false,
        eligible_source_ids: vec!["fiction:alpha".into(), "fiction:beta".into()],
    })
    .unwrap();
    assert!(cancel.load(Ordering::Acquire));
    assert!(s.admit_event_pairs(&flag).unwrap().is_empty());
    s.set_processing_policy(Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into(), "fiction:beta".into()],
    })
    .unwrap();
    let pairs = s.admit_event_pairs(&flag).unwrap();
    s.finish_event_pair(
        &pairs[0],
        None,
        &[],
        Some((
            "cancelled",
            "Provider may have charged this interrupted request",
        )),
        None,
    )
    .unwrap();
    for p in &pairs[1..] {
        finish(&s, p)
    }
    assert!(s.publish_event_links(&AtomicBool::new(true)).is_err());
    assert_eq!(query(&s, "fiction:a", Scope::default())["total"], 0);
    let config = s.config.clone();
    drop(s);
    let s = Store::open(config).unwrap();
    assert!(s.admit_event_pairs(&flag).unwrap().is_empty());
    assert_eq!(s.event_link_status().unwrap()["unknown_cost_requests"], 1);
    s.publish_event_links(&flag).unwrap();
}

#[test]
fn rate_limit_deadlines_retries_and_projection_failures_are_isolated() {
    let (_tmp, s) = workspace();
    seed(&s);
    let flag = AtomicBool::new(false);
    s.connect().unwrap().execute("UPDATE interpretations SET json=json_set(json,'$.source_input_hash','invalid') WHERE evidence_rowid=(SELECT rowid FROM evidence WHERE id='fiction:c')",[]).unwrap();
    assert_eq!(s.index_event_claims(&flag).unwrap(), 4);
    assert_eq!(s.event_link_status().unwrap()["projection_errors"], 1);
    let pairs = s.admit_event_pairs(&flag).unwrap();
    assert_eq!(pairs.len(), 3);
    assert_eq!(s.event_link_status().unwrap()["unknown_cost_requests"], 0);
    s.finish_event_pair(
        &pairs[0],
        Some(429),
        br#"{"error":{"code":429}}"#,
        None,
        Some("3600"),
    )
    .unwrap();
    s.finish_event_pair(
        &pairs[1],
        None,
        &[],
        Some(("model_not_sent", "Not dispatched")),
        None,
    )
    .unwrap();
    finish(&s, &pairs[2]);
    let cooling_version = s.event_link_work_version().unwrap();
    let deadline: String = s
        .connect()
        .unwrap()
        .query_row(
            "SELECT retry_at FROM event_link_attempts WHERE id=?1",
            [&pairs[0].key],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        chrono::DateTime::parse_from_rfc3339(&deadline).unwrap()
            > chrono::Utc::now() + chrono::Duration::minutes(59)
    );
    assert!(s.admit_event_pairs(&flag).unwrap().is_empty());
    s.connect()
        .unwrap()
        .execute(
            "UPDATE event_link_attempts SET retry_at='2000-01-01T00:00:00Z' WHERE id=?1",
            [&pairs[0].key],
        )
        .unwrap();
    assert_ne!(s.event_link_work_version().unwrap(), cooling_version);
    let retry = s.admit_event_pairs(&flag).unwrap();
    assert_eq!(retry.len(), 2);
    assert!(retry.iter().any(|p| p.key == pairs[0].key));
    let config = s.config.clone();
    drop(s);
    let s = Store::open(config).unwrap();
    assert!(!s.event_links_ready().unwrap());
    assert_eq!(s.event_link_status().unwrap()["unknown_cost_requests"], 2);
    s.set_event_links_enabled(true).unwrap();
    assert!(s.admit_event_pairs(&flag).unwrap().is_empty());
}

fn embedding_body(vectors: &[Vec<f32>]) -> Vec<u8> {
    let data = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| json!({"index":i,"embedding":v}))
        .collect::<Vec<_>>();
    serde_json::to_vec(&json!({"model":"text-embedding-3-small","provider":"OpenAI","usage":{"cost":0.000002},"data":data})).unwrap()
}
fn unit(parts: &[(usize, f32)]) -> Vec<f32> {
    let mut v = vec![0.0; relic_app::embeddings::DIMENSIONS];
    let norm = parts.iter().map(|(_, x)| x * x).sum::<f32>().sqrt();
    for (i, x) in parts {
        v[*i] = x / norm;
    }
    v
}
#[test]
fn placeless_claims_link_through_semantic_candidates_and_groups_follow_scope() {
    let (_tmp, s) = workspace();
    let texts = [
        ("fiction:a", "A drone struck the fuel depot at Terminal 2.", "fiction:alpha"),
        ("fiction:b", "Drone strike reported on the Terminal 2 fuel depot.", "fiction:beta"),
        ("fiction:c", "A choir performed at the harbour festival.", "fiction:beta"),
    ];
    let rows = texts.iter().map(|(id, text, source)| row(id, text, source)).collect::<Vec<_>>();
    s.import_job(json!({"schema_version":1,"fictional":true,"title":"Fictional placeless reports","sources":[{"id":"fiction:alpha","label":"Fictional Alpha","family":"authored_report"},{"id":"fiction:beta","label":"Fictional Beta","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
    let resolver = GeoResolver::load(&s.config.root).unwrap();
    let mut readings = vec![];
    for (id, text, _) in texts {
        let e = s.detail(id, "v1").unwrap();
        let checked = validate_output(text, &json!({"language":"en","translation":null,"headline":text,"kind":"event","claim_status":"reported","topic":"safety","claims":[{"text":text,"quote":text,"state":"reported","event_type":"strike","time_text":null,"location_indices":[]}],"locations":[]})).unwrap();
        let rowid = s.connect().unwrap().query_row("SELECT rowid FROM evidence WHERE id=?1", [id], |r| r.get(0)).unwrap();
        readings.push(processing_store::prepare(rowid, &e, &checked, &resolver, "fixture/gold").unwrap());
    }
    s.publish_processing(readings, &ident("processing"), &AtomicBool::new(false)).unwrap().unwrap();
    s.set_processing_policy(Policy { enabled: true, eligible_source_ids: vec!["fiction:alpha".into(), "fiction:beta".into()] }).unwrap();
    s.set_event_links_enabled(true).unwrap();
    let flag = AtomicBool::new(false);
    assert_eq!(s.index_event_claims(&flag).unwrap(), 3);
    // Without a single resolved place nothing is admitted by place identity.
    assert!(s.admit_event_pairs(&flag).unwrap().is_empty());
    // Admission is durable before the request and fills each distinct text once.
    let batches = s.reserve_claim_embeddings(&flag).unwrap();
    assert_eq!((batches.len(), batches[0].texts.len()), (1, 3));
    let c = s.connect().unwrap();
    assert_eq!(c.query_row("SELECT status FROM event_embedding_attempts", [], |r| r.get::<_, String>(0)).unwrap(), "reserved");
    let vector = |text: &str| match text {
        t if t.starts_with("A drone") => unit(&[(0, 1.0)]),
        t if t.starts_with("Drone") => unit(&[(0, 0.9), (1, 0.436)]),
        _ => unit(&[(2, 1.0)]),
    };
    let body = embedding_body(&batches[0].texts.iter().map(|t| vector(t)).collect::<Vec<_>>());
    s.finish_claim_embeddings(&batches[0], Some(200), &body, None).unwrap();
    assert!(s.reserve_claim_embeddings(&flag).unwrap().is_empty());
    // Only the close neighbour is a candidate; the unrelated report is not.
    assert_eq!(s.scan_semantic_neighbours(&flag).unwrap(), 1);
    assert_eq!(s.scan_semantic_neighbours(&flag).unwrap(), 0);
    let pairs = s.admit_event_pairs(&flag).unwrap();
    assert_eq!(pairs.len(), 1);
    assert_eq!((pairs[0].a.evidence_id.as_str(), pairs[0].b.evidence_id.as_str()), ("fiction:a", "fiction:b"));
    assert!(pairs[0].a.claim.place.is_none() && pairs[0].b.claim.place.is_none());
    finish(&s, &pairs[0]);
    assert_eq!(s.publish_event_links(&flag).unwrap(), 1);
    let status = s.event_link_status().unwrap();
    assert_eq!((status["semantic"]["vectors"].clone(), status["semantic"]["requests"].clone()), (json!(3), json!(1)));
    assert!((status["known_cost_usd"].as_f64().unwrap() - 0.001002).abs() < 1e-9);
    // The group and the feed counts use the same scope.
    let group = s.event_group(json!({"id":"fiction:a","revision":"v1","scope":value(Scope::default())})).unwrap();
    assert_eq!(group["groups"].as_array().unwrap().len(), 1);
    let group = &group["groups"][0];
    assert_eq!(group["members"].as_array().unwrap().len(), 2);
    assert_eq!(group["members"][0]["anchor"], true);
    assert_eq!(group["links"][0]["relation"], "update");
    let feed = s.query(relic_app::model::Query { scope: value(Scope::default()), generation: 0, cursor: None, limit: None }).unwrap();
    let counts = feed["items"].as_array().unwrap().iter().map(|e| (e["id"].as_str().unwrap().to_owned(), e["event_links"].clone())).collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!((counts["fiction:a"].clone(), counts["fiction:b"].clone(), counts["fiction:c"].clone()), (json!(1), json!(1), Value::Null));
    // Both reports carry the same event group id and its report count.
    let groups = feed["items"].as_array().unwrap().iter().filter_map(|e| e.get("event_group").cloned()).collect::<Vec<_>>();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0], groups[1]);
    assert_eq!(groups[0]["reports"], 2);
    let alpha = Scope { source_ids: vec!["fiction:alpha".into()], ..Scope::default() };
    let hidden = s.event_group(json!({"id":"fiction:a","revision":"v1","scope":value(&alpha)})).unwrap();
    assert!(hidden["groups"].as_array().unwrap().is_empty());
    let feed = s.query(relic_app::model::Query { scope: value(&alpha), generation: 0, cursor: None, limit: None }).unwrap();
    assert!(feed["items"].as_array().unwrap().iter().all(|e| e.get("event_links").is_none()));
    // An embedding interrupted by restart is unknown billing and stops the lane.
    c.execute("INSERT INTO event_embedding_attempts VALUES('embedding:interrupted','reserved','2026-09-23T12:00:00Z',NULL,(SELECT request_hash FROM event_embedding_attempts LIMIT 1),NULL,NULL)", []).unwrap();
    drop(c);
    let config = s.config.clone();
    drop(s);
    let s = Store::open(config).unwrap();
    assert!(!s.event_links_ready().unwrap());
    let status = s.event_link_status().unwrap();
    assert_eq!(status["unknown_cost_requests"], 1);
    assert_eq!(status["semantic"]["failed"], 1);
}

#[test]
fn credit_rejections_cool_down_without_holding_the_lane_and_errors_are_readable() {
    let (_tmp, s) = workspace();
    seed(&s);
    let flag = AtomicBool::new(false);
    s.index_event_claims(&flag).unwrap();
    let pairs = s.admit_event_pairs(&flag).unwrap();
    let credits = json!({"error":{"message":"This request requires more credits.","code":402,"metadata":{"limit_source":"openrouter_credits","provider_name":null}}});
    s.finish_event_pair(&pairs[0], Some(402), &serde_json::to_vec(&credits).unwrap(), Some(("generation_id", "gen-fixture")), None).unwrap();
    let status = s.event_link_status().unwrap();
    assert_eq!(status["stopped"], false);
    assert_eq!(status["unknown_cost_requests"], 0);
    assert!(status["last_error"].as_str().unwrap().contains("credits are exhausted"));
    // Cooling: no new admissions until the deadline, then the pair is retried.
    assert!(s.admit_event_pairs(&flag).unwrap().is_empty());
    s.connect().unwrap().execute("UPDATE event_link_attempts SET retry_at='2000-01-01T00:00:00.000Z' WHERE status='rate_limited'", []).unwrap();
    assert!(s.admit_event_pairs(&flag).unwrap().iter().any(|p| p.key == pairs[0].key));
    // Any other HTTP failure holds the lane with the provider's own message.
    let server = json!({"error":{"message":"Upstream overloaded","code":503}});
    s.finish_event_pair(&pairs[1], Some(503), &serde_json::to_vec(&server).unwrap(), Some(("generation_id", "gen-fixture-2")), None).unwrap();
    let status = s.event_link_status().unwrap();
    assert_eq!(status["stopped"], true);
    assert_eq!(status["last_error"], "Relationship provider HTTP 503: Upstream overloaded");
}

#[test]
fn a_message_with_two_events_joins_two_groups_without_merging_them() {
    let (_tmp, s) = workspace();
    let a_text = "A fire broke out at the North Pier warehouse in London. Police arrested two protesters at East Park in London.";
    let texts = [
        ("fiction:a", a_text, "fiction:alpha"),
        ("fiction:b", "Firefighters contained the fire at the North Pier warehouse in London.", "fiction:beta"),
        ("fiction:c", "Police in London said two protesters were arrested at East Park.", "fiction:beta"),
    ];
    let rows = texts.iter().map(|(id, text, source)| row(id, text, source)).collect::<Vec<_>>();
    s.import_job(json!({"schema_version":1,"fictional":true,"title":"Fictional two-event message","sources":[{"id":"fiction:alpha","label":"Fictional Alpha","family":"authored_report"},{"id":"fiction:beta","label":"Fictional Beta","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
    let resolver = GeoResolver::load(&s.config.root).unwrap();
    let claim = |text: &str, quote: &str, kind: &str| json!({"text":text,"quote":quote,"state":"reported","event_type":kind,"time_text":null,"location_indices":[0]});
    let mut readings = vec![];
    for (id, text, _) in texts {
        let claims = if id == "fiction:a" {
            vec![
                claim("A fire broke out at the North Pier warehouse in London.", "A fire broke out at the North Pier warehouse in London.", "fire"),
                claim("Police arrested two protesters at East Park in London.", "Police arrested two protesters at East Park in London.", "arrest"),
            ]
        } else {
            vec![claim(text, text, if id == "fiction:b" { "fire" } else { "arrest" })]
        };
        let checked = validate_output(text, &json!({"language":"en","translation":null,"headline":text,"kind":"event","claim_status":"reported","topic":"safety","claims":claims,"locations":[{"mention":"London","quote":text,"role":"event","canonical_name":"London","country_code":"GB"}]})).unwrap();
        let e = s.detail(id, "v1").unwrap();
        let rowid = s.connect().unwrap().query_row("SELECT rowid FROM evidence WHERE id=?1", [id], |r| r.get(0)).unwrap();
        readings.push(processing_store::prepare(rowid, &e, &checked, &resolver, "fixture/gold").unwrap());
    }
    s.publish_processing(readings, &ident("processing"), &AtomicBool::new(false)).unwrap().unwrap();
    s.set_processing_policy(Policy { enabled: true, eligible_source_ids: vec!["fiction:alpha".into(), "fiction:beta".into()] }).unwrap();
    s.set_event_links_enabled(true).unwrap();
    let flag = AtomicBool::new(false);
    s.index_event_claims(&flag).unwrap();
    // Fire links to fire and arrest to arrest; everything else is distinct.
    for p in s.admit_event_pairs(&flag).unwrap() {
        let relation = if p.a.claim.event_type == p.b.claim.event_type { "same_occurrence" } else { "distinct" };
        let response = json!({"model":"openai/gpt-6-luna","provider":"OpenAI","usage":{"cost":0.001},"choices":[{"finish_reason":"stop","message":{"content":json!({"relation":relation,"a_quote":p.a.claim.text,"b_quote":p.b.claim.text,"reason":"Fixture decision."}).to_string()}}]});
        s.finish_event_pair(&p, Some(200), &serde_json::to_vec(&response).unwrap(), None, None).unwrap();
    }
    s.publish_event_links(&flag).unwrap();
    let groups = s.event_group(json!({"id":"fiction:a","revision":"v1","scope":value(Scope::default())})).unwrap();
    let groups = groups["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "one group per reported event");
    let members = |g: &Value| g["members"].as_array().unwrap().iter().map(|m| m["evidence"]["id"].as_str().unwrap().to_owned()).collect::<std::collections::BTreeSet<_>>();
    let sets = groups.iter().map(members).collect::<Vec<_>>();
    assert!(sets.contains(&["fiction:a".to_owned(), "fiction:b".to_owned()].into()));
    assert!(sets.contains(&["fiction:a".to_owned(), "fiction:c".to_owned()].into()));
    // In the feed, b and c are different events; a joins one of them.
    let feed = s.query(relic_app::model::Query { scope: value(Scope::default()), generation: 0, cursor: None, limit: None }).unwrap();
    let group_of = |id: &str| feed["items"].as_array().unwrap().iter().find(|e| e["id"] == id).unwrap()["event_group"]["id"].clone();
    assert_ne!(group_of("fiction:b"), group_of("fiction:c"));
    assert!(group_of("fiction:a") == group_of("fiction:b") || group_of("fiction:a") == group_of("fiction:c"));
    assert_eq!(s.event_link_status().unwrap()["links"], 2);
}

#[test]
fn same_place_reports_of_one_kind_group_by_rule_until_a_reviewer_says_distinct() {
    let (_tmp, s) = workspace();
    // Owner, 26 October 2026: obvious repeats were left apart by the reviewer.
    let texts = [
        ("fiction:a", "A fictional strike was reported in London this morning.", "fiction:alpha", "2026-09-23T08:00:00Z", "strike"),
        ("fiction:b", "A fictional explosion was heard in London after the strike.", "fiction:beta", "2026-09-23T09:30:00Z", "explosion"),
        ("fiction:c", "Another fictional strike was reported in London tonight.", "fiction:beta", "2026-09-23T15:30:00Z", "strike"),
        ("fiction:d", "A fictional fire broke out in London.", "fiction:alpha", "2026-09-23T09:00:00Z", "fire"),
    ];
    let rows = texts.iter().map(|(id, text, source, at, _)| {
        let mut r = row(id, text, source);
        r["published_at"] = json!(at);
        r["acquired_at"] = json!(at);
        r
    }).collect::<Vec<_>>();
    s.import_job(json!({"schema_version":1,"fictional":true,"title":"Fictional repeated reports","sources":[{"id":"fiction:alpha","label":"Fictional Alpha","family":"authored_report"},{"id":"fiction:beta","label":"Fictional Beta","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
    let resolver = GeoResolver::load(&s.config.root).unwrap();
    let mut readings = vec![];
    for (id, text, _, _, kind) in texts {
        let checked = validate_output(text, &json!({"language":"en","translation":null,"headline":text,"kind":"event","claim_status":"reported","topic":"safety","claims":[{"text":text,"quote":text,"state":"reported","event_type":kind,"time_text":null,"location_indices":[0]}],"locations":[{"mention":"London","quote":text,"role":"event","canonical_name":"London","country_code":"GB"}]})).unwrap();
        let e = s.detail(id, "v1").unwrap();
        let rowid = s.connect().unwrap().query_row("SELECT rowid FROM evidence WHERE id=?1", [id], |r| r.get(0)).unwrap();
        readings.push(processing_store::prepare(rowid, &e, &checked, &resolver, "fixture/gold").unwrap());
    }
    s.publish_processing(readings, &ident("processing"), &AtomicBool::new(false)).unwrap().unwrap();
    s.set_processing_policy(Policy { enabled: true, eligible_source_ids: vec!["fiction:alpha".into(), "fiction:beta".into()] }).unwrap();
    s.set_event_links_enabled(true).unwrap();
    let flag = AtomicBool::new(false);
    s.index_event_claims(&flag).unwrap();
    let groups = |s: &Store| {
        let feed = s.query(relic_app::model::Query { scope: value(Scope::default()), generation: 0, cursor: None, limit: None }).unwrap();
        texts.map(|(id, ..)| feed["items"].as_array().unwrap().iter().find(|e| e["id"] == id).unwrap()["event_group"]["id"].clone())
    };
    // No reviewed link yet: the rule joins the strike and the explosion 90
    // minutes later (one attack family, one place); the strike six hours later
    // and the fire stay apart.
    let [a, b, c, d] = groups(&s);
    assert!(a.is_string() && a == b, "{a} {b}");
    assert!(c.is_null() && d.is_null(), "{c} {d}");
    // A reviewer's "distinct" decision vetoes the rule for that pair.
    for p in s.admit_event_pairs(&flag).unwrap() {
        let response = json!({"model":"openai/gpt-6-luna","provider":"OpenAI","usage":{"cost":0.001},"choices":[{"finish_reason":"stop","message":{"content":json!({"relation":"distinct","a_quote":p.a.claim.text,"b_quote":p.b.claim.text,"reason":"Fixture decision."}).to_string()}}]});
        s.finish_event_pair(&p, Some(200), &serde_json::to_vec(&response).unwrap(), None, None).unwrap();
    }
    s.publish_event_links(&flag).unwrap();
    let [a, b, ..] = groups(&s);
    assert!(a.is_null() && b.is_null(), "{a} {b}");
}
