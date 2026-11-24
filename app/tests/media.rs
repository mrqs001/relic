use relic_app::{
    config::Config,
    media_store::MAX_MEDIA_BYTES,
    model::{hash, now},
    store::Store,
};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};

fn png() -> Vec<u8> {
    let hex = "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000b49444154789c636000020000050001a5f645400000000049454e44ae426082";
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn setup() -> (tempfile::TempDir, Store, Value) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::local(
        std::env::current_dir().unwrap(),
        temp.path().join("data"),
        "127.0.0.1:8792".parse().unwrap(),
    );
    let store = Store::open(config).unwrap();
    let sub = store.create_subscription(json!({"provider":"x","label":"Synthetic media profile","target":"@synthetic_qa","enabled":true})).unwrap();
    let id = sub["id"].as_str().unwrap();
    let mut state = store.collection_state(id).unwrap();
    state["last_success_at"] = json!(now());
    store.publish_collection_page(id,1,json!({"id":"x:user:100","label":"Synthetic media profile"}),vec![json!({"id":"x:post:1000","text":"Exact caption","published_at":"2026-09-10T10:00:00Z","media":[{"id":"synthetic-image-1","type":"photo"}]})],json!({"synthetic":true}),state,Arc::new(AtomicBool::new(false))).unwrap();
    let raw: String = store
        .connect()
        .unwrap()
        .query_row("SELECT json FROM evidence LIMIT 1", [], |r| r.get(0))
        .unwrap();
    (temp, store, serde_json::from_str(&raw).unwrap())
}

#[test]
fn exact_media_roundtrip_is_immutable_idempotent_and_executed_in_lineage() {
    let (_temp, store, e) = setup();
    let id = e["id"].as_str().unwrap();
    let revision = e["revision"].as_str().unwrap();
    assert!(store.retained_media(id, revision, 0).unwrap().is_none());
    let bytes = png();
    let retained = store
        .retain_media(id, revision, 0, bytes.clone(), "image/png; charset=binary")
        .unwrap();
    assert_eq!(
        retained,
        json!({"hash":hash(&bytes),"bytes":bytes.len(),"media_type":"image/png"})
    );
    assert_eq!(retained.as_object().unwrap().len(), 3);
    assert_eq!(
        store.retained_media(id, revision, 0).unwrap(),
        Some(retained.clone())
    );
    assert_eq!(
        store.media_bytes(id, revision, 0).unwrap(),
        (bytes.clone(), "image/png".into())
    );
    assert_eq!(
        store
            .retain_media(id, revision, 0, bytes.clone(), "image/png")
            .unwrap(),
        retained
    );
    let mut changed = bytes;
    changed[42] ^= 1;
    assert_eq!(
        store
            .retain_media(id, revision, 0, changed, "image/png")
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(store.detail(id, revision).unwrap()["text"], "Exact caption");
    let count: i64 = store
        .connect()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM lineage_edges WHERE target=?1",
            [format!("media:{}", retained["hash"].as_str().unwrap())],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn cancelled_media_validation_removes_staging_and_keeps_exact_evidence() {
    use std::io::Write;
    let (_temp, store, e) = setup();
    let id = e["id"].as_str().unwrap();
    let revision = e["revision"].as_str().unwrap();
    let mut file = store.media_staging().unwrap();
    file.write_all(&png()).unwrap();
    let path = file.path().to_owned();
    let error = store
        .retain_media_file_cancellable(id, revision, 0, file, "image/png", &AtomicBool::new(true))
        .unwrap_err();
    assert_eq!(error.code, "cancelled");
    assert!(!path.exists());
    assert!(store.retained_media(id, revision, 0).unwrap().is_none());
    assert_eq!(store.detail(id, revision).unwrap()["text"], "Exact caption");
}

#[tokio::test]
async fn progress_endpoint_reuses_cached_exact_media_without_source_connection() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (_temp, store, e) = setup();
    let id = e["id"].as_str().unwrap();
    let revision = e["revision"].as_str().unwrap();
    store
        .retain_media(id, revision, 0, png(), "image/png")
        .unwrap();
    let host = store.config.address.to_string();
    let router = relic_app::api::router(store.clone());
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/media/download")
                .header("host", host)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"id":id,"revision":revision,"index":0}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "application/x-ndjson");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let events: Vec<Value> = std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .map(|v| serde_json::from_str(v).unwrap())
        .collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["kind"], "ready");
    assert_eq!(events[1]["receipt"]["revision"], revision);
    assert_eq!(events[1]["receipt"]["bytes"], png().len());
}

