//! MSCIO maritime-security warnings: the official site's map list, polled and
//! retained as institutional-advisory evidence with the issuer's exact wording
//! and reported position. `RegistrationDate` is when MSCIO registered the
//! warning, not necessarily when the incident happened; the source gives it
//! without a zone and Relic reads it as UTC, recorded as an inference.
//!
//! MSCIO publishes no expiry or cancellation, so whether a warning is current
//! is judged by its age and by whether MSCIO still lists it. A complete,
//! successful poll that no longer lists a retained warning records "no longer
//! listed" beside the evidence, dated by the first poll that missed it; a later
//! list naming it again records it as listed again. Retained revisions never
//! change. A failed, partial or suspicious list changes no warning's listing.
use crate::{
    error::{Error, Result},
    live_layers::clock_text,
    model::{hash, ident, now, string},
    store::Store,
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, atomic::AtomicBool, atomic::Ordering},
    time::Duration,
};

pub const SOURCE: &str = "mscio:alerts";
const URL: &str = "https://mscio.eu/alerts/filter";
const INTERVAL: Duration = Duration::from_secs(600);
/// From this many listed warnings on, a list that would unlist more than half
/// of them at once is held back until the next poll returns the same list.
const DROP_GUARD_FROM: usize = 10;
/// The layer needs attention once its last successful poll is older than this.
const OVERDUE_MINUTES: i64 = 30;
/// Listing state beside the retained warnings: the current state of each, and
/// every change with the exact response that showed it. Created on the first
/// judged poll; before that every retained warning counts as listed.
const LISTING_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS mscio_listing(evidence_id TEXT PRIMARY KEY, listed INTEGER NOT NULL, unlisted_since TEXT, last_listed_at TEXT);
CREATE TABLE IF NOT EXISTS mscio_listing_changes(id INTEGER PRIMARY KEY, evidence_id TEXT NOT NULL, listed INTEGER NOT NULL, at TEXT NOT NULL, response_sha256 TEXT NOT NULL REFERENCES objects(hash));
CREATE INDEX IF NOT EXISTS mscio_listing_changes_evidence ON mscio_listing_changes(evidence_id, at);";
/// Evidence per import bundle, keeping each bundle well inside the import bound.
const BATCH: usize = 200;
/// MSCIO's map positions are deliberately inexact; see [`content`].
pub const PRECISION: &str = "MSCIO map position, shifted by up to about 5 km on each request";
const TYPES: [&str; 9] = ["Attack", "Attempted boarding", "Boarding", "Hijack", "Kidnap", "Piracy", "Armed robbery", "Suspicious activity", "Other maritime crimes"];

