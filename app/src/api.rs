use crate::{
    access::{self, Access},
    collection::Runtime as CollectionRuntime,
    error::{Error, Result},
    model::{Query as ScopeQuery, ident},
    store::{MAX_IMPORT_BYTES, Store, markdown_export},
    subscriptions::SubscriptionQuery,
};
use axum::{
    Extension, Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, Request, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post, put},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    convert::Infallible,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use tower_http::services::{ServeDir, ServeFile};

type Body = std::result::Result<Json<Value>, JsonRejection>;
fn body(input: Body) -> Result<Value> {
    input
        .map(|Json(v)| v)
        .map_err(|_| Error::invalid("Expected valid bounded JSON request"))
}
async fn work<F>(s: Store, f: F) -> Result<Json<Value>>
where
    F: FnOnce(Store) -> Result<Value> + Send + 'static,
{
    let workers = s.reads.clone();
    let admission = s.read_admission.clone();
    admitted_work(s, workers, admission, f).await
}
async fn control<F>(s: Store, f: F) -> Result<Json<Value>>
where
    F: FnOnce(Store) -> Result<Value> + Send + 'static,
{
    let workers = s.controls.clone();
    let admission = s.control_admission.clone();
    admitted_work(s, workers, admission, f).await
}
async fn admitted_work<F>(
    s: Store,
    workers: Arc<tokio::sync::Semaphore>,
    admission: Arc<tokio::sync::Semaphore>,
    f: F,
) -> Result<Json<Value>>
where
    F: FnOnce(Store) -> Result<Value> + Send + 'static,
{
    if s.shutdown.load(std::sync::atomic::Ordering::Acquire) {
        return Err(Error::busy());
    }
    let admitted = admission.try_acquire_owned().map_err(|_| Error::busy())?;
    let permit = tokio::time::timeout(Duration::from_millis(250), workers.acquire_owned())
        .await
        .map_err(|_| Error::busy())?
        .map_err(|_| Error::busy())?;
    if s.shutdown.load(std::sync::atomic::Ordering::Acquire) {
        return Err(Error::busy());
    }
    let result = tokio::task::spawn_blocking(move || {
        let (_permit, _admitted) = (permit, admitted);
        f(s)
    })
    .await
    .map_err(|_| Error::internal())??;
    Ok(Json(result))
}