#[test]
fn mismatched_unsafe_truncated_and_oversized_media_never_publish() {
    let (_temp, store, e) = setup();
    let id = e["id"].as_str().unwrap();
    let revision = e["revision"].as_str().unwrap();
    for (bytes, kind) in [
        (png(), "image/jpeg"),
        (b"<html>login required</html>".to_vec(), "image/png"),
        (
            b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec(),
            "image/svg+xml",
        ),
        (png(), "text/html"),
        (Vec::new(), "image/png"),
        (b"\x89PNG\r\n\x1a\n".to_vec(), "image/png"),
        (vec![0; MAX_MEDIA_BYTES + 1], "video/mp4"),
    ] {
        assert!(store.retain_media(id, revision, 0, bytes, kind).is_err());
    }
    assert!(store.retained_media(id, revision, 0).unwrap().is_none());
    assert_eq!(
        store.media_bytes(id, revision, 0).unwrap_err().code,
        "not_found"
    );
    let count: i64 = store
        .connect()
        .unwrap()
        .query_row("SELECT count(*) FROM media_assets", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn absent_evidence_revision_or_media_index_cannot_acquire_an_asset() {
    let (_temp, store, e) = setup();
    let id = e["id"].as_str().unwrap();
    let revision = e["revision"].as_str().unwrap();
    for (id, revision, index) in [
        ("x:post:missing", revision, 0),
        (id, "missing-revision", 0),
        (id, revision, 1),
        (id, revision, 100),
    ] {
        assert!(
            store
                .retain_media(id, revision, index, png(), "image/png")
                .is_err()
        );
        assert!(store.retained_media(id, revision, index).is_err());
    }
    assert!(store.retained_media(id, revision, 0).unwrap().is_none());
}

#[test]
fn retained_media_survives_verified_restore_and_detects_local_corruption() {
    let (_temp, store, e) = setup();
    let id = e["id"].as_str().unwrap();
    let revision = e["revision"].as_str().unwrap();
    let metadata = store
        .retain_media(id, revision, 0, png(), "image/png")
        .unwrap();
    let key = store.detail(id, revision).unwrap()["message_key"]
        .as_str()
        .unwrap()
        .to_owned();
    let backup = store.backup().unwrap();
    let verified = store.verify_backup(backup["id"].as_str().unwrap()).unwrap();
    assert_eq!(verified["verified"], true);
    assert_eq!(verified["media_assets"], 1);
    let mut restored_config = store.config.clone();
    restored_config.data = verified["restored_directory"].as_str().unwrap().into();
    let restored = Store::open(restored_config).unwrap();
    assert_eq!(
        restored.message_reference(&key).unwrap(),
        json!({"id":id,"revision":revision})
    );
    assert_eq!(
        restored.media_bytes(id, revision, 0).unwrap(),
        (png(), "image/png".into())
    );
    std::fs::write(
        store.config.object_path(metadata["hash"].as_str().unwrap()),
        b"not retained media",
    )
    .unwrap();
    assert!(store.media_bytes(id, revision, 0).is_err());
    assert_eq!(restored.media_bytes(id, revision, 0).unwrap().0, png());
}

#[test]
fn older_backup_without_media_table_remains_valid() {
    let (_temp, store, _) = setup();
    store
        .connect()
        .unwrap()
        .execute("DROP TABLE media_assets", [])
        .unwrap();
    let backup = store.backup().unwrap();
    let verified = store.verify_backup(backup["id"].as_str().unwrap()).unwrap();
    assert_eq!(verified["verified"], true);
    assert_eq!(verified["media_assets"], 0);
}

fn telegram_setup() -> (tempfile::TempDir, Store, Value, Value, String) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(Config::local(
        std::env::current_dir().unwrap(),
        temp.path().join("data"),
        "127.0.0.1:8792".parse().unwrap(),
    ))
    .unwrap();
    let fixture: Value =
        serde_json::from_str(include_str!("../../examples/telegram-media-recovery.json")).unwrap();
    let sub = store.create_subscription(json!({"provider":"telegram","label":"Synthetic media channel","target":"@synthetic_media","enabled":false})).unwrap();
    let sid = sub["id"].as_str().unwrap();
    let mut state = store.collection_state(sid).unwrap();
    state["watermark"] = json!("40");
    state["continuation"] = json!({"before":35,"highest":42});
    state["stopped"] = json!(true);
    state["last_success_at"] = json!("2026-09-12T10:02:00.000Z");
    let publication = store
        .publish_collection_page(
            sid,
            1,
            fixture["source"].clone(),
            vec![fixture["message"].clone()],
            json!({"synthetic":true}),
            state,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    let e: String = store
        .connect()
        .unwrap()
        .query_row("SELECT json FROM evidence LIMIT 1", [], |r| r.get(0))
        .unwrap();
    (
        temp,
        store,
        serde_json::from_str(&e).unwrap(),
        fixture,
        publication["publication_id"].as_str().unwrap().into(),
    )
}

#[test]
fn telegram_timestamp_format_and_transport_refresh_do_not_invent_edits() {
    let (_temp, store, e, fixture, _) = telegram_setup();
    assert_eq!(e["published_at"], "2026-09-12T10:00:00.000Z");
    let before = store.collection_states().unwrap();
    for instant in fixture["expected"]["same_instant_formats"]
        .as_array()
        .unwrap()
    {
        let mut current = fixture["message"].clone();
        current["published_at"] = instant.clone();
        // Acquisition/counters/file-reference rotation cannot revise content.
        current["acquired_at"] = json!(now());
        current["counters"] = json!({"views":999});
        current["media"][0]["file_reference"] = json!([8, 9]);
        let (_, revision) = store
            .reconcile_telegram_media(
                &e,
                0,
                current,
                json!({"synthetic":true}),
                png(),
                "image/png",
            )
            .unwrap();
        assert_eq!(revision, e["revision"]);
    }
    assert_eq!(store.collection_states().unwrap(), before);
    let count: i64 = store
        .connect()
        .unwrap()
        .query_row("SELECT count(*) FROM evidence", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn telegram_real_edit_recovers_once_without_rewriting_old_evidence_or_progress() {
    let (_temp, store, e, fixture, publication) = telegram_setup();
    let before = store.collection_states().unwrap()[0].clone();
    let mut current = fixture["message"].clone();
    current["text"] = json!("Fictional correction: power is still out.");
    current["source_edited_at"] = json!("2026-09-12T10:03:00Z");
    current["media"][0]["id"] = json!("synthetic-photo-2");
    let (_, revision) = store
        .reconcile_telegram_media(
            &e,
            0,
            current.clone(),
            json!({"synthetic":true,"message":current}),
            png(),
            "image/png",
        )
        .unwrap();
    assert_ne!(revision, e["revision"]);
    let id = e["id"].as_str().unwrap();
    let old_revision = e["revision"].as_str().unwrap();
    assert_eq!(store.detail(id, old_revision).unwrap()["text"], e["text"]);
    assert!(store.retained_media(id, old_revision, 0).unwrap().is_none());
    assert_eq!(store.media_bytes(id, &revision, 0).unwrap().0, png());
    assert_eq!(
        store.detail(id, &revision).unwrap()["text"],
        current["text"]
    );
    let after = store.collection_states().unwrap()[0].clone();
    for field in [
        "watermark",
        "continuation",
        "stopped",
        "last_success_at",
        "configuration_version",
    ] {
        assert_eq!(before[field], after[field], "{field}");
    }
    let old_member: String = store.connect().unwrap().query_row("SELECT e.revision FROM membership m JOIN evidence e ON e.rowid=m.evidence_rowid WHERE m.publication_id=?1", [&publication], |r| r.get(0)).unwrap();
    assert_eq!(old_member, old_revision);
    let (_, again) = store
        .reconcile_telegram_media(
            &e,
            0,
            current,
            json!({"synthetic":true}),
            png(),
            "image/png",
        )
        .unwrap();
    assert_eq!(again, revision);
    let count: i64 = store
        .connect()
        .unwrap()
        .query_row("SELECT count(*) FROM evidence", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn telegram_wrong_origin_invalid_media_and_cancel_do_not_publish_recovery() {
    let (_temp, store, e, fixture, _) = telegram_setup();
    for field in ["account_id", "peer_id", "message_id"] {
        let mut wrong = fixture["message"].clone();
        wrong["relationships"][field] = json!("999");
        assert!(
            store
                .reconcile_telegram_media(&e, 0, wrong, json!({}), png(), "image/png")
                .is_err()
        );
    }
    let mut current = fixture["message"].clone();
    current["text"] = json!("A real fictional edit");
    assert!(
        store
            .reconcile_telegram_media(
                &e,
                0,
                current.clone(),
                json!({}),
                b"<html>".to_vec(),
                "image/png"
            )
            .is_err()
    );
    store
        .shutdown
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        store
            .reconcile_telegram_media(&e, 0, current, json!({}), png(), "image/png")
            .unwrap_err()
            .code,
        "cancelled"
    );
    let count: i64 = store
        .connect()
        .unwrap()
        .query_row("SELECT count(*) FROM evidence", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn compact_message_links_resolve_exact_revisions_and_reject_invalid_keys() {
    let (_temp, store, e) = setup();
    let id = e["id"].as_str().unwrap();
    let revision = e["revision"].as_str().unwrap();
    let detail = store.detail(id, revision).unwrap();
    let key = detail["message_key"].as_str().unwrap();
    assert_eq!(
        store.message_reference(key).unwrap(),
        json!({"id":id,"revision":revision})
    );
    for invalid in ["0", "-1", "01", "1 OR 1=1", "999999999999999999999", ""] {
        assert_eq!(
            store.message_reference(invalid).unwrap_err().code,
            "invalid_input"
        );
    }
    assert_eq!(
        store.message_reference("999999").unwrap_err().code,
        "not_found"
    );
}

#[tokio::test]
async fn large_video_streams_ranges_and_restores_without_a_large_byte_buffer() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use std::io::Write;
    use tower::ServiceExt;
    let (_temp, store, e) = setup();
    let id = e["id"].as_str().unwrap();
    let revision = e["revision"].as_str().unwrap();
    let mut stage = store.media_staging().unwrap();
    let stage_path = stage.path().to_owned();
    stage
        .write_all(b"\0\0\0\x18ftypisom\0\0\0\0isommp42")
        .unwrap();
    let chunk = [0u8; 64 * 1024];
    for _ in 0..320 {
        stage.write_all(&chunk).unwrap();
    }
    let size = 24 + 320 * chunk.len();
    let retained = store
        .retain_media_file(id, revision, 0, stage, "video/mp4")
        .unwrap();
    assert!(!stage_path.exists());
    assert_eq!(retained["bytes"], size);
    assert!(store.media_bytes(id, revision, 0).is_err());
    let router = relic_app::api::router(store.clone());
    let path = format!("/api/v1/media?id={id}&revision={revision}&index=0");
    for (method, range, status, bytes, content_range) in [
        (
            "GET",
            Some("bytes=4-11"),
            206,
            8,
            Some(format!("bytes 4-11/{size}")),
        ),
        (
            "GET",
            Some("bytes=-16"),
            206,
            16,
            Some(format!("bytes {}-{}/{size}", size - 16, size - 1)),
        ),
        ("HEAD", None, 200, 0, None),
        (
            "GET",
            Some("bytes=99999999-"),
            416,
            0,
            Some(format!("bytes */{size}")),
        ),
    ] {
        let mut req = Request::builder()
            .method(method)
            .uri(&path)
            .header("host", "127.0.0.1:8792");
        if let Some(range) = range {
            req = req.header("range", range);
        }
        let response = router
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status);
        if let Some(range) = content_range {
            assert_eq!(response.headers()["content-range"], range);
        }
        let body = response.into_body().collect().await.unwrap().to_bytes();
        if status != 416 {
            assert_eq!(body.len(), bytes);
        }
    }
    let backup = store.backup().unwrap();
    assert_eq!(
        store.verify_backup(backup["id"].as_str().unwrap()).unwrap()["media_assets"],
        1
    );
    let mut oversized = store.media_staging().unwrap();
    oversized
        .write_all(b"\0\0\0\x18ftypisom\0\0\0\0isommp42")
        .unwrap();
    oversized
        .as_file()
        .set_len(relic_app::media_store::MAX_MEDIA_FILE_BYTES as u64 + 1)
        .unwrap();
    let abandoned = oversized.path().to_owned();
    assert!(
        store
            .retain_media_file(id, revision, 0, oversized, "video/mp4")
            .is_err()
    );
    assert!(!abandoned.exists());
}

#[test]
fn legacy_media_limit_migrates_without_changing_retained_associations() {
    let (_temp, store, e) = setup();
    let id = e["id"].as_str().unwrap();
    let revision = e["revision"].as_str().unwrap();
    let retained = store
        .retain_media(id, revision, 0, png(), "image/png")
        .unwrap();
    let c = store.connect().unwrap();
    let sql: String = c
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='media_assets'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let legacy = sql
        .replace("media_assets(", "legacy_media(")
        .replace("536870912", "8388608");
    c.execute_batch(&format!("BEGIN;{legacy};INSERT INTO legacy_media SELECT * FROM media_assets;DROP TABLE media_assets;ALTER TABLE legacy_media RENAME TO media_assets;COMMIT;")).unwrap();
    drop(c);
    let restarted = Store::open(store.config.clone()).unwrap();
    assert_eq!(
        restarted.retained_media(id, revision, 0).unwrap(),
        Some(retained)
    );
    assert_eq!(restarted.media_bytes(id, revision, 0).unwrap().0, png());
    let sql: String = restarted
        .connect()
        .unwrap()
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='media_assets'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(sql.contains("536870912"));
    assert!(!sql.contains("8388608"));
}

#[test]
fn empty_collected_followup_uses_its_own_revision_and_never_inherits_coordinates() {
    let (_tmp, store, parent) = setup();
    let c = store.connect().unwrap();
    let sub: String = c
        .query_row("SELECT id FROM subscriptions LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let state = store.collection_state(&sub).unwrap();
    store.publish_collection_page(&sub,1,json!({"id":"x:user:100","label":"Synthetic media profile"}),vec![json!({"id":"x:post:1001","text":"","published_at":"2026-09-10T10:01:00Z","media":[{"kind":"photo"}],"relationships":{"author":{"id":"100"}}})],json!({"synthetic":true}),state,Arc::new(AtomicBool::new(false))).unwrap();
    let detail = store
        .detail_with_publication(
            parent["id"].as_str().unwrap(),
            parent["revision"].as_str().unwrap(),
            None,
        )
        .unwrap();
    let members = detail["media_context"]["members"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["id"], "x:post:1001");
    let child = store
        .detail_with_publication(
            "x:post:1001",
            members[0]["revision"].as_str().unwrap(),
            None,
        )
        .unwrap();
    assert_eq!(child["text"], "");
    assert!(child["geometry"].is_null());
    assert!(child["map_locations"].as_array().unwrap().is_empty());
    assert_eq!(child["media_context"]["anchor"]["id"], parent["id"]);
}

#[tokio::test]
async fn paused_media_bodies_cannot_exhaust_query_capacity() {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let (_tmp, store, evidence) = setup();
    let id = evidence["id"].as_str().unwrap();
    let rev = evidence["revision"].as_str().unwrap();
    store.retain_media(id, rev, 0, png(), "image/png").unwrap();
    let router = relic_app::api::router(store.clone());
    let uri = format!("/api/v1/media?id={id}&revision={rev}&index=0");
    let request = |uri: &str| {
        Request::builder()
            .uri(uri)
            .header("host", "127.0.0.1:8792")
            .body(Body::empty())
            .unwrap()
    };
    let mut held = vec![];
    for _ in 0..8 {
        let response = router.clone().oneshot(request(&uri)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    assert_eq!(store.reads.available_permits(), 4);
    assert_eq!(
        router
            .clone()
            .oneshot(request(&uri))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request("/api/v1/workspace"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    drop(held);
    assert_eq!(
        router.oneshot(request(&uri)).await.unwrap().status(),
        StatusCode::OK
    );
}
