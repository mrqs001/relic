//! Fictional warning records; the parser and retention path are the real ones.
use relic_app::{advisories, config::Config, store::Store};
use serde_json::json;

fn workspace() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let s = Store::open(Config::local(root, tmp.path().join("data"), "127.0.0.1:8796".parse().unwrap())).unwrap();
    (tmp, s)
}
#[test]
fn warnings_are_retained_once_revised_on_change_and_keep_the_raw_record() {
    let (_tmp, s) = workspace();
    let body = json!({"data":[
        {"AlertId":"101","Latitude":"12.6","Longitude":"43.3","RegistrationDate":"2026-09-20T10:00:00.000","ThreatDescription":"Fictional vessel reported an attack near a fictional strait.","AlertTypeType":"Attack","AlertType":"1"},
        {"AlertId":"102","Latitude":"26.5","Longitude":"56.3","RegistrationDate":"2026-09-21T11:30:00","ThreatDescription":"Fictional suspicious approach.","AlertType":"8"}
    ]});
    let raw = serde_json::to_vec(&body).unwrap();
    let (warnings, rejected) = advisories::parse(&body).unwrap();
    assert_eq!((warnings.len(), rejected), (2, 0));
    assert_eq!(s.retain_warnings(&warnings, &raw).unwrap(), (2, 0));
    assert_eq!(s.retain_warnings(&warnings, &raw).unwrap(), (0, 2), "an unchanged list imports nothing");
    // MSCIO shifts every position a little on each request; that alone is not a revision.
    let mut moved = body.clone();
    moved["data"][0]["Latitude"] = json!("12.61");
    moved["data"][1]["Longitude"] = json!("56.28");
    let (shifted, _) = advisories::parse(&moved).unwrap();
    assert_eq!(s.retain_warnings(&shifted, &serde_json::to_vec(&moved).unwrap()).unwrap(), (0, 2));
    let layer = s.warnings(None).unwrap();
    let features = layer["features"].as_array().unwrap();
    assert_eq!(features.len(), 2);
    let attack = features.iter().find(|f| f["properties"]["alert_id"] == "101").unwrap();
    assert_eq!(attack["geometry"]["coordinates"], json!([43.3, 12.6]));
    assert_eq!(attack["properties"]["type"], "Attack");
    assert_eq!(attack["properties"]["registered_at"], "2026-09-20T10:00:00.000Z");
    let precision: String = s.connect().unwrap().query_row("SELECT json_extract(json,'$.location_precision') FROM evidence WHERE id='mscio:alert:101'", [], |r| r.get(0)).unwrap();
    assert_eq!(precision, advisories::PRECISION);
    // Changed issuer wording is a new revision; the old one stays retained.
    let mut changed = body.clone();
    changed["data"][0]["ThreatDescription"] = json!("Fictional vessel reported an attack near a fictional strait. Crew safe.");
    let (w2, _) = advisories::parse(&changed).unwrap();
    assert_eq!(s.retain_warnings(&w2, &serde_json::to_vec(&changed).unwrap()).unwrap(), (1, 1));
    let revisions: i64 = s.connect().unwrap().query_row("SELECT count(*) FROM evidence WHERE id='mscio:alert:101'", [], |r| r.get(0)).unwrap();
    assert_eq!(revisions, 2);
    let layer = s.warnings(None).unwrap();
    let latest = layer["features"].as_array().unwrap().iter().find(|f| f["properties"]["alert_id"] == "101").unwrap().clone();
    assert!(latest["properties"]["text"].as_str().unwrap().ends_with("Crew safe."));
    // The exact provider record travels in the retained original.
    let hash: String = s.connect().unwrap().query_row("SELECT json_extract(json,'$.object_hash') FROM evidence WHERE id='mscio:alert:102'", [], |r| r.get(0)).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&s.original_object(&hash).unwrap()).unwrap();
    assert_eq!(original["provider_record"]["AlertType"], "8");
    assert_eq!(original["advisory"]["type"], "Suspicious activity");
    assert!(original["advisory"]["time_basis"].as_str().unwrap().contains("inferred"));
}
#[test]
fn a_fictional_workspace_never_mixes_in_real_warnings() {
    let (_tmp, s) = workspace();
    s.import_job(json!({"schema_version":1,"fictional":true,"title":"Fictional","sources":[{"id":"fiction:a","label":"Fictional A","family":"authored_report"}],"evidence_revisions":[{"id":"fiction:x","revision":"v1","source":"fiction:a","published_at":"2026-09-20T10:00:00Z","acquired_at":"2026-09-20T10:01:00Z","text":"Fictional text.","geometry":null,"location_precision":"unresolved","origin_group":null,"dependence":"unknown","language":"en","fictional":true,"availability":"available","citation":{"evidence_id":"fiction:x","revision":"v1","start_character":0,"end_character":15}}]}), "job:f".into(), std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))).unwrap();
    let body = json!({"data":[{"AlertId":"1","Latitude":"1","Longitude":"1","RegistrationDate":"2026-09-20T10:00:00","ThreatDescription":"Real warning."}]});
    let (w, _) = advisories::parse(&body).unwrap();
    assert!(s.retain_warnings(&w, &serde_json::to_vec(&body).unwrap()).is_err());
}

