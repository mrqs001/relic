//! Immutable selected facility and place releases. Activation replaces the selected set
//! atomically; historical objects and shapes remain available to old readings.
use crate::{
    error::{Error, Result},
    model::{hash, now, required, string},
    osm,
    store::{Store, edge, node},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    fs,
    sync::atomic::{AtomicBool, Ordering},
};
fn invalid() -> Error {
    Error::invalid("OSM reference failed validation; active facilities are unchanged")
}
#[derive(Clone)]
pub struct Index {
    pub edition: String,
    pub manifest: Value,
    pub manifest_hash: String,
    pub path: std::path::PathBuf,
}
fn quick_check(c: &Connection, cancel: &AtomicBool) -> Result<()> {
    let done = AtomicBool::new(false);
    let handle = c.get_interrupt_handle();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Acquire) {
                if cancel.load(Ordering::Acquire) {
                    handle.interrupt();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
        struct Finish<'a>(&'a AtomicBool);
        impl Drop for Finish<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let _finish = Finish(&done);
        let result = c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0));
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        if result? != "ok" {
            return Err(invalid());
        }
        Ok(())
    })
}
impl Index {
    pub fn matches(&self, name: &str) -> Result<Vec<Value>> {
        let c = Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        c.execute_batch("PRAGMA trusted_schema=OFF;")?;
        let mut q=c.prepare("SELECT f.id,f.version,f.timestamp,f.tags,f.classes,f.geometry,f.status,a.tag,a.name,b.min_lon,b.min_lat,b.max_lon,b.max_lat FROM features f JOIN aliases a ON a.feature=f.id LEFT JOIN feature_bounds b ON b.rowid=f.rowid WHERE a.key=?1 AND a.tag NOT IN('old_name','ref','iata','icao') AND a.rowid=(SELECT min(a2.rowid) FROM aliases a2 WHERE a2.feature=f.id AND a2.key=a.key AND a2.tag NOT IN('old_name','ref','iata','icao')) ORDER BY f.id LIMIT 21")?;
        let rows = q
            .query_map([osm::key(name)], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                    [
                        r.get::<_, Option<f64>>(9)?,
                        r.get(10)?,
                        r.get(11)?,
                        r.get(12)?,
                    ],
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter().map(|(id,version,timestamp,tags,classes,geometry,status,tag,name,bounds)| Ok(json!({"id":id,"version":version,"timestamp":timestamp,"tags":serde_json::from_str::<Value>(&tags)?,"classes":serde_json::from_str::<Value>(&classes)?,"geometry":geometry.map(|g|serde_json::from_str::<Value>(&g)).transpose()?,"status":status,"bounds":bounds,"alias":{"tag":tag,"name":name},"manifest_hash":self.manifest_hash,"raw_sha256":self.manifest["raw"]["sha256"],"source":self.manifest["selection"]["pbf"]["resolved_url"],"region":self.manifest["selection"]["feature"]["properties"]["id"],"snapshot_timestamp":self.manifest["header"]["replication_timestamp"]}))).collect()
    }
}
impl Store {
    pub fn osm(&self) -> Result<Option<Value>> {
        self.connect()?
            .query_row(
                "SELECT value FROM metadata WHERE key='active_osm'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|s| serde_json::from_str(&s).map_err(Error::from))
            .transpose()
    }
    pub fn osm_indexes(&self) -> Result<Vec<Index>> {
        let Some(active) = self.osm()? else {
            return Ok(vec![]);
        };
        active["editions"]
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .map(|e| {
                let h = required(e, "manifest_hash")?;
                let manifest: Value = serde_json::from_slice(&self.original_object(h)?)?;
                if !osm::MAPPINGS.contains(&manifest["mapping"].as_str().unwrap_or("")) {
                    return Err(invalid());
                }
                let path = self
                    .config
                    .object_path(required(&manifest["assets"]["index.sqlite3"], "sha256")?);
                Ok(Index {
                    edition: active["edition"].as_str().ok_or_else(invalid)?.into(),
                    manifest,
                    manifest_hash: h.into(),
                    path,
                })
            })
            .collect()
    }
    pub fn activate_osm(&self, paths: &[String], cancel: &AtomicBool) -> Result<Value> {
        if paths.is_empty() {
            return Err(invalid());
        }
        let root = self.config.root.canonicalize()?;
        // No count or size cap on the selected regions. Every asset is copied into
        // the object store and display outlines into the database, so the only
        // question is whether this disk can hold them.
        let mut manifests = vec![];
        let mut needed = 0u64;
        for path in paths {
            let dir = root.join(path).canonicalize()?;
            if !dir.starts_with(&root) {
                return Err(invalid());
            }
            let m: Value = serde_json::from_slice(&fs::read(dir.join("manifest.json"))?)?;
            for receipt in [&m["raw"], &m["index"], &m["selection"]["index"], &m["selection"]["checksum"]] {
                needed = needed.saturating_add(receipt["bytes"].as_u64().ok_or_else(invalid)?);
            }
            needed = needed.saturating_add(m["index"]["bytes"].as_u64().unwrap_or(0).saturating_mul(2));
            manifests.push((dir, m));
        }
        crate::disk::ensure(&self.config.data, needed, "Activating these OSM regions")?;
        let mut editions = vec![];
        let mut mappings = std::collections::BTreeSet::new();
        let mut assets = std::collections::BTreeMap::new();
        let mut regions = std::collections::BTreeSet::new();
        // Display outlines are staged on disk, never accumulated in memory.
        let staged = tempfile::NamedTempFile::new_in(self.config.data.join("staging"))?;
        let s = Connection::open(staged.path())?;
        s.execute_batch("PRAGMA journal_mode=OFF;PRAGMA synchronous=OFF;CREATE TABLE shapes(hash TEXT PRIMARY KEY,product TEXT NOT NULL,source_id TEXT NOT NULL,label TEXT NOT NULL,level TEXT NOT NULL,geometry TEXT NOT NULL,bounds TEXT NOT NULL,metadata TEXT NOT NULL,manifest_hash TEXT NOT NULL);BEGIN;")?;
        // All copying/validation precedes the writer transaction.
        for (dir, mut m) in manifests {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let mapping = m["mapping"].as_str().unwrap_or("");
            if m["schema"] != "relic.osm-reference.v1"
                || !osm::MAPPINGS.contains(&mapping)
                || m["license"] != "ODbL-1.0"
            {
                return Err(invalid());
            }
            mappings.insert(mapping.to_owned());
            let region = required(&m["selection"]["feature"]["properties"], "id")?.to_owned();
            if !regions.insert(region.clone()) {
                return Err(invalid());
            }
            let mut manifest_assets = serde_json::Map::new();
            for (name, receipt) in [
                ("source.osm.pbf", &m["raw"]),
                ("index.sqlite3", &m["index"]),
                ("index.json", &m["selection"]["index"]),
                ("source.md5", &m["selection"]["checksum"]),
            ] {
                let (h, n) =
                    self.retain_reference_asset_bounded(&dir.join(name), receipt, cancel, u64::MAX)?;
                manifest_assets.insert(name.into(), json!({"sha256":h,"bytes":n}));
                assets.insert(h, n);
            }
            let c = Connection::open_with_flags(
                self.config.object_path(required(&m["index"], "sha256")?),
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            c.execute_batch("PRAGMA trusted_schema=OFF;")?;
            quick_check(&c, cancel)?;
            let count: i64 = c.query_row("SELECT count(*) FROM features", [], |r| r.get(0))?;
            if count < 1 || Some(count) != m["selected_features"].as_i64() {
                return Err(invalid());
            }
            let orphan:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM aliases a LEFT JOIN features f ON f.id=a.feature WHERE f.id IS NULL)",[],|r|r.get(0))?;
            if orphan {
                return Err(invalid());
            }
            let mut q=c.prepare("SELECT id,version,timestamp,tags,geometry,status,classes FROM features WHERE geometry IS NOT NULL ORDER BY id")?;
            let mut rows = q.query([])?;
            while let Some(row) = rows.next()? {
                if cancel.load(Ordering::Acquire) {
                    return Err(Error::cancelled());
                }
                let raw: String = row.get(4)?;
                // Malformed-data guard: build keeps shapes within its vertex bound.
                if raw.len() > 8 * 1024 * 1024 {
                    return Err(invalid());
                }
                let geometry: Value = serde_json::from_str(&raw)?;
                if !["Polygon", "MultiPolygon"]
                    .iter()
                    .any(|t| geometry["type"] == *t)
                {
                    continue;
                }
                let area = crate::area_geometry::AreaGeometry::parse(&geometry, 100_000, cancel)?;
                let tags: Value = serde_json::from_str(&row.get::<_, String>(3)?)?;
                let classes: Value = serde_json::from_str(&row.get::<_, String>(6)?)?;
                let record = json!({"id":row.get::<_,String>(0)?,"version":row.get::<_,Option<i64>>(1)?,"timestamp":row.get::<_,Option<i64>>(2)?,"tags":tags,"geometry":geometry,"status":row.get::<_,String>(5)?,"snapshot_timestamp":m["header"]["replication_timestamp"]});
                let h = crate::osm_resolution::record_hash(&record);
                let metadata = json!({"boundaryYearRepresented":crate::osm_resolution::snapshot_time(&record),"object_modified_at_ms":record["timestamp"],"relic_record_sha256":h,"osm_identity":record["id"],"version":record["version"],"reference_only":true});
                let place = crate::osm_resolution::is_place(&classes);
                let label = record["tags"]["name:en"]
                    .as_str()
                    .or(record["tags"]["name"].as_str())
                    .unwrap_or(if place { "Named place" } else { "Named facility" });
                // Match the resolver's deterministic product order regardless
                // of the order selected by the operator. All memberships remain
                // in retained indexes and candidate provenance.
                s.prepare_cached("INSERT INTO shapes VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'') ON CONFLICT(hash) DO UPDATE SET product=excluded.product,source_id=excluded.source_id,label=excluded.label,level=excluded.level,geometry=excluded.geometry,bounds=excluded.bounds,metadata=excluded.metadata,manifest_hash='' WHERE excluded.product<shapes.product")?
                    .execute(params![h,region,required(&record,"id")?,label,if place {"PLACE"} else {"FACILITY"},raw,string(&json!(area.bounds)),string(&metadata)])?;
            }
            m["assets"] = Value::Object(manifest_assets);
            let bytes = serde_json::to_vec(&m)?;
            let h = self.write_object(&bytes)?;
            assets.insert(h.clone(), bytes.len() as u64);
            s.execute("UPDATE shapes SET manifest_hash=?1 WHERE product=?2", params![h, region])?;
            editions.push(json!({"product":region,"manifest_hash":h,"features":count}));
        }
        s.execute_batch("COMMIT;")?;
        drop(s);
        editions.sort_by_key(|e| e["product"].as_str().unwrap_or("").to_owned());
        // A single mapping keeps its earlier edition identity.
        let mapping = if mappings.len() == 1 { json!(mappings.first().unwrap()) } else { json!(mappings) };
        let edition = hash(string(&json!({"mapping":mapping,"editions":editions})).as_bytes());
        let active = json!({"edition":edition,"editions":editions,"activated_at":now(),"attribution":"© OpenStreetMap contributors, ODbL-1.0","coverage":"Selected named facilities and places; no inferred centroids or extract-based country assignment"});
        self.sync_object_directories(assets.keys().cloned(), cancel)?;
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        c.execute("ATTACH DATABASE ?1 AS staged", [staged.path().to_string_lossy()])?;
        let tx = c.transaction()?;
        for (h, n) in assets {
            tx.execute(
                "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/octet-stream')",
                params![h, n as i64],
            )?;
        }
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        tx.execute("INSERT OR IGNORE INTO boundary_shapes SELECT 'osm:'||?1||':'||hash,?1,'','',level,source_id,label,geometry,geometry,bounds,metadata,manifest_hash FROM staged.shapes", [&edition])?;
        let id = format!("osm:{edition}");
        node(
            &tx,
            &id,
            "reference_edition",
            "Selected OSM facilities and places",
            &active,
        )?;
        for e in active["editions"].as_array().unwrap() {
            let input = format!("reference:{}", required(e, "manifest_hash")?);
            node(&tx, &input, "reference_input", required(e, "product")?, e)?;
            edge(&tx, &input, &id, "reference_input", e)?;
        }
        crate::boundary_store::record_context(
            &tx,
            self.geography()?.as_ref(),
            self.boundaries()?.as_ref(),
            self.marine()?.as_ref(),
            Some(&active),
        )?;
        tx.execute("INSERT INTO metadata VALUES('active_osm',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[string(&active)])?;
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        tx.commit()?;
        c.execute("DETACH DATABASE staged", [])?;
        Ok(active)
    }
}