pub fn router(store: Store) -> Router {
    let runtime = CollectionRuntime::new(store.clone());
    router_with_runtime(store, runtime)
}
pub fn router_with_runtime(store: Store, runtime: Arc<CollectionRuntime>) -> Router {
    router_with_sources(store, runtime, crate::sources::Sources::default())
}
/// The router with the source adapters this build registered.
pub fn router_with_sources(store: Store, runtime: Arc<CollectionRuntime>, sources: crate::sources::Sources) -> Router {
    let processing = crate::processing::Runtime::new(store.clone());
    processing.start();
    crate::supplemental::start(store.clone());
    crate::event_link_worker::start(store.clone());
    crate::advisories::start(store.clone());
    let layers = Arc::new(crate::live_layers::LiveLayers::new(&store.config.source_values).with_aircraft(sources.aircraft.clone()));
    let vessels = crate::vessel_batch::VesselBatch::new(store.clone(), layers.clone());
    let marinetraffic: Arc<dyn crate::sources::ShipSnapshotLayer> = sources.ship_snapshots.clone().unwrap_or_else(|| Arc::new(crate::sources::NoShipSnapshots));
    vessels.start();
    let static_files = ServeDir::new(store.config.under_root("web/dist")).fallback(ServeFile::new(
        store.config.under_root("web/dist/index.html"),
    ));
    let access = Access::new(store.config.api_token.clone());
    let sessions = Router::new()
        .route("/session", get(access::session_status))
        .route("/session/login", post(access::login))
        .route("/session/logout", post(access::logout))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(access.clone());
    let api = Router::new()
        .merge(
            Router::new()
                .route(
                    "/subscriptions",
                    get(subscriptions).post(create_subscription),
                )
                .route(
                    "/subscriptions/{id}",
                    get(subscription).put(update_subscription),
                )
                .route("/subscriptions/{id}/collect", post(collect_source))
                .route("/connections", get(connections))
                .route("/connections/{provider}/connect", post(connect_provider))
                .route(
                    "/connections/{provider}/disconnect",
                    post(disconnect_provider),
                )
                .route("/connections/telegram/auth", post(telegram_auth))
                .route("/collection/metrics", get(collection_metrics))
                .route("/media", get(retained_media).post(load_media))
                .route("/media/download", post(download_media))
                .route("/boundaries", get(boundaries_status))
                .route("/area-selection", post(area_selection))
                .route("/osm", get(osm_status))
                .route("/geography", get(geography_status))
                .route("/geography/reresolve", post(geography_reresolve))
                .route("/processing", get(processing_status))
                .route("/processing/failures", get(processing_failures))
                .route("/source-attention", get(source_attention))
                .route("/processing/policy", put(processing_policy))
                .route("/processing/sources/{id}", put(processing_source))
                .route("/processing/supplemental", put(supplemental_policy))
                .route("/processing/relationships", put(relationship_policy))
                .route("/processing/run", post(processing_run))
                .route("/processing/stop", post(processing_stop))
                .route("/processing/recover", post(processing_recover))
                .route("/processing/retry-interrupted", post(processing_retry_interrupted))
                .layer(DefaultBodyLimit::max(4096)),
        )
        // Reference activation takes a list of edition folders with no count
        // cap (79 boundary editions are about 4 KB); 1 MB bounds the request.
        .merge(
            Router::new()
                .route("/boundaries/activate", post(boundaries_activate))
                .route("/osm/activate", post(osm_activate))
                .route("/geography/activate", post(geography_activate))
                .layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route("/health", get(health))
        .route("/event-links/query", post(relationship_query))
        .route("/event-links/group", post(relationship_group))
        .route("/workspace", get(workspace))
        .route("/imports/demo", post(demo_import))
        .route("/imports", post(import))
        .route("/query", post(query))
        .route("/query/preview", post(query_preview))
        .route("/query/page", post(query_page))
        .route("/area-shape", get(area_shape))
        .route("/evidence", get(evidence))
        .route("/messages/{key}", get(message_reference))
        .route("/objects/{hash}", get(original_object))
        .route("/investigations", post(create_investigation))
        .route("/investigations/{id}", put(update_investigation))
        .route("/investigations/{id}/assessments", post(assessment))
        .route("/briefs", post(save_brief))
        .route("/briefs/{id}", get(brief))
        .route("/briefs/{id}/export", get(export))
        .route("/lineage", get(lineage))
        .route("/lineage/message", get(message_lineage))
        .route("/watches/preview", post(watch_preview))
        .route("/watches", post(create_watch))
        .route("/watches/{id}", put(update_watch))
        .route("/alerts", get(alerts))
        .route("/alerts/{id}", put(update_alert))
        .route("/jobs", get(jobs))
        .route("/jobs/{id}/cancel", post(cancel_job))
        .route("/backup", post(backup))
        .route("/backup/{id}/verify", post(verify))
        .route("/events", get(events))
        // Live layers can carry tens of thousands of vessels; compressed.
        .merge(
            Router::new()
                .route("/layers", get(layers_status))
                .route("/layers/ships/tiles/{z}/{x}/{y}", get(layer_ship_tile))
                .route("/layers/ships/search", get(layer_ship_search))
                .route("/layers/ships/coverage", get(layer_ship_coverage))
                .route("/layers/ships/marinetraffic", get(layer_marinetraffic_ships))
                .route("/layers/ships/marinetraffic/{ship}", get(layer_marinetraffic_ship))
                .route("/layers/ships/marinetraffic/{ship}/photo", get(layer_marinetraffic_photo))
                .route("/layers/ships/{id}", get(layer_ship))
                .route("/layers/ships/{id}/track", get(layer_ship_track))
                .route("/layers/ships/{id}/names", get(layer_ship_names))
                .route("/layers/ships/{id}/messages", get(layer_ship_messages))
                .route("/layers/aircraft", get(layer_aircraft))
                .route("/layers/aircraft/search", get(layer_aircraft_search))
                .route("/layers/aircraft/top", get(layer_aircraft_top))
                .route("/layers/aircraft/{flight}", get(layer_flight))
                .route("/layers/aircraft/{flight}/photo", get(layer_aircraft_photo))
                .route("/layers/warnings", get(layer_warnings))
                .route("/evidence/vessels", get(message_vessels).post(find_message_vessels))
                .route("/evidence/vessels/decision", put(decide_message_vessel))
                .route("/evidence/vessels/marks", post(message_vessel_marks))
                .route("/evidence/vessels/nearby", get(message_nearby_vessels))
                .route("/processing/vessels", get(vessel_batch_status).post(vessel_batch_action).put(vessel_batch_auto))
                .layer(tower_http::compression::CompressionLayer::new()),
        )
        .route_layer(middleware::from_fn_with_state(
            access,
            access::require_access,
        ))
        .merge(sessions);
    Router::new()
        .nest("/api/v1", api)
        .fallback_service(static_files)
        .layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES))
        .layer(Extension(runtime))
        .layer(Extension(processing))
        .layer(Extension(layers))
        .layer(Extension(vessels))
        .layer(Extension(marinetraffic))
        .layer(middleware::from_fn_with_state(
            store.clone(),
            local_boundary,
        ))
        .with_state(store)
}
type Layers = Extension<Arc<crate::live_layers::LiveLayers>>;
type MarineTrafficLayer = Extension<Arc<dyn crate::sources::ShipSnapshotLayer>>;
type Params = axum::extract::Query<std::collections::BTreeMap<String, String>>;
/// Layer status cards, each with its current `health`, the MarineTraffic
/// ship source card and the warnings card.
async fn layers_status(State(s): State<Store>, Extension(l): Layers, Extension(mt): MarineTrafficLayer) -> Result<Json<Value>> {
    let (health, warnings) = tokio::join!(l.health(), work(s, |s| crate::advisories::layer_card(&s)));
    let Json(warnings) = warnings?;
    let mut status = l.status();
    if let Some(cards) = status["layers"].as_array_mut() {
        for card in cards.iter_mut() {
            if let Some(h) = health.iter().find(|h| h["id"] == card["id"]) {
                card["health"] = json!({"state":h["state"],"detail":h["detail"],"since":h["since"],"checked_at":h["checked_at"]});
            }
        }
        cards.push(mt.card());
        cards.push(warnings);
    }
    Ok(Json(status))
}
/// Ships in view from MarineTraffic's public map, through the configured
/// proxies only: `?west&south&east&north&zoom`.
async fn layer_marinetraffic_ships(Extension(mt): MarineTrafficLayer, axum::extract::Query(q): Params) -> Result<Json<Value>> {
    Ok(Json(mt.ships(&q).await?))
}
/// One MarineTraffic ship's live record (info window and particulars).
async fn layer_marinetraffic_ship(Extension(mt): MarineTrafficLayer, axum::extract::Path(ship): axum::extract::Path<String>) -> Result<Json<Value>> {
    Ok(Json(mt.vessel(&ship).await?))
}
/// The ship's MarineTraffic photo, fetched through the configured proxy; the
/// browser never contacts the image host.
async fn layer_marinetraffic_photo(Extension(mt): MarineTrafficLayer, axum::extract::Path(ship): axum::extract::Path<String>) -> Result<Response> {
    let photo = mt.photo(&ship).await?;
    let mut response = Response::new(axum::body::Body::from(photo.body.clone()));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, photo.content_type.parse().map_err(|_| Error::internal())?);
    headers.insert(header::CACHE_CONTROL, "private, max-age=3600".parse().unwrap());
    Ok(response)
}
/// A KeelWatch vector tile through Relic: the browser never learns the
/// private service address. Compressed bytes pass through unchanged.
async fn layer_ship_tile(Extension(l): Layers, axum::extract::Path((z, x, y)): axum::extract::Path<(u8, u32, String)>, axum::extract::Query(q): Params) -> Result<Response> {
    let y = y.trim_end_matches(".mvt").trim_end_matches(".pbf").parse::<u32>().map_err(|_| Error::invalid("The tile row must be a number"))?;
    let (status, bytes, encoding, cache) = l.ship_tile(z, x, y, &q).await?;
    let mut response = Response::new(axum::body::Body::from(bytes));
    *response.status_mut() = axum::http::StatusCode::from_u16(status).map_err(|_| Error::internal())?;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, "application/vnd.mapbox-vector-tile".parse().unwrap());
    // KeelWatch's lifetime for past moments (30 min once older than 6 h).
    let cache = cache.filter(|c| q.contains_key("at") && c.starts_with("private")).unwrap_or_else(|| crate::live_layers::TILE_CACHE.into());
    headers.insert(header::CACHE_CONTROL, cache.parse().map_err(|_| Error::internal())?);
    if let Some(value) = encoding.and_then(|e| e.parse().ok()) {
        headers.insert(header::CONTENT_ENCODING, value);
    }
    Ok(response)
}
async fn layer_ship(Extension(l): Layers, axum::extract::Path(id): axum::extract::Path<String>) -> Result<Json<Value>> {
    Ok(Json(l.ship(&id).await?))
}
async fn layer_ship_track(Extension(l): Layers, axum::extract::Path(id): axum::extract::Path<String>, axum::extract::Query(q): Params) -> Result<Json<Value>> {
    let time = |k: &str| q.get(k).and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&chrono::Utc));
    if let (Some(from), Some(to)) = (time("from"), time("to")) {
        return Ok(Json(l.ship_track_between(&id, from, to).await?));
    }
    let hours = q.get("hours").and_then(|h| h.parse::<i64>().ok()).unwrap_or(24);
    Ok(Json(l.ship_track(&id, hours).await?))
}
/// Ship finder for one message revision: detection, retained extraction,
/// KeelWatch candidates and decisions.
async fn message_vessels(State(s): State<Store>, Extension(l): Layers, Query(q): Query<EvidenceQuery>) -> Result<Json<Value>> {
    Ok(Json(crate::vessel_finder::lookup(&s, &l, &q.id, &q.revision).await?))
}
/// Run the paid extraction once, on the analyst's request.
async fn find_message_vessels(State(s): State<Store>, Extension(l): Layers, v: Body) -> Result<Json<Value>> {
    let input = body(v)?;
    let (id, revision) = (input["id"].as_str().unwrap_or_default().to_owned(), input["revision"].as_str().unwrap_or_default().to_owned());
    if id.is_empty() || revision.is_empty() {
        return Err(Error::invalid("Send the message id and revision"));
    }
    crate::vessel_finder::extract(&s, &id, &revision).await?;
    Ok(Json(crate::vessel_finder::lookup(&s, &l, &id, &revision).await?))
}
/// Ships on feed rows: confirmed or suggested vessels per message revision.
async fn message_vessel_marks(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    let input = body(v)?;
    let items = input["items"].as_array().ok_or_else(|| Error::invalid("Send items: [{id, revision}]"))?;
    if items.len() > 500 {
        return Err(Error::invalid("At most 500 messages at once"));
    }
    let items = items.iter().filter_map(|i| Some((i["id"].as_str()?.to_owned(), i["revision"].as_str()?.to_owned()))).collect::<Vec<_>>();
    work(s, move |s| s.vessel_marks(&items)).await
}
/// Ships KeelWatch recorded near the message's places around its time. Free.
async fn message_nearby_vessels(State(s): State<Store>, Extension(l): Layers, Query(q): Query<EvidenceQuery>) -> Result<Json<Value>> {
    Ok(Json(crate::vessel_finder::nearby(&s, &l, &q.id, &q.revision).await?))
}
type Vessels = Extension<Arc<crate::vessel_batch::VesselBatch>>;
async fn vessel_batch_status(Extension(b): Vessels) -> Result<Json<Value>> {
    Ok(Json(b.status().await?))
}
/// `{action:"start"}` finds ships in every waiting flagged message (the whole
/// backlog); `{action:"stop"}` lets requests already sent finish and sends no more.
async fn vessel_batch_action(Extension(b): Vessels, v: Body) -> Result<Json<Value>> {
    match body(v)?["action"].as_str() {
        Some("start") => b.clone().begin(false).await?,
        Some("stop") => b.stop(),
        _ => return Err(Error::invalid("The action is start or stop")),
    }
    Ok(Json(b.status().await?))
}
/// `{auto:true}` takes flagged messages acquired from now on, checked every
/// two minutes; it starts no run over the backlog. `{auto:false}` switches it off.
async fn vessel_batch_auto(State(s): State<Store>, Extension(b): Vessels, v: Body) -> Result<Json<Value>> {
    let on = body(v)?["auto"].as_bool().ok_or_else(|| Error::invalid("Send {auto: true|false}"))?;
    let batch = b.clone();
    let _ = work(s, move |_| batch.set_auto(on).map(|_| json!({}))).await?;
    Ok(Json(b.status().await?))
}
/// When KeelWatch was not collecting between `from` and `to` (RFC 3339).
async fn layer_ship_coverage(Extension(l): Layers, axum::extract::Query(q): Params) -> Result<Json<Value>> {
    let time = |k: &str| q.get(k).and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&chrono::Utc)).ok_or_else(|| Error::invalid("Send from and to as RFC 3339 times"));
    Ok(Json(l.ship_coverage(time("from")?, time("to")?).await?))
}
async fn layer_ship_names(Extension(l): Layers, axum::extract::Path(id): axum::extract::Path<String>) -> Result<Json<Value>> {
    Ok(Json(l.ship_names(&id).await?))
}
/// Messages the ship finder links to this vessel (confirmed or suggested).
async fn layer_ship_messages(State(s): State<Store>, axum::extract::Path(id): axum::extract::Path<String>, axum::extract::Query(q): Params) -> Result<Json<Value>> {
    let mmsi = q.get("mmsi").cloned();
    work(s, move |s| s.vessel_messages(&id, mmsi.as_deref())).await
}
async fn decide_message_vessel(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    let input = body(v)?;
    work(s, move |s| {
        s.decide_vessel(input["lookup"].as_str().unwrap_or_default(), input["mention"].as_i64().unwrap_or(-1), input["vessel_key"].as_str().unwrap_or_default(), input["decision"].as_str())?;
        Ok(json!({"saved":true}))
    }).await
}
async fn layer_aircraft(Extension(l): Layers, axum::extract::Query(q): Params) -> Result<Json<Value>> {
    Ok(Json(l.aircraft(&q).await?))
}
/// `?hours=1..87600` (or the older `?days=`) counts back from now, or from
/// `at`; a past moment also reads each warning's listing as recorded then.
async fn layer_warnings(State(s): State<Store>, axum::extract::Query(q): Params) -> Result<Json<Value>> {
    let hours = match q.get("hours") {
        Some(h) => Some(h.parse::<i64>().ok().filter(|h| (1..=87_600).contains(h)).ok_or_else(|| Error::invalid("hours is a whole number from 1 to 87600"))?),
        None => q.get("days").and_then(|d| d.parse::<i64>().ok()).map(|d| d.clamp(1, 3650) * 24),
    };
    let at = q.get("at").cloned();
    work(s, move |s| s.warnings_within(hours, at.as_deref())).await
}
async fn layer_flight(Extension(l): Layers, axum::extract::Path(flight): axum::extract::Path<u32>) -> Result<Json<Value>> {
    Ok(Json(l.flight(flight).await?))
}
async fn layer_ship_search(Extension(l): Layers, axum::extract::Query(q): Params) -> Result<Json<Value>> {
    Ok(Json(l.ship_search(q.get("q").map_or("", String::as_str), 20).await?))
}
async fn layer_aircraft_search(Extension(l): Layers, axum::extract::Query(q): Params) -> Result<Json<Value>> {
    Ok(Json(l.aircraft_search(q.get("q").map_or("", String::as_str)).await?))
}
/// Flightradar24's most-tracked flights now, with each one's current position.
async fn layer_aircraft_top(Extension(l): Layers) -> Result<Json<Value>> {
    Ok(Json(l.aircraft_top().await?))
}
/// The provider's aircraft photo, fetched through the configured proxy; the
/// browser never contacts the image host.
async fn layer_aircraft_photo(Extension(l): Layers, axum::extract::Path(flight): axum::extract::Path<u32>) -> Result<Response> {
    let (image, credit) = l.aircraft_photo(flight).await?;
    let mut response = Response::new(axum::body::Body::from(image.body.clone()));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, image.content_type.parse().map_err(|_| Error::internal())?);
    headers.insert(header::CACHE_CONTROL, "private, max-age=3600".parse().unwrap());
    // Credit travels with the bytes; non-ASCII names are percent-encoded.
    let credit: String = url::form_urlencoded::byte_serialize(credit.as_bytes()).collect();
    if let Ok(value) = credit.parse() {
        headers.insert("x-photo-credit", value);
    }
    Ok(response)
}
async fn local_boundary(State(s): State<Store>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let valid_host = [
        format!("127.0.0.1:{}", s.config.address.port()),
        format!("localhost:{}", s.config.address.port()),
        format!("[::1]:{}", s.config.address.port()),
    ];
    if !valid_host.iter().any(|v| v == host) {
        return (StatusCode::FORBIDDEN,Json(json!({"error":{"code":"invalid_host","message":"Only the configured loopback host is permitted"}}))).into_response();
    }
    if request
        .headers()
        .get("sec-fetch-site")
        .is_some_and(|v| v == "cross-site")
    {
        return (
            StatusCode::FORBIDDEN,
            Json(
                json!({"error":{"code":"invalid_origin","message":"Cross-site access is denied"}}),
            ),
        )
            .into_response();
    }
    if let Some(origin) = request.headers().get(header::ORIGIN)
        && !origin
            .to_str()
            .is_ok_and(|o| s.config.origins.iter().any(|x| x == o))
    {
        return (StatusCode::FORBIDDEN,Json(json!({"error":{"code":"invalid_origin","message":"Origin is outside the local workspace"}}))).into_response();
    }
    let path = request.uri().path();
    let immutable_asset = path.strip_prefix("/assets/").is_some_and(|file| {
        file.rsplit_once('-').is_some_and(|(_, suffix)| {
            suffix.split_once('.').is_some_and(|(hash, extension)| {
                hash.len() >= 8
                    && hash
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                    && ["js", "css", "woff2"].contains(&extension)
            })
        })
    });
    let immutable_shape = path == "/api/v1/area-shape";
    let is_api = path.starts_with("/api/");
    let mut response = next.run(request).await;
    if is_api
        && (response.status().is_client_error() || response.status().is_server_error())
        && !response
            .headers()
            .get(header::CONTENT_TYPE)
            .is_some_and(|h| h.to_str().is_ok_and(|t| t.starts_with("application/json")))
    {
        let content_range = response.headers().get(header::CONTENT_RANGE).cloned();
        response=(response.status(),Json(json!({"error":{"code":"invalid_request","message":"Request could not be handled; check the API route, method and parameters."}}))).into_response();
        if let Some(range) = content_range {
            response.headers_mut().insert(header::CONTENT_RANGE, range);
        }
    }
    let immutable = response.status().is_success() && (immutable_asset || immutable_shape);
    let h = response.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    h.insert(
        header::CACHE_CONTROL,
        if immutable {
            "private, max-age=31536000, immutable"
        } else {
            "no-store"
        }
        .parse()
        .unwrap(),
    );
    h.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    // Basemap hosts only: OpenFreeMap (style, tiles, glyphs) and Esri World
    // Imagery raster tiles for the satellite basemap. Provider data never.
    h.insert(header::CONTENT_SECURITY_POLICY,"default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: https://tiles.openfreemap.org https://server.arcgisonline.com; connect-src 'self' https://tiles.openfreemap.org https://server.arcgisonline.com; worker-src 'self' blob:; frame-src 'none'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'".parse().unwrap());
    response
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaQuery {
    id: String,
    revision: String,
    index: usize,
}
async fn retained_media(
    State(s): State<Store>,
    Query(q): Query<MediaQuery>,
    request: Request,
) -> Result<Response> {
    use futures_util::StreamExt;
    let permit = s
        .media_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::busy())?;
    let (path, mime) =
        tokio::task::spawn_blocking(move || s.media_file(&q.id, &q.revision, q.index))
            .await
            .map_err(|_| Error::internal())??;
    // ServeFile implements bounded byte ranges (including suffixes and HEAD).
    // Media has separate bounded capacity; paused videos cannot exhaust map/feed workers.
    // Keep the media permit until the response ends or the browser disconnects.
    let mut response = ServeFile::new(path)
        .with_buf_chunk_size(64 * 1024)
        .try_call(request)
        .await?;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        mime.parse().map_err(|_| Error::internal())?,
    );
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, "inline".parse().unwrap());
    let (parts, body) = response.into_parts();
    let mut chunks = axum::body::Body::new(body).into_data_stream();
    let stream = async_stream::stream! {
        let _permit = permit;
        while let Some(chunk) = chunks.next().await { yield chunk; }
    };
    Ok(Response::from_parts(
        parts,
        axum::body::Body::from_stream(stream),
    ))
}

