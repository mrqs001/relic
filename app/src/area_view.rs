//! Areas share message membership while geometry is sent once per area.
use crate::{
    error::Result,
    model::{Scope, hash},
};
use rusqlite::{Connection, types::Value as SqlValue};
use serde_json::{Value, json};
pub fn filter(scope: &Scope, bounds: bool) -> String {
    filter_in(scope, bounds, None)
}
/// Areas of the scope's publication whose full geometry intersects its bounds,
/// as a JSON array. Evaluated once per query; only outlines whose bounding
/// rectangle can touch the bounds are read (CASE keeps that order).
pub(crate) fn bounded_areas(c: &Connection, scope: &Scope) -> Result<Option<String>> {
    let Some([w, s, e, n]) = scope.bbox else {
        return Ok(None);
    };
    let publication = scope.publication_id.as_deref().unwrap_or("");
    let ids = c
        .prepare("SELECT id FROM boundary_shapes WHERE id IN(SELECT area_id FROM reading_message_areas WHERE publication_id=?1) AND CASE WHEN relic_bounds_may_intersect(bounds,?2,?3,?4,?5) THEN relic_area_intersects(full_json,?2,?3,?4,?5) ELSE 0 END")?
        .query_map(rusqlite::params![publication, w, s, e, n], |r| r.get::<_, Option<String>>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Some(serde_json::to_string(&ids)?))
}
/// As [`filter`]; `areas` is the scope's [`bounded_areas`] set when already known.
pub(crate) fn filter_in(scope: &Scope, bounds: bool, areas: Option<&str>) -> String {
    format!(
        "a.evidence_rowid=e.rowid AND a.publication_id=e.publication_id{}",
        conditions(scope, bounds, areas)
    )
}
/// The event-type and bounds conditions of [`filter_in`] on the area alone.
fn conditions(scope: &Scope, bounds: bool, areas: Option<&str>) -> String {
    let mut sql = String::new();
    if let Some(kind) = crate::processing_ai::EVENT_TYPES
        .iter()
        .find(|k| scope.event_type.as_deref() == Some(**k))
    {
        sql.push_str(&format!(" AND a.event_type='{kind}'"));
    }
    if bounds && scope.bbox.is_some() && let Some(areas) = areas {
        // Retained area identities, quoted as one SQL string literal.
        sql.push_str(&format!(
            " AND a.area_id IN(SELECT value FROM json_each('{}'))",
            areas.replace('\'', "''")
        ));
    } else if bounds && let Some([w, s, e, n]) = scope.bbox {
        let publication = scope
            .publication_id
            .as_deref()
            .unwrap_or("")
            .replace("'", "''");
        sql.push_str(&format!(" AND a.area_id IN(SELECT id FROM boundary_shapes WHERE id IN(SELECT area_id FROM reading_message_areas WHERE publication_id='{publication}') AND CASE WHEN relic_bounds_may_intersect(bounds,{w},{s},{e},{n}) THEN relic_area_intersects(full_json,{w},{s},{e},{n}) ELSE 0 END)"));
    }
    sql
}
/// `areas`: the scope's precomputed [`bounded_areas`] set, if known.
pub(crate) fn has_area_in(scope: &Scope, bounds: bool, areas: Option<&str>) -> String {
    format!(
        "EXISTS(SELECT 1 FROM reading_message_areas a WHERE {})",
        filter_in(scope, bounds, areas)
    )
}
pub fn attach(c: &Connection, scope: &Scope, items: &mut [Value]) -> Result<()> {
    attach_in(c, scope, items, None)
}
pub(crate) fn attach_in(
    c: &Connection,
    scope: &Scope,
    items: &mut [Value],
    areas: Option<&str>,
) -> Result<()> {
    let requested = json!(
        items
            .iter()
            .map(|i| json!({"id":i["id"],"revision":i["revision"]}))
            .collect::<Vec<_>>()
    );
    let scope_filter = filter_in(scope, true, areas);
    let mut stmt=c.prepare(&format!("SELECT DISTINCT e.id,e.revision,b.id,b.name,b.bounds,b.level,json_extract(b.metadata,'$.boundaryYearRepresented'),b.iso2 FROM json_each(?2) requested JOIN reading_evidence e ON e.id=json_extract(requested.value,'$.id') AND e.revision=json_extract(requested.value,'$.revision') JOIN reading_message_areas a ON {scope_filter} JOIN boundary_shapes b ON b.id=a.area_id WHERE e.publication_id=?1 ORDER BY e.id,e.revision,b.id"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![scope.publication_id, requested.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                ))
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut by_message = std::collections::HashMap::<(String, String), Vec<Value>>::new();
    for (evidence, revision, id, label, bounds, level, represented_time, country_code) in rows {
        by_message.entry((evidence,revision)).or_default().push(json!({"id":id,"label":label,"bounds":serde_json::from_str::<Value>(&bounds)?,"level":level,"represented_time":represented_time,"country_code":country_code}));
    }
    for item in items {
        item["map_areas"] = json!(
            by_message
                .remove(&(
                    crate::model::required(item, "id")?.into(),
                    crate::model::required(item, "revision")?.into()
                ))
                .unwrap_or_default()
        );
    }
    Ok(())
}

/// Every area of these messages (a JSON array of message keys of the scope's
/// publication), with its newest messages, and the messages within any area.
/// No area-count or byte cap: the map view omits outlines (`shapes: false`)
/// and the browser fetches each immutable outline once from `/area-shape`.
/// `areas` is the scope's precomputed [`bounded_areas`] set.
pub(crate) fn query(
    c: &Connection,
    scope: &Scope,
    keys: &str,
    shapes: bool,
    areas: Option<&str>,
) -> Result<(Value, bool, std::collections::HashSet<i64>)> {
    let conditions = conditions(scope, true, areas);
    // One pass over the messages' area memberships gives each area's exact
    // message count and its newest 100 messages; message details are then read
    // once per message, not once per area that contains it. Every area of a
    // publication belongs to one of its messages (a published reading requires
    // membership), so memberships are read for these keys alone.
    let mut stmt = c.prepare(&format!("SELECT DISTINCT a.area_id,a.evidence_rowid,coalesce(e.published_at,e.acquired_at) FROM reading_message_areas a JOIN evidence e ON e.rowid=a.evidence_rowid WHERE a.publication_id=?1 AND a.evidence_rowid IN (SELECT value FROM json_each(?2)){conditions}"))?;
    let mut by_area = std::collections::BTreeMap::<String, Vec<(String, i64)>>::new();
    let mut within = std::collections::HashSet::new();
    for row in stmt.query_map(rusqlite::params![scope.publication_id, keys], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
    })? {
        let (area, rowid, stamp) = row?;
        within.insert(rowid);
        by_area.entry(area).or_default().push((stamp, rowid));
    }
    let mut chosen = std::collections::BTreeSet::new();
    let rows: Vec<(String, i64, Vec<i64>)> = by_area
        .into_iter()
        .map(|(area, mut members)| {
            // Newest first: message time, then the newer retained row.
            members.sort_unstable_by(|a, b| b.cmp(a));
            let count = members.len() as i64;
            let newest: Vec<i64> = members.iter().take(100).map(|(_, rowid)| *rowid).collect();
            chosen.extend(newest.iter().copied());
            (area, count, newest)
        })
        .collect();
    let mut detail = c.prepare("SELECT e.rowid,e.id,e.revision,e.source,coalesce(p.title,e.text),e.published_at,e.acquired_at,p.events,i.id FROM evidence e LEFT JOIN interpretations i ON i.id=(SELECT pi.interpretation_id FROM publication_interpretations pi WHERE pi.evidence_rowid=e.rowid AND pi.publication_id=?1) LEFT JOIN interpretation_previews p ON p.interpretation_id=i.id WHERE e.rowid IN (SELECT value FROM json_each(?2))")?;
    let mut details = std::collections::HashMap::<i64, Value>::new();
    for row in detail.query_map(rusqlite::params![scope.publication_id, serde_json::to_string(&chosen)?], |r| {
        let text: String = r.get(4)?;
        Ok((r.get::<_, i64>(0)?, json!({"id":r.get::<_,String>(1)?,"revision":r.get::<_,String>(2)?,"source":r.get::<_,String>(3)?,"title":text.chars().take(600).collect::<String>(),"published_at":r.get::<_,Option<String>>(5)?,"acquired_at":r.get::<_,String>(6)?,"events":crate::geographic_view::preview_events(r.get::<_,Option<String>>(7)?.as_deref()),"processed":r.get::<_,Option<String>>(8)?.is_some()})))
    })? {
        let (rowid, choice) = row?;
        details.insert(rowid, choice);
    }
    let mut features = vec![];
    let sql = if shapes {
        "SELECT name,display_json,level,bounds,iso2 FROM boundary_shapes WHERE id=?1"
    } else {
        "SELECT name,NULL,level,bounds,iso2 FROM boundary_shapes WHERE id=?1"
    };
    let mut shape = c.prepare(sql)?;
    for (area_id, count, newest) in rows {
        let (name, geometry, level, bounds, country_code): (
            String,
            Option<String>,
            String,
            String,
            String,
        ) = shape.query_row([&area_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        let geometry = geometry.map(|g| serde_json::from_str::<Value>(&g)).transpose()?;
        let choices = newest
            .iter()
            .map(|rowid| details.get(rowid).cloned().ok_or_else(crate::error::Error::internal))
            .collect::<Result<Vec<_>>>()?;
        features.push(json!({"type":"Feature","id":hash(area_id.as_bytes()),"geometry":geometry,"properties":{"area_id":area_id,"label":name,"level":level,"country_code":country_code,"bounds":serde_json::from_str::<Value>(&bounds)?,"message_count":count,"choices":choices}}));
    }
    Ok((json!({"type":"FeatureCollection","features":features}), false, within))
}

impl crate::store::Store {
    pub fn area_selection(&self, input: &Value) -> Result<Value> {
        let mut scope = Scope::parse(&input["scope"])?;
        let c = self.connect()?;
        Self::resolve_scope(&c, &mut scope)?;
        let (pred, mut args) = crate::store::predicate(&scope);
        args.push(SqlValue::Text(crate::model::required(input, "id")?.into()));
        args.push(SqlValue::Text(
            crate::model::required(input, "revision")?.into(),
        ));
        let mut stmt = c.prepare(&format!("SELECT DISTINCT a.area_id FROM reading_evidence e JOIN reading_message_areas a ON {} WHERE {pred} AND e.id=? AND e.revision=? ORDER BY a.area_id LIMIT 25", filter(&scope,true)))?;
        let ids = stmt
            .query_map(rusqlite::params_from_iter(args), |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(json!({"ids":ids}))
    }
}