#[test]
fn warnings_are_polled_only_with_a_private_configuration_on_a_real_workspace() {
    let (_tmp, s) = workspace();
    // Tests and credential-free review servers have no private configuration.
    assert!(!advisories::polling_allowed(&s));
    // Source values alone (a test's synthetic X values, or process environment
    // variables) are not a private configuration file.
    let mut values = s.clone();
    values.config.source_values.insert("X_AUTH_TOKEN".into(), "synthetic".into());
    values.config.source_values.insert("KEELWATCH_URL".into(), "http://127.0.0.1:9".into());
    assert!(!advisories::polling_allowed(&values));
    let mut configured = s.clone();
    configured.config.explicit_env_file = true;
    assert!(advisories::polling_allowed(&configured));
    // Never for a fictional workspace, even with the private configuration.
    s.import_job(json!({"schema_version":1,"fictional":true,"title":"Fictional","sources":[{"id":"fiction:a","label":"Fictional A","family":"authored_report"}],"evidence_revisions":[{"id":"fiction:y","revision":"v1","source":"fiction:a","published_at":"2026-09-20T10:00:00Z","acquired_at":"2026-09-20T10:01:00Z","text":"Fictional text.","geometry":null,"location_precision":"unresolved","origin_group":null,"dependence":"unknown","language":"en","fictional":true,"availability":"available","citation":{"evidence_id":"fiction:y","revision":"v1","start_character":0,"end_character":15}}]}), "job:g".into(), std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))).unwrap();
    assert!(!advisories::polling_allowed(&configured));
}

#[test]
fn only_an_explicit_env_file_marks_the_private_configuration() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_str().unwrap().to_owned();
    let data = tmp.path().join("data").to_str().unwrap().to_owned();
    let env = tmp.path().join("fictional.env");
    std::fs::write(&env, "KEELWATCH_FICTIONAL_CHECK=1\n").unwrap();
    let args = |extra: &[&str]| {
        ["--root", root.as_str(), "--data-dir", data.as_str()].iter().chain(extra).map(|s| s.to_string()).collect::<Vec<_>>()
    };
    assert!(!Config::from_args(args(&[])).unwrap().explicit_env_file);
    let configured = Config::from_args(args(&["--env", env.to_str().unwrap()])).unwrap();
    assert!(configured.explicit_env_file);
    assert_eq!(configured.source_values.get("KEELWATCH_FICTIONAL_CHECK").map(String::as_str), Some("1"));
    assert!(!Config::local(root.into(), data.into(), "127.0.0.1:8796".parse().unwrap()).explicit_env_file);
}

type Reply = std::sync::Arc<std::sync::Mutex<(u16, String)>>;
/// A stand-in MSCIO list: each poll gets the status and body set last.
async fn mscio() -> (String, Reply) {
    use axum::{Router, extract::State, routing::get};
    let reply: Reply = std::sync::Arc::new(std::sync::Mutex::new((200, "{\"data\":[]}".to_string())));
    let app = Router::new()
        .route("/alerts/filter", get(|State(r): State<Reply>| async move {
            let (status, body) = r.lock().unwrap().clone();
            (axum::http::StatusCode::from_u16(status).unwrap(), [("content-type", "application/json")], body)
        }))
        .with_state(reply.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/alerts/filter", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, reply)
}
fn alert(id: &str) -> serde_json::Value {
    json!({"AlertId":id,"Latitude":"12.6","Longitude":"43.3","RegistrationDate":"2026-10-01T10:00:00","ThreatDescription":format!("Fictional warning {id}."),"AlertType":"1"})
}
fn list(ids: &[&str]) -> String {
    json!({"data":ids.iter().map(|id| alert(id)).collect::<Vec<_>>()}).to_string()
}
fn client() -> wreq::Client {
    wreq::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(5)).build().unwrap()
}
fn feature(layer: &serde_json::Value, alert_id: &str) -> serde_json::Value {
    layer["features"].as_array().unwrap().iter().find(|f| f["properties"]["alert_id"] == alert_id).unwrap()["properties"].clone()
}
fn retained(s: &Store, id: &str) -> Vec<(String, String)> {
    s.connect().unwrap().prepare("SELECT revision,json FROM evidence WHERE id=?1 ORDER BY rowid").unwrap()
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
}

