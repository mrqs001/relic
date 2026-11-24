//! Prepare an empty replacement workspace from explicit setup and reference data.
//! Never changes the source or installs the replacement; the operator stops the
//! application and swaps directories only after validating the prepared result.
use crate::{
    config::Config,
    error::{Error, Result},
    model::{required, string},
    store::Store,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::Path,
};

pub fn prepare(config: Config, source: &Path) -> Result<Value> {
    if config.data.exists() {
        return Err(Error::invalid("Fresh workspace destination must not exist"));
    }
    let source = source.canonicalize()?;
    let old = Connection::open_with_flags(
        source.join("relic.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let active = [
        "active_geography",
        "active_boundaries",
        "active_marine",
        "active_osm",
    ]
    .map(|key| {
        old.query_row("SELECT value FROM metadata WHERE key=?1", [key], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map_err(Error::from)
        .and_then(|s| {
            s.map(|s| serde_json::from_str::<Value>(&s).map_err(Error::from))
                .transpose()
        })
    });
    let [geography, boundaries, marine, osm] = active;
    let geography = geography?;
    let boundaries = boundaries?;
    let marine = marine?;
    let osm = osm?;
    let mut hashes = BTreeSet::new();
    let mut nodes = BTreeSet::new();
    for (prefix, active) in [
        ("geography", &geography),
        ("boundaries", &boundaries),
        ("marine", &marine),
        ("osm", &osm),
    ] {
        if let Some(active) = active {
            nodes.insert(format!("{prefix}:{}", required(active, "edition")?));
            for edition in active["editions"].as_array().ok_or_else(Error::internal)? {
                let h = required(edition, "manifest_hash")?;
                check_hash(h)?;
                hashes.insert(h.to_owned());
                nodes.insert(format!("reference:{h}"));
                let path = source.join("objects/sha256").join(&h[..2]).join(h);
                if fs::metadata(&path)?.len() > 2 * 1024 * 1024 {
                    return Err(Error::invalid("Reference manifest is oversized"));
                }
                let manifest: Value = serde_json::from_slice(&fs::read(path)?)?;
                for asset in manifest["assets"]
                    .as_object()
                    .ok_or_else(Error::internal)?
                    .values()
                {
                    let h = required(asset, "sha256")?;
                    check_hash(h)?;
                    hashes.insert(h.to_owned());
                }
            }
        }
    }
    if let Some(edition) = crate::boundary_store::combined_edition(
        geography.as_ref(),
        boundaries.as_ref(),
        marine.as_ref(),
        osm.as_ref(),
    ) {
        nodes.insert(format!("geography:{edition}"));
    }
    let store = Store::open(config)?;
    let mut c = store.connect()?;
    let tx = c.transaction()?;
    let uri = url::Url::from_file_path(source.join("relic.sqlite3"))
        .map_err(|_| Error::invalid("Invalid source path"))?;
    tx.execute("ATTACH DATABASE ?1 AS previous", [format!("{uri}?mode=ro")])?;
    let mut bytes = 0u64;
    for h in &hashes {
        let (size, mime): (i64, String) = old.query_row(
            "SELECT bytes,media_type FROM objects WHERE hash=?1",
            [h],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let size = u64::try_from(size).map_err(|_| Error::invalid("Invalid reference size"))?;
        bytes += size;
        if bytes > 4 * 1024 * 1024 * 1024 {
            return Err(Error::invalid("Reference copy exceeds 4 GiB"));
        }
        let from = source.join("objects/sha256").join(&h[..2]).join(h);
        let to = store.config.object_path(h);
        fs::create_dir_all(to.parent().ok_or_else(Error::internal)?)?;
        let mut input = fs::File::open(from)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)?;
        let mut buffer = [0u8; 64 * 1024];
        use sha2::Digest;
        let mut digest = sha2::Sha256::new();
        let mut copied = 0u64;
        loop {
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            copied += n as u64;
            if copied > size {
                return Err(Error::invalid("Reference byte count changed"));
            }
            digest.update(&buffer[..n]);
            output.write_all(&buffer[..n])?;
        }
        output.sync_all()?;
        if copied != size || format!("{:x}", digest.finalize()) != *h {
            return Err(Error::invalid("Reference checksum mismatch"));
        }
        tx.execute(
            "INSERT INTO objects VALUES(?1,?2,?3)",
            params![h, size as i64, mime],
        )?;
    }
    tx.execute_batch("INSERT INTO subscriptions SELECT * FROM previous.subscriptions;
        INSERT INTO sources SELECT * FROM previous.sources;
        INSERT INTO metadata SELECT key,value FROM previous.metadata WHERE key IN
        ('active_geography','active_boundaries','active_marine','active_osm','collection:telegram:active','collection:x:active',
        'collection:telegram:retry_at','collection:x:retry_at','processing_eligibility','live_workspace');")?;
    // Actual account cooldowns survive a content reset. Checkpoints, cursor state,
    // previous readings, AI receipts and old evaluation approvals do not.
    for active in [&boundaries, &marine, &osm].into_iter().flatten() {
        tx.execute(
            "INSERT INTO boundary_shapes SELECT * FROM previous.boundary_shapes WHERE edition=?1",
            [required(active, "edition")?],
        )?;
    }
    for id in &nodes {
        tx.execute(
            "INSERT INTO lineage_nodes SELECT * FROM previous.lineage_nodes WHERE id=?1",
            [id],
        )?;
    }
    tx.execute("INSERT INTO lineage_edges SELECT p.* FROM previous.lineage_edges p JOIN lineage_nodes s ON s.id=p.source JOIN lineage_nodes t ON t.id=p.target",[])?;
    let policy: Option<String> = old
        .query_row(
            "SELECT value FROM metadata WHERE key='processing_policy'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(policy) = policy {
        let mut policy: Value = serde_json::from_str(&policy)?;
        policy["enabled"] = json!(false);
        tx.execute(
            "INSERT INTO metadata VALUES('processing_policy',?1)",
            [string(&policy)],
        )?;
    }
    store.sync_object_directories(
        hashes.iter().cloned(),
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    tx.commit()?;
    c.execute_batch("DETACH DATABASE previous")?;
    let subscriptions: i64 = c.query_row("SELECT count(*) FROM subscriptions", [], |r| r.get(0))?;
    let areas: i64 = c.query_row("SELECT count(*) FROM boundary_shapes", [], |r| r.get(0))?;
    let foreign_keys: i64 =
        c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })?;
    if foreign_keys != 0 {
        return Err(Error::invalid("Prepared workspace has broken references"));
    }
    store.geography_indexes()?;
    store.area_records()?;
    c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    Ok(
        json!({"subscriptions":subscriptions,"boundary_shapes":areas,"reference_objects":hashes.len(),"reference_bytes":bytes,"messages":0,"readings":0,"collection_checkpoints":0,"processing_enabled":false,"foreign_key_errors":foreign_keys}),
    )
}
fn check_hash(h: &str) -> Result<()> {
    if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::invalid("Invalid reference hash"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_setup_but_clears_evidence_checkpoints_approvals_and_attempts() {
        let root = tempfile::tempdir().unwrap();
        let config = |name: &str| {
            Config::local(
                root.path().to_path_buf(),
                root.path().join(name),
                "127.0.0.1:0".parse().unwrap(),
            )
        };
        let old = Store::open(config("old")).unwrap();
        old.create_subscription(json!({"provider":"telegram","label":"Fictional channel","target":"@fictional_channel","enabled":true})).unwrap();
        let c = old.connect().unwrap();
        c.execute_batch("INSERT INTO metadata VALUES('processing_evaluation','{\"passed\":true}'),('collection:telegram:active','true'),('processing_policy','{\"enabled\":true,\"eligible_source_ids\":[]}'); INSERT INTO sources VALUES('fictional','{}'); INSERT INTO jobs VALUES('old-job','{}'); INSERT INTO publications VALUES('old-publication','old','2026-01-01','complete',NULL);").unwrap();
        c.execute_batch("INSERT INTO objects VALUES('old-object',4,'text/plain'); INSERT INTO evidence VALUES(1,'old-message','old-revision','fictional','2026-01-01',NULL,NULL,NULL,NULL,'old text','old-object','{}'); INSERT INTO membership VALUES('old-publication',1); INSERT INTO processing_attempts VALUES('old-attempt',1,'2026-01-01','2026-01-01','failed',0.05,NULL,'{}');").unwrap();
        let before: i64 = c
            .query_row("SELECT count(*) FROM publications", [], |r| r.get(0))
            .unwrap();
        let result = prepare(config("new"), &root.path().join("old")).unwrap();
        assert_eq!(result["subscriptions"], 1);
        assert_eq!(result["messages"], 0);
        let new = Connection::open(root.path().join("new/relic.sqlite3")).unwrap();
        for table in [
            "jobs",
            "publications",
            "evidence",
            "collection_states",
            "processing_attempts",
            "supplemental_attempts",
        ] {
            assert_eq!(
                new.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        assert_eq!(
            new.query_row(
                "SELECT count(*) FROM metadata WHERE key='processing_evaluation'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert!(!new.query_row("SELECT json_extract(value,'$.enabled') FROM metadata WHERE key='processing_policy'",[],|r|r.get::<_,bool>(0)).unwrap());
        assert_eq!(
            c.query_row("SELECT count(*) FROM publications", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            before
        );
        assert!(prepare(config("new"), &root.path().join("old")).is_err());
    }
}
