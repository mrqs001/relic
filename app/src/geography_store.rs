//! Workspace-owned, immutable reference editions. Large copies precede the short
//! pointer transaction; every input belongs to the existing backup object store.
use crate::{
    error::{Error, Result},
    geonames::Index,
    media_store::file_hash_cancellable,
    model::{hash, now, required, string},
    store::{Store, edge, node},
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
pub const RESOLVER_VERSION: &str = "relic.geography.v3";
fn invalid() -> Error {
    Error::invalid("Reference edition failed activation validation; active geography is unchanged")
}
fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::cancelled())
    } else {
        Ok(())
    }
}
fn valid_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())
}
impl Store {
    pub fn geography(&self) -> Result<Option<Value>> {
        self.connect()?
            .query_row(
                "SELECT value FROM metadata WHERE key='active_geography'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|s| serde_json::from_str(&s).map_err(Error::from))
            .transpose()
    }
    pub fn geography_indexes(&self) -> Result<Vec<Index>> {
        self.geography_indexes_cancellable(&AtomicBool::new(false))
    }
    pub fn geography_indexes_cancellable(&self, cancel: &AtomicBool) -> Result<Vec<Index>> {
        check(cancel)?;
        let Some(active) = self.geography()? else {
            return Ok(vec![]);
        };
        if active["resolver_version"] != RESOLVER_VERSION {
            return Err(invalid());
        }
        let editions = active["editions"]
            .as_array()
            .filter(|v| !v.is_empty())
            .ok_or_else(invalid)?;
        editions
            .iter()
            .map(|edition| {
                check(cancel)?;
                let h = required(edition, "manifest_hash")?;
                let manifest: Value = serde_json::from_slice(&self.original_object(h)?)?;
                let index_hash = required(&manifest["assets"]["index.sqlite3"], "sha256")?;
                if !valid_hash(index_hash) {
                    return Err(invalid());
                }
                Index::stored(
                    manifest.clone(),
                    h.into(),
                    self.config.object_path(index_hash),
                    cancel,
                )
            })
            .collect()
    }
    pub(crate) fn retain_reference_asset(
        &self,
        path: &Path,
        expected: &Value,
        cancel: &AtomicBool,
    ) -> Result<(String, u64)> {
        self.retain_reference_asset_bounded(path, expected, cancel, u64::MAX)
    }
    pub(crate) fn retain_reference_asset_bounded(
        &self,
        path: &Path,
        expected: &Value,
        cancel: &AtomicBool,
        max_asset: u64,
    ) -> Result<(String, u64)> {
        check(cancel)?;
        let expected_hash = required(expected, "sha256")?;
        let size = expected["bytes"]
            .as_u64()
            .filter(|n| *n <= max_asset)
            .ok_or_else(invalid)?;
        if !valid_hash(expected_hash) || fs::metadata(path)?.len() != size {
            return Err(invalid());
        }
        let mut input = fs::File::open(path)?;
        let mut stage = tempfile::NamedTempFile::new_in(self.config.data.join("staging"))?;
        let mut digest = sha2::Sha256::new();
        use sha2::Digest;
        let mut total = 0u64;
        let mut bytes = [0; 64 * 1024];
        loop {
            check(cancel)?;
            let n = input.read(&mut bytes)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > size {
                return Err(invalid());
            }
            digest.update(&bytes[..n]);
            stage.write_all(&bytes[..n])?;
        }
        let actual = format!("{:x}", digest.finalize());
        if actual != expected_hash || total != size {
            return Err(invalid());
        }
        stage.as_file().sync_all()?;
        check(cancel)?;
        let dest = self.config.object_path(&actual);
        fs::create_dir_all(dest.parent().ok_or_else(invalid)?)?;
        match stage.persist_noclobber(&dest) {
            Ok(_) => {}
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                if file_hash_cancellable(&dest, cancel)? != (actual.clone(), total) {
                    return Err(invalid());
                }
            }
            Err(_) => return Err(Error::internal()),
        }
        Ok((actual, total))
    }
    pub fn activate_geography(&self, paths: &[String], cancel: &AtomicBool) -> Result<Value> {
        if paths.is_empty() {
            return Err(invalid());
        }
        let root = self.config.root.canonicalize()?;
        let mut editions = vec![];
        let mut objects = std::collections::BTreeMap::new();
        let mut products = std::collections::BTreeSet::new();
        let mut total = 0u64;
        for path in paths {
            check(cancel)?;
            let dir = root.join(path).canonicalize()?;
            if !dir.starts_with(&root) {
                return Err(invalid());
            }
            let manifest_path = dir.join("manifest.json");
            if fs::metadata(&manifest_path)?.len() > 64 * 1024 {
                return Err(invalid());
            }
            let bytes = fs::read(&manifest_path)?;
            let m: Value = serde_json::from_slice(&bytes)?;
            let product = required(&m["acquisition"], "product")?;
            if m["schema_version"] != 1
                || m["status"] != "candidate"
                || m["publisher"] != "GeoNames"
                || m["license"] != "CC-BY-4.0"
                || m["format"] != "geoname-19/alternateNamesV2-10"
                || (product != "cities500"
                    && (product.len() != 2 || !product.bytes().all(|b| b.is_ascii_uppercase())))
                || !products.insert(product.to_string())
            {
                return Err(invalid());
            }
            let assets = m["assets"]
                .as_object()
                .filter(|v| v.len() <= 6)
                .ok_or_else(invalid)?;
            for required in [
                "places.txt",
                "alternateNamesV2.txt",
                "index.sqlite3",
                "places.zip",
                "readme.txt",
            ] {
                if !assets.contains_key(required) {
                    return Err(invalid());
                }
            }
            // Older global editions carry no alternate-name file (two requests);
            // newer ones join the worldwide language-tagged file.
            let tagged = product != "cities500" || assets.contains_key("aliases.zip");
            if tagged && !assets.contains_key("aliases.zip") {
                return Err(invalid());
            }
            let requests = m["acquisition"]["requests"]
                .as_array()
                .filter(|a| a.len() == if tagged { 3 } else { 2 })
                .ok_or_else(invalid)?;
            let expected_urls = vec![
                format!("https://download.geonames.org/export/dump/{product}.zip"),
                "https://download.geonames.org/export/dump/readme.txt".into(),
                if product == "cities500" {
                    "https://download.geonames.org/export/dump/alternateNamesV2.zip".into()
                } else {
                    format!("https://download.geonames.org/export/dump/alternatenames/{product}.zip")
                },
            ];
            for (request, url) in requests.iter().zip(expected_urls) {
                if request["url"] != url || request["acquired_at"].as_str().is_none() {
                    return Err(invalid());
                }
            }
            for (name, asset) in assets {
                if ![
                    "places.txt",
                    "alternateNamesV2.txt",
                    "index.sqlite3",
                    "places.zip",
                    "aliases.zip",
                    "readme.txt",
                ]
                .contains(&name.as_str())
                {
                    return Err(invalid());
                }
                let asset_path = dir.join(name).canonicalize()?;
                if !asset_path.starts_with(&dir) {
                    return Err(invalid());
                }
                total = total
                    .checked_add(asset["bytes"].as_u64().ok_or_else(invalid)?)
                    .ok_or_else(invalid)?;
                // No total cap: only this disk's free space limits a selection.
                crate::disk::ensure(&self.config.data, total, "Activating these GeoNames editions")?;
                let (h, n) = self.retain_reference_asset(&asset_path, asset, cancel)?;
                objects.insert(h, n);
            }
            let h = self.write_object(&bytes)?;
            objects.insert(h.clone(), bytes.len() as u64);
            editions.push(json!({"product":product,"manifest_hash":h,"places":m["place_count"],"aliases":m["indexed_aliases"],"countries":m["countries"],"excluded_empty_aliases":m["empty_aliases_excluded"],"unjoined_aliases":m["unjoined_aliases"]}));
        }
        editions.sort_by_key(|e| e["product"].as_str().unwrap().to_owned());
        let edition = hash(&serde_json::to_vec(
            &json!({"resolver_version":RESOLVER_VERSION,"editions":editions}),
        )?);
        let active = json!({"edition":edition,"resolver_version":RESOLVER_VERSION,"editions":editions,"activated_at":now(),"attribution":"GeoNames, CC BY 4.0; Natural Earth public domain","coverage":"Exact locality names and language-tagged aliases. Countries, administrative areas and facilities retain context only; locality coordinates are approximate. Ambiguous names remain unresolved.","input_bytes":total});
        self.sync_object_directories(objects.keys().cloned(), cancel)?;
        // Validate SQLite integrity/schema before making this edition queryable.
        for e in active["editions"].as_array().unwrap() {
            check(cancel)?;
            let m: Value = serde_json::from_slice(&fs::read(
                self.config.object_path(required(e, "manifest_hash")?),
            )?)?;
            let c = rusqlite::Connection::open_with_flags(
                self.config
                    .object_path(required(&m["assets"]["index.sqlite3"], "sha256")?),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            if c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))? != "ok" {
                return Err(invalid());
            }
            let count: i64 = c.query_row("SELECT count(*) FROM places", [], |r| r.get(0))?;
            if m["place_count"].as_i64() != Some(count) {
                return Err(invalid());
            }
            if let Some(expected) = m["possible_name_count"].as_i64() {
                let actual: i64 =
                    c.query_row("SELECT count(*) FROM possible_names", [], |r| r.get(0))?;
                if expected != actual {
                    return Err(invalid());
                }
            }
        }
        check(cancel)?;
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let boundaries = self.boundaries()?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        for (h, n) in objects {
            tx.execute(
                "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/octet-stream')",
                params![h, n as i64],
            )?;
        }
        let id = format!("geography:{edition}");
        node(
            &tx,
            &id,
            "reference_edition",
            "Activated geographic reference",
            &active,
        )?;
        for e in active["editions"].as_array().unwrap() {
            let h = required(e, "manifest_hash")?;
            let input = format!("reference:{h}");
            node(&tx, &input, "reference_input", required(e, "product")?, e)?;
            edge(
                &tx,
                &input,
                &id,
                "reference_input",
                &json!({"manifest_hash":h}),
            )?;
        }
        crate::boundary_store::record_context(
            &tx,
            Some(&active),
            boundaries.as_ref(),
            self.marine()?.as_ref(),
            self.osm()?.as_ref(),
        )?;
        tx.execute("INSERT INTO metadata VALUES('active_geography',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[string(&active)])?;
        check(cancel)?;
        tx.commit()?;
        Ok(active)
    }
}

