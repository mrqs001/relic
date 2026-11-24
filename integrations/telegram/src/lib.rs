//! Bounded native Telegram acquisition. No operation discovers configuration or credentials.
//! `open` reads a private session without connecting; only explicit network methods connect.
mod client;
mod normalize;
mod session;

pub use client::TelegramClient;
pub use normalize::{normalize_message, normalize_raw};

use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf, time::Duration};
use zeroize::Zeroizing;

/// Private application configuration. Deliberately implements neither Debug nor Serialize.
pub struct Config {
    pub api_id: i32,
    pub api_hash: Zeroizing<String>,
    pub session_path: PathBuf,
    pub request_timeout: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    Disconnected,
    CodeRequired,
    PasswordRequired,
    Connected,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    pub state: AuthState,
    /// A local session exists; this does not prove its current upstream validity.
    pub session_authorized: bool,
    pub account_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelIdentity {
    /// Signed Bot API-style numeric identity; no access hash.
    pub peer_id: String,
    pub peer_type: String,
    pub title: String,
    pub username: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub longitude: f64,
    pub latitude: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageRef {
    pub peer_id: String,
    pub message_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Forward {
    pub peer_id: Option<String>,
    pub message_id: Option<String>,
    pub date: String,
    pub author: Option<String>,
    pub saved_from: Option<MessageRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaRef {
    pub kind: String,
    pub id: Option<String>,
    pub mime_type: Option<String>,
    pub size: Option<u64>,
    /// Presence is not successful archival. Retrieve by exact originating message.
    pub archive_status: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Counters {
    pub views: Option<i32>,
    pub forwards: Option<i32>,
    pub replies: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub account_id: String,
    pub peer_id: String,
    pub message_id: String,
    pub text: String,
    pub published_at: String,
    pub edited_at: Option<String>,
    pub acquired_at: String,
    pub sender_id: Option<String>,
    pub reply: Option<MessageRef>,
    pub forward: Option<Forward>,
    pub media: Option<MediaRef>,
    pub grouped_id: Option<String>,
    pub point: Option<Point>,
    pub counters: Counters,
    pub content_fingerprint: String,
    /// Retained TL representation with private access hashes/file references removed.
    /// Volatile counters remain available here, but are excluded from the fingerprint.
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryPage {
    pub messages: Vec<Message>,
    /// Continue towards the durable `after_id` using this exclusive offset.
    pub resume_before: Option<i32>,
    /// True only after reaching the watermark or successfully exhausting the history.
    pub interval_drained: bool,
    /// Candidate only. Caller accumulates across pages and commits after durable acquisition.
    pub candidate_high_water: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageEvent {
    New {
        message: Message,
    },
    Edited {
        message: Message,
    },
    Deleted {
        account_id: String,
        peer_id: String,
        message_ids: Vec<String>,
        acquired_at: String,
    },
    /// Never apply unscoped deletions to any channel.
    UnscopedDeletion {
        message_ids: Vec<String>,
    },
    Ignored,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateDelivery {
    pub sequence: u64,
    pub event: MessageEvent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExactMessages {
    pub messages: Vec<Message>,
    /// Exact IDs absent from a successful authorized reply; not produced on access errors.
    pub unavailable_ids: Vec<String>,
}

pub struct MediaBytes {
    pub bytes: Vec<u8>,
    pub mime_type: Option<String>,
    pub sha256: String,
    pub origin: Message,
}

pub const MAX_STREAM_MEDIA_BYTES: usize = 512 * 1024 * 1024;
/// Download receipt; bytes have been written to the caller's bounded sink.
pub struct MediaDownload {
    pub bytes: usize,
    pub sha256: String,
    pub prefix: Vec<u8>,
    pub mime_type: Option<String>,
    pub origin: Message,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Configuration,
    SessionUnavailable,
    SessionBusy,
    SessionCorrupt,
    NotConnected,
    AuthenticationRequired,
    InvalidPhone,
    InvalidCode,
    CodeExpired,
    InvalidPassword,
    SignUpRequired,
    InvalidTarget,
    UnsupportedPeer,
    PeerUnavailable,
    InvalidBounds,
    PendingAcknowledgement,
    InvalidAcknowledgement,
    Timeout,
    FloodWait(u32),
    AccessDenied,
    Network,
    Upstream,
    UpdateOverflow,
    MediaUnavailable,
    MediaTooLarge,
    MediaStorage,
    UnsupportedMedia,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Configuration => "Telegram application configuration is incomplete or invalid",
            Self::SessionUnavailable => "Telegram private session could not be saved or opened",
            Self::SessionBusy => "Telegram session is already owned by another process",
            Self::SessionCorrupt => "Telegram session is unreadable; no replacement was created",
            Self::NotConnected => "Telegram is disconnected",
            Self::AuthenticationRequired => "Telegram account login is required",
            Self::InvalidPhone => "Enter a valid international phone number",
            Self::InvalidCode => "Telegram login code is invalid",
            Self::CodeExpired => "Telegram login code expired; request another code",
            Self::InvalidPassword => "Telegram two-factor password is invalid",
            Self::SignUpRequired => "Create the Telegram account in an official client first",
            Self::InvalidTarget => "Use a public Telegram channel handle",
            Self::UnsupportedPeer => "Only channels and groups can be collected",
            Self::PeerUnavailable => "Telegram peer is unavailable in this session",
            Self::InvalidBounds => "Telegram request exceeds the configured bounds",
            Self::PendingAcknowledgement => "Persist and acknowledge the previous update first",
            Self::InvalidAcknowledgement => "Telegram update acknowledgement is out of order",
            Self::Timeout => "Telegram request timed out",
            Self::FloodWait(seconds) => {
                return write!(f, "Telegram account must wait {seconds} seconds");
            }
            Self::AccessDenied => "Telegram denied access; message absence is unknown",
            Self::Network => "Telegram connection failed",
            Self::Upstream => "Telegram rejected the request",
            Self::UpdateOverflow => {
                "Telegram update buffer filled; reconnect and catch up from the last durable checkpoint"
            }
            Self::MediaUnavailable => "The exact Telegram message has no available media",
            Self::MediaTooLarge => "Telegram media exceeds the download byte limit",
            Self::MediaStorage => "Telegram media could not be written to local storage",
            Self::UnsupportedMedia => "This Telegram media type cannot be archived yet",
        };
        f.write_str(message)
    }
}
impl std::error::Error for Error {}
