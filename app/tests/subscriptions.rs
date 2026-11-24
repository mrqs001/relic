use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use relic_app::{
    api,
    config::Config,
    store::Store,
    subscriptions::{MAX_SUBSCRIPTIONS, SubscriptionQuery},
};
use serde_json::{Value, json};
use std::sync::{Arc, Barrier};
use tower::ServiceExt;

fn store() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let config = Config::local(
        std::env::current_dir().unwrap(),
        tmp.path().join("data"),
        "127.0.0.1:8792".parse().unwrap(),
    );
    let s = Store::open(config).unwrap();
    (tmp, s)
}
fn setup(provider: &str, target: &str) -> Value {
    json!({"provider":provider,"label":"Fictional source","target":target,"enabled":true})
}
fn list(s: &Store) -> Value {
    s.subscriptions(SubscriptionQuery::default()).unwrap()
}
fn edit(s: &Value, label: &str, expected: i64) -> Value {
    json!({"expected_version":expected,"provider":s["provider"],"label":label,"target":s["target"],"mode":s["mode"],"poll_interval_seconds":s["poll_interval_seconds"],"enabled":s["enabled"]})
}

#[test]
fn fictional_contract_has_truthful_state_and_no_acquisition_side_effects() {
    let (_tmp, s) = store();
    let fixture: Value = serde_json::from_str(include_str!("fixtures/subscriptions.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let result = s.create_subscription(case["input"].clone()).unwrap();
        for (key, expected) in case["expected"].as_object().unwrap() {
            assert_eq!(&result[key], expected, "{key}");
        }
        assert!(result["id"].as_str().unwrap().starts_with("subscription:"));
        assert_eq!(result.as_object().unwrap().len(), 12);
        assert!(
            result["status_detail"]
                .as_str()
                .unwrap()
                .contains("not connected")
        );
    }
    let workspace = s.workspace().unwrap();
    assert_eq!(workspace["empty"], true);
    for key in ["sources", "publications", "jobs"] {
        assert!(workspace[key].as_array().unwrap().is_empty());
    }
    let events = s.replay_events(0).unwrap().1;
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|e| e["kind"] == "collection_settings" && e["publication_id"].is_null())
    );
    assert_eq!(
        s.health()["limits"]["live_sources"],
        "explicit_account_activation"
    );
}

