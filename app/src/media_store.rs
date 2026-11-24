//! Bounded, immutable media retained for an exact evidence revision.
//! Source adapters own all network work; no provider URL enters this API.
use crate::{
    error::{Error, Result},
    model::{now, required},
    store::{Store, edge, node},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tempfile::NamedTempFile;

pub const MAX_MEDIA_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_MEDIA_FILE_BYTES: usize = relic_telegram::MAX_STREAM_MEDIA_BYTES;

pub(crate) fn migrate_media_bound(c: &Connection) -> Result<()> {
    let schema: String = c.query_row(
        "SELECT sql FROM sqlite_master WHERE name='media_assets'",
        [],
        |r| r.get(0),
    )?;
    if schema.contains("8388608") {
        let tx = c.unchecked_transaction()?;
        tx.execute_batch("CREATE TABLE media_assets_streaming(
            evidence_id TEXT NOT NULL, revision TEXT NOT NULL, asset_index INTEGER NOT NULL CHECK(asset_index >= 0 AND asset_index < 100),
            hash TEXT NOT NULL REFERENCES objects(hash), media_type TEXT NOT NULL, bytes INTEGER NOT NULL CHECK(bytes > 0 AND bytes <= 536870912),
            PRIMARY KEY(evidence_id,revision,asset_index), FOREIGN KEY(evidence_id,revision) REFERENCES evidence(id,revision));
            INSERT INTO media_assets_streaming SELECT * FROM media_assets;
            DROP TABLE media_assets;
            ALTER TABLE media_assets_streaming RENAME TO media_assets;")?;
        tx.commit()?;
    }
    Ok(())
}

impl Store {
    /// Reuse collection's canonical revision calculation, including timestamps.
    /// A media lookup is one exact-message observation, never a history refresh.
    pub fn reconcile_telegram_media(
        &self,
        requested: &Value,
        index: usize,
        current: Value,
        raw: Value,
        bytes: Vec<u8>,
        media_type: &str,
    ) -> Result<(Value, String)> {
        validate_media(&bytes, media_type)?;
        let mut file = self.media_staging()?;
        file.write_all(&bytes)?;
        self.reconcile_telegram_media_file(requested, index, current, raw, file, media_type)
    }

    pub fn media_staging(&self) -> Result<NamedTempFile> {
        Ok(tempfile::Builder::new()
            .prefix("media-")
            .tempfile_in(self.config.data.join("staging"))?)
    }

    pub fn reconcile_telegram_media_file(
        &self,
        requested: &Value,
        index: usize,
        current: Value,
        raw: Value,
        file: NamedTempFile,
        media_type: &str,
    ) -> Result<(Value, String)> {
        self.reconcile_telegram_media_file_cancellable(
            requested,
            index,
            current,
            raw,
            file,
            media_type,
            self.shutdown.clone(),
        )
    }

    #[allow(clippy::too_many_arguments)] // Preserve the exact-origin compatibility API above.
    pub fn reconcile_telegram_media_file_cancellable(
        &self,
        requested: &Value,
        index: usize,
        current: Value,
        raw: Value,
        file: NamedTempFile,
        media_type: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<(Value, String)> {
        check_cancel(&cancel)?;
        let id = required(requested, "id")?;
        let revision = required(requested, "revision")?;
        if !id.starts_with("telegram:")
            || current["id"] != id
            || index != 0
            || ["account_id", "peer_id", "message_id"].iter().any(|field| {
                current["relationships"][field].as_str().is_none()
                    || current["relationships"][field] != requested["relationships"][field]
            })
            || current["media"]
                .as_array()
                .is_none_or(|items| items.len() != 1)
        {
            return Err(Error::invalid(
                "Telegram media does not match the requested message",
            ));
        }
        inspect_file_cancellable(file.path(), media_type, &cancel)?;
        let c = self.connect()?;
        media_reference(&c, id, revision, index)?;
        let source: String = c.query_row(
            "SELECT json FROM sources WHERE id=?1",
            [required(requested, "source")?],
            |r| r.get(0),
        )?;
        let source: Value = serde_json::from_str(&source)?;
        let canonical =
            crate::collection_store::normalize_message(&c, current.clone(), &source, &now())?;
        let resolved = required(&canonical, "revision")?.to_owned();
        if resolved != revision {
            // Reconcile through the existing publication contract, keeping the
            // source's history cursor, pause state and configuration unchanged.
            let state: Option<String> = c.query_row(
                "SELECT cs.json FROM collection_states cs JOIN subscriptions s ON s.id=cs.subscription_id WHERE json_extract(cs.json,'$.source_id')=?1 AND s.provider='telegram' AND s.version=json_extract(cs.json,'$.configuration_version') LIMIT 1",
                [required(requested, "source")?], |r| r.get(0)).optional()?;
            let mut state: Value = serde_json::from_str(&state.ok_or_else(|| Error::conflict("The source configuration changed during media recovery. Try again after collection reconnects."))?)?;
            let subscription = required(&state, "subscription_id")?.to_owned();
            let version = state["configuration_version"]
                .as_i64()
                .ok_or_else(Error::internal)?;
            state
                .as_object_mut()
                .ok_or_else(Error::internal)?
                .remove("run_id");
            drop(c);
            let _permit = self
                .imports
                .clone()
                .try_acquire_owned()
                .map_err(|_| Error::busy())?;
            self.publish_collection_page(&subscription, version, source, vec![current],
                json!({"provider":"telegram","operation":"media_exact_reconciliation","message":raw}),
                state, cancel.clone())?;
        }
        let metadata =
            self.retain_media_file_cancellable(id, &resolved, index, file, media_type, &cancel)?;
        Ok((metadata, resolved))
    }

    pub fn retained_media(&self, id: &str, revision: &str, index: usize) -> Result<Option<Value>> {
        let c = self.connect()?;
        media_reference(&c, id, revision, index)?;
        media_row(&c, id, revision, index)
    }

    pub fn retain_media(
        &self,
        id: &str,
        revision: &str,
        index: usize,
        bytes: Vec<u8>,
        media_type: &str,
    ) -> Result<Value> {
        let media_type = validate_media(&bytes, media_type)?;
        let mut file = self.media_staging()?;
        file.write_all(&bytes)?;
        self.retain_media_file(id, revision, index, file, &media_type)
    }

    pub fn retain_media_file(
        &self,
        id: &str,
        revision: &str,
        index: usize,
        file: NamedTempFile,
        media_type: &str,
    ) -> Result<Value> {
        self.retain_media_file_cancellable(id, revision, index, file, media_type, &self.shutdown)
    }

    pub fn retain_media_file_cancellable(
        &self,
        id: &str,
        revision: &str,
        index: usize,
        file: NamedTempFile,
        media_type: &str,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        check_cancel(cancel)?;
        let proposed = inspect_file_cancellable(file.path(), media_type, cancel)?;
        let media_type = required(&proposed, "media_type")?;
        let size = proposed["bytes"].as_u64().ok_or_else(Error::internal)?;
        {
            let c = self.connect()?;
            media_reference(&c, id, revision, index)?;
            if let Some(retained) = media_row(&c, id, revision, index)? {
                let retained = same_asset(retained, proposed)?;
                verified_asset_path(&self.config.objects(), &retained)?;
                return Ok(retained);
            }
        }
        let object_hash = required(&proposed, "hash")?.to_owned();
        file.as_file().sync_all()?;
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        check_cancel(cancel)?;
        check_cancel(&self.shutdown)?;
        // The complete, validated file now enters a short durable commit. A
        // close after this checkpoint may keep that completed asset, never a partial.
        let dest = self.config.object_path(&object_hash);
        fs::create_dir_all(dest.parent().ok_or_else(Error::internal)?)?;
        if let Err(error) = file.persist_noclobber(&dest) {
            if error.error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.error.into());
            }
            verified_asset_path(&self.config.objects(), &proposed)?;
        }
        self.sync_object_directories(
            std::iter::once(object_hash.clone()),
            &AtomicBool::new(false),
        )?;
        let mut c = self.connect()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let evidence = media_reference(&tx, id, revision, index)?;
        if let Some(retained) = media_row(&tx, id, revision, index)? {
            return same_asset(retained, proposed);
        }
        tx.execute(
            "INSERT OR IGNORE INTO objects VALUES(?1,?2,?3)",
            params![object_hash, size as i64, media_type],
        )?;
        tx.execute(
            "INSERT INTO media_assets VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                id,
                revision,
                index as i64,
                object_hash,
                media_type,
                size as i64
            ],
        )?;
        let root = format!("media:{object_hash}");
        node(
            &tx,
            &root,
            "retained_media",
            &format!("Retained {media_type}"),
            &proposed,
        )?;
        let detail = json!({"evidence_id":id,"revision":revision,"asset_index":index,"hash":object_hash,"bytes":size,"media_type":media_type,"retained_at":now(),"operation":"validate_and_retain_media_v1"});
        edge(
            &tx,
            &format!("{id}@{revision}"),
            &root,
            "retained_asset",
            &detail,
        )?;
        edge(
            &tx,
            required(&evidence, "source")?,
            &root,
            "media_acquired_from",
            &detail,
        )?;
        let event = Self::event(
            &tx,
            "media_retained",
            Self::latest(&tx)?.as_deref(),
            vec![id.into()],
        )?;
        tx.commit()?;
        let _ = self.events.send(event);
        Ok(proposed)
    }

    pub fn media_file(&self, id: &str, revision: &str, index: usize) -> Result<(PathBuf, String)> {
        let retained = self
            .retained_media(id, revision, index)?
            .ok_or_else(|| Error::missing("This media asset has not been retained"))?;
        let path = verified_asset_path(&self.config.objects(), &retained)?;
        Ok((path, required(&retained, "media_type")?.into()))
    }

    /// Compatibility helper for small assets; HTTP uses streaming media_file.
    pub fn media_bytes(&self, id: &str, revision: &str, index: usize) -> Result<(Vec<u8>, String)> {
        let retained = self
            .retained_media(id, revision, index)?
            .ok_or_else(|| Error::missing("This media asset has not been retained"))?;
        if retained["bytes"]
            .as_u64()
            .is_none_or(|n| n > MAX_MEDIA_BYTES as u64)
        {
            return Err(Error::invalid(
                "Use the streaming media endpoint for large assets",
            ));
        }
        let bytes = fs::read(verified_asset_path(&self.config.objects(), &retained)?)?;
        Ok((bytes, required(&retained, "media_type")?.into()))
    }
}

