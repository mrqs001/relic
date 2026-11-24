use relic_app::{
    config::Config,
    model::{Query, Scope, hash, ident, value},
    processing_store::Policy,
    store::Store,
    supplemental_ai::{self as ai, Context},
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
        "127.0.0.1:8793".parse().unwrap(),
    ))
    .unwrap();
    let mut rows:Vec<Value>=(0..4).map(|n|json!({"id":format!("fiction:{n}"),"revision":"v1","source":if n==3{"fiction:beta"}else{"fiction:alpha"},"text":if n==0{"Fictional fire reported in London."}else if n==1{"More images from the fire."}else{"Fictional general commentary."},"published_at":format!("2026-09-10T12:0{n}:00Z"),"acquired_at":"2026-09-10T12:05:00Z","geometry":null,"location_precision":"unresolved","origin_group":null,"dependence":"unknown","language":"en","fictional":true,"availability":"available","citation":null})).collect();
    for e in &mut rows {
        e["citation"] = json!({"evidence_id":e["id"],"revision":"v1","start_character":0,"end_character":e["text"].as_str().unwrap().chars().count()});
    }
    s.import_job(json!({"schema_version":1,"fictional":true,"title":"Supplementary fixture","sources":[{"id":"fiction:alpha","label":"Fictional Alpha","family":"authored_report"},{"id":"fiction:beta","label":"Fictional Beta","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
    s.set_processing_policy(Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into()],
    })
    .unwrap();
    s.set_supplemental_enabled(true).unwrap();
    (tmp, s)
}
fn response(body: &Value) -> Value {
    let mut answers = json!({});
    for (key, q) in body["questions"].as_object().unwrap() {
        let chosen = if q["criteria"].get("uncertain").is_some() {
            "uncertain"
        } else {
            q["criteria"].as_object().unwrap().keys().next().unwrap()
        };
        let mut probabilities = json!({});
        for k in q["criteria"].as_object().unwrap().keys() {
            probabilities[k] = json!(if k == chosen { 1.0 } else { 0.0 });
        }
        answers[key] =
            json!({"type":"choice","choice":chosen,"probabilities":probabilities,"confidence":0.8});
    }
    json!({"model":ai::SERVED_MODEL,"answers":answers,"usage":{"cost":0.0001},"id":"gen-dec-fictional"})
}
fn audit(body: &Value, response: &Value) -> Value {
    let raw = response.to_string();
    json!({"http_status":200,"request_hash":hash(&serde_json::to_vec(body).unwrap()),"response_hash":hash(raw.as_bytes()),"raw_response":raw,"response":response})
}
fn query(s: &Store, events: bool) -> Value {
    let mut scope = Scope::default();
    if events {
        scope.content_view = "events".into();
    }
    s.query(Query {
        scope: value(scope),
        generation: 1,
        cursor: None,
        limit: Some(100),
    })
    .unwrap()
}
/// A Jev answer: every question uncertain or absent, except the given event
/// types present at the given confidence and the given claim state.
fn answers(body: &Value, state: &str, present: &[(&str, f64)]) -> Value {
    // Valid answers give the choice the highest probability, summing to one.
    let pick = |answer: &mut Value, choice: &str, confidence: f64| {
        let other = answer["probabilities"].as_object().unwrap().keys().find(|k| *k != choice).unwrap().clone();
        for (_, p) in answer["probabilities"].as_object_mut().unwrap() {
            *p = json!(0.0);
        }
        answer["probabilities"][choice] = json!(confidence);
        // Rounded, so the value survives the receipt's JSON round trip exactly.
        answer["probabilities"][other] = json!(((1.0 - confidence) * 1000.0).round() / 1000.0);
        answer["choice"] = json!(choice);
        answer["confidence"] = json!(confidence);
    };
    let mut result = response(body);
    for (key, answer) in result["answers"].as_object_mut().unwrap() {
        if key.starts_with("event_") {
            pick(answer, "absent", 1.0);
        }
    }
    for (kind, confidence) in present {
        pick(&mut result["answers"][format!("event_{kind}")], "present", *confidence);
    }
    pick(&mut result["answers"]["claim_state"], state, 1.0);
    result
}
fn ids(result: &Value) -> Vec<String> {
    result["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect()
}
#[test]
fn jev_possible_events_join_event_reports_only_when_the_main_reading_has_none() {
    let (_tmp, s) = workspace();
    let work = s.next_supplemental().unwrap().unwrap();
    let id = work.context.message["id"].as_str().unwrap().to_string();
    // Before the main reading: Jev's first answer alone shows nothing.
    let first = s.reserve_supplemental(&work).unwrap();
    s.finish_supplemental(&first, &audit(&work.request, &answers(&work.request, "reported", &[("security_alert", 0.97)])), None).unwrap();
    assert!(!ids(&query(&s, true)).contains(&id));
    // The main reading finds no event.
    let evidence = s.detail(&id, "v1").unwrap();
    let output = json!({"language":"en","translation":null,"headline":"Fictional notice","kind":"statement","claim_status":"unclear","topic":"other","claims":[],"locations":[]});
    let checked = relic_app::processing_ai::validate_output(evidence["text"].as_str().unwrap(), &output).unwrap();
    let resolver = relic_app::processing_geo::GeoResolver::load(&s.config.root).unwrap();
    let prepared = relic_app::processing_store::prepare(work.rowid, &evidence, &checked, &resolver, "fixture/gold").unwrap();
    s.publish_processing(vec![prepared], "fixture:processing", &AtomicBool::new(false)).unwrap();
    // Read with no event: Jev's first answer now applies until a newer one.
    let first_view = query(&s, true);
    let item = first_view["items"].as_array().unwrap().iter().find(|i| i["id"] == id.as_str()).unwrap();
    assert_eq!(item["possible_event"]["confidence"], 0.97);
    // Jev's answer with the reading as context replaces it.
    let next = s.next_supplemental().unwrap().unwrap();
    assert_eq!((next.rowid, next.phase.as_str()), (work.rowid, "enriched"));
    let second = s.reserve_supplemental(&next).unwrap();
    s.finish_supplemental(&second, &audit(&next.request, &answers(&next.request, "reported", &[("security_alert", 0.93), ("election", 0.99)])), None).unwrap();
    let events = query(&s, true);
    assert_eq!(events["items"].as_array().unwrap().len(), first_view["items"].as_array().unwrap().len());
    let item = events["items"].as_array().unwrap().iter().find(|i| i["id"] == id.as_str()).unwrap();
    assert_eq!(item["possible_event"], json!({"type":"security_alert","confidence":0.93,"suggested_by":"jev"}));
    assert_eq!(item["interpretation"]["kind"], "statement");
    assert!(item["geometry"].is_null());
    // Type filters: its own type matches; an excluded or absent type does not.
    let typed = |kind: &str, events: bool| {
        let scope = Scope {
            event_type: Some(kind.into()),
            content_view: if events { "events" } else { "all" }.into(),
            ..Scope::default()
        };
        ids(&s.query(Query { scope: value(scope), generation: 1, cursor: None, limit: Some(100) }).unwrap())
    };
    assert!(typed("security_alert", true).contains(&id));
    assert!(typed("security_alert", false).contains(&id));
    assert!(!typed("election", true).contains(&id));
    assert!(!typed("armed_attack", true).contains(&id));
    // The reader shows it; the reading itself is unchanged.
    let detail = s.detail_with_publication(&id, "v1", None).unwrap();
    assert_eq!(detail["possible_event"]["type"], "security_alert");
    assert!(detail["interpretation"]["events"].as_array().is_none_or(Vec::is_empty));
    // Below the threshold, or not reported: no possible event.
    let c = s.connect().unwrap();
    for (state, confidence) in [("reported", 0.79), ("planned", 0.97)] {
        let mut result: Value = serde_json::from_str(&c.query_row("SELECT result_json FROM supplemental_attempts WHERE id=?1", [&second], |r| r.get::<_, String>(0)).unwrap()).unwrap();
        result["event_security_alert"]["confidence"] = json!(confidence);
        result["event_security_alert"]["probabilities"] = json!({"absent":1.0-confidence,"present":confidence,"uncertain":0.0});
        result["claim_state"]["choice"] = json!(state);
        c.execute("UPDATE supplemental_attempts SET result_json=?2 WHERE id=?1", rusqlite::params![second, result.to_string()]).unwrap();
        relic_app::jev_hints::refresh(&c, Some(work.rowid)).unwrap();
        assert!(!ids(&query(&s, true)).contains(&id), "{state} {confidence}");
    }
}
#[test]
fn messages_older_than_the_workspace_age_limit_are_not_read() {
    // Fixture messages are dated 10 September 2026; without a limit they are read.
    let (_tmp, s) = workspace();
    let policy = s.processing_policy().unwrap();
    assert!(!s.processing_pending(&policy).unwrap().is_empty());
    assert!(s.next_supplemental().unwrap().is_some());
    // The owner's five-day limit (23 October 2026) leaves them unread by both lanes.
    s.connect().unwrap().execute("INSERT INTO metadata VALUES('processing_max_age_days','5')", []).unwrap();
    assert!(s.processing_pending(&policy).unwrap().is_empty());
    assert!(s.next_supplemental().unwrap().is_none());
}
#[test]
fn suggestions_cannot_discard_or_skip_baseline_and_survive_backup() {
    let (_tmp, s) = workspace();
    let before = query(&s, false);
    let pending = s
        .processing_pending(&s.processing_policy().unwrap())
        .unwrap()
        .len();
    let work = s.next_supplemental().unwrap().unwrap();
    assert_eq!(work.context.neighbours.len(), 2);
    assert!(
        work.context
            .neighbours
            .iter()
            .all(|n| n["source"] == "fiction:alpha")
    );
    let id = s.reserve_supplemental(&work).unwrap();
    let result = response(&work.request);
    s.finish_supplemental(&id, &audit(&work.request, &result), None)
        .unwrap();
    assert!(s.reserve_supplemental(&work).is_err());
    assert_eq!(
        s.processing_pending(&s.processing_policy().unwrap())
            .unwrap()
            .len(),
        pending
    );
    assert_eq!(query(&s, false), before);
    assert_eq!(query(&s, true)["total"], 0);
    let detail = s
        .detail_with_publication(work.context.message["id"].as_str().unwrap(), "v1", None)
        .unwrap();
    assert_eq!(detail["supplemental"]["mode"], "supplementary_only");
    assert!(detail["geometry"].is_null());
    assert_eq!(detail["text"], work.context.message["text"]);
    assert_eq!(s.supplemental_status().unwrap()["completed"], 1);
    let backup = s.backup().unwrap();
    assert_eq!(
        s.verify_backup(backup["id"].as_str().unwrap()).unwrap()["verified"],
        true
    );
}
#[test]
fn unknown_cost_restart_no_retry_or_baseline_hold() {
    let (_tmp, s) = workspace();
    let work = s.next_supplemental().unwrap().unwrap();
    let id = s.reserve_supplemental(&work).unwrap();
    let config = s.config.clone();
    drop(s);
    let reopened = Store::open(config).unwrap();
    let status: String = reopened
        .connect()
        .unwrap()
        .query_row(
            "SELECT status FROM supplemental_attempts WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "unknown");
    assert!(reopened.reserve_supplemental(&work).is_err());
    assert_eq!(
        reopened.supplemental_status().unwrap()["today"]["unknown_cost_requests"],
        1
    );
    reopened.reserve_processing(work.rowid).unwrap();
}
#[test]
fn pause_and_eligibility_checked_before_outbound_admission() {
    let (_tmp, s) = workspace();
    let work = s.next_supplemental().unwrap().unwrap();
    s.set_processing_policy(Policy {
        enabled: false,
        eligible_source_ids: vec!["fiction:alpha".into()],
    })
    .unwrap();
    assert!(s.next_supplemental().unwrap().is_none());
    assert!(s.reserve_supplemental(&work).is_err());
    s.set_processing_policy(Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:beta".into()],
    })
    .unwrap();
    assert!(s.reserve_supplemental(&work).is_err());
    assert_eq!(
        s.next_supplemental().unwrap().unwrap().context.message["source"],
        "fiction:beta"
    );
}
#[test]
fn both_lanes_continue_beyond_former_daily_request_and_spend_caps() {
    let (_tmp, s) = workspace();
    let work = s.next_supplemental().unwrap().unwrap();
    let id = s.reserve_supplemental(&work).unwrap();
    let baseline = s.reserve_processing(work.rowid).unwrap();
    let c = s.connect().unwrap();
    // Historical fixture attempts exceed every former quota in each lane.
    c.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<20001) INSERT INTO processing_attempts SELECT 'history-base-'||x,evidence_rowid,day,created_at,'completed',0.05,0.1,json FROM processing_attempts,n WHERE id=?1", [&baseline]).unwrap();
    c.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<20001) INSERT INTO supplemental_attempts(id,evidence_rowid,recipe,phase,context_hash,publication_id,interpretation_id,created_at,day,status,reserved_usd,cost_usd,request_hash) SELECT 'history-supp-'||x,evidence_rowid,'historical-'||x,phase,context_hash,publication_id,interpretation_id,created_at,day,'completed',0.002,0.1,request_hash FROM supplemental_attempts,n WHERE id=?1", [&id]).unwrap();
    c.execute(
        "UPDATE supplemental_attempts SET status='completed',cost_usd=0.1 WHERE id=?1",
        [&id],
    )
    .unwrap();
    c.execute(
        "UPDATE processing_attempts SET status='completed',cost_usd=0.1 WHERE id=?1",
        [&baseline],
    )
    .unwrap();
    let reopened = Store::open(s.config.clone()).unwrap();
    let status = reopened.processing_status(false).unwrap();
    for today in [&status["today"], &status["supplemental"]["today"]] {
        assert!(today["calls"].as_i64().unwrap() > 20000);
        assert!(today["known_cost_usd"].as_f64().unwrap() > 2000.0);
        assert!(today["limit"].is_null());
        assert!(today["spend_limit_usd"].is_null());
    }
    let next = reopened.next_supplemental().unwrap().unwrap();
    reopened.reserve_supplemental(&next).unwrap();
    reopened.reserve_processing(next.rowid).unwrap();
    assert!(reopened.reserve_supplemental(&next).is_err());
    assert!(reopened.reserve_processing(next.rowid).is_err());
}

#[test]
fn strict_choices_model_probabilities_and_receipt_identity() {
    let (_tmp, s) = workspace();
    let work = s.next_supplemental().unwrap().unwrap();
    let good = response(&work.request);
    assert!(ai::validate(&work.request, &good).is_ok());
    for edit in ["model", "choice", "sum", "missing", "maximum", "unknown"] {
        let mut bad = good.clone();
        match edit {
            "model" => bad["model"] = json!("typesafe/jev-latest"),
            "choice" => bad["answers"]["kind"]["choice"] = json!("discard"),
            "sum" => bad["answers"]["kind"]["probabilities"]["uncertain"] = json!(0.2),
            "missing" => {
                bad["answers"].as_object_mut().unwrap().remove("kind");
            }
            "maximum" => bad["answers"]["kind"]["choice"] = json!("other"),
            _ => bad["answers"]["kind"]["extra"] = json!(true),
        }
        assert!(ai::validate(&work.request, &bad).is_err(), "{edit}");
    }
    let mut bad = audit(&work.request, &good);
    bad["request_hash"] = json!("mismatch");
    assert!(ai::accepted(&work.request, &bad).is_err());
    let id = s.reserve_supplemental(&work).unwrap();
    s.finish_supplemental(&id, &bad, None).unwrap();
    assert_eq!(s.supplemental_status().unwrap()["completed"], 0);
    assert_eq!(
        s.processing_pending(&s.processing_policy().unwrap())
            .unwrap()
            .len(),
        3
    );
}
#[test]
fn geographic_choices_are_only_real_candidates_and_do_not_contain_coordinates() {
    let context = Context {
        message: json!({"text":"Fictional: fire in Springfield; governor speaking from elsewhere."}),
        neighbours: vec![],
        locations: vec![
            json!({"mention":"Springfield","quote":"fire in Springfield","candidates":[{"id":"fixture:a","name":"Springfield","country_code":"US"},{"id":"fixture:b","name":"Springfield","country_code":"AU"}]}),
        ],
        interpretation_id: Some("fixture:reading".into()),
        omitted_locations: 0,
        omitted_neighbours: 0,
    };
    let req = ai::request(&context).unwrap();
    assert_eq!(
        req["questions"]["geo_0"]["criteria"]
            .as_object()
            .unwrap()
            .len(),
        5
    );
    let mut res = response(&req);
    res["answers"]["geo_0"]["choice"] = json!("invented_place");
    assert!(ai::validate(&req, &res).is_err());
    assert_eq!(
        req["questions"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with("event_"))
            .count(),
        30
    );
}
#[test]
fn historical_publication_does_not_gain_future_suggestions() {
    let (_tmp, s) = workspace();
    let work = s.next_supplemental().unwrap().unwrap();
    let id = s.reserve_supplemental(&work).unwrap();
    s.finish_supplemental(&id, &audit(&work.request, &response(&work.request)), None)
        .unwrap();
    assert!(
        s.supplemental_detail(
            work.context.message["id"].as_str().unwrap(),
            "v1",
            None,
            true,
            &work.publication
        )
        .unwrap()
        .is_null()
    );
    assert!(
        s.supplemental_detail(
            work.context.message["id"].as_str().unwrap(),
            "v2",
            None,
            false,
            &work.publication
        )
        .unwrap()
        .is_null()
    );
}
#[tokio::test]
async fn ordinary_stop_cancels_supplemental_lane_without_a_fake_job() {
    let (_tmp, s) = workspace();
    let flag = Arc::new(AtomicBool::new(false));
    s.running
        .lock()
        .unwrap()
        .insert("processing:supplemental:test".into(), flag.clone());
    let runtime = relic_app::processing::Runtime::new(s.clone());
    assert!(runtime.is_running());
    runtime.stop().await.unwrap();
    assert!(flag.load(std::sync::atomic::Ordering::Acquire));
}
#[test]
fn baseline_reading_can_trigger_one_enrichment_without_rebilling_initial_context() {
    let (_tmp, s) = workspace();
    let work = s.next_supplemental().unwrap().unwrap();
    let id = s.reserve_supplemental(&work).unwrap();
    s.finish_supplemental(&id, &audit(&work.request, &response(&work.request)), None)
        .unwrap();
    let evidence = s
        .detail(work.context.message["id"].as_str().unwrap(), "v1")
        .unwrap();
    let output = json!({"language":"en","translation":null,"headline":"Fictional commentary","kind":"analysis","claim_status":"unclear","topic":"other","claims":[],"locations":[]});
    let checked =
        relic_app::processing_ai::validate_output(evidence["text"].as_str().unwrap(), &output)
            .unwrap();
    let resolver = relic_app::processing_geo::GeoResolver::load(&s.config.root).unwrap();
    let prepared = relic_app::processing_store::prepare(
        work.rowid,
        &evidence,
        &checked,
        &resolver,
        "fixture/gold",
    )
    .unwrap();
    s.publish_processing(
        vec![prepared],
        "fixture:processing",
        &AtomicBool::new(false),
    )
    .unwrap();
    let next = s.next_supplemental().unwrap().unwrap();
    assert_eq!(next.rowid, work.rowid);
    assert_eq!(next.phase, "enriched");
    let id = s.reserve_supplemental(&next).unwrap();
    s.finish_supplemental(&id, &audit(&next.request, &response(&next.request)), None)
        .unwrap();
    assert!(s.reserve_supplemental(&next).is_err());
    assert_ne!(s.next_supplemental().unwrap().unwrap().rowid, work.rowid);
}
#[test]
fn oversized_supplement_does_not_block_the_queue_or_skip_baseline() {
    let (_tmp, s) = workspace();
    let text = "a".repeat(16_000);
    // Controlled fixture enlargement, preserving its exact source text in JSON.
    s.connect()
        .unwrap()
        .execute(
            "UPDATE evidence SET text=?1,json=json_set(json,'$.text',?1) WHERE id='fiction:2'",
            [text],
        )
        .unwrap();
    assert!(s.next_supplemental().unwrap().is_none());
    assert_eq!(s.supplemental_status().unwrap()["request_excluded"], 1);
    let next = s.next_supplemental().unwrap().unwrap();
    assert_ne!(next.context.message["id"], "fiction:2");
    assert_eq!(s.supplemental_status().unwrap()["today"]["calls"], 0);
    assert_eq!(
        s.processing_pending(&s.processing_policy().unwrap())
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn invalid_paid_classification_does_not_pause_other_messages_but_provider_errors_do() {
    let (_tmp, s) = workspace();
    let work = s.next_supplemental().unwrap().unwrap();
    let id = s.reserve_supplemental(&work).unwrap();
    let mut result = response(&work.request);
    result["answers"]["kind"]["probabilities"]["uncertain"] = json!(0.2);
    s.finish_supplemental(&id, &audit(&work.request, &result), None)
        .unwrap();
    assert!(s.reserve_supplemental(&work).is_err());
    let next = s
        .next_supplemental()
        .unwrap()
        .expect("local validation cannot cool down unrelated messages");
    let next_id = s.reserve_supplemental(&next).unwrap();
    let mut receipt = audit(&next.request, &response(&next.request));
    receipt["http_status"] = json!(429);
    s.finish_supplemental(&next_id, &receipt, None).unwrap();
    assert!(s.next_supplemental().unwrap().is_none());
    assert!(
        Store::open(s.config.clone())
            .unwrap()
            .next_supplemental()
            .unwrap()
            .is_none()
    );
}
#[test]
fn media_only_messages_without_text_are_not_sent_to_jev() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let s = Store::open(Config::local(root, tmp.path().join("data"), "127.0.0.1:8793".parse().unwrap())).unwrap();
    let message = |id: &str, text: &str| json!({"id":id,"revision":"v1","source":"fiction:alpha","text":text,"published_at":"2026-09-10T12:00:00Z","acquired_at":"2026-09-10T12:05:00Z","geometry":null,"location_precision":"unresolved","origin_group":null,"dependence":"unknown","language":"und","fictional":true,"availability":"available","media":[{"kind":"photo"}],"citation":{"evidence_id":id,"revision":"v1","start_character":0,"end_character":text.chars().count()}});
    let import = |rows: Vec<Value>| s.import_job(json!({"schema_version":1,"fictional":true,"title":"Media-only fixture","sources":[{"id":"fiction:alpha","label":"Fictional Alpha","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
    // Imports require text; live collection stores a media-only post with empty text.
    import(vec![message("fiction:media", "placeholder")]);
    s.connect().unwrap().execute("UPDATE evidence SET text=' \n', json=json_set(json,'$.text',' \n') WHERE id='fiction:media'", []).unwrap();
    s.set_processing_policy(Policy { enabled: true, eligible_source_ids: vec!["fiction:alpha".into()] }).unwrap();
    s.set_supplemental_enabled(true).unwrap();
    assert!(s.next_supplemental().unwrap().is_none());
    import(vec![message("fiction:text", "Fictional fire reported in London.")]);
    assert_eq!(s.next_supplemental().unwrap().unwrap().context.message["id"], "fiction:text");
}
