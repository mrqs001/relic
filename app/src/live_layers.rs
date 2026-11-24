//! Current-only context layers: KeelWatch ships, and aircraft from a
//! registered [`AircraftLayer`] adapter (none in the public build).
//! Read-only viewport views; nothing here publishes evidence. KeelWatch owns
//! collected ship history. Aircraft positions are live context, not a historical
//! record of the whole aircraft population. Map movement never triggers provider
//! collection: ships read KeelWatch's stored observations, aircraft read the live
//! feed for the requested view only, cached briefly.
use crate::{
    error::{Error, Result},
    model::now,
};
use crate::sources::{AircraftLayer, Image};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Distinct recent requests whose exact responses are kept for reuse, up to
/// about this many response bytes per cache (a larger response is not kept).
const CACHE_ENTRIES: usize = 64;
const CACHE_BYTES: usize = 8 * 1024 * 1024;
/// Vector tiles: KeelWatch serves zooms 0 to 14 and the map overzooms beyond.
const TILE_MAX_ZOOM: u8 = 14;
/// KeelWatch rebuilds a tile at most every 30 seconds and may serve one up to
/// 90 seconds old while the next is built; the browser refetches every 30 s.
pub const TILE_CACHE: &str = "private, max-age=20";
/// A vector tile through Relic: status, bytes, encoding and cache lifetime.
pub type Tile = (u16, Vec<u8>, Option<String>, Option<String>);
/// Why aircraft are unavailable when no adapter is registered.
const AIRCRAFT_NOT_INCLUDED: &str = "Not included in this build: the live aircraft adapter is not registered";
/// KeelWatch's health is checked at most this often (the map asks every 30 s).
const HEALTH_TTL: Duration = Duration::from_secs(60);
/// A health check waits at most this long; then the state is unknown.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
/// KeelWatch is not collecting once no collection run started for this long
/// (its collectors start one every two minutes at least).
const COLLECTION_GAP_MINUTES: i64 = 45;
/// The collection record read for health: long enough to date most gaps.
const COLLECTION_LOOKBACK_HOURS: i64 = 48;

/// A time for a sentence, in UTC: "02:12 UTC" today, "2 Oct 02:12 UTC" otherwise.
pub fn clock(at: chrono::DateTime<chrono::Utc>) -> String {
    if at.date_naive() == chrono::Utc::now().date_naive() { at.format("%H:%M UTC").to_string() } else { at.format("%-d %b %H:%M UTC").to_string() }
}
/// [`clock`] for an RFC 3339 time; any other text is returned as given.
pub fn clock_text(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso).map(|t| clock(t.with_timezone(&chrono::Utc))).unwrap_or_else(|_| iso.to_owned())
}
fn rfc3339(v: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    v.as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|t| t.with_timezone(&chrono::Utc))
}
pub fn wall(at: Instant) -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::from_std(at.elapsed()).unwrap_or_default()
}

/// A layer's health: "ok", "degraded" (needs attention), "unknown" (could not
/// be judged now) or "not_configured". `reason` names the kind of problem, so
/// a lasting one keeps the time it began.
#[derive(Clone, Debug, PartialEq)]
pub struct Health {
    pub state: &'static str,
    pub reason: &'static str,
    pub detail: Option<String>,
    pub since: Option<String>,
    pub checked_at: String,
}
impl Health {
    pub fn ok() -> Self {
        Self { state: "ok", reason: "ok", detail: None, since: None, checked_at: now() }
    }
    pub fn unknown(reason: &'static str, detail: String) -> Self {
        Self { state: "unknown", reason, detail: Some(detail), since: None, checked_at: now() }
    }
    /// Degraded since `since`, or since the same problem was first seen.
    pub fn degraded(reason: &'static str, detail: String, since: Option<chrono::DateTime<chrono::Utc>>, previous: Option<&Health>) -> Self {
        let since = since.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).or_else(|| previous.filter(|p| p.state == "degraded" && p.reason == reason).and_then(|p| p.since.clone())).unwrap_or_else(now);
        Self { state: "degraded", reason, detail: Some(detail), since: Some(since), checked_at: now() }
    }
    /// Degraded since a time nobody recorded (before the record read).
    pub fn degraded_from_unknown(reason: &'static str, detail: String) -> Self {
        Self { state: "degraded", reason, detail: Some(detail), since: None, checked_at: now() }
    }
    pub fn json(&self, id: &str, label: &str) -> Value {
        json!({"id":id,"label":label,"state":self.state,"detail":self.detail,"since":self.since,"checked_at":self.checked_at})
    }
}

pub struct LiveLayers {
    keelwatch: Option<KeelWatch>,
    aircraft: Option<Arc<dyn AircraftLayer>>,
}

/// A viewport, snapped outward so small pans reuse a cached response. West may
/// exceed east when the view crosses the antimeridian; south < north always.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}
impl View {
    pub fn parse(q: &BTreeMap<String, String>) -> Result<Self> {
        let get = |k: &str| {
            q.get(k)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .ok_or_else(|| Error::invalid("A finite west, south, east and north are required"))
        };
        let v = Self { west: get("west")?, south: get("south")?, east: get("east")?, north: get("north")? };
        if !(-180.0..=180.0).contains(&v.west)
            || !(-180.0..=180.0).contains(&v.east)
            || !(-90.0..=90.0).contains(&v.south)
            || !(-90.0..=90.0).contains(&v.north)
            || v.south >= v.north
        {
            return Err(Error::invalid("The view must lie within longitude ±180 and latitude ±90, south below north"));
        }
        Ok(v)
    }
    fn width(&self) -> f64 {
        if self.west <= self.east { self.east - self.west } else { self.east + 360.0 - self.west }
    }
    /// Outward snap to a power-of-two step about an eighth of the view. A
    /// view across the antimeridian whose gap is narrower than the step would
    /// stop crossing (or close up) when snapped; it becomes the whole world.
    pub fn snapped(&self) -> Self {
        let span = self.width().max(self.north - self.south).max(1e-6);
        let step = 2f64.powf((span / 8.0).log2().floor()).clamp(1.0 / 64.0, 8.0);
        let down = |v: f64| (v / step).floor() * step;
        let up = |v: f64| (v / step).ceil() * step;
        let mut v = Self {
            west: down(self.west).max(-180.0),
            south: down(self.south).max(-85.0),
            east: up(self.east).min(180.0),
            north: up(self.north).min(85.0),
        };
        if self.west > self.east && (v.west <= v.east || (v.west == 180.0 && v.east == -180.0)) {
            (v.west, v.east) = (-180.0, 180.0);
        }
        v
    }
    pub fn bbox(&self) -> String {
        format!("{},{},{},{}", self.west, self.south, self.east, self.north)
    }
}

