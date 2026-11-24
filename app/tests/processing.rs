use relic_app::{
    config::Config,
    model::{Citation, Query, Scope, ident, value},
    processing_ai::validate_output,
    processing_geo::GeoResolver,
    processing_store::{self, Policy, Prepared},
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
        "127.0.0.1:8793".parse().unwrap(),
    ))
    .unwrap();
    (tmp, s)
}
#[test]
fn evaluation_activation_keeps_baseline_and_correction_gates_independent() {
    use relic_app::processing_ai::{self, DEFAULT_MODEL};
    let (_tmp, mut s) = workspace();
    s.config
        .source_values
        .insert("OPENROUTER_API_KEY".into(), "fictional".into());
    let baseline = json!({"passed":true,"fictional":true,"recipe":processing_store::RECIPE,"model":DEFAULT_MODEL,"prompt_version":processing_ai::PROMPT_VERSION,"request_profile":processing_ai::evaluation_profile(DEFAULT_MODEL,false)});
    let mut correction = baseline.clone();
    correction["correction_version"] = json!(processing_ai::CORRECTION_VERSION);
    correction["request_profile"] = processing_ai::evaluation_profile(DEFAULT_MODEL, true);
    s.record_processing_evaluation(&baseline).unwrap();
    assert!(!s.processing_correction_evaluated().unwrap());
    s.record_processing_evaluation(&correction).unwrap();
    assert!(s.processing_correction_evaluated().unwrap());
    assert_eq!(s.processing_evaluation().unwrap(), Some(baseline.clone()));
    s.record_processing_evaluation(&baseline).unwrap();
    assert!(s.processing_correction_evaluated().unwrap());
    correction["request_profile"]["decoder_version"] = json!("obsolete");
    assert!(s.record_processing_evaluation(&correction).is_err());
    correction["correction_version"] = json!(42);
    assert!(s.record_processing_evaluation(&correction).is_err());
    assert!(s.processing_correction_evaluated().unwrap());
}
fn row(id: &str, source: &str, text: &str) -> Value {
    json!({"id":id,"revision":"v1","source":source,"published_at":"2026-09-10T12:00:00Z","acquired_at":"2026-09-10T12:01:00Z","text":text,"geometry":null,"location_precision":"unresolved","origin_group":null,"dependence":"unknown","language":"en","fictional":true,"availability":"available","citation":{"evidence_id":id,"revision":"v1","start_character":0,"end_character":text.chars().count()}})
}
fn import(s: &Store, rows: Vec<Value>) -> Value {
    s.import_job(json!({"schema_version":1,"fictional":true,"title":"Fictional processing test","sources":[{"id":"fiction:alpha","label":"Fictional Alpha","family":"authored_report"},{"id":"fiction:beta","label":"Fictional Beta","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap()
}
fn query(s: &Store, scope: Scope) -> Value {
    s.query(Query {
        scope: value(scope),
        generation: 1,
        cursor: None,
        limit: Some(100),
    })
    .unwrap()
}
fn candidate(s: &Store, id: &str, kind: &str, state: &str, translation: Option<&str>) -> Prepared {
    let e = s.detail(id, "v1").unwrap();
    let text = e["text"].as_str().unwrap();
    let output = json!({"language":if translation.is_some(){"he"}else{"en"},"translation":translation,"headline":"Fictional source reports on London","kind":kind,"claim_status":state,"topic":"infrastructure","claims":[{"text":"Fictional claim","quote":text,"state":state}],"locations":[{"mention":"London","quote":text,"role":"event","canonical_name":"London","country_code":null}]});
    let checked = validate_output(text, &output).unwrap();
    let resolver = GeoResolver::load(&s.config.root).unwrap();
    let rowid = s
        .connect()
        .unwrap()
        .query_row(
            "SELECT rowid FROM evidence WHERE id=?1 AND revision='v1'",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    processing_store::prepare(rowid, &e, &checked, &resolver, "fixture/gold").unwrap()
}
#[test]
fn processing_preserves_originals_history_scope_citations_and_restore() {
    let (_tmp, s) = workspace();
    let old = import(
        &s,
        vec![
            row(
                "fiction:a",
                "fiction:alpha",
                "London: צוותים דיווחו על חסימה של 2 כבישים.",
            ),
            row(
                "fiction:b",
                "fiction:alpha",
                "London officials denied that any bridge had collapsed.",
            ),
            row(
                "fiction:c",
                "fiction:alpha",
                "London tours: buy a ticket today.",
            ),
        ],
    );
    // Unread counts only sources selected for reading; the rest are counted apart.
    let unselected = query(&s, Scope::default());
    assert_eq!(unselected["unread"], 0);
    assert_eq!(unselected["not_selected"], 3);
    let policy = s.set_source_reading("fiction:alpha", true).unwrap();
    assert_eq!(policy.eligible_source_ids, vec!["fiction:alpha".to_string()]);
    assert!(!policy.enabled, "selecting a source does not start reading");
    let unread = query(&s, Scope::default());
    assert_eq!(unread["unread"], 3);
    assert_eq!(unread["not_selected"], 0);
    assert_eq!(unread["media_only"], 0);
    let original = s.detail("fiction:a", "v1").unwrap();
    let candidates = vec![
        candidate(
            &s,
            "fiction:a",
            "event",
            "reported",
            Some("London: crews reported that 2 roads were blocked."),
        ),
        candidate(&s, "fiction:b", "statement", "denied", None),
        candidate(&s, "fiction:c", "promotion", "unclear", None),
    ];
    let publication = s
        .publish_processing(candidates, &ident("processing"), &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    let q = query(&s, Scope::default());
    let preview = s
        .query_preview(Query {
            scope: json!({}),
            generation: 1,
            cursor: None,
            limit: Some(100),
        })
        .unwrap();
    assert_eq!(preview["total"], q["total"]);
    assert_eq!(preview["publication_id"], q["publication_id"]);
    // The preview map is the full map without the per-point fields the map
    // never reads; every point, geometry and remaining property is unchanged.
    let mut map = q["map"].clone();
    for point in map["features"].as_array_mut().unwrap() {
        for key in ["message_key", "location_label", "approximate", "event_reports"] {
            point["properties"].as_object_mut().unwrap().remove(key);
        }
    }
    assert_eq!(preview["map"], map);
    assert!(!q["map"]["features"].as_array().unwrap().is_empty());
    for item in preview["items"].as_array().unwrap() {
        assert!(item["interpretation"].get("claims").is_none());
        assert_eq!(
            s.detail(item["id"].as_str().unwrap(), "v1").unwrap()["text"],
            item["text"]
        );
    }

    assert_eq!(q["unread"], 0);
    let a = q["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "fiction:a")
        .unwrap();
    assert_eq!(q["total"], 3);
    assert_eq!(q["located"], 1);
    assert_eq!(q["map_loaded"], 1);
    assert_eq!(
        q["map"]["features"][0]["properties"]["title"],
        a["interpretation"]["headline"]
    );
    assert_eq!(
        q["map"]["features"][0]["properties"]["published_at"],
        "2026-09-10T12:00:00.000Z"
    );
    assert_eq!(a["text"], original["text"]);
    assert_eq!(a["object_hash"], original["object_hash"]);
    assert_eq!(
        a["interpretation"]["location_precision"],
        "approximate_city"
    );
    assert!(s.detail("fiction:a", "v1").unwrap()["geometry"].is_null());
    let oldq = query(
        &s,
        Scope {
            publication_id: Some(old["publication_id"].as_str().unwrap().into()),
            ..Scope::default()
        },
    );
    assert_eq!(oldq["located"], 0);
    assert!(oldq["items"][0]["interpretation"].is_null());
    let signals = query(
        &s,
        Scope {
            content_view: "signals".into(),
            ..Scope::default()
        },
    );
    assert_eq!(signals["total"], 2);
    assert_eq!(signals["located"], 1);
    let translated = query(
        &s,
        Scope {
            query: "roads".into(),
            ..Scope::default()
        },
    );
    assert_eq!(translated["total"], 1);
    let bbox = query(
        &s,
        Scope {
            bbox: Some([-1., 50., 1., 53.]),
            include_unlocated: false,
            ..Scope::default()
        },
    );
    assert_eq!(bbox["total"], 1);
    assert_eq!(bbox["map_loaded"], 1);
    let cit: Citation =
        serde_json::from_value(a["interpretation"]["claims"][0]["citation"].clone()).unwrap();
    let scope = Scope {
        publication_id: Some(publication.clone()),
        ..Scope::default()
    };
    let support = Store::check_citations(
        &s.connect().unwrap(),
        std::slice::from_ref(&cit),
        Some(&scope),
    )
    .unwrap();
    assert_eq!(support[0]["quote"], original["text"]);
    assert_eq!(
        support[0]["interpretation"]["translation"],
        "London: crews reported that 2 roads were blocked."
    );
    let notebook=s.create_investigation(json!({"title":"Fictional processed brief","question":"What is supported?","scope":scope})).unwrap();
    let brief=s.save_brief(json!({"title":"Fictional cited report","question":"What is supported?","investigation_id":notebook["id"],"finding":"The source reports two blocked roads; this is unverified.","assessment":"unverified","limitations":["Fictional test"],"citations":[cit],"scope":scope,"publication_id":publication})).unwrap();
    let exported = s
        .export(brief["id"].as_str().unwrap(), brief["revision"].as_str())
        .unwrap();
    assert_eq!(
        exported["support"][0]["interpretation"]["id"],
        a["interpretation"]["id"]
    );
    assert_eq!(exported["scope_membership"].as_array().unwrap().len(), 3);
    let lineage = s
        .lineage(Some(a["interpretation"]["id"].as_str().unwrap()), 3, "both")
        .unwrap();
    assert!(
        lineage["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["kind"] == "interpretation")
    );
    let backup = s.backup().unwrap();
    assert!(s.verify_backup(backup["id"].as_str().unwrap()).is_ok());
}
#[test]
fn related_reporting_respects_selected_sources_without_claiming_confirmation() {
    let (_tmp, s) = workspace();
    import(
        &s,
        vec![
            row(
                "fiction:a",
                "fiction:alpha",
                "London reports a road closure.",
            ),
            row(
                "fiction:b",
                "fiction:beta",
                "London reports a different road closure.",
            ),
        ],
    );
    s.publish_processing(
        vec![
            candidate(&s, "fiction:a", "event", "reported", None),
            candidate(&s, "fiction:b", "event", "reported", None),
        ],
        &ident("processing"),
        &AtomicBool::new(false),
    )
    .unwrap();
    let q = query(&s, Scope::default());
    assert_eq!(q["items"][0]["interpretation"]["related_count"], 1);
    assert_eq!(q["unknown_origins"], 2);
    assert_eq!(q["origin_groups"], 0);
    let q = query(
        &s,
        Scope {
            source_ids: vec!["fiction:alpha".into()],
            ..Scope::default()
        },
    );
    assert_eq!(q["items"][0]["interpretation"]["related_count"], 0);
    let key = q["items"][0]["interpretation"]["related_key"]
        .as_str()
        .unwrap()
        .to_string();
    let q = query(
        &s,
        Scope {
            related_key: Some(key),
            ..Scope::default()
        },
    );
    assert_eq!(q["total"], 2);
}
#[test]
fn later_import_carries_only_unchanged_revision_interpretations() {
    let (_tmp, s) = workspace();
    import(
        &s,
        vec![
            row(
                "fiction:a",
                "fiction:alpha",
                "London reports a road closure.",
            ),
            row(
                "fiction:b",
                "fiction:beta",
                "London reports a second closure.",
            ),
        ],
    );
    let old = s
        .publish_processing(
            vec![
                candidate(&s, "fiction:a", "event", "reported", None),
                candidate(&s, "fiction:b", "event", "reported", None),
            ],
            &ident("processing"),
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
    let mut revised = row(
        "fiction:a",
        "fiction:alpha",
        "London has corrected its report.",
    );
    revised["revision"] = json!("v2");
    revised["citation"]["revision"] = json!("v2");
    revised["acquired_at"] = json!("2026-09-10T12:05:00Z");
    import(&s, vec![revised]);
    let q = query(&s, Scope::default());
    let items = q["items"].as_array().unwrap();
    assert!(items.iter().find(|e| e["id"] == "fiction:a").unwrap()["interpretation"].is_null());
    assert!(items.iter().find(|e| e["id"] == "fiction:b").unwrap()["interpretation"].is_object());
    assert_eq!(
        query(
            &s,
            Scope {
                publication_id: Some(old),
                ..Scope::default()
            }
        )["located"],
        2
    );
}
#[test]
fn cancellation_and_conflicting_publication_leave_last_view_usable() {
    let (_tmp, s) = workspace();
    let old = import(
        &s,
        vec![row(
            "fiction:a",
            "fiction:alpha",
            "London reports a road closure.",
        )],
    );
    assert_eq!(
        s.publish_processing(
            vec![candidate(&s, "fiction:a", "event", "reported", None)],
            &ident("processing"),
            &AtomicBool::new(true)
        )
        .unwrap_err()
        .code,
        "cancelled"
    );
    assert_eq!(
        query(&s, Scope::default())["publication_id"],
        old["publication_id"]
    );
    let mut wrong = candidate(&s, "fiction:a", "event", "reported", None);
    wrong.interpretation["reference_edition"] = json!("missing-retired-context");
    assert_eq!(
        s.publish_processing(vec![wrong], &ident("processing"), &AtomicBool::new(false))
            .unwrap_err()
            .code,
        "conflict"
    );
    let mut wrong = candidate(&s, "fiction:a", "event", "reported", None);
    wrong.interpretation["revision"] = json!("missing");
    assert!(
        s.publish_processing(vec![wrong], &ident("processing"), &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(
        query(&s, Scope::default())["publication_id"],
        old["publication_id"]
    );
}
#[test]
fn unknown_billing_and_source_eligibility_survive_restart() {
    let (_tmp, s) = workspace();
    import(
        &s,
        vec![row(
            "fiction:a",
            "fiction:alpha",
            "London reports a road closure.",
        )],
    );
    assert!(s.processing_pending(&Policy::default()).unwrap().is_empty());
    let p = Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into()],
    };
    s.set_processing_policy(p.clone()).unwrap();
    let grant: String = s
        .connect()
        .unwrap()
        .query_row(
            "SELECT value FROM metadata WHERE key='processing_eligibility'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let grant: Value = serde_json::from_str(&grant).unwrap();
    assert_eq!(grant["source_ids"], json!(["fiction:alpha"]));
    assert_eq!(grant["destination"], "OpenRouter");
    assert!(grant["checked_at"].is_string());
    let rowid = s.processing_pending(&p).unwrap()[0].rowid;
    let attempt = s.reserve_processing(rowid).unwrap();
    assert!(s.reserve_processing(rowid).is_err());
    s.finish_processing_attempt(
        &attempt,
        "failed",
        None,
        &Value::Null,
        Some("Fictional uncertain request"),
    )
    .unwrap();
    let reopened = Store::open(s.config.clone()).unwrap();
    assert!(reopened.reserve_processing(rowid).is_err());
    assert!(
        reopened.claim_processing_receipt().unwrap().is_none(),
        "no lookup or guessed charge without an identifier"
    );
    assert!(reopened.processing_pending(&p).unwrap().is_empty());
    assert_eq!(
        reopened.processing_status(false).unwrap()["today"]["cost_usd"],
        Value::Null
    );
    assert!(
        reopened
            .set_processing_policy(Policy {
                enabled: true,
                eligible_source_ids: vec!["missing".into()]
            })
            .is_err()
    );
    assert!(reopened.set_source_reading("missing", true).is_err());
    // Deselecting the last selected source pauses reading; an enabled policy
    // with nothing to read is not kept.
    let paused = reopened.set_source_reading("fiction:alpha", false).unwrap();
    assert!(paused.eligible_source_ids.is_empty() && !paused.enabled);
}

#[test]
fn typed_events_preserve_each_claim_and_share_map_feed_scope() {
    let (_tmp, s) = workspace();
    let mut fixture: Value =
        serde_json::from_str(include_str!("../../examples/processing-events.json")).unwrap();
    let taxonomy: Value =
        serde_json::from_str(include_str!("../../examples/processing-taxonomy.json")).unwrap();
    fixture["cases"]
        .as_array_mut()
        .unwrap()
        .extend(taxonomy["cases"].as_array().unwrap().iter().cloned());
    let cases = fixture["cases"].as_array().unwrap();
    let imported = import(
        &s,
        cases
            .iter()
            .map(|case| {
                row(
                    &format!("fiction:{}", case["id"].as_str().unwrap()),
                    "fiction:alpha",
                    case["original"].as_str().unwrap(),
                )
            })
            .collect(),
    );
    let resolver = GeoResolver::load(&s.config.root).unwrap();
    let mut prepared = vec![];
    for case in cases {
        let e = s
            .detail(&format!("fiction:{}", case["id"].as_str().unwrap()), "v1")
            .unwrap();
        let checked = validate_output(e["text"].as_str().unwrap(), &case["gold"]).unwrap();
        let key = s
            .message_reference(e["message_key"].as_str().unwrap())
            .unwrap();
        assert_eq!(key["id"], e["id"]);
        let rowid = e["message_key"].as_str().unwrap().parse().unwrap();
        prepared.push(
            processing_store::prepare(rowid, &e, &checked, &resolver, "fixture/gold").unwrap(),
        );
    }
    s.publish_processing(prepared, &ident("processing"), &AtomicBool::new(false))
        .unwrap();
    let all = query(&s, Scope::default());
    assert_eq!(all["total"], 41);
    let find = |name: &str| {
        all["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == format!("fiction:{name}"))
            .unwrap()
    };
    let mixed = find("event_mixed_states");
    assert_eq!(mixed["interpretation"]["location_label"], "London");
    assert_eq!(mixed["interpretation"]["events"][0]["type"], "strike");
    assert_eq!(mixed["interpretation"]["events"][1]["state"], "denied");
    assert!(mixed["interpretation"]["events"][1]["geometry"].is_null());
    let multi = find("event_two_sites");
    assert_eq!(multi["map_locations"].as_array().unwrap().len(), 2);
    assert!(multi["geometry"].is_null());
    assert_eq!(
        multi["interpretation"]["events"][0]["location_label"],
        "London"
    );
    assert_eq!(
        multi["interpretation"]["events"][1]["location_label"],
        "Paris"
    );
    assert_eq!(
        find("event_speaker_location")["interpretation"]["location_label"],
        "Paris"
    );
    for name in [
        "event_denial_ar",
        "event_plan_fa",
        "event_history",
        "event_unclear_he",
        "event_non_event",
    ] {
        assert!(find(name)["geometry"].is_null(), "{name}");
    }
    let events = query(
        &s,
        Scope {
            content_view: "events".into(),
            ..Scope::default()
        },
    );
    assert_eq!(events["total"], 32);
    for kind in relic_app::processing_ai::EVENT_TYPES {
        let filtered = query(
            &s,
            Scope {
                content_view: "events".into(),
                event_type: Some((*kind).into()),
                ..Scope::default()
            },
        );
        let expected = cases
            .iter()
            .filter(|case| {
                case["gold"]["claims"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c["event_type"] == *kind && c["state"] == "reported")
            })
            .count();
        assert_eq!(filtered["total"], expected, "{kind}");
        let activity: u64 = filtered["timeline"]["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["count"].as_u64().unwrap())
            .sum();
        assert_eq!(
            activity + filtered["timeline"]["undated"].as_u64().unwrap(),
            expected as u64,
            "timeline {kind}"
        );
        assert_eq!(filtered["items"].as_array().unwrap().len(), expected);
        let mapped_messages = filtered["map"]["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["properties"]["evidence_id"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            mapped_messages.len() as u64,
            filtered["located"].as_u64().unwrap()
        );
        for f in filtered["map"]["features"].as_array().unwrap() {
            assert!(
                filtered["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["id"] == f["properties"]["evidence_id"])
            );
        }
    }
    assert_eq!(
        query(
            &s,
            Scope {
                content_view: "events".into(),
                event_type: Some("armed_attack".into()),
                ..Scope::default()
            }
        )["total"],
        1
    );
    assert_eq!(
        query(
            &s,
            Scope {
                event_type: Some("armed_attack".into()),
                ..Scope::default()
            }
        )["total"],
        2
    );
    let attack_scope = Scope {
        event_type: Some("armed_attack".into()),
        ..Scope::default()
    };
    let attacks = query(&s, attack_scope.clone());
    assert_eq!(attacks["total"], 2);
    assert_eq!(attacks["located"], 1);
    assert!(
        attacks["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "fiction:event_mixed_states")
            .unwrap()["geometry"]
            .is_null()
    );
    let located_attacks = query(
        &s,
        Scope {
            include_unlocated: false,
            ..attack_scope
        },
    );
    assert_eq!(located_attacks["total"], 1);
    let old = query(
        &s,
        Scope {
            publication_id: Some(imported["publication_id"].as_str().unwrap().into()),
            ..Scope::default()
        },
    );
    assert_eq!(old["total"], 41);
    assert!(
        old["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["interpretation"].is_null())
    );
    assert!(Scope::parse(&json!({"event_type":"bogus"})).is_err());
    for event in mixed["interpretation"]["events"].as_array().unwrap() {
        let c: Citation = serde_json::from_value(event["citation"].clone()).unwrap();
        let retained = Store::check_citations(&s.connect().unwrap(), &[c], None).unwrap();
        assert_eq!(retained[0]["quote"], event["quote"]);
    }
}

#[test]
fn event_place_and_time_support_cannot_escape_the_claim() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../examples/processing-events.json")).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "event_mixed_states")
        .unwrap();
    let text = case["original"].as_str().unwrap();
    for patch in [
        json!({"location_indices":[1]}),
        json!({"location_indices":[0,0]}),
        json!({"location_indices":[5]}),
        json!({"time_text":"yesterday"}),
        json!({"event_type":"invented"}),
        json!({"event_type":null}),
    ] {
        let mut output = case["gold"].clone();
        for (key, value) in patch.as_object().unwrap() {
            output["claims"][0][key] = value.clone();
        }
        assert!(validate_output(text, &output).is_err(), "{patch}");
    }
}

#[test]
fn an_unresolved_second_event_cannot_be_collapsed_into_the_first_city() {
    let (_tmp, s) = workspace();
    let text = "Fictional: a fire broke out in London. Another fire broke out in Fictional Bay.";
    let output = json!({"language":"en","translation":null,"headline":"Two separate fictional fires","kind":"event","claim_status":"reported","topic":"disaster","claims":[{"text":"Fire in London","quote":"a fire broke out in London.","state":"reported","event_type":"fire","time_text":null,"location_indices":[0]},{"text":"Another fire in Fictional Bay","quote":"Another fire broke out in Fictional Bay.","state":"reported","event_type":"fire","time_text":null,"location_indices":[1]}],"locations":[{"mention":"London","quote":"a fire broke out in London.","role":"event","canonical_name":"London","country_code":null},{"mention":"Fictional Bay","quote":"Another fire broke out in Fictional Bay.","role":"event","canonical_name":"Fictional Bay","country_code":null}]});
    let checked = validate_output(text, &output).unwrap();
    let resolver = GeoResolver::load(&s.config.root).unwrap();
    let prepared = processing_store::prepare(
        1,
        &row("fiction:separate", "fiction:alpha", text),
        &checked,
        &resolver,
        "fixture/gold",
    )
    .unwrap();
    assert!(prepared.interpretation["geometry"].is_null());
    assert_eq!(
        prepared.interpretation["events"][0]["location_label"],
        "London"
    );
    assert!(prepared.interpretation["events"][1]["geometry"].is_null());
}

#[test]
fn reviewed_taxonomy_preserves_one_shared_vocabulary_and_exact_support() {
    let vocabulary: Value =
        serde_json::from_str(include_str!("../../web/src/event-taxonomy.json")).unwrap();
    let ids = vocabulary["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, relic_app::processing_ai::EVENT_TYPES);
    let fixtures: Value =
        serde_json::from_str(include_str!("../../examples/processing-taxonomy.json")).unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let checked = validate_output(case["original"].as_str().unwrap(), &case["gold"]).unwrap();
        assert_eq!(
            serde_json::to_value(&checked.output.claims).unwrap(),
            case["gold"]["claims"]
        );
        assert_eq!(
            serde_json::to_value(&checked.output.topic).unwrap(),
            case["gold"]["topic"]
        );
        assert!(checked.output.locations.is_empty());
    }
}

#[test]
fn paid_response_recovery_keeps_receipts_and_never_repays_or_accepts_bad_quotes() {
    use relic_app::processing_ai::{self, DEFAULT_MODEL};
    let (_tmp, mut s) = workspace();
    s.config
        .source_values
        .insert("OPENROUTER_API_KEY".into(), "fictional-no-network".into());
    let fixture: Value =
        serde_json::from_str(include_str!("../../examples/processing-ai.json")).unwrap();
    let original = fixture["cases"][0]["original"].as_str().unwrap();
    import(
        &s,
        vec![
            row("fiction:recover", "fiction:alpha", original),
            row("fiction:invalid", "fiction:alpha", original),
            row("fiction:held", "fiction:alpha", original),
        ],
    );
    s.set_processing_policy(Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into()],
    })
    .unwrap();
    let mut attempts = vec![];
    for id in ["fiction:recover", "fiction:invalid", "fiction:held"] {
        let mut output = fixture["cases"][0]["gold"].clone();
        output["translation"] =
            json!("Discard this English rewrite; it must never become the reading.");
        if id.ends_with("invalid") {
            output["claims"][0]["quote"] = json!("A sentence that was never in the source.");
        }
        let raw = serde_json::to_vec(&json!({"id":"gen-fictional-recovery","model":DEFAULT_MODEL,"provider":"fictional","choices":[{"finish_reason":"stop","message":{"role":"assistant","content":output.to_string()}}],"usage":{"cost":0.001}})).unwrap();
        let result = processing_ai::replay_response(original, DEFAULT_MODEL, 200, raw, 10);
        let audit = match result {
            Ok(a) => a.audit,
            Err(e) => *e.audit.unwrap(),
        };
        let rowid = s.detail(id, "v1").unwrap()["message_key"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let attempt = s.reserve_processing(rowid).unwrap();
        s.finish_processing_attempt(
            &attempt,
            "failed",
            Some(0.001),
            &value(&audit),
            Some("original rejection"),
        )
        .unwrap();
        if id.ends_with("held") {
            s.connect().unwrap().execute("UPDATE processing_attempts SET json=json_set(json,'$.recovery_review',json(?2)) WHERE id=?1",rusqlite::params![attempt,json!({"decision":"hold","reason":"Fictional semantic review"}).to_string()]).unwrap();
        }
        attempts.push((attempt, rowid, value(&audit)));
    }
    assert!(s.recover_processing(&AtomicBool::new(true)).is_err());
    assert_eq!(
        s.recover_processing(&AtomicBool::new(false)).unwrap(),
        json!({"examined":2,"recovered":1,"still_rejected":1,"model_calls":0})
    );
    assert_eq!(
        s.recover_processing(&AtomicBool::new(false)).unwrap()["examined"],
        0
    );
    let (count, cost): (i64, f64) = s
        .connect()
        .unwrap()
        .query_row(
            "SELECT count(*),sum(cost_usd) FROM processing_attempts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 3);
    assert_eq!(cost, 0.003);
    assert!(s.cached_processing(attempts[2].1).unwrap().is_none());
    let saved: String = s
        .connect()
        .unwrap()
        .query_row(
            "SELECT json FROM processing_attempts WHERE id=?1",
            [&attempts[0].0],
            |r| r.get(0),
        )
        .unwrap();
    let saved: Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(saved["recovery"]["original_error"], "original rejection");
    let old: Value = serde_json::from_slice(
        &s.original_object(saved["recovery"]["original_audit_hash"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(old, attempts[0].2);
    let cached = s.cached_processing(attempts[0].1).unwrap().unwrap();
    assert!(cached.extracted.output.translation.is_none());
    assert!(
        cached
            .audit
            .conservative_normalizations
            .contains(&"english_translation_discarded_original_retained".into())
    );
    assert!(s.cached_processing(attempts[1].1).unwrap().is_none());
    let held: String = s
        .connect()
        .unwrap()
        .query_row(
            "SELECT json FROM processing_attempts WHERE id=?1",
            [&attempts[1].0],
            |r| r.get(0),
        )
        .unwrap();
    let held: Value = serde_json::from_str(&held).unwrap();
    assert_eq!(held["error"], "original rejection");
    assert_eq!(held["replay_rejection"]["kind"], "unsupported_quote");
    assert_eq!(held["replay_rejection"]["detail"], "claims[0].quote");
    // A later rejected correction must not strand a baseline recovered from
    // retained bytes, even if geography has changed since the original call.
    s.connect().unwrap().execute("UPDATE processing_attempts SET json=json_set(json,'$.recipe_key','message-events-v6/geo:previous') WHERE id=?1",[&attempts[0].0]).unwrap();
    s.connect().unwrap().execute("INSERT INTO processing_attempts SELECT 'fiction:later-correction',evidence_rowid,day,created_at,'failed',reserved_usd,cost_usd,json_set(json,'$.recipe_key','message-events-v6/correction-v8','$.recovery',NULL) FROM processing_attempts WHERE id=?1",[&attempts[0].0]).unwrap();
    assert_eq!(
        s.processing_pending(&s.processing_policy().unwrap())
            .unwrap()
            .len(),
        1
    );
    let evidence = s.detail("fiction:recover", "v1").unwrap();
    let mut p = processing_store::prepare(
        attempts[0].1,
        &evidence,
        &cached.extracted,
        &GeoResolver::load(&s.config.root).unwrap(),
        DEFAULT_MODEL,
    )
    .unwrap();
    p.audit = value(&cached.audit);
    s.publish_processing(vec![p], &ident("recovered"), &AtomicBool::new(false))
        .unwrap();
    assert!(
        s.processing_pending(&s.processing_policy().unwrap())
            .unwrap()
            .is_empty()
    );
    assert_eq!(s.detail("fiction:recover", "v1").unwrap()["text"], original);
}

#[test]
fn a_geography_change_does_not_rebill_a_failed_semantic_recipe() {
    let (_tmp, s) = workspace();
    import(
        &s,
        vec![row(
            "fiction:failed-geo",
            "fiction:alpha",
            "Fictional report from London.",
        )],
    );
    let policy = Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into()],
    };
    let rowid = s.processing_pending(&policy).unwrap()[0].rowid;
    let attempt = s.reserve_processing(rowid).unwrap();
    s.finish_processing_attempt(
        &attempt,
        "failed",
        Some(0.001),
        &json!({}),
        Some("Fictional rejected output"),
    )
    .unwrap();
    s.connect()
        .unwrap()
        .execute(
            "INSERT INTO metadata VALUES('active_geography',?1)",
            [json!({"edition":"fictional-next-geography"}).to_string()],
        )
        .unwrap();
    assert!(s.processing_pending(&policy).unwrap().is_empty());
    s.connect()
        .unwrap()
        .execute(
            "UPDATE processing_attempts SET json=json_set(json,'$.recipe_key',?1) WHERE id=?2",
            [
                format!("{}/geo:fictional-old-geography", processing_store::RECIPE),
                attempt.clone(),
            ],
        )
        .unwrap();
    assert!(s.processing_pending(&policy).unwrap().is_empty());
    // A deliberately upgraded semantic recipe is independently eligible.
    s.connect().unwrap().execute("UPDATE processing_attempts SET json=json_set(json,'$.recipe_key','older-semantic-recipe/geo:fictional-old-geography') WHERE id=?1",[attempt]).unwrap();
    assert_eq!(s.processing_pending(&policy).unwrap().len(), 1);
}

#[test]
fn receipt_recovery_survives_cancel_restart_and_never_rebills_or_changes_outcome() {
    let (_tmp, s) = workspace();
    import(
        &s,
        vec![row(
            "fiction:receipt",
            "fiction:alpha",
            "London reports a road closure.",
        )],
    );
    let policy = Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into()],
    };
    let rowid = s.processing_pending(&policy).unwrap()[0].rowid;
    let attempt = s.reserve_processing(rowid).unwrap();
    s.save_processing_generation(&attempt, "gen-fixture-receipt", "fixture/model")
        .unwrap();
    assert!(
        s.save_processing_generation(&attempt, "gen-other", "fixture/model")
            .is_err()
    );
    s.finish_processing_attempt(
        &attempt,
        "unknown",
        None,
        &Value::Null,
        Some("Cancelled fictional request"),
    )
    .unwrap();
    let restarted = Store::open(s.config.clone()).unwrap();
    let request = restarted.claim_processing_receipt().unwrap().unwrap();
    assert_eq!(request.generation_id, "gen-fixture-receipt");
    assert!(
        restarted.claim_processing_receipt().unwrap().is_none(),
        "paced across repeated worker ticks"
    );
    let mut receipt = json!({"data":{"id":"gen-wrong","model":"fixture/model","cancelled":true,"total_cost":0.003}});
    assert!(
        restarted
            .apply_processing_receipt(&request, &receipt)
            .is_err()
    );
    receipt["data"]["id"] = json!("gen-fixture-receipt");
    restarted
        .apply_processing_receipt(&request, &receipt)
        .unwrap();
    restarted
        .apply_processing_receipt(&request, &receipt)
        .unwrap();
    assert!(
        restarted.processing_pending(&policy).unwrap().is_empty(),
        "cancelled message must not be automatically rebilled"
    );
    let (status, cost, raw): (String, f64, String) = restarted
        .connect()
        .unwrap()
        .query_row(
            "SELECT status,cost_usd,json FROM processing_attempts WHERE id=?1",
            [&attempt],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "unknown");
    assert_eq!(cost, 0.003);
    let record: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(record["error"], "Cancelled fictional request");
    let retained: Value = serde_json::from_slice(
        &restarted
            .original_object(record["billing_receipt_hash"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(retained, receipt);
    let backup = restarted.backup().unwrap();
    let restored = restarted
        .verify_backup(backup["id"].as_str().unwrap())
        .unwrap();
    let restored_db = rusqlite::Connection::open(
        std::path::Path::new(restored["restored_directory"].as_str().unwrap())
            .join("relic.sqlite3"),
    )
    .unwrap();
    let restored_record: String = restored_db
        .query_row(
            "SELECT json FROM processing_attempts WHERE id=?1",
            [&attempt],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&restored_record).unwrap(),
        record
    );
    receipt["data"]["total_cost"] = json!(0.0);
    assert!(
        restarted
            .apply_processing_receipt(&request, &receipt)
            .is_err()
    );
}

#[test]
fn multiple_sites_keep_message_membership_exact_across_scope_and_history() {
    let (_tmp, s) = workspace();
    let fixture: Value =
        serde_json::from_str(include_str!("../../examples/processing-multisite.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    let imported = import(
        &s,
        cases
            .iter()
            .map(|case| {
                let mut e = row(
                    &format!("fiction:{}", case["id"].as_str().unwrap()),
                    "fiction:alpha",
                    case["original"].as_str().unwrap(),
                );
                if !case["source_geometry"].is_null() {
                    e["geometry"] = case["source_geometry"].clone();
                    e["location_precision"] = json!("source_supplied");
                }
                e
            })
            .collect(),
    );
    let resolver = GeoResolver::load(&s.config.root).unwrap();
    let prepared = cases
        .iter()
        .map(|case| {
            let e = s
                .detail(&format!("fiction:{}", case["id"].as_str().unwrap()), "v1")
                .unwrap();
            let checked = validate_output(e["text"].as_str().unwrap(), &case["gold"]).unwrap();
            processing_store::prepare(
                e["message_key"].as_str().unwrap().parse().unwrap(),
                &e,
                &checked,
                &resolver,
                "fixture/gold",
            )
            .unwrap()
        })
        .collect();
    let publication = s
        .publish_processing(prepared, &ident("processing"), &AtomicBool::new(false))
        .unwrap();
    let all = query(&s, Scope::default());
    assert_eq!(all["total"], 4);
    assert_eq!(all["located"], 4);
    assert_eq!(all["map_loaded"], 5);
    assert_eq!(all["map_truncated"], false);
    assert_eq!(
        all["timeline"]["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["count"].as_u64().unwrap())
            .sum::<u64>(),
        4
    );
    for (id, sites) in [
        ("different_types", 2),
        ("same_city", 1),
        ("partial", 1),
        ("source_geotag", 1),
    ] {
        let detail = s
            .detail_with_publication(&format!("fiction:{id}"), "v1", publication.as_deref())
            .unwrap();
        assert_eq!(
            detail["map_locations"].as_array().unwrap().len(),
            sites,
            "{id}"
        );
        let original = s.detail(&format!("fiction:{id}"), "v1").unwrap();
        assert_eq!(detail["object_hash"], original["object_hash"]);
        if id == "partial" {
            assert!(detail["interpretation"]["events"][1]["geometry"].is_null());
        }
    }
    let paris = Scope {
        bbox: Some([2., 48., 3., 49.]),
        include_unlocated: false,
        ..Scope::default()
    };
    let q = query(&s, paris.clone());
    assert_eq!(q["total"], 1);
    assert_eq!(q["map_loaded"], 1);
    assert_eq!(q["items"][0]["map_locations"][0]["label"], "Paris");
    assert_eq!(
        query(
            &s,
            Scope {
                event_type: Some("fire".into()),
                ..paris.clone()
            }
        )["total"],
        0
    );
    assert_eq!(
        query(
            &s,
            Scope {
                event_type: Some("explosion".into()),
                ..paris.clone()
            }
        )["total"],
        1
    );
    assert_eq!(
        query(
            &s,
            Scope {
                source_ids: vec!["fiction:beta".into()],
                ..paris.clone()
            }
        )["total"],
        0
    );
    assert_eq!(
        query(
            &s,
            Scope {
                from: Some("2026-09-11T00:00:00Z".into()),
                ..paris
            }
        )["total"],
        0
    );
    let dateline = query(
        &s,
        Scope {
            bbox: Some([170., 0., -170., 20.]),
            include_unlocated: false,
            ..Scope::default()
        },
    );
    assert_eq!(dateline["total"], 1);
    assert_eq!(dateline["items"][0]["id"], "fiction:source_geotag");
    assert_eq!(
        dateline["map"]["features"][0]["properties"]["approximate"],
        false
    );
    // A source geotag is not the occurrence location of a planned strike.
    assert_eq!(
        query(
            &s,
            Scope {
                event_type: Some("strike".into()),
                include_unlocated: false,
                ..Scope::default()
            }
        )["total"],
        0
    );
    let old = query(
        &s,
        Scope {
            publication_id: Some(imported["publication_id"].as_str().unwrap().into()),
            ..Scope::default()
        },
    );
    assert_eq!(old["located"], 1);
    assert_eq!(old["map_loaded"], 1);
    // Simulate a pre-index workspace: reopening must derive all supported sites
    // and event filters from immutable readings, including zero-site records.
    s.connect().unwrap().execute_batch("DELETE FROM interpretation_sites; DELETE FROM interpretation_event_states; DELETE FROM interpretation_geography_indexed;").unwrap();
    let reopened = Store::open(s.config.clone()).unwrap();
    assert_eq!(query(&reopened, Scope::default())["map"], all["map"]);
    assert_eq!(
        query(
            &reopened,
            Scope {
                event_type: Some("explosion".into()),
                ..Scope::default()
            }
        )["total"],
        2
    );
    let page = s
        .query(Query {
            scope: value(Scope::default()),
            generation: 1,
            cursor: None,
            limit: Some(1),
        })
        .unwrap();
    assert_eq!(page["map"], all["map"]);
    assert_eq!(page["total"], 4);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let backup = s.backup().unwrap();
    let verified = s.verify_backup(backup["id"].as_str().unwrap()).unwrap();
    let mut restored_config = s.config.clone();
    restored_config.data = verified["restored_directory"].as_str().unwrap().into();
    let restored = Store::open(restored_config).unwrap();
    assert_eq!(query(&restored, Scope::default())["map"], all["map"]);
}

#[test]
fn unresolved_receipts_do_not_stop_other_messages_or_fabricate_cost() {
    let (_tmp, s) = workspace();
    import(
        &s,
        (0..110)
            .map(|n| {
                row(
                    &format!("fiction:reserve-{n}"),
                    "fiction:alpha",
                    "A fictional report.",
                )
            })
            .collect(),
    );
    let policy = Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into()],
    };
    let mut admitted = 0;
    loop {
        let rows = s.processing_pending(&policy).unwrap();
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let id = s.reserve_processing(row.rowid).unwrap();
            s.finish_processing_attempt(
                &id,
                "unknown",
                None,
                &Value::Null,
                Some("Fictional interruption"),
            )
            .unwrap();
            admitted += 1;
        }
    }
    assert_eq!(admitted, 110);
    let reopened = Store::open(s.config.clone()).unwrap();
    let status = reopened.processing_status(false).unwrap();
    assert!(status["today"]["cost_usd"].is_null());
    assert_eq!(status["today"]["known_cost_usd"], 0.0);
    assert_eq!(status["today"]["unknown_cost_requests"], 110);
    assert!(status["today"]["admission_reserved_usd"].as_f64().unwrap() > 5.0);
    assert!(status["today"]["limit"].is_null());
    assert!(status["today"]["spend_limit_usd"].is_null());
    assert!(
        reopened.processing_pending(&policy).unwrap().is_empty(),
        "unknown attempts are not automatically rebilled"
    );
    import(
        &reopened,
        vec![row(
            "fiction:new",
            "fiction:alpha",
            "A new fictional report.",
        )],
    );
    let next = reopened.processing_pending(&policy).unwrap();
    assert_eq!(next.len(), 1);
    reopened.reserve_processing(next[0].rowid).unwrap();
}

#[test]
fn untranslated_echo_repairs_are_targeted_once_and_keep_originals() {
    let (_tmp, s) = workspace();
    let original = "London: צוותים דיווחו על חסימה של 2 כבישים.";
    import(
        &s,
        vec![
            row("fiction:echo", "fiction:alpha", original),
            row(
                "fiction:good",
                "fiction:alpha",
                "London reports a road closure.",
            ),
        ],
    );
    let mut old = candidate(
        &s,
        "fiction:echo",
        "event",
        "reported",
        Some("London: crews reported that 2 roads were blocked."),
    );
    let rowid = old.rowid;
    // A legacy accepted reading with a copied rather than translated body.
    old.interpretation["translation"] = json!(original);
    let good = candidate(&s, "fiction:good", "event", "reported", None);
    let publication = s
        .publish_processing(
            vec![old, good],
            &ident("processing"),
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
    let policy = Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into()],
    };
    let pending = s.processing_pending(&policy).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].rowid, rowid);
    let attempt = s.reserve_processing(rowid).unwrap();
    assert!(s.reserve_processing(rowid).is_err());
    assert!(s.processing_pending(&policy).unwrap().is_empty());
    s.finish_processing_attempt(
        &attempt,
        "failed",
        Some(0.001),
        &Value::Null,
        Some("Fictional rejected repair"),
    )
    .unwrap();
    let reopened = Store::open(s.config.clone()).unwrap();
    assert!(
        reopened.processing_pending(&policy).unwrap().is_empty(),
        "failed repair must not loop or rebill"
    );
    let old = s
        .detail_with_publication("fiction:echo", "v1", Some(publication.as_str()))
        .unwrap();
    assert_eq!(old["text"], original);
    assert_eq!(old["interpretation"]["translation"], original);
    assert!(!relic_app::processing_ai::translation_is_echo(
        "NATO",
        "und",
        Some("NATO")
    ));
    assert!(!relic_app::processing_ai::translation_is_echo(
        "Café",
        "fr",
        Some("Café")
    ));
}

#[test]
fn translation_repair_publishes_a_distinct_version_without_reprocessing_good_readings() {
    let (_tmp, s) = workspace();
    let original = "London: צוותים דיווחו על חסימה של 2 כבישים.";
    import(&s, vec![row("fiction:echo", "fiction:alpha", original)]);
    let mut old = candidate(
        &s,
        "fiction:echo",
        "event",
        "reported",
        Some("London: crews reported that 2 roads were blocked."),
    );
    let prior = old.interpretation["id"].clone();
    old.interpretation["translation"] = json!(original);
    let old_publication = s
        .publish_processing(vec![old], &ident("processing"), &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    let policy = Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into()],
    };
    s.set_processing_policy(policy.clone()).unwrap();
    let pending = s.processing_pending(&policy).unwrap();
    assert_eq!(pending[0].repair_of.as_deref(), prior.as_str());
    let mut repaired = candidate(
        &s,
        "fiction:echo",
        "event",
        "reported",
        Some("London: crews reported that 2 roads were blocked."),
    );
    repaired.interpretation["reading_revision"] = json!("translation-repair-v1");
    repaired.interpretation["replaces_reading"] = prior;
    s.publish_processing(
        vec![repaired],
        &ident("processing"),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(s.processing_pending(&policy).unwrap().is_empty());
    assert_eq!(s.processing_status(false).unwrap()["needs_upgrade"], 0);
    assert_eq!(
        s.detail_with_publication("fiction:echo", "v1", None)
            .unwrap()["interpretation"]["translation"],
        "London: crews reported that 2 roads were blocked."
    );
    assert_eq!(
        s.detail_with_publication("fiction:echo", "v1", Some(&old_publication))
            .unwrap()["interpretation"]["translation"],
        original
    );
}

#[test]
fn corrections_are_paid_once_and_queue_counts_exclude_unavailable_messages() {
    use relic_app::processing_ai::{self, DEFAULT_MODEL};
    let (_tmp, mut s) = workspace();
    s.config
        .source_values
        .insert("OPENROUTER_API_KEY".into(), "fictional-never-used".into());
    let original = "Fictional report: A fire in London today.";
    let good = json!({"language":"en","translation":null,"headline":"Fictional source reports a fire in London","kind":"event","claim_status":"reported","topic":"disaster","claims":[{"text":"A fictional source reports a fire in London today.","quote":original,"state":"reported","event_type":"fire","time_text":"today","location_indices":[0]}],"locations":[{"mention":"London","quote":original,"role":"event","canonical_name":"London","country_code":null}]});
    let mut bad = good.clone();
    bad["claims"][0]["quote"] = json!("An invented supporting passage.");
    let decode = |output: &Value| {
        processing_ai::replay_response(original, DEFAULT_MODEL,200,serde_json::to_vec(&json!({"model":DEFAULT_MODEL,"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":output.to_string()}}],"usage":{"cost":0.001}})).unwrap(),1)
    };
    let rejected = decode(&bad).err().unwrap();
    let error_text = rejected.to_string();
    let mut rejected_audit = value(rejected.audit.as_ref().unwrap());
    rejected_audit["prompt_version"] = json!(processing_ai::PREVIOUS_PROMPT_VERSION);
    let truncated = processing_ai::replay_response(original, DEFAULT_MODEL, 200,
        serde_json::to_vec(&json!({"model":DEFAULT_MODEL,"choices":[{"finish_reason":"length","message":{"role":"assistant","content":"{"}}],"usage":{"cost":0.001}})).unwrap(), 1).err().unwrap();
    let truncated_audit = value(truncated.audit.as_ref().unwrap());
    let truncated_error = truncated.to_string();
    let mut removed = row("fiction:removed", "fiction:alpha", original);
    removed["upstream_state"] = json!("removed");
    import(
        &s,
        vec![
            row("fiction:correct", "fiction:alpha", original),
            row("fiction:still-bad", "fiction:alpha", original),
            row("fiction:unknown", "fiction:alpha", original),
            row("fiction:fresh", "fiction:alpha", original),
            row("fiction:media", "fiction:alpha", "media placeholder"),
            row("fiction:off", "fiction:beta", original),
            row("fiction:large", "fiction:alpha", &"x".repeat(16001)),
            removed,
        ],
    );
    // Acquisition supports media-only rows; the JSON import fixture requires text.
    s.connect()
        .unwrap()
        .execute(
            "UPDATE evidence SET text='',json=json_set(json,'$.text','') WHERE id='fiction:media'",
            [],
        )
        .unwrap();
    s.connect().unwrap().execute("INSERT INTO metadata VALUES('processing_correction_evaluation',?1)",[json!({"passed":true,"fictional":true,"recipe":processing_store::RECIPE,"model":DEFAULT_MODEL,"prompt_version":processing_ai::PROMPT_VERSION,"correction_version":processing_ai::CORRECTION_VERSION,"request_profile":processing_ai::evaluation_profile(DEFAULT_MODEL,true)}).to_string()]).unwrap();
    let policy = Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into()],
    };
    s.set_processing_policy(policy.clone()).unwrap();
    let mut ids = vec![];
    for id in ["fiction:correct", "fiction:still-bad", "fiction:unknown"] {
        let rowid: i64 = s.detail(id, "v1").unwrap()["message_key"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let attempt = s.reserve_processing(rowid).unwrap();
        s.finish_processing_attempt(
            &attempt,
            "failed",
            if id.ends_with("unknown") {
                None
            } else {
                Some(0.001)
            },
            if id.ends_with("still-bad") {
                &truncated_audit
            } else {
                &rejected_audit
            },
            Some(if id.ends_with("still-bad") {
                &truncated_error
            } else {
                &error_text
            }),
        )
        .unwrap();
        ids.push((rowid, attempt));
    }
    // Baseline evaluation alone must never enable a new corrective prompt.
    let c = s.connect().unwrap();
    let gate: String = c
        .query_row(
            "SELECT value FROM metadata WHERE key='processing_correction_evaluation'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    c.execute(
        "DELETE FROM metadata WHERE key='processing_correction_evaluation'",
        [],
    )
    .unwrap();
    assert_eq!(s.processing_status(false).unwrap()["queued"], 1);
    assert_eq!(s.processing_status(false).unwrap()["failed"], 3);
    let failures = s.processing_failures(None).unwrap();
    assert_eq!(failures["items"].as_array().unwrap().len(), 3);
    let last = failures["items"].as_array().unwrap().last().unwrap()["message_key"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    assert!(
        s.processing_failures(Some(last)).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        s.reserve_processing_correction(ids[0].0, &ids[0].1)
            .is_err()
    );
    c.execute(
        "INSERT INTO metadata VALUES('processing_correction_evaluation',?1)",
        [gate],
    )
    .unwrap();
    let status = s.processing_status(false).unwrap();
    assert_eq!(status["queued"], 3);
    assert_eq!(status["corrections_queued"], 2);
    assert_eq!(status["failed"], 1);
    assert_eq!(
        status["unavailable"],
        json!({"without_text":1,"removed":1,"ineligible":1,"oversized":1})
    );
    let pending = s.processing_pending(&policy).unwrap();
    assert_eq!(pending.len(), 3);
    assert_eq!(
        pending[0].evidence["id"], "fiction:fresh",
        "fresh messages take precedence"
    );
    assert!(
        s.reserve_processing_correction(ids[2].0, &ids[2].1)
            .is_err(),
        "unknown cost cannot authorize a correction"
    );
    let correction = s
        .reserve_processing_correction(ids[0].0, &ids[0].1)
        .unwrap();
    assert!(
        s.reserve_processing_correction(ids[0].0, &ids[0].1)
            .is_err()
    );
    assert_eq!(s.processing_status(true).unwrap()["in_flight"], 1);
    let valid = decode(&good).unwrap();
    s.finish_processing_attempt(&correction, "completed", Some(0.001), &value(&valid), None)
        .unwrap();
    assert!(s.cached_processing(ids[0].0).unwrap().is_some());
    let e = s.detail("fiction:correct", "v1").unwrap();
    let candidate = processing_store::prepare(
        ids[0].0,
        &e,
        &valid.extracted,
        &GeoResolver::load(&s.config.root).unwrap(),
        DEFAULT_MODEL,
    )
    .unwrap();
    s.publish_processing(
        vec![candidate],
        &ident("correction"),
        &AtomicBool::new(false),
    )
    .unwrap();
    let second = s
        .reserve_processing_correction(ids[1].0, &ids[1].1)
        .unwrap();
    s.finish_processing_attempt(
        &second,
        "failed",
        Some(0.001),
        &rejected_audit,
        Some(&error_text),
    )
    .unwrap();
    let reopened = Store::open(s.config.clone()).unwrap();
    let remaining = reopened.processing_pending(&policy).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].evidence["id"], "fiction:fresh");
    assert!(
        reopened
            .reserve_processing_correction(ids[1].0, &ids[1].1)
            .is_err()
    );
    assert!(
        reopened
            .reserve_processing_correction(ids[0].0, &ids[0].1)
            .is_err()
    );
    assert_eq!(
        reopened.detail("fiction:correct", "v1").unwrap()["text"],
        original
    );
    let final_status = reopened.processing_status(false).unwrap();
    assert_eq!(final_status["processed"], 1);
    assert_eq!(final_status["failed"], 2);
    assert_eq!(final_status["queued"], 1);
    assert_eq!(final_status["today"]["calls"], 5);
    let record: String = reopened
        .connect()
        .unwrap()
        .query_row(
            "SELECT json FROM processing_attempts WHERE id=?1",
            [correction],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&record).unwrap()["correction_of"],
        ids[0].1
    );
}

#[test]
fn mixed_prose_translation_repairs_reach_older_messages_and_never_loop() {
    let (_tmp, mut s) = workspace();
    s.config
        .source_values
        .insert("OPENROUTER_API_KEY".into(), "fictional-no-network".into());
    let original = "Fictional report: a warehouse fire was contained by crews in London.\n\nگزارش فرضی: آتش‌سوزی در یک انبار رخ داد و آتش‌نشانان آتش را مهار کردند.";
    let mut rows = vec![row("fiction:mixed", "fiction:alpha", original)];
    rows.extend((0..110).map(|n| {
        row(
            &format!("fiction:good-{n}"),
            "fiction:alpha",
            "London reports a warehouse fire that crews have contained.\nعلی",
        )
    }));
    import(&s, rows);
    let mut old = candidate(
        &s,
        "fiction:mixed",
        "event",
        "reported",
        Some("Fictional report: a warehouse fire was contained by crews in London."),
    );
    old.interpretation["language"] = json!("en");
    old.interpretation["translation"] = Value::Null;
    let rowid = old.rowid;
    let output = json!({"language":"mul","translation":"A fictional fire in London was contained.","headline":"Fictional fire contained","kind":"statement","claim_status":"reported","topic":"disaster","claims":[],"locations":[]});
    let response=serde_json::to_vec(&json!({"model":relic_app::processing_ai::DEFAULT_MODEL,"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":output.to_string()}}],"usage":{"cost":0.001}})).unwrap();
    let mut legacy = relic_app::processing_ai::replay_response(
        original,
        relic_app::processing_ai::DEFAULT_MODEL,
        200,
        response,
        1,
    )
    .unwrap();
    legacy.extracted.output.language = "en".into();
    legacy.extracted.output.translation = None;
    let old_attempt = s.reserve_processing(rowid).unwrap();
    s.finish_processing_attempt(
        &old_attempt,
        "completed",
        Some(0.001),
        &value(&legacy),
        None,
    )
    .unwrap();
    assert!(
        s.cached_processing(rowid).unwrap().is_none(),
        "the legacy English-labelled cache must not bypass repair"
    );
    let mut candidates = vec![old];
    candidates.extend(
        (0..110).map(|n| candidate(&s, &format!("fiction:good-{n}"), "event", "reported", None)),
    );
    let publication = s
        .publish_processing(candidates, &ident("processing"), &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    let policy = Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into()],
    };
    s.set_processing_policy(policy.clone()).unwrap();
    assert_eq!(s.processing_status(false).unwrap()["queued"], 1);
    let pending = s.processing_pending(&policy).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].rowid, rowid);
    assert!(pending[0].repair_of.is_some());
    let attempt = s.reserve_processing(rowid).unwrap();
    assert_eq!(s.processing_status(true).unwrap()["in_flight"], 1);
    assert!(s.reserve_processing(rowid).is_err());
    s.finish_processing_attempt(
        &attempt,
        "unknown",
        None,
        &Value::Null,
        Some("Fictional unknown receipt"),
    )
    .unwrap();
    assert!(s.processing_pending(&policy).unwrap().is_empty());
    assert_eq!(s.processing_status(false).unwrap()["failed"], 1);
    let reopened = Store::open(s.config.clone()).unwrap();
    assert!(reopened.processing_pending(&policy).unwrap().is_empty());
    let historic = reopened
        .detail_with_publication("fiction:mixed", "v1", Some(&publication))
        .unwrap();
    assert_eq!(historic["text"], original);
    assert!(historic["interpretation"]["translation"].is_null());
}