async fn load_media(
    Extension(runtime): Extension<Arc<CollectionRuntime>>,
    v: Body,
) -> Result<Json<Value>> {
    let q: MediaQuery = serde_json::from_value(body(v)?)
        .map_err(|_| Error::invalid("Supply an exact evidence id, revision and media index"))?;
    Ok(Json(runtime.load_media(q.id, q.revision, q.index).await?))
}
async fn download_media(
    Extension(runtime): Extension<Arc<CollectionRuntime>>,
    v: Body,
) -> Result<Response> {
    let q: MediaQuery = serde_json::from_value(body(v)?)
        .map_err(|_| Error::invalid("Supply an exact evidence id, revision and media index"))?;
    let stream = crate::media_stream::download(move |progress, cancel| async move {
        runtime
            .load_media_with_progress(q.id, q.revision, q.index, progress, cancel)
            .await
    });
    Ok((
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        axum::body::Body::from_stream(stream),
    )
        .into_response())
}
async fn health(State(s): State<Store>) -> Json<Value> {
    Json(s.health())
}
async fn processing_status(
    State(s): State<Store>,
    Extension(runtime): Extension<Arc<crate::processing::Runtime>>,
) -> Result<Json<Value>> {
    let running = runtime.is_running();
    control(s, move |s| s.processing_status(running)).await
}
async fn processing_policy(
    Extension(runtime): Extension<Arc<crate::processing::Runtime>>,
    v: Body,
) -> Result<Json<Value>> {
    let policy = serde_json::from_value(body(v)?)?;
    Ok(Json(runtime.set_policy(policy).await?))
}
/// Select or deselect one retained source for reading: `{"reading": bool}`.
async fn processing_source(
    Extension(runtime): Extension<Arc<crate::processing::Runtime>>,
    Path(id): Path<String>,
    v: Body,
) -> Result<Json<Value>> {
    let reading = body(v)?["reading"]
        .as_bool()
        .ok_or_else(|| Error::invalid("Send {\"reading\": true} or {\"reading\": false}"))?;
    Ok(Json(runtime.set_source_reading(id, reading).await?))
}
async fn processing_run(
    Extension(runtime): Extension<Arc<crate::processing::Runtime>>,
) -> Result<Json<Value>> {
    Ok(Json(runtime.run().await?))
}
/// Explicit, one-time retry of messages held by an interrupted request. When a
/// batch is already running, the automatic loop admits them in its next batch.
async fn processing_retry_interrupted(
    State(s): State<Store>,
    Extension(runtime): Extension<Arc<crate::processing::Runtime>>,
) -> Result<Json<Value>> {
    let authorized = control(s.clone(), |s| {
        Ok(json!({"authorized":s.authorize_interrupted_retry()?,"enabled":s.processing_policy()?.enabled}))
    })
    .await?;
    if authorized["enabled"] == true && !runtime.is_running() {
        match runtime.run().await {
            Ok(_) => {}
            Err(e) if e.code == "capacity_exceeded" => {}
            Err(e) => return Err(e),
        }
    }
    let running = runtime.is_running();
    control(s, move |s| s.processing_status(running)).await
}
async fn processing_stop(
    Extension(runtime): Extension<Arc<crate::processing::Runtime>>,
) -> Result<Json<Value>> {
    Ok(Json(runtime.stop().await?))
}
async fn workspace(State(s): State<Store>) -> Result<Json<Value>> {
    work(s, |s| s.workspace()).await
}
async fn subscriptions(
    State(s): State<Store>,
    Extension(runtime): Extension<Arc<CollectionRuntime>>,
    Query(q): Query<SubscriptionQuery>,
) -> Result<Json<Value>> {
    let Json(value) = work(s, move |s| s.subscriptions(q)).await?;
    Ok(Json(runtime.decorate(value).await?))
}
async fn create_subscription(
    State(s): State<Store>,
    Extension(runtime): Extension<Arc<CollectionRuntime>>,
    v: Body,
) -> Result<Json<Value>> {
    let v = body(v)?;
    let Json(value) = work(s, move |s| s.create_subscription(v)).await?;
    runtime.wake.notify_one();
    Ok(Json(runtime.decorate(value).await?))
}
async fn subscription(
    State(s): State<Store>,
    Extension(runtime): Extension<Arc<CollectionRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let Json(value) = work(s, move |s| s.subscription(&id)).await?;
    Ok(Json(runtime.decorate(value).await?))
}
async fn update_subscription(
    State(s): State<Store>,
    Extension(runtime): Extension<Arc<CollectionRuntime>>,
    Path(id): Path<String>,
    v: Body,
) -> Result<Json<Value>> {
    let v = body(v)?;
    let updated_id = id.clone();
    let Json(value) = work(s, move |s| s.update_subscription(&id, v)).await?;
    runtime.source_changed(&updated_id).await;
    Ok(Json(runtime.decorate(value).await?))
}

