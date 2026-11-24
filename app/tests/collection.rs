//! Synthetic decoded adapter output only. No account/session/provider calls.
use relic_app::{
    config::Config,
    model::{Citation, Scope, now},
    store::Store,
};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};

fn workspace() -> (tempfile::TempDir, Store, String) {
    let tmp = tempfile::tempdir().unwrap();
    let config = Config::local(
        std::env::current_dir().unwrap(),
        tmp.path().join("data"),
        "127.0.0.1:8792".parse().unwrap(),
    );
    let store = Store::open(config).unwrap();
    let subscription = store.create_subscription(json!({"provider":"x","label":"Synthetic profile","target":"@synthetic_qa","enabled":true})).unwrap();
    (tmp, store, subscription["id"].as_str().unwrap().into())
}
fn source() -> Value {
    json!({"id":"x:user:100","label":"Synthetic profile","coverage":"Synthetic bounded page; no live upstream coverage claim"})
}
fn message(text: &str) -> Value {
    json!({"id":"x:post:1000","text":text,"published_at":"2026-09-10T10:00:00Z","media":[],"relationships":{}})
}
fn next(store: &Store, id: &str, watermark: &str) -> Value {
    let mut state = store.collection_state(id).unwrap();
    state["watermark"] = json!(watermark);
    state["last_success_at"] = json!(now());
    state
}
fn publish(store: &Store, id: &str, messages: Vec<Value>, state: Value) -> Value {
    store
        .publish_collection_page(
            id,
            1,
            source(),
            messages.clone(),
            json!({"messages":messages,"synthetic":true}),
            state,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap()
}
fn count(store: &Store, table: &str) -> i64 {
    store
        .connect()
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
fn latest_evidence(store: &Store) -> Value {
    let c = store.connect().unwrap();
    let raw:String = c.query_row("SELECT e.json FROM membership m JOIN evidence e ON m.evidence_rowid=e.rowid WHERE m.publication_id=(SELECT value FROM metadata WHERE key='current_publication') ORDER BY e.rowid DESC LIMIT 1",[],|r|r.get(0)).unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn checkpoint_and_evidence_commit_together_after_failure_and_replay() {
    let (_tmp, store, id) = workspace();
    let state = next(&store, &id, "1000");
    store.connect().unwrap().execute_batch("CREATE TRIGGER fail_checkpoint BEFORE INSERT ON collection_states BEGIN SELECT RAISE(ABORT,'synthetic crash at checkpoint'); END;").unwrap();
    let failed = store.publish_collection_page(
        &id,
        1,
        source(),
        vec![message("Exact retained text")],
        json!({"synthetic":"page"}),
        state.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    assert!(failed.is_err());
    for table in [
        "sources",
        "evidence",
        "publications",
        "acquisitions",
        "collection_states",
        "jobs",
    ] {
        assert_eq!(count(&store, table), 0, "{table}");
    }
    assert_eq!(store.collection_state(&id).unwrap()["generation"], 0);
    store
        .connect()
        .unwrap()
        .execute("DROP TRIGGER fail_checkpoint", [])
        .unwrap();
    let result = publish(
        &store,
        &id,
        vec![message("Exact retained text")],
        state.clone(),
    );
    assert_eq!(result["accepted"], 1);
    assert_eq!(result["state"]["watermark"], "1000");
    assert_eq!(result["state"]["generation"], 1);
    assert!(
        store
            .original_object(result["original_input_hash"].as_str().unwrap())
            .is_ok()
    );
    // Simulate a lost success acknowledgement. A stale generation cannot write
    // the checkpoint backwards or republish after the actual atomic commit.
    assert_eq!(
        store
            .publish_collection_page(
                &id,
                1,
                source(),
                vec![message("Exact retained text")],
                json!({}),
                state,
                Arc::new(AtomicBool::new(false))
            )
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(count(&store, "evidence"), 1);
    assert_eq!(count(&store, "publications"), 1);
}

#[test]
fn counters_and_repeated_acquisition_do_not_duplicate_content_revisions() {
    let (_tmp, store, id) = workspace();
    let mut first = message("Événement 🛰 preserved");
    first["views"] = json!(12);
    first["observed_at"] = json!("2026-09-10T10:01:00Z");
    let one = publish(&store, &id, vec![first.clone()], next(&store, &id, "1000"));
    let retained = latest_evidence(&store);
    first["views"] = json!(900);
    first["observed_at"] = json!("2026-09-10T10:02:00Z");
    let two = publish(&store, &id, vec![first], next(&store, &id, "1000"));
    assert_eq!(two["accepted"], 0);
    assert_eq!(two["reused"], 1);
    assert_eq!(two["publication_id"], one["publication_id"]);
    assert_eq!(count(&store, "evidence"), 1);
    assert_eq!(latest_evidence(&store), retained);
    assert!(two["original_input_hash"].is_null());
    assert_eq!(two["unchanged"], true);
    assert_eq!(count(&store, "acquisitions"), 1);
    assert_eq!(count(&store, "jobs"), 1);
    let detail = store
        .detail(
            retained["id"].as_str().unwrap(),
            retained["revision"].as_str().unwrap(),
        )
        .unwrap();
    assert_eq!(detail["original_input_hashes"].as_array().unwrap().len(), 1);
    assert_eq!(count(&store, "objects"), 2); // normalized revision and first acquisition
}

#[test]
fn edits_keep_exact_citations_old_publication_and_stable_source_identity() {
    let (_tmp, store, id) = workspace();
    let one = publish(
        &store,
        &id,
        vec![message("Café 🚢 open")],
        next(&store, &id, "1000"),
    );
    let original = latest_evidence(&store);
    let citation: Citation = serde_json::from_value(original["citation"].clone()).unwrap();
    let mut edited = message("Café 🚢 closed");
    edited["source_edited_at"] = json!("2026-09-10T10:03:00Z");
    let two = publish(&store, &id, vec![edited], next(&store, &id, "1000"));
    assert_ne!(one["publication_id"], two["publication_id"]);
    assert_eq!(count(&store, "evidence"), 2);
    assert_eq!(latest_evidence(&store)["text"], "Café 🚢 closed");
    let scope = Scope {
        publication_id: Some(one["publication_id"].as_str().unwrap().into()),
        ..Default::default()
    };
    assert!(Store::check_citations(&store.connect().unwrap(), &[citation], Some(&scope)).is_ok());
    assert_eq!(
        store
            .detail(
                original["id"].as_str().unwrap(),
                original["revision"].as_str().unwrap()
            )
            .unwrap()["text"],
        "Café 🚢 open"
    );
    let mut renamed = source();
    renamed["label"] = json!("New display name");
    store
        .publish_collection_page(
            &id,
            1,
            renamed,
            vec![],
            json!({"synthetic":true}),
            next(&store, &id, "1000"),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    assert_eq!(count(&store, "sources"), 1);
    assert_eq!(
        store.workspace().unwrap()["sources"][0]["label"],
        "New display name"
    );
    assert_eq!(store.workspace().unwrap()["sources"][0]["provider"], "x");
    assert_eq!(original["source_label"], "Synthetic profile");
}

#[test]
fn media_only_empty_pages_and_explicit_removal_never_invent_text_or_geometry() {
    let (_tmp, store, id) = workspace();
    let empty = publish(&store, &id, vec![], next(&store, &id, "0"));
    assert_eq!(empty["accepted"], 0);
    assert_eq!(count(&store, "evidence"), 0);
    assert_eq!(count(&store, "publications"), 1);
    let mut media = message("");
    media["media"] =
        json!([{"id":"synthetic-media-1","type":"photo","archive_status":"not_downloaded"}]);
    publish(&store, &id, vec![media], next(&store, &id, "1000"));
    let entry = latest_evidence(&store);
    assert_eq!(entry["text"], "");
    assert!(entry["citation"].is_null());
    assert!(entry["geometry"].is_null());
    assert_eq!(entry["location_precision"], "unresolved");
    let mut written = message("Text before removal");
    written["source_edited_at"] = json!("2026-09-10T10:03:00Z");
    publish(&store, &id, vec![written], next(&store, &id, "1000"));
    let prior = latest_evidence(&store);
    let mut removed = message("");
    removed["upstream_state"] = json!("removed");
    publish(
        &store,
        &id,
        vec![removed.clone()],
        next(&store, &id, "1000"),
    );
    let current = latest_evidence(&store);
    assert_eq!(current["upstream_state"], "removed");
    assert_eq!(current["text"], "Text before removal");
    assert_eq!(current["retained_text_from"]["revision"], prior["revision"]);
    assert_eq!(
        publish(&store, &id, vec![removed], next(&store, &id, "1000"))["accepted"],
        0
    );
}

#[test]
fn stale_configuration_generation_cancel_and_status_only_checkpoint_changes_are_rejected() {
    let (_tmp, store, id) = workspace();
    let state = store.collection_state(&id).unwrap();
    let mut running = state.clone();
    running["status"] = json!("running");
    let saved = store.save_collection_state(&id, 1, running).unwrap();
    assert_eq!(saved["generation"], 1);
    assert!(store.save_collection_state(&id, 1, state).is_err());
    let mut illicit = saved.clone();
    illicit["watermark"] = json!("9999");
    assert!(store.save_collection_state(&id, 1, illicit).is_err());
    let paused = json!({"provider":"x","target":"@synthetic_qa","label":"Synthetic profile","enabled":false,"expected_version":1});
    store.update_subscription(&id, paused).unwrap();
    assert!(
        store
            .publish_collection_page(
                &id,
                1,
                source(),
                vec![message("late")],
                json!({}),
                saved.clone(),
                Arc::new(AtomicBool::new(false))
            )
            .is_err()
    );
    assert_eq!(
        store
            .publish_collection_page(
                &id,
                2,
                source(),
                vec![message("cancelled")],
                json!({}),
                saved,
                Arc::new(AtomicBool::new(true))
            )
            .unwrap_err()
            .code,
        "cancelled"
    );
    assert_eq!(count(&store, "evidence"), 0);
    assert_eq!(count(&store, "sources"), 0);
}

#[test]
fn fictional_workspace_rejection_precedes_all_collection_persistence() {
    let (_tmp, store, id) = workspace();
    store
        .import_job(
            serde_json::from_str(include_str!("../../examples/port-investigation.json")).unwrap(),
            "job:fixture".into(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    let counts: Vec<_> = [
        "sources",
        "evidence",
        "objects",
        "jobs",
        "acquisitions",
        "collection_states",
    ]
    .iter()
    .map(|table| (*table, count(&store, table)))
    .collect();
    assert!(store.assert_real_collection_allowed().is_err());
    assert!(
        store
            .publish_collection_page(
                &id,
                1,
                source(),
                vec![message("real")],
                json!({}),
                next(&store, &id, "1000"),
                Arc::new(AtomicBool::new(false))
            )
            .is_err()
    );
    for (table, prior) in counts {
        assert_eq!(count(&store, table), prior, "{table}");
    }
}

#[test]
fn cumulative_publications_preserve_other_sources_and_operation_lineage() {
    let (_tmp, store, id) = workspace();
    let mut state = next(&store, &id, "1000");
    state["run_id"] = json!("collection:synthetic-operation");
    let one = publish(&store, &id, vec![message("One source")], state);
    let second = store.create_subscription(json!({"provider":"telegram","label":"Synthetic channel","target":"@synthetic_channel","enabled":true})).unwrap();
    let second_id = second["id"].as_str().unwrap();
    let result = store.publish_collection_page(second_id,1,json!({"id":"telegram:channel:200","label":"Synthetic channel"}),vec![json!({"id":"telegram:200:1","text":"Other source","published_at":"2026-09-10T10:02:00Z"})],json!({"synthetic":true}),next(&store,second_id,"1"),Arc::new(AtomicBool::new(false))).unwrap();
    let c = store.connect().unwrap();
    let total: i64 = c
        .query_row(
            "SELECT count(*) FROM membership WHERE publication_id=?1",
            [result["publication_id"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(total, 2);
    assert_ne!(one["publication_id"], result["publication_id"]);
    assert!(c.query_row("SELECT EXISTS(SELECT 1 FROM lineage_edges WHERE source='collection:synthetic-operation' AND target=?1 AND relationship='executes_page')",[one["job_id"].as_str().unwrap()],|r|r.get::<_,bool>(0)).unwrap());
    assert_eq!(store.collection_states().unwrap().len(), 2);
}

#[test]
fn state_raw_revisions_and_citations_survive_restart_and_verified_backup() {
    let (_tmp, store, id) = workspace();
    publish(
        &store,
        &id,
        vec![message("Durable across restore")],
        next(&store, &id, "1000"),
    );
    let expected = store.collection_state(&id).unwrap();
    let expected_evidence = latest_evidence(&store);
    let backup = store.backup().unwrap();
    let config = store.config.clone();
    drop(store);
    let store = Store::open(config).unwrap();
    assert_eq!(store.collection_state(&id).unwrap(), expected);
    let verified = store.verify_backup(backup["id"].as_str().unwrap()).unwrap();
    assert_eq!(verified["verified"], true);
    assert_eq!(verified["collection_states"], 1);
    let mut restored_config = store.config.clone();
    restored_config.data = verified["restored_directory"].as_str().unwrap().into();
    let restored = Store::open(restored_config).unwrap();
    assert_eq!(restored.collection_state(&id).unwrap(), expected);
    assert_eq!(latest_evidence(&restored), expected_evidence);
    let citation: Citation = serde_json::from_value(expected_evidence["citation"].clone()).unwrap();
    assert!(Store::check_citations(&restored.connect().unwrap(), &[citation], None).is_ok());
}

#[test]
fn watch_corrections_use_the_same_atomic_publication_without_repeat_alerts() {
    let (_tmp, store, id) = workspace();
    publish(
        &store,
        &id,
        vec![message("Initial text")],
        next(&store, &id, "1000"),
    );
    store.create_watch(json!({"title":"Synthetic review","question":"Did the source correct its text?","scope":{},"conditions":["source correction"],"baseline_policy":"fresh"})).unwrap();
    assert_eq!(count(&store, "alerts"), 0);
    let mut edit = message("Corrected text");
    edit["source_edited_at"] = json!("2026-09-10T10:03:00Z");
    let result = publish(&store, &id, vec![edit.clone()], next(&store, &id, "1000"));
    assert_eq!(count(&store, "alerts"), 1);
    assert_eq!(result["state"]["messages_acquired"], 2);
    assert_eq!(result["state"]["attempt_accepted"], 2);
    publish(&store, &id, vec![edit], next(&store, &id, "1000"));
    assert_eq!(count(&store, "alerts"), 1);
    assert_eq!(store.collection_state(&id).unwrap()["messages_acquired"], 2);
}

#[test]
fn recent_publication_summary_is_bounded_but_old_exact_membership_remains_available() {
    let (_tmp, store, id) = workspace();
    let first = publish(
        &store,
        &id,
        vec![message("Retained source text")],
        next(&store, &id, "1000"),
    );
    let mut c = store.connect().unwrap();
    let tx = c.transaction().unwrap();
    for index in 0..135 {
        tx.execute("INSERT INTO publications VALUES(?1,'Synthetic historical publication','2026-09-11T00:00:00.000Z','complete',NULL)",[format!("publication:synthetic-{index}")]).unwrap();
    }
    tx.commit().unwrap();
    let workspace = store.workspace().unwrap();
    assert_eq!(workspace["publications"].as_array().unwrap().len(), 100);
    assert_eq!(workspace["publications_total"], 136);
    assert_eq!(workspace["publications_truncated"], true);
    // The actual current pointer remains visible even if artificial timestamp
    // order would otherwise push it outside the recent-summary window.
    assert_eq!(workspace["publications"][0]["id"], first["publication_id"]);
    let mut exact = Scope {
        publication_id: Some("publication:synthetic-0".into()),
        ..Default::default()
    };
    assert!(Store::resolve_scope(&store.connect().unwrap(), &mut exact).is_ok());
}

#[test]
fn repeated_absence_is_one_revision_and_late_history_does_not_resurrect_it() {
    let (_tmp, store, id) = workspace();
    let mut latest = message("Latest retained text");
    latest["source_edited_at"] = json!("2026-09-10T10:03:00Z");
    publish(&store, &id, vec![latest], next(&store, &id, "1000"));
    let mut removal = message("");
    removal["upstream_state"] = json!("removed");
    removal["upstream_observed_at"] = json!(now());
    publish(
        &store,
        &id,
        vec![removal.clone()],
        next(&store, &id, "1000"),
    );
    let removed = latest_evidence(&store);
    let mut older = message("Older retained text");
    older["published_at"] = json!("2026-09-10T09:59:00Z");
    publish(&store, &id, vec![older], next(&store, &id, "1000"));
    assert_eq!(latest_evidence(&store), removed);
    removal["upstream_observed_at"] = json!("2027-01-01T00:00:00Z");
    // Be robust to older adapters that used a reconciliation timestamp in the
    // edit field; new adapters retain this as upstream_observed_at instead.
    removal["source_edited_at"] = json!("2027-01-01T00:00:00Z");
    let repeated = publish(&store, &id, vec![removal], next(&store, &id, "1000"));
    assert_eq!(repeated["accepted"], 0);
    assert_eq!(latest_evidence(&store), removed);
    assert_eq!(count(&store, "evidence"), 3);
    // The strict import path must use the same current-revision rule when it
    // rebuilds cumulative membership after an unrelated manually imported item.
    let imported = json!({"schema_version":1,"fictional":false,"sources":[{"id":"local:other","label":"Synthetic imported source","family":"reporting"}],"evidence_revisions":[{"id":"local:report:1","revision":"v1","source":"local:other","text":"Independent imported text","fictional":false,"acquired_at":now(),"published_at":"2026-09-10T10:00:00Z","availability":"available","language":"en","geometry":null,"location_precision":"unresolved","citation":{"evidence_id":"local:report:1","revision":"v1","start_character":0,"end_character":25}}]});
    store
        .import_job(
            imported,
            "job:synthetic-local-import".into(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    let c = store.connect().unwrap();
    let current:String = c.query_row("SELECT e.json FROM membership m JOIN evidence e ON e.rowid=m.evidence_rowid WHERE e.id='x:post:1000' AND m.publication_id=(SELECT value FROM metadata WHERE key='current_publication')",[],|row|row.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&current).unwrap(), removed);
}

#[test]
fn thousand_messages_keep_prior_page_visible_through_failed_commit_and_exact_replay() {
    let (_tmp, store, id) = workspace();
    let batch = |start: usize| {
        (start..start + 500)
            .map(|index| {
                let mut record = message(&format!("Synthetic batch message {index}"));
                record["id"] = json!(format!("x:post:{index}"));
                record
            })
            .collect::<Vec<_>>()
    };
    let one = publish(&store, &id, batch(1000), next(&store, &id, "1499"));
    let before = store.collection_state(&id).unwrap();
    let second = batch(1500);
    let raw = json!({"messages":second,"synthetic":true});
    store.connect().unwrap().execute_batch("CREATE TRIGGER fail_page_checkpoint BEFORE UPDATE ON collection_states BEGIN SELECT RAISE(ABORT,'synthetic page commit failure'); END;").unwrap();
    assert!(
        store
            .publish_collection_page(
                &id,
                1,
                source(),
                second.clone(),
                raw.clone(),
                next(&store, &id, "1999"),
                Arc::new(AtomicBool::new(false))
            )
            .is_err()
    );
    assert_eq!(store.collection_state(&id).unwrap(), before);
    assert_eq!(count(&store, "evidence"), 500);
    assert_eq!(
        Store::latest(&store.connect().unwrap()).unwrap().as_deref(),
        one["publication_id"].as_str()
    );
    store
        .connect()
        .unwrap()
        .execute("DROP TRIGGER fail_page_checkpoint", [])
        .unwrap();
    let two = publish(&store, &id, second, next(&store, &id, "1999"));
    assert_eq!(count(&store, "evidence"), 1000);
    assert_eq!(two["accepted"], 500);
    assert_eq!(two["state"]["messages_acquired"], 1000);
    assert_eq!(
        store
            .original_object(two["original_input_hash"].as_str().unwrap())
            .unwrap(),
        serde_json::to_vec(&raw).unwrap()
    );
    let c = store.connect().unwrap();
    for (publication, expected) in [
        (&one["publication_id"], 500),
        (&two["publication_id"], 1000),
    ] {
        let total: i64 = c
            .query_row(
                "SELECT count(*) FROM membership WHERE publication_id=?1",
                [publication.as_str().unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total, expected);
    }
    let state = store.collection_state(&id).unwrap();
    assert!(
        store
            .publish_collection_page(
                &id,
                1,
                source(),
                vec![message("Oversized count"); 2001],
                json!({}),
                state.clone(),
                Arc::new(AtomicBool::new(false))
            )
            .is_err()
    );
    assert!(
        store
            .publish_collection_page(
                &id,
                1,
                source(),
                vec![],
                json!({"oversized":"x".repeat(2*1024*1024)}),
                state.clone(),
                Arc::new(AtomicBool::new(false))
            )
            .is_err()
    );
    assert_eq!(store.collection_state(&id).unwrap(), state);
}

#[test]
fn explicit_rebaseline_preserves_gap_evidence_and_rejects_stale_actions() {
    let (_tmp, store, id) = workspace();
    let mut state = next(&store, &id, "1000");
    state["provider"] = json!("x");
    state["continuation"] = json!({"cursor":"fictional-next","original_watermark":"1000"});
    state["status"] = json!("needs_attention");
    publish(
        &store,
        &id,
        vec![message("Retained before deliberate restart")],
        state,
    );
    let prior = store.collection_state(&id).unwrap();
    assert!(store.rebaseline_collection(&id, 2, prior.clone()).is_err());
    let result = store.rebaseline_collection(&id, 1, prior.clone()).unwrap();
    assert!(result["watermark"].is_null());
    assert!(result["continuation"].is_null());
    assert_eq!(result["requested_action"], "baseline");
    assert_eq!(count(&store, "evidence"), 1);
    let gap: String = store
        .connect()
        .unwrap()
        .query_row("SELECT json FROM collection_gaps", [], |r| r.get(0))
        .unwrap();
    let gap: Value = serde_json::from_str(&gap).unwrap();
    assert_eq!(gap["prior_state"], prior);
    assert!(store.rebaseline_collection(&id, 1, prior).is_err());
    let backup = store.backup().unwrap();
    assert_eq!(
        store.verify_backup(backup["id"].as_str().unwrap()).unwrap()["verified"],
        true
    );
}

/// An unchanged poll moves the checkpoint to the latest publication, which
/// another source may have created; its acquisition stays the earlier one.
/// Restore verification must accept that, and still reject a checkpoint whose
/// acquisition is missing or has a different input.
#[test]
fn unchanged_checks_after_another_sources_publication_survive_backup_verification() {
    let (_tmp, store, id) = workspace();
    publish(&store, &id, vec![message("First retained text")], next(&store, &id, "1000"));
    let other = store.create_subscription(json!({"provider":"x","label":"Other synthetic profile","target":"@other_qa","enabled":true})).unwrap();
    let other_id = other["id"].as_str().unwrap();
    store
        .publish_collection_page(
            other_id,
            1,
            json!({"id":"x:user:200","label":"Other synthetic profile","coverage":"Synthetic bounded page; no live upstream coverage claim"}),
            vec![json!({"id":"x:post:2000","text":"Other source text","published_at":"2026-09-10T11:00:00Z","media":[],"relationships":{}})],
            json!({"synthetic":true}),
            next(&store, other_id, "2000"),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    let unchanged = publish(&store, &id, vec![message("First retained text")], next(&store, &id, "1000"));
    assert_eq!(unchanged["unchanged"], true);
    let backup = store.backup().unwrap();
    let verified = store.verify_backup(backup["id"].as_str().unwrap()).unwrap();
    assert_eq!(verified["verified"], true, "{verified}");
    // A checkpoint pointing at an acquisition with another input is still detached.
    let c = store.connect().unwrap();
    let mut state = store.collection_state(&id).unwrap();
    state["last_input_hash"] = json!("sha256:0000000000000000000000000000000000000000000000000000000000000000");
    c.execute("UPDATE collection_states SET json=?1 WHERE subscription_id=?2", rusqlite::params![state.to_string(), id]).unwrap();
    drop(c);
    let detached = store.backup().unwrap();
    let error = store.verify_backup(detached["id"].as_str().unwrap()).unwrap_err();
    assert!(error.message.contains("detached"), "{}", error.message);
}
