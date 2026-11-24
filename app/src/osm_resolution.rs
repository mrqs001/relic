//! Exact facility identity is additional reference support, never an inferred
//! incident coordinate. Country constraints must pass accepted full boundaries.
use crate::{
    area_geometry::AreaGeometry,
    error::{Error, Result},
    model::{hash, string},
    osm_store::Index,
    processing_geo::{
        LocationCandidate, LocationProposal, LocationResolution, LocationRole, ReferenceIdentity,
    },
    store::Store,
};
use geo::Contains;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, atomic::AtomicBool},
};
pub type CountryCache = Arc<Mutex<BTreeMap<String, Option<Arc<AreaGeometry>>>>>;
pub fn snapshot_time(record: &Value) -> String {
    record["snapshot_timestamp"]
        .as_i64()
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|t| format!("OSM snapshot {}", t.format("%Y-%m-%d %H:%M UTC")))
        .unwrap_or_else(|| "OSM snapshot time not supplied".into())
}
/// A named place (settlement, district, island, square, peak) rather than a
/// facility: only place classes, per the versioned mapping.
pub fn is_place(classes: &Value) -> bool {
    classes.as_array().is_some_and(|c| {
        !c.is_empty()
            && c.iter()
                .all(|v| crate::osm::PLACE_CLASSES.contains(&v.as_str().unwrap_or("")))
    })
}
pub fn record_hash(record: &Value) -> String {
    hash(string(&json!({"id":record["id"],"version":record["version"],"timestamp":record["timestamp"],"tags":record["tags"],"geometry":record["geometry"],"status":record["status"]})).as_bytes())
}
pub fn shape_id(edition: &str, record: &Value) -> String {
    format!("osm:{edition}:{}", record_hash(record))
}
fn contained(
    store: &Store,
    cache: &CountryCache,
    areas: &[crate::boundary_store::AreaRecord],
    country: &str,
    geometry: &Value,
    bounds: &Value,
) -> Result<bool> {
    let mut cache = cache.lock().map_err(|_| Error::internal())?;
    if !cache.contains_key(country) {
        let choices = areas
            .iter()
            .filter(|a| a.level == "ADM0" && a.iso2 == country)
            .collect::<Vec<_>>();
        let area = if choices.len() == 1 {
            let raw: String = store.connect()?.query_row(
                "SELECT full_json FROM boundary_shapes WHERE id=?1",
                [&choices[0].id],
                |r| r.get(0),
            )?;
            Some(Arc::new(AreaGeometry::accepted(&serde_json::from_str(
                &raw,
            )?)?))
        } else {
            None
        };
        cache.insert(country.into(), area);
    }
    let Some(area) = cache.get(country).and_then(Option::as_ref).cloned() else {
        return Ok(false);
    };
    drop(cache);
    // R-tree bounds are outward-rounded. Rejection is conservative; passing
    // this cheap gate still requires containment by the full country geometry.
    if let Ok([w, s, e, n]) = serde_json::from_value::<[f64; 4]>(bounds.clone()) {
        let [aw, as_, ae, an] = area.bounds;
        if aw >= -180. && ae <= 180. && (e < aw || w > ae || n < as_ || s > an) {
            return Ok(false);
        }
    }
    Ok(match geometry["type"].as_str() {
        Some("Point") => {
            let p: [f64; 2] = serde_json::from_value(geometry["coordinates"].clone())?;
            area.polygons.contains(&geo::Point::from(p))
        }
        Some("LineString") => {
            let points: Vec<[f64; 2]> = serde_json::from_value(geometry["coordinates"].clone())?;
            area.polygons.contains(&geo::LineString::from(points))
        }
        Some("Polygon" | "MultiPolygon") => area
            .polygons
            .contains(&AreaGeometry::accepted(geometry)?.polygons),
        _ => false,
    })
}
/// The accepted countries whose full boundaries contain this geometry.
fn containing_countries(
    store: &Store,
    cache: &CountryCache,
    areas: &[crate::boundary_store::AreaRecord],
    geometry: &Value,
    bounds: &Value,
) -> Result<Vec<String>> {
    let Ok([w, s, e, n]) = serde_json::from_value::<[f64; 4]>(bounds.clone()) else {
        return Ok(vec![]);
    };
    let mut found = vec![];
    for a in areas.iter().filter(|a| a.level == "ADM0") {
        let Ok([aw, as_, ae, an]) = serde_json::from_value::<[f64; 4]>(a.bounds.clone()) else {
            continue;
        };
        if e < aw || w > ae || n < as_ || s > an {
            continue;
        }
        if contained(store, cache, areas, &a.iso2, geometry, bounds)? {
            found.push(a.iso2.clone());
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}
/// Several eligible records for one name within ~5 km sharing a class are one
/// feature mapped twice (an airport's point and outline). Prefer its outline.
fn same_feature(records: &[(Value, bool)], eligible: &[usize]) -> Option<usize> {
    let bounds = |i: usize| serde_json::from_value::<[f64; 4]>(records[i].0["bounds"].clone()).ok();
    let first = bounds(eligible[0])?;
    let classes = |i: usize| records[i].0["classes"].as_array().cloned().unwrap_or_default();
    const NEAR: f64 = 0.05;
    for &i in &eligible[1..] {
        let b = bounds(i)?;
        if b[2] + NEAR < first[0] || b[0] - NEAR > first[2] || b[3] + NEAR < first[1] || b[1] - NEAR > first[3] {
            return None;
        }
        if !classes(i).iter().any(|c| classes(eligible[0]).contains(c)) {
            return None;
        }
    }
    let area = |i: &&usize| {
        ["Polygon", "MultiPolygon"].iter().any(|t| records[**i].0["geometry"]["type"] == *t)
    };
    Some(*eligible.iter().find(area).unwrap_or(&eligible[0]))
}
pub fn resolve(
    indexes: &[Index],
    store: &Store,
    cache: &CountryCache,
    areas: &[crate::boundary_store::AreaRecord],
    p: &LocationProposal,
    allowed: bool,
    mut base: LocationResolution,
) -> Result<LocationResolution> {
    if !["unresolved", "proposal_only"].contains(&base.status.as_str()) {
        return Ok(base);
    }
    let mut records = BTreeMap::<String, (Value, &Index)>::new();
    let mut truncated = false;
    for index in indexes {
        let matches = index.matches(&p.mention)?;
        truncated |= matches.len() > 20;
        for mut record in matches {
            let membership = json!({"region":record["region"],"manifest_hash":record["manifest_hash"],"raw_sha256":record["raw_sha256"],"source":record["source"],"snapshot_timestamp":record["snapshot_timestamp"]});
            let key = record_hash(&record);
            if let Some((existing, _)) = records.get_mut(&key) {
                existing["memberships"]
                    .as_array_mut()
                    .unwrap()
                    .push(membership);
            } else {
                record["memberships"] = json!([membership]);
                records.insert(key, (record, index));
            }
        }
    }
    if records.is_empty() {
        return Ok(base);
    }
    let mut candidates = vec![];
    let mut selected_records = vec![];
    // Without a supplied country, a name no gazetteer knows anywhere may still
    // match one OSM feature; its country comes from the accepted boundary that
    // contains it, never from the extract it was downloaded in.
    let country_free = p.country_code.is_none() && base.candidate_count == 0 && base.status == "unresolved";
    for (_, (record, index)) in records {
        let (matches, country) = if let Some(country) = p.country_code.as_deref().map(str::to_ascii_uppercase) {
            (
                contained(store, cache, areas, &country, &record["geometry"], &record["bounds"])?,
                Some(country),
            )
        } else if country_free {
            // One country; or two for a border crossing, which lies on the
            // line between them and so belongs to neither alone.
            let found = containing_countries(store, cache, areas, &record["geometry"], &record["bounds"])?;
            let crossing = record["classes"].as_array().is_some_and(|c| c.iter().any(|class| class == "border_crossing"));
            match found.len() {
                1 => (true, found.into_iter().next()),
                2 if crossing => (true, None),
                _ => (false, None),
            }
        } else {
            (false, None)
        };
        let id = shape_id(&index.edition, &record);
        let name = record["tags"]["name:en"]
            .as_str()
            .or(record["tags"]["name"].as_str())
            .unwrap_or(&p.mention);
        let kind = record["geometry"]["type"].as_str();
        // A place point is as approximate as a settlement point from any other
        // reference; a facility point keeps its facility precision.
        let place = is_place(&record["classes"]);
        let precision = match (kind, place) {
            (Some("Point"), true) => "city_level_approximate",
            (Some("Point"), false) => "approximate_facility",
            (Some("LineString"), true) => "place_line",
            (Some("LineString"), false) => "facility_line",
            (_, true) => "place_area",
            (_, false) => "facility_area",
        };
        candidates.push(LocationCandidate{id:id.clone(),name:name.into(),kind:if place {"place"} else {"facility"}.into(),country_code:matches.then(||country.clone()).flatten(),country_name:None,geometry:if kind==Some("Point"){Some(record["geometry"].clone())}else{None},precision:precision.into(),reference:ReferenceIdentity{dataset_id:"geofabrik-osm".into(),publisher:"OpenStreetMap contributors / Geofabrik".into(),edition:index.edition.clone(),product_version:index.manifest["mapping"].as_str().unwrap_or(crate::osm::MAPPING).into(),source_path:record["source"].as_str().unwrap_or("").into(),source_sha256:record["raw_sha256"].as_str().unwrap_or("").into(),publisher_record_id:record["id"].as_str().unwrap_or("").into(),record_sha256:record_hash(&record),acceptance_id:format!("osm:{}",index.edition),acceptance_sha256:index.manifest_hash.clone(),represented_time:snapshot_time(&record)},match_basis:"literal_current_osm_name".into(),matched_alias:record["alias"]["name"].as_str().map(str::to_owned),alias_reference:Some(json!({"alias":record["alias"],"source_memberships":record["memberships"],"version":record["version"],"object_modified_at_ms":record["timestamp"],"region":record["region"],"country_verified_against":areas.iter().find(|a|a.level=="ADM0"&&Some(&a.iso2)==country.as_ref()).map(|a|&a.edition)})),country_constraint_match:country.as_ref().map(|_|matches)});
        selected_records.push((record, matches));
    }
    base.candidate_count = candidates.len();
    base.candidates_truncated = truncated || candidates.len() > 20;
    let eligible = selected_records
        .iter()
        .enumerate()
        .filter(|(_, (_, matches))| *matches)
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let chosen = if base.candidates_truncated || eligible.is_empty() {
        None
    } else if eligible.len() == 1 {
        Some(eligible[0])
    } else {
        same_feature(&selected_records, &eligible)
    };
    // A proposed translation must match the same feature's current alias too.
    let canonical_conflict =
        if let (Some(chosen), Some(canonical)) = (chosen, p.canonical_name.as_deref()) {
            if crate::osm::key(canonical) == crate::osm::key(&p.mention) {
                false
            } else {
                let identity = record_hash(&selected_records[chosen].0);
                let mut same = false;
                for index in indexes {
                    same |= index
                        .matches(canonical)?
                        .iter()
                        .any(|r| record_hash(r) == identity);
                }
                !same
            }
        } else {
            false
        };
    base.geometry = None;
    base.area = None;
    base.selected_candidate_id = None;
    base.occurrence_eligible = false;
    if canonical_conflict {
        base.status = "canonical_conflict".into();
        base.reason =
            "The proposed canonical name does not identify this same retained facility.".into();
    } else if let Some(chosen) = chosen {
        let c = &candidates[chosen];
        let record = &selected_records[chosen].0;
        base.selected_candidate_id = Some(c.id.clone());
        base.precision = c.precision.clone();
        let place = is_place(&record["classes"]);
        base.status = "context_only".into();
        base.reason = match (place, country_free) {
            (true, false) => "Exact OpenStreetMap place name and accepted country geometry match. The place point is approximate, not an observed incident coordinate.",
            (false, false) => "Exact facility name and accepted country geometry match. This reference is not an observed incident coordinate.",
            (_, true) => "The only OpenStreetMap feature with this exact name in the loaded regions; no gazetteer knows the name elsewhere. Its country is the accepted boundary containing it. Approximate, not an observed incident coordinate.",
        }
        .into();
        if allowed && p.role == LocationRole::Event && record["status"] == "complete" {
            if record["geometry"]["type"] == "Point" {
                base.geometry = c.geometry.clone();
                base.status = "resolved".into();
            } else if ["Polygon", "MultiPolygon"]
                .iter()
                .any(|t| record["geometry"]["type"] == *t)
            {
                let area =
                    AreaGeometry::parse(&record["geometry"], 100_000, &AtomicBool::new(false))?;
                base.area = Some(
                    json!({"id":c.id,"label":c.name,"bounds":area.bounds,"level":if place {"PLACE"} else {"FACILITY"},"represented_time":snapshot_time(record)}),
                );
                base.status = "resolved_area".into();
            }
            base.occurrence_eligible = base.geometry.is_some() || base.area.is_some();
            if record["geometry"]["type"] == "LineString" {
                base.reason="The named linear feature matches. Its line remains context; no point or filled area was inferred.".into();
            }
        }
    } else {
        base.status = if eligible.len() > 1 || base.candidates_truncated {
            "ambiguous"
        } else {
            "proposal_only"
        }
        .into();
        base.reason="OpenStreetMap candidates require one unambiguous current name and an accepted country boundary containing the full reference geometry. Extract membership is not country evidence.".into();
    }
    candidates.truncate(20);
    base.candidates = candidates;
    Ok(base)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn record(geometry: &str, bounds: [f64; 4], classes: &[&str]) -> (Value, bool) {
        (json!({"geometry":{"type":geometry},"bounds":bounds,"classes":classes}), true)
    }
    #[test]
    fn one_feature_mapped_twice_merges_but_distinct_places_do_not() {
        // An airport's point and its outline, 2 km apart: one feature, outline wins.
        let twice = [
            record("Point", [51.31, 35.69, 51.31, 35.69], &["airport"]),
            record("Polygon", [51.29, 35.67, 51.33, 35.71], &["airport"]),
        ];
        assert_eq!(same_feature(&twice, &[0, 1]), Some(1));
        // Two villages with one name 100 km apart stay ambiguous.
        let apart = [
            record("Point", [35.5, 33.2, 35.5, 33.2], &["village"]),
            record("Point", [36.5, 33.9, 36.5, 33.9], &["village"]),
        ];
        assert_eq!(same_feature(&apart, &[0, 1]), None);
        // Nearby but different kinds of thing are not merged.
        let kinds = [
            record("Point", [51.31, 35.69, 51.31, 35.69], &["airport"]),
            record("Point", [51.32, 35.70, 51.32, 35.70], &["hospital"]),
        ];
        assert_eq!(same_feature(&kinds, &[0, 1]), None);
        assert!(is_place(&json!(["island"])));
    }
}