/// One validated warning, with the exact provider record retained beside it.
#[derive(Debug, Clone, PartialEq)]
pub struct Warning {
    pub alert_id: String,
    pub lon: f64,
    pub lat: f64,
    pub registered_at: String,
    pub registered_original: String,
    pub text: String,
    pub kind: Option<String>,
    pub kind_code: Option<String>,
    pub reference: Option<String>,
    pub record: Value,
}
fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}
/// Parse the list. A missing `data` array is a schema change and fails the whole
/// poll; individual malformed rows are rejected and counted, never repaired.
pub fn parse(body: &Value) -> Result<(Vec<Warning>, usize)> {
    let rows = body["data"].as_array().ok_or_else(|| Error::invalid("MSCIO list changed shape: no data array"))?;
    let mut out = vec![];
    let mut rejected = 0;
    for r in rows {
        let parsed = (|| {
            let alert_id = text(&r["AlertId"])?;
            let lat = text(&r["Latitude"])?.parse::<f64>().ok().filter(|v| v.is_finite() && v.abs() <= 90.0)?;
            let lon = text(&r["Longitude"])?.parse::<f64>().ok().filter(|v| v.is_finite() && v.abs() <= 180.0)?;
            let original = text(&r["RegistrationDate"])?;
            let naive = chrono::NaiveDateTime::parse_from_str(&original, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
            let body = r["ThreatDescription"].as_str().filter(|t| !t.trim().is_empty())?.to_owned();
            let code = text(&r["AlertType"]);
            let kind = text(&r["AlertTypeType"]).or_else(|| {
                code.as_ref().and_then(|c| c.parse::<usize>().ok()).and_then(|n| TYPES.get(n.wrapping_sub(1))).map(|s| s.to_string())
            });
            let reference = regex_reference(&body);
            Some(Warning {
                alert_id,
                lon,
                lat,
                registered_at: naive.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                registered_original: original,
                text: body,
                kind,
                kind_code: code,
                reference,
                record: r.clone(),
            })
        })();
        match parsed {
            Some(w) => out.push(w),
            None => rejected += 1,
        }
    }
    Ok((out, rejected))
}
/// Which warnings a list names. Every row's `AlertId` counts, including rows
/// rejected for other reasons (MSCIO still lists them); a row without one
/// makes the list partial.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Listing {
    pub ids: BTreeSet<String>,
    pub unidentified: usize,
}
pub fn listing(body: &Value) -> Result<Listing> {
    let rows = body["data"].as_array().ok_or_else(|| Error::invalid("MSCIO list changed shape: no data array"))?;
    let mut out = Listing::default();
    for r in rows {
        match text(&r["AlertId"]) {
            Some(id) => {
                out.ids.insert(id);
            }
            None => out.unidentified += 1,
        }
    }
    Ok(out)
}
/// "ALERT 12/25"-style issuer reference, when the text carries one.
fn regex_reference(body: &str) -> Option<String> {
    let upper = body.to_uppercase();
    let at = upper.find("ALERT ")?;
    let rest = &body[at + 6..];
    let token: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '/').collect();
    (token.contains('/') && token.len() >= 3).then(|| format!("ALERT {token}"))
}
/// The warning's content without its position. MSCIO moves every position by
/// up to about 5 km on each request (measured 27 September 2026: all 135
/// warnings moved between two requests seconds apart, median 2 km, nothing else
/// changed), so a position change alone is not a new revision.
fn content(record: &Value) -> String {
    let mut v = record.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("Latitude");
        o.remove("Longitude");
    }
    hash(string(&v).as_bytes())
}
fn evidence(w: &Warning, response_hash: &str, acquired_at: &str) -> Value {
    let id = format!("mscio:alert:{}", w.alert_id);
    let revision = format!("sha256:{}", content(&w.record));
    json!({"id":id,"revision":revision,"source":SOURCE,"published_at":w.registered_at,"acquired_at":acquired_at,
        "text":w.text,"geometry":{"type":"Point","coordinates":[w.lon,w.lat]},"location_precision":PRECISION,
        "origin_group":null,"dependence":"unknown","language":"en","fictional":false,"availability":"available",
        "citation":{"evidence_id":id,"revision":revision,"start_character":0,"end_character":w.text.chars().count()},
        "advisory":{"issuer":"MSCIO","alert_id":w.alert_id,"type":w.kind,"type_code":w.kind_code,"reference":w.reference,
            "registration_date_original":w.registered_original,"time_basis":"registration time; the zone-less source value is read as UTC (inferred)"},
        "provider_record":w.record,"provider_response_sha256":response_hash})
}

