//! Bounded message-time counts over exactly the map/feed publication predicate.
use crate::{error::Result, model::Scope};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};

fn millis(value: &str) -> i64 {
    // Scope timestamps were validated and normalized by Scope::parse.
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .timestamp_millis()
}
fn timestamp(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The scope's message-time column and its SQL millisecond stamp. Round,
/// rather than truncate, to preserve stored millisecond boundaries.
pub(crate) fn stamp_sql(scope: &Scope) -> (&'static str, String) {
    let field = if scope.time_basis == "acquisition_time" {
        "e.acquired_at"
    } else {
        "e.published_at"
    };
    (field, format!("CAST(round(unixepoch({field},'subsec')*1000) AS INTEGER)"))
}

/// One in-scope message for the timeline: its [`stamp_sql`] value and whether
/// its time column is NULL. A non-NULL time SQLite cannot read has no stamp
/// but is not undated.
#[derive(Clone, Copy)]
pub(crate) struct Stamp {
    pub stamp: Option<i64>,
    pub undated: bool,
}

/// The timeline of every message in a scope, from one read of its stamps.
pub(crate) fn summarise(scope: &Scope, rows: &[Stamp]) -> Result<Value> {
    let first = rows.iter().filter_map(|r| r.stamp).min();
    let last = rows.iter().filter_map(|r| r.stamp).max();
    let undated = rows.iter().filter(|r| r.undated).count() as i64;
    let dated = || rows.iter().filter(|r| !r.undated);
    let mut start = scope.from.as_deref().map(millis).or(first);
    let mut earlier = 0_i64;
    // Keep All membership intact while preventing a pinned outlier from
    // compressing nearly all activity into one bar. Older rows stay selectable.
    if scope.from.is_none()
        && scope.to.is_none()
        && let (Some(first), Some(last)) = (first, last)
    {
        for days in [7, 30, 90] {
            let cutoff = last - days * 24 * 60 * 60 * 1000;
            if first < cutoff {
                let old = dated().filter(|r| r.stamp.is_some_and(|s| s < cutoff)).count() as i64;
                let total = dated().count() as i64;
                if old > 0 && old * 5 <= total {
                    start = Some(cutoff);
                    earlier = old;
                    break;
                }
            }
        }
    }
    let end = scope.to.as_deref().map(millis).or(last);
    let (Some(start), Some(end)) = (start.or(end), end.or(start)) else {
        return Ok(
            json!({"from":null,"to":null,"time_basis":scope.time_basis,"undated":undated,"buckets":[]}),
        );
    };
    let span = end - start + 1;
    let width = (span + 35) / 36;
    let size = ((span + width - 1) / width) as usize;
    let mut counts = vec![0_i64; size];
    for stamp in dated().filter_map(|r| r.stamp).filter(|s| *s >= start) {
        *counts
            .get_mut(((stamp - start) / width) as usize)
            .ok_or_else(crate::error::Error::internal)? += 1;
    }
    let buckets: Vec<_> = counts.into_iter().enumerate().map(|(i, count)| {
        let from = start + i as i64 * width;
        json!({"from":timestamp(from),"to":timestamp((from + width - 1).min(end)),"count":count})
    }).collect();
    Ok(
        json!({"from":timestamp(start),"to":timestamp(end),"time_basis":scope.time_basis,"undated":undated,"earlier":earlier,"earliest":first.map(timestamp),"buckets":buckets}),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rusqlite::{Connection, types::Value as SqlValue};

    /// The SQL timeline this module replaced: one statement per aggregate.
    pub(crate) fn reference(c: &Connection, scope: &Scope, pred: &str, vals: &[SqlValue]) -> Result<Value> {
        let (field, time) = stamp_sql(scope);
        let (first, last, undated): (Option<i64>, Option<i64>, i64) = c.query_row(
            &format!("SELECT min({time}),max({time}),coalesce(sum({field} IS NULL),0) FROM reading_evidence e WHERE {pred}"),
            rusqlite::params_from_iter(vals), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let mut start = scope.from.as_deref().map(millis).or(first);
        let mut earlier = 0_i64;
        if scope.from.is_none()
            && scope.to.is_none()
            && let (Some(first), Some(last)) = (first, last)
        {
            for days in [7, 30, 90] {
                let cutoff = last - days * 24 * 60 * 60 * 1000;
                if first < cutoff {
                    let mut args = vals.to_vec();
                    args.push(SqlValue::Integer(cutoff));
                    let (old, total): (i64,i64) = c.query_row(&format!("WITH matched AS (SELECT {time} AS stamp FROM reading_evidence e WHERE {pred} AND {field} IS NOT NULL) SELECT coalesce(sum(stamp<?),0),count(*) FROM matched"), rusqlite::params_from_iter(args), |r| Ok((r.get(0)?,r.get(1)?)))?;
                    if old > 0 && old * 5 <= total {
                        start = Some(cutoff);
                        earlier = old;
                        break;
                    }
                }
            }
        }
        let end = scope.to.as_deref().map(millis).or(last);
        let (Some(start), Some(end)) = (start.or(end), end.or(start)) else {
            return Ok(
                json!({"from":null,"to":null,"time_basis":scope.time_basis,"undated":undated,"buckets":[]}),
            );
        };
        let span = end - start + 1;
        let width = (span + 35) / 36;
        let size = ((span + width - 1) / width) as usize;
        let mut counts = vec![0_i64; size];
        let mut args = vals.to_vec();
        args.push(SqlValue::Integer(start));
        args.push(SqlValue::Integer(width));
        args.push(SqlValue::Integer(start));
        let mut stmt = c.prepare(&format!(
            "WITH matched AS (SELECT {time} AS stamp FROM reading_evidence e WHERE {pred} AND {field} IS NOT NULL) SELECT (stamp-?)/? AS bucket,count(*) FROM matched WHERE stamp>=? GROUP BY bucket",
        ))?;
        for row in stmt.query_map(rusqlite::params_from_iter(args), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })? {
            let (index, count) = row?;
            *counts
                .get_mut(index as usize)
                .ok_or_else(crate::error::Error::internal)? = count;
        }
        let buckets: Vec<_> = counts.into_iter().enumerate().map(|(i, count)| {
            let from = start + i as i64 * width;
            json!({"from":timestamp(from),"to":timestamp((from + width - 1).min(end)),"count":count})
        }).collect();
        Ok(
            json!({"from":timestamp(start),"to":timestamp(end),"time_basis":scope.time_basis,"undated":undated,"earlier":earlier,"earliest":first.map(timestamp),"buckets":buckets}),
        )
    }

    /// The scope's time bounds, as the store predicate applies them.
    fn predicate(scope: &Scope) -> (String, Vec<SqlValue>) {
        let (field, _) = stamp_sql(scope);
        let mut sql = "e.source=?".to_owned();
        let mut vals = vec![SqlValue::Text("s".into())];
        for (bound, op) in [(&scope.from, ">="), (&scope.to, "<=")] {
            if let Some(bound) = bound {
                sql.push_str(&format!(" AND {field}{op}?"));
                vals.push(SqlValue::Text(bound.clone()));
            }
        }
        (sql, vals)
    }

    fn summary(c: &Connection, scope: &Scope) -> Value {
        let (field, time) = stamp_sql(scope);
        let (pred, vals) = predicate(scope);
        let rows = c
            .prepare(&format!("SELECT {time},{field} IS NULL FROM reading_evidence e WHERE {pred}"))
            .unwrap()
            .query_map(rusqlite::params_from_iter(vals), |r| Ok(Stamp { stamp: r.get(0)?, undated: r.get(1)? }))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        summarise(scope, &rows).unwrap()
    }

    #[test]
    fn one_pass_summary_equals_the_sql_timeline() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE reading_evidence(source TEXT,published_at TEXT,acquired_at TEXT NOT NULL);").unwrap();
        let insert = |published: Option<&str>, acquired: &str| {
            c.execute("INSERT INTO reading_evidence VALUES('s',?1,?2)", rusqlite::params![published, acquired]).unwrap();
        };
        let scopes = || {
            let all = Scope::default();
            let acquisition = Scope { time_basis: "acquisition_time".into(), ..Scope::default() };
            let window = Scope {
                from: Some("2026-01-01T00:00:00.000Z".into()),
                to: Some("2026-03-01T00:00:00.000Z".into()),
                ..Scope::default()
            };
            let open = Scope { from: Some("2026-02-01T00:00:00.000Z".into()), ..Scope::default() };
            let until = Scope { to: Some("2026-02-01T00:00:00.000Z".into()), ..Scope::default() };
            [all, acquisition, window, open, until]
        };
        let check = |label: &str| {
            for scope in scopes() {
                let (pred, vals) = predicate(&scope);
                let expected = reference(&c, &scope, &pred, &vals).unwrap();
                assert_eq!(summary(&c, &scope), expected, "{label}: {scope:?}");
            }
        };
        check("empty");
        insert(None, "2026-01-05T00:00:00Z");
        check("undated only");
        insert(Some("2026-02-01T10:00:00.250Z"), "2026-02-01T10:00:01Z");
        check("one message");
        for day in 1..=28 {
            insert(Some(&format!("2026-02-{day:02}T12:00:00.{:03}Z", day * 7)), "2026-03-01T00:00:00Z");
        }
        // Unreadable times have no stamp but are not undated.
        insert(Some("unreadable"), "2026-03-02T00:00:00Z");
        check("dense month");
        // A pinned outlier: 7-day, 30-day and 90-day cutoffs are all tried.
        insert(Some("2025-01-01T00:00:00Z"), "2025-01-02T00:00:00Z");
        insert(Some("2025-11-15T00:00:00Z"), "2025-11-16T00:00:00Z");
        check("outliers");
        for hour in 0..24 {
            insert(Some(&format!("2026-02-28T{hour:02}:30:00Z")), "2026-02-28T23:59:59.999Z");
        }
        check("recent burst");
    }
}
