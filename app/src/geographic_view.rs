//! Shared, publication-pinned site projection. Message membership remains one
//! row even when several supported event sites are visible on the map.
use crate::{error::Result, model::Scope};
use rusqlite::{Connection, types::Value as SqlValue};
use serde_json::{Value, json};

// Scope::parse validates finite coordinates and the fixed event vocabulary.
// Only those typed values are embedded; user text never enters SQL here.
pub(crate) fn site_filter(scope: &Scope, bounds: bool) -> String {
    format!(
        "s.evidence_rowid=e.rowid AND s.publication_id=e.publication_id{}",
        site_conditions(scope, bounds)
    )
}
/// The event-type and bounds conditions of [`site_filter`] on the site alone.
fn site_conditions(scope: &Scope, bounds: bool) -> String {
    let mut sql = String::new();
    if let Some(kind) = crate::processing_ai::EVENT_TYPES
        .iter()
        .find(|kind| scope.event_type.as_deref() == Some(**kind))
    {
        sql.push_str(&format!(" AND s.event_type='{kind}'"));
    }
    if bounds && let Some([w, s, e, n]) = scope.bbox {
        sql.push_str(&format!(" AND s.lat>={s} AND s.lat<={n} AND "));
        if w <= e {
            sql.push_str(&format!("s.lon>={w} AND s.lon<={e}"));
        } else {
            sql.push_str(&format!("(s.lon>={w} OR s.lon<={e})"));
        }
    }
    sql
}
pub(crate) fn has_site(scope: &Scope, bounds: bool) -> String {
    has_site_in(scope, bounds, None)
}
/// `areas`: the scope's precomputed bounded-area set ([`crate::area_view::bounded_areas`]).
pub(crate) fn has_site_in(scope: &Scope, bounds: bool, areas: Option<&str>) -> String {
    format!(
        "(EXISTS(SELECT 1 FROM reading_message_sites s WHERE {}) OR {})",
        site_filter(scope, bounds),
        crate::area_view::has_area_in(scope, bounds, areas)
    )
}

/// Group by publisher place identity within a message, never by a claimed
/// independent incident. A site's event types remain separate until filtered.
pub(crate) fn sites(
    c: &Connection,
    scope: &Scope,
    pred: &str,
    vals: &[SqlValue],
    limit: usize,
) -> Result<Vec<Value>> {
    let filter = site_filter(scope, true);
    let matched = format!("SELECT e.rowid AS evidence_rowid,s.site_key,s.lon,s.lat,min(s.label) AS label,max(s.approximate) AS approximate,CASE WHEN count(DISTINCT s.event_type)=1 THEN min(s.event_type) END AS event_type FROM reading_evidence e JOIN reading_message_sites s ON {filter} WHERE {pred} GROUP BY e.rowid,s.site_key ORDER BY e.rowid,s.site_key LIMIT {limit}");
    features(c, scope, &matched, vals.to_vec())
}
/// [`sites`] of exactly these messages (a JSON array of message keys) of the
/// scope's publication. Every site of a publication belongs to one of its
/// messages (a published reading requires membership), so the site view is
/// read for these keys alone, without composing each message's reading.
pub(crate) fn sites_of(c: &Connection, scope: &Scope, keys: &str, limit: usize) -> Result<Vec<Value>> {
    let conditions = site_conditions(scope, true);
    let matched = format!("SELECT s.evidence_rowid,s.site_key,s.lon,s.lat,min(s.label) AS label,max(s.approximate) AS approximate,CASE WHEN count(DISTINCT s.event_type)=1 THEN min(s.event_type) END AS event_type FROM reading_message_sites s WHERE s.publication_id=? AND s.evidence_rowid IN (SELECT value FROM json_each(?)){conditions} GROUP BY s.evidence_rowid,s.site_key ORDER BY s.evidence_rowid,s.site_key LIMIT {limit}");
    let args = vec![
        SqlValue::Text(scope.publication_id.clone().unwrap_or_default()),
        SqlValue::Text(keys.to_owned()),
    ];
    features(c, scope, &matched, args)
}
/// Map features of grouped sites. Site membership is bounded and grouped
/// first: reading the full composed evidence JSON for every map point rebuilt
/// every original + reading. Titles and event types come from reading previews.
fn features(c: &Connection, scope: &Scope, matched: &str, mut args: Vec<SqlValue>) -> Result<Vec<Value>> {
    args.push(SqlValue::Text(
        scope.publication_id.clone().unwrap_or_default(),
    ));
    let mut stmt = c.prepare(&format!("WITH matched_sites AS ({matched}) SELECT e.rowid,e.id,e.revision,e.source,coalesce(p.title,e.text),s.site_key,s.lon,s.lat,s.label,s.approximate,s.event_type,e.published_at,e.acquired_at,p.events,i.id FROM matched_sites s JOIN evidence e ON e.rowid=s.evidence_rowid LEFT JOIN interpretations i ON i.id=(SELECT pi.interpretation_id FROM publication_interpretations pi WHERE pi.evidence_rowid=e.rowid AND pi.publication_id=?) LEFT JOIN interpretation_previews p ON p.interpretation_id=i.id ORDER BY e.rowid,s.site_key"))?;
    Ok(stmt.query_map(rusqlite::params_from_iter(args), |row| {
        let key: i64 = row.get(0)?;
        let site_key: String = row.get(5)?;
        let title: String = row.get(4)?;
        Ok(json!({"type":"Feature","id":format!("{key}:{}",crate::model::hash(site_key.as_bytes())),"geometry":{"type":"Point","coordinates":[row.get::<_,f64>(6)?,row.get::<_,f64>(7)?]},"properties":{"message_key":key.to_string(),"evidence_id":row.get::<_,String>(1)?,"revision":row.get::<_,String>(2)?,"source":row.get::<_,String>(3)?,"title":title.chars().take(400).collect::<String>(),"site_key":site_key,"location_label":row.get::<_,Option<String>>(8)?,"approximate":row.get::<_,bool>(9)?,"event_type":row.get::<_,Option<String>>(10)?,"published_at":row.get::<_,Option<String>>(11)?,"acquired_at":row.get::<_,String>(12)?,"events":preview_events(row.get::<_,Option<String>>(13)?.as_deref()),"processed":row.get::<_,Option<String>>(14)?.is_some()}}))
    })?.collect::<std::result::Result<_,_>>()?)
}