impl Store {
    /// Import warnings not yet retained at this exact revision. Returns (new, known).
    pub fn retain_warnings(&self, warnings: &[Warning], response: &[u8]) -> Result<(usize, usize)> {
        let fictional: Option<bool> = self
            .connect()?
            .query_row("SELECT json_extract(json,'$.fictional') FROM sources LIMIT 1", [], |r| r.get(0))
            .optional()?;
        if fictional == Some(true) {
            return Err(Error::conflict("This workspace holds fictional evidence; real warnings need a separate workspace"));
        }
        let response_hash = self.write_object(response)?;
        self.sync_object_directories(std::iter::once(response_hash.clone()), &AtomicBool::new(false))?;
        {
            let _lock = self.writer.lock().map_err(|_| Error::internal())?;
            let c = self.connect()?;
            c.execute("INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')", params![response_hash, response.len() as i64])?;
            // Accurate source identity before the first import (imports keep an existing row).
            c.execute("INSERT OR IGNORE INTO sources(id,json) VALUES(?1,?2)", params![SOURCE, string(&json!({"id":SOURCE,"label":"MSCIO maritime warnings","family":"institutional_advisory","provider":"mscio","status":"available","coverage":"The official MSCIO warning map list, polled every 10 minutes. Registration time, not necessarily incident time; returned coverage only.","fictional":false,"detail":"Official warnings while situations develop, not verified damage assessments."}))])?;
        }
        // Compare with each warning's latest retained content, position aside.
        let latest: BTreeMap<String, String> = self
            .connect()?
            .prepare("SELECT id,json_extract(json,'$.provider_record') FROM evidence e WHERE source=?1 AND rowid=(SELECT max(x.rowid) FROM evidence x WHERE x.id=e.id)")?
            .query_map([SOURCE], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?
            .filter_map(|r| r.map(|(id, raw)| Some((id, content(&serde_json::from_str(&raw?).ok()?)))).transpose())
            .collect::<std::result::Result<_, _>>()?;
        let acquired_at = now();
        let fresh = warnings
            .iter()
            .filter(|w| latest.get(&format!("mscio:alert:{}", w.alert_id)) != Some(&content(&w.record)))
            .map(|w| evidence(w, &response_hash, &acquired_at))
            .collect::<Vec<_>>();
        for chunk in fresh.chunks(BATCH) {
            let bundle = json!({"schema_version":1,"fictional":false,"title":"MSCIO maritime warnings","sources":[{"id":SOURCE,"label":"MSCIO maritime warnings","family":"institutional_advisory"}],"evidence_revisions":chunk});
            self.import_job(bundle, ident("job"), Arc::new(AtomicBool::new(false)))?;
        }
        Ok((fresh.len(), warnings.len() - fresh.len()))
    }
    pub fn warnings_status(&self) -> Result<Value> {
        Ok(self
            .connect()?
            .query_row("SELECT value FROM metadata WHERE key='mscio_status'", [], |r| r.get::<_, String>(0))
            .optional()?
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or(json!({"checked_at":null})))
    }
    fn record_warnings_status(&self, status: &Value) -> Result<()> {
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        self.connect()?.execute("INSERT INTO metadata VALUES('mscio_status',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [string(status)])?;
        Ok(())
    }
    /// Record a poll's status. A failure keeps the last success's time and
    /// the time of the first failure in a row.
    fn record_poll(&self, mut status: Value) -> Result<Value> {
        let prior = self.warnings_status()?;
        if status["ok"] != true {
            status["last_success_at"] = prior["last_success_at"].clone();
            status["failing_since"] = if prior["ok"] == false {
                [&prior["failing_since"], &prior["checked_at"]].into_iter().find(|v| v.is_string()).cloned().unwrap_or(Value::Null)
            } else {
                status["checked_at"].clone()
            };
        }
        self.record_warnings_status(&status)?;
        Ok(status)
    }
    /// Judge which retained warnings MSCIO still lists from one successful
    /// poll's exact response. Only a complete, plausible list changes
    /// anything: a row without an `AlertId`, an empty list while warnings are
    /// listed, or a list leaving out more than half of many listed warnings at
    /// once (until the next poll returns the same list) changes nothing.
    /// Returns `{judged,reason,suspicious,listed,unlisted,relisted,not_listed}`.
    pub fn record_listing(&self, response: &[u8], polled_at: &str) -> Result<Value> {
        let body: Value = serde_json::from_slice(response).map_err(|_| Error::invalid("MSCIO returned malformed JSON"))?;
        let list = listing(&body)?;
        let response_hash = self.write_object(response)?;
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        c.execute_batch(LISTING_SCHEMA)?;
        let tx = c.transaction()?;
        tx.execute("INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')", params![response_hash, response.len() as i64])?;
        // Each retained warning, listed unless a judged poll recorded otherwise.
        let retained: Vec<(String, bool)> = tx
            .prepare("SELECT DISTINCT e.id,coalesce(l.listed,1) FROM evidence e LEFT JOIN mscio_listing l ON l.evidence_id=e.id WHERE e.source=?1")?
            .query_map([SOURCE], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? == 1)))?
            .collect::<std::result::Result<_, _>>()?;
        let named = |id: &str| id.strip_prefix("mscio:alert:").is_some_and(|a| list.ids.contains(a));
        let listed_before = retained.iter().filter(|(_, l)| *l).count();
        let missing = retained.iter().filter(|(id, l)| *l && !named(id)).count();
        let fingerprint = hash(string(&json!(list.ids)).as_bytes());
        let prior: Value = tx
            .query_row("SELECT value FROM metadata WHERE key='mscio_listing'", [], |r| r.get::<_, String>(0))
            .optional()?
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or(json!({}));
        let mut pending = Value::Null;
        let refusal = if list.unidentified > 0 {
            Some((format!("{} rows in MSCIO's list had no AlertId; a partial list marks nothing unlisted", list.unidentified), false))
        } else if list.ids.is_empty() && listed_before > 0 {
            Some((format!("MSCIO returned an empty list while {listed_before} warnings were listed; nothing was marked unlisted"), true))
        } else if listed_before >= DROP_GUARD_FROM && missing * 2 > listed_before && prior["pending_drop"]["sha256"] != fingerprint.as_str() {
            pending = json!({"sha256":fingerprint,"first_at":polled_at,"missing":missing});
            Some((format!("MSCIO's list leaves out {missing} of the {listed_before} listed warnings at once; nothing is marked unlisted unless the next poll returns the same list"), true))
        } else {
            None
        };
        tx.execute("INSERT INTO metadata VALUES('mscio_listing',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [string(&json!({"pending_drop":pending,"at":polled_at}))])?;
        if let Some((reason, suspicious)) = refusal {
            tx.commit()?;
            return Ok(json!({"judged":false,"reason":reason,"suspicious":suspicious,"listed":list.ids.len()}));
        }
        let (mut unlisted, mut relisted) = (0, 0);
        for (id, was_listed) in &retained {
            if named(id) {
                tx.execute("INSERT INTO mscio_listing(evidence_id,listed,last_listed_at) VALUES(?1,1,?2) ON CONFLICT(evidence_id) DO UPDATE SET listed=1,unlisted_since=NULL,last_listed_at=excluded.last_listed_at", params![id, polled_at])?;
                if !was_listed {
                    relisted += 1;
                    tx.execute("INSERT INTO mscio_listing_changes(evidence_id,listed,at,response_sha256) VALUES(?1,1,?2,?3)", params![id, polled_at, response_hash])?;
                }
            } else if *was_listed {
                unlisted += 1;
                tx.execute("INSERT INTO mscio_listing(evidence_id,listed,unlisted_since) VALUES(?1,0,?2) ON CONFLICT(evidence_id) DO UPDATE SET listed=0,unlisted_since=excluded.unlisted_since", params![id, polled_at])?;
                tx.execute("INSERT INTO mscio_listing_changes(evidence_id,listed,at,response_sha256) VALUES(?1,0,?2,?3)", params![id, polled_at, response_hash])?;
            }
        }
        tx.commit()?;
        let not_listed = retained.len() - listed_before + unlisted - relisted;
        Ok(json!({"judged":true,"reason":null,"suspicious":false,"listed":list.ids.len(),"unlisted":unlisted,"relisted":relisted,"not_listed":not_listed}))
    }
    /// Retained warnings (latest revision of each) registered within `days`.
    pub fn warnings(&self, days: Option<i64>) -> Result<Value> {
        self.warnings_at(days, None)
    }
    /// [`Store::warnings_within`] with the window in whole days (1 to 3650).
    pub fn warnings_at(&self, days: Option<i64>, at: Option<&str>) -> Result<Value> {
        self.warnings_within(days.map(|d| d.clamp(1, 3650) * 24), at)
    }
    /// Retained warnings registered within `hours` (1 to 87,600; all when
    /// `None`) before now, or before `at` (RFC 3339), newest first, with each
    /// warning's MSCIO listing: `listed`, `unlisted_since` (the first complete
    /// poll without it) and `last_listed_at`. With `at` only warnings
    /// registered by then count, the listing is the one recorded as of that
    /// moment, and `last_listed_at` is not given. `window` states the
    /// registration times covered and `retained` how many warnings Relic holds
    /// up to the same end, so a short window's count is never mistaken for all.
    pub fn warnings_within(&self, hours: Option<i64>, at: Option<&str>) -> Result<Value> {
        let moment = at
            .map(|a| chrono::DateTime::parse_from_rfc3339(a).map(|t| t.with_timezone(&chrono::Utc)).map_err(|_| Error::invalid("A moment needs an RFC 3339 time")))
            .transpose()?;
        let stamp = |t: chrono::DateTime<chrono::Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let at = moment.map(stamp);
        let hours = hours.map(|h| h.clamp(1, 87_600));
        let since = hours.map(|h| stamp(moment.unwrap_or_else(chrono::Utc::now) - chrono::Duration::hours(h)));
        let c = self.connect()?;
        let retained: i64 = c.query_row("SELECT count(DISTINCT e.id) FROM evidence e WHERE e.source=?1 AND e.lon IS NOT NULL AND e.lat IS NOT NULL AND e.rowid=(SELECT max(x.rowid) FROM evidence x WHERE x.id=e.id) AND (?2 IS NULL OR e.published_at<=?2)", params![SOURCE, at], |r| r.get(0))?;
        type State = (bool, Option<String>, Option<String>);
        let mut listings: BTreeMap<String, State> = BTreeMap::new();
        if c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='mscio_listing_changes')", [], |r| r.get::<_, bool>(0))? {
            let rows = match &at {
                None => c
                    .prepare("SELECT evidence_id,listed,unlisted_since,last_listed_at FROM mscio_listing")?
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)? == 1, r.get(2)?, r.get(3)?))))?
                    .collect::<std::result::Result<Vec<_>, _>>()?,
                Some(moment) => c
                    .prepare("SELECT c.evidence_id,c.listed,c.at FROM mscio_listing_changes c WHERE c.id=(SELECT max(x.id) FROM mscio_listing_changes x WHERE x.evidence_id=c.evidence_id AND x.at<=?1)")?
                    .query_map([moment], |r| {
                        let listed = r.get::<_, i64>(1)? == 1;
                        Ok((r.get::<_, String>(0)?, (listed, (!listed).then(|| r.get::<_, String>(2)).transpose()?, None)))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            };
            listings.extend(rows);
        }
        let rows = c
            .prepare("SELECT e.id,e.revision,e.lon,e.lat,e.published_at,e.text,e.json FROM evidence e WHERE e.source=?1 AND e.rowid=(SELECT max(x.rowid) FROM evidence x WHERE x.id=e.id) AND (?2 IS NULL OR e.published_at>=?2) AND (?3 IS NULL OR e.published_at<=?3) ORDER BY e.published_at DESC")?
            .query_map(params![SOURCE, since, at], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<f64>>(2)?, r.get::<_, Option<f64>>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut features = vec![];
        for (id, revision, lon, lat, registered, body, raw) in rows {
            let (Some(lon), Some(lat)) = (lon, lat) else { continue };
            let e: Value = serde_json::from_str(&raw)?;
            let advisory = &e["advisory"];
            let (listed, unlisted_since, last_listed_at) = listings.remove(&id).unwrap_or((true, None, None));
            features.push(json!({"type":"Feature","geometry":{"type":"Point","coordinates":[lon,lat]},"properties":{"id":id,"revision":revision,"registered_at":registered,"text":body,"type":advisory["type"],"reference":advisory["reference"],"alert_id":advisory["alert_id"],
                "listed":listed,"unlisted_since":unlisted_since,"last_listed_at":last_listed_at}}));
        }
        Ok(json!({"layer":"warnings","provider":"MSCIO","features":features,"status":self.warnings_status()?,
            "window":{"hours":hours,"from":since,"to":at,"basis":"MSCIO registration time"},"retained":retained,
            "listing":{"at":at,"basis":"MSCIO gives no expiry or cancellation. A warning is no longer listed from the first complete poll that left it out; before Relic first judged a list, retained warnings count as listed."}}))
    }
}