fn same_asset(retained: Value, proposed: Value) -> Result<Value> {
    if retained != proposed {
        return Err(Error::conflict(
            "Media for this exact evidence revision is immutable; retain changed content under a new evidence revision",
        ));
    }
    Ok(retained)
}

fn media_reference(c: &Connection, id: &str, revision: &str, index: usize) -> Result<Value> {
    if index >= 100 {
        return Err(Error::invalid(
            "Media index exceeds the 100-asset per-message bound",
        ));
    }
    let evidence = Store::evidence(c, id, revision)?;
    if evidence["media"]
        .as_array()
        .and_then(|items| items.get(index))
        .is_none_or(|item| !item.is_object())
    {
        return Err(Error::missing(
            "The exact evidence revision has no media reference at this index",
        ));
    }
    Ok(evidence)
}

fn media_row(c: &Connection, id: &str, revision: &str, index: usize) -> Result<Option<Value>> {
    Ok(c.query_row("SELECT hash,bytes,media_type FROM media_assets WHERE evidence_id=?1 AND revision=?2 AND asset_index=?3",params![id,revision,index as i64],|r|Ok(json!({"hash":r.get::<_,String>(0)?,"bytes":r.get::<_,i64>(1)?,"media_type":r.get::<_,String>(2)?}))).optional()?)
}

