//! Source adapter interfaces.
//!
//! Relic's core owns scheduling, storage, catch-up safety, publication and
//! the HTTP API. Provider-specific clients plug in through the traits below
//! and are registered once at startup through [`Sources`]. A build with no
//! adapter registered still runs: each missing source reports itself as "not
//! included in this build" instead of pretending to be empty.
//!
//! The public build registers none of them. Telegram is built in.
use crate::error::{Error, Result};
use axum::body::Bytes;
use serde_json::Value;
use std::{collections::BTreeMap, fmt, future::Future, pin::Pin, sync::Arc};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Bytes fetched by an adapter for the browser, which never contacts the
/// provider itself.
#[derive(Clone, Debug)]
pub struct Image {
    pub body: Bytes,
    pub content_type: String,
}

/// How a source request failed, as far as the collector needs to act on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// The provider's own rate limit: the account waits out a cooldown.
    RateLimited,
    /// Credentials or access were refused: reconnecting is needed.
    Unauthorized,
    /// A temporary provider fault: retried with backoff.
    Transient,
    /// The answer was missing, malformed or over its byte budget.
    Unavailable,
    /// Transport, configuration or anything else.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFailure {
    pub kind: FailureKind,
    pub message: String,
}
impl SourceFailure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}
impl fmt::Display for SourceFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// One post on a timeline page, already normalized into Relic's message
/// shape (`id`, `text`, `published_at`, `media`, `relationships`, ...).
#[derive(Clone, Debug)]
pub struct TimelinePost {
    /// Exact numeric post identity; ordering follows the provider's IDs.
    pub id: u64,
    pub author_id: Option<u64>,
    /// An ordinary timeline entry. Pins and embedded modules are not, and
    /// never count as watermark overlap.
    pub primary: bool,
    pub message: Value,
}

/// One page of a profile timeline, newest first.
#[derive(Clone, Debug)]
pub struct TimelinePage {
    pub posts: Vec<TimelinePost>,
    pub unreadable: usize,
    pub next_cursor: Option<String>,
    /// The decoded provider payload, retained as the page's raw evidence.
    pub raw_payload: Value,
}

/// A profile timeline reader (the `x` provider). The collector keeps the
/// watermark, cursor-repeat and gap rules; the adapter only reads pages.
pub trait PostTimeline: Send + Sync {
    /// Stable key for the configured account. A change invalidates saved
    /// intervals, which then need review before resuming.
    fn account_key(&self) -> String;
    /// Whether the adapter rotates its egress between collection runs.
    fn rotates(&self) -> bool;
    fn rotate(&self) -> BoxFuture<'_, std::result::Result<(), SourceFailure>>;
    /// Validates access without importing anything.
    fn check(&self) -> BoxFuture<'_, std::result::Result<(), SourceFailure>>;
    /// The stable numeric identity behind a handle, if readable.
    fn resolve<'a>(&'a self, handle: &'a str) -> BoxFuture<'a, std::result::Result<Option<u64>, SourceFailure>>;
    fn page<'a>(&'a self, user: u64, cursor: Option<&'a str>) -> BoxFuture<'a, std::result::Result<TimelinePage, SourceFailure>>;
    /// A media asset referenced by a normalized post, bounded by `max_bytes`.
    fn fetch_asset<'a>(&'a self, url: &'a str, max_bytes: usize) -> BoxFuture<'a, std::result::Result<Image, SourceFailure>>;
}

pub type Query = BTreeMap<String, String>;

/// A live aircraft context layer. Responses are the JSON bodies the map
/// draws; health is a `{id,label,state,detail,since,checked_at}` card.
pub trait AircraftLayer: Send + Sync {
    /// Whether the adapter has the settings it needs.
    fn configured(&self) -> bool;
    fn health(&self) -> Value;
    fn aircraft<'a>(&'a self, q: &'a Query) -> BoxFuture<'a, Result<Value>>;
    fn search<'a>(&'a self, query: &'a str) -> BoxFuture<'a, Result<Value>>;
    fn top(&self) -> BoxFuture<'_, Result<Value>>;
    fn flight(&self, flight_id: u32) -> BoxFuture<'_, Result<Value>>;
    /// The aircraft's photo and its credit line.
    fn photo(&self, flight_id: u32) -> BoxFuture<'_, Result<(Image, String)>>;
}

/// A live ship-snapshot layer (positions from a public map feed, as opposed
/// to the KeelWatch history service built into the core).
pub trait ShipSnapshotLayer: Send + Sync {
    /// The layer status card shown in the layer menu.
    fn card(&self) -> Value;
    fn health(&self) -> Value;
    fn ships<'a>(&'a self, q: &'a Query) -> BoxFuture<'a, Result<Value>>;
    fn vessel<'a>(&'a self, ship: &'a str) -> BoxFuture<'a, Result<Value>>;
    fn photo<'a>(&'a self, ship: &'a str) -> BoxFuture<'a, Result<Image>>;
}

/// Adapters registered at startup.
#[derive(Clone, Default)]
pub struct Sources {
    pub timeline: Option<Arc<dyn PostTimeline>>,
    /// When the timeline adapter is part of this build but not configured,
    /// the settings it needs (shown on the connection card).
    pub timeline_setup: Option<String>,
    pub aircraft: Option<Arc<dyn AircraftLayer>>,
    pub ship_snapshots: Option<Arc<dyn ShipSnapshotLayer>>,
}

/// The ship-snapshot layer when no adapter is registered: it says so on its
/// card and refuses requests with that reason.
pub struct NoShipSnapshots;
/// Why the timeline is unavailable when no adapter is part of the build.
pub const TIMELINE_NOT_INCLUDED: &str = "Not included in this build: the X timeline adapter is not registered.";
const NOT_INCLUDED: &str = "Not included in this build: the live ship-snapshot adapter is not registered";
impl ShipSnapshotLayer for NoShipSnapshots {
    fn card(&self) -> Value {
        serde_json::json!({"id":"marinetraffic","provider":"MarineTraffic","configured":false,"family":"physical observation","temporal":"current public map","coverage":NOT_INCLUDED,
            "health":{"state":"not_configured","detail":NOT_INCLUDED,"since":null,"checked_at":crate::model::now()}})
    }
    fn health(&self) -> Value {
        serde_json::json!({"id":"marinetraffic","label":"MarineTraffic ships","state":"not_configured","detail":NOT_INCLUDED,"since":null,"checked_at":crate::model::now()})
    }
    fn ships<'a>(&'a self, _: &'a Query) -> BoxFuture<'a, Result<Value>> {
        Box::pin(async { Err(Error::not_configured(NOT_INCLUDED)) })
    }
    fn vessel<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Value>> {
        Box::pin(async { Err(Error::not_configured(NOT_INCLUDED)) })
    }
    fn photo<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Image>> {
        Box::pin(async { Err(Error::not_configured(NOT_INCLUDED)) })
    }
}
