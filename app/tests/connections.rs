//! Synthetic local-only runtime/API checks. Never reads configuration files
//! or opens a provider session: the timeline source is a probe that records
//! every request and can stall one on purpose.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use relic_app::{
    api,
    collection::Runtime,
    config::Config,
    model::ident,
    sources::{BoxFuture, Image, PostTimeline, SourceFailure, Sources, TimelinePage},
    store::Store,
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{sync::Notify, time::timeout};
use tower::ServiceExt;

fn config(temp: &tempfile::TempDir) -> Config {
    let mut config = Config::local(
        std::env::current_dir().unwrap(),
        temp.path().join("data"),
        "127.0.0.1:8792".parse().unwrap(),
    );
    config.session_dir = temp.path().join("private-session");
    config
}
fn setup() -> (tempfile::TempDir, Store, Arc<Runtime>) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(config(&temp)).unwrap();
    let runtime = Runtime::new(store.clone());
    (temp, store, runtime)
}
/// A configured timeline source that never answers: each request is
/// counted, announced and then left pending until it is cancelled.
#[derive(Default)]
struct Probe {
    requests: AtomicUsize,
    entered: Notify,
}
impl Probe {
    fn stall<T: Send + 'static>(&self) -> BoxFuture<'_, std::result::Result<T, SourceFailure>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        Box::pin(std::future::pending())
    }
    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}