async fn connections(Extension(runtime): Extension<Arc<CollectionRuntime>>) -> Result<Json<Value>> {
    Ok(Json(runtime.status().await?))
}
async fn connect_provider(
    Extension(runtime): Extension<Arc<CollectionRuntime>>,
    Path(provider): Path<String>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::to_value(
        runtime.connect(&provider).await?,
    )?))
}
async fn disconnect_provider(
    Extension(runtime): Extension<Arc<CollectionRuntime>>,
    Path(provider): Path<String>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::to_value(
        runtime.disconnect(&provider).await?,
    )?))
}
async fn telegram_auth(
    Extension(runtime): Extension<Arc<CollectionRuntime>>,
    input: Body,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::to_value(
        runtime.telegram_auth(body(input)?).await?,
    )?))
}
async fn collection_metrics(Extension(runtime): Extension<Arc<CollectionRuntime>>) -> Json<Value> {
    Json(runtime.metrics())
}
async fn collect_source(
    Extension(runtime): Extension<Arc<CollectionRuntime>>,
    Path(id): Path<String>,
    input: Body,
) -> Result<Json<Value>> {
    let v = body(input)?;
    if v.as_object().is_none_or(|o| o.len() != 2) {
        return Err(Error::invalid(
            "Collection action requires action and expected_version",
        ));
    }
    let action = v["action"]
        .as_str()
        .ok_or_else(|| Error::invalid("Collection action is required"))?;
    let expected = v["expected_version"]
        .as_i64()
        .filter(|v| *v > 0)
        .ok_or_else(|| Error::invalid("Expected source version is required"))?;
    Ok(Json(runtime.request(id, expected, action).await?))
}
async fn demo_import(State(s): State<Store>) -> Result<Json<Value>> {
    run_import(
        s,
        include_bytes!("../../examples/port-investigation.json").to_vec(),
    )
    .await
}
async fn import(
    State(s): State<Store>,
    headers: HeaderMap,
    bytes: std::result::Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<Json<Value>> {
    if !headers.get(header::CONTENT_TYPE).is_some_and(|h| {
        h.to_str()
            .is_ok_and(|v| v.split(';').next() == Some("application/json"))
    }) {
        return Err(Error::invalid(
            "Import requires Content-Type: application/json",
        ));
    }
    run_import(
        s,
        bytes
            .map_err(|_| Error::invalid("Import body exceeds its 2 MiB bound"))?
            .to_vec(),
    )
    .await
}
async fn run_import(s: Store, original: Vec<u8>) -> Result<Json<Value>> {
    if s.shutdown.load(std::sync::atomic::Ordering::Acquire) {
        return Err(Error::busy());
    }
    let permit = s
        .imports
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::busy())?;
    let id = ident("job");
    let cancel = Arc::new(AtomicBool::new(false));
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let bundle = serde_json::from_slice(&original)?;
        s.running
            .lock()
            .map_err(|_| Error::internal())?
            .insert(id.clone(), cancel.clone());
        let result = s.import_job_with_raw(bundle, id.clone(), cancel, Some(original));
        s.running.lock().map_err(|_| Error::internal())?.remove(&id);
        result
    })
    .await
    .map_err(|_| Error::internal())??;
    Ok(Json(result))
}
async fn query(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    let q: ScopeQuery = serde_json::from_value(body(v)?)?;
    work(s, move |s| s.query(q)).await
}
#[derive(Deserialize)]
struct EvidenceQuery {
    id: String,
    revision: String,
    publication_id: Option<String>,
}
async fn evidence(State(s): State<Store>, Query(q): Query<EvidenceQuery>) -> Result<Json<Value>> {
    work(s, move |s| {
        s.detail_with_publication(&q.id, &q.revision, q.publication_id.as_deref())
    })
    .await
}
async fn message_reference(State(s): State<Store>, Path(key): Path<String>) -> Result<Json<Value>> {
    work(s, move |s| s.message_reference(&key)).await
}
async fn original_object(State(s): State<Store>, Path(hash): Path<String>) -> Result<Response> {
    let permit = s
        .reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::busy())?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        s.original_object(&hash)
    })
    .await
    .map_err(|_| Error::internal())??;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"relic-retained-input.json\"",
            ),
        ],
        bytes,
    )
        .into_response())
}
async fn create_investigation(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    let v = body(v)?;
    work(s, move |s| s.create_investigation(v)).await
}
async fn update_investigation(
    State(s): State<Store>,
    Path(id): Path<String>,
    v: Body,
) -> Result<Json<Value>> {
    let v = body(v)?;
    work(s, move |s| s.update_investigation(&id, v, false)).await
}
async fn assessment(
    State(s): State<Store>,
    Path(id): Path<String>,
    v: Body,
) -> Result<Json<Value>> {
    let v = body(v)?;
    work(s, move |s| s.update_investigation(&id, v, true)).await
}
async fn save_brief(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    let v = body(v)?;
    work(s, move |s| s.save_brief(v)).await
}
#[derive(Deserialize)]
struct RevisionQuery {
    revision: Option<String>,
    format: Option<String>,
}
async fn brief(
    State(s): State<Store>,
    Path(id): Path<String>,
    Query(q): Query<RevisionQuery>,
) -> Result<Json<Value>> {
    work(s, move |s| s.brief(&id, q.revision.as_deref())).await
}
async fn export(
    State(s): State<Store>,
    Path(id): Path<String>,
    Query(q): Query<RevisionQuery>,
) -> Result<Response> {
    let format = q.format.unwrap_or_else(|| "markdown".into());
    if !["markdown", "json"].contains(&format.as_str()) {
        return Err(Error::invalid("Export format must be markdown or json"));
    }
    let Json(v) = work(s, move |s| s.export(&id, q.revision.as_deref())).await?;
    let response = if format == "json" {
        (
            [
                (header::CONTENT_TYPE, "application/json"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"relic-brief.json\"",
                ),
            ],
            serde_json::to_string_pretty(&v)?,
        )
            .into_response()
    } else {
        (
            [
                (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"relic-brief.md\"",
                ),
            ],
            markdown_export(&v),
        )
            .into_response()
    };
    Ok(response)
}
#[derive(Deserialize)]
struct LineageQuery {
    root: Option<String>,
    depth: Option<usize>,
    direction: Option<String>,
}
async fn lineage(State(s): State<Store>, Query(q): Query<LineageQuery>) -> Result<Json<Value>> {
    work(s, move |s| {
        s.lineage(
            q.root.as_deref(),
            q.depth.unwrap_or(3),
            q.direction.as_deref().unwrap_or("both"),
        )
    })
    .await
}
#[derive(Deserialize)]
struct MessageLineageQuery {
    root: String,
}
async fn message_lineage(
    State(s): State<Store>,
    Query(q): Query<MessageLineageQuery>,
) -> Result<Json<Value>> {
    work(s, move |s| s.message_lineage(&q.root)).await
}
async fn watch_preview(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    let v = body(v)?;
    work(s, move |s| s.watch_preview(v)).await
}
async fn create_watch(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    let v = body(v)?;
    work(s, move |s| s.create_watch(v)).await
}
async fn update_watch(
    State(s): State<Store>,
    Path(id): Path<String>,
    v: Body,
) -> Result<Json<Value>> {
    let v = body(v)?;
    work(s, move |s| s.update_watch(&id, v)).await
}
async fn update_alert(
    State(s): State<Store>,
    Path(id): Path<String>,
    v: Body,
) -> Result<Json<Value>> {
    let v = body(v)?;
    work(s, move |s| s.update_alert(&id, v)).await
}
async fn alerts(State(s): State<Store>) -> Result<Json<Value>> {
    work(s, |s| s.list("alerts")).await
}
async fn jobs(State(s): State<Store>) -> Result<Json<Value>> {
    work(s, |s| s.list("jobs")).await
}
async fn cancel_job(
    State(s): State<Store>,
    Extension(runtime): Extension<Arc<CollectionRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    runtime.stop_job(&id).await?;
    control(s, move |s| s.cancel_job(&id)).await
}
async fn backup(State(s): State<Store>) -> Result<Json<Value>> {
    work(s, |s| s.backup()).await
}
async fn verify(State(s): State<Store>, Path(id): Path<String>) -> Result<Json<Value>> {
    work(s, move |s| s.check_backup(&id)).await
}
async fn events(
    State(s): State<Store>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_util::Stream<Item = std::result::Result<Event, Infallible>>>> {
    let permit = s
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::busy())?;
    let stopping = s.shutdown.clone();
    let mut receiver = s.events.subscribe();
    let since = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let (resync, replay) = s.replay_events(since)?;
    let stream = async_stream::stream! {
        let _permit = permit;
        let mut sequence = since;
        if resync { yield Ok(resync_event("Replay window expired; reload a bounded snapshot")); }
        for value in replay {
            sequence = value["sequence"].as_u64().unwrap_or(sequence);
            yield Ok(change_event(sequence, &value));
        }
        loop {
            if stopping.load(std::sync::atomic::Ordering::Acquire) { break; }
            match receiver.recv().await {
                Ok(value) => {
                    if value["kind"] == "shutdown" { break; }
                    if value["kind"] == "resync" {
                        yield Ok(resync_event("Connection state changed; reload its current state"));
                        continue;
                    }
                    let next = value["sequence"].as_u64().unwrap_or(sequence);
                    if next > sequence {
                        sequence = next;
                        yield Ok(change_event(sequence, &value));
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    yield Ok(resync_event("Client fell behind; reload a bounded snapshot"));
                }
                Err(_) => break,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

async fn osm_status(State(s): State<Store>) -> Result<Json<Value>> {
    work(s, |s| Ok(json!({"active":s.osm()?}))).await
}
async fn osm_activate(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        editions: Vec<String>,
    }
    let input: Input = serde_json::from_value(body(v)?)?;
    if input.editions.is_empty() {
        return Err(Error::invalid("Select at least one OSM edition"));
    }
    Ok(Json(
        crate::geography_store::start_job(s, "osm", move |s, _, cancel| {
            s.activate_osm(&input.editions, cancel)
        })
        .await?,
    ))
}
async fn geography_status(State(s): State<Store>) -> Result<Json<Value>> {
    work(s, |s| Ok(json!({"active":s.geography()?}))).await
}
async fn geography_activate(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        editions: Vec<String>,
    }
    let input: Input = serde_json::from_value(body(v)?)?;
    if input.editions.is_empty() {
        return Err(Error::invalid("Select at least one reference edition"));
    }
    Ok(Json(
        crate::geography_store::start_job(s, "geography", move |s, _, cancel| {
            s.activate_geography(&input.editions, cancel)
        })
        .await?,
    ))
}
async fn geography_reresolve(
    Extension(runtime): Extension<Arc<crate::processing::Runtime>>,
) -> Result<Json<Value>> {
    Ok(Json(runtime.reresolve().await?))
}
async fn processing_recover(
    Extension(runtime): Extension<Arc<crate::processing::Runtime>>,
) -> Result<Json<Value>> {
    Ok(Json(runtime.recover().await?))
}

async fn boundaries_status(State(s): State<Store>) -> Result<Json<Value>> {
    work(s, |s| Ok(json!({"active":s.boundaries()?}))).await
}
async fn boundaries_activate(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        editions: Vec<String>,
    }
    let input: Input = serde_json::from_value(body(v)?)?;
    if input.editions.is_empty() {
        return Err(Error::invalid("Select at least one boundary edition"));
    }
    Ok(Json(
        crate::geography_store::start_job(s, "boundaries", move |s, _, cancel| {
            s.activate_boundaries(&input.editions, cancel)
        })
        .await?,
    ))
}
async fn area_selection(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    let input = body(v)?;
    work(s, move |s| s.area_selection(&input)).await
}

async fn supplemental_policy(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Policy {
        enabled: bool,
    }
    let p: Policy = serde_json::from_value(body(v)?)?;
    work(s, move |s| s.set_supplemental_enabled(p.enabled)).await
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_burst_waits_but_status_and_shutdown_have_separate_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let s = Store::open(crate::config::Config::local(
            std::env::current_dir().unwrap(),
            temp.path().join("data"),
            "127.0.0.1:8792".parse().unwrap(),
        ))
        .unwrap();
        let held = s.reads.clone().acquire_many_owned(4).await.unwrap();
        let waiting = tokio::spawn(work(s.clone(), |_| Ok(json!({"ok":true}))));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());
        assert!(
            control(s.clone(), |_| Ok(json!({"status":"ok"})))
                .await
                .is_ok()
        );
        drop(held);
        assert_eq!(waiting.await.unwrap().unwrap().0["ok"], true);
        let _held = s.reads.clone().acquire_many_owned(4).await.unwrap();
        let started = std::time::Instant::now();
        assert!(work(s.clone(), |_| panic!("no admission")).await.is_err());
        assert!(started.elapsed() >= Duration::from_millis(240));
        let _admitted = s
            .read_admission
            .clone()
            .acquire_many_owned(20)
            .await
            .unwrap();
        assert!(work(s.clone(), |_| panic!("bounded queue")).await.is_err());
        s.begin_shutdown();
        assert!(control(s, |_| panic!("shutdown")).await.is_err());
    }
}