#[test]
fn accounted_deadline_gets_one_evaluated_correction_but_uncertain_cost_does_not() {
    use relic_app::processing_ai::{self, DEFAULT_MODEL};
    let (_tmp, mut s) = workspace();
    s.config
        .source_values
        .insert("OPENROUTER_API_KEY".into(), "fictional".into());
    import(
        &s,
        vec![row(
            "fiction:deadline",
            "fiction:alpha",
            "Fictional bridge inspection in London.",
        )],
    );
    let policy = Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into()],
    };
    s.set_processing_policy(policy.clone()).unwrap();
    s.connect().unwrap().execute("INSERT INTO metadata VALUES('processing_correction_evaluation',?1)", [json!({"passed":true,"fictional":true,"recipe":processing_store::RECIPE,"model":DEFAULT_MODEL,"prompt_version":processing_ai::PROMPT_VERSION,"correction_version":processing_ai::CORRECTION_VERSION,"request_profile":processing_ai::evaluation_profile(DEFAULT_MODEL,true)}).to_string()]).unwrap();
    let rowid = s.processing_pending(&policy).unwrap()[0].rowid;
    let attempt = s.reserve_processing(rowid).unwrap();
    s.save_processing_generation(&attempt, "gen-fictional-deadline", DEFAULT_MODEL)
        .unwrap();
    s.finish_processing_attempt(
        &attempt,
        "failed",
        None,
        &Value::Null,
        Some("Relic stopped waiting after its 120-second model request deadline"),
    )
    .unwrap();
    assert!(s.processing_pending(&policy).unwrap().is_empty());
    assert!(s.reserve_processing_correction(rowid, &attempt).is_err());
    let request = s.claim_processing_receipt().unwrap().unwrap();
    s.apply_processing_receipt(&request,&json!({"data":{"id":"gen-fictional-deadline","model":DEFAULT_MODEL,"cancelled":false,"finish_reason":"stop","total_cost":0.001}})).unwrap();
    assert_eq!(s.processing_pending(&policy).unwrap().len(), 1);
    let correction = s.reserve_processing_correction(rowid, &attempt).unwrap();
    assert!(s.reserve_processing_correction(rowid, &attempt).is_err());
    s.finish_processing_attempt(
        &correction,
        "failed",
        None,
        &Value::Null,
        Some("Relic stopped waiting after its 120-second model request deadline"),
    )
    .unwrap();
    assert!(s.processing_pending(&policy).unwrap().is_empty());
}

