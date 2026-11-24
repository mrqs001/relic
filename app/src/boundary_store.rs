//! Accepted area editions share Relic's object store and publication provenance.
use crate::{
    boundary_reference::{self, invalid},
    error::{Error, Result},
    model::{hash, now, required, string},
    store::{Store, edge, node},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    fs,
    sync::atomic::{AtomicBool, Ordering},
};

pub fn combined_edition(
    geography: Option<&Value>,
    boundaries: Option<&Value>,
    marine: Option<&Value>,
    osm: Option<&Value>,
) -> Option<String> {
    if geography.is_none() && boundaries.is_none() && marine.is_none() && osm.is_none() {
        return None;
    }
    let mut context = json!({"policy":"relic.location-context.v20","corrections":crate::processing_geo::corrections_hash(),"marine_features":crate::processing_geo::marine_features_hash(),"places":geography.map(|g|&g["edition"]),"areas":boundaries.map(|b|&b["edition"]),"marine":marine.map(|m|&m["edition"])});
    if let Some(osm) = osm {
        context["osm"] = osm["edition"].clone();
    }
    Some(hash(string(&context).as_bytes()))
}
pub(crate) fn record_context(
    c: &Connection,
    geography: Option<&Value>,
    boundaries: Option<&Value>,
    marine: Option<&Value>,
    osm: Option<&Value>,
) -> Result<()> {
    if geography.is_none() && boundaries.is_none() && marine.is_none() && osm.is_none() {
        return Ok(());
    }
    let id = format!(
        "geography:{}",
        combined_edition(geography, boundaries, marine, osm).unwrap()
    );
    node(
        c,
        &id,
        "reference_edition",
        "Places and geographic areas",
        &json!({"places":geography,"areas":boundaries,"marine":marine,"osm":osm}),
    )?;
    for (prefix, active, role) in [
        ("geography", geography, "place_names"),
        ("boundaries", boundaries, "analytical_areas"),
        ("marine", marine, "marine_areas"),
        ("osm", osm, "facilities"),
    ] {
        if let Some(active) = active {
            edge(
                c,
                &format!("{prefix}:{}", required(active, "edition")?),
                &id,
                "reference_input",
                &json!({"role":role}),
            )?;
        }
    }
    Ok(())
}
#[derive(Clone)]
pub struct AreaRecord {
    pub id: String,
    pub name: String,
    pub iso2: String,
    pub level: String,
    pub bounds: Value,
    pub metadata: Value,
    pub source_sha256: String,
    pub record_sha256: String,
    pub edition: String,
    pub shape_id: String,
}
impl Store {
    pub fn boundaries(&self) -> Result<Option<Value>> {
        self.connect()?
            .query_row(
                "SELECT value FROM metadata WHERE key='active_boundaries'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|s| serde_json::from_str(&s).map_err(Error::from))
            .transpose()
    }
    pub fn area_records(&self) -> Result<Vec<AreaRecord>> {
        let editions = [self.boundaries()?, self.marine()?]
            .into_iter()
            .flatten()
            .filter_map(|a| a["edition"].as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        let c = self.connect()?;
        let mut stmt=c.prepare("SELECT id,name,iso2,level,bounds,metadata,manifest_hash,edition,shape_id FROM boundary_shapes WHERE edition IN (SELECT value FROM json_each(?)) ORDER BY id")?;
        let rows = stmt
            .query_map([string(&json!(editions))], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(id, name, iso2, level, bounds, metadata, _manifest_hash, edition, shape_id)| {
                    Ok(AreaRecord {
                        id,
                        name,
                        iso2,
                        level,
                        bounds: serde_json::from_str(&bounds)?,
                        metadata: serde_json::from_str(&metadata)?,
                        source_sha256:
                            serde_json::from_str::<Value>(&metadata)?["relic_source_sha256"]
                                .as_str()
                                .ok_or_else(invalid)?
                                .into(),
                        record_sha256:
                            serde_json::from_str::<Value>(&metadata)?["relic_record_sha256"]
                                .as_str()
                                .ok_or_else(invalid)?
                                .into(),
                        edition,
                        shape_id,
                    })
                },
            )
            .collect()
    }
    pub fn activate_boundaries(&self, paths: &[String], cancel: &AtomicBool) -> Result<Value> {
        if paths.is_empty() {
            return Err(invalid());
        }
        let root = self.config.root.canonicalize()?;
        // No edition-count or size cap. Assets are copied into the object store
        // and shapes into the database twice (full and display), so only free
        // disk space is checked; parsed shapes are staged on disk, not in memory.
        let mut needed = 0u64;
        for path in paths {
            let m: Value = serde_json::from_slice(&fs::read(root.join(path).join("manifest.json"))?)?;
            for name in ["metadata.json", "full.geojson", "display.geojson"] {
                needed = needed.saturating_add(m["assets"][name]["bytes"].as_u64().unwrap_or(0).saturating_mul(3));
            }
        }
        crate::disk::ensure(&self.config.data, needed, "Activating these boundaries")?;
        let mut products = std::collections::BTreeSet::new();
        let mut objects = std::collections::BTreeMap::new();
        let mut editions = vec![];
        let mut features = 0usize;
        let staged = tempfile::NamedTempFile::new_in(self.config.data.join("staging"))?;
        let s = rusqlite::Connection::open(staged.path())?;
        s.execute_batch("PRAGMA journal_mode=OFF;PRAGMA synchronous=OFF;CREATE TABLE shapes(shape_id TEXT NOT NULL,iso2 TEXT NOT NULL,iso3 TEXT NOT NULL,level TEXT NOT NULL,name TEXT NOT NULL,full_json TEXT NOT NULL,display_json TEXT NOT NULL,bounds TEXT NOT NULL,metadata TEXT NOT NULL,manifest_hash TEXT NOT NULL);BEGIN;")?;
        // Accepted country codes supply the namespace crosswalk; names never do.
        let reference = root.join("reference/accepted/natural-earth");
        let accepted: Value =
            serde_json::from_slice(&fs::read(reference.join("processing-manifest.json"))?)?;
        // Both accepted identity files: the main countries, then tiny countries
        // (Bahrain, Singapore, …) that the 1:110m country file omits.
        let mut country_features = vec![];
        for country_file in [
            "raw/ne_110m_admin_0_countries.geojson",
            "raw/ne_110m_admin_0_tiny_countries.geojson",
        ] {
            let file = accepted["country_identity"]["source_files"]
                .as_array()
                .ok_or_else(invalid)?
                .iter()
                .find(|f| f["path"] == country_file)
                .ok_or_else(invalid)?;
            let country_bytes = fs::read(reference.join(country_file))?;
            if hash(&country_bytes) != required(file, "sha256")?
                || file["bytes"].as_u64() != Some(country_bytes.len() as u64)
            {
                return Err(invalid());
            }
            let parsed: Value = serde_json::from_slice(&country_bytes)?;
            country_features.extend(parsed["features"].as_array().ok_or_else(invalid)?.iter().cloned());
        }
        for path in paths {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let dir = root.join(path).canonicalize()?;
            if !dir.starts_with(&root) {
                return Err(invalid());
            }
            if fs::metadata(dir.join("manifest.json"))?.len() > 64 * 1024 {
                return Err(invalid());
            }
            let manifest_bytes = fs::read(dir.join("manifest.json"))?;
            if manifest_bytes.len() > 64 * 1024 {
                return Err(invalid());
            }
            let m: Value = serde_json::from_slice(&manifest_bytes)?;
            let iso = required(&m, "country")?;
            let tier = required(&m, "level")?;
            if m["schema_version"] != 1
                || m["status"] != "candidate"
                || m["publisher"] != "geoBoundaries"
                || m["collection"] != "gbOpen"
                || !products.insert(format!("{iso}-{tier}"))
            {
                return Err(invalid());
            }
            let code = country_features
                .iter()
                .find(|f| f["properties"]["ADM0_A3"] == iso || f["properties"]["ISO_A3_EH"] == iso)
                .ok_or_else(|| {
                    Error::invalid(format!(
                        "{iso} is not in the accepted country identity reference; prior areas remain active"
                    ))
                })?;
            let p = &code["properties"];
            let iso2 = [&p["ISO_A2"], &p["ISO_A2_EH"]]
                .into_iter()
                .filter_map(Value::as_str)
                .find(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_uppercase()))
                .ok_or_else(invalid)?
                .to_owned();
            let mut assets = vec![];
            for name in ["metadata.json", "full.geojson", "display.geojson"] {
                let path = dir.join(name).canonicalize()?;
                let expected = &m["assets"][name];
                let size = fs::metadata(&path)?.len();
                if !path.starts_with(&dir) || expected["bytes"].as_u64() != Some(size) {
                    return Err(invalid());
                }
                let bytes = fs::read(path)?;
                if hash(&bytes) != required(expected, "sha256")? {
                    return Err(invalid());
                }
                assets.push(bytes);
            }
            let metadata: Value = serde_json::from_slice(&assets[0])?;
            if metadata["boundaryISO"] != iso
                || metadata["boundaryType"] != tier
                || metadata["boundaryID"] != m["boundary_id"]
            {
                return Err(invalid());
            }
            let review = boundary_reference::reconciliation(&assets)?;
            if m["reconciliation"] != review.clone().unwrap_or(Value::Null) {
                return Err(invalid());
            }
            let shapes = boundary_reference::validate_assets(&assets, cancel)?;
            if shapes.len() != m["features"].as_u64().ok_or_else(invalid)? as usize {
                return Err(invalid());
            }
            for bytes in assets {
                let h = self.write_object(&bytes)?;
                objects.insert(h, bytes.len());
            }
            let mh = self.write_object(&manifest_bytes)?;
            objects.insert(mh.clone(), manifest_bytes.len());
            editions.push(json!({"product":format!("{iso}-{tier}"),"manifest_hash":mh,"features":shapes.len(),"represented_time":metadata["boundaryYearRepresented"]}));
            features += shapes.len();
            for shape in shapes {
                let mut record_metadata = metadata.clone();
                if let Some(review) = &review {
                    record_metadata["relic_reconciliation"] = review.clone();
                }
                record_metadata["relic_source_sha256"] =
                    m["assets"]["full.geojson"]["sha256"].clone();
                record_metadata["relic_record_sha256"] = json!(shape.record_sha256);
                s.prepare_cached("INSERT INTO shapes VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")?.execute(params![shape.id,iso2,iso,tier,shape.name,string(&shape.full),string(&shape.display),string(&json!(shape.bounds)),string(&record_metadata),mh])?;
            }
        }
        s.execute_batch("COMMIT;")?;
        drop(s);
        editions.sort_by_key(|v| v["product"].as_str().unwrap().to_owned());
        let edition =
            hash(string(&json!({"policy":"relic.areas.v1","editions":editions})).as_bytes());
        let active = json!({"edition":edition,"editions":editions,"features":features,"activated_at":now(),"attribution":"geoBoundaries / William & Mary. Publisher boundaries and represented years; not live legal geography."});
        self.sync_object_directories(objects.keys().cloned(), cancel)?;
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let geography = self.geography()?;
        let mut c = self.connect()?;
        c.execute("ATTACH DATABASE ?1 AS staged", [staged.path().to_string_lossy()])?;
        let tx = c.transaction()?;
        for (h, n) in objects {
            tx.execute(
                "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
                params![h, n as i64],
            )?;
        }
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        tx.execute("INSERT OR IGNORE INTO boundary_shapes SELECT 'gbOpen:'||?1||':'||shape_id,?1,iso2,iso3,level,shape_id,name,full_json,display_json,bounds,metadata,manifest_hash FROM staged.shapes", [&edition])?;
        let node_id = format!("boundaries:{edition}");
        node(
            &tx,
            &node_id,
            "reference_edition",
            "Accepted geographic areas",
            &active,
        )?;
        for e in active["editions"].as_array().unwrap() {
            let input = format!("reference:{}", required(e, "manifest_hash")?);
            node(&tx, &input, "reference_input", required(e, "product")?, e)?;
            edge(&tx, &input, &node_id, "reference_input", e)?;
        }
        record_context(
            &tx,
            geography.as_ref(),
            Some(&active),
            self.marine()?.as_ref(),
            self.osm()?.as_ref(),
        )?;
        tx.execute("INSERT INTO metadata VALUES('active_boundaries',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[string(&active)])?;
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        tx.commit()?;
        c.execute("DETACH DATABASE staged", [])?;
        Ok(active)
    }
}
