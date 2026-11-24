//! The map/feed scope query as it was before the one-pass rewrite (one SQL
//! statement per figure, reading documents parsed per map point and area
//! member, the site view reading each located message's reading), kept to
//! prove the rewrite returns identical results on a varied workspace.
use super::*;

const OLD_SITE_VIEW: &str = "DROP VIEW reading_message_sites; CREATE VIEW reading_message_sites AS
 SELECT pi.evidence_rowid,pi.publication_id,s.site_key,s.lon,s.lat,s.label,1 AS approximate,s.event_type
 FROM publication_interpretations pi JOIN interpretation_sites s ON s.interpretation_id=pi.interpretation_id
 UNION ALL
 SELECT e.rowid,m.publication_id,CAST('single' AS TEXT),CAST(coalesce(e.lon,i.lon) AS REAL),CAST(coalesce(e.lat,i.lat) AS REAL),
 CAST(CASE WHEN e.lon IS NOT NULL THEN json_extract(e.json,'$.location_precision') ELSE json_extract(i.json,'$.location_label') END AS TEXT),
 e.lon IS NULL,CAST(NULL AS TEXT)
 FROM membership m JOIN evidence e ON e.rowid=m.evidence_rowid
 LEFT JOIN interpretation_intervals pi ON pi.evidence_rowid=e.rowid AND pi.from_seq<=(SELECT seq FROM publication_order WHERE id=m.publication_id) AND (pi.until_seq IS NULL OR pi.until_seq>(SELECT seq FROM publication_order WHERE id=m.publication_id))
 LEFT JOIN interpretations i ON i.id=pi.interpretation_id
 WHERE e.lon IS NOT NULL OR (i.lon IS NOT NULL AND coalesce(json_array_length(i.json,'$.events'),0)=0);";

fn reference_sites(c: &Connection, scope: &Scope, pred: &str, vals: &[SqlValue], limit: usize) -> Result<Vec<Value>> {
    let filter = crate::geographic_view::site_filter(scope, true);
    let mut args = vals.to_vec();
    args.push(SqlValue::Text(scope.publication_id.clone().unwrap_or_default()));
    let mut stmt = c.prepare(&format!("WITH matched_sites AS (SELECT e.rowid AS evidence_rowid,s.site_key,s.lon,s.lat,min(s.label) AS label,max(s.approximate) AS approximate,CASE WHEN count(DISTINCT s.event_type)=1 THEN min(s.event_type) END AS event_type FROM reading_evidence e JOIN reading_message_sites s ON {filter} WHERE {pred} GROUP BY e.rowid,s.site_key ORDER BY e.rowid,s.site_key LIMIT {limit}) SELECT e.rowid,e.id,e.revision,e.source,coalesce(nullif(json_extract(i.json,'$.headline'),''),nullif(json_extract(i.json,'$.translation'),''),e.text),s.site_key,s.lon,s.lat,s.label,s.approximate,s.event_type,e.published_at,e.acquired_at,json_extract(i.json,'$.events'),i.id FROM matched_sites s JOIN evidence e ON e.rowid=s.evidence_rowid LEFT JOIN interpretations i ON i.id=(SELECT pi.interpretation_id FROM publication_interpretations pi WHERE pi.evidence_rowid=e.rowid AND pi.publication_id=?) ORDER BY e.rowid,s.site_key"))?;
    Ok(stmt.query_map(rusqlite::params_from_iter(args), |row| {
        let key: i64 = row.get(0)?;
        let site_key: String = row.get(5)?;
        let title: String = row.get(4)?;
        Ok(json!({"type":"Feature","id":format!("{key}:{}",crate::model::hash(site_key.as_bytes())),"geometry":{"type":"Point","coordinates":[row.get::<_,f64>(6)?,row.get::<_,f64>(7)?]},"properties":{"message_key":key.to_string(),"evidence_id":row.get::<_,String>(1)?,"revision":row.get::<_,String>(2)?,"source":row.get::<_,String>(3)?,"title":title.chars().take(400).collect::<String>(),"site_key":site_key,"location_label":row.get::<_,Option<String>>(8)?,"approximate":row.get::<_,bool>(9)?,"event_type":row.get::<_,Option<String>>(10)?,"published_at":row.get::<_,Option<String>>(11)?,"acquired_at":row.get::<_,String>(12)?,"events":crate::geographic_view::preview_events(row.get::<_,Option<String>>(13)?.as_deref()),"processed":row.get::<_,Option<String>>(14)?.is_some()}}))
    })?.collect::<std::result::Result<_,_>>()?)
}

