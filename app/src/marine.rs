//! Explicit SeaVoX v19 selections, retained as areas with no synthetic points.
use crate::{
    area_geometry::{AreaGeometry, MAX_VERTICES},
    boundary_store::AreaRecord,
    error::{Error, Result},
    model::{hash, now, required, string},
    store::{Store, edge, node},
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// No per-area size cap (27 September 2026); only free disk space is checked.
pub const MAX_BYTES: usize = usize::MAX;
pub fn selections() -> Value {
    serde_json::from_str(include_str!("../../reference/marine-selections.json"))
        .expect("checked marine selections")
}
fn invalid() -> Error {
    Error::invalid(
        "Marine release failed identity, completeness, checksum or geometry validation; previous areas remain active",
    )
}
pub fn request_url(id: &str) -> Result<String> {
    if !selections()["areas"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["mrgid"] == id)
    {
        return Err(invalid());
    }
    let mut u = url::Url::parse("https://geo.vliz.be/geoserver/MarineRegions/wfs").unwrap();
    u.query_pairs_mut().extend_pairs([
        ("service", "WFS"),
        ("version", "2.0.0"),
        ("request", "GetFeature"),
        ("typeNames", "MarineRegions:seavox_v19"),
        ("cql_filter", &format!("mrgid_sr={id}")),
        ("count", "2"),
        ("outputFormat", "application/json"),
        ("srsName", "EPSG:4326"),
    ]);
    Ok(u.into())
}
fn validate(raw: &Value, selection: &Value, cancel: &AtomicBool) -> Result<(Value, [f64; 4])> {
    if cancel.load(Ordering::Acquire) {
        return Err(Error::cancelled());
    }
    if raw["type"] != "FeatureCollection"
        || raw["numberMatched"] != 1
        || raw["numberReturned"] != 1
        || raw["totalFeatures"] != 1
        || raw["crs"]["properties"]["name"] != "urn:ogc:def:crs:EPSG::4326"
    {
        return Err(invalid());
    }
    let features = raw["features"]
        .as_array()
        .filter(|a| a.len() == 1)
        .ok_or_else(invalid)?;
    let f = &features[0];
    let p = &f["properties"];
    if p["mrgid_sr"] != selection["mrgid"]
        || p["sub_region"] != selection["publisher_name"]
        || p["skos_url"] != selection["vocabulary_uri"]
    {
        return Err(invalid());
    }
    let shape = AreaGeometry::parse_marine(&f["geometry"], MAX_VERTICES, cancel)?;
    for (key, value) in ["minlong", "minlat", "maxlong", "maxlat"]
        .iter()
        .zip(shape.bounds)
    {
        if p[key].as_f64().is_none_or(|v| (v - value).abs() > 0.001) {
            return Err(invalid());
        }
    }
    // Start with exact full geometry as display geometry. Never silently repair
    // topology or simplify away islands/holes to pass admission.
    Ok((f.clone(), shape.bounds))
}
/// Map display outline: the exact publisher polygon simplified until it has at
/// most `DISPLAY_VERTICES` vertices. Display only: membership and placement keep
/// the exact full geometry. Features too small to keep a ring at that scale
/// (tiny islets) drop out of the display outline, never out of analysis.
pub const DISPLAY_VERTICES: usize = 20_000;
pub fn display_geometry(geometry: &Value) -> Result<Value> {
    use geo::{CoordsIter, Simplify};
    let full = AreaGeometry::parse_marine(geometry, MAX_VERTICES, &AtomicBool::new(false))?;
    if full.vertices <= DISPLAY_VERTICES {
        return Ok(geometry.clone());
    }
    use geo::BoundingRect;
    // Rings smaller than a few simplification steps are invisible at this scale.
    let visible = |ring: &geo::LineString<f64>, epsilon: f64| {
        ring.0.len() >= 4
            && ring.bounding_rect().is_some_and(|r| r.width().max(r.height()) > epsilon * 4.0)
    };
    let mut epsilon = 0.001_f64;
    loop {
        let simple = full.polygons.simplify(epsilon);
        let kept = geo::MultiPolygon::new(
            simple
                .0
                .into_iter()
                .filter(|p| visible(p.exterior(), epsilon))
                .map(|p| {
                    let (exterior, interiors) = p.into_inner();
                    let interiors = interiors.into_iter().filter(|r| visible(r, epsilon)).collect();
                    geo::Polygon::new(exterior, interiors)
                })
                .collect(),
        );
        if kept.coords_count() <= DISPLAY_VERTICES || epsilon > 1.0 {
            let rings = |l: &geo::LineString<f64>| l.0.iter().map(|c| [c.x, c.y]).collect::<Vec<_>>();
            let polygons = kept
                .0
                .iter()
                .map(|p| {
                    std::iter::once(rings(p.exterior()))
                        .chain(p.interiors().iter().map(rings))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            return Ok(json!({"type":"MultiPolygon","coordinates":polygons}));
        }
        epsilon *= 1.6;
    }
}
async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Acquire) {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
pub async fn acquire(destination: &Path, cancel: &Arc<AtomicBool>) -> Result<Value> {
    if cancel.load(Ordering::Acquire) {
        return Err(Error::cancelled());
    }
    use futures_util::StreamExt;
    if destination.exists() {
        return Err(Error::invalid("Marine destination must be new"));
    }
    let client = wreq::Client::builder()
        .no_proxy()
        .redirect(wreq::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|_| invalid())?;
    tokio::fs::create_dir_all(destination).await?;
    let catalog = selections();
    let mut assets = json!({});
    let mut requests = vec![];
    for s in catalog["areas"].as_array().unwrap() {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        let id = required(s, "mrgid")?;
        let url = request_url(id)?;
        let response = tokio::select! {
            r = client.get(&url).send() => r.map_err(|_| invalid())?,
            _ = cancelled(cancel) => return Err(Error::cancelled()),
        };
        if !response.status().is_success() {
            return Err(invalid());
        }
        if let Some(n) = response.content_length() {
            crate::disk::ensure(destination, n.saturating_mul(3), "Downloading this marine area")?;
        }
        let mut stream = response.bytes_stream();
        let mut bytes = vec![];
        while let Some(chunk) = tokio::select! {
            r = stream.next() => r,
            _ = cancelled(cancel) => return Err(Error::cancelled()),
        } {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let chunk = chunk.map_err(|_| invalid())?;
            bytes.extend_from_slice(&chunk);
        }
        let selection = s.clone();
        let flag = cancel.clone();
        // Flag-based cancellation drains the worker. Callers must signal this
        // flag and await acquire, rather than aborting/dropping its future.
        let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            if flag.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            validate(&serde_json::from_slice(&bytes)?, &selection, &flag)?;
            Ok(bytes)
        })
        .await
        .map_err(|_| Error::internal())??;
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        let filename = format!("{id}.geojson");
        tokio::fs::write(destination.join(&filename), &bytes).await?;
        assets[&filename] = json!({"sha256":hash(&bytes),"bytes":bytes.len()});
        requests.push(json!({"url":url,"acquired_at":now()}));
    }
    let m = json!({"schema_version":1,"status":"candidate","publisher":"Marine Regions / VLIZ","product":"SeaVoX v19","license":"CC-BY-4.0","selections_sha256":hash(string(&catalog).as_bytes()),"selection_review":catalog,"assets":assets,"requests":requests});
    if cancel.load(Ordering::Acquire) {
        return Err(Error::cancelled());
    }
    tokio::fs::write(
        destination.join("manifest.json"),
        serde_json::to_vec_pretty(&m)?,
    )
    .await?;
    Ok(m)
}
impl Store {
    pub fn marine(&self) -> Result<Option<Value>> {
        self.connect()?
            .query_row(
                "SELECT value FROM metadata WHERE key='active_marine'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|s| serde_json::from_str(&s).map_err(Error::from))
            .transpose()
    }
    pub fn activate_marine(&self, path: &Path, cancel: &AtomicBool) -> Result<Value> {
        let root = self.config.root.canonicalize()?;
        let path = path.canonicalize()?;
        if !path.starts_with(&root) {
            return Err(invalid());
        }
        let read = |name: &str, max: u64| -> Result<Vec<u8>> {
            let p = path.join(name).canonicalize()?;
            if !p.starts_with(&path) || fs::metadata(&p)?.len() > max {
                return Err(invalid());
            }
            Ok(fs::read(p)?)
        };
        let mb = read("manifest.json", 65536)?;
        let manifest: Value = serde_json::from_slice(&mb)?;
        let catalog = selections();
        if manifest["schema_version"] != 1
            || manifest["status"] != "candidate"
            || manifest["product"] != "SeaVoX v19"
            || manifest["publisher"] != "Marine Regions / VLIZ"
            || manifest["license"] != "CC-BY-4.0"
            || manifest["selections_sha256"] != hash(string(&catalog).as_bytes())
            || manifest["selection_review"] != catalog
        {
            return Err(invalid());
        }
        let selections = catalog["areas"].as_array().unwrap();
        if manifest["assets"]
            .as_object()
            .is_none_or(|a| a.len() != selections.len())
            || manifest["requests"]
                .as_array()
                .is_none_or(|a| a.len() != selections.len())
        {
            return Err(invalid());
        }
        let mut objects = vec![];
        let mut shapes = vec![];
        for (index, s) in selections.iter().enumerate() {
            let id = required(s, "mrgid")?;
            let filename = format!("{id}.geojson");
            let bytes = read(&filename, MAX_BYTES as u64)?;
            if manifest["assets"][&filename]["sha256"] != hash(&bytes)
                || manifest["assets"][&filename]["bytes"] != bytes.len()
                || manifest["requests"][index]["url"] != request_url(id)?
                || manifest["requests"][index]["acquired_at"]
                    .as_str()
                    .is_none()
            {
                return Err(invalid());
            }
            let (feature, bounds) = validate(&serde_json::from_slice(&bytes)?, s, cancel)?;
            shapes.push((s.clone(), feature, bounds, hash(&bytes)));
            objects.push(bytes);
        }
        let edition = hash(&mb);
        let active = json!({"edition":edition,"features":shapes.len(),"editions":[{"product":"SeaVoX v19","manifest_hash":edition}],"activated_at":now(),"attribution":"Marine Regions / VLIZ, SeaVoX v19 (2023), CC-BY-4.0. Geographic reference, not legal or navigation authority."});
        objects.push(mb);
        let mut retained = vec![];
        for bytes in objects {
            retained.push((self.write_object(&bytes)?, bytes.len()));
        }
        self.sync_object_directories(retained.iter().map(|(h, _)| h.clone()), cancel)?;
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let geography = self.geography()?;
        let boundaries = self.boundaries()?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        for (h, n) in retained {
            tx.execute(
                "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
                params![h, n as i64],
            )?;
        }
        for (s, f, bounds, source_hash) in shapes {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let sid = required(&s, "mrgid")?;
            let id = format!("seavox:{edition}:{sid}");
            let meta = json!({"publisher":"Marine Regions / VLIZ","product":"SeaVoX v19","boundaryYearRepresented":"2023 release; not a live observation","aliases":s["aliases"],"aliases_sha256":hash(string(&catalog).as_bytes()),"vocabulary_uri":s["vocabulary_uri"],"url":request_url(sid)?,"relic_source_sha256":source_hash,"relic_record_sha256":hash(string(&f).as_bytes())});
            // Re-activating the same edition refreshes only the display outline;
            // the exact analytical geometry and identity never change.
            tx.execute("INSERT INTO boundary_shapes VALUES(?1,?2,'','','MARINE',?3,?4,?5,?6,?7,?8,?2) ON CONFLICT(id) DO UPDATE SET display_json=excluded.display_json",params![id,edition,sid,required(&s,"label")?,string(&f["geometry"]),string(&display_geometry(&f["geometry"])?),string(&json!(bounds)),string(&meta)])?;
        }
        let input = format!("reference:{edition}");
        node(
            &tx,
            &input,
            "reference_input",
            "SeaVoX selected marine areas",
            &manifest,
        )?;
        let id = format!("marine:{edition}");
        node(
            &tx,
            &id,
            "reference_edition",
            "Accepted marine areas",
            &active,
        )?;
        edge(
            &tx,
            &input,
            &id,
            "reference_input",
            &json!({"manifest_hash":edition}),
        )?;
        crate::boundary_store::record_context(
            &tx,
            geography.as_ref(),
            boundaries.as_ref(),
            Some(&active),
            self.osm()?.as_ref(),
        )?;
        tx.execute("INSERT INTO metadata VALUES('active_marine',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[string(&active)])?;
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        tx.commit()?;
        Ok(active)
    }
}

pub fn resolve(
    areas: &[AreaRecord],
    p: &crate::processing_geo::LocationProposal,
    allowed: bool,
    mut base: crate::processing_geo::LocationResolution,
) -> crate::processing_geo::LocationResolution {
    use crate::processing_geo::{LocationCandidate, LocationRole, ReferenceIdentity};
    if base.status == "unsupported_quote" {
        return base;
    }
    let norm = |s: &str| {
        s.trim()
            .trim_start_matches('#')
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let matches = areas
        .iter()
        .filter(|a| {
            a.level == "MARINE"
                && (norm(&a.name) == norm(&p.mention)
                    || a.metadata["aliases"].as_array().is_some_and(|aliases| {
                        aliases
                            .iter()
                            .filter_map(Value::as_str)
                            .any(|s| norm(s) == norm(&p.mention))
                    }))
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return base;
    }
    base.geometry = None;
    base.area = None;
    base.occurrence_eligible = false;
    base.selected_candidate_id = None;
    base.candidate_count = matches.len();
    base.candidates_truncated = false;
    base.candidates=matches.iter().map(|a|LocationCandidate{id:a.id.clone(),name:a.name.clone(),kind:"marine_area".into(),country_code:None,country_name:None,geometry:None,precision:"marine_area".into(),reference:ReferenceIdentity{dataset_id:"marine-regions-seavox".into(),publisher:"Marine Regions / VLIZ".into(),edition:a.edition.clone(),product_version:"19".into(),source_path:a.metadata["url"].as_str().unwrap_or("").into(),source_sha256:a.source_sha256.clone(),publisher_record_id:a.shape_id.clone(),record_sha256:a.record_sha256.clone(),acceptance_id:format!("marine:{}",a.edition),acceptance_sha256:a.edition.clone(),represented_time:"2023 release; not a live observation".into()},match_basis:"exact_name_or_reviewed_marine_alias".into(),matched_alias:Some(p.mention.clone()),alias_reference:Some(json!({"review":"reference/marine-selections.json","sha256":a.metadata["aliases_sha256"]})),country_constraint_match:None}).collect();
    if matches.len() != 1 {
        base.status = "ambiguous".into();
        base.reason = "Several accepted marine identities match.".into();
        return base;
    }
    let a = matches[0];
    base.selected_candidate_id = Some(a.id.clone());
    base.precision = "marine_area".into();
    // A country label cannot narrow a transboundary sea. Canonical names do not
    // override a literal marine identity; a different accepted name is a conflict.
    let canonical_conflict = p.canonical_name.as_ref().is_some_and(|name| {
        areas.iter().any(|other| {
            other.level == "MARINE"
                && other.id != a.id
                && (norm(name) == norm(&other.name)
                    || other.metadata["aliases"].as_array().is_some_and(|v| {
                        v.iter()
                            .filter_map(Value::as_str)
                            .any(|s| norm(s) == norm(name))
                    }))
        })
    });
    if canonical_conflict {
        base.selected_candidate_id = None;
        base.status = "canonical_conflict".into();
        base.reason =
            "The canonical marine identity conflicts with the literal source name.".into();
        return base;
    }
    if allowed && p.role == LocationRole::Event {
        base.area = Some(
            json!({"id":a.id,"label":a.name,"bounds":a.bounds,"level":"MARINE","represented_time":a.metadata["boundaryYearRepresented"]}),
        );
        base.occurrence_eligible = true;
        base.status = "resolved_area".into();
        base.reason="A reported occurrence names this marine region. The full publisher area expresses regional precision, not an incident footprint or a country assignment.".into();
    } else {
        base.status = "context_only".into();
        base.reason =
            "The marine region is contextual; it does not establish an occurrence there.".into();
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_outline_is_bounded_but_small_shapes_stay_exact() {
        let small = json!({"type":"Polygon","coordinates":[[[10.,20.],[11.,20.],[11.,21.],[10.,21.],[10.,20.]]]});
        assert_eq!(display_geometry(&small).unwrap(), small);
        // A 40,000-vertex ring is simplified for display to at most the bound.
        let ring = (0..40_000)
            .map(|i| {
                let a = i as f64 / 40_000.0 * std::f64::consts::TAU;
                [50.0 + a.cos() * (1.0 + 0.001 * (i % 7) as f64), 20.0 + a.sin()]
            })
            .chain(std::iter::once([51.0, 20.0]))
            .collect::<Vec<_>>();
        let big = json!({"type":"Polygon","coordinates":[ring]});
        let shown = display_geometry(&big).unwrap();
        let n: usize = shown["coordinates"].as_array().unwrap().iter().flat_map(|p| p.as_array().unwrap()).map(|r| r.as_array().unwrap().len()).sum();
        assert!((4..=DISPLAY_VERTICES).contains(&n), "{n}");
    }
    #[test]
    fn publisher_identity_crs_counts_and_bounds_must_all_match() {
        let selection = selections()["areas"][0].clone();
        let raw = json!({"type":"FeatureCollection","numberMatched":1,"numberReturned":1,"totalFeatures":1,"crs":{"properties":{"name":"urn:ogc:def:crs:EPSG::4326"}},"features":[{"type":"Feature","properties":{"mrgid_sr":selection["mrgid"],"sub_region":selection["publisher_name"],"skos_url":selection["vocabulary_uri"],"minlong":10,"minlat":20,"maxlong":11,"maxlat":21},"geometry":{"type":"Polygon","coordinates":[[[10,20],[11,20],[11,21],[10,21],[10,20]]]}}]});
        let cancel = AtomicBool::new(false);
        assert!(validate(&raw, &selection, &cancel).is_ok());
        for (pointer, bad) in [
            ("/numberMatched", json!(2)),
            ("/numberReturned", json!(0)),
            ("/totalFeatures", json!("unknown")),
            ("/crs/properties/name", json!("EPSG:3857")),
            ("/features/0/properties/mrgid_sr", json!("22705")),
            (
                "/features/0/properties/sub_region",
                json!("A different strait"),
            ),
            (
                "/features/0/properties/skos_url",
                json!("https://example.invalid/other"),
            ),
            ("/features/0/properties/minlong", json!(20)),
        ] {
            let mut changed = raw.clone();
            *changed.pointer_mut(pointer).unwrap() = bad;
            assert!(
                validate(&changed, &selection, &cancel).is_err(),
                "{pointer}"
            );
        }
        for bad in ["22705", "24076 OR 1=1", "../24076", "24076&count=100000"] {
            assert!(request_url(bad).is_err());
        }
    }
}