#[tokio::test]
async fn a_warning_is_unlisted_only_by_a_complete_successful_poll_and_listed_again_when_it_returns() {
    let (_tmp, s) = workspace();
    let (url, reply) = mscio().await;
    let http = client();
    *reply.lock().unwrap() = (200, list(&["201", "202", "203"]));
    let first = advisories::poll_once(&s, &http, &url).await;
    assert_eq!((first["ok"].clone(), first["new"].clone()), (json!(true), json!(3)));
    assert_eq!(first["listing"]["judged"], true);
    let layer = s.warnings(None).unwrap();
    assert!(layer["features"].as_array().unwrap().iter().all(|f| f["properties"]["listed"] == true && f["properties"]["unlisted_since"].is_null()));
    let before = retained(&s, "mscio:alert:203");

    // Failed, malformed, empty and partial lists never unlist anything.
    for (status, body) in [
        (500, "{}".to_string()),
        (200, "not json".to_string()),
        (200, "{\"results\":[]}".to_string()),
        (200, "{\"data\":[]}".to_string()),
        (200, json!({"data":[alert("201"), alert("202"), {"Latitude":"1","Longitude":"1","ThreatDescription":"No identity."}]}).to_string()),
    ] {
        *reply.lock().unwrap() = (status, body.clone());
        let polled = advisories::poll_once(&s, &http, &url).await;
        assert_eq!(polled["listing"]["judged"], false, "{body}");
        assert!(polled["listing"]["reason"].as_str().is_some_and(|r| !r.is_empty()), "{body}");
        assert_eq!(feature(&s.warnings(None).unwrap(), "203")["listed"], true, "{body}");
    }

    // Polls are dated to the millisecond and a release build can run several
    // within one; the past views below need each change at its own time.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    // A complete list without 203: it is no longer listed, from this poll on.
    *reply.lock().unwrap() = (200, list(&["201", "202"]));
    let missing = advisories::poll_once(&s, &http, &url).await;
    assert_eq!((missing["ok"].clone(), missing["listing"]["judged"].clone(), missing["listing"]["unlisted"].clone()), (json!(true), json!(true), json!(1)));
    let gone = feature(&s.warnings(None).unwrap(), "203");
    assert_eq!(gone["listed"], false);
    assert_eq!(gone["unlisted_since"], missing["checked_at"]);
    assert_eq!(gone["last_listed_at"], first["checked_at"]);
    assert_eq!(feature(&s.warnings(None).unwrap(), "201")["listed"], true);
    // Still missing next time: the first poll without it stays the date.
    let again = advisories::poll_once(&s, &http, &url).await;
    assert_eq!(again["listing"]["unlisted"], 0);
    assert_eq!(feature(&s.warnings(None).unwrap(), "203")["unlisted_since"], missing["checked_at"]);
    // Every retained revision stays as it was: the state is recorded beside it.
    assert_eq!(retained(&s, "mscio:alert:203"), before);

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    // It returns: listed again, and both changes stay on record.
    *reply.lock().unwrap() = (200, list(&["201", "202", "203"]));
    let back = advisories::poll_once(&s, &http, &url).await;
    assert_eq!(back["listing"]["relisted"], 1);
    let returned = feature(&s.warnings(None).unwrap(), "203");
    assert_eq!((returned["listed"].clone(), returned["unlisted_since"].clone()), (json!(true), json!(null)));
    let changes: Vec<(i64, String)> = s.connect().unwrap().prepare("SELECT listed,at FROM mscio_listing_changes WHERE evidence_id='mscio:alert:203' ORDER BY id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(changes, vec![(0, missing["checked_at"].as_str().unwrap().to_string()), (1, back["checked_at"].as_str().unwrap().to_string())]);
    assert_eq!(retained(&s, "mscio:alert:203"), before);

    // A past view reads the listing as it was then.
    let during = (chrono::DateTime::parse_from_rfc3339(missing["checked_at"].as_str().unwrap()).unwrap() + chrono::Duration::milliseconds(1)).to_rfc3339();
    let then = feature(&s.warnings_at(None, Some(&during)).unwrap(), "203");
    assert_eq!((then["listed"].clone(), then["unlisted_since"].clone()), (json!(false), missing["checked_at"].clone()));
    let earlier = feature(&s.warnings_at(None, Some(first["checked_at"].as_str().unwrap())).unwrap(), "203");
    assert_eq!(earlier["listed"], true);
    assert!(s.warnings_at(None, Some("yesterday")).is_err());
}

#[tokio::test]
async fn a_sudden_drop_waits_for_the_same_list_again() {
    let (_tmp, s) = workspace();
    let (url, reply) = mscio().await;
    let http = client();
    let ids: Vec<String> = (300..312).map(|i| i.to_string()).collect();
    *reply.lock().unwrap() = (200, list(&ids.iter().map(String::as_str).collect::<Vec<_>>()));
    advisories::poll_once(&s, &http, &url).await;
    *reply.lock().unwrap() = (200, list(&["300", "301"]));
    let drop = advisories::poll_once(&s, &http, &url).await;
    assert_eq!(drop["listing"]["judged"], false);
    assert!(drop["listing"]["reason"].as_str().unwrap().contains("12"));
    assert!(s.warnings(None).unwrap()["features"].as_array().unwrap().iter().all(|f| f["properties"]["listed"] == true));
    // The same smaller list again confirms it.
    let confirmed = advisories::poll_once(&s, &http, &url).await;
    assert_eq!((confirmed["listing"]["judged"].clone(), confirmed["listing"]["unlisted"].clone()), (json!(true), json!(10)));
}

#[tokio::test]
async fn warnings_health_follows_the_poller_only_where_it_may_poll() {
    let (_tmp, s) = workspace();
    // Tests and review servers never poll: the layer is off, not failing.
    let off = advisories::health(&s).unwrap();
    assert_eq!((off["id"].clone(), off["state"].clone()), (json!("warnings"), json!("off")));
    let mut polling = s.clone();
    polling.config.explicit_env_file = true;
    assert_eq!(advisories::health(&polling).unwrap()["state"], "unknown", "never polled yet");
    let (url, reply) = mscio().await;
    let http = client();
    *reply.lock().unwrap() = (200, list(&["401", "402"]));
    let ok = advisories::poll_once(&s, &http, &url).await;
    assert_eq!(advisories::health(&polling).unwrap()["state"], "ok");
    *reply.lock().unwrap() = (502, "{}".into());
    let failed = advisories::poll_once(&s, &http, &url).await;
    assert_eq!(failed["last_success_at"], ok["checked_at"]);
    let h = advisories::health(&polling).unwrap();
    assert_eq!((h["state"].clone(), h["since"].clone()), (json!("degraded"), failed["checked_at"].clone()));
    assert!(h["detail"].as_str().unwrap().contains("HTTP 502"));
    // A second failure keeps the first failure's time.
    advisories::poll_once(&s, &http, &url).await;
    assert_eq!(advisories::health(&polling).unwrap()["since"], failed["checked_at"]);
    // An empty list where warnings were listed is suspicious.
    *reply.lock().unwrap() = (200, "{\"data\":[]}".into());
    advisories::poll_once(&s, &http, &url).await;
    let empty = advisories::health(&polling).unwrap();
    assert_eq!(empty["state"], "degraded");
    assert!(empty["detail"].as_str().unwrap().contains("empty"));
    // A last success older than 30 minutes is overdue.
    let old = (chrono::Utc::now() - chrono::Duration::minutes(40)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    s.connect().unwrap().execute("UPDATE metadata SET value=?1 WHERE key='mscio_status'", [json!({"checked_at":old,"last_success_at":old,"ok":true,"warnings":2,"listing":{"judged":true}}).to_string()]).unwrap();
    let overdue = advisories::health(&polling).unwrap();
    assert_eq!((overdue["state"].clone(), overdue["since"].clone()), (json!("degraded"), json!(old)));
}

/// A stand-in KeelWatch answering health and its collection record.
async fn keelwatch(health: serde_json::Value, collection: serde_json::Value) -> String {
    use axum::{Json, Router, routing::get};
    let app = Router::new()
        .route("/api/v1/health", get(move || { let h = health.clone(); async move { Json(h) } }))
        .route("/api/v1/history/collection", get(move || { let c = collection.clone(); async move { Json(c) } }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}
async fn get(app: &axum::Router, uri: &str) -> serde_json::Value {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let response = app.clone().oneshot(axum::http::Request::builder().uri(uri).header("host", "127.0.0.1:8796").body(axum::body::Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), 200, "{uri}");
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}
fn router_with(values: &[(&str, String)]) -> (tempfile::TempDir, axum::Router) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let mut config = Config::local(root, tmp.path().join("data"), "127.0.0.1:8796".parse().unwrap());
    for (k, v) in values {
        config.source_values.insert(k.to_string(), v.clone());
    }
    let s = Store::open(config).unwrap();
    (tmp, relic_app::api::router(s))
}

#[tokio::test]
async fn attention_counts_a_layer_that_is_not_collecting_and_leaves_healthy_layers_quiet() {
    let now = chrono::Utc::now();
    let at = |minutes: i64| (now - chrono::Duration::minutes(minutes)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    // KeelWatch answers, but no collection run started for 70 minutes.
    let stopped = keelwatch(json!({"status":"ok"}), json!({"from":at(2880),"to":at(0),"gaps":[{"from":at(70),"to":at(0),"seconds":4200}],"gap_seconds":4200,"collecting_seconds":168600,"sources":[{"source":"a","runs":10,"last_run":at(70)}]})).await;
    let (_tmp, app) = router_with(&[("KEELWATCH_URL", stopped)]);
    let attention = get(&app, "/api/v1/source-attention").await;
    assert_eq!((attention["count"].clone(), attention["sources"].clone()), (json!(1), json!(0)));
    let layers = attention["layers"].as_array().unwrap();
    assert_eq!(layers.len(), 1);
    assert_eq!((layers[0]["id"].clone(), layers[0]["state"].clone(), layers[0]["since"].clone()), (json!("ships"), json!("degraded"), json!(at(70))));
    assert!(layers[0]["detail"].as_str().unwrap().contains("not been collecting since"));
    assert!(layers[0]["detail"].as_str().unwrap().contains("missing, not absent"));
    let cards = get(&app, "/api/v1/layers").await;
    let card = |id: &str| cards["layers"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap().clone();
    assert_eq!(card("ships")["health"]["state"], "degraded");
    assert!(card("ships")["health"]["checked_at"].is_string());
    assert_eq!(card("aircraft")["health"]["state"], "not_configured");
    assert_eq!((card("warnings")["provider"].clone(), card("warnings")["health"]["state"].clone()), (json!("MSCIO"), json!("off")));

    // A collecting KeelWatch and layers not included in this build need nothing.
    let collecting = keelwatch(json!({"status":"ok"}), json!({"from":at(2880),"to":at(0),"gaps":[{"from":at(600),"to":at(500),"seconds":6000}],"sources":[{"source":"a","runs":900,"last_run":at(1)}]})).await;
    let (_tmp2, app) = router_with(&[("KEELWATCH_URL", collecting)]);
    let quiet = get(&app, "/api/v1/source-attention").await;
    assert_eq!(quiet, json!({"count":0,"sources":0,"layers":[]}));
    let cards = get(&app, "/api/v1/layers").await;
    let state = |id: &str| cards["layers"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap()["health"]["state"].clone();
    assert_eq!((state("ships"), state("aircraft"), state("marinetraffic")), (json!("ok"), json!("not_configured"), json!("not_configured")));

    // KeelWatch reporting degraded counts too.
    let degraded = keelwatch(json!({"status":"degraded","degraded":["sweep failing: a north"]}), json!({"from":at(2880),"to":at(0),"gaps":[],"sources":[{"source":"a","runs":900,"last_run":at(1)}]})).await;
    let (_tmp3, app) = router_with(&[("KEELWATCH_URL", degraded)]);
    let reported = get(&app, "/api/v1/source-attention").await;
    assert_eq!(reported["count"], 1);
    assert!(reported["layers"][0]["detail"].as_str().unwrap().contains("sweep failing"));
}

/// Fictional warnings, each registered the given hours ago.
fn registered_ago(s: &Store, hours: &[(&str, i64)]) {
    let now = chrono::Utc::now();
    let rows = hours.iter().map(|(id, h)| json!({"AlertId":id,"Latitude":"12.6","Longitude":"43.3","RegistrationDate":(now - chrono::Duration::hours(*h)).format("%Y-%m-%dT%H:%M:%S").to_string(),"ThreatDescription":format!("Fictional warning {id}."),"AlertType":"8"})).collect::<Vec<_>>();
    let body = json!({"data":rows});
    let (warnings, _) = advisories::parse(&body).unwrap();
    s.retain_warnings(&warnings, &serde_json::to_vec(&body).unwrap()).unwrap();
}
fn ids(layer: &serde_json::Value) -> Vec<String> {
    layer["features"].as_array().unwrap().iter().map(|f| f["properties"]["alert_id"].as_str().unwrap().to_owned()).collect()
}
#[test]
fn warnings_filter_by_hours_counted_back_from_now_or_the_chosen_moment() {
    let (_tmp, s) = workspace();
    registered_ago(&s, &[("501", 2), ("502", 30), ("503", 5 * 24), ("504", 40 * 24)]);
    // Newest first; a short window still says how many are retained.
    let six = s.warnings_within(Some(6), None).unwrap();
    assert_eq!((ids(&six), six["retained"].clone(), six["window"]["hours"].clone()), (vec!["501".to_string()], json!(4), json!(6)));
    assert!(six["window"]["from"].is_string() && six["window"]["to"].is_null());
    assert_eq!(ids(&s.warnings_within(Some(48), None).unwrap()), ["501", "502"]);
    assert_eq!(ids(&s.warnings_within(Some(72), None).unwrap()), ["501", "502"]);
    assert_eq!(ids(&s.warnings_within(Some(7 * 24), None).unwrap()), ["501", "502", "503"]);
    assert_eq!(ids(&s.warnings_within(None, None).unwrap()).len(), 4);
    // Whole days keep working as before.
    assert_eq!(ids(&s.warnings(Some(7)).unwrap()), ["501", "502", "503"]);
    // At a past moment: registered by then, and counted back from then.
    let day_ago = (chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    let then = s.warnings_within(Some(12), Some(&day_ago)).unwrap();
    assert_eq!((ids(&then), then["retained"].clone()), (vec!["502".to_string()], json!(3)));
    assert_eq!(ids(&s.warnings_within(None, Some(&day_ago)).unwrap()), ["502", "503", "504"]);
}
#[tokio::test]
async fn the_warnings_route_takes_hours_and_marinetraffic_routes_say_what_they_need() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (_tmp, app) = router_with(&[]);
    let status = |uri: &'static str| {
        let app = app.clone();
        async move {
            let r = app.oneshot(axum::http::Request::builder().uri(uri).header("host", "127.0.0.1:8796").body(axum::body::Body::empty()).unwrap()).await.unwrap();
            let code = r.status().as_u16();
            (code, serde_json::from_slice::<serde_json::Value>(&r.into_body().collect().await.unwrap().to_bytes()).unwrap_or_default())
        }
    };
    let (code, body) = status("/api/v1/layers/warnings?hours=6").await;
    assert_eq!((code, body["window"]["hours"].clone(), body["retained"].clone()), (200, json!(6), json!(0)));
    assert_eq!(status("/api/v1/layers/warnings?days=3").await.1["window"]["hours"], 72);
    for bad in ["/api/v1/layers/warnings?hours=0", "/api/v1/layers/warnings?hours=6h", "/api/v1/layers/warnings?hours=100000"] {
        assert_eq!(status(bad).await.0, 400, "{bad}");
    }
    // Without the ship-snapshot adapter: unavailable, saying why; nothing is sent.
    for uri in ["/api/v1/layers/ships/marinetraffic?west=55&south=25&east=57&north=27&zoom=8", "/api/v1/layers/ships/marinetraffic/700001", "/api/v1/layers/ships/marinetraffic/700001/photo"] {
        let (code, body) = status(uri).await;
        assert_eq!((code, body["error"]["code"].clone()), (503, json!("not_configured")), "{uri}");
        assert!(body["error"]["message"].as_str().unwrap().contains("Not included in this build"));
    }
    let cards = get(&app, "/api/v1/layers").await;
    let mt = cards["layers"].as_array().unwrap().iter().find(|c| c["id"] == "marinetraffic").unwrap().clone();
    assert_eq!((mt["provider"].clone(), mt["configured"].clone(), mt["health"]["state"].clone()), (json!("MarineTraffic"), json!(false), json!("not_configured")));
    // KeelWatch's own routes are unchanged beside the new ones.
    assert_eq!(status("/api/v1/layers/ships/search?q=ab").await.1["error"]["code"], "not_configured");
    // An unconfigured MarineTraffic never needs attention.
    assert_eq!(get(&app, "/api/v1/source-attention").await["layers"].as_array().unwrap().iter().filter(|l| l["id"] == "marinetraffic").count(), 0);
}
