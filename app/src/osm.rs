//! Selected Geofabrik snapshots, streamed into disk-backed supporting geometry.
//! No Nominatim dependency, inferred centroids or source-observation claims.
use crate::{
    area_geometry::AreaGeometry,
    error::{Error, Result},
    model::string,
};
use osmpbf::{BlobDecode, BlobReader, Element};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
/// v2 adds named places (settlements, districts, islands, squares, peaks) and
/// border crossings to the v1 facilities. v1 editions stay readable.
pub const MAPPING: &str = "relic.osm-features.v2";
pub const MAPPINGS: [&str; 2] = ["relic.osm-facilities.v1", MAPPING];
/// Classes that name a place rather than a facility.
pub const PLACE_CLASSES: [&str; 6] = ["city", "village", "district", "island", "square", "mountain"];
/// Malformed-data guard: OSM caps ways at 2,000 nodes, so a larger member
/// list means a corrupt file, not a big region.
const MAX_REFS: usize = 100_000;
/// Output index size relative to the extract; nothing else is staged on disk.
const BUILD_FACTOR: u64 = 1;
fn invalid() -> Error {
    Error::invalid("OSM snapshot is malformed, incomplete or exceeds an import bound")
}
fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::cancelled())
    } else {
        Ok(())
    }
}
pub fn key(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn classes(tags: &BTreeMap<String, String>) -> Vec<&'static str> {
    let mut result = vec![];
    for (tag, values, class) in [
        ("aeroway", &["aerodrome", "helipad"][..], "airport"),
        ("harbour", &["yes"][..], "port"),
        ("landuse", &["port"][..], "port"),
        ("industrial", &["port"][..], "port"),
        ("seamark:type", &["harbour"][..], "port"),
        ("power", &["plant", "substation"][..], "energy"),
        ("landuse", &["military"][..], "military"),
        (
            "military",
            &["base", "airfield", "naval_base", "barracks"][..],
            "military",
        ),
        ("amenity", &["hospital"][..], "hospital"),
        ("railway", &["station", "yard"][..], "rail"),
        ("waterway", &["dam"][..], "dam"),
        (
            "man_made",
            &["water_works", "wastewater_plant"][..],
            "water",
        ),
        ("landuse", &["industrial"][..], "industrial"),
        ("barrier", &["border_control"][..], "border_crossing"),
        ("place", &["city", "town"][..], "city"),
        ("place", &["village", "hamlet"][..], "village"),
        ("place", &["suburb", "quarter", "neighbourhood"][..], "district"),
        ("place", &["island", "islet"][..], "island"),
        ("place", &["square"][..], "square"),
        ("natural", &["peak"][..], "mountain"),
    ] {
        if tags.get(tag).is_some_and(|v| values.contains(&v.as_str())) {
            result.push(class);
        }
    }
    result.sort_unstable();
    result.dedup();
    result
}
fn language_name_tag(tag: &str) -> bool {
    let Some((kind, language)) = tag.split_once(':') else {
        return false;
    };
    if !["name", "official_name", "alt_name"].contains(&kind) || language.contains(':') {
        return false;
    }
    let mut pieces = language.split('-');
    let primary = pieces.next().unwrap_or("");
    (2..=3).contains(&primary.len())
        && primary.bytes().all(|b| b.is_ascii_alphabetic())
        && pieces
            .all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}