#[test]
fn no_response_audit_does_not_block_recovery_of_other_paid_responses() {
    use relic_app::processing_ai::{self, DEFAULT_MODEL};
    let (_tmp, mut s) = workspace();
    s.config
        .source_values
        .insert("OPENROUTER_API_KEY".into(), "fictional".into());
    let fixture: Value =
        serde_json::from_str(include_str!("../../examples/processing-ai.json")).unwrap();
    let text = fixture["cases"][0]["original"].as_str().unwrap();
    import(
        &s,
        vec![
            row("fiction:null", "fiction:alpha", text),
            row("fiction:recover", "fiction:alpha", text),
        ],
    );
    s.set_processing_policy(Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into()],
    })
    .unwrap();
    for (index, id) in ["fiction:null", "fiction:recover"].iter().enumerate() {
        let rowid = s.detail(id, "v1").unwrap()["message_key"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let attempt = s.reserve_processing(rowid).unwrap();
        let audit = if index == 0 {
            Value::Null
        } else {
            let raw=serde_json::to_vec(&json!({"model":DEFAULT_MODEL,"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":fixture["cases"][0]["gold"].to_string()}}],"usage":{"cost":0.001}})).unwrap();
            value(
                processing_ai::replay_response(text, DEFAULT_MODEL, 200, raw, 1)
                    .unwrap()
                    .audit,
            )
        };
        s.finish_processing_attempt(
            &attempt,
            "failed",
            Some(0.),
            &audit,
            Some("fixture failure"),
        )
        .unwrap();
    }
    let result = s.recover_processing(&AtomicBool::new(false)).unwrap();
    assert_eq!(result["examined"], 2);
    assert_eq!(result["recovered"], 1);
    assert_eq!(
        s.recover_processing(&AtomicBool::new(false)).unwrap()["examined"],
        0
    );
}