fn reference_areas(c: &Connection, scope: &Scope, pred: &str, vals: &[SqlValue], shapes: bool) -> Result<(Value, bool)> {
    let filter = crate::area_view::filter(scope, true);
    let mut stmt=c.prepare(&format!("SELECT a.area_id,count(DISTINCT e.rowid) FROM reading_evidence e JOIN reading_message_areas a ON {filter} WHERE {pred} GROUP BY a.area_id ORDER BY a.area_id"))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(vals), |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut args = vals.to_vec();
    args.push(SqlValue::Text(serde_json::to_string(&rows.iter().map(|(id, _)| id).collect::<Vec<_>>())?));
    let mut member_stmt=c.prepare(&format!("WITH matched AS (SELECT DISTINCT a.area_id,e.rowid AS evidence_rowid,e.id,e.revision,e.source,coalesce(nullif(json_extract(i.json,'$.headline'),''),nullif(json_extract(i.json,'$.translation'),''),e.text) AS title,e.published_at,e.acquired_at,json_extract(i.json,'$.events') AS events,i.id AS reading FROM reading_evidence e JOIN reading_message_areas a ON {filter} LEFT JOIN interpretation_intervals pi ON pi.evidence_rowid=e.rowid AND pi.from_seq<=(SELECT seq FROM publication_order WHERE id=e.publication_id) AND (pi.until_seq IS NULL OR pi.until_seq>(SELECT seq FROM publication_order WHERE id=e.publication_id)) LEFT JOIN interpretations i ON i.id=pi.interpretation_id WHERE {pred} AND a.area_id IN (SELECT value FROM json_each(?))), ranked AS (SELECT *,row_number() OVER (PARTITION BY area_id ORDER BY coalesce(published_at,acquired_at) DESC,evidence_rowid DESC) AS n FROM matched) SELECT area_id,id,revision,source,title,published_at,acquired_at,events,reading FROM ranked WHERE n<=100 ORDER BY area_id,n"))?;
    let mut members: HashMap<String, Vec<Value>> = HashMap::new();
    for row in member_stmt.query_map(rusqlite::params_from_iter(args), |r| {
        let text: String = r.get(4)?;
        Ok((r.get::<_, String>(0)?, json!({"id":r.get::<_,String>(1)?,"revision":r.get::<_,String>(2)?,"source":r.get::<_,String>(3)?,"title":text.chars().take(600).collect::<String>(),"published_at":r.get::<_,Option<String>>(5)?,"acquired_at":r.get::<_,String>(6)?,"events":crate::geographic_view::preview_events(r.get::<_,Option<String>>(7)?.as_deref()),"processed":r.get::<_,Option<String>>(8)?.is_some()})))
    })? {
        let (area, choice) = row?;
        members.entry(area).or_default().push(choice);
    }
    let mut features = vec![];
    let sql = if shapes {
        "SELECT name,display_json,level,bounds,iso2 FROM boundary_shapes WHERE id=?1"
    } else {
        "SELECT name,NULL,level,bounds,iso2 FROM boundary_shapes WHERE id=?1"
    };
    let mut shape = c.prepare(sql)?;
    for (area_id, count) in rows {
        let (name, geometry, level, bounds, country_code): (String, Option<String>, String, String, String) =
            shape.query_row([&area_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
        let geometry = geometry.map(|g| serde_json::from_str::<Value>(&g)).transpose()?;
        let choices = members.remove(&area_id).unwrap_or_default();
        features.push(json!({"type":"Feature","id":hash(area_id.as_bytes()),"geometry":geometry,"properties":{"area_id":area_id,"label":name,"level":level,"country_code":country_code,"bounds":serde_json::from_str::<Value>(&bounds)?,"message_count":count,"choices":choices}}));
    }
    Ok((json!({"type":"FeatureCollection","features":features}), false))
}

fn reference_groups(c: &Connection, pubid: &str, pred: &str, vals: &[SqlValue]) -> Result<HashMap<i64, (String, usize)>> {
    use std::collections::BTreeSet;
    if !c.query_row("SELECT EXISTS(SELECT 1 FROM event_links)", [], |r| r.get::<_, bool>(0))? {
        return Ok(HashMap::new());
    }
    let seq: i64 = c.query_row("SELECT seq FROM publication_order WHERE id=?1", [pubid], |r| r.get(0))?;
    let links = c
        .prepare("SELECT l.a,l.b FROM event_links l JOIN publication_order o ON o.id=l.publication_id JOIN event_link_claims ca ON ca.id=l.a JOIN publication_interpretations pa ON pa.interpretation_id=ca.interpretation_id AND pa.publication_id=?1 JOIN event_link_claims cb ON cb.id=l.b JOIN publication_interpretations pb ON pb.interpretation_id=cb.interpretation_id AND pb.publication_id=?1 WHERE o.seq<=?2")?
        .query_map(params![pubid, seq], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if links.is_empty() {
        return Ok(HashMap::new());
    }
    let ids = links.iter().flat_map(|(a, b)| [a.clone(), b.clone()]).collect::<BTreeSet<_>>();
    let mut args = vals.to_vec();
    args.push(SqlValue::Text(string(&json!(ids))));
    let in_scope: HashMap<String, i64> = c
        .prepare(&format!("SELECT c.id,c.evidence_rowid FROM event_link_claims c JOIN reading_evidence e ON e.rowid=c.evidence_rowid WHERE {pred} AND c.id IN (SELECT value FROM json_each(?)) AND c.interpretation_id=json_extract(e.json,'$.interpretation.id')"))?
        .query_map(rusqlite::params_from_iter(args), |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let mut parent: HashMap<String, String> = HashMap::new();
    fn find(parent: &mut HashMap<String, String>, x: &str) -> String {
        let mut root = x.to_owned();
        while let Some(p) = parent.get(&root).filter(|p| **p != root) {
            root = p.clone();
        }
        let mut node = x.to_owned();
        while node != root {
            let next = parent.insert(node.clone(), root.clone()).unwrap_or_else(|| root.clone());
            node = next;
        }
        root
    }
    for (a, b) in &links {
        if !in_scope.contains_key(a) || !in_scope.contains_key(b) {
            continue;
        }
        parent.entry(a.clone()).or_insert_with(|| a.clone());
        parent.entry(b.clone()).or_insert_with(|| b.clone());
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            let (keep, other) = if ra < rb { (ra, rb) } else { (rb, ra) };
            parent.insert(other, keep);
        }
    }
    let claims = parent.keys().cloned().collect::<Vec<_>>();
    let mut groups: HashMap<String, BTreeSet<i64>> = HashMap::new();
    let mut member: Vec<(String, i64)> = vec![];
    for claim in claims {
        let root = find(&mut parent, &claim);
        let row = in_scope[&claim];
        groups.entry(root.clone()).or_default().insert(row);
        member.push((root, row));
    }
    let mut out: HashMap<i64, (String, usize)> = HashMap::new();
    for (root, row) in member {
        let reports = groups[&root].len();
        let id = format!("event:{}", &hash(root.as_bytes())[..16]);
        let better = out.get(&row).is_none_or(|(old, n)| reports > *n || (reports == *n && id < *old));
        if better {
            out.insert(row, (id, reports));
        }
    }
    Ok(out)
}

fn reference_query(c: &Connection, input: Query, rows_only: bool, area_shapes: bool) -> Result<Value> {
    let mut scope = Scope::parse(&input.scope)?;
    if Store::latest(c)?.is_none() && scope.publication_id.is_none() {
        return Err(Error::missing("No complete publication. Import evidence first."));
    }
    let pubid = Store::resolve_scope(c, &mut scope)?;
    let scope_revision = hash(string(&value(&scope)).as_bytes());
    let limit = input.limit.unwrap_or(100).clamp(1, 500);
    let (pred, vals) = predicate(&scope);
    let located_sql = if scope.include_unlocated { crate::geographic_view::has_site(&scope, true) } else { "1".into() };
    let timeline = if rows_only { Value::Null } else { crate::timeline::tests::reference(c, &scope, &pred, &vals)? };
    let (total, located, origins, unknown, unread, not_selected, media_only): (i64, i64, i64, i64, i64, i64, i64) =
        if rows_only {
            (0, 0, 0, 0, 0, 0, 0)
        } else {
            let unread = "e.kind='pending' AND length(trim(e.text))>0 AND coalesce(json_extract(e.json,'$.upstream_state'),'present')!='removed'";
            let selected = "e.source IN(SELECT value FROM json_each((SELECT json_extract(value,'$.eligible_source_ids') FROM metadata WHERE key='processing_policy')))";
            c.query_row(&format!("SELECT count(*),coalesce(sum({located_sql}),0),count(DISTINCT e.origin_group),coalesce(sum(e.origin_group IS NULL),0),coalesce(sum({unread} AND {selected}),0),coalesce(sum({unread} AND NOT {selected}),0),coalesce(sum(e.kind='pending' AND length(trim(e.text))=0 AND coalesce(json_extract(e.json,'$.upstream_state'),'present')!='removed'),0) FROM reading_evidence e WHERE {pred}"),rusqlite::params_from_iter(&vals),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)))?
        };
    let mut page_vals = vals.clone();
    let mut page_pred = pred.clone();
    let sort_time = if scope.time_basis == "acquisition_time" { "e.acquired_at" } else { "coalesce(e.published_at,e.acquired_at)" };
    if let Some(cursor) = input.cursor {
        let (generation, key) = cursor.split_once(':').ok_or_else(|| Error::invalid("Invalid page cursor"))?;
        if generation != scope_revision {
            return Err(Error::conflict("Page cursor belongs to a different scope/publication"));
        }
        let key = key.parse::<i64>().map_err(|_| Error::invalid("Invalid page cursor"))?;
        let mut anchor_vals = vals.clone();
        anchor_vals.push(SqlValue::Integer(key));
        let anchor: Option<String> = c
            .query_row(&format!("SELECT {sort_time} FROM reading_evidence e WHERE {pred} AND e.rowid=?"), rusqlite::params_from_iter(anchor_vals), |r| r.get(0))
            .optional()?;
        let anchor = anchor.ok_or_else(|| Error::invalid("Page cursor is outside its scope"))?;
        page_pred.push_str(&format!(" AND ({sort_time},e.rowid)<(?,?)"));
        page_vals.push(SqlValue::Text(anchor));
        page_vals.push(SqlValue::Integer(key));
    }
    page_vals.push(SqlValue::Integer(limit as i64 + 1));
    page_vals.push(SqlValue::Text(pubid.clone()));
    let mut stmt = c.prepare(&format!(
        "WITH page AS MATERIALIZED (SELECT e.rowid,({located_sql}) AS located FROM reading_evidence e WHERE {page_pred} ORDER BY {sort_time} DESC,e.rowid DESC LIMIT ?) SELECT e.rowid,e.json,page.located FROM page JOIN reading_evidence e ON e.rowid=page.rowid WHERE e.publication_id=? ORDER BY {sort_time} DESC,e.rowid DESC"
    ))?;
    let mut rows: Vec<(i64, String, bool)> = stmt
        .query_map(rusqlite::params_from_iter(page_vals), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let more = rows.len() > limit;
    if more {
        rows.pop();
    }
    let cursor = if more { rows.last().map(|(id, _, _)| format!("{scope_revision}:{id}")) } else { None };
    let mut items: Vec<Value> = rows
        .into_iter()
        .map(|(key, s, located)| {
            let mut evidence: Value = serde_json::from_str(&s)?;
            evidence["message_key"] = json!(key.to_string());
            if !located {
                evidence["geometry"] = Value::Null;
            }
            Ok(evidence)
        })
        .collect::<Result<_>>()?;
    let related_keys: Vec<&str> = items.iter().filter_map(|e| e["interpretation"]["related_key"].as_str()).collect();
    let mut group_counts = HashMap::new();
    if !related_keys.is_empty() {
        let mut group_vals = vals.clone();
        group_vals.push(SqlValue::Text(string(&json!(related_keys))));
        let mut stmt=c.prepare(&format!("SELECT e.related_key,count(*) FROM reading_evidence e WHERE {pred} AND e.related_key IN (SELECT value FROM json_each(?)) GROUP BY e.related_key"))?;
        for row in stmt.query_map(rusqlite::params_from_iter(group_vals), |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (key, count) = row?;
            group_counts.insert(key, count);
        }
    }
    for e in &mut items {
        if let Some(key) = e["interpretation"]["related_key"].as_str() {
            let count = group_counts.get(key).copied().unwrap_or(1) - 1;
            e["interpretation"]["related_count"] = json!(count);
        }
    }
    let mut page_site_vals = vals.clone();
    page_site_vals.push(SqlValue::Text(string(&json!(items.iter().map(|e| e["message_key"].as_str().unwrap().parse::<i64>().unwrap()).collect::<Vec<_>>()))));
    let page_sites = reference_sites(c, &scope, &format!("{pred} AND e.rowid IN (SELECT value FROM json_each(?))"), &page_site_vals, limit * 25)?;
    for item in &mut items {
        crate::geographic_view::attach_sites(item, &page_sites);
    }
    crate::area_view::attach(c, &scope, &mut items)?;
    let event_groups = reference_groups(c, &pubid, &pred, &vals)?;
    crate::event_link_store::attach_event_links(&event_groups, &mut items);
    if rows_only {
        return Ok(json!({"scope_revision":scope_revision,"scope":scope,"publication_id":pubid,"generation":input.generation,"loaded":items.len(),"items":items,"next_cursor":cursor}));
    }
    let (map_areas, areas_truncated) = reference_areas(c, &scope, &pred, &vals, area_shapes)?;
    let mut features = reference_sites(c, &scope, &pred, &vals, MAX_MAP + 1)?;
    for f in &mut features {
        if let Some((id, reports)) = f["properties"]["message_key"].as_str().and_then(|k| k.parse::<i64>().ok()).and_then(|k| event_groups.get(&k)) {
            f["properties"]["event_group"] = json!(id);
            f["properties"]["event_reports"] = json!(reports);
        }
    }
    let map_truncated = features.len() > MAX_MAP;
    features.truncate(MAX_MAP);
    let mut cov = coverage();
    if areas_truncated {
        cov.push("Some area shapes exceed the map display cap; message counts and feed membership remain complete.".into());
    }
    if map_truncated {
        cov.push(format!("Map shows the first {MAX_MAP} sites; message list/count membership remains complete."));
    }
    Ok(json!({"scope_revision":scope_revision,"scope":scope,"publication_id":pubid,"generation":input.generation,"total":total,"located":located,"unlocated":total-located,"unread":unread,"not_selected":not_selected,"media_only":media_only,"origin_groups":origins,"unknown_origins":unknown,"loaded":items.len(),"items":items,"map_loaded":features.len(),"map_truncated":map_truncated,"map_areas":map_areas,"areas_truncated":areas_truncated,"map":{"type":"FeatureCollection","features":features},"next_cursor":cursor,"coverage":cov,"timeline":timeline}))
}

fn workspace() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let store = Store::open(Config::local(root, tmp.path().join("workspace"), "127.0.0.1:8799".parse().unwrap())).unwrap();
    (tmp, store)
}

const KEY_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const KEY_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// A fictional message with varied time, place, origin, text and upstream state.
fn add_message(c: &Connection, n: i64) {
    let published = match n {
        _ if n % 11 == 0 => None,
        _ if n % 17 == 0 => Some("not a time".to_owned()),
        5 => Some("2024-06-01T00:00:00Z".to_owned()),
        _ => Some(format!("2026-0{}-{:02}T{:02}:{:02}:00.{:03}Z", 1 + n % 3, 1 + n % 27, n % 24, n % 60, n * 7 % 1000)),
    };
    let acquired = format!("2026-03-{:02}T00:00:{:02}Z", 1 + n % 20, n % 60);
    let place: Option<[f64; 2]> = match n % 7 {
        0 => Some([35.0, 35.0]),
        3 => Some([175.0, 5.0]),
        _ => None,
    };
    let text = if n % 13 == 0 { String::new() } else { format!("Fictional message {n}{}", if n % 3 == 0 { " about a drone" } else { "" }) };
    let upstream = if n % 19 == 0 { json!("removed") } else { Value::Null };
    let document = json!({"id":format!("fiction:{n}"),"revision":"v1","text":text,"geometry":place.map(|p| json!({"type":"Point","coordinates":p})),"location_precision":if place.is_some() {"source_point"} else {"unresolved"},"upstream_state":upstream,"fictional":true});
    c.execute(
        "INSERT INTO evidence VALUES(?1,?2,'v1',?3,?4,?5,?6,?7,?8,?9,'obj',?10)",
        params![n, format!("fiction:{n}"), format!("s{}", 1 + n % 3), acquired, published, place.map(|p| p[0]), place.map(|p| p[1]), (n % 4 != 0).then(|| format!("origin:{}", n % 6)), text, string(&document)],
    )
    .unwrap();
}

/// A reading of message `n`: events with supported sites and areas, legacy
/// readings without events, and located readings without events.
fn add_reading(c: &Connection, n: i64, version: &str) -> String {
    let id = format!("reading:{n}:{version}");
    let kind = ["event", "statement", "promotion", "other"][(n % 4) as usize];
    let state = if n % 6 == 1 { "denied" } else { "reported" };
    let kind_of = if n % 2 == 0 { "strike" } else { "armed_attack" };
    let site = |lon: f64, lat: f64, area: Option<&str>| json!({"role":"event","occurrence_eligible":true,"selected_candidate_id":format!("place:{lon}:{lat}"),"candidates":[{"id":format!("place:{lon}:{lat}"),"name":format!("Place {lon}"),"country_code":"AA"}],"geometry":{"type":"Point","coordinates":[lon,lat]},"area":area.map(|a| json!({"id":a})),"mention":"somewhere"});
    let events = match n % 10 {
        3 => None,
        7 => Some(json!([])),
        4 => Some(json!([{"type":kind_of,"state":state,"locations":[site(36.0,36.0,Some("area:a")),site(0.0,0.0,Some("area:b"))]},{"type":"explosion","state":"reported","locations":[site(36.0,36.0,None)]}])),
        8 => Some(json!([7, {"type":"strike","state":"reported","locations":[site(178.0,5.0,Some("area:c"))]}])),
        _ => Some(json!([{"type":kind_of,"state":state,"locations":[site(31.0 + (n % 5) as f64,32.0,(n % 3 == 0).then_some("area:a"))]}])),
    };
    let related = match n % 8 {
        0 => Some(KEY_A),
        1 => Some(KEY_B),
        _ => None,
    };
    let headline = if n % 9 == 0 { json!("") } else { json!(format!("Headline {n} {version} {}", "é".repeat((n % 5) as usize * 150))) };
    let translation = if n % 2 == 0 { json!(format!("Translated drone report {n}")) } else { Value::Null };
    let mut reading = json!({"id":id,"kind":kind,"headline":headline,"translation":translation,"related_key":related,"location_label":format!("Label {n}"),"claim_status":"reported","claims":[],"model":"fictional"});
    if let Some(events) = events {
        reading["events"] = events;
    }
    let located = n % 10 == 7 || n % 10 == 3;
    c.execute(
        "INSERT INTO interpretations VALUES(?1,?2,?3,'2026-03-01T00:00:00Z',?4,?5,?6,?7,?8,'obj')",
        params![id, n, format!("recipe:{version}"), kind, located.then_some(20.0), located.then_some(-5.0), related, string(&reading)],
    )
    .unwrap();
    id
}

fn publish(c: &Connection, id: &str) {
    c.execute("INSERT INTO publications VALUES(?1,?1,'2026-03-01T00:00:00Z','complete',NULL)", [id]).unwrap();
}

fn seed(c: &Connection) {
    c.execute_batch(r#"
        INSERT INTO sources VALUES('s1','{}'),('s2','{}'),('s3','{}');
        INSERT INTO objects VALUES('obj',1,'application/json');
        INSERT INTO boundary_shapes VALUES
         ('area:a','e1','AA','AAA','ADM1','a','Area A','{"type":"Polygon","coordinates":[[[30,30],[40,30],[40,40],[30,40],[30,30]]]}','{"type":"Polygon","coordinates":[[[30,30],[40,30],[40,40],[30,40],[30,30]]]}','[30,30,40,40]','{"boundaryYearRepresented":"2020"}','obj'),
         ('area:b','e1','BB','BBB','ADM0','b','Area B','{"type":"Polygon","coordinates":[[[-10,-10],[10,-10],[10,10],[-10,10],[-10,-10]]]}','{"type":"Polygon","coordinates":[[[-10,-10],[10,-10],[10,10],[-10,10],[-10,-10]]]}','[-10,-10,10,10]','{"boundaryYearRepresented":"2021"}','obj'),
         ('area:c','e1','CC','CCC','ADM1','c','Area C','{"type":"Polygon","coordinates":[[[170,0],[179,0],[179,10],[170,10],[170,0]]]}','{"type":"Polygon","coordinates":[[[170,0],[179,0],[179,10],[170,10],[170,0]]]}','[170,0,179,10]','{"boundaryYearRepresented":"2022"}','obj');
        INSERT INTO event_link_attempts VALUES('attempt','profile','complete','2026-03-01T00:00:00Z',0,'obj',NULL,NULL,NULL,NULL);
    "#).unwrap();
    // First publication: messages 1-70; readings for four in five.
    publish(c, "publication:one");
    let mut first = HashMap::new();
    for n in 1..=70 {
        add_message(c, n);
        c.execute("INSERT INTO membership VALUES('publication:one',?1)", [n]).unwrap();
        if n % 5 != 0 {
            let reading = add_reading(c, n, "v1");
            c.execute("INSERT INTO publication_interpretations VALUES('publication:one',?1,?2)", params![n, reading]).unwrap();
            first.insert(n, reading);
        }
    }
    let claim = |n: i64, reading: &str| {
        let id = format!("claim:{n}:{reading}");
        c.execute("INSERT INTO event_link_claims VALUES(?1,?2,?3,'place','strike',0,'{}')", params![id, n, reading]).unwrap();
        id
    };
    let link = |a: &str, b: &str, publication: &str| {
        c.execute("INSERT INTO event_links VALUES(?1,?2,?3,'attempt',?4,'same_event','{}')", params![format!("link:{a}:{b}"), a, b, publication]).unwrap();
    };
    let (c1, c2, c4) = (claim(1, &first[&1]), claim(2, &first[&2]), claim(4, &first[&4]));
    link(&c1, &c2, "publication:one");
    // Second publication: new messages, removals and replaced readings.
    publish(c, "publication:two");
    for n in 71..=90 {
        add_message(c, n);
        c.execute("INSERT INTO membership VALUES('publication:two',?1)", [n]).unwrap();
        if n % 5 != 0 {
            let reading = add_reading(c, n, "v1");
            c.execute("INSERT INTO publication_interpretations VALUES('publication:two',?1,?2)", params![n, reading]).unwrap();
        }
    }
    for n in [3, 9, 33] {
        c.execute("DELETE FROM membership WHERE publication_id='publication:two' AND evidence_rowid=?1", [n]).unwrap();
    }
    let mut second = HashMap::new();
    for n in [2, 4, 12, 24, 27, 40] {
        let reading = add_reading(c, n, "v2");
        c.execute("INSERT INTO publication_interpretations VALUES('publication:two',?1,?2)", params![n, reading]).unwrap();
        second.insert(n, reading);
    }
    let (c2b, c6, c8, c71) = (claim(2, &second[&2]), claim(6, &first[&6]), claim(8, &first[&8]), claim(71, "reading:71:v1"));
    link(&c2b, &c6, "publication:two");
    link(&c6, &c8, "publication:two");
    link(&c1, &c71, "publication:two");
    // A claim of a replaced reading: visible at the first publication only.
    link(&c4, &c8, "publication:two");
    c.execute_batch(r#"
        INSERT INTO metadata VALUES('current_publication','publication:two') ON CONFLICT(key) DO UPDATE SET value=excluded.value;
        INSERT INTO metadata VALUES('processing_policy','{"eligible_source_ids":["s1","s2"]}') ON CONFLICT(key) DO UPDATE SET value=excluded.value;
    "#).unwrap();
}

fn scopes() -> Vec<Value> {
    vec![
        json!({}),
        json!({"content_view":"events"}),
        json!({"content_view":"signals"}),
        json!({"content_view":"events","event_type":"strike"}),
        json!({"event_type":"armed_attack"}),
        json!({"event_type":"explosion","bbox":[30.0,30.0,40.0,40.0]}),
        json!({"include_unlocated":false}),
        json!({"bbox":[25.0,25.0,45.0,45.0]}),
        json!({"bbox":[25.0,25.0,45.0,45.0],"include_unlocated":false}),
        json!({"bbox":[160.0,-5.0,-170.0,15.0]}),
        json!({"bbox":[-5.0,-5.0,5.0,5.0],"include_unlocated":false}),
        json!({"source_ids":["s1"]}),
        json!({"source_ids":["s2","s3"],"include_unlocated":false}),
        json!({"from":"2026-02-01T00:00:00Z","to":"2026-03-15T00:00:00Z"}),
        json!({"time_basis":"acquisition_time"}),
        json!({"time_basis":"acquisition_time","from":"2026-03-05T00:00:00Z"}),
        json!({"query":"drone"}),
        json!({"related_key":KEY_A}),
        json!({"publication_id":"publication:one"}),
        json!({"publication_id":"publication:one","content_view":"events","bbox":[25.0,25.0,45.0,45.0]}),
    ]
}

/// The first page, then every continuation page of a scope.
fn pages(c: &Connection, scope: &Value, limit: usize, query: fn(&Connection, Query, bool, bool) -> Result<Value>) -> Vec<Value> {
    let first = query(c, Query { scope: scope.clone(), generation: 3, cursor: None, limit: Some(limit) }, false, true).unwrap();
    let preview = query(c, Query { scope: scope.clone(), generation: 3, cursor: None, limit: Some(limit) }, false, false).unwrap();
    let mut out = vec![first.clone(), preview];
    let mut cursor = first["next_cursor"].as_str().map(str::to_owned);
    while let Some(next) = cursor {
        let page = query(c, Query { scope: first["scope"].clone(), generation: 3, cursor: Some(next), limit: Some(limit) }, true, true).unwrap();
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        out.push(page);
    }
    out
}

/// Every response of every scope, new and reference; returns the default scope's first page.
fn assert_same(c: &Connection, limit: usize) -> Value {
    let current: Vec<Vec<Value>> = scopes().iter().map(|s| pages(c, s, limit, query_conn)).collect();
    c.execute_batch(OLD_SITE_VIEW).unwrap();
    let reference: Vec<Vec<Value>> = scopes().iter().map(|s| pages(c, s, limit, reference_query)).collect();
    c.execute_batch("DROP VIEW reading_message_sites;").unwrap();
    c.execute_batch(include_str!("../schema.sql")).unwrap();
    for ((scope, new), old) in scopes().iter().zip(&current).zip(&reference) {
        assert_eq!(new.len(), old.len(), "page count for {scope}");
        for (n, (a, b)) in new.iter().zip(old).enumerate() {
            assert_eq!(a, b, "{scope} response {n}");
        }
    }
    current[0][0].clone()
}

#[test]
fn one_pass_scope_query_equals_the_statement_per_figure_query() {
    let (_tmp, store) = workspace();
    let c = store.connect().unwrap();
    seed(&c);
    let all = assert_same(&c, 7);
    // The fixture exercises what it claims to: located, unread, undated and grouped rows.
    assert!(all["located"].as_i64().unwrap() > 10 && all["unlocated"].as_i64().unwrap() > 10);
    assert_eq!(all["map_areas"]["features"].as_array().unwrap().len(), 3);
    assert!(all["map"]["features"].as_array().unwrap().iter().any(|f| f["properties"]["event_group"].is_string()));
    assert!(all["timeline"]["undated"].as_i64().unwrap() > 0);
    assert!(all["unread"].as_i64().unwrap() > 0 && all["not_selected"].as_i64().unwrap() > 0 && all["media_only"].as_i64().unwrap() > 0);
    assert!(all["next_cursor"].is_string());
    assert_same(&c, 500);
    // A JSON null among eligible sources makes `source IN (...)` NULL for other
    // sources: such messages count as neither unread nor not selected.
    c.execute("UPDATE metadata SET value='{\"eligible_source_ids\":[\"s1\",null]}' WHERE key='processing_policy'", []).unwrap();
    let all = assert_same(&c, 7);
    assert!(all["unread"].as_i64().unwrap() > 0 && all["not_selected"] == 0);
    c.execute("DELETE FROM metadata WHERE key='processing_policy'", []).unwrap();
    let all = assert_same(&c, 7);
    assert!(all["unread"] == 0 && all["not_selected"].as_i64().unwrap() > 0);
}

#[test]
fn page_locations_survive_a_truncated_map() {
    let (_tmp, store) = workspace();
    let c = store.connect().unwrap();
    seed(&c);
    publish(&c, "publication:three");
    let tx = c.unchecked_transaction().unwrap();
    for n in 1000..1000 + MAX_MAP as i64 + 40 {
        add_message(&tx, n);
        tx.execute("UPDATE evidence SET lon=?2,lat=?3 WHERE rowid=?1", params![n, (n % 300) as f64 / 10.0, 10.0]).unwrap();
        tx.execute("INSERT INTO membership VALUES('publication:three',?1)", [n]).unwrap();
    }
    tx.commit().unwrap();
    c.execute("UPDATE metadata SET value='publication:three' WHERE key='current_publication'", []).unwrap();
    // Located-only membership, and all messages: located then cannot come from
    // the truncated map and is read for the whole scope.
    let scopes = [json!({"include_unlocated":false}), json!({})];
    let run = |query: fn(&Connection, Query, bool, bool) -> Result<Value>| {
        scopes
            .iter()
            .map(|scope| {
                let request = |cursor: Option<String>| Query { scope: scope.clone(), generation: 1, cursor, limit: Some(50) };
                let first = query(&c, request(None), false, true).unwrap();
                let cursor = first["next_cursor"].as_str().map(str::to_owned);
                let next = query(&c, Query { scope: first["scope"].clone(), ..request(cursor) }, true, true).unwrap();
                (first, next)
            })
            .collect::<Vec<_>>()
    };
    let new = run(query_conn);
    for (first, _) in &new {
        assert_eq!(first["map_truncated"], true);
        assert!(first["located"].as_i64().unwrap() > MAX_MAP as i64);
    }
    let (located_only, _) = &new[0];
    assert!(located_only["items"].as_array().unwrap().iter().all(|i| !i["map_locations"].as_array().unwrap().is_empty()));
    c.execute_batch(OLD_SITE_VIEW).unwrap();
    assert_eq!(new, run(reference_query));
}

#[test]
fn reading_previews_match_the_reading_documents() {
    let (_tmp, store) = workspace();
    let c = store.connect().unwrap();
    c.execute_batch("INSERT INTO sources VALUES('s1','{}'),('s2','{}'); INSERT INTO objects VALUES('obj',1,'application/json');").unwrap();
    add_message(&c, 1);
    let readings = [
        json!({"headline":"Shown","translation":"Hidden","events":[{"type":"strike","state":"reported"},{"type":"fire"},7,null,[1],{"state":{"nested":true}}]}),
        json!({"headline":"","translation":"Translated","events":[]}),
        json!({"headline":null,"translation":"","events":{"count":1}}),
        json!({"translation":"Only translation","events":"[1,2]","location_label":7}),
        json!({"headline":"Unicode é ✓ 😀","location_label":"Somewhere"}),
        json!({"headline":5,"events":[{"type":"strike","state":"reported","type":"fire"}]}),
    ];
    for (n, reading) in readings.iter().enumerate() {
        let mut reading = reading.clone();
        reading["id"] = json!(format!("reading:{n}"));
        c.execute("INSERT INTO interpretations VALUES(?1,1,?1,'2026-03-01T00:00:00Z','event',NULL,NULL,NULL,?2,'obj')", params![format!("reading:{n}"), string(&reading)]).unwrap();
    }
    // Re-read readings stored before the projection existed.
    c.execute("DELETE FROM interpretation_previews WHERE interpretation_id='reading:1'", []).unwrap();
    crate::geographic_view::backfill(&c).unwrap();
    let mut stmt = c.prepare("SELECT p.title IS coalesce(nullif(json_extract(i.json,'$.headline'),''),nullif(json_extract(i.json,'$.translation'),'')),typeof(p.title)=typeof(coalesce(nullif(json_extract(i.json,'$.headline'),''),nullif(json_extract(i.json,'$.translation'),''))),p.location_label IS json_extract(i.json,'$.location_label'),typeof(p.location_label)=typeof(json_extract(i.json,'$.location_label')),p.event_count=coalesce(json_array_length(i.json,'$.events'),0),p.events,json_extract(i.json,'$.events'),json_type(i.json,'$.events') FROM interpretations i JOIN interpretation_previews p ON p.interpretation_id=i.id ORDER BY i.id").unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, bool>(0)?, r.get::<_, bool>(1)?, r.get::<_, bool>(2)?, r.get::<_, bool>(3)?, r.get::<_, bool>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?, r.get::<_, Option<String>>(7)?)))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), readings.len());
    for (title, title_type, label, label_type, count, preview, events, kind) in rows {
        assert!(title && title_type && label && label_type && count);
        // A non-array events value previews as no events, as before.
        let expected = if kind.as_deref() == Some("array") { crate::geographic_view::preview_events(events.as_deref()) } else { vec![] };
        assert_eq!(crate::geographic_view::preview_events(preview.as_deref()), expected);
    }
    // A changed reading document is re-projected.
    c.execute("UPDATE interpretations SET json=json_set(json,'$.headline','Changed') WHERE id='reading:1'", []).unwrap();
    let title: String = c.query_row("SELECT title FROM interpretation_previews WHERE interpretation_id='reading:1'", [], |r| r.get(0)).unwrap();
    assert_eq!(title, "Changed");
}