fn aliases(tags: &BTreeMap<String, String>) -> Vec<(String, String, String)> {
    let mut result = vec![];
    for (tag, text) in tags {
        if [
            "name",
            "official_name",
            "short_name",
            "alt_name",
            "old_name",
            "iata",
            "icao",
            "ref",
        ]
        .contains(&tag.as_str())
            || language_name_tag(tag)
        {
            for name in text
                .split(';')
                .map(str::trim)
                .filter(|n| !n.is_empty() && n.len() <= 1024)
            {
                result.push((
                    tag.clone(),
                    tag.split_once(':').map(|(_, l)| l).unwrap_or("").into(),
                    name.into(),
                ));
            }
        }
    }
    result
}
fn area_way(tags: &BTreeMap<String, String>) -> bool {
    if tags.get("area").is_some_and(|v| v == "no") {
        return false;
    }
    tags.get("area").is_some_and(|v| v == "yes")
        || ["building", "landuse", "amenity", "military", "place"]
            .iter()
            .any(|k| tags.contains_key(*k))
        || tags
            .get("aeroway")
            .is_some_and(|v| ["aerodrome", "helipad"].contains(&v.as_str()))
        || tags
            .get("man_made")
            .is_some_and(|v| ["water_works", "wastewater_plant"].contains(&v.as_str()))
        || tags
            .get("power")
            .is_some_and(|v| ["plant", "substation"].contains(&v.as_str()))
}
fn tags<'a>(it: impl Iterator<Item = (&'a str, &'a str)>) -> Result<BTreeMap<String, String>> {
    let mut tags = BTreeMap::new();
    for (k, v) in it {
        if tags.len() >= 1024
            || k.len() > 1024
            || v.len() > 65536
            || tags.insert(k.into(), v.into()).is_some()
        {
            return Err(invalid());
        }
    }
    Ok(tags)
}
/// One selected element, in file position order.
struct Tagged {
    pos: (usize, usize),
    kind: &'static str,
    id: i64,
    version: Option<i32>,
    time: Option<i64>,
    tags: BTreeMap<String, String>,
    refs: Value,
    point: Option<[f64; 2]>,
}
fn valid_identity(id: i64, version: Option<i32>) -> Result<()> {
    if id <= 0 || version.is_some_and(|v| v <= 0) {
        return Err(invalid());
    }
    Ok(())
}
fn selected(tags: &BTreeMap<String, String>) -> bool {
    !classes(tags).is_empty() && !aliases(tags).is_empty()
}
/// Decode every data blob in parallel; results keep their blob position.
fn scan<T: Send>(
    pbf: &Path,
    cancel: &AtomicBool,
    per_block: impl Fn(&osmpbf::PrimitiveBlock) -> Result<T> + Sync + Send,
) -> Result<(Value, Vec<(usize, T)>)> {
    use rayon::prelude::*;
    enum Part<T> {
        Header(Value),
        Data(T),
    }
    let parts = BlobReader::from_path(pbf)
        .map_err(|_| invalid())?
        .enumerate()
        .par_bridge()
        .map(|(i, blob)| -> Result<(usize, Part<T>)> {
            check(cancel)?;
            match blob.map_err(|_| invalid())?.decode().map_err(|_| invalid())? {
                BlobDecode::OsmHeader(h) => {
                    if i != 0
                        || !h.required_features().iter().any(|f| f == "OsmSchema-V0.6")
                        || h.required_features()
                            .iter()
                            .any(|f| !["OsmSchema-V0.6", "DenseNodes"].contains(&f.as_str()))
                    {
                        return Err(invalid());
                    }
                    Ok((i, Part::Header(json!({"required_features":h.required_features(),"optional_features":h.optional_features(),"writing_program":h.writing_program(),"source":h.source(),"replication_timestamp":h.osmosis_replication_timestamp(),"replication_sequence":h.osmosis_replication_sequence_number(),"replication_url":h.osmosis_replication_base_url(),"bounds":h.bbox().map(|b|[b.left,b.bottom,b.right,b.top])}))))
                }
                BlobDecode::OsmData(block) => {
                    if i == 0 || !block.validate_snapshot() {
                        return Err(invalid());
                    }
                    Ok((i, Part::Data(per_block(&block)?)))
                }
                BlobDecode::Unknown(_) => Err(invalid()),
            }
        })
        .collect::<Result<Vec<_>>>()?;
    let mut header = None;
    let mut data = vec![];
    for (i, part) in parts {
        match part {
            Part::Header(h) => header = Some(h),
            Part::Data(d) => data.push((i, d)),
        }
    }
    data.sort_unstable_by_key(|d| d.0);
    Ok((header.ok_or_else(invalid)?, data))
}
fn points(nodes: &HashMap<i64, [f64; 2]>, refs: &[i64]) -> Option<Vec<[f64; 2]>> {
    if refs.len() > MAX_REFS {
        return None;
    }
    refs.iter().map(|id| nodes.get(id).copied()).collect()
}
fn rings(paths: Vec<Vec<i64>>, cancel: &AtomicBool) -> Result<Option<Vec<Vec<i64>>>> {
    check(cancel)?;
    use std::collections::{BTreeMap, BTreeSet};
    // Closed rings never participate in joining open paths, regardless of
    // relation member order (two valid shells may touch at one vertex).
    let (closed, paths): (Vec<_>, Vec<_>) = paths
        .into_iter()
        .partition(|p| p.len() >= 4 && p.first() == p.last());
    let mut endpoints = BTreeMap::<i64, BTreeSet<usize>>::new();
    for (i, path) in paths.iter().enumerate() {
        check(cancel)?;
        if path.len() < 2 {
            return Ok(None);
        }
        for end in [path[0], *path.last().unwrap()] {
            endpoints.entry(end).or_default().insert(i);
        }
    }
    let mut paths = paths.into_iter().map(Some).collect::<Vec<_>>();
    let mut result = closed;
    let take = |i: usize,
                paths: &mut Vec<Option<Vec<i64>>>,
                endpoints: &mut BTreeMap<i64, BTreeSet<usize>>| {
        let path = paths[i].take().unwrap();
        for end in [path[0], *path.last().unwrap()] {
            if let Some(ids) = endpoints.get_mut(&end) {
                ids.remove(&i);
            }
        }
        path
    };
    for i in 0..paths.len() {
        check(cancel)?;
        if paths[i].is_none() {
            continue;
        }
        let mut ring = take(i, &mut paths, &mut endpoints);
        while ring.first() != ring.last() {
            check(cancel)?;
            let end = *ring.last().unwrap();
            let Some(ids) = endpoints.get(&end).filter(|ids| ids.len() == 1) else {
                return Ok(None);
            };
            let j = *ids.first().unwrap();
            let mut next = take(j, &mut paths, &mut endpoints);
            if next.last() == Some(&end) {
                next.reverse();
            }
            ring.extend_from_slice(&next[1..]);
            if ring.len() > MAX_REFS {
                return Ok(None);
            }
        }
        if ring.len() < 4 {
            return Ok(None);
        }
        result.push(ring);
    }
    Ok(Some(result))
}
fn relation_geometry(
    nodes: &HashMap<i64, [f64; 2]>,
    ways: &HashMap<i64, Vec<i64>>,
    refs: &Value,
    tags: &BTreeMap<String, String>,
    cancel: &AtomicBool,
) -> Result<(Option<Value>, &'static str)> {
    if tags.get("type").is_none_or(|t| t != "multipolygon") {
        return Ok((None, "unsupported_relation"));
    }
    let mut outer = vec![];
    let mut inner = vec![];
    let mut total_refs = 0;
    for r in refs.as_array().ok_or_else(invalid)? {
        check(cancel)?;
        if r["type"] != "way" || !["outer", "inner", ""].iter().any(|x| r["role"] == *x) {
            return Ok((None, "unsupported_member"));
        }
        let Some(path) = ways.get(&r["id"].as_i64().ok_or_else(invalid)?) else {
            return Ok((None, "missing_member"));
        };
        total_refs += path.len();
        if total_refs > MAX_REFS {
            return Ok((None, "relation_vertex_limit"));
        }
        if r["role"] == "inner" {
            inner.push(path.clone())
        } else {
            outer.push(path.clone())
        }
    }
    let (Some(outer), Some(inner)) = (rings(outer, cancel)?, rings(inner, cancel)?) else {
        return Ok((None, "unclosed_or_branching_rings"));
    };
    if outer.is_empty() {
        return Ok((None, "missing_outer"));
    }
    let mut polygons = vec![];
    for ring in outer {
        let Some(p) = points(nodes, &ring) else {
            return Ok((None, "missing_node"));
        };
        polygons.push(vec![p]);
    }
    use geo::Contains;
    for ring in inner {
        let Some(p) = points(nodes, &ring) else {
            return Ok((None, "missing_node"));
        };
        let hole = geo::Polygon::new(geo::LineString::from(p.clone()), vec![]);
        let mut parents = vec![];
        for (i, poly) in polygons.iter().enumerate() {
            check(cancel)?;
            let outer = geo::Polygon::new(geo::LineString::from(poly[0].clone()), vec![]);
            if outer.contains(&hole) {
                use geo::Area;
                parents.push((i, outer.unsigned_area()));
            }
        }
        parents.sort_by(|a, b| a.1.total_cmp(&b.1));
        if parents.is_empty() || (parents.len() > 1 && parents[0].1 == parents[1].1) {
            return Ok((None, "unassigned_hole"));
        }
        polygons[parents[0].0].push(p);
    }
    Ok((
        Some(json!({"type":"MultiPolygon","coordinates":polygons})),
        "complete",
    ))
}
fn geometry(
    nodes: &HashMap<i64, [f64; 2]>,
    ways: &HashMap<i64, Vec<i64>>,
    f: &Tagged,
    cancel: &AtomicBool,
) -> Result<(Option<Value>, &'static str)> {
    if let Some(p) = f.point {
        return Ok((Some(json!({"type":"Point","coordinates":p})), "complete"));
    }
    if f.kind == "relation" {
        return relation_geometry(nodes, ways, &f.refs, &f.tags, cancel);
    }
    let refs: Vec<i64> = serde_json::from_value(f.refs.clone())?;
    let Some(p) = points(nodes, &refs) else {
        return Ok((None, "missing_node"));
    };
    if p.len() < 2 {
        return Ok((None, "too_few_nodes"));
    }
    if area_way(&f.tags) {
        if refs.first() != refs.last() || refs.len() < 4 {
            return Ok((None, "unclosed_area"));
        }
        Ok((
            Some(json!({"type":"Polygon","coordinates":[p]})),
            "complete",
        ))
    } else {
        Ok((
            Some(json!({"type":"LineString","coordinates":p})),
            "complete",
        ))
    }
}
/// Display bounds, or the invalid-topology outcome for an unusable polygon.
fn bounds(
    g: Option<Value>,
    status: &'static str,
    cancel: &AtomicBool,
) -> Result<(Option<Value>, String, Option<[f64; 4]>)> {
    let Some(shape) = g else {
        return Ok((None, status.into(), None));
    };
    if ["Polygon", "MultiPolygon"].iter().any(|t| shape["type"] == *t) {
        return match AreaGeometry::parse(&shape, MAX_REFS, cancel) {
            Ok(a) => Ok((Some(shape), status.into(), Some(a.bounds))),
            Err(_) => {
                check(cancel)?;
                Ok((None, "invalid_topology".into(), None))
            }
        };
    }
    let points: Vec<[f64; 2]> = if shape["type"] == "Point" {
        vec![serde_json::from_value(shape["coordinates"].clone())?]
    } else {
        serde_json::from_value(shape["coordinates"].clone())?
    };
    let mut b = [180_f64, 90_f64, -180_f64, -90_f64];
    for p in points {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    Ok((Some(shape), status.into(), Some(b)))
}
/// A fresh directory is required. A failed/cancelled import has no manifest and
/// cannot activate. Call on a blocking worker and drain it on cancellation.
pub fn build(
    pbf: &Path,
    destination: &Path,
    selection: Value,
    cancel: &Arc<AtomicBool>,
) -> Result<Value> {
    build_inner(pbf, destination, selection, cancel).map_err(|e| {
        if cancel.load(Ordering::Acquire) {
            Error::cancelled()
        } else {
            e
        }
    })
}
/// Three parallel passes over the extract (relations and tagged ways, then the
/// member ways of selected relations, then nodes), keeping only coordinates the
/// selected features use. Nothing else from the extract is stored.
fn build_inner(
    pbf: &Path,
    destination: &Path,
    selection: Value,
    cancel: &Arc<AtomicBool>,
) -> Result<Value> {
    use rayon::prelude::*;
    check(cancel)?;
    if destination.exists() {
        return Err(Error::conflict(
            "OSM destination already exists; use a new candidate directory",
        ));
    }
    let pbf_bytes = fs::metadata(pbf)?.len();
    crate::disk::ensure(
        destination.parent().unwrap_or(destination),
        pbf_bytes.saturating_mul(BUILD_FACTOR),
        "Building this OSM extract",
    )?;
    let cancel: &AtomicBool = cancel;
    // Pass 1: ways and relations; every element's identity/tags are validated.
    let (header, blocks) = scan(pbf, cancel, |block| {
        let mut out = (0u64, vec![]);
        for (j, e) in block.elements().enumerate() {
            if j % 1024 == 0 {
                check(cancel)?;
            }
            match e {
                Element::Way(w) => {
                    out.0 += 1;
                    let refs = w.refs().take(MAX_REFS + 1).collect::<Vec<_>>();
                    if refs.len() > MAX_REFS {
                        return Err(invalid());
                    }
                    let version = w.info().version();
                    valid_identity(w.id(), version)?;
                    let tags = tags(w.tags())?;
                    if selected(&tags) {
                        out.1.push(Tagged { pos: (0, j), kind: "way", id: w.id(), version, time: w.info().milli_timestamp(), tags, refs: json!(refs), point: None });
                    }
                }
                Element::Relation(r) => {
                    out.0 += 1;
                    let mut members = vec![];
                    for m in r.members().take(MAX_REFS + 1) {
                        members.push(json!({"id":m.member_id,"role":m.role().map_err(|_|invalid())?,"type":match m.member_type {osmpbf::RelMemberType::Node=>"node",osmpbf::RelMemberType::Way=>"way",osmpbf::RelMemberType::Relation=>"relation"}}));
                    }
                    if members.len() > MAX_REFS {
                        return Err(invalid());
                    }
                    let version = r.info().version();
                    valid_identity(r.id(), version)?;
                    let tags = tags(r.tags())?;
                    if selected(&tags) {
                        out.1.push(Tagged { pos: (0, j), kind: "relation", id: r.id(), version, time: r.info().milli_timestamp(), tags, refs: json!(members), point: None });
                    }
                }
                Element::Node(_) | Element::DenseNode(_) => {}
            }
        }
        Ok(out)
    })?;
    let mut count = 0u64;
    let mut features = vec![];
    for (i, (n, items)) in blocks {
        count += n;
        features.extend(items.into_iter().map(|mut t| {
            t.pos.0 = i;
            t
        }));
    }
    // Pass 2: member ways of selected multipolygons (tagged or not).
    let members: HashSet<i64> = features
        .iter()
        .filter(|f| f.kind == "relation" && f.tags.get("type").is_some_and(|t| t == "multipolygon"))
        .flat_map(|f| f.refs.as_array().into_iter().flatten())
        .filter(|m| m["type"] == "way")
        .filter_map(|m| m["id"].as_i64())
        .collect();
    let ways: HashMap<i64, Vec<i64>> = if members.is_empty() {
        HashMap::new()
    } else {
        scan(pbf, cancel, |block| {
            let mut out = vec![];
            for (j, e) in block.elements().enumerate() {
                if j % 1024 == 0 {
                    check(cancel)?;
                }
                if let Element::Way(w) = e
                    && members.contains(&w.id())
                {
                    out.push((w.id(), w.refs().collect::<Vec<_>>()));
                }
            }
            Ok(out)
        })?
        .1
        .into_iter()
        .flat_map(|(_, v)| v)
        .collect()
    };
    // Pass 3: nodes. Every node is validated; coordinates are kept only when a
    // selected way or multipolygon member uses them.
    let needed: HashSet<i64> = features
        .iter()
        .filter(|f| f.kind == "way")
        .flat_map(|f| f.refs.as_array().into_iter().flatten().filter_map(Value::as_i64))
        .chain(ways.values().flatten().copied())
        .collect();
    let (_, blocks) = scan(pbf, cancel, |block| {
        let mut out = (0u64, vec![], vec![]);
        let mut node = |j: usize, id: i64, version: Option<i32>, time: Option<i64>, tags: BTreeMap<String, String>, x: f64, y: f64| -> Result<()> {
            out.0 += 1;
            valid_identity(id, version)?;
            if !x.is_finite() || !y.is_finite() || x.abs() > 180. || y.abs() > 90. {
                return Err(invalid());
            }
            if needed.contains(&id) {
                out.2.push((id, [x, y]));
            }
            if selected(&tags) {
                out.1.push(Tagged { pos: (0, j), kind: "node", id, version, time, tags, refs: json!([]), point: Some([x, y]) });
            }
            Ok(())
        };
        for (j, e) in block.elements().enumerate() {
            if j % 1024 == 0 {
                check(cancel)?;
            }
            match e {
                Element::Node(n) => node(j, n.id(), n.info().version(), n.info().milli_timestamp(), tags(n.tags())?, n.lon(), n.lat())?,
                Element::DenseNode(n) => node(j, n.id(), n.info().map(|i| i.version()), n.info().map(|i| i.milli_timestamp()), tags(n.tags())?, n.lon(), n.lat())?,
                Element::Way(_) | Element::Relation(_) => {}
            }
        }
        Ok(out)
    })?;
    let mut nodes = HashMap::with_capacity(needed.len());
    for (i, (n, items, coords)) in blocks {
        count += n;
        nodes.extend(coords);
        features.extend(items.into_iter().map(|mut t| {
            t.pos.0 = i;
            t
        }));
    }
    drop(needed);
    // File order, as a sequential reader would insert them.
    features.sort_unstable_by_key(|f| f.pos);
    if features.is_empty() {
        return Err(Error::invalid(
            "No selected named OSM facilities or places in this snapshot",
        ));
    }
    let shapes = features
        .par_iter()
        .map(|f| {
            let (g, status) = geometry(&nodes, &ways, f, cancel)?;
            bounds(g, status, cancel)
        })
        .collect::<Result<Vec<_>>>()?;
    drop((nodes, ways));
    fs::create_dir_all(destination)?;
    let path = destination.join("index.sqlite3");
    let c = Connection::open(&path)?;
    c.execute_batch("PRAGMA journal_mode=DELETE;PRAGMA synchronous=OFF;CREATE TABLE features(rowid INTEGER PRIMARY KEY,id TEXT UNIQUE NOT NULL,osm_type TEXT NOT NULL,osm_id TEXT NOT NULL,version INTEGER,timestamp INTEGER,tags TEXT NOT NULL,refs TEXT NOT NULL,classes TEXT NOT NULL,geometry TEXT,status TEXT NOT NULL);CREATE TABLE aliases(key TEXT NOT NULL,feature TEXT NOT NULL,tag TEXT NOT NULL,language TEXT NOT NULL,name TEXT NOT NULL,PRIMARY KEY(key,feature,tag,name));CREATE VIRTUAL TABLE feature_bounds USING rtree(rowid,min_lon,max_lon,min_lat,max_lat);BEGIN IMMEDIATE;")?;
    let mut statuses = BTreeMap::<String, u64>::new();
    for (i, (f, (g, status, b))) in features.iter().zip(shapes).enumerate() {
        if i % 1024 == 0 {
            check(cancel)?;
        }
        let rowid = i as i64 + 1;
        let identity = format!("osm:{}:{}", f.kind, f.id);
        execute(
            &c,
            "INSERT INTO features(rowid,id,osm_type,osm_id,version,timestamp,tags,refs,classes,geometry,status) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![rowid, identity, f.kind, f.id.to_string(), f.version, f.time, string(&json!(f.tags)), string(&f.refs), string(&json!(classes(&f.tags))), g.map(|g| string(&g)), status],
        )?;
        for (tag, lang, name) in aliases(&f.tags) {
            execute(
                &c,
                "INSERT OR IGNORE INTO aliases VALUES(?1,?2,?3,?4,?5)",
                params![key(&name), identity, tag, lang, name],
            )?;
        }
        if let Some([w, s, e, n]) = b {
            execute(&c, "INSERT INTO feature_bounds VALUES(?1,?2,?3,?4,?5)", params![rowid, w, e, s, n])?;
        }
        *statuses.entry(status).or_default() += 1;
    }
    let total = features.len();
    c.execute_batch("COMMIT;PRAGMA optimize;")?;
    drop(c);
    fs::File::open(&path)?.sync_all()?;
    check(cancel)?;
    let (index_hash, index_bytes) = file_hash(&path, cancel)?;
    let (pbf_hash, pbf_bytes) = file_hash(pbf, cancel)?;
    let manifest = json!({"schema":"relic.osm-reference.v1","mapping":MAPPING,"selection":selection,"header":header,"license":"ODbL-1.0","attribution":"© OpenStreetMap contributors","rights_url":"https://www.openstreetmap.org/copyright","raw":{"sha256":pbf_hash,"bytes":pbf_bytes},"index":{"sha256":index_hash,"bytes":index_bytes},"objects_scanned":count,"selected_features":total,"geometry_status":statuses,"limitations":["Named facilities and places selected by versioned native-tag mapping; not all OSM features","Source reference geometry is not an incident observation","Missing members and unsupported relation types remain explicit; no invented centroid","Old-name aliases retained but not eligible for current exact-name matching"]});
    fs::write(
        destination.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::File::open(destination.join("manifest.json"))?.sync_all()?;
    fs::File::open(destination)?.sync_all()?;
    Ok(manifest)
}

pub fn file_hash(path: &Path, cancel: &AtomicBool) -> Result<(String, u64)> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 65536];
    let mut total = 0;
    loop {
        check(cancel)?;
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        total += n as u64;
    }
    Ok((format!("{:x}", hash.finalize()), total))
}