pub struct Cached {
    pub etag: Option<String>,
    pub body: Arc<Value>,
    pub fetched_at: String,
    pub at: Instant,
    /// Approximate size: the response's serialized length.
    pub bytes: usize,
}
#[derive(Default)]
pub struct Cache {
    entries: VecDeque<(String, Cached)>,
    bytes: usize,
}
impl Cache {
    pub fn get(&self, key: &str) -> Option<&Cached> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    /// The oldest entries leave first while the count or byte budget is exceeded.
    pub fn put(&mut self, key: String, value: Cached) {
        if let Some(old) = self.entries.iter().position(|(k, _)| *k == key).and_then(|i| self.entries.remove(i)) {
            self.bytes -= old.1.bytes;
        }
        self.bytes += value.bytes;
        self.entries.push_back((key, value));
        while self.entries.len() > CACHE_ENTRIES || self.bytes > CACHE_BYTES {
            let Some((_, old)) = self.entries.pop_front() else { break };
            self.bytes -= old.bytes;
        }
    }
}

struct KeelWatch {
    base: String,
    token: Option<String>,
    freshness: String,
    anonymous: bool,
    http: wreq::Client,
    cache: Mutex<Cache>,
    /// The last health answer and when it was taken; one check at a time.
    health: Mutex<Option<(Instant, Health)>>,
    checking: tokio::sync::Mutex<()>,
}