#[test]
fn explicit_rate_rejections_requeue_with_durable_cooldown_but_uncertain_errors_do_not() {
    use relic_app::processing_ai::{self, DEFAULT_MODEL};
    let (_tmp, s) = workspace();
    let original = "Fictional report of a fire.";
    import(&s, vec![row("fiction:limit", "fiction:alpha", original)]);
    let policy = Policy {
        enabled: true,
        eligible_source_ids: vec!["fiction:alpha".into()],
    };
    s.set_processing_policy(policy.clone()).unwrap();
    let rowid = s.processing_pending(&policy).unwrap()[0].rowid;
    let rejected = |status, body: Value| {
        processing_ai::replay_response(
            original,
            DEFAULT_MODEL,
            status,
            serde_json::to_vec(&body).unwrap(),
            1,
        )
        .err()
        .unwrap()
    };
    let mut error = rejected(
        429,
        json!({"error":{"code":429,"message":"Fictional rate limit"}}),
    );
    error.retry_after_seconds = Some(3600);
    let attempt = s.reserve_processing(rowid).unwrap();
    s.defer_processing_attempt(&attempt, &error).unwrap();
    assert_eq!(s.processing_pending(&policy).unwrap().len(), 1);
    assert_eq!(s.processing_status(false).unwrap()["failed"], 0);
    assert_eq!(
        s.processing_status(false).unwrap()["today"]["unknown_cost_requests"],
        0
    );
    assert_eq!(s.processing_backoff().unwrap()["concurrency"], 32);
    assert!(
        s.processing_backoff().unwrap()["retry_at"]
            .as_str()
            .unwrap()
            > relic_app::model::now().as_str()
    );
    let restarted = Store::open(s.config.clone()).unwrap();
    assert_eq!(
        s.processing_backoff().unwrap(),
        restarted.processing_backoff().unwrap()
    );
    let next = s.reserve_processing(rowid).unwrap();
    assert!(s.reserve_processing(rowid).is_err());
    for (status, body) in [
        (500, json!({"error":{"message":"unknown"}})),
        (429, json!({"error":{},"id":"gen-maybe-charged"})),
        (429, json!({"error":{},"usage":{"cost":0.01}})),
        (429, json!({"error":{},"choices":[]})),
    ] {
        let uncertain = rejected(status, body);
        assert!(!uncertain.rejected_before_generation());
        assert!(s.defer_processing_attempt(&next, &uncertain).is_err());
    }
    // Several in-flight requests can be rejected during one provider window.
    // That window reduces admission once, rather than once per response.
    s.defer_processing_attempt(&next, &error).unwrap();
    assert_eq!(s.processing_backoff().unwrap()["concurrency"], 32);
    let later = s.reserve_processing(rowid).unwrap();
    s.connect().unwrap().execute(
        "UPDATE metadata SET value=json_set(value,'$.retry_at','2000-01-01T00:00:00.000Z') WHERE key='processing_backoff'",
        [],
    ).unwrap();
    s.defer_processing_attempt(&later, &error).unwrap();
    assert_eq!(s.processing_backoff().unwrap()["concurrency"], 16);
}