/// This validates a supported container signature and bounded structure, not a
/// full codec decode. The browser receives the canonical MIME with nosniff from
/// the controlled application endpoint; SVG/HTML and arbitrary files are denied.
fn validate_media(bytes: &[u8], reported: &str) -> Result<String> {
    if bytes.is_empty() || bytes.len() > MAX_MEDIA_BYTES {
        return Err(Error::invalid("Media must contain 1 byte to 8 MiB"));
    }
    validate_signature(bytes, bytes, bytes.len(), reported)
}

fn validate_signature(bytes: &[u8], tail: &[u8], size: usize, reported: &str) -> Result<String> {
    let media_type = reported
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let matches = match media_type.as_str() {
        "image/jpeg" => {
            bytes.len() >= 4
                && bytes.starts_with(&[0xff, 0xd8, 0xff])
                && tail.ends_with(&[0xff, 0xd9])
        }
        "image/png" => {
            bytes.len() >= 45
                && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                && &bytes[8..16] == b"\0\0\0\rIHDR"
                && bytes[16..20] != [0, 0, 0, 0]
                && bytes[20..24] != [0, 0, 0, 0]
                && tail.ends_with(b"\0\0\0\0IEND\xaeB`\x82")
        }
        "image/gif" => {
            bytes.len() >= 14
                && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"))
                && bytes[6..8] != [0, 0]
                && bytes[8..10] != [0, 0]
                && tail.last() == Some(&0x3b)
        }
        "image/webp" => {
            bytes.len() >= 20
                && bytes.starts_with(b"RIFF")
                && &bytes[8..12] == b"WEBP"
                && [b"VP8 ".as_slice(), b"VP8L".as_slice(), b"VP8X".as_slice()]
                    .contains(&&bytes[12..16])
                && u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize == size - 8
        }
        "video/mp4" => valid_mp4(bytes),
        _ => {
            return Err(Error::invalid(
                "Supported retained media types are JPEG, PNG, WebP, GIF and MP4",
            ));
        }
    };
    if !matches {
        return Err(Error::invalid(
            "Media bytes do not match the reported supported format",
        ));
    }
    Ok(media_type)
}