#[derive(Deserialize)]
struct FailurePage {
    before: Option<i64>,
}
async fn processing_failures(
    State(s): State<Store>,
    Query(q): Query<FailurePage>,
) -> Result<Json<Value>> {
    work(s, move |s| s.processing_failures(q.before)).await
}
/// Collection sources and live layers that need attention:
/// `{count,sources,layers:[{id,label,state,detail,since}]}`, `count` the total.
/// The live layers' health never holds a status worker and never waits long.
async fn source_attention(State(s): State<Store>, Extension(l): Layers, Extension(mt): MarineTrafficLayer) -> Result<Json<Value>> {
    let counted = control(s, |s| {
        let c=s.connect()?;
        let states=c.prepare("SELECT c.json,coalesce(s.poll_interval_seconds,120) FROM collection_states c JOIN subscriptions s ON s.id=c.subscription_id WHERE s.enabled=1 AND s.version=c.configuration_version")?
            .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut count=0;
        for (raw,interval) in states {
            let state:Value=serde_json::from_str(&raw)?;
            if ["needs_attention","blocked","error"].contains(&state["status"].as_str().unwrap_or("")) || crate::collection::catchup_delayed(&state,interval) || crate::collection::collection_stalled(&state,interval) { count+=1; }
        }
        Ok(json!({"sources":count,"warnings":crate::advisories::health(&s)?}))
    });
    let (counted, live) = tokio::join!(counted, l.health());
    let Json(counted) = counted?;
    let sources = counted["sources"].as_u64().unwrap_or(0);
    let layers = live
        .into_iter()
        .chain([mt.health(), counted["warnings"].clone()])
        .filter(|h| h["state"] == "degraded")
        .map(|h| json!({"id":h["id"],"label":h["label"],"state":h["state"],"detail":h["detail"],"since":h["since"]}))
        .collect::<Vec<_>>();
    Ok(Json(json!({"count":sources+layers.len() as u64,"sources":sources,"layers":layers})))
}