fn execute(c: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<usize> {
    Ok(c.prepare_cached(sql)?.execute(params)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn places_and_crossings_are_selected_but_admin_labels_are_not_facilities() {
        let t = |pairs: &[(&str, &str)]| {
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>()
        };
        assert_eq!(classes(&t(&[("place", "village")])), vec!["village"]);
        assert_eq!(classes(&t(&[("place", "town")])), vec!["city"]);
        assert_eq!(classes(&t(&[("place", "neighbourhood")])), vec!["district"]);
        assert_eq!(classes(&t(&[("place", "islet")])), vec!["island"]);
        assert_eq!(classes(&t(&[("barrier", "border_control")])), vec!["border_crossing"]);
        assert_eq!(classes(&t(&[("natural", "peak")])), vec!["mountain"]);
        assert!(classes(&t(&[("place", "locality")])).is_empty());
        assert!(crate::osm_resolution::is_place(&json!(["village"])));
        assert!(!crate::osm_resolution::is_place(&json!(["village", "military"])));
        assert!(!crate::osm_resolution::is_place(&json!([])));
        // An island drawn as a closed way is an area, not a coastline.
        assert!(area_way(&t(&[("place", "island")])));
    }
    #[test]
    fn aliases_exclude_name_metadata_and_rings_obey_cancellation() {
        let tags = BTreeMap::from([
            ("name:en".into(), "Cedar Airport".into()),
            ("name:fa".into(), "فرودگاه سرو".into()),
            ("name:signed".into(), "no".into()),
            ("name:etymology:wikidata".into(), "Q1".into()),
            ("name:pronunciation".into(), "noise".into()),
        ]);
        assert_eq!(aliases(&tags).len(), 2);
        let flag = AtomicBool::new(false);
        assert_eq!(
            rings(vec![vec![1, 2, 3], vec![1, 4, 3]], &flag)
                .unwrap()
                .unwrap(),
            vec![vec![1, 2, 3, 4, 1]]
        );
        assert!(
            rings(vec![vec![1, 2], vec![2, 3], vec![2, 4]], &flag)
                .unwrap()
                .is_none()
        );
        let closed = vec![3, 7, 8, 3];
        let open_a = vec![1, 2, 3];
        let open_b = vec![3, 4, 1];
        let first = rings(vec![open_a.clone(), closed.clone(), open_b.clone()], &flag)
            .unwrap()
            .unwrap();
        let second = rings(vec![closed, open_a, open_b], &flag).unwrap().unwrap();
        assert_eq!(first, second);
        flag.store(true, Ordering::Release);
        assert_eq!(
            rings(vec![vec![1, 2, 3, 1]], &flag).unwrap_err().code,
            "cancelled"
        );
    }
}
