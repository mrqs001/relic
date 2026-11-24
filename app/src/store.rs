use crate::{
    config::Config,
    error::{Error, Result},
    model::*,
};
use rusqlite::{Connection, OptionalExtension, params, types::Value as SqlValue};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, broadcast};

pub const MAX_IMPORT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_IMPORT_RECORDS: usize = 2000;
pub const MAX_MAP: usize = 10_000;
pub const MAX_SCOPE_MEMBERS: usize = 100_000;
// Retain a small number of initialized connections so ordinary polling keeps a
// warm SQLite cache and an open WAL owner. Leases never cross Store lifetimes.
pub struct StoreConnection {
    connection: Option<Connection>,
    idle: Arc<Mutex<Vec<Connection>>>,
}
impl std::ops::Deref for StoreConnection {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.connection.as_ref().expect("live connection lease")
    }
}
impl std::ops::DerefMut for StoreConnection {
    fn deref_mut(&mut self) -> &mut Connection {
        self.connection.as_mut().expect("live connection lease")
    }
}
impl Drop for StoreConnection {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take()
            && connection.is_autocommit()
            && let Ok(mut idle) = self.idle.lock()
            && idle.len() < 4
        {
            idle.push(connection);
        }
    }
}
/// Take the exclusive lock on `<data>/.relic.lock`; it lasts while the
/// returned file stays open. Startup maintenance in [`Store::open`] marks
/// every reserved request unknown, which is only true when no other Relic
/// process still has requests in flight. [`Store::open`] takes this itself.
pub fn lock_workspace(data: &std::path::Path) -> Result<fs::File> {
    fs::create_dir_all(data)?;
    let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(data.join(".relic.lock"))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(fs::TryLockError::WouldBlock) => Err(Error::conflict("Another Relic process is already running on this data directory. Stop it before starting another.")),
        Err(fs::TryLockError::Error(e)) => Err(e.into()),
    }
}
/// Data directories this process holds, by canonical path. The first
/// [`Store::open`] of a directory takes its lock and keeps it until the
/// process exits; later opens in this process (a test simulating a restart,
/// a tool reopening its workspace) reuse it. Another
/// process gets the conflict error from [`lock_workspace`].
static HELD_WORKSPACES: std::sync::LazyLock<Mutex<HashMap<std::path::PathBuf, File>>> =
    std::sync::LazyLock::new(Default::default);
fn hold_workspace(data: &std::path::Path) -> Result<()> {
    fs::create_dir_all(data)?;
    let key = data.canonicalize()?;
    let mut held = HELD_WORKSPACES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let std::collections::hash_map::Entry::Vacant(entry) = held.entry(key) {
        let lock = lock_workspace(entry.key())?;
        entry.insert(lock);
    }
    Ok(())
}
#[derive(Clone)]
pub struct Store {
    pub config: Config,
    idle_connections: Arc<Mutex<Vec<Connection>>>,
    pub reads: Arc<Semaphore>,
    pub media_reads: Arc<Semaphore>,
    pub read_admission: Arc<Semaphore>,
    pub controls: Arc<Semaphore>,
    pub control_admission: Arc<Semaphore>,
    pub imports: Arc<Semaphore>,
    pub streams: Arc<Semaphore>,
    /// No new work is admitted. Requests already sent to a provider may finish.
    pub shutdown: Arc<AtomicBool>,
    /// Hard stop: cancel in-flight provider requests too.
    pub halt: Arc<AtomicBool>,
    pub writer: Arc<Mutex<()>>,
    pub events: broadcast::Sender<Value>,
    pub running: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}