#[test]
fn identical_text_revision_reuses_only_the_same_identity_and_active_request_profile() {
    use relic_app::processing_ai::{self, DEFAULT_MODEL};
    let (_tmp, mut s) = workspace();
    s.config
        .source_values
        .insert("OPENROUTER_API_KEY".into(), "fictional".into());
    let fixture: Value =
        serde_json::from_str(include_str!("../../examples/processing-ai.json")).unwrap();
    let original = fixture["cases"][0]["original"].as_str().unwrap();
    import(&s, vec![row("fiction:reuse", "fiction:alpha", original)]);
    let rowid = s.detail("fiction:reuse", "v1").unwrap()["message_key"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let raw=serde_json::to_vec(&json!({"model":DEFAULT_MODEL,"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":fixture["cases"][0]["gold"].to_string()}}],"usage":{"cost":0.001}})).unwrap();
    let mut analysis =
        processing_ai::replay_response(original, DEFAULT_MODEL, 200, raw, 1).unwrap();
    analysis.audit.parameters["request_profile"] =
        processing_ai::evaluation_profile(DEFAULT_MODEL, false);
    let attempt = s.reserve_processing(rowid).unwrap();
    s.finish_processing_attempt(&attempt, "completed", Some(0.001), &value(&analysis), None)
        .unwrap();
    let mut revision = row("fiction:reuse", "fiction:alpha", original);
    revision["revision"] = json!("v2");
    revision["citation"]["revision"] = json!("v2");
    import(
        &s,
        vec![
            revision,
            row("fiction:different", "fiction:alpha", original),
        ],
    );
    let key = |id, rev| {
        s.detail(id, rev).unwrap()["message_key"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap()
    };
    assert!(
        s.cached_processing(key("fiction:different", "v1"))
            .unwrap()
            .is_none()
    );
    let reused = s
        .cached_processing(key("fiction:reuse", "v2"))
        .unwrap()
        .unwrap();
    assert_eq!(value(&reused.extracted), value(&analysis.extracted));
    assert_eq!(reused.audit.parameters["reused_from"]["revision"], "v1");
    assert_eq!(reused.audit.parameters["reused_from"]["model_calls"], 0);
    let mut changed = row(
        "fiction:reuse",
        "fiction:alpha",
        "An actually changed fictional report.",
    );
    changed["revision"] = json!("v3");
    changed["citation"]["revision"] = json!("v3");
    import(&s, vec![changed]);
    assert!(
        s.cached_processing(key("fiction:reuse", "v3"))
            .unwrap()
            .is_none()
    );
    let count: i64 = s
        .connect()
        .unwrap()
        .query_row("SELECT count(*) FROM processing_attempts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let v2 = key("fiction:reuse", "v2");
    s.config.source_values.insert(
        "AI_DEFAULT_MODEL".into(),
        "deepseek/deepseek-v4-flash-0731".into(),
    );
    assert!(s.cached_processing(v2).unwrap().is_none());
}

#[test]
fn promotions_mentioning_an_occurrence_stay_out_of_events() {
    let (_tmp, s) = workspace();
    let report = "Fictional: a drone strike hit a warehouse in London this morning.";
    let advert = "Fictional: subscribe now for our film on the London warehouse strike.";
    import(&s, vec![row("fiction:report", "fiction:alpha", report), row("fiction:advert", "fiction:alpha", advert)]);
    let resolver = GeoResolver::load(&s.config.root).unwrap();
    let prepared = [("fiction:report", report, "event"), ("fiction:advert", advert, "promotion")].map(|(id, text, kind)| {
        let e = s.detail(id, "v1").unwrap();
        let gold = json!({"language":"en","translation":null,"headline":text,"kind":kind,"claim_status":"reported","topic":"conflict","claims":[{"text":text,"quote":text,"state":"reported","event_type":"strike","time_text":null,"location_indices":[0]}],"locations":[{"mention":"London","canonical_name":"London","quote":text,"role":"event","country_code":null}]});
        let checked = validate_output(text, &gold).unwrap();
        let rowid = e["message_key"].as_str().unwrap().parse().unwrap();
        processing_store::prepare(rowid, &e, &checked, &resolver, "fixture/gold").unwrap()
    });
    s.publish_processing(prepared.into(), &ident("processing"), &AtomicBool::new(false)).unwrap();
    let ids = |scope: Scope| query(&s, scope)["items"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    assert_eq!(ids(Scope { content_view: "events".into(), ..Scope::default() }), vec!["fiction:report"]);
    assert_eq!(ids(Scope { content_view: "events".into(), event_type: Some("strike".into()), ..Scope::default() }), vec!["fiction:report"]);
    assert_eq!(ids(Scope::default()).len(), 2);
}

#[test]
fn interrupted_request_is_held_until_one_explicit_retry() {
    let (_tmp, s) = workspace();
    import(&s, vec![row("fiction:cut", "fiction:alpha", "Fictional crews report a road closure in London.")]);
    let policy = Policy { enabled: true, eligible_source_ids: vec!["fiction:alpha".into()] };
    s.set_processing_policy(policy.clone()).unwrap();
    let rowid: i64 = s.connect().unwrap().query_row("SELECT rowid FROM evidence WHERE id='fiction:cut'", [], |r| r.get(0)).unwrap();
    let cut = |s: &Store| {
        let attempt = s.reserve_processing(rowid).unwrap();
        s.finish_processing_attempt(&attempt, "unknown", None, &Value::Null, Some("Processing cancelled during a request; billing is unknown and no retry was sent")).unwrap();
    };
    cut(&s);
    // Held: no automatic retry of a request whose billing is unknown.
    assert!(s.processing_pending(&policy).unwrap().is_empty());
    assert_eq!(s.processing_status(false).unwrap()["interrupted"], 1);
    assert_eq!(s.authorize_interrupted_retry().unwrap(), 1);
    assert_eq!(s.processing_pending(&policy).unwrap().len(), 1);
    assert_eq!(s.processing_status(false).unwrap()["interrupted"], 0);
    // The ledger keeps the original attempt; a retry cut off again is held again.
    cut(&s);
    assert!(s.processing_pending(&policy).unwrap().is_empty());
    assert_eq!(s.processing_status(false).unwrap()["interrupted"], 1);
    let attempts: i64 = s.connect().unwrap().query_row("SELECT count(*) FROM processing_attempts WHERE evidence_rowid=?1", [rowid], |r| r.get(0)).unwrap();
    assert_eq!(attempts, 2);
}

#[test]
fn an_authorized_retry_of_an_interrupted_correction_is_that_correction() {
    use relic_app::processing_ai::{self, DEFAULT_MODEL};
    let (_tmp, mut s) = workspace();
    s.config
        .source_values
        .insert("OPENROUTER_API_KEY".into(), "fictional-never-used".into());
    let original = "Fictional report: A fire in London today.";
    let bad = json!({"language":"en","translation":null,"headline":"Fictional source reports a fire in London","kind":"event","claim_status":"reported","topic":"disaster","claims":[{"text":"A fictional source reports a fire in London today.","quote":"An invented supporting passage.","state":"reported","event_type":"fire","time_text":"today","location_indices":[0]}],"locations":[{"mention":"London","quote":original,"role":"event","canonical_name":"London","country_code":null}]});
    let rejected = processing_ai::replay_response(original, DEFAULT_MODEL, 200, serde_json::to_vec(&json!({"model":DEFAULT_MODEL,"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":bad.to_string()}}],"usage":{"cost":0.001}})).unwrap(), 1).err().unwrap();
    let error_text = rejected.to_string();
    let rejected_audit = value(rejected.audit.as_ref().unwrap());
    import(&s, vec![row("fiction:cut-correction", "fiction:alpha", original)]);
    s.connect().unwrap().execute("INSERT INTO metadata VALUES('processing_correction_evaluation',?1)",[json!({"passed":true,"fictional":true,"recipe":processing_store::RECIPE,"model":DEFAULT_MODEL,"prompt_version":processing_ai::PROMPT_VERSION,"correction_version":processing_ai::CORRECTION_VERSION,"request_profile":processing_ai::evaluation_profile(DEFAULT_MODEL,true)}).to_string()]).unwrap();
    let policy = Policy { enabled: true, eligible_source_ids: vec!["fiction:alpha".into()] };
    s.set_processing_policy(policy.clone()).unwrap();
    let rowid: i64 = s.connect().unwrap().query_row("SELECT rowid FROM evidence WHERE id='fiction:cut-correction'", [], |r| r.get(0)).unwrap();
    let first = s.reserve_processing(rowid).unwrap();
    s.finish_processing_attempt(&first, "failed", Some(0.001), &rejected_audit, Some(&error_text)).unwrap();
    let correctable = |s: &Store| {
        let pending = s.processing_pending(&policy).unwrap();
        assert!(pending.len() <= 1);
        pending.first().and_then(|p| p.correction_of.clone())
    };
    assert_eq!(correctable(&s).as_deref(), Some(first.as_str()));
    let cut = |s: &Store| {
        let correction = s.reserve_processing_correction(rowid, &first).unwrap();
        assert!(s.reserve_processing_correction(rowid, &first).is_err());
        s.finish_processing_attempt(&correction, "unknown", None, &Value::Null, Some("Processing cancelled during a request; billing is unknown and no retry was sent")).unwrap();
        correction
    };
    let correction = cut(&s);
    // Held: an unknown outcome is never retried automatically, and a later
    // reconciled cost does not authorize anything by itself.
    assert!(s.processing_pending(&policy).unwrap().is_empty());
    assert_eq!(s.processing_interrupted().unwrap(), 1);
    s.connect().unwrap().execute("UPDATE processing_attempts SET cost_usd=0.0005 WHERE id=?1", [&correction]).unwrap();
    assert!(s.processing_pending(&policy).unwrap().is_empty());
    assert!(s.reserve_processing_correction(rowid, &first).is_err());
    // The explicit retry is the one correction, not a second baseline request.
    assert_eq!(s.authorize_interrupted_retry().unwrap(), 1);
    assert_eq!(s.processing_interrupted().unwrap(), 0);
    assert_eq!(correctable(&s).as_deref(), Some(first.as_str()));
    assert!(s.reserve_processing(rowid).is_err());
    // A retry cut off again is held again until authorized again.
    cut(&s);
    assert!(s.processing_pending(&policy).unwrap().is_empty());
    assert_eq!(s.processing_interrupted().unwrap(), 1);
    assert_eq!(s.authorize_interrupted_retry().unwrap(), 1);
    assert_eq!(correctable(&s).as_deref(), Some(first.as_str()));
    // A definite correction outcome spends the allowance.
    let retry = s.reserve_processing_correction(rowid, &first).unwrap();
    s.finish_processing_attempt(&retry, "failed", Some(0.001), &rejected_audit, Some(&error_text)).unwrap();
    let reopened = Store::open(s.config.clone()).unwrap();
    assert!(reopened.processing_pending(&policy).unwrap().is_empty());
    assert!(reopened.reserve_processing_correction(rowid, &first).is_err());
    assert_eq!(reopened.processing_interrupted().unwrap(), 0);
    let attempts: i64 = reopened.connect().unwrap().query_row("SELECT count(*) FROM processing_attempts WHERE evidence_rowid=?1", [rowid], |r| r.get(0)).unwrap();
    assert_eq!(attempts, 4);
}

