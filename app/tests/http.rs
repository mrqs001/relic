use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use relic_app::{api, config::Config, store::Store};
use serde_json::Value;
use tower::ServiceExt;
fn app(token: Option<&str>) -> (tempfile::TempDir, axum::Router) {
    let tmp = tempfile::tempdir().unwrap();
    let mut config = Config::local(
        std::env::current_dir().unwrap(),
        tmp.path().join("data"),
        "127.0.0.1:8792".parse().unwrap(),
    );
    config.api_token = token.map(String::from);
    let store = Store::open(config).unwrap();
    (tmp, api::router(store))
}
fn request(uri: &str) -> axum::http::request::Builder {
    Request::builder().uri(uri).header("host", "127.0.0.1:8792")
}
#[tokio::test]
async fn host_and_origin_boundary_rejects_cross_site_requests() {
    let (_tmp, app) = app(None);
    let foreign = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/workspace")
                .header("host", "evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    let cross = app
        .clone()
        .oneshot(
            request("/api/v1/workspace")
                .header("origin", "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross.status(), StatusCode::FORBIDDEN);
    let bad = app
        .oneshot(
            request("/api/v1/workspace")
                .header(
                    "origin",
                    axum::http::HeaderValue::from_bytes(b"\xff").unwrap(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn configured_workspace_requires_session_but_never_returns_token() {
    let (_tmp, app) = app(Some("synthetic-test-only-token"));
    let unauthorized = app
        .clone()
        .oneshot(request("/api/v1/workspace").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(
            request("/api/v1/session/login")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"token":"synthetic-test-only-token"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(!String::from_utf8_lossy(&body).contains("synthetic-test-only-token"));
    let authorized = app
        .oneshot(
            request("/api/v1/workspace")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized.status(), StatusCode::OK);
    let data: Value =
        serde_json::from_slice(&authorized.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(data["empty"], true);
}
#[tokio::test]
async fn original_upload_download_round_trip_preserves_whitespace() {
    let (_tmp, app) = app(None);
    let original = include_bytes!("../../examples/port-investigation.json");
    let response = app
        .clone()
        .oneshot(
            request("/api/v1/imports")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(original.as_slice()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let summary: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let url = format!(
        "/api/v1/objects/{}",
        summary["original_input_hash"].as_str().unwrap()
    );
    let downloaded = app
        .oneshot(request(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(downloaded.status(), StatusCode::OK);
    assert_eq!(
        downloaded
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        original
    );
}

#[tokio::test]
async fn in_memory_connection_change_reaches_existing_sse_clients() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(Config::local(
        std::env::current_dir().unwrap(),
        temp.path().join("data"),
        "127.0.0.1:8792".parse().unwrap(),
    ))
    .unwrap();
    let app = api::router(store.clone());
    let response = app
        .oneshot(request("/api/v1/events").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    store
        .events
        .send(serde_json::json!({"kind":"resync"}))
        .unwrap();
    let mut body = response.into_body();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(1), body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let bytes = frame.into_data().unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("event: resync"));
}