impl LiveLayers {
    pub fn new(values: &BTreeMap<String, String>) -> Self {
        Self { keelwatch: KeelWatch::new(values), aircraft: None }
    }
    /// Registers the aircraft adapter, if this build has one.
    pub fn with_aircraft(mut self, aircraft: Option<Arc<dyn AircraftLayer>>) -> Self {
        self.aircraft = aircraft;
        self
    }
    /// Layer status cards: provider, configuration and coverage meaning.
    pub fn status(&self) -> Value {
        json!({"layers":[
            {"id":"ships","provider":"KeelWatch","configured":self.keelwatch.is_some(),"family":"physical observation","temporal":"collected AIS within the freshness window, or each vessel's last stored fix before a chosen past moment","coverage":"Collected observations only; vessels not reported to the collectors are absent. Counts are not all ships at sea.","freshness":self.keelwatch.as_ref().map(|k|k.freshness.clone())},
            {"id":"aircraft","provider":"Flightradar24","configured":self.aircraft.as_ref().is_some_and(|a| a.configured()),"family":"physical observation","temporal":"current live feed for the viewed area","coverage":if self.aircraft.is_some() {"Aircraft visible to the provider's receivers; tiles that hit the provider's limit are marked incomplete. Not a fleet total."} else {AIRCRAFT_NOT_INCLUDED}}
        ]})
    }
    /// Each live layer's health, `{id,label,state,detail,since,checked_at}`.
    /// KeelWatch's comes from a check at most a minute old that never waits
    /// long; the aircraft adapter's from its own state, with no request.
    pub async fn health(&self) -> Vec<Value> {
        let unconfigured = |setting: &str| Health { state: "not_configured", reason: "not_configured", detail: Some(format!("Not configured: set {setting} in the private configuration")), since: None, checked_at: now() };
        let ships = match &self.keelwatch {
            Some(k) => k.health().await,
            None => unconfigured("KEELWATCH_URL"),
        };
        let aircraft = match &self.aircraft {
            Some(a) => a.health(),
            None => json!({"id":"aircraft","label":"Flightradar24 aircraft","state":"not_configured","detail":AIRCRAFT_NOT_INCLUDED,"since":null,"checked_at":now()}),
        };
        vec![ships.json("ships", "KeelWatch ships"), aircraft]
    }
    fn keelwatch(&self) -> Result<&KeelWatch> {
        self.keelwatch.as_ref().ok_or_else(|| Error::not_configured("Ships are not configured: set KEELWATCH_URL in the private configuration"))
    }
    fn aircraft_layer(&self) -> Result<&Arc<dyn AircraftLayer>> {
        self.aircraft.as_ref().ok_or_else(|| Error::not_configured(AIRCRAFT_NOT_INCLUDED))
    }
    /// Aircraft in view: `west,south,east,north` and optional `services`.
    pub async fn aircraft(&self, q: &BTreeMap<String, String>) -> Result<Value> {
        self.aircraft_layer()?.aircraft(q).await
    }
    /// Worldwide lookup by callsign, flight number or registration.
    pub async fn aircraft_search(&self, query: &str) -> Result<Value> {
        self.aircraft_layer()?.search(query).await
    }
    /// The provider's most-tracked flights now, each with its current state.
    pub async fn aircraft_top(&self) -> Result<Value> {
        self.aircraft_layer()?.top().await
    }
    /// The aircraft's photo and its credit line.
    pub async fn aircraft_photo(&self, flight_id: u32) -> Result<(Image, String)> {
        self.aircraft_layer()?.photo(flight_id).await
    }
    pub async fn flight(&self, flight_id: u32) -> Result<Value> {
        self.aircraft_layer()?.flight(flight_id).await
    }
    /// One KeelWatch vector tile (layer `vessels`), passed through unchanged:
    /// merged per half pixel up to zoom 5, every ship from zoom 6. With `at`
    /// (a past moment, whole minutes) and `window` (1 to 24 hours, 2 when
    /// absent) each vessel is drawn at its last stored fix in the window up to
    /// that moment. Returns status, bytes, content encoding and KeelWatch's
    /// cache lifetime (half an hour for moments older than six hours); 204 is
    /// an empty tile.
    pub async fn ship_tile(&self, z: u8, x: u32, y: u32, q: &BTreeMap<String, String>) -> Result<Tile> {
        let k = self.keelwatch()?;
        if z > TILE_MAX_ZOOM || x >= 1u32 << z || y >= 1u32 << z {
            return Err(Error::invalid("The tile is outside zoom 0 to 14 or its grid"));
        }
        let mut params = match q.get("at") {
            Some(at) => {
                let moment = chrono::DateTime::parse_from_rfc3339(at)
                    .map(|t| t.with_timezone(&chrono::Utc))
                    .ok()
                    .filter(|t| *t <= chrono::Utc::now())
                    .ok_or_else(|| Error::invalid("A past moment needs an RFC 3339 time that is not in the future"))?;
                let window = q.get("window").map_or("2h", String::as_str);
                if !window.strip_suffix('h').and_then(|h| h.parse::<u8>().ok()).is_some_and(|h| (1..=24).contains(&h)) {
                    return Err(Error::invalid("The window is 1h to 24h"));
                }
                vec![("at", moment.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)), ("window", window.to_owned())]
            }
            None => vec![("freshness", q.get("freshness").filter(|f| ["30m", "2h", "24h", "7d", "all"].contains(&f.as_str())).cloned().unwrap_or_else(|| k.freshness.clone()))],
        };
        if k.anonymous {
            params.push(("include", "anonymous".into()));
        }
        k.raw(&format!("/api/v1/tiles/{z}/{x}/{y}.mvt"), &params).await
    }
    /// When KeelWatch was not collecting in a window (at most 92 days): gaps of
    /// 45 minutes or more in which no collection run started. A past view or a
    /// missing position inside a gap says nothing about where ships were.
    pub async fn ship_coverage(&self, from: chrono::DateTime<chrono::Utc>, to: chrono::DateTime<chrono::Utc>) -> Result<Value> {
        let k = self.keelwatch()?;
        if to <= from || to - from > chrono::Duration::days(92) {
            return Err(Error::invalid("A coverage window runs forwards and spans at most 92 days"));
        }
        let body = k.fetch("/api/v1/history/collection", &[("from", from.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)), ("to", to.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))]).await?;
        Ok(json!({"from":from.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),"to":to.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "gaps":body["gaps"].as_array().map(|g| g.iter().map(|g| json!({"from":g["from"],"to":g["to"],"seconds":g["seconds"]})).collect::<Vec<_>>()).unwrap_or_default(),
            "gap_seconds":body["gap_seconds"],"collecting_seconds":body["collecting_seconds"],"acquired_at":now()}))
    }
    /// Ranked search across KeelWatch's registry: exact names, MMSI and IMO
    /// score 1.0, exact call signs and former names 0.95, then prefixes,
    /// substrings and similar spellings. Each item keeps every name the vessel
    /// has been reported under.
    pub async fn ship_search(&self, query: &str, limit: u32) -> Result<Value> {
        let k = self.keelwatch()?;
        let query = query.trim();
        if query.chars().count() < 2 || query.len() > 80 {
            return Err(Error::invalid("Search needs 2 to 80 characters: a name, call sign, MMSI or IMO"));
        }
        let body = k.fetch("/api/v1/search", &[("q", query.to_owned()), ("limit", limit.clamp(1, 200).to_string())]).await?;
        let items = body["items"].as_array().cloned().unwrap_or_default().into_iter().map(|i| {
            let mut v = vessel_summary(&i["vessel"]);
            v["match"] = i["match"].clone();
            v["matched"] = i["matched"].clone();
            v["score"] = i["score"].clone();
            v["names"] = i["names"].clone();
            v
        }).collect::<Vec<_>>();
        Ok(json!({"query":query,"items":items,"acquired_at":now()}))
    }
    /// Every name KeelWatch has recorded for a vessel, with first and last time.
    pub async fn ship_names(&self, id: &str) -> Result<Value> {
        let k = self.keelwatch()?;
        let (body, fetched_at, stale, error) = k.conditional(&format!("/api/v1/vessels/{}/names", urlencode(id)), &[]).await?;
        Ok(json!({"names":body["names"],"acquired_at":fetched_at,"stale":stale,"error":error}))
    }
    /// Where each vessel was around a moment: the stored fix before and after
    /// it and, with places, the fix that came closest within the tolerance.
    /// Points are `(lat, lon)` and areas `[west, south, east, north]`
    /// rectangles (zero inside), at most ten of each. Up to 200 vessels;
    /// nothing is interpolated.
    pub async fn ship_positions(&self, keys: &[String], at: chrono::DateTime<chrono::Utc>, tolerance_hours: i64, near: &[(f64, f64)], boxes: &[[f64; 4]]) -> Result<Value> {
        let k = self.keelwatch()?;
        if keys.is_empty() || keys.len() > 200 || near.len() > 10 || boxes.len() > 10 || !(1..=168).contains(&tolerance_hours) {
            return Err(Error::invalid("Positions take 1 to 200 vessels, at most ten points and ten areas, and a tolerance of 1 to 168 hours"));
        }
        let mut body = json!({"keys":keys,"at":at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),"tolerance":format!("{tolerance_hours}h")});
        if !near.is_empty() {
            body["near"] = json!(near.iter().map(|(lat, lon)| [*lat, *lon]).collect::<Vec<_>>());
        }
        if !boxes.is_empty() {
            body["near_boxes"] = json!(boxes);
        }
        k.post("/api/v1/positions", &body).await
    }
    /// Vessels with a stored fix inside a box during a window, at most 31 days
    /// and 5,000 vessels; `meta.truncated` says when more matched.
    pub async fn ship_area(&self, bbox: [f64; 4], from: chrono::DateTime<chrono::Utc>, to: chrono::DateTime<chrono::Utc>, limit: u32) -> Result<Value> {
        self.area(vec![("bbox", format!("{},{},{},{}", bbox[0], bbox[1], bbox[2], bbox[3]))], from, to, limit).await
    }
    /// The same within a radius of a point (at most 500 km), nearest first.
    pub async fn ship_area_near(&self, lon: f64, lat: f64, radius_m: f64, from: chrono::DateTime<chrono::Utc>, to: chrono::DateTime<chrono::Utc>, limit: u32) -> Result<Value> {
        self.area(vec![("lat", lat.to_string()), ("lon", lon.to_string()), ("radius_m", radius_m.clamp(1.0, 500_000.0).round().to_string())], from, to, limit).await
    }
    async fn area(&self, mut params: Vec<(&str, String)>, from: chrono::DateTime<chrono::Utc>, to: chrono::DateTime<chrono::Utc>, limit: u32) -> Result<Value> {
        let k = self.keelwatch()?;
        if to <= from || to - from > chrono::Duration::days(31) {
            return Err(Error::invalid("An area window runs forwards and spans at most 31 days"));
        }
        params.extend([
            ("from", from.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            ("to", to.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            ("limit", limit.clamp(1, 5000).to_string()),
        ]);
        k.fetch("/api/v1/history/area", &params).await
    }
    pub async fn ship(&self, id: &str) -> Result<Value> {
        let k = self.keelwatch()?;
        let path = format!("/api/v1/vessels/{}", urlencode(id));
        let (body, fetched_at, stale, error) = k.conditional(&path, &[]).await?;
        Ok(json!({"vessel":*body,"acquired_at":fetched_at,"stale":stale,"error":error}))
    }
    pub async fn ship_track(&self, id: &str, hours: i64) -> Result<Value> {
        let to = chrono::Utc::now();
        self.ship_track_between(id, to - chrono::Duration::hours(hours.clamp(1, 24 * 30)), to).await
    }
    /// A vessel's KeelWatch observations between two times (at most 30 days).
    /// Not cached: windows end at the request time, so no two are alike, and
    /// up to 20,000 observations would push vessel records out of the cache.
    pub async fn ship_track_between(&self, id: &str, from: chrono::DateTime<chrono::Utc>, to: chrono::DateTime<chrono::Utc>) -> Result<Value> {
        let k = self.keelwatch()?;
        if to <= from || to - from > chrono::Duration::days(30) {
            return Err(Error::invalid("A track window runs forwards and spans at most 30 days"));
        }
        let params = vec![
            ("from", from.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            ("to", to.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            ("limit", "20000".to_string()),
        ];
        let body = k.fetch(&format!("/api/v1/vessels/{}/observations", urlencode(id)), &params).await?;
        Ok(json!({"items":body["items"],"meta":body["meta"],"from":params[0].1,"to":params[1].1,"acquired_at":now(),"stale":false,"error":null}))
    }
}
/// KeelWatch's vessel record in the field names the map and cards use.
pub fn vessel_summary(v: &Value) -> Value {
    json!({"id":v["id"],"name":v["name"],"type":v["type"],"category":v["category"],"flag":v["flag"],"mmsi":v["mmsi"],"imo":v["imo"],"callsign":v["callsign"],
        "lon":v["longitude"],"lat":v["latitude"],"observed_at":v["observed_at"],"sog":v["speed_knots"],"cog":v["course"],"heading":v["heading"],
        "nav_status":v["nav_status"],"destination":v["destination"],"length_m":v["length_m"],"width_m":v["width_m"]})
}
pub fn with_cache(mut body: Value, fetched_at: &str, stale: bool, error: Option<String>) -> Value {
    body["acquired_at"] = json!(fetched_at);
    body["stale"] = json!(stale);
    body["error"] = json!(error);
    body
}
fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
/// From KeelWatch's collection record of a window ending now: `None` while
/// it collects, otherwise since when it has not (`None` inside when the gap
/// began before the window). A gap reaching the end of the window is ongoing
/// (KeelWatch counts the window's edges as runs); the newest run of any
/// source over 45 minutes ago says the same.
fn not_collecting(body: &Value, requested_to: chrono::DateTime<chrono::Utc>) -> std::result::Result<Option<Option<chrono::DateTime<chrono::Utc>>>, String> {
    let gaps = body["gaps"].as_array().ok_or_else(|| "its collection record changed shape".to_string())?;
    let to = rfc3339(&body["to"]).unwrap_or(requested_to);
    let started = rfc3339(&body["from"]);
    let near = |a: chrono::DateTime<chrono::Utc>, b: chrono::DateTime<chrono::Utc>| (a - b).num_seconds().abs() <= 60;
    let since = |t: chrono::DateTime<chrono::Utc>| (!started.is_some_and(|s| near(s, t))).then_some(t);
    if let Some(begun) = gaps.iter().filter_map(|g| Some((rfc3339(&g["from"])?, rfc3339(&g["to"])?))).find(|(_, end)| near(*end, to)).map(|(begun, _)| begun) {
        return Ok(Some(since(begun)));
    }
    if let Some(sources) = body["sources"].as_array() {
        match sources.iter().filter_map(|s| rfc3339(&s["last_run"])).max() {
            Some(last) if to - last > chrono::Duration::minutes(COLLECTION_GAP_MINUTES) => return Ok(Some(since(last))),
            None if sources.is_empty() => return Ok(Some(None)),
            _ => {}
        }
    }
    Ok(None)
}

impl KeelWatch {
    fn new(values: &BTreeMap<String, String>) -> Option<Self> {
        let base = values.get("KEELWATCH_URL").map(|u| u.trim().trim_end_matches('/').to_owned()).filter(|u| !u.is_empty())?;
        let timeout = values.get("KEELWATCH_TIMEOUT_SECONDS").and_then(|v| v.parse::<u64>().ok()).unwrap_or(15).clamp(1, 120);
        // KeelWatch is the owner's own service; the X/Flightradar24 proxy rule does not apply.
        let http = wreq::Client::builder().no_proxy().redirect(wreq::redirect::Policy::none()).timeout(Duration::from_secs(timeout)).build().ok()?;
        Some(Self {
            base,
            token: values.get("KEELWATCH_TOKEN").filter(|t| !t.is_empty()).cloned(),
            freshness: values.get("KEELWATCH_FRESHNESS").filter(|f| ["30m", "2h", "24h", "7d", "all"].contains(&f.as_str())).cloned().unwrap_or_else(|| "24h".into()),
            anonymous: values.get("KEELWATCH_INCLUDE_ANONYMOUS").is_some_and(|v| v == "true"),
            http,
            cache: Mutex::new(Cache::default()),
            health: Mutex::new(None),
            checking: tokio::sync::Mutex::new(()),
        })
    }
    /// Health, answered from the last check for a minute. Only one check runs
    /// at a time and none waits more than [`HEALTH_TIMEOUT`]: meanwhile others
    /// get the last answer, or "unknown" before the first.
    async fn health(&self) -> Health {
        let last = || self.health.lock().ok().and_then(|h| h.clone());
        let fresh = || last().filter(|(at, _)| at.elapsed() < HEALTH_TTL).map(|(_, h)| h);
        if let Some(h) = fresh() {
            return h;
        }
        let Ok(_turn) = self.checking.try_lock() else {
            return last().map(|(_, h)| h).unwrap_or_else(|| Health::unknown("checking", "KeelWatch's health is being checked".into()));
        };
        if let Some(h) = fresh() {
            return h;
        }
        let previous = last().map(|(_, h)| h);
        let h = match tokio::time::timeout(HEALTH_TIMEOUT, self.check(previous.as_ref())).await {
            Ok(h) => h,
            // A second slow answer in a row is a problem, not just unknown.
            Err(_) => {
                let detail = format!("KeelWatch did not answer its health check within {} s; its state is unknown", HEALTH_TIMEOUT.as_secs());
                match &previous {
                    Some(p) if p.reason == "timeout" => Health::degraded("timeout", detail, None, Some(p)),
                    _ => Health::unknown("timeout", detail),
                }
            }
        };
        if let Ok(mut cached) = self.health.lock() {
            *cached = Some((Instant::now(), h.clone()));
        }
        h
    }
    /// KeelWatch's own deep health and its collection record: degraded when
    /// it cannot be reached, reports anything but ok, or has started no
    /// collection run for 45 minutes (an ongoing gap: positions since then
    /// are missing, not absent).
    async fn check(&self, previous: Option<&Health>) -> Health {
        let to = chrono::Utc::now();
        let from = to - chrono::Duration::hours(COLLECTION_LOOKBACK_HOURS);
        let window = [("from", from.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)), ("to", to.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))];
        let (health, collection) = tokio::join!(self.probe("/api/v1/health", &[]), self.probe("/api/v1/history/collection", &window));
        let reported = match health {
            Err(e) => return Health::degraded("unreachable", format!("{e}: ships, tracks and the ship finder cannot be read now"), None, previous),
            Ok((401 | 403, _)) => Health::degraded("refused", "KeelWatch refused Relic's token".into(), None, previous),
            Ok((200, body)) => match body["status"].as_str() {
                Some("ok") => Health::ok(),
                Some(status) => {
                    let reasons = body["degraded"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect::<Vec<_>>()).unwrap_or_default();
                    let listed = if reasons.is_empty() { String::new() } else { format!(": {}{}", reasons.iter().take(3).copied().collect::<Vec<_>>().join("; "), if reasons.len() > 3 { format!(" and {} more", reasons.len() - 3) } else { String::new() }) };
                    Health::degraded("reported", format!("KeelWatch reports {status}{listed}"), None, previous)
                }
                None => Health::unknown("shape", "KeelWatch's health answer has no status".into()),
            },
            Ok((code, body)) => {
                let down = body["status"].as_str().map(|s| format!(", status {s}")).unwrap_or_default();
                Health::degraded("http", format!("KeelWatch's health check failed (HTTP {code}{down})"), None, previous)
            }
        };
        let collecting = match collection {
            Ok((200, body)) => not_collecting(&body, to),
            Ok((code, _)) => Err(format!("HTTP {code}")),
            Err(e) => Err(e),
        };
        match collecting {
            Ok(Some(Some(t))) => Health::degraded("not_collecting", format!("KeelWatch has not been collecting since {}: positions after that are missing, not absent", clock(t)), Some(t), previous),
            Ok(Some(None)) => Health::degraded_from_unknown("not_collecting", format!("KeelWatch has not been collecting for more than {COLLECTION_LOOKBACK_HOURS} hours: positions since then are missing, not absent")),
            Err(e) if reported.state == "ok" => Health::unknown("collection", format!("KeelWatch answers, but its collection record is unavailable ({e}); whether it is collecting now is unknown")),
            _ => reported,
        }
    }
    /// A GET for health checks: the status and JSON body (null when not
    /// JSON), or why there was no answer.
    async fn probe(&self, path: &str, params: &[(&str, String)]) -> std::result::Result<(u16, Value), String> {
        let mut url = url::Url::parse(&format!("{}{}", self.base, path)).map_err(|_| "KEELWATCH_URL is not a valid URL".to_string())?;
        url.query_pairs_mut().extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        let mut request = self.http.get(url.as_str()).header("Accept", "application/json");
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = request.send().await.map_err(|_| "KeelWatch is unreachable".to_string())?;
        let status = response.status().as_u16();
        let bytes = response.bytes().await.map_err(|_| "KeelWatch's answer was interrupted".to_string())?;
        Ok((status, serde_json::from_slice(&bytes).unwrap_or(Value::Null)))
    }
    /// A plain GET without the conditional cache, for search and view tiles.
    async fn fetch(&self, path: &str, params: &[(&str, String)]) -> Result<Value> {
        let mut url = url::Url::parse(&format!("{}{}", self.base, path)).map_err(|_| Error::not_configured("KEELWATCH_URL is not a valid URL"))?;
        url.query_pairs_mut().extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        let mut request = self.http.get(url.as_str()).header("Accept", "application/json");
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = request.send().await.map_err(|_| Error::unavailable("KeelWatch is unreachable"))?;
        let status = response.status().as_u16();
        if status == 404 {
            return Err(Error::missing("KeelWatch has no such record (HTTP 404)"));
        }
        if status != 200 {
            return Err(Error::unavailable(format!("KeelWatch HTTP {status}")));
        }
        let bytes = response.bytes().await.map_err(|_| Error::unavailable("KeelWatch response was interrupted"))?;
        serde_json::from_slice(&bytes).map_err(|_| Error::unavailable("KeelWatch returned malformed JSON"))
    }
    /// A JSON POST (batch positions and lookups).
    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let mut request = self.http.post(format!("{}{}", self.base, path)).header("Accept", "application/json").header("Content-Type", "application/json").body(body.to_string());
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = request.send().await.map_err(|_| Error::unavailable("KeelWatch is unreachable"))?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(Error::unavailable(format!("KeelWatch HTTP {status}")));
        }
        let bytes = response.bytes().await.map_err(|_| Error::unavailable("KeelWatch response was interrupted"))?;
        serde_json::from_slice(&bytes).map_err(|_| Error::unavailable("KeelWatch returned malformed JSON"))
    }
    /// A GET whose compressed body is passed through as is (vector tiles).
    /// Every tile inside the grid exists (204 when empty), so a 404 is a
    /// service failure, never an empty sea.
    async fn raw(&self, path: &str, params: &[(&str, String)]) -> Result<Tile> {
        let mut url = url::Url::parse(&format!("{}{}", self.base, path)).map_err(|_| Error::not_configured("KEELWATCH_URL is not a valid URL"))?;
        url.query_pairs_mut().extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        let mut request = self.http.get(url.as_str()).header("Accept-Encoding", "gzip");
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = request.send().await.map_err(|_| Error::unavailable("KeelWatch is unreachable"))?;
        let status = response.status().as_u16();
        if status != 200 && status != 204 {
            return Err(Error::unavailable(format!("KeelWatch HTTP {status}")));
        }
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let (encoding, cache) = (header("content-encoding"), header("cache-control"));
        let bytes = response.bytes().await.map_err(|_| Error::unavailable("KeelWatch response was interrupted"))?;
        Ok((status, bytes.to_vec(), encoding, cache))
    }
    /// GET with the exact-request conditional cache: a 304 reuses that exact body;
    /// a failure serves the last body for the same request, labelled stale. A
    /// 404 means KeelWatch has no such record now, so nothing stale is served.
    async fn conditional(&self, path: &str, params: &[(&str, String)]) -> Result<(Arc<Value>, String, bool, Option<String>)> {
        let mut url = url::Url::parse(&format!("{}{}", self.base, path)).map_err(|_| Error::not_configured("KEELWATCH_URL is not a valid URL"))?;
        url.query_pairs_mut().extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        let key = url.to_string();
        let prior = self.cache.lock().map_err(|_| Error::internal())?.get(&key).map(|c| (c.etag.clone(), c.body.clone(), c.fetched_at.clone()));
        let mut request = self.http.get(url.as_str()).header("Accept", "application/json");
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(etag) = prior.as_ref().and_then(|p| p.0.clone()) {
            request = request.header("If-None-Match", etag);
        }
        let failure = |message: String| -> Result<(Arc<Value>, String, bool, Option<String>)> {
            match &prior {
                Some((_, body, at)) => Ok((body.clone(), at.clone(), true, Some(message))),
                None => Err(Error::unavailable(message)),
            }
        };
        let response = match request.send().await {
            Ok(r) => r,
            Err(_) => return failure("KeelWatch is unreachable".into()),
        };
        let status = response.status().as_u16();
        if status == 404 {
            return Err(Error::missing("KeelWatch has no such record (HTTP 404)"));
        }
        if status == 304 {
            return match &prior {
                Some((_, body, _)) => Ok((body.clone(), now(), false, None)),
                None => failure("KeelWatch answered 304 without a cached response".into()),
            };
        }
        if status != 200 {
            return failure(format!("KeelWatch HTTP {status}"));
        }
        let etag = response.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_owned);
        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(_) => return failure("KeelWatch response was interrupted".into()),
        };
        let body: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => return failure("KeelWatch returned malformed JSON".into()),
        };
        let fetched_at = now();
        let body = Arc::new(body);
        self.cache.lock().map_err(|_| Error::internal())?.put(key, Cached { etag, body: body.clone(), fetched_at: fetched_at.clone(), at: Instant::now(), bytes: bytes.len() });
        Ok((body, fetched_at, false, None))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn views_snap_outward_and_reject_inverted_latitude() {
        let q = |w: f64, s: f64, e: f64, n: f64| {
            BTreeMap::from([("west".into(), w.to_string()), ("south".into(), s.to_string()), ("east".into(), e.to_string()), ("north".into(), n.to_string())])
        };
        let v = View::parse(&q(55.3, 25.1, 57.2, 26.9)).unwrap().snapped();
        assert!(v.west <= 55.3 && v.south <= 25.1 && v.east >= 57.2 && v.north >= 26.9);
        // Two slightly different pans share one snapped request.
        assert_eq!(v, View::parse(&q(55.31, 25.12, 57.19, 26.88)).unwrap().snapped());
        assert!(View::parse(&q(10.0, 30.0, 20.0, 20.0)).is_err());
        // An antimeridian view keeps west > east rather than being swapped.
        let wrapped = View::parse(&q(170.0, -10.0, -170.0, 10.0)).unwrap();
        assert!((wrapped.width() - 20.0).abs() < 1e-9);
        assert!(wrapped.snapped().west > wrapped.snapped().east);
        // A nearly whole-world view across the antimeridian would snap to a
        // thin strip (or to equal edges, which the provider refuses): the world.
        for (w, e) in [(-145.0, -150.0), (-146.0, -156.0), (179.5, 179.0)] {
            let v = View::parse(&q(w, -60.0, e, 60.0)).unwrap().snapped();
            assert_eq!((v.west, v.east), (-180.0, 180.0), "{w} {e}");
            assert!(v.south < v.north);
        }
    }
    #[test]
    fn caches_are_bounded_by_bytes_as_well_as_entries() {
        let entry = |bytes: usize| Cached { etag: None, body: Arc::new(Value::Null), fetched_at: String::new(), at: Instant::now(), bytes };
        let mut cache = Cache::default();
        for i in 0..5 {
            cache.put(format!("k{i}"), entry(CACHE_BYTES / 4));
        }
        assert!(cache.bytes <= CACHE_BYTES && cache.get("k0").is_none() && cache.get("k4").is_some());
        // Replacing an entry counts its size once; an oversized one is not kept.
        cache.put("k4".into(), entry(10));
        assert_eq!(cache.bytes, 3 * (CACHE_BYTES / 4) + 10);
        cache.put("huge".into(), entry(CACHE_BYTES + 1));
        assert!(cache.entries.is_empty() && cache.bytes == 0);
    }
    /// A stand-in KeelWatch: one vessel, its observations, and nothing else.
    async fn keelwatch(hits: Arc<std::sync::atomic::AtomicUsize>) -> LiveLayers {
        use axum::{Json, Router, routing::get};
        let app = Router::new()
            .route("/api/v1/vessels/a", get(|| async { Json(json!({"id":"a","name":"STAR"})) }))
            .route("/api/v1/vessels/a/observations", get(move || {
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { Json(json!({"items":[{"observed_at":"2026-10-01T12:00:00Z","longitude":2.0,"latitude":1.0}],"meta":{}})) }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        LiveLayers::new(&BTreeMap::from([("KEELWATCH_URL".to_string(), format!("http://{address}"))]))
    }
    #[tokio::test]
    async fn keelwatch_answers_keep_their_meaning_and_tracks_are_not_cached() {
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let layers = keelwatch(hits.clone()).await;
        assert_eq!(layers.ship("a").await.unwrap()["vessel"]["name"], "STAR");
        // KeelWatch's 404 stays a 404, not a bad request.
        let gone = layers.ship("gone").await.err().unwrap();
        assert_eq!((gone.status.as_u16(), gone.code), (404, "not_found"));
        for _ in 0..2 {
            assert_eq!(layers.ship_track("a", 6).await.unwrap()["items"][0]["longitude"], 2.0);
        }
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
        let k = layers.keelwatch().unwrap();
        assert_eq!(k.cache.lock().unwrap().entries.len(), 1, "only the vessel record is cached");
        // An unreachable service is an upstream failure (502).
        let closed = LiveLayers::new(&BTreeMap::from([("KEELWATCH_URL".to_string(), "http://127.0.0.1:9".to_string())]));
        let e = closed.ship("a").await.err().unwrap();
        assert_eq!((e.status.as_u16(), e.code), (502, "provider_unavailable"));
    }
    #[tokio::test]
    async fn tiles_outside_the_grid_are_refused_before_any_request() {
        let values = BTreeMap::from([("KEELWATCH_URL".to_string(), "http://127.0.0.1:9".to_string())]);
        let layers = LiveLayers::new(&values);
        for (z, x, y) in [(15, 0, 0), (3, 8, 0), (3, 0, 8)] {
            assert!(layers.ship_tile(z, x, y, &BTreeMap::new()).await.err().unwrap().message.contains("zoom 0 to 14"));
        }
    }
    #[tokio::test]
    async fn past_tiles_need_a_past_moment_and_a_bounded_window() {
        let values = BTreeMap::from([("KEELWATCH_URL".to_string(), "http://127.0.0.1:9".to_string())]);
        let layers = LiveLayers::new(&values);
        let q = |at: &str, window: Option<&str>| {
            let mut q = BTreeMap::from([("at".to_string(), at.to_string())]);
            if let Some(w) = window {
                q.insert("window".into(), w.into());
            }
            q
        };
        let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        assert!(layers.ship_tile(3, 1, 1, &q(&future, None)).await.err().unwrap().message.contains("not in the future"));
        assert!(layers.ship_tile(3, 1, 1, &q("yesterday", None)).await.err().unwrap().message.contains("RFC 3339"));
        for bad in ["0h", "25h", "2d", "h"] {
            assert!(layers.ship_tile(3, 1, 1, &q("2026-10-01T16:00:00Z", Some(bad))).await.err().unwrap().message.contains("1h to 24h"), "{bad}");
        }
        // A valid request reaches the (closed) service and fails there, not in validation.
        assert_eq!(layers.ship_tile(3, 1, 1, &q("2026-10-01T16:00:00Z", Some("2h"))).await.err().unwrap().code, "provider_unavailable");
        let from = chrono::Utc::now() - chrono::Duration::days(100);
        assert!(layers.ship_coverage(from, chrono::Utc::now()).await.err().unwrap().message.contains("92 days"));
    }
    /// A stand-in KeelWatch health and collection record, counting requests;
    /// `delay` holds each health answer back.
    async fn keelwatch_health(health: Value, collection: Value, delay: Duration, hits: Arc<std::sync::atomic::AtomicUsize>) -> LiveLayers {
        use axum::{Json, Router, routing::get};
        let counted = hits.clone();
        let app = Router::new()
            .route("/api/v1/health", get(move || {
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let h = health.clone();
                async move {
                    tokio::time::sleep(delay).await;
                    Json(h)
                }
            }))
            .route("/api/v1/history/collection", get(move || {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let c = collection.clone();
                async move { Json(c) }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        LiveLayers::new(&BTreeMap::from([("KEELWATCH_URL".to_string(), format!("http://{address}"))]))
    }
    fn ago(minutes: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }
    #[tokio::test]
    async fn keelwatch_health_is_cached_for_a_minute_and_never_waits_long() {
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let collecting = json!({"from":ago(2880),"to":ago(0),"gaps":[],"sources":[{"source":"a","last_run":ago(1)}]});
        let layers = keelwatch_health(json!({"status":"ok"}), collecting.clone(), Duration::ZERO, hits.clone()).await;
        let k = layers.keelwatch().unwrap();
        for _ in 0..3 {
            assert_eq!(k.health().await.state, "ok");
        }
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2, "one health and one collection request for three asks");
        // Once the answer is a minute old the next ask checks again.
        if let Some((at, _)) = k.health.lock().unwrap().as_mut() {
            *at = Instant::now().checked_sub(HEALTH_TTL).unwrap();
        }
        k.health().await;
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 4);

        // A KeelWatch that hangs is unknown within the timeout, and is not asked again for a minute.
        let slow_hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let slow = keelwatch_health(json!({"status":"ok"}), collecting, Duration::from_secs(30), slow_hits.clone()).await;
        let started = Instant::now();
        let h = slow.keelwatch().unwrap().health().await;
        assert!(started.elapsed() < HEALTH_TIMEOUT + Duration::from_millis(800), "{:?}", started.elapsed());
        assert_eq!((h.state, h.reason), ("unknown", "timeout"));
        let again = Instant::now();
        assert_eq!(slow.keelwatch().unwrap().health().await, h);
        assert!(again.elapsed() < Duration::from_millis(100));
        assert_eq!(slow_hits.load(std::sync::atomic::Ordering::SeqCst), 2);
        // A second timeout in a row needs attention.
        let s = slow.keelwatch().unwrap();
        if let Some((at, _)) = s.health.lock().unwrap().as_mut() {
            *at = Instant::now().checked_sub(HEALTH_TTL).unwrap();
        }
        let second = s.health().await;
        assert_eq!((second.state, second.reason), ("degraded", "timeout"));
        // While a check runs, another ask gets the last answer at once.
        let _turn = s.checking.try_lock().unwrap();
        if let Some((at, _)) = s.health.lock().unwrap().as_mut() {
            *at = Instant::now().checked_sub(HEALTH_TTL).unwrap();
        }
        let meanwhile = Instant::now();
        assert_eq!(s.health().await.reason, "timeout");
        assert!(meanwhile.elapsed() < Duration::from_millis(100));
    }
    #[tokio::test]
    async fn keelwatch_health_names_what_is_wrong_and_when_it_began() {
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Not collecting for longer than the record read: no start time is invented.
        let layers = keelwatch_health(json!({"status":"degraded","degraded":["successful coverage stale: a north"]}), json!({"from":ago(2880),"to":ago(0),"gaps":[{"from":ago(2880),"to":ago(0),"seconds":172800}],"sources":[]}), Duration::ZERO, hits.clone()).await;
        let h = layers.keelwatch().unwrap().health().await;
        assert_eq!((h.state, h.reason, h.since.clone()), ("degraded", "not_collecting", None));
        assert!(h.detail.as_deref().unwrap().contains("more than 48 hours"), "{h:?}");
        // No gap reported, but no run for an hour either.
        let body = json!({"from":ago(2880),"to":ago(0),"gaps":[],"sources":[{"source":"a","last_run":ago(60)},{"source":"b","last_run":ago(90)}]});
        let since = not_collecting(&body, chrono::Utc::now()).unwrap().unwrap().unwrap();
        assert_eq!(since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true), ago(60));
        assert!(not_collecting(&json!({"to":ago(0)}), chrono::Utc::now()).is_err(), "a changed record is not read as collecting");
        assert_eq!(not_collecting(&json!({"from":ago(2880),"to":ago(0),"gaps":[{"from":ago(300),"to":ago(200)}]}), chrono::Utc::now()).unwrap(), None);
        // Unreachable is degraded, and a lasting problem keeps when it began.
        let closed = LiveLayers::new(&BTreeMap::from([("KEELWATCH_URL".to_string(), "http://127.0.0.1:9".to_string())]));
        let k = closed.keelwatch().unwrap();
        let first = k.health().await;
        assert_eq!((first.state, first.reason), ("degraded", "unreachable"));
        let earlier = "2026-10-09T02:12:00Z".to_string();
        if let Some((at, h)) = k.health.lock().unwrap().as_mut() {
            *at = Instant::now().checked_sub(HEALTH_TTL).unwrap();
            h.since = Some(earlier.clone());
        }
        assert_eq!(k.health().await.since, Some(earlier));
        assert_eq!(clock_text("not a time"), "not a time");
    }
    #[tokio::test]
    async fn unconfigured_layers_report_their_setting_as_health() {
        let health = LiveLayers::new(&BTreeMap::new()).health().await;
        assert_eq!(health.iter().map(|h| (h["id"].as_str().unwrap(), h["state"].as_str().unwrap())).collect::<Vec<_>>(), [("ships", "not_configured"), ("aircraft", "not_configured")]);
        assert!(health[0]["detail"].as_str().unwrap().contains("KEELWATCH_URL"));
    }
    #[test]
    fn unconfigured_layers_explain_their_state() {
        let layers = LiveLayers::new(&BTreeMap::new());
        let status = layers.status();
        assert_eq!(status["layers"][0]["configured"], false);
        assert!(layers.keelwatch().err().unwrap().message.contains("KEELWATCH_URL"));
        let e = layers.aircraft_layer().err().unwrap();
        assert!(e.message.contains("Not included in this build") && e.status.as_u16() == 503 && e.code == "not_configured");
    }
}
