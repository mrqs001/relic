//! Saved collection intent. This module never acquires data or starts a scheduler.
//! Stable upstream peer/user identity is resolved by the connected adapter;
//! public handles here are setup targets, distinct from retained evidence sources.
use crate::{
    error::{Error, Result},
    model::{hash, ident, now, timestamp},
    store::Store,
};
use axum::http::StatusCode;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const MAX_SUBSCRIPTIONS: usize = 1000;
pub const MAX_SUBSCRIPTION_PAGE: usize = 100;
const COLUMNS: &str = "rowid,id,version,provider,label,target,mode,poll_interval_seconds,enabled,created_at,updated_at";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionQuery {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    pub query: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    provider: String,
    label: String,
    target: String,
    mode: Option<String>,
    poll_interval_seconds: Option<i64>,
    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct Subscription {
    id: String,
    version: i64,
    provider: String,
    label: String,
    target: String,
    mode: String,
    poll_interval_seconds: Option<i64>,
    enabled: bool,
    created_at: String,
    updated_at: String,
    status: &'static str,
    status_detail: &'static str,
}

impl Input {
    fn parse(v: Value) -> Result<Self> {
        let mut input: Self = serde_json::from_value(v)?;
        if !["telegram", "x"].contains(&input.provider.as_str()) {
            return Err(Error::invalid("Choose Telegram or X for collection setup"));
        }
        input.label = input.label.trim().to_string();
        if input.label.is_empty()
            || input.label.len() > 240
            || input.label.chars().any(char::is_control)
        {
            return Err(Error::invalid(
                "Source name requires 1–240 bytes without control characters",
            ));
        }
        input.target = normalize_target(&input.provider, &input.target)?;
        let expected_mode = if input.provider == "telegram" {
            "stream"
        } else {
            "poll"
        };
        if input
            .mode
            .as_deref()
            .is_some_and(|mode| mode != expected_mode)
        {
            return Err(Error::invalid(
                "Telegram uses channel updates; X uses profile polling",
            ));
        }
        input.mode = Some(expected_mode.into());
        if input.provider == "telegram" {
            if input.poll_interval_seconds.is_some() {
                return Err(Error::invalid(
                    "Telegram channel updates do not use a polling interval",
                ));
            }
        } else {
            let interval = input.poll_interval_seconds.unwrap_or(120);
            if !(30..=86400).contains(&interval) {
                return Err(Error::invalid(
                    "X polling interval must be 30–86400 seconds",
                ));
            }
            input.poll_interval_seconds = Some(interval);
        }
        Ok(input)
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.provider, self.label, self.target).to_lowercase()
    }
}

fn normalize_target(provider: &str, raw: &str) -> Result<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 256 {
        return Err(Error::invalid("Enter a public channel or profile handle"));
    }
    let handle = if let Some(url) = raw.strip_prefix("https://") {
        // Deliberately accept only an exact public profile/channel path. No URL
        // decoding, credentials, invite links, post paths, queries or fragments.
        let (host, path) = url
            .split_once('/')
            .ok_or_else(|| Error::invalid("A public profile or channel URL needs a handle"))?;
        let host = host.to_ascii_lowercase();
        let allowed = match provider {
            "telegram" => ["t.me", "telegram.me"].contains(&host.as_str()),
            "x" => {
                ["x.com", "www.x.com", "twitter.com", "www.twitter.com"].contains(&host.as_str())
            }
            _ => false,
        };
        if !allowed {
            return Err(Error::invalid(
                "Use a public Telegram channel or X profile URL for the selected provider",
            ));
        }
        path.strip_suffix('/').unwrap_or(path)
    } else {
        raw.strip_prefix('@').unwrap_or(raw)
    };
    let valid_chars = handle
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_');
    let valid_length = match provider {
        "telegram" => {
            (5..=32).contains(&handle.len())
                && handle
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphabetic)
        }
        "x" => (1..=15).contains(&handle.len()),
        _ => false,
    };
    let handle = handle.to_ascii_lowercase();
    let reserved = match provider {
        "telegram" => [
            "joinchat",
            "addstickers",
            "addemoji",
            "share",
            "proxy",
            "socks",
            "login",
        ]
        .contains(&handle.as_str()),
        "x" => [
            "home",
            "explore",
            "search",
            "settings",
            "messages",
            "notifications",
            "compose",
            "intent",
            "i",
            "login",
            "logout",
            "signup",
            "tos",
            "privacy",
        ]
        .contains(&handle.as_str()),
        _ => true,
    };
    if !valid_chars || !valid_length || reserved {
        return Err(Error::invalid(
            "Use a public handle only; private invites, post links and credentials are not collection targets",
        ));
    }
    Ok(format!("@{handle}"))
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, Subscription)> {
    let enabled: bool = row.get(8)?;
    Ok((
        row.get(0)?,
        Subscription {
            id: row.get(1)?,
            version: row.get(2)?,
            provider: row.get(3)?,
            label: row.get(4)?,
            target: row.get(5)?,
            mode: row.get(6)?,
            poll_interval_seconds: row.get(7)?,
            enabled,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
            status: if enabled { "not_connected" } else { "paused" },
            status_detail: if enabled {
                "Collection requested. The live adapter is not connected; no messages are being acquired."
            } else {
                "Collection is paused. The live adapter is not connected; retained messages remain available."
            },
        },
    ))
}