async fn poll(store: &Store, client: &wreq::Client, url: &str) -> Value {
    let checked_at = now();
    let fetched = async {
        let response = client.get(url).header("Accept", "application/json").send().await.map_err(|_| "MSCIO is unreachable".to_string())?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(format!("MSCIO HTTP {status}"));
        }
        response.bytes().await.map(|b| b.to_vec()).map_err(|_| "MSCIO response was interrupted".to_string())
    }
    .await;
    let unjudged = json!({"judged":false,"reason":"The poll failed; no warning's listing changed","suspicious":false});
    let bytes = match fetched {
        Ok(b) => b,
        Err(error) => return json!({"checked_at":checked_at,"ok":false,"error":error,"listing":unjudged}),
    };
    let parsed = serde_json::from_slice::<Value>(&bytes).map_err(|_| Error::invalid("MSCIO returned malformed JSON")).and_then(|v| parse(&v));
    let (warnings, rejected) = match parsed {
        Ok(p) => p,
        Err(e) => return json!({"checked_at":checked_at,"ok":false,"error":e.message,"listing":unjudged}),
    };
    let s = store.clone();
    let count = warnings.len();
    let at = checked_at.clone();
    // The listing is judged only once the list itself is retained.
    let retained = tokio::task::spawn_blocking(move || {
        let kept = s.retain_warnings(&warnings, &bytes)?;
        let listing = s.record_listing(&bytes, &at).unwrap_or_else(|e| json!({"judged":false,"reason":format!("Recording the listing failed: {}", e.message),"suspicious":false}));
        Ok::<_, Error>((kept, listing))
    })
    .await;
    match retained {
        Ok(Ok(((new, known), listing))) => json!({"checked_at":checked_at,"last_success_at":checked_at,"ok":true,"warnings":count,"new":new,"unchanged":known,"rejected":rejected,"error":null,"listing":listing}),
        Ok(Err(e)) => json!({"checked_at":checked_at,"ok":false,"error":e.message,"rejected":rejected,"listing":unjudged}),
        Err(_) => json!({"checked_at":checked_at,"ok":false,"error":"Retaining warnings failed","listing":unjudged}),
    }
}
/// One poll of `url`: fetch, retain, judge the listing and record the
/// status, which it returns. [`start`] polls MSCIO; tests poll stand-ins.
pub async fn poll_once(store: &Store, client: &wreq::Client, url: &str) -> Value {
    let status = poll(store, client, url).await;
    let s = store.clone();
    let recorded = status.clone();
    match tokio::task::spawn_blocking(move || s.record_poll(recorded)).await {
        Ok(Ok(v)) => v,
        _ => status,
    }
}
/// The warnings layer's health, `{id,label,state,detail,since,checked_at}`:
/// "degraded" when the last poll failed, the last success is over 30 minutes
/// old, or the last list looked incomplete; "unknown" before the first poll.
/// Where Relic does not poll MSCIO the layer is "off": retained warnings still
/// show, and it never needs attention.
pub fn health(store: &Store) -> Result<Value> {
    let out = |state: &str, detail: Option<String>, since: Value, checked: Value| json!({"id":"warnings","label":"MSCIO warnings","state":state,"detail":detail,"since":since,"checked_at":checked});
    if !polling_allowed(store) {
        return Ok(out("off", Some("Relic polls MSCIO only when started with its private configuration on a real workspace; retained warnings still show".into()), Value::Null, Value::Null));
    }
    let st = store.warnings_status()?;
    let checked = st["checked_at"].clone();
    if !checked.is_string() {
        return Ok(out("unknown", Some("MSCIO has not been checked yet".into()), Value::Null, Value::Null));
    }
    let last = st["last_success_at"].as_str();
    if st["ok"] != true {
        let error = st["error"].as_str().unwrap_or("no reason given");
        let shown = last.map_or("no warning list has been received yet".to_string(), |t| format!("warnings shown are from the check at {}", clock_text(t)));
        let since = [&st["failing_since"], &checked].into_iter().find(|v| v.is_string()).cloned().unwrap_or(Value::Null);
        return Ok(out("degraded", Some(format!("The last MSCIO check failed ({error}); {shown}")), since, checked));
    }
    let succeeded = last.and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
    if succeeded.is_none_or(|t| chrono::Utc::now().signed_duration_since(t) > chrono::Duration::minutes(OVERDUE_MINUTES)) {
        let when = last.map_or("Relic started".to_string(), clock_text);
        return Ok(out("degraded", Some(format!("MSCIO has not been checked successfully since {when}; warnings may be out of date")), st["last_success_at"].clone(), checked));
    }
    if st["listing"]["suspicious"] == true {
        return Ok(out("degraded", st["listing"]["reason"].as_str().map(str::to_owned), checked.clone(), checked));
    }
    Ok(out("ok", None, Value::Null, checked))
}
/// The warnings layer's status card for `GET /layers`.
pub fn layer_card(store: &Store) -> Result<Value> {
    let h = health(store)?;
    Ok(json!({"id":"warnings","provider":"MSCIO","configured":polling_allowed(store),"family":"institutional advisory",
        "temporal":"registration time, not necessarily incident time; MSCIO gives no expiry or cancellation, so currency is judged by age and by whether MSCIO still lists a warning",
        "coverage":"The official MSCIO warning map list, polled every 10 minutes while Relic runs with its private configuration. Retained warnings stay as history.",
        "health":{"state":h["state"],"detail":h["detail"],"since":h["since"],"checked_at":h["checked_at"]}}))
}
/// Poll every 10 minutes while the app runs. Nothing runs for a fictional workspace.
/// MSCIO is real collection: it runs only for an app started with its private
/// configuration file (`--env`, [`crate::config::Config::explicit_env_file`])
/// on a workspace that allows real collection, never for tests (even with
/// synthetic source values), credential-free review servers, an app given
/// only process environment variables, or the fictional example.
pub fn polling_allowed(store: &Store) -> bool {
    store.config.explicit_env_file && store.assert_real_collection_allowed().is_ok()
}
pub fn start(store: Store) {
    if !store.config.explicit_env_file {
        return;
    }
    tokio::spawn(async move {
        let Ok(client) = wreq::Client::builder().no_proxy().redirect(wreq::redirect::Policy::none()).timeout(Duration::from_secs(30)).build() else {
            return;
        };
        loop {
            if store.shutdown.load(Ordering::Acquire) {
                break;
            }
            // A fictional fixture can be imported while the app runs.
            let s = store.clone();
            if !tokio::task::spawn_blocking(move || polling_allowed(&s)).await.unwrap_or(false) {
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            }
            poll_once(&store, &client, URL).await;
            let mut waited = Duration::ZERO;
            while waited < INTERVAL && !store.shutdown.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_secs(1)).await;
                waited += Duration::from_secs(1);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rows_are_validated_not_repaired_and_the_time_basis_is_explicit() {
        let body = json!({"data":[
            {"AlertId":"7","Latitude":"26.6","Longitude":"56.4","RegistrationDate":"2025-05-18T19:16:33.392","ThreatDescription":"ALERT 03/25 GPS interference reported.","AlertTypeType":"Other Crimes on the Maritime Enviroment","AlertType":"9"},
            {"AlertId":"8","Latitude":"95","Longitude":"56.4","RegistrationDate":"2025-05-18T19:16:33","ThreatDescription":"Bad latitude"},
            {"AlertId":"9","Latitude":"12.1","Longitude":"43.2","RegistrationDate":"2025-06-01T08:00:00","ThreatDescription":"Vessel attacked.","AlertType":"1"}
        ]});
        let (w, rejected) = parse(&body).unwrap();
        assert_eq!((w.len(), rejected), (2, 1));
        assert_eq!(w[0].registered_at, "2025-05-18T19:16:33.392Z");
        assert_eq!(w[0].registered_original, "2025-05-18T19:16:33.392");
        assert_eq!(w[0].reference.as_deref(), Some("ALERT 03/25"));
        assert_eq!(w[1].kind.as_deref(), Some("Attack"));
        assert!(parse(&json!({"results":[]})).is_err());
        // The revision follows the exact provider record.
        let a = evidence(&w[0], "h", "t");
        let mut changed = w[0].clone();
        changed.record["ThreatDescription"] = json!("Updated wording.");
        assert_ne!(a["revision"], evidence(&changed, "h", "t")["revision"]);
    }
    #[test]
    fn a_listing_counts_every_identified_row_and_notes_unidentified_ones() {
        let body = json!({"data":[{"AlertId":"7"},{"AlertId":8,"Latitude":"95"},{"AlertId":" "},{"ThreatDescription":"No identity"}]});
        let l = listing(&body).unwrap();
        assert_eq!(l.ids, BTreeSet::from(["7".to_string(), "8".to_string()]));
        assert_eq!(l.unidentified, 2);
        assert!(listing(&json!({"results":[]})).is_err());
    }
}