#[test]
fn reading_reservation_left_by_an_exit_becomes_a_retryable_interruption() {
    let (_tmp, s) = workspace();
    import(&s, vec![row("fiction:exit", "fiction:alpha", "Fictional crews report a bridge closure in London.")]);
    let policy = Policy { enabled: true, eligible_source_ids: vec!["fiction:alpha".into()] };
    s.set_processing_policy(policy.clone()).unwrap();
    let rowid: i64 = s.connect().unwrap().query_row("SELECT rowid FROM evidence WHERE id='fiction:exit'", [], |r| r.get(0)).unwrap();
    s.reserve_processing(rowid).unwrap();
    assert_eq!(s.processing_status(false).unwrap()["in_flight"], 1);
    let reopened = Store::open(s.config.clone()).unwrap();
    let status = reopened.processing_status(false).unwrap();
    assert_eq!(status["in_flight"], 0);
    assert_eq!(status["interrupted"], 1);
    assert_eq!(reopened.authorize_interrupted_retry().unwrap(), 1);
    assert_eq!(reopened.processing_pending(&policy).unwrap().len(), 1);
}

#[test]
fn repeated_status_polls_follow_every_attempt_and_publication_change() {
    let (_tmp, s) = workspace();
    import(&s, vec![
        row("fiction:poll-a", "fiction:alpha", "Fictional crews report a road closure in London."),
        row("fiction:poll-b", "fiction:alpha", "Fictional crews report a bridge closure in London."),
        row("fiction:poll-c", "fiction:alpha", "Fictional crews report a tunnel closure in London."),
        row("fiction:poll-off", "fiction:beta", "Fictional crews report a ferry closure in London."),
    ]);
    let policy = Policy { enabled: true, eligible_source_ids: vec!["fiction:alpha".into()] };
    s.set_processing_policy(policy.clone()).unwrap();
    let rowid = |id: &str| -> i64 { s.detail(id, "v1").unwrap()["message_key"].as_str().unwrap().parse().unwrap() };
    let counts = |status: &Value| {
        ["pending", "queued", "in_flight", "failed", "interrupted", "processed"]
            .map(|key| status[key].as_i64().unwrap())
    };
    // Each poll must agree with the dispatcher's queue, the interrupted count
    // and the failure list, none of which go through the status memo.
    let poll = |s: &Store, policy: &Policy| {
        let status = s.processing_status(false).unwrap();
        assert_eq!(status["interrupted"], s.processing_interrupted().unwrap());
        assert_eq!(status["queued"].as_u64().unwrap() as usize, s.processing_pending(policy).unwrap().len());
        let failures = s.processing_failures(None).unwrap();
        assert_eq!(status["failed"].as_u64().unwrap() as usize, failures["items"].as_array().unwrap().len());
        status
    };
    let first = poll(&s, &policy);
    assert_eq!(counts(&first), [4, 3, 0, 0, 0, 0]);
    assert_eq!(poll(&s, &policy), first, "an unchanged workspace returns the same status");

    // A reservation, then its outcome, change one message's state.
    let a = s.reserve_processing(rowid("fiction:poll-a")).unwrap();
    assert_eq!(counts(&poll(&s, &policy)), [4, 2, 1, 0, 0, 0]);
    let rejection = "The model output did not match the bounded analysis schema";
    s.finish_processing_attempt(&a, "failed", Some(0.001), &Value::Null, Some(rejection)).unwrap();
    let failed = poll(&s, &policy);
    assert_eq!(counts(&failed), [4, 2, 0, 1, 0, 0]);
    assert_eq!(failed["last_error"], rejection);

    // An unknown outcome is held. Authorizing its retry edits only the attempt
    // JSON, and that alone must release the message.
    let b = s.reserve_processing(rowid("fiction:poll-b")).unwrap();
    s.finish_processing_attempt(&b, "unknown", None, &Value::Null, Some("Processing cancelled during a request; billing is unknown and no retry was sent")).unwrap();
    assert_eq!(counts(&poll(&s, &policy)), [4, 1, 0, 2, 1, 0]);
    assert_eq!(s.authorize_interrupted_retry().unwrap(), 1);
    assert_eq!(counts(&poll(&s, &policy)), [4, 2, 0, 1, 0, 0]);

    // A processing publication gives one message a current reading.
    let reading = candidate(&s, "fiction:poll-c", "event", "reported", None);
    s.publish_processing(vec![reading], &ident("processing"), &AtomicBool::new(false)).unwrap().unwrap();
    assert_eq!(counts(&poll(&s, &policy)), [3, 1, 0, 1, 0, 1]);

    // A collection publication adds a message; a policy change reclassifies all.
    import(&s, vec![row("fiction:poll-d", "fiction:alpha", "Fictional crews report a rail closure in London.")]);
    assert_eq!(counts(&poll(&s, &policy)), [4, 2, 0, 1, 0, 1]);
    let wider = Policy { enabled: true, eligible_source_ids: vec!["fiction:alpha".into(), "fiction:beta".into()] };
    s.set_processing_policy(wider.clone()).unwrap();
    let status = poll(&s, &wider);
    assert_eq!(counts(&status), [4, 3, 0, 1, 0, 1]);
    assert_eq!(status["unavailable"]["ineligible"], 0);

    // A second store on the same workspace and the first each see the other's
    // writes; neither reuses a class computed before them.
    let other = Store::open(s.config.clone()).unwrap();
    assert_eq!(other.processing_status(false).unwrap(), s.processing_status(false).unwrap());
    let d = other.reserve_processing(rowid("fiction:poll-d")).unwrap();
    assert_eq!(counts(&poll(&s, &wider)), [4, 2, 1, 1, 0, 1]);
    other.finish_processing_attempt(&d, "failed", Some(0.001), &Value::Null, Some(rejection)).unwrap();
    assert_eq!(counts(&poll(&s, &wider)), [4, 2, 0, 2, 0, 1]);
    assert_eq!(counts(&poll(&other, &wider)), [4, 2, 0, 2, 0, 1]);
}