impl Store {
    pub fn open(config: Config) -> Result<Self> {
        // Before anything in the workspace changes: another process may still
        // have requests in flight that the maintenance below would mark unknown.
        hold_workspace(&config.data)?;
        fs::create_dir_all(config.objects())?;
        fs::create_dir_all(config.data.join("staging"))?;
        fs::create_dir_all(config.data.join("backups"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&config.data, fs::Permissions::from_mode(0o700))?;
        }
        let conn = Connection::open(config.db_path())?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(Duration::from_secs(2))?;
        let version = rusqlite::version_number();
        if version < 3_051_003 {
            return Err(Error::invalid(
                "Linked SQLite must include WAL-reset fix: SQLite >=3.51.3",
            ));
        }
        conn.execute_batch(include_str!("schema.sql"))?;
        crate::publication_history::migrate(&conn)?;
        conn.execute_batch(include_str!("event_link_schema.sql"))?;
        conn.execute_batch(include_str!("vessel_finder.sql"))?;
        crate::vessel_finder::migrate(&conn)?;
        let interrupted_links = conn.execute("UPDATE event_link_attempts SET status='unknown',error='Interrupted by restart; no automatic rebilling' WHERE status='reserved'", [])?
            + conn.execute("UPDATE event_embedding_attempts SET status='unknown',error='Interrupted by restart; no automatic rebilling' WHERE status='reserved'", [])?;
        if interrupted_links > 0 {
            conn.execute("INSERT INTO metadata VALUES('event_links_error','Relationship requests were interrupted; billing is unknown. Resume after review.') ON CONFLICT(key) DO UPDATE SET value=excluded.value",[])?;
        }
        conn.execute_batch(
            "DROP VIEW IF EXISTS reading_evidence; DROP VIEW IF EXISTS reading_message_sites;",
        )?;
        conn.execute_batch(include_str!("schema.sql"))?;
        crate::jev_hints::migrate(&conn)?;
        conn.execute("UPDATE supplemental_attempts SET status='unknown',error='Interrupted by restart; reservation retained and no retry sent' WHERE status='reserved'", [])?;
        // Nothing is in flight at startup. A reading reservation left by an exit
        // that could not record its outcome would otherwise read "in flight"
        // forever and block its message. Receipt reconciliation still applies.
        conn.execute("UPDATE processing_attempts SET status='unknown',json=json_set(json,'$.error','Interrupted by restart; billing is unknown and no retry was sent') WHERE status='reserved'", [])?;
        reclassify_credit_rejections(&conn, &config.data)?;
        crate::geographic_view::backfill(&conn)?;
        crate::media_store::migrate_media_bound(&conn)?;
        // Early geographic jobs predated the shared API job counters. Preserve
        // their outcomes while making saved workspaces readable by API v1.
        conn.execute_batch("UPDATE jobs SET json=json_set(json,'$.accepted',coalesce(json_extract(json,'$.accepted'),json_extract(json,'$.result.resolved_readings'),json_array_length(json,'$.result.editions'),0),'$.total',coalesce(json_extract(json,'$.total'),json_extract(json,'$.result.resolved_readings'),json_array_length(json,'$.result.editions'),0)) WHERE json_extract(json,'$.kind') IN ('geography','geography-activate','geography-reresolve') AND (json_type(json,'$.accepted') IS NULL OR json_type(json,'$.total') IS NULL);")?;
        let old_jobs = all_json(&conn, "SELECT json FROM jobs")?;
        for mut j in old_jobs {
            if ["queued", "running", "cancelling"].contains(&j["status"].as_str().unwrap_or("")) {
                j["status"] = json!("failed");
                j["detail"] = json!(
                    "Interrupted by restart; last complete publication retained. Reimport the original bundle to retry."
                );
                conn.execute(
                    "UPDATE jobs SET json=?1 WHERE id=?2",
                    params![string(&j), j["id"].as_str()],
                )?;
            }
        }
        // This process owns staging. Only incomplete candidates are removed; committed objects are immutable.
        for entry in fs::read_dir(config.data.join("staging"))?.take(2048) {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                fs::remove_file(entry.path())?;
            }
        }
        let (events, _) = broadcast::channel(128);
        register_query_functions(&conn)?;
        conn.pragma_update(None, "cache_size", -8192)?;
        Ok(Self {
            config,
            idle_connections: Arc::new(Mutex::new(vec![conn])),
            reads: Arc::new(Semaphore::new(4)),
            media_reads: Arc::new(Semaphore::new(8)),
            read_admission: Arc::new(Semaphore::new(20)),
            controls: Arc::new(Semaphore::new(2)),
            control_admission: Arc::new(Semaphore::new(10)),
            imports: Arc::new(Semaphore::new(1)),
            streams: Arc::new(Semaphore::new(8)),
            shutdown: Arc::new(AtomicBool::new(false)),
            halt: Arc::new(AtomicBool::new(false)),
            writer: Arc::new(Mutex::new(())),
            events,
            running: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    /// Stop admitting work but let paid requests already sent finish and be
    /// recorded. Cutting one off leaves an unknown charge whose message is held
    /// for review, so a routine restart would otherwise strand a whole batch.
    pub fn begin_drain(&self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = self.events.send(json!({"kind":"shutdown"}));
    }
    /// Model requests (reading, Jev, event links, ship finder) sent and not yet recorded.
    pub fn in_flight_model_requests(&self) -> Result<i64> {
        Ok(self.connect()?.query_row(
            "SELECT (SELECT count(*) FROM processing_attempts WHERE status='reserved')+(SELECT count(*) FROM supplemental_attempts WHERE status='reserved')+(SELECT count(*) FROM event_link_attempts WHERE status='reserved')+(SELECT count(*) FROM event_embedding_attempts WHERE status='reserved')+(SELECT count(*) FROM vessel_lookups WHERE status='reserved')",
            [],
            |r| r.get(0),
        )?)
    }
    /// Processing jobs still running. A batch publishes its finished readings
    /// after its last request is recorded, so draining waits for the job too.
    pub fn processing_jobs_running(&self) -> usize {
        self.running
            .lock()
            .map_or(0, |r| r.keys().filter(|k| k.starts_with("processing:")).count())
    }
    pub fn begin_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.halt.store(true, Ordering::Release);
        if let Ok(running) = self.running.lock() {
            for cancel in running.values() {
                cancel.store(true, Ordering::Release);
            }
        }
        let _ = self.events.send(json!({"kind":"shutdown"}));
    }
    pub fn connect(&self) -> Result<StoreConnection> {
        let cached = self
            .idle_connections
            .lock()
            .map_err(|_| Error::internal())?
            .pop();
        let c = if let Some(c) = cached {
            c
        } else {
            let c = Connection::open(self.config.db_path())?;
            c.pragma_update(None, "foreign_keys", "ON")?;
            c.pragma_update(None, "synchronous", "FULL")?;
            c.pragma_update(None, "cache_size", -8192)?;
            c.busy_timeout(Duration::from_secs(2))?;
            register_query_functions(&c)?;
            c
        };
        Ok(StoreConnection {
            connection: Some(c),
            idle: self.idle_connections.clone(),
        })
    }
    pub fn health(&self) -> Value {
        json!({"status":"ok","api_version":1,"sqlite_version":rusqlite::version(),"limits":{"read_workers":4,"media_streams":8,"import_workers":1,"import_bytes":MAX_IMPORT_BYTES,"import_records":MAX_IMPORT_RECORDS,"map_features":MAX_MAP,"exact_scope_members":MAX_SCOPE_MEMBERS,"list_page":500,"subscriptions":crate::subscriptions::MAX_SUBSCRIPTIONS,"subscription_page":crate::subscriptions::MAX_SUBSCRIPTION_PAGE,"sse_replay":128,"sse_streams":8,"control_workers":2,"read_waiters":16,"control_waiters":8,"queue_policy":"bounded admission; wait up to 250 ms for a worker","sqlite_busy_timeout_ms":2000,"live_sources":"explicit_account_activation"}})
    }
    // File contents are synced here; publication calls sync_object_directories before metadata commit.
    pub(crate) fn write_object(&self, bytes: &[u8]) -> Result<String> {
        let h = hash(bytes);
        let dest = self.config.object_path(&h);
        if dest.exists() {
            if fs::metadata(&dest)?.len() != bytes.len() as u64 || hash(&fs::read(&dest)?) != h {
                return Err(Error::internal());
            }
            return Ok(h);
        }
        let parent = dest.parent().ok_or_else(Error::internal)?;
        fs::create_dir_all(parent)?;
        let stage = self.config.data.join("staging").join(ident("object"));
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(stage, &dest)?;
        Ok(h)
    }
    pub(crate) fn sync_object_directories(
        &self,
        hashes: impl Iterator<Item = String>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let prefixes: HashSet<String> = hashes.map(|h| h[..2].to_string()).collect();
        for prefix in prefixes {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            File::open(self.config.objects().join(prefix))?.sync_all()?;
        }
        for dir in [
            self.config.objects(),
            self.config.data.join("objects"),
            self.config.data.join("staging"),
            self.config.data.clone(),
        ] {
            File::open(dir)?.sync_all()?;
        }
        Ok(())
    }
    pub fn original_object(&self, h: &str) -> Result<Vec<u8>> {
        if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::invalid("Invalid retained object hash"));
        }
        let c = self.connect()?;
        let size: Option<i64> = c
            .query_row("SELECT bytes FROM objects WHERE hash=?1", [h], |r| r.get(0))
            .optional()?;
        let size = size.ok_or_else(|| Error::missing("Retained object unavailable"))?;
        if size > MAX_IMPORT_BYTES as i64 {
            return Err(Error::invalid("Object exceeds local download bound"));
        }
        let bytes = fs::read(self.config.object_path(h))?;
        if bytes.len() as i64 != size || hash(&bytes) != h {
            return Err(Error::internal());
        }
        Ok(bytes)
    }
    pub fn latest(c: &Connection) -> Result<Option<String>> {
        Ok(c.query_row(
            "SELECT value FROM metadata WHERE key='current_publication'",
            [],
            |r| r.get(0),
        )
        .optional()?)
    }
    pub fn sequence(c: &Connection) -> Result<u64> {
        Ok(
            c.query_row("SELECT coalesce(max(sequence),0) FROM events", [], |r| {
                r.get::<_, i64>(0).map(|v| v as u64)
            })?,
        )
    }
    pub fn event(
        c: &Connection,
        kind: &str,
        pubid: Option<&str>,
        ids: Vec<String>,
    ) -> Result<Value> {
        let mut v = json!({"kind":kind,"publication_id":pubid,"affected_ids":ids});
        c.execute("INSERT INTO events(json) VALUES(?1)", [string(&v)])?;
        let seq = c.last_insert_rowid();
        v["sequence"] = json!(seq);
        c.execute(
            "UPDATE events SET json=?1 WHERE sequence=?2",
            params![string(&v), seq],
        )?;
        c.execute("DELETE FROM events WHERE sequence <= ?1", [seq - 128])?;
        Ok(v)
    }
    pub fn workspace(&self) -> Result<Value> {
        let c = self.connect()?;
        let sources = all_json(&c, "SELECT json FROM sources ORDER BY id")?;
        let mut pubs = vec![];
        let publications_total: i64 =
            c.query_row("SELECT count(*) FROM publications", [], |r| r.get(0))?;
        let mut stmt=c.prepare("SELECT p.id,p.label,p.created_at,p.status,(SELECT count(*) FROM membership m WHERE p.id=m.publication_id) FROM (SELECT * FROM publications ORDER BY (id=(SELECT value FROM metadata WHERE key='current_publication')) DESC,created_at DESC,id LIMIT 100) p ORDER BY (p.id=(SELECT value FROM metadata WHERE key='current_publication')) DESC,p.created_at DESC,p.id")?;
        for r in stmt.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"label":r.get::<_,String>(1)?,"created_at":r.get::<_,String>(2)?,"status":r.get::<_,String>(3)?,"count":r.get::<_,i64>(4)?})))?{pubs.push(r?)}
        Ok(
            json!({"name":"Relic","empty":sources.is_empty(),"fictional":sources.iter().any(|s|s["fictional"]==true),"sources":sources,"investigations":all_json(&c,"SELECT json FROM investigations ORDER BY id")?,"briefs":all_json(&c,"SELECT b.json FROM briefs b WHERE b.version=(SELECT max(x.version) FROM briefs x WHERE x.id=b.id) ORDER BY id")?,"publications":pubs,"publications_total":publications_total,"publications_truncated":publications_total>100,"watches":all_json(&c,"SELECT json FROM watches ORDER BY id")?,"jobs":all_json(&c,"SELECT json FROM jobs ORDER BY rowid DESC LIMIT 50")?,"coverage":coverage(),"sequence":Self::sequence(&c)?}),
        )
    }
    pub fn evidence(c: &Connection, id: &str, revision: &str) -> Result<Value> {
        json_row(
            c,
            "SELECT json FROM evidence WHERE id=?1 AND revision=?2",
            params![id, revision],
        )?
        .ok_or_else(|| Error::missing("Exact evidence revision is not retained"))
    }
    pub fn detail(&self, id: &str, revision: &str) -> Result<Value> {
        let c = self.connect()?;
        let mut e = Self::evidence(&c, id, revision)?;
        let key: i64 = c.query_row(
            "SELECT rowid FROM evidence WHERE id=?1 AND revision=?2",
            params![id, revision],
            |r| r.get(0),
        )?;
        e["message_key"] = json!(key.to_string());
        let mut stmt =
            c.prepare("SELECT revision FROM evidence WHERE id=?1 ORDER BY acquired_at,revision")?;
        let revisions: Vec<String> = stmt
            .query_map([id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        e["revisions"] = json!(revisions);
        e["lineage_root"] = json!(format!("{id}@{revision}"));
        let mut claims = vec![];
        for inv in all_json(&c, "SELECT json FROM investigations")? {
            for claim in inv["claims"].as_array().unwrap_or(&vec![]) {
                if claim["citations"].as_array().is_some_and(|cs| {
                    cs.iter()
                        .any(|x| x["evidence_id"] == id && x["revision"] == revision)
                }) {
                    claims.push(claim.clone())
                }
            }
        }
        e["claims"] = json!(claims);
        e["original_input_hashes"] = json!(original_input_hashes(&c, id, revision)?);
        Ok(e)
    }
    /// Compact, workspace-local link to an immutable retained revision.
    pub fn message_reference(&self, key: &str) -> Result<Value> {
        let rowid = key
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0 && id.to_string() == key)
            .ok_or_else(|| Error::invalid("Invalid message link"))?;
        self.connect()?
            .query_row(
                "SELECT id,revision FROM evidence WHERE rowid=?1",
                [rowid],
                |r| Ok(json!({"id":r.get::<_,String>(0)?,"revision":r.get::<_,String>(1)?})),
            )
            .optional()?
            .ok_or_else(|| Error::missing("This message is not retained in this workspace"))
    }
    pub fn resolve_scope(c: &Connection, scope: &mut Scope) -> Result<String> {
        let pubid = scope
            .publication_id
            .clone()
            .or(Self::latest(c)?)
            .ok_or_else(|| Error::missing("No complete publication. Import evidence first."))?;
        if !c.query_row("SELECT EXISTS(SELECT 1 FROM publications WHERE id=?1 AND status IN ('complete','frozen'))",[&pubid],|r|r.get::<_,bool>(0))?{return Err(Error::missing("Requested publication is unavailable"))}
        for source in &scope.source_ids {
            if !c.query_row(
                "SELECT EXISTS(SELECT 1 FROM sources WHERE id=?1)",
                [source],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(Error::invalid(format!(
                    "Selected source is unavailable: {source}"
                )));
            }
        }
        scope.publication_id = Some(pubid.clone());
        Ok(pubid)
    }
    pub fn query(&self, input: Query) -> Result<Value> {
        let c = self.connect()?;
        query_conn(&c, input, false, true)
    }
    /// The map view: area outlines are fetched and cached separately per area.
    pub fn query_without_area_shapes(&self, input: Query) -> Result<Value> {
        let c = self.connect()?;
        query_conn(&c, input, false, false)
    }
    pub fn query_page(&self, input: Query) -> Result<Value> {
        if input.cursor.is_none() || !input.scope["publication_id"].is_string() {
            return Err(Error::invalid(
                "A continuation page requires its cursor and exact publication",
            ));
        }
        let c = self.connect()?;
        query_conn(&c, input, true, true)
    }
    pub fn check_citations(
        c: &Connection,
        citations: &[Citation],
        scope: Option<&Scope>,
    ) -> Result<Vec<Value>> {
        if citations.len() > 100 {
            return Err(Error::invalid("At most 100 exact citations are allowed"));
        }
        let mut out = vec![];
        for cit in citations {
            let e = Self::evidence(c, &cit.evidence_id, &cit.revision)
                .map_err(|_| Error::invalid("Citation refers to an unavailable exact revision"))?;
            let chars: Vec<char> = e["text"]
                .as_str()
                .ok_or_else(Error::internal)?
                .chars()
                .collect();
            if cit.start_character >= cit.end_character || cit.end_character > chars.len() {
                return Err(Error::invalid(
                    "Citation span is outside the exact retained Unicode text",
                ));
            }
            if let Some(s) = scope {
                let pred = predicate(s);
                let mut vals = pred.1;
                vals.push(SqlValue::Text(cit.evidence_id.clone()));
                vals.push(SqlValue::Text(cit.revision.clone()));
                let sql = format!(
                    "SELECT EXISTS(SELECT 1 FROM reading_evidence e WHERE {} AND e.id=? AND e.revision=?)",
                    pred.0
                );
                if !c.query_row(&sql, rusqlite::params_from_iter(vals), |r| {
                    r.get::<_, bool>(0)
                })? {
                    return Err(Error::invalid(
                        "Citation is outside the declared scope/publication",
                    ));
                }
            }
            let quote: String = chars[cit.start_character..cit.end_character]
                .iter()
                .collect();
            let interpretation: Option<String> = if let Some(s) = scope {
                c.query_row("SELECT i.json FROM publication_interpretations pi JOIN interpretations i ON i.id=pi.interpretation_id JOIN evidence e ON e.rowid=pi.evidence_rowid WHERE pi.publication_id=?1 AND e.id=?2 AND e.revision=?3",params![s.publication_id,cit.evidence_id,cit.revision],|r|r.get(0)).optional()?
            } else {
                None
            };
            out.push(json!({"citation":cit,"quote":quote,"evidence":e,"interpretation":interpretation.map(|s|serde_json::from_str::<Value>(&s)).transpose()?,"original_input_hashes":original_input_hashes(c,&cit.evidence_id,&cit.revision)?}));
        }
        Ok(out)
    }
    pub fn import_job(&self, bundle: Value, id: String, cancel: Arc<AtomicBool>) -> Result<Value> {
        self.import_job_with_raw(bundle, id, cancel, None)
    }
    pub fn import_job_with_raw(
        &self,
        bundle: Value,
        id: String,
        cancel: Arc<AtomicBool>,
        original_bytes: Option<Vec<u8>>,
    ) -> Result<Value> {
        let initial = json!({"id":id,"kind":"import","status":"running","accepted":0,"total":bundle["evidence_revisions"].as_array().map(Vec::len).unwrap_or(0),"created_at":now(),"detail":"Validating originals and staging durable evidence; prior publication remains active."});
        self.save_job(&initial)?;
        let result = self.import_inner(&bundle, &id, &cancel, original_bytes.as_deref());
        let mut job = initial;
        match &result {
            Ok(v) => {
                job["status"] = json!("completed");
                job["accepted"] = v["accepted"].clone();
                job["detail"] = json!(
                    "Complete publication committed. Exact input JSON bytes, normalized record JSON and exact source text retained."
                )
            }
            Err(e) => {
                job["status"] = json!(if e.code == "cancelled" {
                    "cancelled"
                } else {
                    "failed"
                });
                job["detail"] = json!(e.message)
            }
        }
        self.save_job(&job)?;
        self.running
            .lock()
            .map_err(|_| Error::internal())?
            .remove(&id);
        result
    }
    pub fn save_job(&self, job: &Value) -> Result<()> {
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let c = self.connect()?;
        c.execute("INSERT INTO jobs(id,json) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET json=excluded.json",params![job["id"].as_str(),string(job)])?;
        let ev = Self::event(
            &c,
            "job",
            Self::latest(&c)?.as_deref(),
            vec![job["id"].as_str().unwrap_or("").into()],
        )?;
        let _ = self.events.send(ev);
        Ok(())
    }
    pub fn cancel_job(&self, id: &str) -> Result<Value> {
        let cancel = self
            .running
            .lock()
            .map_err(|_| Error::internal())?
            .get(id)
            .cloned();
        let was_running = cancel.is_some();
        if let Some(cancel) = cancel {
            cancel.store(true, Ordering::Release);
        }
        let c = self.connect()?;
        let mut job = json_row(&c, "SELECT json FROM jobs WHERE id=?1", [id])?
            .ok_or_else(|| Error::missing("Job unavailable"))?;
        if ["running", "queued"].contains(&job["status"].as_str().unwrap_or("")) && was_running {
            job["status"] = json!("cancelling");
            job["detail"] = json!(
                "Stop acknowledged; worker will check before publication. A commit already in progress can complete."
            );
        }
        Ok(job)
    }
    fn import_inner(
        &self,
        bundle: &Value,
        run_id: &str,
        cancel: &AtomicBool,
        original_bytes: Option<&[u8]>,
    ) -> Result<Value> {
        if bundle["schema_version"] != 1 {
            return Err(Error::invalid(
                "Unsupported bundle schema; expected version 1",
            ));
        }
        let fictional = bundle["fictional"]
            .as_bool()
            .ok_or_else(|| Error::invalid("Bundle fictional flag is required"))?;
        let sources = bundle["sources"]
            .as_array()
            .ok_or_else(|| Error::invalid("Bundle sources are required"))?;
        let inputs = bundle["evidence_revisions"]
            .as_array()
            .ok_or_else(|| Error::invalid("Evidence revisions are required"))?;
        if sources.is_empty()
            || sources.len() > 100
            || inputs.is_empty()
            || inputs.len() > MAX_IMPORT_RECORDS
        {
            return Err(Error::invalid(
                "Import requires 1–100 sources and 1–2000 revisions per bounded batch",
            ));
        }
        let bytes = serde_json::to_vec(bundle)?;
        if bytes.len() > MAX_IMPORT_BYTES {
            return Err(Error::invalid(
                "Import exceeds 2 MiB; split into bounded batches",
            ));
        }
        let bundle_hash = hash(&bytes);
        let original = original_bytes.unwrap_or(&bytes);
        if original.len() > MAX_IMPORT_BYTES
            || serde_json::from_slice::<Value>(original)? != *bundle
        {
            return Err(Error::invalid(
                "Original JSON bytes do not match the parsed import or exceed its bound",
            ));
        }
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        let raw_hash = self.write_object(original)?;
        {
            let c = self.connect()?;
            if let Some(p) = c
                .query_row(
                    "SELECT publication_id FROM import_bundles WHERE hash=?1",
                    [&bundle_hash],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                self.sync_object_directories(std::iter::once(raw_hash.clone()), cancel)?;
                let _guard = self.writer.lock().map_err(|_| Error::internal())?;
                let mut c = self.connect()?;
                let tx = c.transaction()?;
                tx.execute(
                    "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
                    params![raw_hash, original.len() as i64],
                )?;
                retain_acquisition(&tx, bundle, run_id, &bundle_hash, &raw_hash, &p)?;
                node(
                    &tx,
                    run_id,
                    "run_step",
                    "Retain equivalent acquired input",
                    &json!({"status":"reused","input_hash":raw_hash,"model_calls":0}),
                )?;
                edge(
                    &tx,
                    run_id,
                    &p,
                    "reused_publication",
                    &json!({"meaning":"Equivalent evidence; original input bytes retained without reprocessing or new alerts"}),
                )?;
                let event = Self::event(&tx, "acquisition", Some(&p), vec![run_id.into()])?;
                if cancel.load(Ordering::Acquire) {
                    return Err(Error::cancelled());
                }
                tx.commit()?;
                let _ = self.events.send(event);
                return Ok(
                    json!({"publication_id":p,"accepted":0,"reused":inputs.len(),"original_input_hash":raw_hash,"warnings":import_warnings(bundle)}),
                );
            }
        }
        let mut srcmap = HashMap::new();
        for src in sources {
            let id = bounded_text(src, "id", 200)?;
            let label = bounded_text(src, "label", 200)?;
            let family = bounded_text(src, "family", 100)?;
            if srcmap.insert(id.clone(),json!({"id":id,"label":label,"family":family,"provider":"local_import","status":"available","coverage":"Imported bundle only; no continuous or complete upstream coverage asserted","fictional":fictional,"detail":"Retained local input; this import has no continuous upstream collection."})).is_some(){return Err(Error::invalid("Duplicate source identity in bundle"))}
        }
        let mut seen = HashSet::new();
        let mut entries = vec![];
        for input in inputs {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let id = bounded_text(input, "id", 240)?;
            let revision = bounded_text(input, "revision", 100)?;
            if !id.contains(':') {
                return Err(Error::invalid(
                    "Evidence identifiers must be source-qualified",
                ));
            }
            if !seen.insert((id.clone(), revision.clone())) {
                return Err(Error::invalid(
                    "Repeated revision identity within one bundle",
                ));
            }
            for field in [
                "published_at",
                "source_edited_at",
                "event_at",
                "origin_group",
                "dependence",
            ] {
                if let Some(v) = input.get(field)
                    && !v.is_null()
                    && v.as_str().is_none_or(|x| x.is_empty() || x.len() > 500)
                {
                    return Err(Error::invalid(format!(
                        "{field} must be a bounded string or null"
                    )));
                }
            }
            let source = required(input, "source")?;
            let src = srcmap
                .get(source)
                .ok_or_else(|| Error::invalid("Evidence refers to an undeclared source"))?;
            let text = bounded_text(input, "text", 64 * 1024)?;
            if input["fictional"] != fictional {
                return Err(Error::invalid(
                    "Mixed fictional and real evidence is not permitted in one bundle",
                ));
            }
            let acq = timestamp(required(input, "acquired_at")?)?;
            let pubtime = input["published_at"].as_str().map(timestamp).transpose()?;
            let mut e = input.clone();
            e["id"] = json!(id);
            e["revision"] = json!(revision);
            e["acquired_at"] = json!(acq);
            e["published_at"] = json!(pubtime);
            e["source_label"] = src["label"].clone();
            e["family"] = src["family"].clone();
            e["event_at"] = input.get("event_at").cloned().unwrap_or(Value::Null);
            e["origin_group"] = input.get("origin_group").cloned().unwrap_or(Value::Null);
            if required(input, "availability")? != "available" {
                return Err(Error::invalid(
                    "This importer only accepts locally available retained evidence; reference-only and expiry states require the retention workflow",
                ));
            }
            e["availability"] = json!("available");
            e["language"] = json!(required(input, "language")?);
            if let Some(s) = input["source_edited_at"].as_str() {
                e["source_edited_at"] = json!(timestamp(s)?)
            }
            if let Some(s) = input["event_at"].as_str() {
                e["event_at"] = json!(timestamp(s)?)
            }
            if input["geometry"].is_null() {
                e["geometry"] = Value::Null;
                if input["location_precision"] != "unresolved"
                    && input["location_precision"] != "unknown"
                {
                    return Err(Error::invalid(
                        "Absent geometry must be labeled unresolved or unknown",
                    ));
                }
            } else {
                let g = &input["geometry"];
                let cs = g["coordinates"]
                    .as_array()
                    .ok_or_else(|| Error::invalid("Point coordinates required"))?;
                if g["type"] != "Point" || cs.len() != 2 {
                    return Err(Error::invalid(
                        "Initial evidence geometry supports a two-dimensional GeoJSON Point or null",
                    ));
                }
                let lon = cs[0]
                    .as_f64()
                    .ok_or_else(|| Error::invalid("Invalid longitude"))?;
                let lat = cs[1]
                    .as_f64()
                    .ok_or_else(|| Error::invalid("Invalid latitude"))?;
                if !lon.is_finite() || !lat.is_finite() || lon.abs() > 180. || lat.abs() > 90. {
                    return Err(Error::invalid("Coordinates outside WGS84 bounds"));
                }
                bounded_text(input, "location_precision", 100)?;
            }
            let cit: Citation = serde_json::from_value(input["citation"].clone())?;
            if cit.evidence_id != id
                || cit.revision != revision
                || cit.start_character >= cit.end_character
                || cit.end_character > text.chars().count()
            {
                return Err(Error::invalid(
                    "Imported citation must identify an exact in-range Unicode span of its own revision",
                ));
            }
            let original = serde_json::to_vec(input)?;
            let object_hash = self.write_object(&original)?;
            e["object_hash"] = json!(object_hash);
            entries.push((e, original.len()));
        }
        self.sync_object_directories(
            entries
                .iter()
                .map(|(e, _)| e["object_hash"].as_str().unwrap().to_string())
                .chain(std::iter::once(raw_hash.clone())),
            cancel,
        )?;
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        if fictional
            && tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='live_workspace' AND value='true')",
                [],
                |r| r.get::<_, bool>(0),
            )?
        {
            return Err(Error::conflict(
                "This workspace was opened for live accounts. Import the fictional demo into a separate data directory.",
            ));
        }
        if let Some(existing_fictional) = tx
            .query_row(
                "SELECT json_extract(json,'$.fictional') FROM sources LIMIT 1",
                [],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            && existing_fictional != fictional
        {
            return Err(Error::conflict(
                "Fictional and real evidence require separate workspace directories; this publication cannot mix them",
            ));
        }
        for (id, src) in &srcmap {
            if let Some(old) = json_row(&tx, "SELECT json FROM sources WHERE id=?1", [id])?
                && (old["fictional"] != fictional || old["family"] != src["family"])
            {
                return Err(Error::conflict(
                    "Source identity cannot change evidence family or fictional status",
                ));
            }
            tx.execute(
                "INSERT OR IGNORE INTO sources(id,json) VALUES(?1,?2)",
                params![id, string(src)],
            )?;
            node(
                &tx,
                id,
                "source_instance",
                src["label"].as_str().unwrap_or(id),
                src,
            )?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
            params![raw_hash, original.len() as i64],
        )?;
        node(
            &tx,
            run_id,
            "run_step",
            "Validate and import retained evidence",
            &json!({"status":"succeeded","operation":"strict_json_import","input_hash":raw_hash,"input_records":entries.len(),"model_calls":0,"started_at":now(),"field_mappings":[{"input":"text","output":"text","operation":"preserve exact Unicode text"},{"input":"geometry","output":"geometry","operation":"preserve without inventing coordinates"}]}),
        )?;
        let mut accepted = 0;
        let mut reused = 0;
        let mut changed = vec![];
        for (e, bytes_len) in &entries {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let id = required(e, "id")?;
            let rev = required(e, "revision")?;
            if let Some(source) = tx
                .query_row(
                    "SELECT source FROM evidence WHERE id=?1 LIMIT 1",
                    [id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                && e["source"] != source
            {
                return Err(Error::conflict(
                    "An evidence identity cannot change source across revisions",
                ));
            }
            if let Some(old) = json_row(
                &tx,
                "SELECT json FROM evidence WHERE id=?1 AND revision=?2",
                params![id, rev],
            )? {
                if old != *e {
                    return Err(Error::conflict(
                        "An existing evidence revision is immutable; supply a new revision identity",
                    ));
                }
                reused += 1;
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
                params![e["object_hash"].as_str(), *bytes_len as i64],
            )?;
            tx.execute("INSERT INTO evidence(id,revision,source,acquired_at,published_at,lon,lat,origin_group,text,object_hash,json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![id,rev,e["source"].as_str(),e["acquired_at"].as_str(),e["published_at"].as_str(),e["geometry"]["coordinates"][0].as_f64(),e["geometry"]["coordinates"][1].as_f64(),e["origin_group"].as_str(),e["text"].as_str(),e["object_hash"].as_str(),string(e)])?;
            let root = format!("{id}@{rev}");
            node(
                &tx,
                &root,
                "evidence_revision",
                &root,
                &json!({"evidence_id":id,"revision":rev,"object_hash":e["object_hash"],"acquired_at":e["acquired_at"],"fictional":fictional}),
            )?;
            edge(
                &tx,
                e["source"].as_str().unwrap(),
                &root,
                "acquired_from",
                &json!({"retained_original":e["object_hash"]}),
            )?;
            edge(
                &tx,
                &root,
                run_id,
                "input_record",
                &json!({"evidence_id":id,"revision":rev}),
            )?;
            accepted += 1;
            changed.push(id.to_string());
        }
        for (e, _) in &entries {
            for (key, relationship) in [
                ("copies_from", "repeated_by"),
                ("supersedes", "superseded_by"),
            ] {
                if let Some(reference) = e.get(key) {
                    let prior = Self::evidence(
                        &tx,
                        required(reference, "id")?,
                        required(reference, "revision")?,
                    )?;
                    if key == "copies_from"
                        && !prior["origin_group"].is_null()
                        && !e["origin_group"].is_null()
                        && prior["origin_group"] != e["origin_group"]
                    {
                        return Err(Error::invalid(
                            "A copied report cannot declare a different observed origin group from its source",
                        ));
                    }
                    if key == "supersedes" && prior["id"] != e["id"] {
                        return Err(Error::invalid(
                            "A revision can only supersede its own evidence identity",
                        ));
                    }
                    let parent = format!(
                        "{}@{}",
                        required(reference, "id")?,
                        required(reference, "revision")?
                    );
                    let child = format!("{}@{}", required(e, "id")?, required(e, "revision")?);
                    edge(
                        &tx,
                        &parent,
                        &child,
                        relationship,
                        &json!({"meaning":if key=="copies_from"{"Shared origin is not independent confirmation"}else{"Earlier exact revision remains retained"}}),
                    )?;
                }
            }
        }
        let pubid = ident("publication");
        let created = now();
        tx.execute(
            "INSERT INTO publications VALUES(?1,?2,?3,'complete',?4)",
            params![
                pubid,
                bundle["title"].as_str().unwrap_or("Imported evidence"),
                created,
                raw_hash
            ],
        )?;
        retain_acquisition(&tx, bundle, run_id, &bundle_hash, &raw_hash, &pubid)?;
        crate::publication_history::inherit(&tx, &pubid, Self::latest(&tx)?.as_deref())?;
        tx.execute("DELETE FROM membership WHERE publication_id=?1 AND evidence_rowid NOT IN (SELECT e.rowid FROM evidence e WHERE e.rowid=(SELECT x.rowid FROM evidence x WHERE x.id=e.id ORDER BY CASE WHEN json_extract(x.json,'$.upstream_state')='removed' THEN coalesce(json_extract(x.json,'$.source_edited_at'),x.acquired_at) ELSE coalesce(json_extract(x.json,'$.source_edited_at'),x.published_at,x.acquired_at) END DESC,x.acquired_at DESC,x.revision DESC LIMIT 1))",[&pubid])?;
        // Source edit/publication time wins over late acquisition; explicit
        // removal has its observed acquisition time. Match collection ordering
        // so an unrelated import cannot resurrect a removed source message.
        tx.execute("INSERT INTO membership SELECT ?1,e.rowid FROM evidence e WHERE e.rowid=(SELECT x.rowid FROM evidence x WHERE x.id=e.id ORDER BY CASE WHEN json_extract(x.json,'$.upstream_state')='removed' THEN coalesce(json_extract(x.json,'$.source_edited_at'),x.acquired_at) ELSE coalesce(json_extract(x.json,'$.source_edited_at'),x.published_at,x.acquired_at) END DESC,x.acquired_at DESC,x.revision DESC LIMIT 1)",[&pubid])?;
        node(
            &tx,
            &pubid,
            "publication",
            bundle["title"]
                .as_str()
                .unwrap_or("Complete imported publication"),
            &json!({"status":"complete","created_at":created,"input_hash":raw_hash,"recipe_revision":"strict-import-v1"}),
        )?;
        edge(
            &tx,
            run_id,
            &pubid,
            "published",
            &json!({"accepted":accepted,"reused":reused}),
        )?;
        if let Some(prior) = Self::latest(&tx)? {
            edge(
                &tx,
                &prior,
                &pubid,
                "extends_publication",
                &json!({"meaning":"Cumulative retained latest revisions; earlier publication remains available"}),
            )?;
        }
        tx.execute("INSERT INTO metadata VALUES('current_publication',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[&pubid])?;
        crate::processing_store::carry_interpretations(&tx, &pubid)?;
        if bundle.get("baseline_brief").is_some() {
            self.import_fixture_work(&tx, bundle)?;
        }
        self.evaluate_watches(&tx, &pubid)?;
        tx.execute(
            "INSERT INTO import_bundles VALUES(?1,?2)",
            params![bundle_hash, pubid],
        )?;
        let event = Self::event(
            &tx,
            "publication",
            Some(&pubid),
            changed.into_iter().take(100).collect(),
        )?;
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        tx.commit()?;
        let _ = self.events.send(event);
        Ok(
            json!({"publication_id":pubid,"accepted":accepted,"reused":reused,"original_input_hash":raw_hash,"warnings":import_warnings(bundle)}),
        )
    }
    fn import_fixture_work(&self, c: &Connection, bundle: &Value) -> Result<()> {
        let baseline = &bundle["baseline_brief"];
        let inv = &bundle["investigation"];
        let invid = required(inv, "id")?;
        let briefid = required(baseline, "id")?;
        let briefrev = required(baseline, "revision")?;
        if bundle["fictional"] != true
            || invid != "demo:investigation-1"
            || briefid != "demo:brief-1"
        {
            return Err(Error::invalid(
                "Bundled analyst work currently supports only the supplied fictional demonstration; import reporting separately and create analyst work through its explicit API",
            ));
        }
        if json_row(
            c,
            "SELECT json FROM briefs WHERE id=?1 AND revision=?2",
            params![briefid, briefrev],
        )?
        .is_some()
        {
            return Ok(());
        }
        let snapshot = &baseline["input_snapshot"];
        let snapid = required(snapshot, "id")?;
        let cutoff = timestamp(required(baseline, "knowledge_cutoff")?)?;
        c.execute("INSERT INTO publications VALUES(?1,'Frozen fixture baseline · 09:15 cutoff',?2,'frozen',NULL)",params![snapid,cutoff])?;
        crate::publication_history::inherit(c, snapid, None)?;
        node(
            c,
            snapid,
            "publication",
            "Frozen reporting membership at 09:15",
            &json!({"status":"frozen","knowledge_cutoff":cutoff,"meaning":snapshot["meaning"]}),
        )?;
        let freeze = ident("freeze");
        node(
            c,
            &freeze,
            "run_step",
            "Validate and freeze explicit input snapshot",
            &json!({"operation":"explicit_snapshot_import","status":"succeeded","model_calls":0}),
        )?;
        for rf in snapshot["evidence_revisions"]
            .as_array()
            .ok_or_else(|| Error::invalid("Baseline membership required"))?
        {
            let id = required(rf, "id")?;
            let rev = required(rf, "revision")?;
            let e = Self::evidence(c, id, rev)?;
            if required(&e, "acquired_at")? > cutoff.as_str() {
                return Err(Error::invalid(
                    "Frozen snapshot includes evidence acquired after its knowledge cutoff",
                ));
            }
            c.execute(
                "INSERT INTO membership SELECT ?1,rowid FROM evidence WHERE id=?2 AND revision=?3",
                params![snapid, id, rev],
            )?;
            edge(c, &format!("{id}@{rev}"), &freeze, "input_record", rf)?;
        }
        edge(
            c,
            &freeze,
            snapid,
            "published",
            &json!({"knowledge_cutoff":cutoff}),
        )?;
        let mut scope = Scope::fixture(&inv["scope"])?;
        scope.publication_id = None;
        let claims = vec![
            json!({"id":"demo:claim-access","text":"The reporting establishes an unplanned access disruption at fictional Port Selene.","state":"unresolved","citations":baseline["citations"]}),
        ];
        let investigation = json!({"id":invid,"title":"Port Selene · evidence and assessment","question":inv["question"],"scope":scope,"version":1,"notes":"","pins":[],"assessments":[],"claims":claims});
        c.execute(
            "INSERT OR IGNORE INTO investigations VALUES(?1,?2)",
            params![invid, string(&investigation)],
        )?;
        node(
            c,
            invid,
            "investigation",
            "Port Selene investigation",
            &json!({"question":inv["question"],"authorship":"fixture","fictional":true}),
        )?;
        let mut frozen_scope = Scope::fixture(&baseline["scope"])?;
        frozen_scope.publication_id = Some(snapid.into());
        let citations: Vec<Citation> = serde_json::from_value(baseline["citations"].clone())?;
        Self::check_citations(c, &citations, Some(&frozen_scope))?;
        let brief = json!({"id":briefid,"revision":briefrev,"version":1,"title":"Port Selene · saved baseline","investigation_id":invid,"question":inv["question"],"finding":baseline["finding"],"assessment":baseline["assessment"],"limitations":baseline["limitations"],"citations":citations,"scope":frozen_scope,"publication_id":snapid,"knowledge_cutoff":cutoff,"created_at":cutoff,"authorship":"fixture","unsupported":false});
        save_brief_row(c, &brief)?;
        Ok(())
    }
    pub fn create_investigation(&self, v: Value) -> Result<Value> {
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let title = bounded_text(&v, "title", 300)?;
        let question = bounded_text(&v, "question", 4000)?;
        let mut scope = Scope::parse(&v["scope"])?;
        Self::resolve_scope(&tx, &mut scope)?;
        let id = ident("investigation");
        let inv = json!({"id":id,"title":title,"question":question,"scope":scope,"version":1,"notes":"","pins":[],"assessments":[],"claims":[]});
        tx.execute(
            "INSERT INTO investigations VALUES(?1,?2)",
            params![id, string(&inv)],
        )?;
        node(
            &tx,
            &id,
            "investigation",
            &title,
            &json!({"question":question,"created_at":now()}),
        )?;
        let ev = Self::event(
            &tx,
            "analyst_work",
            scope.publication_id.as_deref(),
            vec![id],
        )?;
        tx.commit()?;
        let _ = self.events.send(ev);
        Ok(inv)
    }
    pub fn update_investigation(&self, id: &str, v: Value, assessment: bool) -> Result<Value> {
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let mut inv = json_row(&tx, "SELECT json FROM investigations WHERE id=?1", [id])?
            .ok_or_else(|| Error::missing("Investigation unavailable"))?;
        check_version(&inv, &v)?;
        if assessment {
            let claim_id = bounded_text(&v, "claim_id", 240)?;
            let status = required(&v, "status")?;
            if ![
                "supported",
                "contested",
                "insufficiently_supported",
                "not_assessed",
            ]
            .contains(&status)
            {
                return Err(Error::invalid("Invalid assessment status"));
            }
            let rationale = bounded_text(&v, "rationale", 20000)?;
            let mut scope = Scope::parse(&v["scope"])?;
            let pubid = Self::resolve_scope(&tx, &mut scope)?;
            if v["publication_id"] != pubid {
                return Err(Error::invalid(
                    "Assessment publication and scope must agree",
                ));
            }
            let citations: Vec<Citation> = serde_json::from_value(v["citations"].clone())?;
            Self::check_citations(&tx, &citations, Some(&scope))?;
            if citations.is_empty() && status != "not_assessed" {
                return Err(Error::invalid(
                    "An assessment needs exact support; use not_assessed until support is selected",
                ));
            }
            if !inv["claims"]
                .as_array()
                .ok_or_else(Error::internal)?
                .iter()
                .any(|x| x["id"] == claim_id)
            {
                return Err(Error::invalid(
                    "Unknown claim; add an explicit claim to this investigation first",
                ));
            }
            let aid = ident("assessment");
            let a = json!({"id":aid,"claim_id":claim_id,"status":status,"rationale":rationale,"citations":citations,"scope":scope,"publication_id":pubid,"created_at":now(),"actor":"local analyst"});
            inv["assessments"]
                .as_array_mut()
                .ok_or_else(Error::internal)?
                .push(a.clone());
            node(&tx, &aid, "assessment", status, &a)?;
            edge(&tx, &aid, id, "recorded_in", &json!({}))?;
            for cit in &citations {
                edge(
                    &tx,
                    &format!("{}@{}", cit.evidence_id, cit.revision),
                    &aid,
                    "considered_by",
                    &value(cit),
                )?;
            }
        } else {
            for (key, max) in [("title", 300), ("question", 4000)] {
                if v.get(key).is_some() {
                    inv[key] = json!(bounded_text(&v, key, max)?);
                }
            }
            if let Some(notes) = v.get("notes") {
                if !notes.is_string() || notes.as_str().unwrap().len() > 100_000 {
                    return Err(Error::invalid("Notes must be text up to 100 kB"));
                }
                inv["notes"] = notes.clone();
            }
            if let Some(scope) = v.get("scope") {
                let mut scope = Scope::parse(scope)?;
                Self::resolve_scope(&tx, &mut scope)?;
                inv["scope"] = value(scope);
            }
            if let Some(pins) = v.get("pins") {
                let pins = pins
                    .as_array()
                    .ok_or_else(|| Error::invalid("Pins must be an array"))?;
                if pins.len() > 200 {
                    return Err(Error::invalid(
                        "At most 200 evidence pins per investigation",
                    ));
                }
                let mut seen = HashSet::new();
                for pin in pins {
                    let id = required(pin, "evidence_id")?;
                    let rev = required(pin, "revision")?;
                    Self::evidence(&tx, id, rev)?;
                    if !seen.insert((id, rev)) {
                        return Err(Error::invalid("Duplicate evidence pin"));
                    }
                    if pin["reason"].as_str().is_none_or(|r| r.len() > 4000) {
                        return Err(Error::invalid("Pin reason must be text up to 4000 bytes"));
                    }
                }
                inv["pins"] = json!(pins);
            }
            // Explicitly authored claims are separate from raw reports and subsequent assessments.
            if let Some(claims) = v.get("claims") {
                let claims = claims
                    .as_array()
                    .ok_or_else(|| Error::invalid("Claims must be an array"))?;
                if claims.len() > 100 {
                    return Err(Error::invalid("At most 100 claims per investigation"));
                }
                let mut claim_ids = HashSet::new();
                for claim in claims {
                    let claim_id = bounded_text(claim, "id", 240)?;
                    if !claim_ids.insert(claim_id) {
                        return Err(Error::invalid(
                            "Claim identities must be unique within an investigation",
                        ));
                    }
                    bounded_text(claim, "text", 4000)?;
                    if !["asserted", "denied", "corrected", "unresolved", "withdrawn"]
                        .contains(&required(claim, "state")?)
                    {
                        return Err(Error::invalid("Invalid claim state"));
                    }
                    let cs: Vec<Citation> = serde_json::from_value(claim["citations"].clone())?;
                    Self::check_citations(&tx, &cs, None)?;
                }
                for assessment in inv["assessments"].as_array().ok_or_else(Error::internal)? {
                    let prior = inv["claims"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|x| x["id"] == assessment["claim_id"]);
                    let proposed = claims.iter().find(|x| x["id"] == assessment["claim_id"]);
                    if proposed.is_none() || proposed != prior {
                        return Err(Error::conflict(
                            "A claim referenced by an assessment is immutable; preserve it and author a new claim identity for changed meaning",
                        ));
                    }
                }
                inv["claims"] = json!(claims);
            }
        }
        inv["version"] = json!(inv["version"].as_u64().unwrap_or(0) + 1);
        tx.execute(
            "UPDATE investigations SET json=?1 WHERE id=?2",
            params![string(&inv), id],
        )?;
        let ev = Self::event(
            &tx,
            "analyst_work",
            Self::latest(&tx)?.as_deref(),
            vec![id.into()],
        )?;
        tx.commit()?;
        let _ = self.events.send(ev);
        Ok(inv)
    }
    pub fn save_brief(&self, v: Value) -> Result<Value> {
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let title = bounded_text(&v, "title", 300)?;
        let question = bounded_text(&v, "question", 4000)?;
        let finding = bounded_text(&v, "finding", 50000)?;
        let invid = required(&v, "investigation_id")?;
        if json_row(&tx, "SELECT json FROM investigations WHERE id=?1", [invid])?.is_none() {
            return Err(Error::missing("Investigation unavailable"));
        }
        let mut scope = Scope::parse(&v["scope"])?;
        let pubid = Self::resolve_scope(&tx, &mut scope)?;
        if v["publication_id"] != pubid {
            return Err(Error::invalid("Brief publication and scope must agree"));
        }
        let citations: Vec<Citation> = serde_json::from_value(v["citations"].clone())?;
        let unsupported = v["unsupported"].as_bool().unwrap_or(false);
        if citations.is_empty() && !unsupported {
            return Err(Error::invalid(
                "A finding requires exact citations or explicit unsupported:true",
            ));
        }
        Self::check_citations(&tx, &citations, Some(&scope))?;
        let assessment = bounded_text(&v, "assessment", 10000)?;
        let limitations = v["limitations"]
            .as_array()
            .ok_or_else(|| Error::invalid("Limitations must be a string array"))?;
        if limitations.len() > 100
            || limitations
                .iter()
                .any(|x| x.as_str().is_none_or(|s| s.len() > 4000))
        {
            return Err(Error::invalid("Invalid limitations"));
        }
        let id = v["id"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| ident("brief"));
        let previous = json_row(
            &tx,
            "SELECT json FROM briefs WHERE id=?1 ORDER BY version DESC LIMIT 1",
            [&id],
        )?;
        let version = if let Some(old) = previous {
            check_version(&old, &v)?;
            if old["investigation_id"] != invid {
                return Err(Error::conflict(
                    "A brief cannot be moved to another investigation",
                ));
            }
            old["version"].as_u64().unwrap() + 1
        } else {
            if v.get("id").is_some() {
                return Err(Error::missing("Existing brief identity unavailable"));
            }
            1
        };
        let revision = format!("v{version}");
        let created = now();
        let brief = json!({"id":id,"revision":revision,"version":version,"title":title,"investigation_id":invid,"question":question,"finding":finding,"assessment":assessment,"limitations":limitations,"citations":citations,"scope":scope,"publication_id":pubid,"knowledge_cutoff":created,"created_at":created,"authorship":"analyst","unsupported":unsupported});
        save_brief_row(&tx, &brief)?;
        let ev = Self::event(&tx, "analyst_work", Some(&pubid), vec![id])?;
        tx.commit()?;
        let _ = self.events.send(ev);
        Ok(brief)
    }
    pub fn brief(&self, id: &str, revision: Option<&str>) -> Result<Value> {
        let c = self.connect()?;
        brief_row(&c, id, revision)
    }
    pub fn export(&self, id: &str, revision: Option<&str>) -> Result<Value> {
        let c = self.connect()?;
        let brief = brief_row(&c, id, revision)?;
        let citations: Vec<Citation> = serde_json::from_value(brief["citations"].clone())?;
        let scope = Scope::parse(&brief["scope"])?;
        let support = Self::check_citations(&c, &citations, Some(&scope))?;
        let members = scope_members(&c, &scope)?;
        Ok(
            json!({"schema_version":1,"application":"Relic","brief":brief,"support":support,"scope_membership":members,"omissions":coverage(),"exported_at":now(),"reproducibility":"Exact retained reporting revisions and frozen scope; arbitrary known-at-time and live sensor reconstruction are not implemented."}),
        )
    }
    pub fn watch_preview(&self, v: Value) -> Result<Value> {
        validate_watch(&v)?;
        let c = self.connect()?;
        let mut scope = Scope::parse(&v["scope"])?;
        Self::resolve_scope(&c, &mut scope)?;
        let q = query_conn(
            &c,
            Query {
                scope: value(&scope),
                generation: 0,
                cursor: None,
                limit: Some(1),
            },
            false,
            false,
        )?;
        Ok(
            json!({"matched":q["total"],"groups":q["origin_groups"],"frequency":"unknown","coverage":coverage(),"baseline_policy":"fresh","estimated_cost":"no external calls"}),
        )
    }
    pub fn create_watch(&self, v: Value) -> Result<Value> {
        validate_watch(&v)?;
        if v["baseline_policy"] != "fresh" {
            return Err(Error::invalid(
                "Activation requires explicit baseline_policy: fresh",
            ));
        }
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let mut scope = Scope::parse(&v["scope"])?;
        let pubid = Self::resolve_scope(&tx, &mut scope)?;
        let members = scope_members(&tx, &scope)?;
        scope.publication_id = None;
        let id = ident("watch");
        let watch = json!({"id":id,"title":v["title"],"question":v["question"],"scope":scope,"conditions":v["conditions"],"status":"active","baseline_publication_id":pubid,"created_at":now(),"frequency":"unknown"});
        tx.execute(
            "INSERT INTO watches VALUES(?1,?2)",
            params![id, string(&watch)],
        )?;
        for e in members {
            tx.execute(
                "INSERT INTO watch_seen VALUES(?1,?2,?3,?4)",
                params![
                    id,
                    e["evidence_id"].as_str(),
                    e["revision"].as_str(),
                    e["origin_group"].as_str()
                ],
            )?;
        }
        let ev = Self::event(&tx, "watch", Some(&pubid), vec![id])?;
        tx.commit()?;
        let _ = self.events.send(ev);
        Ok(watch)
    }
    pub(crate) fn evaluate_watches(&self, c: &Connection, pubid: &str) -> Result<()> {
        for watch in all_json(c, "SELECT json FROM watches")? {
            if watch["status"] != "active" {
                continue;
            }
            let id = required(&watch, "id")?;
            let mut scope = Scope::parse(&watch["scope"])?;
            scope.publication_id = Some(pubid.into());
            let members = scope_members(c, &scope)?;
            let mut stmt = c.prepare(
                "SELECT identity,revision,origin_group FROM watch_seen WHERE watch_id=?1",
            )?;
            let seen: HashMap<String, (String, Option<String>)> = stmt
                .query_map([id], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
                .collect::<std::result::Result<_, _>>()?;
            let known_origins: HashSet<String> =
                seen.values().filter_map(|(_, o)| o.clone()).collect();
            for e in members {
                let eid = required(&e, "evidence_id")?;
                let rev = required(&e, "revision")?;
                if seen.get(eid).is_some_and(|(r, _)| r == rev) {
                    continue;
                }
                let origin = e["origin_group"].as_str();
                let correction = seen.contains_key(eid);
                let is_new_origin = origin.is_some_and(|o| !known_origins.contains(o));
                let conditions = watch["conditions"].as_array().ok_or_else(Error::internal)?;
                let attention = (correction && conditions.iter().any(|x| x == "source correction"))
                    || (is_new_origin && conditions.iter().any(|x| x == "new observed origin"));
                let key = origin
                    .map(String::from)
                    .unwrap_or_else(|| format!("unknown:{eid}"));
                let previous = json_row(
                    c,
                    "SELECT json FROM alerts WHERE watch_id=?1 AND group_key=?2",
                    params![id, key],
                )?;
                if attention || previous.is_some() {
                    let mut a=previous.unwrap_or_else(||json!({"id":ident("alert"),"watch_id":id,"reason":if correction{"Source revision changed; assessment needs review"}else{"New observed origin entered scope"},"evidence":[],"origin_group":origin,"first_at":now(),"latest_at":now(),"review_status":"new","delivery_status":"in_app"}));
                    let evidence = a["evidence"].as_array_mut().ok_or_else(Error::internal)?;
                    if !evidence
                        .iter()
                        .any(|r| r["evidence_id"] == eid && r["revision"] == rev)
                    {
                        evidence.push(json!({"evidence_id":eid,"revision":rev}))
                    }
                    if attention && a["review_status"] != "new" {
                        let review = json!({"review_status":a["review_status"],"snoozed_until":a["snoozed_until"],"superseded_at":now(),"reason":"New material evidence requires review"});
                        if !a["review_history"].is_array() {
                            a["review_history"] = json!([]);
                        }
                        a["review_history"].as_array_mut().unwrap().push(review);
                        a["review_status"] = json!("new");
                        a.as_object_mut().unwrap().remove("snoozed_until");
                    }
                    a["latest_at"] = json!(now());
                    if attention && correction {
                        a["reason"] = json!(
                            "Source revision changed; earlier triggering evidence is retained"
                        )
                    }
                    c.execute("INSERT INTO alerts(id,watch_id,group_key,json) VALUES(?1,?2,?3,?4) ON CONFLICT(watch_id,group_key) DO UPDATE SET json=excluded.json",params![a["id"].as_str(),id,key,string(&a)])?;
                }
                c.execute("INSERT INTO watch_seen VALUES(?1,?2,?3,?4) ON CONFLICT(watch_id,identity) DO UPDATE SET revision=excluded.revision,origin_group=excluded.origin_group",params![id,eid,rev,origin])?;
            }
        }
        Ok(())
    }
    pub fn update_watch(&self, id: &str, v: Value) -> Result<Value> {
        let status = required(&v, "status")?;
        if !["active", "paused"].contains(&status) {
            return Err(Error::invalid("Watch status must be active or paused"));
        }
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let c = self.connect()?;
        let mut w = json_row(&c, "SELECT json FROM watches WHERE id=?1", [id])?
            .ok_or_else(|| Error::missing("Watch unavailable"))?;
        w["status"] = json!(status);
        c.execute(
            "UPDATE watches SET json=?1 WHERE id=?2",
            params![string(&w), id],
        )?;
        Ok(w)
    }
    pub fn update_alert(&self, id: &str, v: Value) -> Result<Value> {
        let status = required(&v, "review_status")?;
        if !["new", "acknowledged", "irrelevant", "snoozed"].contains(&status) {
            return Err(Error::invalid("Invalid alert review status"));
        }
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let c = self.connect()?;
        let mut a = json_row(&c, "SELECT json FROM alerts WHERE id=?1", [id])?
            .ok_or_else(|| Error::missing("Alert unavailable"))?;
        a["review_status"] = json!(status);
        if status == "snoozed" {
            a["snoozed_until"] = json!(timestamp(required(&v, "snoozed_until")?)?)
        } else {
            a.as_object_mut().unwrap().remove("snoozed_until");
        }
        c.execute(
            "UPDATE alerts SET json=?1 WHERE id=?2",
            params![string(&a), id],
        )?;
        Ok(a)
    }
    pub fn list(&self, kind: &str) -> Result<Value> {
        let sql = match kind {
            "jobs" => "SELECT json FROM jobs ORDER BY rowid DESC LIMIT 100",
            "alerts" => "SELECT json FROM alerts ORDER BY rowid DESC LIMIT 500",
            _ => return Err(Error::missing("Unsupported collection")),
        };
        Ok(json!({"items":all_json(&*self.connect()?,sql)?}))
    }
    pub fn lineage(&self, root: Option<&str>, depth: usize, direction: &str) -> Result<Value> {
        if !["both", "upstream", "downstream"].contains(&direction) {
            return Err(Error::invalid("Invalid lineage direction"));
        }
        let c = self.connect()?;
        let root = root
            .map(String::from)
            .or(Self::latest(&c)?)
            .ok_or_else(|| Error::missing("No lineage exists before import"))?;
        let depth = depth.clamp(1, 8);
        let exists = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM lineage_nodes WHERE id=?1)",
            [&root],
            |r| r.get::<_, bool>(0),
        )?;
        if !exists {
            return Err(Error::missing("Lineage root unavailable"));
        }
        let mut visited = HashSet::from([root.clone()]);
        let mut queue = VecDeque::from([(root, 0usize)]);
        let mut edges = HashMap::new();
        let mut hidden = HashSet::new();
        while let Some((id, level)) = queue.pop_front() {
            let mut stmt=c.prepare(match direction{"upstream"=>"SELECT id,source,target,relationship,detail FROM lineage_edges WHERE target=?1 LIMIT 201","downstream"=>"SELECT id,source,target,relationship,detail FROM lineage_edges WHERE source=?1 LIMIT 201",_=>"SELECT id,source,target,relationship,detail FROM lineage_edges WHERE source=?1 OR target=?1 LIMIT 201"})?;
            for row in stmt.query_map([&id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })? {
                let (eid, source, target, rel, detail) = row?;
                let next = if source == id {
                    target.clone()
                } else {
                    source.clone()
                };
                if !visited.contains(&next) {
                    if level >= depth || visited.len() >= 100 {
                        hidden.insert(next);
                        continue;
                    }
                    visited.insert(next.clone());
                    queue.push_back((next, level + 1));
                }
                edges.insert(eid.clone(),json!({"id":eid,"source":source,"target":target,"relationship":rel,"detail":serde_json::from_str::<Value>(&detail)?}));
            }
        }
        let mut nodes = vec![];
        for id in &visited {
            let (kind, label, detail) = c.query_row(
                "SELECT kind,label,detail FROM lineage_nodes WHERE id=?1",
                [id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )?;
            nodes.push(json!({"id":id,"kind":kind,"label":label,"detail":serde_json::from_str::<Value>(&detail)?}));
        }
        nodes.sort_by_key(|v| v["id"].as_str().unwrap().to_string());
        let mut edges: Vec<_> = edges.into_values().collect();
        edges.sort_by_key(|v| v["id"].as_str().unwrap().to_string());
        Ok(
            json!({"nodes":nodes,"edges":edges,"hidden_nodes":hidden.len(),"snapshot":Self::sequence(&c)?.to_string(),"limits":{"nodes":100,"depth":depth,"adjacent_edges_per_node":200},"hidden_count_is_lower_bound":true}),
        )
    }
    pub fn backup(&self) -> Result<Value> {
        // Snapshot without blocking writers. CAS objects never change or disappear.
        // Link only an independently copied prior backup, never the live store.
        let c = self.connect()?;
        let root = self.config.data.join("backups");
        let prior = fs::read_dir(&root)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name().to_string_lossy().starts_with("backup:")
                    && e.path().join("manifest.json").is_file()
            })
            .max_by_key(|e| e.metadata().ok().and_then(|m| m.modified().ok()))
            .map(|e| e.path());
        let temporary = tempfile::Builder::new()
            .prefix("backup-staging-")
            .tempdir_in(&root)?;
        let dir = temporary.path();
        let id = ident("backup");
        let dest = dir.join("relic.sqlite3");
        let mut destination = Connection::open(&dest)?;
        {
            let backup = rusqlite::backup::Backup::new(&c, &mut destination)?;
            backup.run_to_completion(256, Duration::from_millis(5), None)?;
        }
        drop(destination);
        File::open(&dest)?.sync_all()?;
        let created = now();
        let mut manifest = std::io::BufWriter::new(File::create(dir.join("manifest.json"))?);
        let mut header = string(
            &json!({"schema_version":1,"id":id,"created_at":created,"sqlite_version":rusqlite::version(),"database_hash":crate::media_store::file_hash(&dest)?.0}),
        );
        header.pop();
        write!(manifest, "{header},\"objects\":[")?;
        let snapshot = Connection::open(&dest)?;
        let mut stmt = snapshot.prepare("SELECT hash,bytes FROM objects ORDER BY hash")?;
        let mut rows = stmt.query([])?;
        let mut count = 0usize;
        let mut linked = 0usize;
        let mut prefixes = HashSet::new();
        while let Some(row) = rows.next()? {
            let h: String = row.get(0)?;
            let bytes: i64 = row.get(1)?;
            let source = self.config.object_path(&h);
            let (digest, size) = crate::media_store::file_hash(&source)?;
            if digest != h || size as i64 != bytes {
                return Err(Error::internal());
            }
            let target = dir.join("objects").join(&h[..2]).join(&h);
            fs::create_dir_all(target.parent().unwrap())?;
            let mut reused = false;
            if let Some(old) = prior
                .as_ref()
                .map(|p| p.join("objects").join(&h[..2]).join(&h))
                .filter(|p| p.is_file())
            {
                let (digest, size) = crate::media_store::file_hash(&old)?;
                if digest == h && size as i64 == bytes && fs::hard_link(old, &target).is_ok() {
                    reused = true;
                    linked += 1;
                }
            }
            if !reused {
                let mut out = File::create(&target)?;
                std::io::copy(&mut File::open(&source)?, &mut out)?;
                out.sync_all()?;
                let (digest, size) = crate::media_store::file_hash(&target)?;
                if digest != h || size as i64 != bytes {
                    return Err(Error::internal());
                }
            }
            if count > 0 {
                manifest.write_all(b",")?;
            }
            serde_json::to_writer(&mut manifest, &json!({"hash":h,"bytes":bytes}))?;
            prefixes.insert(h[..2].to_owned());
            count += 1;
        }
        drop(rows);
        drop(stmt);
        drop(snapshot);
        manifest.write_all(b"]}")?;
        manifest.flush()?;
        manifest.get_ref().sync_all()?;
        for prefix in prefixes {
            File::open(dir.join("objects").join(prefix))?.sync_all()?;
        }
        if dir.join("objects").exists() {
            File::open(dir.join("objects"))?.sync_all()?;
        }
        File::open(dir)?.sync_all()?;
        fs::rename(dir, root.join(&id))?;
        File::open(root)?.sync_all()?;
        Ok(
            json!({"id":id,"created_at":created,"objects":count,"reused_backup_objects":linked,"verified":false,"detail":"Consistent SQLite snapshot and verified objects saved. Unchanged objects share storage with a previous backup, independently of the live store. Run restore verification to validate exact citations."}),
        )
    }
    pub fn verify_backup(&self, id: &str) -> Result<Value> {
        self.verify_backup_with_retention(id, true)
    }
    pub fn check_backup(&self, id: &str) -> Result<Value> {
        self.verify_backup_with_retention(id, false)
    }
    fn verify_backup_with_retention(&self, id: &str, retain: bool) -> Result<Value> {
        if id.len() > 100
            || !id.starts_with("backup:")
            || !id
                .chars()
                .all(|x| x.is_ascii_alphanumeric() || [':', '-'].contains(&x))
        {
            return Err(Error::invalid("Invalid backup identity"));
        }
        let dir = self.config.data.join("backups").join(id);
        let manifest_path = dir.join("manifest.json");
        if !manifest_path.exists() {
            return Err(Error::missing("Backup unavailable"));
        }
        let manifest: Value = serde_json::from_slice(&fs::read(manifest_path)?)?;
        if manifest["database_hash"] != crate::media_store::file_hash(&dir.join("relic.sqlite3"))?.0
        {
            return Err(Error::invalid(
                "Backup database hash does not match manifest",
            ));
        }
        let parent = self.config.data.join("restore-verifications");
        fs::create_dir_all(&parent)?;
        let temporary = tempfile::Builder::new()
            .prefix("restore-")
            .tempdir_in(parent)?;
        let verify = temporary.path().to_path_buf();
        fs::copy(dir.join("relic.sqlite3"), verify.join("relic.sqlite3"))?;
        let objects = manifest["objects"]
            .as_array()
            .ok_or_else(|| Error::invalid("Invalid backup manifest"))?;
        for object in objects {
            let h = required(object, "hash")?;
            if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Error::invalid("Invalid object hash in backup"));
            }
            let source = dir.join("objects").join(&h[..2]).join(h);
            let (digest, size) = crate::media_store::file_hash(&source)?;
            if digest != h || Some(size) != object["bytes"].as_u64() {
                return Err(Error::invalid("Backup object hash/size mismatch"));
            }
            let dst = verify.join("objects/sha256").join(&h[..2]).join(h);
            fs::create_dir_all(dst.parent().unwrap())?;
            fs::copy(source, dst)?;
        }
        let c = Connection::open(verify.join("relic.sqlite3"))?;
        c.pragma_update(None, "foreign_keys", "ON")?;
        register_query_functions(&c)?;
        let integrity: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if integrity != "ok"
            || c.prepare("PRAGMA foreign_key_check")?
                .query([])?
                .next()?
                .is_some()
        {
            return Err(Error::invalid(
                "Restored database failed integrity or foreign-key checks",
            ));
        }
        let mut stmt = c.prepare("SELECT hash,bytes FROM objects")?;
        let referenced: Vec<(String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        for (h, size) in referenced {
            let (digest, bytes) = crate::media_store::file_hash(
                &verify.join("objects/sha256").join(&h[..2]).join(&h),
            )?;
            if digest != h || bytes as i64 != size {
                return Err(Error::invalid(
                    "A restored database object is missing or invalid",
                ));
            }
        }
        let briefs = all_json(&c, "SELECT json FROM briefs")?;
        for brief in &briefs {
            let cs: Vec<Citation> = serde_json::from_value(brief["citations"].clone())?;
            let scope = Scope::parse(&brief["scope"])?;
            Self::check_citations(&c, &cs, Some(&scope))?;
        }
        for inv in all_json(&c, "SELECT json FROM investigations")? {
            for pin in inv["pins"].as_array().unwrap_or(&vec![]) {
                Self::evidence(
                    &c,
                    required(pin, "evidence_id")?,
                    required(pin, "revision")?,
                )?;
            }
            for a in inv["assessments"].as_array().unwrap_or(&vec![]) {
                let cs: Vec<Citation> = serde_json::from_value(a["citations"].clone())?;
                Self::check_citations(&c, &cs, Some(&Scope::parse(&a["scope"])?))?;
            }
        }
        let subscriptions = crate::subscriptions::verify_subscriptions(&c)?;
        let collection_states = crate::collection_store::verify_collection_states(&c)?;
        let media_assets =
            crate::media_store::verify_media_assets(&c, &verify.join("objects/sha256"))?;
        drop(stmt);
        drop(c);
        let restored_directory = if retain {
            Some(temporary.keep())
        } else {
            temporary.close()?;
            None
        };
        Ok(
            json!({"verified":true,"objects":objects.len(),"briefs":briefs.len(),"subscriptions":subscriptions,"collection_states":collection_states,"media_assets":media_assets,"restored_directory":restored_directory,"verification_copy_retained":retain,"detail":"Restored into a fresh directory; database integrity, foreign keys, retained object hashes and media signatures, collection settings and checkpoints, pins, assessment support and all brief revision citations verified. Active workspace was unchanged."}),
        )
    }
    pub fn replay_events(&self, since: u64) -> Result<(bool, Vec<Value>)> {
        let c = self.connect()?;
        let min: i64 = c.query_row("SELECT coalesce(min(sequence),0) FROM events", [], |r| {
            r.get(0)
        })?;
        let mut stmt =
            c.prepare("SELECT json FROM events WHERE sequence>?1 ORDER BY sequence LIMIT 128")?;
        let rows: Vec<String> = stmt
            .query_map([since as i64], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok((
            since > 0 && since + 1 < min as u64,
            rows.into_iter()
                .map(|s| serde_json::from_str(&s).map_err(Error::from))
                .collect::<Result<_>>()?,
        ))
    }
}

fn retain_acquisition(
    c: &Connection,
    bundle: &Value,
    id: &str,
    semantic_hash: &str,
    raw_hash: &str,
    pubid: &str,
) -> Result<()> {
    c.execute(
        "INSERT INTO acquisitions VALUES(?1,?2,?3,?4,?5)",
        params![id, semantic_hash, raw_hash, pubid, now()],
    )?;
    for e in bundle["evidence_revisions"]
        .as_array()
        .ok_or_else(|| Error::invalid("Evidence revisions required"))?
    {
        c.execute("INSERT INTO evidence_acquisitions SELECT rowid,?1 FROM evidence WHERE id=?2 AND revision=?3",params![id,required(e,"id")?,required(e,"revision")?])?;
    }
    Ok(())
}
fn original_input_hashes(c: &Connection, id: &str, revision: &str) -> Result<Vec<String>> {
    let mut stmt=c.prepare("SELECT DISTINCT a.object_hash FROM acquisitions a JOIN evidence_acquisitions ea ON ea.acquisition_id=a.id JOIN evidence e ON e.rowid=ea.evidence_rowid WHERE e.id=?1 AND e.revision=?2 ORDER BY a.object_hash LIMIT 100")?;
    Ok(stmt
        .query_map(params![id, revision], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?)
}
fn register_query_functions(c: &Connection) -> Result<()> {
    crate::area_geometry::register(c)?;
    c.create_scalar_function(
        "unicode_lower",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let text: String = ctx.get(0)?;
            Ok(text.to_lowercase())
        },
    )?;
    Ok(())
}
fn coverage() -> Vec<String> {
    vec!["Retained reporting only; arbitrary known-at-time replay and live observation layers are unavailable.".into(),"Unknown locations and source dependence remain unresolved. Copied publications are not independent confirmation.".into(),"X and Telegram coverage is bounded by each source policy. Processed readings are derived proposals. Flightradar24 and KeelWatch collection are not enabled.".into()]
}
fn import_warnings(bundle: &Value) -> Vec<String> {
    let mut w = vec![];
    if bundle.get("observations").is_some() {
        w.push("Contextual ship/thermal observations are retained inside the original bundle but are not published as reporting evidence or a continuous route.".into())
    }
    if bundle.get("lineage_example").is_some() {
        w.push("Illustrative lineage is retained as input only. The graph records actual import/publication operations.".into())
    }
    w
}
pub fn all_json(c: &Connection, sql: &str) -> Result<Vec<Value>> {
    let mut stmt = c.prepare(sql)?;
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    rows.into_iter()
        .map(|s| serde_json::from_str(&s).map_err(Error::from))
        .collect()
}
fn json_row<P: rusqlite::Params>(c: &Connection, sql: &str, params: P) -> Result<Option<Value>> {
    let row: Option<String> = c.query_row(sql, params, |r| r.get(0)).optional()?;
    row.map(|s| serde_json::from_str(&s).map_err(Error::from))
        .transpose()
}
fn check_version(old: &Value, v: &Value) -> Result<()> {
    if v["expected_version"].as_u64() != old["version"].as_u64() {
        return Err(Error::conflict(
            "Save conflict: the durable record changed. Reload and reconcile your draft.",
        ));
    }
    Ok(())
}
pub(crate) fn node(
    c: &Connection,
    id: &str,
    kind: &str,
    label: &str,
    detail: &Value,
) -> Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO lineage_nodes VALUES(?1,?2,?3,?4)",
        params![id, kind, label, string(detail)],
    )?;
    Ok(())
}
pub(crate) fn edge(
    c: &Connection,
    source: &str,
    target: &str,
    relationship: &str,
    detail: &Value,
) -> Result<()> {
    let id = hash(format!("{source}\0{target}\0{relationship}\0{}", string(detail)).as_bytes());
    c.execute(
        "INSERT OR IGNORE INTO lineage_edges VALUES(?1,?2,?3,?4,?5)",
        params![id, source, target, relationship, string(detail)],
    )?;
    Ok(())
}
fn save_brief_row(c: &Connection, b: &Value) -> Result<()> {
    let id = required(b, "id")?;
    let revision = required(b, "revision")?;
    c.execute(
        "INSERT INTO briefs VALUES(?1,?2,?3,?4)",
        params![id, revision, b["version"].as_i64(), string(b)],
    )?;
    let root = format!("{id}@{revision}");
    node(c, &root, "brief_revision", required(b, "title")?, b)?;
    edge(
        c,
        required(b, "publication_id")?,
        &root,
        "frozen_scope_membership",
        &b["scope"],
    )?;
    edge(
        c,
        &root,
        required(b, "investigation_id")?,
        "saved_in",
        &json!({}),
    )?;
    for cit in b["citations"].as_array().ok_or_else(Error::internal)? {
        edge(
            c,
            &format!(
                "{}@{}",
                required(cit, "evidence_id")?,
                required(cit, "revision")?
            ),
            &root,
            "cited_by",
            cit,
        )?;
    }
    Ok(())
}
fn brief_row(c: &Connection, id: &str, revision: Option<&str>) -> Result<Value> {
    let row = if let Some(revision) = revision {
        json_row(
            c,
            "SELECT json FROM briefs WHERE id=?1 AND revision=?2",
            params![id, revision],
        )?
    } else {
        json_row(
            c,
            "SELECT json FROM briefs WHERE id=?1 ORDER BY version DESC LIMIT 1",
            [id],
        )?
    };
    row.ok_or_else(|| Error::missing("Brief revision unavailable"))
}
fn validate_watch(v: &Value) -> Result<()> {
    bounded_text(v, "title", 300)?;
    bounded_text(v, "question", 4000)?;
    let cs = v["conditions"]
        .as_array()
        .ok_or_else(|| Error::invalid("Watch conditions required"))?;
    if cs.is_empty()
        || cs.len() > 3
        || cs.iter().any(|x| {
            !["new observed origin", "source correction"].contains(&x.as_str().unwrap_or(""))
        })
    {
        return Err(Error::invalid("Unsupported watch conditions"));
    }
    Ok(())
}
/// Attempts recorded as failures before OpenRouter's pre-flight credit rejection
/// (HTTP 402, never generated or billed) was recognised become cooled-down,
/// zero-cost deferrals, so their messages and pairs are read once credits return.
/// Each is checked against its retained, hash-verified response.
fn reclassify_credit_rejections(c: &Connection, data: &std::path::Path) -> Result<usize> {
    let load = |h: &str| -> Option<Vec<u8>> {
        let bytes = fs::read(data.join("objects/sha256").join(h.get(..2)?).join(h)).ok()?;
        (hash(&bytes) == h).then_some(bytes)
    };
    let at = now();
    let mut changed = 0;
    let readings = c
        .prepare("SELECT id,json_extract(json,'$.audit_hash') FROM processing_attempts WHERE status='failed' AND json_extract(json,'$.error')='The model service rejected the request; no result was published' AND json_type(json,'$.audit_hash')='text' LIMIT 10000")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (id, h) in readings {
        let Some(audit) = load(&h).and_then(|b| serde_json::from_slice::<crate::processing_ai::ResponseAudit>(&b).ok()) else { continue };
        if audit.http_status == 402 && crate::processing_ai::credit_rejection(402, &audit.raw_response) {
            changed += c.execute("UPDATE processing_attempts SET status='rate_limited',cost_usd=0,json=json_patch(json,?2) WHERE id=?1 AND status='failed'", params![id, string(&json!({"error":"OpenRouter credits are exhausted; queued until credits are added","retry_at":at,"cost_basis":"explicit_402_credit_rejection_without_generation","reclassified_at":at}))])?;
        }
    }
    for table in ["event_link_attempts", "event_embedding_attempts"] {
        let rows = c
            .prepare(&format!("SELECT id,receipt_hash,error FROM {table} WHERE status='failed' AND receipt_hash IS NOT NULL LIMIT 10000"))?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (id, h, old) in rows {
            let Some(receipt) = load(&h).and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
            let Some(body) = receipt["raw_body_hash"].as_str().and_then(&load) else { continue };
            if receipt["status"] != 402 || !crate::processing_ai::credit_rejection(402, &body) {
                continue;
            }
            let retry = if table == "event_link_attempts" { ",retry_at=?3" } else { "" };
            let sql = format!("UPDATE {table} SET status='rate_limited',cost_usd=0,error=?2{retry} WHERE id=?1 AND status='failed'");
            changed += if retry.is_empty() {
                c.execute(&sql, params![id, crate::event_link_store::CREDITS])?
            } else {
                c.execute(&sql, params![id, crate::event_link_store::CREDITS, at])?
            };
            // The lane hold named this rejection; it no longer needs the owner.
            if let Some(old) = old {
                c.execute("DELETE FROM metadata WHERE key='event_links_error' AND value=?1", [old])?;
            }
        }
    }
    Ok(changed)
}
pub(crate) fn predicate(scope: &Scope) -> (String, Vec<SqlValue>) {
    predicate_in(scope, None)
}
/// The scope predicate. `areas` is the bounded-area set of this scope from
/// [`crate::area_view::bounded_areas`], evaluated once per query instead of
/// once per geometry subquery; `None` evaluates it inline.
pub(crate) fn predicate_in(scope: &Scope, areas: Option<&str>) -> (String, Vec<SqlValue>) {
    let located = crate::geographic_view::has_site(scope, false);
    let mut sql = "e.publication_id=?".to_string();
    let mut vals = vec![SqlValue::Text(
        scope.publication_id.clone().unwrap_or_default(),
    )];
    if !scope.source_ids.is_empty() {
        sql.push_str(&format!(
            " AND e.source IN ({})",
            vec!["?"; scope.source_ids.len()].join(",")
        ));
        vals.extend(scope.source_ids.iter().cloned().map(SqlValue::Text));
    }
    if scope.content_view == "signals" {
        sql.push_str(" AND e.kind NOT IN ('promotion','other')");
    }
    // An event report is a message containing a supported occurrence claim,
    // not a deduplicated incident. Old readings stay visible during upgrades.
    // Promotions are advertising even when they mention an occurrence; Signals
    // already hides them.
    //
    // A message the main reading found no event in can still join as a marked
    // possible event suggested by Jev (`jev_hints`, owner 24 October 2026).
    // Possible events skip the promotion rule, because the main reading
    // labelled UKMTO security warnings as promotions, so Events is no longer
    // a strict subset of Signals.
    if scope.content_view == "events" && scope.event_type.is_none() {
        // The legacy branch reads the reading itself, never the per-row merged
        // `json` column, which rebuilds the whole message document per row.
        sql.push_str(&format!(" AND ((e.kind!='promotion' AND (EXISTS(SELECT 1 FROM reading_event_states ev WHERE ev.evidence_rowid=e.rowid AND ev.publication_id=e.publication_id AND ev.state='reported') OR (e.kind='event' AND json_type(e.reading_json,'$.events') IS NULL AND json_extract(e.reading_json,'$.claim_status')='reported'))) OR {})", crate::jev_hints::possible(false)));
    }
    if let Some(kind) = &scope.event_type {
        let reported = if scope.content_view == "events" { " AND ev.state='reported'" } else { "" };
        let promotion = if scope.content_view == "events" { "e.kind!='promotion' AND " } else { "" };
        sql.push_str(&format!(" AND (({promotion}EXISTS(SELECT 1 FROM reading_event_states ev WHERE ev.evidence_rowid=e.rowid AND ev.publication_id=e.publication_id AND ev.event_type=?{reported})) OR {})", crate::jev_hints::possible(true)));
        vals.push(SqlValue::Text(kind.clone()));
        vals.push(SqlValue::Text(kind.clone()));
    }
    if let Some(key) = &scope.related_key {
        sql.push_str(" AND e.related_key=?");
        vals.push(SqlValue::Text(key.clone()));
    }
    // A bounded spatial match already implies a located message. Avoid a
    // second identical membership lookup for every row in the same scope.
    if !scope.include_unlocated && scope.bbox.is_none() {
        sql.push_str(&format!(" AND ({located})"))
    }
    if !scope.query.trim().is_empty() {
        sql.push_str(" AND instr(unicode_lower(e.search_text),?)>0");
        vals.push(SqlValue::Text(scope.query.trim().to_lowercase()));
    }
    let time = if scope.time_basis == "acquisition_time" {
        "e.acquired_at"
    } else {
        "e.published_at"
    };
    if let Some(from) = &scope.from {
        sql.push_str(&format!(" AND {time}>=?"));
        vals.push(SqlValue::Text(from.clone()));
    }
    if let Some(to) = &scope.to {
        sql.push_str(&format!(" AND {time}<=?"));
        vals.push(SqlValue::Text(to.clone()));
    }
    if scope.bbox.is_some() {
        let in_bounds = crate::geographic_view::has_site_in(scope, true, areas);
        sql.push_str(&if scope.include_unlocated {
            format!(" AND (NOT ({located}) OR ({in_bounds}))")
        } else {
            format!(" AND ({in_bounds})")
        });
    }
    (sql, vals)
}
fn scope_members(c: &Connection, scope: &Scope) -> Result<Vec<Value>> {
    scope_members_with_limit(c, scope, MAX_SCOPE_MEMBERS)
}
fn scope_members_with_limit(c: &Connection, scope: &Scope, limit: usize) -> Result<Vec<Value>> {
    let (pred, vals) = predicate(scope);
    let mut stmt=c.prepare(&format!("SELECT e.id,e.revision,e.origin_group FROM reading_evidence e WHERE {pred} ORDER BY e.id LIMIT {}", limit + 1))?;
    let rows=stmt.query_map(rusqlite::params_from_iter(vals),|r|Ok(json!({"evidence_id":r.get::<_,String>(0)?,"revision":r.get::<_,String>(1)?,"origin_group":r.get::<_,Option<String>>(2)?})))?;
    let members: Vec<Value> = rows
        .map(|r| r.map_err(Error::from))
        .collect::<Result<_>>()?;
    if members.len() > limit {
        return Err(Error {
            status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            code: "scope_limit_exceeded",
            message: format!(
                "Exact scope membership exceeds {limit} records. Narrow the source, time or geographic scope before exporting or activating a watch. If an active watch blocks an import, pause it before retrying. No truncated membership is accepted."
            ),
        });
    }
    Ok(members)
}
/// Per-step query timings, recorded only when `RELIC_QUERY_PROFILE` is set
/// (measurement tools such as `examples/profile_query.rs`). Nothing is printed.
static QUERY_PROFILE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var_os("RELIC_QUERY_PROFILE").is_some());
thread_local! {
    static QUERY_STEPS: std::cell::RefCell<Vec<(&'static str, f64)>> = const { std::cell::RefCell::new(Vec::new()) };
}
/// Step timings (milliseconds) of the queries run on this thread since the last call.
pub fn take_query_steps() -> Vec<(&'static str, f64)> {
    QUERY_STEPS.with(|s| std::mem::take(&mut *s.borrow_mut()))
}
struct StepTimer(Option<std::time::Instant>);
impl StepTimer {
    fn start() -> Self {
        Self(QUERY_PROFILE.then(std::time::Instant::now))
    }
    fn step(&mut self, name: &'static str) {
        if let Some(last) = self.0.as_mut() {
            let now = std::time::Instant::now();
            let ms = (now - *last).as_secs_f64() * 1000.0;
            *last = now;
            // Bounded when nothing collects them (a server started with the variable set).
            QUERY_STEPS.with(|s| {
                let mut steps = s.borrow_mut();
                if steps.len() < 4096 {
                    steps.push((name, ms));
                }
            });
        }
    }
}
/// One message of a scope, read once per query. Counts, the timeline, page
/// order and related-reporting counts all derive from this one pass, so every
/// figure shares one predicate evaluation.
struct ScopeRow {
    rowid: i64,
    sort_time: String,
    time: crate::timeline::Stamp,
    origin_group: Option<String>,
    related_key: Option<String>,
    // 1/0, or NULL when the eligible-source list holds a JSON null; SQL sums skip NULL.
    unread: Option<i64>,
    not_selected: Option<i64>,
    media_only: i64,
}
fn scope_rows(c: &Connection, scope: &Scope, pred: &str, vals: &[SqlValue]) -> Result<Vec<ScopeRow>> {
    let sort_time = if scope.time_basis == "acquisition_time" {
        "e.acquired_at"
    } else {
        "coalesce(e.published_at,e.acquired_at)"
    };
    let (field, time) = crate::timeline::stamp_sql(scope);
    // Unread means waiting for reading: text from a source selected for
    // reading. Text from other sources is counted apart; it is not a backlog.
    // CASE conditions short-circuit, so the message document is built only for
    // messages waiting for reading. As a value, `a AND b` evaluates both sides:
    // summing `unread AND selected` merged every reading into its message.
    let unread = "e.kind='pending' AND length(trim(e.text))>0 AND coalesce(json_extract(e.json,'$.upstream_state'),'present')!='removed'";
    let selected = "e.source IN(SELECT value FROM json_each((SELECT json_extract(value,'$.eligible_source_ids') FROM metadata WHERE key='processing_policy')))";
    let media_only = "CASE WHEN e.kind='pending' AND length(trim(e.text))=0 THEN coalesce(json_extract(e.json,'$.upstream_state'),'present')!='removed' ELSE 0 END";
    let mut stmt = c.prepare(&format!("SELECT e.rowid,{sort_time},{time},{field} IS NULL,e.origin_group,e.related_key,CASE WHEN {unread} THEN {selected} ELSE 0 END,CASE WHEN {unread} THEN NOT {selected} ELSE 0 END,{media_only} FROM reading_evidence e WHERE {pred}"))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(vals), |r| {
            Ok(ScopeRow {
                rowid: r.get(0)?,
                sort_time: r.get(1)?,
                time: crate::timeline::Stamp { stamp: r.get(2)?, undated: r.get(3)? },
                origin_group: r.get(4)?,
                related_key: r.get(5)?,
                unread: r.get(6)?,
                not_selected: r.get(7)?,
                media_only: r.get(8)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}
/// Messages of these keys with a site or area within the scope's bounds.
fn located_of(c: &Connection, scope: &Scope, keys: &str, areas: Option<&str>) -> Result<HashSet<i64>> {
    let located = crate::geographic_view::has_site_in(scope, true, areas);
    let mut stmt = c.prepare(&format!("SELECT e.rowid FROM reading_evidence e WHERE e.publication_id=?1 AND e.rowid IN (SELECT value FROM json_each(?2)) AND ({located})"))?;
    let rows = stmt
        .query_map(params![scope.publication_id, keys], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}
fn query_conn(c: &Connection, input: Query, rows_only: bool, area_shapes: bool) -> Result<Value> {
    let mut steps = StepTimer::start();
    // One read snapshot for every step, so the counts, timeline, list and map
    // describe the same database state. Read-only: dropping it rolls back.
    let _snapshot = if c.is_autocommit() {
        Some(c.unchecked_transaction()?)
    } else {
        None
    };
    let mut scope = Scope::parse(&input.scope)?;
    if Store::latest(c)?.is_none() && scope.publication_id.is_none() {
        return Err(Error::missing(
            "No complete publication. Import evidence first.",
        ));
    }
    let pubid = Store::resolve_scope(c, &mut scope)?;
    let scope_revision = hash(string(&value(&scope)).as_bytes());
    let limit = input.limit.unwrap_or(100).clamp(1, 500);
    let areas = crate::area_view::bounded_areas(c, &scope)?;
    let areas = areas.as_deref();
    let (pred, vals) = predicate_in(&scope, areas);
    steps.step("scope");
    let mut rows = scope_rows(c, &scope, &pred, &vals)?;
    steps.step("scan");
    // Newest first by message time (acquisition time on that basis), then by
    // the newer retained row; a cursor continues strictly after its anchor.
    rows.sort_unstable_by(|a, b| (&b.sort_time, b.rowid).cmp(&(&a.sort_time, a.rowid)));
    let mut start = 0;
    if let Some(cursor) = input.cursor {
        let (generation, key) = cursor
            .split_once(':')
            .ok_or_else(|| Error::invalid("Invalid page cursor"))?;
        if generation != scope_revision {
            return Err(Error::conflict(
                "Page cursor belongs to a different scope/publication",
            ));
        }
        let key = key
            .parse::<i64>()
            .map_err(|_| Error::invalid("Invalid page cursor"))?;
        let anchor = rows
            .iter()
            .position(|r| r.rowid == key)
            .ok_or_else(|| Error::invalid("Page cursor is outside its scope"))?;
        start = anchor + 1;
    }
    let page = &rows[start..rows.len().min(start + limit)];
    let more = rows.len() > start + limit;
    let cursor = if more {
        page.last().map(|r| format!("{scope_revision}:{}", r.rowid))
    } else {
        None
    };
    let page_keys = string(&json!(page.iter().map(|r| r.rowid).collect::<Vec<_>>()));
    // Later steps select the same membership by message key: the scope
    // predicate (geometry tests included) is not evaluated again.
    let scope_keys = string(&json!(rows.iter().map(|r| r.rowid).collect::<Vec<_>>()));
    let map = if rows_only {
        None
    } else {
        let (map_areas, areas_truncated, within_areas) =
            crate::area_view::query(c, &scope, &scope_keys, area_shapes, areas)?;
        steps.step("area_query");
        let features = crate::geographic_view::sites_of(c, &scope, &scope_keys, MAX_MAP + 1)?;
        steps.step("map_sites");
        Some((map_areas, areas_truncated, within_areas, features))
    };
    // A message is located when it has a site or area within the scope's
    // bounds: exactly the messages of an untruncated map and its areas.
    let located_set: Option<HashSet<i64>> = match &map {
        _ if !scope.include_unlocated => None,
        Some((_, _, within_areas, features)) if features.len() <= MAX_MAP => Some(
            features
                .iter()
                .filter_map(|f| f["properties"]["message_key"].as_str()?.parse().ok())
                .chain(within_areas.iter().copied())
                .collect(),
        ),
        Some(_) => Some(located_of(c, &scope, &scope_keys, areas)?),
        None => Some(located_of(c, &scope, &page_keys, areas)?),
    };
    let is_located = |rowid: i64| located_set.as_ref().is_none_or(|l| l.contains(&rowid));
    steps.step("located");
    let timeline = if rows_only {
        Value::Null
    } else {
        let stamps: Vec<_> = rows.iter().map(|r| r.time).collect();
        crate::timeline::summarise(&scope, &stamps)?
    };
    let sum = |v: &dyn Fn(&ScopeRow) -> Option<i64>| rows.iter().filter_map(v).sum::<i64>();
    let (total, located, origins, unknown, unread, not_selected, media_only) = if rows_only {
        (0, 0, 0, 0, 0, 0, 0)
    } else {
        (
            rows.len() as i64,
            sum(&|r| Some(is_located(r.rowid) as i64)),
            rows.iter().filter_map(|r| r.origin_group.as_deref()).collect::<HashSet<_>>().len() as i64,
            sum(&|r| Some(r.origin_group.is_none() as i64)),
            sum(&|r| r.unread),
            sum(&|r| r.not_selected),
            sum(&|r| Some(r.media_only)),
        )
    };
    steps.step("counts");
    // Full evidence/reading documents only for the page.
    let mut documents = HashMap::new();
    let mut stmt = c.prepare("SELECT e.rowid,e.json FROM reading_evidence e WHERE e.publication_id=?1 AND e.rowid IN (SELECT value FROM json_each(?2))")?;
    for row in stmt.query_map(params![pubid, page_keys], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (key, document) = row?;
        documents.insert(key, document);
    }
    let mut items: Vec<Value> = page
        .iter()
        .map(|row| {
            let document = documents.remove(&row.rowid).ok_or_else(Error::internal)?;
            let mut evidence: Value = serde_json::from_str(&document)?;
            evidence["message_key"] = json!(row.rowid.to_string());
            if !is_located(row.rowid) {
                evidence["geometry"] = Value::Null;
            }
            Ok(evidence)
        })
        .collect::<Result<_>>()?;
    steps.step("page");
    let related_keys: HashSet<&str> = items
        .iter()
        .filter_map(|e| e["interpretation"]["related_key"].as_str())
        .collect();
    let mut group_counts: HashMap<&str, i64> = HashMap::new();
    for key in rows.iter().filter_map(|r| r.related_key.as_deref()) {
        if related_keys.contains(key) {
            *group_counts.entry(key).or_default() += 1;
        }
    }
    for e in &mut items {
        if let Some(key) = e["interpretation"]["related_key"].as_str() {
            let count = group_counts.get(key).copied().unwrap_or(1) - 1;
            e["interpretation"]["related_count"] = json!(count);
        }
    }
    steps.step("related");
    let scoped = "e.publication_id=? AND e.rowid IN (SELECT value FROM json_each(?))";
    let scoped_vals = [SqlValue::Text(pubid.clone()), SqlValue::Text(scope_keys)];
    let event_groups = crate::event_link_store::scope_groups(c, &pubid, scoped, &scoped_vals)?;
    steps.step("event_groups");
    // Page locations are independent of the map draw cap. An untruncated map
    // already holds every site of the page, in the same order.
    let page_sites = match &map {
        Some((_, _, _, features)) if features.len() <= MAX_MAP => {
            let on_page: HashSet<String> = page.iter().map(|r| r.rowid.to_string()).collect();
            features
                .iter()
                .filter(|f| f["properties"]["message_key"].as_str().is_some_and(|k| on_page.contains(k)))
                .take(limit * 25)
                .cloned()
                .collect()
        }
        _ => crate::geographic_view::sites_of(c, &scope, &page_keys, limit * 25)?,
    };
    for item in &mut items {
        crate::geographic_view::attach_sites(item, &page_sites);
    }
    steps.step("page_sites");
    crate::area_view::attach_in(c, &scope, &mut items, areas)?;
    steps.step("area_attach");
    crate::event_link_store::attach_event_links(&event_groups, &mut items);
    crate::jev_hints::attach(c, &mut items, scope.event_type.as_deref())?;
    let Some((map_areas, areas_truncated, _, mut features)) = map else {
        return Ok(
            json!({"scope_revision":scope_revision,"scope":scope,"publication_id":pubid,"generation":input.generation,"loaded":items.len(),"items":items,"next_cursor":cursor}),
        );
    };
    // Map points carry their event group, so one event is one marker per site.
    for f in &mut features {
        if let Some((id, reports)) = f["properties"]["message_key"]
            .as_str()
            .and_then(|k| k.parse::<i64>().ok())
            .and_then(|k| event_groups.get(&k))
        {
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
        cov.push(format!(
            "Map shows the first {MAX_MAP} sites; message list/count membership remains complete."
        ));
    }
    Ok(
        json!({"scope_revision":scope_revision,"scope":scope,"publication_id":pubid,"generation":input.generation,"total":total,"located":located,"unlocated":total-located,"unread":unread,"not_selected":not_selected,"media_only":media_only,"origin_groups":origins,"unknown_origins":unknown,"loaded":items.len(),"items":items,"map_loaded":features.len(),"map_truncated":map_truncated,"map_areas":map_areas,"areas_truncated":areas_truncated,"map":{"type":"FeatureCollection","features":features},"next_cursor":cursor,"coverage":cov,"timeline":timeline}),
    )
}

pub fn markdown_export(manifest: &Value) -> String {
    let b = &manifest["brief"];
    let mut out = format!(
        "# {}\n\nQuestion: {}\n\n{}\n\nAssessment: {}\n\nAuthorship: {} · saved {}\nPublication: {}\nKnowledge cutoff: {}\n\n",
        b["title"].as_str().unwrap_or("Relic brief"),
        b["question"].as_str().unwrap_or(""),
        b["finding"].as_str().unwrap_or(""),
        b["assessment"].as_str().unwrap_or(""),
        b["authorship"].as_str().unwrap_or(""),
        b["created_at"].as_str().unwrap_or(""),
        b["publication_id"].as_str().unwrap_or(""),
        b["knowledge_cutoff"].as_str().unwrap_or("")
    );
    if b["unsupported"] == true {
        out.push_str("**Unsupported analyst finding: no exact evidence support supplied.**\n\n")
    }
    out.push_str("## Limitations\n\n");
    for l in b["limitations"].as_array().unwrap_or(&vec![]) {
        out.push_str(&format!("- {}\n", l.as_str().unwrap_or("")))
    }
    out.push_str("\n## Exact retained support\n\n");
    for (i, s) in manifest["support"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .enumerate()
    {
        let c = &s["citation"];
        out.push_str(&format!("{}. {} @ {} · characters {}–{} · {}\n\n> {}\n\nSource: {} · published {} · acquired {}\nCanonical record JSON SHA-256: {}\n\n",i+1,c["evidence_id"].as_str().unwrap_or(""),c["revision"].as_str().unwrap_or(""),c["start_character"],c["end_character"],s["evidence"]["source_label"].as_str().unwrap_or(""),s["quote"].as_str().unwrap_or("").replace('\n',"\n> "),s["evidence"]["source"].as_str().unwrap_or(""),s["evidence"]["published_at"].as_str().unwrap_or("unknown"),s["evidence"]["acquired_at"].as_str().unwrap_or("unknown"),s["evidence"]["object_hash"].as_str().unwrap_or("")))
    }
    out.push_str("## Original input files\n\n");
    let mut hashes = std::collections::BTreeSet::new();
    for support in manifest["support"].as_array().unwrap_or(&vec![]) {
        for h in support["original_input_hashes"]
            .as_array()
            .unwrap_or(&vec![])
        {
            if let Some(h) = h.as_str() {
                hashes.insert(h.to_string());
            }
        }
    }
    for h in hashes {
        out.push_str(&format!("- Original input JSON SHA-256: {h}\n"));
    }
    out.push('\n');
    out.push_str(&format!(
        "## Frozen scope\n\n```json\n{}\n```\n\n{}\n",
        serde_json::to_string_pretty(&b["scope"]).unwrap(),
        manifest["reproducibility"].as_str().unwrap_or("")
    ));
    out
}

#[cfg(test)]
mod query_equivalence_tests;

#[cfg(test)]
mod membership_bounds_tests {
    use super::*;

    #[test]
    fn normal_ten_thousand_record_scope_remains_complete() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE evidence(rowid INTEGER PRIMARY KEY,id TEXT,revision TEXT,source TEXT,origin_group TEXT);
             CREATE TABLE membership(publication_id TEXT,evidence_rowid INTEGER);
             CREATE VIEW reading_evidence AS SELECT e.*,m.publication_id FROM membership m JOIN evidence e ON e.rowid=m.evidence_rowid;
             WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n<10000)
             INSERT INTO evidence SELECT n,'test:'||n,'v1','source:normal',NULL FROM ids;
             INSERT INTO membership SELECT 'publication:normal',rowid FROM evidence;",
        ).unwrap();
        let scope = Scope {
            publication_id: Some("publication:normal".into()),
            ..Scope::default()
        };
        let members = scope_members(&c, &scope).unwrap();
        assert_eq!(members.len(), 10_000);
        assert!(members.iter().any(|e| e["evidence_id"] == "test:10000"));
    }

    #[test]
    fn exact_membership_accepts_the_limit_and_rejects_overflow_without_truncation() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE evidence(rowid INTEGER PRIMARY KEY,id TEXT,revision TEXT,source TEXT,origin_group TEXT);
             CREATE TABLE membership(publication_id TEXT,evidence_rowid INTEGER);
             CREATE VIEW reading_evidence AS SELECT e.*,m.publication_id FROM membership m JOIN evidence e ON e.rowid=m.evidence_rowid;
             INSERT INTO evidence VALUES(1,'test:a','v1','source:included',NULL),(2,'test:b','v1','source:included',NULL),(3,'test:c','v1','source:other',NULL);
             INSERT INTO membership VALUES('publication:test',1),('publication:test',2),('publication:test',3);",
        ).unwrap();
        let mut scope = Scope {
            publication_id: Some("publication:test".into()),
            ..Scope::default()
        };
        let error = scope_members_with_limit(&c, &scope, 2).unwrap_err();
        assert_eq!(error.code, "scope_limit_exceeded");
        assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert!(error.message.contains("Narrow"));
        // Source predicates run before the limit: an exact bounded subset remains usable.
        scope.source_ids = vec!["source:included".into()];
        let complete = scope_members_with_limit(&c, &scope, 2).unwrap();
        assert_eq!(complete.len(), 2);
        assert_eq!(complete[1]["evidence_id"], "test:b");
        // Rejection does not modify frozen publication membership.
        scope.source_ids.clear();
        assert_eq!(scope_members_with_limit(&c, &scope, 3).unwrap().len(), 3);
        assert_eq!(scope_members(&c, &scope).unwrap().len(), 3);
    }
}