/// One existing import worker, cancellable through /jobs/{id}/cancel. Large
/// reference work never holds a query worker or the SQLite writer while copying.
pub async fn start_job<F>(store: Store, kind: &str, work: F) -> Result<Value>
where
    F: FnOnce(&Store, &str, &AtomicBool) -> Result<Value> + Send + 'static,
{
    if store.shutdown.load(Ordering::Acquire) {
        return Err(Error::busy());
    }
    let permit = store
        .imports
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::busy())?;
    let id = crate::model::ident(kind);
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    store
        .running
        .lock()
        .map_err(|_| Error::internal())?
        .insert(id.clone(), cancel.clone());
    let recovery = kind == "processing-recovery";
    let job = json!({"id":id,"kind":kind,"status":"running","accepted":0,"total":0,"created_at":now(),"detail":if recovery {"Rechecking retained responses without new model calls."} else {"Updating geographic reference; the current map remains available."}});
    if let Err(error) = store.save_job(&job) {
        if let Ok(mut running) = store.running.lock() {
            running.remove(&id);
        }
        return Err(error);
    }
    let response = job.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut job = job;
        match work(&store, &id, &cancel) {
            Ok(result) => {
                let partial = result["complete"] == false;
                job["status"] = json!(if partial { "partial" } else { "completed" });
                let count = result["recovered"]
                    .as_u64()
                    .or(result["resolved_readings"].as_u64())
                    .unwrap_or_else(|| result["editions"].as_array().map_or(0, |v| v.len() as u64));
                job["accepted"] = json!(count);
                let skipped = result["conflicting_readings"].as_u64().unwrap_or(0)
                    + result["changed_during_run"].as_u64().unwrap_or(0);
                job["total"] = result
                    .get("examined")
                    .cloned()
                    .unwrap_or(json!(count + skipped));
                job["result"] = result;
                job["detail"] = json!(if partial {
                    format!(
                        "Geographic update partial: {count} readings updated; {skipped} retained their previous reading because source support or current membership changed."
                    )
                } else if recovery {
                    "Saved responses rechecked. Recovered readings are ready for publication."
                        .into()
                } else {
                    "Geographic update complete.".into()
                });
            }
            Err(e) => {
                job["status"] = json!(if e.code == "cancelled" {
                    "cancelled"
                } else {
                    "failed"
                });
                job["detail"] = json!(e.message);
            }
        }
        let outcome = store.save_job(&job);
        if let Ok(mut running) = store.running.lock() {
            running.remove(&id);
        }
        outcome
    });
    Ok(response)
}
