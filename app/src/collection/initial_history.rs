//! A durable initial interval; independent of later new-message watermarks.
use super::*;
use chrono::{DateTime, Months, SecondsFormat};
use std::collections::BTreeSet;

const MAX_MESSAGES: usize = 100;
const MAX_PAGES: u64 = 30;

pub(super) fn prepare(state: &mut Value, at: &str) -> Result<()> {
    // An older unfinished catch-up keeps its original interval until drained.
    if state["initial_history"].is_object() || state["continuation"].is_object() {
        return Ok(());
    }
    let start = DateTime::parse_from_rfc3339(at).map_err(|_| Error::internal())?;
    let cutoff = start
        .checked_sub_months(Months::new(1))
        .ok_or_else(Error::internal)?;
    state["initial_history"] = json!({"version":1,"started_at":at,"cutoff":cutoff.to_rfc3339_opts(SecondsFormat::Millis,true),"seen_ids":[],"pages":0,"complete":false});
    Ok(())
}
pub(super) fn active(state: &Value) -> bool {
    state["initial_history"].is_object() && state["initial_history"]["complete"] != true
}
pub(super) fn auto_continue(state: &Value) -> bool {
    active(state)
        && state["initial_history"]["pages"]
            .as_u64()
            .unwrap_or(MAX_PAGES)
            < MAX_PAGES
        && (state["status"] == "catching_up"
            || (state["status"] == "needs_attention"
                && state["last_error_code"].is_string()
                && state["last_error_code"] != "source_unavailable"))
}
pub(super) fn page(
    state: &mut Value,
    mut messages: Vec<Value>,
    drained: bool,
) -> Result<(Vec<Value>, bool)> {
    let history = &mut state["initial_history"];
    let cutoff = history["cutoff"]
        .as_str()
        .ok_or_else(Error::internal)?
        .to_owned();
    let mut seen: BTreeSet<String> = history["seen_ids"]
        .as_array()
        .ok_or_else(Error::internal)?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    // Provider pages may contain duplicate/context order. Sort ordinary evidence
    // newest first before enforcing the remaining distinct-message allowance.
    messages.sort_by(|a, b| {
        b["published_at"]
            .as_str()
            .cmp(&a["published_at"].as_str())
            .then_with(|| {
                let sequence = |m: &Value| {
                    m["id"]
                        .as_str()
                        .and_then(|id| id.rsplit(':').next())
                        .and_then(|id| id.parse::<u128>().ok())
                };
                sequence(b).cmp(&sequence(a))
            })
    });
    let mut retained = vec![];
    let mut older = false;
    for message in messages {
        let Some(at) = message["published_at"].as_str() else {
            continue;
        };
        let at = crate::model::timestamp(at)?;
        if at < cutoff {
            older = true;
            continue;
        }
        if seen.len() < MAX_MESSAGES && seen.insert(text(&message, "id")) {
            retained.push(message);
        }
    }
    let complete = seen.len() >= MAX_MESSAGES || older || drained;
    history["seen_ids"] = json!(seen);
    history["pages"] = json!(history["pages"].as_u64().unwrap_or(0) + 1);
    history["complete"] = json!(complete);
    history["stop_reason"] = json!(if seen.len() >= MAX_MESSAGES {
        "message_limit"
    } else if older {
        "month_limit"
    } else if drained {
        "source_end"
    } else {
        "continuing"
    });
    Ok((retained, complete))
}
pub(super) fn description(state: &Value, complete: bool) -> String {
    let h = &state["initial_history"];
    let count = h["seen_ids"].as_array().map_or(0, Vec::len);
    if complete {
        format!(
            "Initial history ready: {count} messages within the last month (maximum 100). Listening for new messages."
        )
    } else if h["pages"].as_u64().unwrap_or(0) >= MAX_PAGES {
        "Initial history reached its 30-page safety limit. Retained pages are safe; inspect the source before continuing.".into()
    } else {
        format!("Loading recent history: {count} of up to 100 messages within the last month.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn message(n: usize, at: &str) -> Value {
        json!({"id":format!("test:{n}"),"published_at":at})
    }
    #[test]
    fn month_and_unique_count_stop_whichever_comes_first() {
        let mut s = json!({});
        prepare(&mut s, "2026-03-31T12:00:00.000Z").unwrap();
        assert_eq!(s["initial_history"]["cutoff"], "2026-02-28T12:00:00.000Z");
        let (first, done) = page(
            &mut s,
            (0..70)
                .map(|n| message(n, "2026-03-20T00:00:00Z"))
                .collect(),
            false,
        )
        .unwrap();
        assert_eq!(first.len(), 70);
        assert!(!done);
        let (second, done) = page(
            &mut s,
            (60..150)
                .map(|n| message(n, "2026-03-19T00:00:00Z"))
                .collect(),
            false,
        )
        .unwrap();
        assert_eq!(second.len(), 30);
        assert!(done);
        assert_eq!(s["initial_history"]["stop_reason"], "message_limit");
        let mut s = json!({});
        prepare(&mut s, "2026-03-31T12:00:00.000Z").unwrap();
        let (items, done) = page(
            &mut s,
            vec![
                message(1, "2026-02-28T12:00:00Z"),
                message(2, "2026-02-28T11:59:59.999Z"),
            ],
            false,
        )
        .unwrap();
        assert_eq!(items.len(), 1);
        assert!(done);
        assert_eq!(s["initial_history"]["stop_reason"], "month_limit");
    }
    #[test]
    fn restart_retains_cutoff_and_existing_watermark_and_catchup() {
        let mut s = json!({"watermark":"123"});
        prepare(&mut s, "2026-09-12T12:00:00.000Z").unwrap();
        let saved = s.clone();
        prepare(&mut s, "2026-09-13T12:00:00.000Z").unwrap();
        assert_eq!(s, saved);
        let mut old = json!({"continuation":{"cursor":"old"},"watermark":"123"});
        prepare(&mut old, "2026-09-12T12:00:00.000Z").unwrap();
        assert!(!active(&old));
        s["status"] = json!("catching_up");
        assert!(auto_continue(&s));
        s["initial_history"]["pages"] = json!(30);
        assert!(!auto_continue(&s));
        assert!(page(&mut s, vec![], true).unwrap().1);
    }
}