#[test]
fn unsafe_targets_invalid_cadence_and_unknown_fields_never_persist() {
    let (_tmp, s) = store();
    for (provider, target) in [
        ("telegram", "https://t.me/+privateInvite"),
        ("telegram", "https://t.me/joinchat"),
        ("telegram", "https://t.me/example/123"),
        ("telegram", "https://t.me/s/example"),
        ("telegram", "https://t.me:443/example"),
        ("telegram", "https://user:password@t.me/example"),
        ("telegram", "https://t.me/example?token=do-not-retain"),
        ("telegram", "https://t.me/example#secret"),
        ("telegram", "https://evil.example/example"),
        ("telegram", "https://t.me/%65xample"),
        ("telegram", "https://t.me/example//"),
        ("telegram", "@ex ample"),
        ("telegram", "12345"),
        ("telegram", "short".get(..4).unwrap()),
        ("x", "https://x.com/example/status/123"),
        ("x", "https://x.com/home"),
        ("x", "http://x.com/example"),
        ("x", "https://t.me/example"),
        ("x", "abcdefghijklmnop"),
        ("x", "@bad-name"),
        ("x", "@"),
    ] {
        let error = s.create_subscription(setup(provider, target)).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST, "{provider} {target}");
        assert!(!error.message.contains("do-not-retain"));
    }
    let mut cases = vec![];
    for interval in [
        json!(29),
        json!(86401),
        json!(-1),
        json!(120.5),
        json!("120"),
    ] {
        let mut v = setup("x", "relic_test");
        v["poll_interval_seconds"] = interval;
        cases.push(v);
    }
    for (key, value) in [
        ("mode", json!("poll")),
        ("poll_interval_seconds", json!(120)),
    ] {
        let mut v = setup("telegram", "relic_test");
        v[key] = value;
        cases.push(v);
    }
    for (key, value) in [
        ("provider", json!("rss")),
        ("mode", json!("stream")),
        ("label", json!(" ")),
        ("label", json!("a".repeat(241))),
        ("label", json!("line\nbreak")),
        ("target", json!("a".repeat(257))),
        ("enabled", json!("yes")),
        ("token", json!("do-not-retain")),
    ] {
        let mut v = setup("x", "relic_test");
        v[key] = value;
        cases.push(v);
    }
    for case in cases {
        assert_eq!(
            s.create_subscription(case).unwrap_err().status,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(list(&s)["total"], 0);
    assert!(s.replay_events(0).unwrap().1.is_empty());
    for interval in [30, 86400] {
        let mut v = setup("x", &format!("boundary{interval}"));
        v["poll_interval_seconds"] = json!(interval);
        assert_eq!(
            s.create_subscription(v).unwrap()["poll_interval_seconds"],
            interval
        );
    }
}

#[test]
fn duplicates_and_stale_edits_leave_existing_settings_intact() {
    let (_tmp, s) = store();
    let one = s.create_subscription(setup("x", "@RELIC_TEST")).unwrap();
    assert_eq!(
        s.create_subscription(setup("x", "https://x.com/relic_test"))
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
    let two = s.create_subscription(setup("x", "other_test")).unwrap();
    let id = one["id"].as_str().unwrap();
    let mut duplicate = edit(&one, "Duplicate", 1);
    duplicate["target"] = two["target"].clone();
    assert_eq!(
        s.update_subscription(id, duplicate).unwrap_err().status,
        StatusCode::CONFLICT
    );
    let mut changed = edit(&one, "Renamed", 1);
    changed["poll_interval_seconds"] = json!(300);
    changed["enabled"] = json!(false);
    let renamed = s.update_subscription(id, changed).unwrap();
    assert_eq!(renamed["version"], 2);
    assert_eq!(renamed["created_at"], one["created_at"]);
    assert_eq!(renamed["status"], "paused");
    assert_eq!(renamed["poll_interval_seconds"], 300);
    assert_eq!(
        s.update_subscription(id, edit(&one, "Stale writer", 1))
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
    let mut missing = edit(&one, "Missing version", 1);
    missing.as_object_mut().unwrap().remove("expected_version");
    assert_eq!(
        s.update_subscription(id, missing).unwrap_err().status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(list(&s)["items"][0], renamed);
    assert_eq!(s.replay_events(0).unwrap().1.len(), 3);
}

#[test]
fn competing_writers_acknowledge_exactly_one_version() {
    let (_tmp, s) = store();
    let entry = s.create_subscription(setup("x", "relic_test")).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|index| {
            let store = s.clone();
            let entry = entry.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.update_subscription(
                    entry["id"].as_str().unwrap(),
                    edit(&entry, &format!("Writer {index}"), 1),
                )
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results.into_iter().find_map(|r| r.err()).unwrap().status,
        StatusCode::CONFLICT
    );
    assert_eq!(list(&s)["items"][0]["version"], 2);
}

#[test]
fn keyset_pagination_is_bounded_and_search_cursor_cannot_cross_queries() {
    let (_tmp, s) = store();
    for index in 0..135 {
        let mut v = setup("x", &format!("relic_{index}"));
        v["label"] = json!(if index % 2 == 0 {
            "Événement port"
        } else {
            "Other source"
        });
        s.create_subscription(v).unwrap();
    }
    let first = list(&s);
    assert_eq!(first["total"], 135);
    assert_eq!(first["items"].as_array().unwrap().len(), 50);
    let mut seen = std::collections::HashSet::new();
    let mut cursor = None;
    loop {
        let page = s
            .subscriptions(SubscriptionQuery {
                limit: Some(17),
                cursor,
                query: Some("ÉVÉNEMENT".into()),
            })
            .unwrap();
        assert_eq!(page["total"], 68);
        assert!(page["items"].as_array().unwrap().len() <= 17);
        for item in page["items"].as_array().unwrap() {
            assert!(seen.insert(item["id"].as_str().unwrap().to_string()));
        }
        cursor = page["next_cursor"].as_str().map(String::from);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 68);
    assert!(
        s.subscriptions(SubscriptionQuery {
            cursor: first["next_cursor"].as_str().map(String::from),
            query: Some("different".into()),
            limit: None
        })
        .is_err()
    );
    for limit in [0, 101] {
        assert!(
            s.subscriptions(SubscriptionQuery {
                limit: Some(limit),
                ..Default::default()
            })
            .is_err()
        );
    }
    assert!(
        s.subscriptions(SubscriptionQuery {
            cursor: Some("bad".into()),
            ..Default::default()
        })
        .is_err()
    );
    assert_eq!(
        s.subscriptions(SubscriptionQuery {
            query: Some("%".into()),
            ..Default::default()
        })
        .unwrap()["total"],
        0
    );
}

#[test]
fn saved_intent_survives_restart_and_snapshot_restore_without_activation() {
    let (_tmp, s) = store();
    let one = s
        .create_subscription(setup("telegram", "relic_test"))
        .unwrap();
    let two = s.create_subscription(setup("x", "relic_test")).unwrap();
    let expected = list(&s);
    let backup = s.backup().unwrap();
    s.update_subscription(
        one["id"].as_str().unwrap(),
        edit(&one, "Changed after snapshot", 1),
    )
    .unwrap();
    let config = s.config.clone();
    drop(s);
    let reopened = Store::open(config).unwrap();
    assert_eq!(
        list(&reopened)["items"][0]["label"],
        "Changed after snapshot"
    );
    assert_eq!(list(&reopened)["items"][1], two);
    let verified = reopened
        .verify_backup(backup["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(verified["verified"], true);
    assert_eq!(verified["subscriptions"], 2);
    let mut restored_config = reopened.config.clone();
    restored_config.data = verified["restored_directory"].as_str().unwrap().into();
    let restored = Store::open(restored_config).unwrap();
    assert_eq!(list(&restored), expected);
    assert!(
        restored.workspace().unwrap()["jobs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        restored.workspace().unwrap()["publications"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn additive_setup_preserves_existing_workspaces_and_older_backups() {
    let (_tmp, s) = store();
    s.import_job(
        serde_json::from_str(include_str!("../../examples/port-investigation.json")).unwrap(),
        relic_app::model::ident("job"),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .unwrap();
    let before = s.workspace().unwrap();
    // Reproduce the previous release's schema without changing any evidence or
    // analyst tables, and retain a backup from that release boundary.
    s.connect()
        .unwrap()
        .execute("DROP TABLE subscriptions", [])
        .unwrap();
    let old_backup = s.backup().unwrap();
    let config = s.config.clone();
    drop(s);
    let migrated = Store::open(config).unwrap();
    assert_eq!(migrated.workspace().unwrap(), before);
    assert_eq!(list(&migrated)["total"], 0);
    let saved = migrated
        .create_subscription(setup("x", "relic_test"))
        .unwrap();
    let verified = migrated
        .verify_backup(old_backup["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(verified["verified"], true);
    assert_eq!(verified["subscriptions"], 0);
    assert_eq!(verified["briefs"], 1);
    assert_eq!(list(&migrated)["items"][0], saved);
}

#[test]
fn configuration_cap_still_allows_existing_settings_to_be_paused() {
    let (_tmp, s) = store();
    let first = s.create_subscription(setup("x", "relic_test")).unwrap();
    // Populate bounded authoritative rows in one transaction; exercise the real
    // API at the boundary rather than performing 1000 independent disk syncs.
    let mut c = s.connect().unwrap();
    let tx = c.transaction().unwrap();
    for i in 1..MAX_SUBSCRIPTIONS {
        tx.execute("INSERT INTO subscriptions(id,version,provider,label,target,mode,poll_interval_seconds,enabled,created_at,updated_at,search_text) SELECT ?1,version,provider,label,?2,mode,poll_interval_seconds,enabled,created_at,updated_at,?2 FROM subscriptions WHERE id=?3", rusqlite::params![format!("subscription:{i}"),format!("@relic_{i}"),first["id"].as_str().unwrap()]).unwrap();
    }
    tx.commit().unwrap();
    let error = s.create_subscription(setup("x", "extra_test")).unwrap_err();
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error.code, "subscription_limit");
    let mut pause = edit(&first, "Paused at capacity", 1);
    pause["enabled"] = json!(false);
    assert_eq!(
        s.update_subscription(first["id"].as_str().unwrap(), pause)
            .unwrap()["status"],
        "paused"
    );
    assert_eq!(list(&s)["total"], MAX_SUBSCRIPTIONS);
}

async fn response(
    app: axum::Router,
    method: &str,
    url: &str,
    payload: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(url)
        .header("host", "127.0.0.1:8792")
        .header("content-type", "application/json")
        .body(payload.map_or_else(Body::empty, |v| Body::from(v.to_string())))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    (
        status,
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap(),
    )
}

#[tokio::test]
async fn detail_returns_latest_settings_outside_filters_and_structured_not_found() {
    let (_tmp, s) = store();
    let created = s.create_subscription(setup("x", "relic_test")).unwrap();
    let id = created["id"].as_str().unwrap();
    let mut changed = edit(&created, "Renamed elsewhere", 1);
    changed["target"] = json!("@other_test");
    changed["poll_interval_seconds"] = json!(45);
    let current = s.update_subscription(id, changed).unwrap();
    assert_eq!(
        s.subscriptions(SubscriptionQuery {
            query: Some("relic_test".into()),
            ..Default::default()
        })
        .unwrap()["total"],
        0
    );
    let app = api::router(s.clone());
    let (status, result) = response(
        app.clone(),
        "GET",
        &format!("/api/v1/subscriptions/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for key in [
        "id",
        "version",
        "provider",
        "label",
        "target",
        "mode",
        "poll_interval_seconds",
        "enabled",
        "created_at",
        "updated_at",
        "status",
    ] {
        assert_eq!(result[key], current[key], "{key}");
    }
    assert_eq!(result["collection"]["messages_acquired"], 0);
    assert_eq!(result["collection"]["can_continue"], false);
    assert!(result.get("items").is_none());
    let (status, error) = response(
        app.clone(),
        "GET",
        "/api/v1/subscriptions/subscription:missing",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "not_found");
    let (status, error) = response(app, "GET", "/api/v1/subscriptions/invalid", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["code"], "invalid_input");
    assert_eq!(s.replay_events(0).unwrap().1.len(), 2);
    let mut config = s.config.clone();
    config.api_token = Some("synthetic-test-only-token".into());
    let protected = api::router(Store::open(config).unwrap());
    assert_eq!(
        response(
            protected,
            "GET",
            &format!("/api/v1/subscriptions/{id}"),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn routes_validate_payloads_and_keep_workspace_access_boundary() {
    let (_tmp, s) = store();
    let app = api::router(s.clone());
    let (status, created) = response(
        app.clone(),
        "POST",
        "/api/v1/subscriptions",
        Some(setup("x", "relic_test")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["status"], "not_connected");
    let url = format!("/api/v1/subscriptions/{}", created["id"].as_str().unwrap());
    assert_eq!(
        response(
            app.clone(),
            "PUT",
            &url,
            Some(edit(&created, "Edited over HTTP", 1))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        response(
            app.clone(),
            "PUT",
            &url,
            Some(edit(&created, "Stale HTTP", 1))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        response(
            app.clone(),
            "GET",
            "/api/v1/subscriptions?limit=1&query=x",
            None
        )
        .await
        .1["total"],
        1
    );
    for url in [
        "/api/v1/subscriptions?limit=101",
        "/api/v1/subscriptions?limit=bad",
        "/api/v1/subscriptions?token=private",
    ] {
        let (status, body) = response(app.clone(), "GET", url, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].is_object());
    }
    let mut oversized = setup("x", "another");
    oversized["label"] = json!("a".repeat(5000));
    let (status, body) = response(app, "POST", "/api/v1/subscriptions", Some(oversized)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"]["message"],
        "Expected valid bounded JSON request"
    );
    let mut config = s.config.clone();
    config.api_token = Some("synthetic-test-only-token".into());
    let protected = api::router(Store::open(config).unwrap());
    assert_eq!(
        response(protected.clone(), "GET", "/api/v1/subscriptions", None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        response(
            protected,
            "POST",
            "/api/v1/subscriptions",
            Some(setup("x", "another"))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}