pub(crate) fn attach_sites(evidence: &mut Value, sites: &[Value]) {
    evidence["map_locations"] = json!(sites.iter().filter(|f| f["properties"]["evidence_id"] == evidence["id"] && f["properties"]["revision"] == evidence["revision"]).map(|f| json!({"id":f["properties"]["site_key"],"label":f["properties"]["location_label"],"geometry":f["geometry"],"approximate":f["properties"]["approximate"]})).collect::<Vec<_>>());
}

/// Older workspaces have no site index. Backfill transactionally from retained
/// records, marking zero-site interpretations too. New publications are indexed
/// by the same SQL projection in an INSERT trigger inside their commit.
pub(crate) fn backfill(c: &Connection) -> Result<()> {
    let tx = c.unchecked_transaction()?;
    tx.execute_batch("INSERT OR IGNORE INTO interpretation_sites SELECT * FROM projected_interpretation_sites WHERE interpretation_id IN (SELECT id FROM interpretations WHERE id NOT IN (SELECT interpretation_id FROM interpretation_geography_indexed)); INSERT OR IGNORE INTO interpretation_event_states SELECT * FROM projected_interpretation_event_states WHERE interpretation_id IN (SELECT id FROM interpretations WHERE id NOT IN (SELECT interpretation_id FROM interpretation_geography_indexed)); INSERT OR IGNORE INTO interpretation_geography_indexed SELECT id FROM interpretations;")?;
    tx.execute_batch("INSERT OR IGNORE INTO interpretation_areas SELECT * FROM projected_interpretation_areas WHERE interpretation_id NOT IN (SELECT interpretation_id FROM interpretation_areas_indexed); INSERT OR IGNORE INTO interpretation_areas_indexed SELECT id FROM interpretations;")?;
    // Every reading has exactly one preview row. Workspaces from before the
    // projection get theirs once here (one parse per retained reading); the
    // missing identities come from the two key indexes, so a complete
    // projection reads no reading documents at startup.
    tx.execute_batch("INSERT OR IGNORE INTO interpretation_previews SELECT * FROM projected_interpretation_previews WHERE interpretation_id IN (SELECT id FROM interpretations EXCEPT SELECT interpretation_id FROM interpretation_previews);")?;
    // Event groups join links to the readings published at a scope's
    // publication by reading identity; without this SQLite rebuilt a
    // temporary index over every reading interval on each map/feed query.
    tx.execute_batch("CREATE INDEX IF NOT EXISTS interpretation_interval_reading ON interpretation_intervals(interpretation_id);")?;
    tx.commit()?;
    Ok(())
}

/// Small presentation projection; exact support remains in the reading.
pub(crate) fn preview_events(raw: Option<&str>) -> Vec<Value> {
    raw.and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|event| json!({"type":event["type"],"state":event["state"]}))
        .collect()
}