impl Store {
    pub fn subscriptions(&self, q: SubscriptionQuery) -> Result<Value> {
        let limit = q.limit.unwrap_or(50);
        let query = q.query.unwrap_or_default().trim().to_lowercase();
        if limit == 0 || limit > MAX_SUBSCRIPTION_PAGE || query.len() > 240 {
            return Err(Error::invalid(
                "Collection query requires a page size of 1–100 and search text of at most 240 bytes",
            ));
        }
        let query_hash = hash(query.as_bytes());
        let after = match q.cursor {
            Some(cursor) if cursor.len() <= 96 => {
                let (cursor_hash, key) = cursor
                    .split_once(':')
                    .ok_or_else(|| Error::invalid("Invalid collection page cursor"))?;
                if cursor_hash != query_hash {
                    return Err(Error::invalid(
                        "Collection page cursor belongs to a different search",
                    ));
                }
                key.parse::<i64>()
                    .ok()
                    .filter(|key| *key >= 0)
                    .ok_or_else(|| Error::invalid("Invalid collection page cursor"))?
            }
            Some(_) => return Err(Error::invalid("Invalid collection page cursor")),
            None => 0,
        };
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let total: i64 = tx.query_row(
            "SELECT count(*) FROM subscriptions WHERE instr(search_text,?1)>0",
            [&query],
            |r| r.get(0),
        )?;
        let rows = tx.prepare(&format!("SELECT {COLUMNS} FROM subscriptions WHERE rowid>?1 AND instr(search_text,?2)>0 ORDER BY rowid LIMIT ?3"))?
            .query_map(params![after, query, (limit + 1) as i64], read_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let next_cursor = if rows.len() > limit {
            Some(format!("{query_hash}:{}", rows[limit - 1].0))
        } else {
            None
        };
        let items: Vec<_> = rows.into_iter().take(limit).map(|(_, item)| item).collect();
        tx.commit()?;
        Ok(json!({"items":items,"total":total,"next_cursor":next_cursor}))
    }

    pub fn create_subscription(&self, v: Value) -> Result<Value> {
        let input = Input::parse(v)?;
        self.save_subscription(None, input)
    }

    pub fn update_subscription(&self, id: &str, mut v: Value) -> Result<Value> {
        if !id.starts_with("subscription:") || id.len() > 64 {
            return Err(Error::invalid("Invalid collection identity"));
        }
        let expected = v
            .as_object_mut()
            .and_then(|object| object.remove("expected_version"))
            .and_then(|v| v.as_i64())
            .filter(|v| *v > 0)
            .ok_or_else(|| Error::invalid("expected_version must be a positive integer"))?;
        let input = Input::parse(v)?;
        self.save_subscription(Some((id, expected)), input)
    }

    pub fn subscription(&self, id: &str) -> Result<Value> {
        if !id.starts_with("subscription:") || id.len() > 64 {
            return Err(Error::invalid("Invalid collection identity"));
        }
        let c = self.connect()?;
        let (_, subscription) = c
            .query_row(
                &format!("SELECT {COLUMNS} FROM subscriptions WHERE id=?1"),
                [id],
                read_row,
            )
            .optional()?
            .ok_or_else(|| Error::missing("Collection configuration is unavailable"))?;
        Ok(serde_json::to_value(subscription)?)
    }

    fn save_subscription(&self, update: Option<(&str, i64)>, input: Input) -> Result<Value> {
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (id, version, created_at) = if let Some((id, expected)) = update {
            let previous = tx
                .query_row(
                    &format!("SELECT {COLUMNS} FROM subscriptions WHERE id=?1"),
                    [id],
                    read_row,
                )
                .optional()?
                .ok_or_else(|| Error::missing("Collection configuration is unavailable"))?
                .1;
            if previous.version != expected {
                return Err(Error::conflict(
                    "Collection settings changed elsewhere; reload before saving",
                ));
            }
            (
                id.to_string(),
                expected.checked_add(1).ok_or_else(Error::internal)?,
                previous.created_at,
            )
        } else {
            let count: i64 =
                tx.query_row("SELECT count(*) FROM subscriptions", [], |r| r.get(0))?;
            if count >= MAX_SUBSCRIPTIONS as i64 {
                return Err(Error {
                    status: StatusCode::UNPROCESSABLE_ENTITY,
                    code: "subscription_limit",
                    message:
                        "This workspace supports at most 1000 saved collection configurations."
                            .into(),
                });
            }
            (ident("subscription"), 1, now())
        };
        let duplicate: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM subscriptions WHERE provider=?1 AND target=?2 AND id<>?3)",
            params![input.provider, input.target, id],
            |r| r.get(0),
        )?;
        if duplicate {
            return Err(Error::conflict(
                "This channel or profile is already configured; edit its existing settings",
            ));
        }
        tx.execute("INSERT INTO subscriptions(id,version,provider,label,target,mode,poll_interval_seconds,enabled,created_at,updated_at,search_text) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(id) DO UPDATE SET version=excluded.version,provider=excluded.provider,label=excluded.label,target=excluded.target,mode=excluded.mode,poll_interval_seconds=excluded.poll_interval_seconds,enabled=excluded.enabled,updated_at=excluded.updated_at,search_text=excluded.search_text",
            params![id,version,input.provider,input.label,input.target,input.mode,input.poll_interval_seconds,input.enabled,created_at,now(),input.search_text()])?;
        let result = tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM subscriptions WHERE id=?1"),
                [&id],
                read_row,
            )?
            .1;
        let event = Self::event(&tx, "collection_settings", None, vec![id])?;
        tx.commit()?;
        let _ = self.events.send(event);
        Ok(serde_json::to_value(result)?)
    }
}

/// Older backups predate collection setup. New backups retain these rows as
/// authoritative SQLite state and validate the same constraints on restore.
pub(crate) fn verify_subscriptions(c: &Connection) -> Result<usize> {
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='subscriptions')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    let rows = c
        .prepare(&format!("SELECT {COLUMNS} FROM subscriptions LIMIT ?1"))?
        .query_map([(MAX_SUBSCRIPTIONS + 1) as i64], read_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if rows.len() > MAX_SUBSCRIPTIONS {
        return Err(Error::invalid(
            "Restored collection configurations exceed their bound",
        ));
    }
    for (_, row) in &rows {
        let input = Input::parse(
            json!({"provider":row.provider,"label":row.label,"target":row.target,"mode":row.mode,"poll_interval_seconds":row.poll_interval_seconds,"enabled":row.enabled}),
        )?;
        if input.target != row.target
            || input.label != row.label
            || input.poll_interval_seconds != row.poll_interval_seconds
            || row.version < 1
            || timestamp(&row.created_at)? != row.created_at
            || timestamp(&row.updated_at)? != row.updated_at
        {
            return Err(Error::invalid(
                "Restored collection configuration is invalid",
            ));
        }
    }
    Ok(rows.len())
}