fn resync_event(reason: &str) -> Event {
    Event::default()
        .event("resync")
        .data(json!({"reason":reason}).to_string())
}
fn change_event(sequence: u64, value: &Value) -> Event {
    Event::default()
        .id(sequence.to_string())
        .event("change")
        .data(value.to_string())
}

async fn query_page(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    let q: ScopeQuery = serde_json::from_value(body(v)?)?;
    work(s, move |s| s.query_page_preview(q)).await
}
async fn query_preview(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    let q: ScopeQuery = serde_json::from_value(body(v)?)?;
    work(s, move |s| s.query_preview(q)).await
}
#[derive(Deserialize)]
struct ShapeQuery {
    id: String,
}
async fn area_shape(State(s): State<Store>, Query(q): Query<ShapeQuery>) -> Result<Json<Value>> {
    work(s, move |s| s.area_shape(&q.id)).await
}

async fn relationship_policy(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    let enabled = body(v)?["enabled"]
        .as_bool()
        .ok_or_else(|| Error::invalid("enabled must be boolean"))?;
    control(s, move |s| s.set_event_links_enabled(enabled)).await
}
async fn relationship_query(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    let v = body(v)?;
    work(s, move |s| s.query_event_links(v)).await
}
async fn relationship_group(State(s): State<Store>, v: Body) -> Result<Json<Value>> {
    let v = body(v)?;
    work(s, move |s| s.event_group(v)).await
}
