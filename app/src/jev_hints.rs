//! Possible events from Jev (owner authorization, 24 October 2026).
//!
//! A message the main reading read with no events joins Event reports as a
//! marked possible event when Jev's newest answer for it finds an event type
//! present with confidence of at least [`MIN_CONFIDENCE`], calls the claim
//! reported, and the type is not one Jev uses for political talk
//! ([`EXCLUDED`]). The rule and its evaluation are in
//! validation/JEV_POSSIBLE_EVENTS.md. Hints never change a reading, a map
//! position, media or event links; the main reading stays authoritative and a
//! message with any main-reading event never shows a hint.
use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::HashMap;

pub const POLICY: &str = "relic.jev-possible-events.v1";
pub const MIN_CONFIDENCE: f64 = 0.8;
/// Types where Jev's high-confidence answers were mostly wrong in the
/// 24 October evaluation (it tags remarks about them as occurrences).
pub const EXCLUDED: [&str; 3] = ["election", "protest", "territorial_change"];

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS jev_event_hints(
 evidence_rowid INTEGER NOT NULL REFERENCES evidence(rowid),event_type TEXT NOT NULL,
 confidence REAL NOT NULL,attempt_id TEXT NOT NULL,PRIMARY KEY(evidence_rowid,event_type));";

/// Qualifying types from each message's newest completed answer of the current
/// recipe (the answer given with the main reading as context, when there is one).
const INSERT: &str = "INSERT INTO jev_event_hints
 SELECT a.evidence_rowid,substr(j.key,7),json_extract(j.value,'$.confidence'),a.id
 FROM supplemental_attempts a,json_each(a.result_json) j
 WHERE (?1 IS NULL OR a.evidence_rowid=?1) AND a.recipe=?2 AND a.status='completed'
 AND a.id=(SELECT b.id FROM supplemental_attempts b WHERE b.evidence_rowid=a.evidence_rowid AND b.recipe=?2 AND b.status='completed' ORDER BY b.phase='enriched' DESC,b.created_at DESC,b.id DESC LIMIT 1)
 AND json_extract(a.result_json,'$.claim_state.choice')='reported'
 AND j.key LIKE 'event!_%' ESCAPE '!' AND json_extract(j.value,'$.choice')='present'
 AND json_extract(j.value,'$.confidence')>=?3
 AND substr(j.key,7) NOT IN (SELECT value FROM json_each(?4))";

fn version() -> String {
    format!("{POLICY}/{}", crate::supplemental_ai::RECIPE)
}

/// Creates the hint table and rebuilds it when the rule or Jev recipe changed.
pub fn migrate(c: &Connection) -> Result<()> {
    c.execute_batch(SCHEMA)?;
    let built: Option<String> = c
        .query_row("SELECT value FROM metadata WHERE key='jev_event_hints'", [], |r| r.get(0))
        .optional()?;
    if built.as_deref() != Some(version().as_str()) {
        refresh(c, None)?;
        c.execute("INSERT INTO metadata VALUES('jev_event_hints',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [version()])?;
    }
    Ok(())
}

/// Rebuilds one message's hints, or every message's with `None`.
pub fn refresh(c: &Connection, rowid: Option<i64>) -> Result<()> {
    c.execute("DELETE FROM jev_event_hints WHERE ?1 IS NULL OR evidence_rowid=?1", [rowid])?;
    c.execute(
        INSERT,
        params![rowid, crate::supplemental_ai::RECIPE, MIN_CONFIDENCE, json!(EXCLUDED).to_string()],
    )?;
    Ok(())
}

/// Scope condition on `reading_evidence e`: the message has a main reading with
/// no events (readings without events omit the field) and Jev suggests one (of
/// the bound type when `typed`).
pub(crate) fn possible(typed: bool) -> String {
    format!(
        "(EXISTS(SELECT 1 FROM jev_event_hints h WHERE h.evidence_rowid=e.rowid{}) AND e.reading_json IS NOT NULL AND coalesce(json_array_length(e.reading_json,'$.events'),0)=0)",
        if typed { " AND h.event_type=?" } else { "" }
    )
}

/// Adds `possible_event` to messages whose reading has no events: Jev's most
/// confident qualifying type, or the `preferred` type when it qualifies.
pub(crate) fn attach(c: &Connection, items: &mut [Value], preferred: Option<&str>) -> Result<()> {
    let key = |item: &Value| -> Option<i64> {
        let reading = item["interpretation"].as_object()?;
        let empty = reading.get("events").is_none_or(|e| e.is_null() || e.as_array().is_some_and(Vec::is_empty));
        if empty { item["message_key"].as_str()?.parse().ok() } else { None }
    };
    let keys: Vec<i64> = items.iter().filter_map(key).collect();
    if keys.is_empty() {
        return Ok(());
    }
    let mut best = HashMap::<i64, (String, f64)>::new();
    let mut stmt = c.prepare("SELECT evidence_rowid,event_type,confidence FROM jev_event_hints WHERE evidence_rowid IN (SELECT value FROM json_each(?1)) ORDER BY evidence_rowid,event_type=?2 DESC,confidence DESC,event_type")?;
    let rows = stmt.query_map(params![serde_json::to_string(&keys)?, preferred], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, f64>(2)?))
    })?;
    for row in rows {
        let (rowid, kind, confidence) = row?;
        best.entry(rowid).or_insert((kind, confidence));
    }
    for item in items {
        if let Some((kind, confidence)) = key(item).and_then(|k| best.get(&k)) {
            item["possible_event"] = json!({"type":kind,"confidence":confidence,"suggested_by":"jev"});
        }
    }
    Ok(())
}