impl PostTimeline for Probe {
    fn account_key(&self) -> String {
        "probe-session".into()
    }
    fn rotates(&self) -> bool {
        false
    }
    fn rotate(&self) -> BoxFuture<'_, std::result::Result<(), SourceFailure>> {
        self.stall()
    }
    fn check(&self) -> BoxFuture<'_, std::result::Result<(), SourceFailure>> {
        self.stall()
    }
    fn resolve<'a>(&'a self, _: &'a str) -> BoxFuture<'a, std::result::Result<Option<u64>, SourceFailure>> {
        self.stall()
    }
    fn page<'a>(&'a self, _: u64, _: Option<&'a str>) -> BoxFuture<'a, std::result::Result<TimelinePage, SourceFailure>> {
        self.stall()
    }
    fn fetch_asset<'a>(&'a self, _: &'a str, _: usize) -> BoxFuture<'a, std::result::Result<Image, SourceFailure>> {
        self.stall()
    }
}
async fn local_proxy() -> (tempfile::TempDir, Store, Arc<Runtime>, Arc<Probe>) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(config(&temp)).unwrap();
    assert!(!relic_app::advisories::polling_allowed(&store));
    let probe = Arc::new(Probe::default());
    let sources = Sources { timeline: Some(probe.clone()), ..Sources::default() };
    let runtime = Runtime::with_sources(store.clone(), &sources);
    (temp, store, runtime, probe)
}
async fn call(
    router: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .uri(path)
        .method(method)
        .header("host", "127.0.0.1:8792");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let response = router
        .clone()
        .oneshot(
            request
                .body(
                    body.map(|v| Body::from(v.to_string()))
                        .unwrap_or_else(Body::empty),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    assert!(
        response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn empty_acquisition(store: &Store) {
    let workspace = store.workspace().unwrap();
    for key in ["sources", "publications", "jobs"] {
        assert!(workspace[key].as_array().unwrap().is_empty(), "{key}");
    }
    assert!(store.collection_states().unwrap().is_empty());
}

#[tokio::test]
async fn connection_collection_and_media_routes_require_workspace_access() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = config(&temp);
    cfg.api_token = Some("synthetic-private-workspace-token".into());
    let store = Store::open(cfg).unwrap();
    let router = api::router(store.clone());
    for (method, path) in [
        ("GET", "/api/v1/connections"),
        ("GET", "/api/v1/collection/metrics"),
        ("POST", "/api/v1/connections/x/connect"),
        ("POST", "/api/v1/connections/x/disconnect"),
        ("POST", "/api/v1/connections/telegram/auth"),
        ("POST", "/api/v1/subscriptions/missing/collect"),
        ("GET", "/api/v1/media?id=x%3Apost%3A100&revision=v1&index=0"),
        ("POST", "/api/v1/media"),
        ("POST", "/api/v1/media/download"),
    ] {
        let (status, body) =
            call(&router, method, path, (method == "POST").then(|| json!({}))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(body["error"]["code"], "unauthorized");
        assert!(
            !body
                .to_string()
                .contains("synthetic-private-workspace-token")
        );
    }
    empty_acquisition(&store);
}

#[tokio::test]
async fn absent_configuration_is_truthful_and_never_creates_acquisition() {
    let (temp, store, runtime) = setup();
    runtime.start();
    let router = api::router_with_runtime(store.clone(), runtime.clone());
    let (status, body) = call(&router, "GET", "/api/v1/connections", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["collection_allowed"], true);
    for provider in body["providers"].as_array().unwrap() {
        assert_eq!(provider["status"], "unconfigured");
        assert!(provider["account_label"].is_null());
        let path = format!(
            "/api/v1/connections/{}/connect",
            provider["provider"].as_str().unwrap()
        );
        assert_eq!(
            call(&router, "POST", &path, Some(json!({}))).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(runtime.metrics()["durable_pages"], 0);
    assert!(!temp.path().join("private-session").exists());
    empty_acquisition(&store);
    store.begin_shutdown();
}

#[tokio::test]
async fn configured_but_unactivated_startup_never_contacts_the_source() {
    let (_temp, store, runtime, probe) = local_proxy().await;
    store.create_subscription(json!({"provider":"x","target":"@synthetic_qa","label":"Synthetic profile","enabled":true})).unwrap();
    runtime.start();
    assert_eq!(runtime.connection("x").unwrap().status, "disconnected");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(probe.requests(), 0);
    assert_eq!(runtime.connection("x").unwrap().status, "disconnected");
    empty_acquisition(&store);
    store.begin_shutdown();
}

#[tokio::test]
async fn fictional_workspace_blocks_connections_auth_and_collection_before_network() {
    let (_temp, store, runtime, probe) = local_proxy().await;
    store
        .import_job(
            serde_json::from_str(include_str!("../../examples/port-investigation.json")).unwrap(),
            ident("job"),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    let source = store.create_subscription(json!({"provider":"x","target":"@synthetic_qa","label":"Synthetic profile","enabled":true})).unwrap();
    let router = api::router_with_runtime(store.clone(), runtime.clone());
    assert_eq!(
        call(&router, "GET", "/api/v1/connections", None).await.1["collection_allowed"],
        false
    );
    for path in [
        "/api/v1/connections/x/connect",
        "/api/v1/connections/telegram/connect",
        "/api/v1/connections/telegram/auth",
    ] {
        assert_eq!(
            call(&router, "POST", path, Some(json!({}))).await.0,
            StatusCode::CONFLICT
        );
    }
    let path = format!(
        "/api/v1/subscriptions/{}/collect",
        source["id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &path,
            Some(json!({"action":"poll","expected_version":1}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(probe.requests(), 0);
    assert!(store.collection_states().unwrap().is_empty());
    assert_eq!(runtime.connection("x").unwrap().status, "disconnected");
}

#[tokio::test]
async fn malformed_auth_and_collection_actions_do_not_change_saved_policy() {
    let (_temp, store, runtime) = setup();
    let source = store.create_subscription(json!({"provider":"x","target":"@synthetic_qa","label":"Synthetic profile","enabled":true})).unwrap();
    let router = api::router_with_runtime(store.clone(), runtime);
    for input in [
        json!({}),
        json!({"phone":"synthetic","code":"synthetic"}),
        json!({"code":7}),
        json!({"code":""}),
    ] {
        let (status, body) = call(
            &router,
            "POST",
            "/api/v1/connections/telegram/auth",
            Some(input),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!body.to_string().contains("synthetic"));
    }
    assert_eq!(
        call(
            &router,
            "POST",
            "/api/v1/connections/telegram/auth",
            Some(json!({"code":"synthetic-code"}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let path = format!(
        "/api/v1/subscriptions/{}/collect",
        source["id"].as_str().unwrap()
    );
    for input in [
        json!({}),
        json!({"action":"poll"}),
        json!({"action":"poll","expected_version":0}),
        json!({"action":"poll","expected_version":1,"url":"https://example.invalid"}),
        json!({"action":"unbounded_backfill","expected_version":1}),
    ] {
        assert_eq!(
            call(&router, "POST", &path, Some(input)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        store.subscription(source["id"].as_str().unwrap()).unwrap(),
        source
    );
    empty_acquisition(&store);
}

#[tokio::test]
async fn saved_policy_update_and_stale_collection_action_keep_exact_version() {
    let (_temp, store, runtime) = setup();
    let source = store.create_subscription(json!({"provider":"x","target":"@synthetic_qa","label":"Synthetic profile","enabled":true})).unwrap();
    let router = api::router_with_runtime(store.clone(), runtime);
    let path = format!("/api/v1/subscriptions/{}", source["id"].as_str().unwrap());
    let (status, updated) = call(&router,"PUT",&path,Some(json!({"provider":"x","target":"@synthetic_qa","label":"Synthetic profile","enabled":false,"mode":"poll","poll_interval_seconds":300,"expected_version":1}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["version"], 2);
    assert_eq!(updated["status"], "paused");
    assert_eq!(
        call(
            &router,
            "POST",
            &format!("{path}/collect"),
            Some(json!({"action":"poll","expected_version":1}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let reopened = Store::open(store.config.clone()).unwrap();
    assert_eq!(
        reopened
            .subscription(source["id"].as_str().unwrap())
            .unwrap()["poll_interval_seconds"],
        300
    );
    empty_acquisition(&store);
}

#[tokio::test]
async fn missing_media_get_is_json_and_never_falls_through_to_network_or_app_html() {
    let (_temp, store, runtime, probe) = local_proxy().await;
    let router = api::router_with_runtime(store.clone(), runtime);
    let (status, body) = call(
        &router,
        "GET",
        "/api/v1/media?id=x%3Apost%3A1000&revision=v1&index=0",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
    for input in [
        json!({"id":"x:post:1000","revision":"v1","index":0,"url":"https://example.invalid"}),
        json!({"id":"x:post:1000","revision":"v1","index":-1}),
    ] {
        assert_eq!(
            call(&router, "POST", "/api/v1/media", Some(input)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(probe.requests(), 0);
    empty_acquisition(&store);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_source_connect_is_cancellable_and_workspace_reads_stay_responsive() {
    let (_temp, store, runtime, probe) = local_proxy().await;
    let connecting = runtime.clone();
    let task = tokio::spawn(async move { connecting.connect("x").await });
    timeout(Duration::from_secs(2), probe.entered.notified())
        .await
        .expect("the configured source should receive the request");
    assert_eq!(runtime.connection("x").unwrap().status, "connecting");
    assert_eq!(
        runtime
            .connect("x")
            .await
            .err()
            .expect("second connection must be rejected")
            .code,
        "capacity_exceeded"
    );
    let router = api::router_with_runtime(store.clone(), runtime.clone());
    for _ in 0..20 {
        let (status, body) = timeout(
            Duration::from_millis(500),
            call(&router, "GET", "/api/v1/workspace", None),
        )
        .await
        .expect("network wait must not hold workspace reads");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["empty"], true);
    }
    let started = Instant::now();
    let disconnected = timeout(Duration::from_millis(500), runtime.disconnect("x"))
        .await
        .expect("disconnect must acknowledge without waiting for the source")
        .unwrap();
    assert_eq!(disconnected.status, "disconnected");
    let result = timeout(Duration::from_millis(500), task)
        .await
        .expect("cancel must terminate the pending connection")
        .unwrap();
    assert_eq!(
        result.err().expect("cancelled connection must fail").code,
        "cancelled"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(runtime.connection("x").unwrap().status, "disconnected");
    assert_eq!(
        store
            .connect()
            .unwrap()
            .query_row(
                "SELECT value FROM metadata WHERE key='collection:x:active'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "false"
    );
    empty_acquisition(&store);
}

#[tokio::test]
async fn exact_unretained_media_get_never_starts_asset_acquisition() {
    let (_temp, store, runtime, probe) = local_proxy().await;
    let subscription = store.create_subscription(json!({"provider":"x","target":"@synthetic_qa","label":"Synthetic media source","enabled":true})).unwrap();
    let id = subscription["id"].as_str().unwrap();
    store.publish_collection_page(id, 1, json!({"id":"x:user:100","label":"Synthetic media source"}), vec![json!({"id":"x:post:1000","text":"Synthetic caption","published_at":"2026-09-10T10:00:00Z","media":[{"kind":"photo","preview_url":"https://pbs.twimg.com/synthetic-never-requested.png"}]})], json!({"synthetic":true}), store.collection_state(id).unwrap(), Arc::new(AtomicBool::new(false))).unwrap();
    let revision: String = store
        .connect()
        .unwrap()
        .query_row(
            "SELECT revision FROM evidence WHERE id='x:post:1000'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let before = store.collection_state(id).unwrap();
    let router = api::router_with_runtime(store.clone(), runtime.clone());
    let path = format!("/api/v1/media?id=x:post:1000&revision={revision}&index=0");
    let (status, body) = call(&router, "GET", &path, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
    assert_eq!(probe.requests(), 0);
    assert!(
        store
            .retained_media("x:post:1000", &revision, 0)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.collection_state(id).unwrap(), before);
    assert_eq!(runtime.connection("x").unwrap().status, "disconnected");
}