fn valid_mp4(bytes: &[u8]) -> bool {
    if bytes.len() < 24 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let size = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
    if size < 16 || size > bytes.len() || !size.is_multiple_of(4) {
        return false;
    }
    let compatible = [
        b"isom", b"iso2", b"iso3", b"iso4", b"iso5", b"iso6", b"mp41", b"mp42", b"avc1", b"dash",
        b"M4V ", b"MSNV",
    ];
    compatible.iter().any(|brand| bytes[8..12] == brand[..])
        || bytes[16..size]
            .as_chunks::<4>()
            .0
            .iter()
            .any(|brand| compatible.iter().any(|known| brand == known.as_slice()))
}

/// Fixed 64 KiB buffers for hashing and format checks, including large videos.
pub(crate) fn file_hash(path: &Path) -> Result<(String, u64)> {
    file_hash_cancellable(path, &AtomicBool::new(false))
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::cancelled())
    } else {
        Ok(())
    }
}
pub(crate) fn file_hash_cancellable(path: &Path, cancel: &AtomicBool) -> Result<(String, u64)> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        bytes += n as u64;
    }
    Ok((format!("{:x}", hash.finalize()), bytes))
}
fn inspect_file(path: &Path, reported: &str) -> Result<Value> {
    inspect_file_cancellable(path, reported, &AtomicBool::new(false))
}
fn inspect_file_cancellable(path: &Path, reported: &str, cancel: &AtomicBool) -> Result<Value> {
    check_cancel(cancel)?;
    let mut file = fs::File::open(path)?;
    let size = file.metadata()?.len();
    if size == 0 || size > MAX_MEDIA_FILE_BYTES as u64 {
        return Err(Error::invalid("Media must contain 1 byte to 512 MiB"));
    }
    let mut prefix = vec![0; size.min(64 * 1024) as usize];
    file.read_exact(&mut prefix)?;
    let mut tail = vec![0; size.min(12) as usize];
    file.seek(SeekFrom::End(-(tail.len() as i64)))?;
    file.read_exact(&mut tail)?;
    let media_type = validate_signature(&prefix, &tail, size as usize, reported)?;
    let (hash, read) = file_hash_cancellable(path, cancel)?;
    if read != size {
        return Err(Error::invalid("Media changed during retention"));
    }
    Ok(json!({"hash":hash,"bytes":size,"media_type":media_type}))
}
fn verified_asset_path(objects: &Path, metadata: &Value) -> Result<PathBuf> {
    let hash_value = required(metadata, "hash")?;
    if hash_value.len() != 64 || !hash_value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::invalid("Retained media has an invalid hash"));
    }
    let path = objects.join(&hash_value[..2]).join(hash_value);
    if inspect_file(&path, required(metadata, "media_type")?)? != *metadata {
        return Err(Error::invalid(
            "Retained media differs from its exact record",
        ));
    }
    Ok(path)
}

pub(crate) fn verify_media_assets(c: &Connection, objects: &Path) -> Result<usize> {
    if !c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='media_assets')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(0);
    }
    let mut statement = c.prepare(
        "SELECT evidence_id,revision,asset_index,hash,bytes,media_type FROM media_assets",
    )?;
    let mut rows = statement.query([])?;
    let mut total = 0;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let revision: String = row.get(1)?;
        let index = usize::try_from(row.get::<_, i64>(2)?)
            .map_err(|_| Error::invalid("Retained media has an invalid asset index"))?;
        media_reference(c, &id, &revision, index)?;
        let metadata = json!({"hash":row.get::<_,String>(3)?,"bytes":row.get::<_,i64>(4)?,"media_type":row.get::<_,String>(5)?});
        let object_matches: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM objects WHERE hash=?1 AND bytes=?2 AND media_type=?3)",
            params![
                metadata["hash"].as_str(),
                metadata["bytes"].as_i64(),
                metadata["media_type"].as_str()
            ],
            |r| r.get(0),
        )?;
        if !object_matches {
            return Err(Error::invalid(
                "Retained media metadata differs from its object record",
            ));
        }
        verified_asset_path(objects, &metadata)?;
        total += 1;
    }
    Ok(total)
}
