//! Account connections and bounded acquisition. Source credentials never leave this module.
mod initial_history;
mod media;
mod scheduler;
mod telegram;
mod x;

use crate::{
    error::{Error, Result},
    model::{now, string},
    store::Store,
};
use chrono::{Duration as ChronoDuration, Utc};
use crate::sources::PostTimeline;
use relic_telegram::TelegramClient;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Notify, Semaphore};

const ATTEMPT_PAGES: usize = 3;
const MAX_BATCH_BYTES: usize = 2 * 1024 * 1024;
const MAX_MEDIA_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Serialize)]
pub struct Connection {
    pub provider: &'static str,
    pub status: String,
    pub detail: String,
    pub account_label: Option<String>,
    pub retry_at: Option<String>,
}

pub struct Runtime {
    pub store: Store,
    x: Option<Arc<dyn PostTimeline>>,
    x_account: String,
    x_rotate: bool,
    telegram: Mutex<Option<TelegramClient>>,
    statuses: RwLock<Vec<Connection>>,
    x_epoch: Arc<AtomicU64>,
    telegram_epoch: Arc<AtomicU64>,
    pub wake: Notify,
    active: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Attempts an operator stopped. Their Stop holds even when its own save
    /// lost a race and a shutdown interrupts the attempt before it finishes.
    stopped_runs: std::sync::Mutex<std::collections::HashSet<String>>,
    operations: [Semaphore; 2],
    media: Semaphore,
    storage: Arc<Semaphore>,
    in_flight: AtomicUsize,
    batch_bytes: Arc<AtomicUsize>,
    peak_batch_bytes: AtomicUsize,
    saved_pages: Arc<AtomicU64>,
    started: Instant,
}

fn index(provider: &str) -> Result<usize> {
    match provider {
        "x" => Ok(0),
        "telegram" => Ok(1),
        _ => Err(Error::invalid("Unknown connection provider")),
    }
}
/// An enabled, unstopped source with no successful collection for 10x its poll
/// interval (at least 30 minutes), whatever the cause: a down proxy, a lost
/// session or a stuck schedule. Explicitly stopped sources are not stalled.
pub(crate) fn collection_stalled(state: &Value, interval: i64) -> bool {
    if state["stopped"] == true {
        return false;
    }
    state["last_success_at"]
        .as_str()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .is_some_and(|at| {
            (Utc::now() - at.with_timezone(&Utc)).num_seconds()
                > interval.saturating_mul(10).max(1800)
        })
}
pub(crate) fn catchup_delayed(state: &Value, interval: i64) -> bool {
    if !state["continuation"].is_object() || state["stopped"] == true {
        return false;
    }
    let since = state["last_success_at"]
        .as_str()
        .or_else(|| state["initial_history"]["started_at"].as_str());
    since
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .is_some_and(|at| {
            (Utc::now() - at.with_timezone(&Utc)).num_seconds()
                > interval.saturating_mul(5).max(1200)
        })
}
fn later(seconds: i64) -> String {
    (Utc::now() + ChronoDuration::seconds(seconds))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

// Only a progressing interval may resume unattended. Explicit false survives
// failures/restarts; older three-page pauses are migrated narrowly by contract.
fn repeated_cursor_retry(state: &Value) -> bool {
    state["provider"] == "x"
        && state["continuation"].is_object()
        && state["cursor_restart_attempts"].as_u64().unwrap_or(0) < 3
        && (state["cursor_restart_pending"] == true
            || state["detail"].as_str()
                == Some(
                    "X repeated a cursor. Acquired pages are retained; the original watermark has not advanced.",
                ))
}
fn auto_continue(state: &Value) -> bool {
    if repeated_cursor_retry(state) {
        return true;
    }
    if state["catchup_auto_resume"] == false {
        return false;
    }
    if initial_history::active(state) {
        return initial_history::auto_continue(state);
    }
    let retryable = state["last_error_code"].is_null()
        || matches!(
            state["last_error_code"].as_str(),
            Some("x_unavailable" | "telegram_unavailable" | "source_transient")
        );
    if !retryable {
        return false;
    }
    if state["catchup_auto_resume"] == true {
        return true;
    }
    // Pre-upgrade records have no flag. Never revive a repeated-cursor pause,
    // an unknown continuation shape, or a source/identity failure.
    let legacy_detail = state["detail"].as_str().unwrap_or_default();
    match state["provider"].as_str() {
        Some("x")
            if legacy_detail
                == "Catch-up is incomplete. Acquired pages are retained; continue the saved interval." =>
        {
            let continuation = &state["continuation"];
            continuation["cursor"].as_str().is_some_and(|cursor| {
                !cursor.is_empty()
                    && continuation["seen_cursors"].as_array().is_some_and(|seen| {
                        seen.len() < 1024 && !seen.iter().any(|old| old.as_str() == Some(cursor))
                    })
            })
        }
        Some("telegram")
            if legacy_detail
                == "Telegram catch-up is incomplete. Continue the saved interval; the original watermark is unchanged." =>
        {
            let continuation = &state["continuation"];
            continuation["before"]
                .as_i64()
                .zip(
                    continuation["original_watermark"]
                        .as_str()
                        .and_then(|v| v.parse::<i64>().ok()),
                )
                .is_some_and(|(before, after)| before > after && after >= 0)
        }
        _ => false,
    }
}

impl Runtime {
    /// A runtime with Telegram only; see [`Runtime::with_sources`].
    pub fn new(store: Store) -> Arc<Self> {
        Self::with_sources(store, &crate::sources::Sources::default())
    }
    /// A runtime with the timeline adapter this build registered, if any.
    pub fn with_sources(store: Store, sources: &crate::sources::Sources) -> Arc<Self> {
        let values = &store.config.source_values;
        let x = sources.timeline.clone();
        let x_account = x.as_ref().map(|x| x.account_key()).unwrap_or_default();
        let x_rotate = x.as_ref().is_some_and(|x| x.rotates());
        let x_setup = sources.timeline_setup.clone().unwrap_or_else(|| crate::sources::TIMELINE_NOT_INCLUDED.into());
        let tg_ready = values
            .get("TELEGRAM_API_ID")
            .and_then(|s| s.parse::<i32>().ok())
            .is_some_and(|id| id > 0)
            && values
                .get("TELEGRAM_API_HASH")
                .is_some_and(|s| !s.trim().is_empty());
        let statuses = vec![
            Connection { provider:"x", status:if x.is_some(){"disconnected"}else{"unconfigured"}.into(), detail:if x.is_some(){"Ready to connect through the configured proxy.".into()}else{x_setup}, account_label:None,retry_at:None },
            Connection { provider:"telegram", status:if tg_ready{"disconnected"}else{"unconfigured"}.into(), detail:if tg_ready{"Ready for a fresh Telegram login."}else{"Load TELEGRAM_API_ID and TELEGRAM_API_HASH in the private server configuration."}.into(),account_label:None,retry_at:None },
        ];
        Arc::new(Self {
            store,
            x,
            x_account,
            x_rotate,
            telegram: Mutex::new(None),
            statuses: RwLock::new(statuses),
            x_epoch: Arc::new(AtomicU64::new(0)),
            telegram_epoch: Arc::new(AtomicU64::new(0)),
            wake: Notify::new(),
            active: Mutex::new(HashMap::new()),
            stopped_runs: Default::default(),
            operations: [Semaphore::new(1), Semaphore::new(1)],
            media: Semaphore::new(1),
            storage: Arc::new(Semaphore::new(4)),
            in_flight: AtomicUsize::new(0),
            batch_bytes: Arc::new(AtomicUsize::new(0)),
            peak_batch_bytes: AtomicUsize::new(0),
            saved_pages: Arc::new(AtomicU64::new(0)),
            started: Instant::now(),
        })
    }

    pub fn connection(&self, provider: &str) -> Result<Connection> {
        let idx = index(provider)?;
        Ok(self.statuses.read().map_err(|_| Error::internal())?[idx].clone())
    }
    fn connected(&self, provider: &str) -> bool {
        self.connection(provider)
            .is_ok_and(|c| c.status == "connected")
    }
    fn set_connection(
        &self,
        provider: &str,
        status: &str,
        detail: impl Into<String>,
        account: Option<String>,
        retry: Option<String>,
    ) -> Result<Connection> {
        let mut statuses = self.statuses.write().map_err(|_| Error::internal())?;
        let c = &mut statuses[index(provider)?];
        c.status = status.into();
        c.detail = detail.into();
        c.account_label = account;
        c.retry_at = retry;
        let result = c.clone();
        drop(statuses);
        // In-memory status does not imply persisted account authorization.
        let _ = self.store.events.send(json!({"kind":"resync"}));
        self.wake.notify_one();
        Ok(result)
    }
    pub async fn status(&self) -> Result<Value> {
        let allowed = match self.block(|s| s.assert_real_collection_allowed()).await {
            Ok(()) => true,
            Err(e) if e.code == "conflict" => false,
            Err(e) => return Err(e),
        };
        let providers = self.statuses.read().map_err(|_| Error::internal())?.clone();
        Ok(
            json!({"providers":providers,"collection_allowed":allowed,"workspace_detail":if allowed{"Incoming messages will be retained in this workspace."}else{"This workspace contains fictional evidence. Start a separate live workspace before connecting accounts."}}),
        )
    }
    pub fn metrics(&self) -> Value {
        json!({"in_flight":self.in_flight.load(Ordering::Relaxed),"active_batch_bytes":self.batch_bytes.load(Ordering::Relaxed),"peak_batch_bytes":self.peak_batch_bytes.load(Ordering::Relaxed),"durable_pages":self.saved_pages.load(Ordering::Relaxed),"uptime_seconds":self.started.elapsed().as_secs(),"limits":{"providers_in_flight":2,"pages_per_attempt":ATTEMPT_PAGES,"batch_bytes":MAX_BATCH_BYTES,"queued_network_tasks":0,"x_media_bytes":MAX_MEDIA_BYTES,"telegram_media_bytes":crate::media_store::MAX_MEDIA_FILE_BYTES,"telegram_media_chunk_bytes":524288,"media_serve_chunk_bytes":65536,"media_concurrency":1},"queue_policy":"Due work stays in SQLite; one attempt per provider; persisted pages backpressure further network reads."})
    }
    async fn block<T: Send + 'static>(
        &self,
        f: impl FnOnce(Store) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .storage
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::busy())?;
        let s = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            f(s)
        })
        .await
        .map_err(|_| Error::internal())?
    }
    async fn activation(&self, provider: &str, active: bool) -> Result<()> {
        let key = format!("collection:{provider}:active");
        self.block(move|s|{let _guard=s.writer.lock().map_err(|_|Error::internal())?;let mut c=s.connect()?;let tx=c.transaction()?;tx.execute("INSERT INTO metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,if active{"true"}else{"false"}])?;let event=Store::event(&tx,"connection",None,vec![])?;tx.commit()?;let _=s.events.send(event);Ok(())}).await
    }
    async fn activation_if_epoch(&self, provider: &str, expected: u64) -> Result<()> {
        let key = format!("collection:{provider}:active");
        let epoch = if provider == "x" {
            self.x_epoch.clone()
        } else {
            self.telegram_epoch.clone()
        };
        self.block(move|s|{
            let _guard=s.writer.lock().map_err(|_|Error::internal())?;
            if epoch.load(Ordering::Acquire)!=expected{return Err(Error::cancelled());}
            let mut c=s.connect()?;let tx=c.transaction()?;
            tx.execute("INSERT INTO metadata(key,value) VALUES(?1,'true') ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key])?;
            let event=Store::event(&tx,"connection",None,vec![])?;tx.commit()?;let _=s.events.send(event);Ok(())
        }).await
    }
    pub async fn connect(self: &Arc<Self>, provider: &str) -> Result<Connection> {
        self.block(|s| s.assert_real_collection_allowed()).await?;
        let idx = index(provider)?;
        let _permit = self.operations[idx]
            .try_acquire()
            .map_err(|_| Error::busy())?;
        if !self.retry_gate(provider).await? {
            return Err(self.retry_error(provider)?);
        }
        let existing = self.connection(provider)?;
        if existing.status == "unconfigured" {
            return Err(Error::invalid(existing.detail));
        }
        if existing.status == "connected" {
            return Ok(existing);
        }
        // Serialize mode selection with imports before any account request.
        self.block(|s|{
            let _guard=s.writer.lock().map_err(|_|Error::internal())?;
            s.assert_real_collection_allowed()?;
            s.connect()?.execute("INSERT INTO metadata(key,value) VALUES('live_workspace','true') ON CONFLICT(key) DO UPDATE SET value='true'",[])?;
            Ok(())
        }).await?;
        let epoch = self.epoch(provider).fetch_add(1, Ordering::AcqRel) + 1;
        self.set_connection(
            provider,
            "connecting",
            "Checking account access…",
            None,
            None,
        )?;
        let outcome = {
            let request = async {
                if provider == "x" {
                    self.connect_x().await
                } else {
                    self.connect_telegram().await
                }
            };
            tokio::pin!(request);
            loop {
                tokio::select! {
                    result=&mut request=>break result,
                    _=tokio::time::sleep(Duration::from_millis(50))=>{
                        if self.epoch(provider).load(Ordering::Acquire)!=epoch||self.store.shutdown.load(Ordering::Acquire){break Err(Error::cancelled());}
                    }
                }
            }
        };
        if self.epoch(provider).load(Ordering::Acquire) != epoch {
            if provider == "telegram"
                && let Some(client) = self.telegram.lock().await.as_mut()
            {
                client.disconnect();
            }
            self.set_connection(
                provider,
                "disconnected",
                "Disconnected. Retained messages remain available.",
                None,
                self.connection(provider)?.retry_at,
            )?;
            return Err(Error::cancelled());
        }
        match outcome {
            Ok(connection) => {
                if connection.status == "connected" {
                    self.activation_if_epoch(provider, epoch).await?;
                }
                self.wake.notify_one();
                Ok(connection)
            }
            Err(e) => {
                self.set_connection(
                    provider,
                    "error",
                    e.message.clone(),
                    None,
                    self.connection(provider)?.retry_at,
                )?;
                Err(e)
            }
        }
    }
    fn epoch(&self, provider: &str) -> &AtomicU64 {
        if provider == "x" {
            &self.x_epoch
        } else {
            &self.telegram_epoch
        }
    }
    fn retry_error(&self, provider: &str) -> Result<Error> {
        let connection = self.connection(provider)?;
        let time = connection
            .retry_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|time| time.with_timezone(&Utc).format("%H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "the displayed retry time".into());
        Ok(Error::conflict(format!(
            "Connection retry is available after {time}."
        )))
    }
    pub async fn disconnect(&self, provider: &str) -> Result<Connection> {
        index(provider)?;
        self.epoch(provider).fetch_add(1, Ordering::AcqRel);
        self.activation(provider, false).await?;
        let ids = self
            .configurations()
            .await?
            .into_iter()
            .filter(|s| s["provider"] == provider)
            .map(|s| text(&s, "id"))
            .collect::<Vec<_>>();
        let running = self.active.lock().await;
        for id in ids {
            if let Some(cancel) = running.get(&id) {
                cancel.store(true, Ordering::Release);
            }
        }
        drop(running);
        self.set_connection(
            provider,
            "disconnected",
            "Disconnected. Retained messages remain available.",
            None,
            self.connection(provider)?.retry_at,
        )?;
        if provider == "telegram"
            && let Some(client) = self.telegram.lock().await.as_mut()
        {
            client.disconnect();
        }
        self.connection(provider)
    }

    pub async fn stop_job(&self, id: &str) -> Result<()> {
        let run = id.to_owned();
        let states = self.block(|s| s.collection_states()).await?;
        if let Some(mut state) = states.into_iter().find(|state| state["run_id"] == run) {
            let sid = text(&state, "subscription_id");
            let version = state["configuration_version"]
                .as_i64()
                .ok_or_else(Error::internal)?;
            // Cancellation is signalled before waiting for a writer; a publication
            // already committing may finish, but no later attempt auto-restarts.
            // The Stop is remembered first: if the save below loses a race with
            // a page, the attempt's finish (or a shutdown) still records it.
            if let Some(cancel) = self.active.lock().await.get(&sid) {
                self.stopped_runs
                    .lock()
                    .map_err(|_| Error::internal())?
                    .insert(run.clone());
                cancel.store(true, Ordering::Release);
            }
            state["stopped"] = json!(true);
            state["status"] = json!("paused");
            state["requested_action"] = Value::Null;
            state["detail"] = json!("Stopped. Check now or continue when ready.");
            let result = self
                .block(move |s| s.save_collection_state(&sid, version, state))
                .await;
            if let Err(e) = result
                && e.code != "conflict"
            {
                return Err(e);
            } // A finishing page is followed by worker cancellation state.
        }
        Ok(())
    }
    pub async fn source_changed(&self, id: &str) {
        if let Some(cancel) = self.active.lock().await.get(id) {
            cancel.store(true, Ordering::Release);
        }
        self.wake.notify_one();
    }
    async fn configurations(&self) -> Result<Vec<Value>> {
        self.block(|s|{let c=s.connect()?;let mut stmt=c.prepare("SELECT id,version,provider,label,target,mode,poll_interval_seconds,enabled FROM subscriptions ORDER BY rowid LIMIT 1000")?;let rows=stmt.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"version":r.get::<_,i64>(1)?,"provider":r.get::<_,String>(2)?,"label":r.get::<_,String>(3)?,"target":r.get::<_,String>(4)?,"mode":r.get::<_,String>(5)?,"poll_interval_seconds":r.get::<_,Option<i64>>(6)?,"enabled":r.get::<_,bool>(7)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;Ok(rows)}).await
    }
    pub async fn decorate(&self, mut value: Value) -> Result<Value> {
        if let Some(items) = value.get_mut("items").and_then(Value::as_array_mut) {
            for item in items {
                self.decorate_one(item).await?;
            }
        } else {
            self.decorate_one(&mut value).await?;
        }
        Ok(value)
    }
    async fn decorate_one(&self, sub: &mut Value) -> Result<()> {
        let id = text(sub, "id");
        let st = self.block(move |s| s.collection_state(&id)).await?;
        let provider = text(sub, "provider");
        let connected = self.connected(&provider);
        let valid = st["configuration_version"] == sub["version"];
        let once = valid && st["requested_action"].is_string();
        let running = self.active.lock().await.contains_key(&text(sub, "id"));
        let delayed =
            valid && catchup_delayed(&st, sub["poll_interval_seconds"].as_i64().unwrap_or(120));
        let status = if !connected {
            if sub["enabled"] == true {
                "not_connected"
            } else {
                "paused"
            }
        } else if running {
            "running"
        } else if (valid && st["stopped"] == true) || (sub["enabled"] != true && !once) {
            "paused"
        } else if valid {
            st["status"].as_str().unwrap_or("waiting")
        } else {
            "waiting"
        };
        sub["status"] = json!(status);
        sub["status_detail"] = json!(if !connected {
            self.connection(&provider)?.detail
        } else if valid {
            st["detail"]
                .as_str()
                .unwrap_or("Waiting for the next collection attempt.")
                .into()
        } else {
            "Saved policy is waiting for collection.".into()
        });
        sub["collection"] = json!({"last_success_at":st["last_success_at"],"next_due_at":st["next_due_at"],"messages_acquired":st["messages_acquired"].as_u64().unwrap_or(0),"can_continue":connected&&valid&&st["continuation"].is_object(),"can_rebaseline":provider=="x"&&connected&&valid&&!running&&st["continuation"].is_object(),"gap_recorded":st["last_gap_id"].is_string(),"delayed":delayed,"stalled":valid&&sub["enabled"]==true&&collection_stalled(&st,sub["poll_interval_seconds"].as_i64().unwrap_or(120))});
        Ok(())
    }
    pub async fn request(&self, id: String, expected: i64, action: &str) -> Result<Value> {
        if !["poll", "continue", "baseline"].contains(&action) {
            return Err(Error::invalid("Unknown collection action"));
        }
        self.block(|s| s.assert_real_collection_allowed()).await?;
        let read_id = id.clone();
        let sub = self.block(move |s| s.subscription(&read_id)).await?;
        if sub["version"] != expected {
            return Err(Error::conflict(
                "Collection settings changed; reload before collecting",
            ));
        }
        if !self.connected(&text(&sub, "provider")) {
            return Err(Error::conflict("Connect this source's account first"));
        }
        if self.active.lock().await.contains_key(&id) {
            return Err(Error::busy());
        }
        let mut state = self.prepare_state(&sub).await?;
        if action == "continue" && !state["continuation"].is_object() {
            return Err(Error::invalid(
                "There is no saved catch-up interval to continue",
            ));
        }
        if action == "poll" && state["continuation"].is_object() {
            return Err(Error::conflict(
                "Continue the saved catch-up interval before starting another poll",
            ));
        }
        if action == "baseline" {
            self.block(move |s| s.rebaseline_collection(&id, expected, state))
                .await?;
            self.wake.notify_one();
            return self.decorate(sub).await;
        }
        if action == "continue" && state["checkpoint_missing"] == true {
            state["cursor_restart_pending"] = json!(true);
        }
        state["stopped"] = json!(false);
        state["last_error_code"] = Value::Null;
        state["requested_action"] = json!(action);
        state["status"] = json!("waiting");
        state["detail"] = json!("Bounded collection requested.");
        state["next_due_at"] = json!(now());
        self.block(move |s| s.save_collection_state(&id, expected, state))
            .await?;
        self.wake.notify_one();
        self.decorate(sub).await
    }
    async fn prepare_state(&self, sub: &Value) -> Result<Value> {
        let id = text(sub, "id");
        let read_id = id.clone();
        let mut state = self.block(move |s| s.collection_state(&read_id)).await?;
        if state["configuration_version"] != sub["version"] || state["generation"] == 0 {
            // A changed target or account never inherits another source's cursor.
            if state["bound_target"] != sub["target"] || state["provider"] != sub["provider"] {
                let generation = state["generation"].clone();
                state = json!({"generation":generation,"watermark":null,"continuation":null,"last_success_at":null,"messages_acquired":null});
            }
            state["stopped"] = json!(false);
            state["last_error_code"] = Value::Null;
            state["bound_target"] = sub["target"].clone();
            state["provider"] = sub["provider"].clone();
            state["status"] = json!("waiting");
            state["detail"] = json!("Waiting for the first bounded collection.");
            state["next_due_at"] = json!(now());
            let version = sub["version"].as_i64().ok_or_else(Error::internal)?;
            state = self
                .block(move |s| s.save_collection_state(&id, version, state))
                .await?;
        }
        Ok(state)
    }
    async fn save_state(&self, sub: &Value, state: Value) -> Result<Value> {
        let id = text(sub, "id");
        let version = sub["version"].as_i64().ok_or_else(Error::internal)?;
        self.block(move |s| s.save_collection_state(&id, version, state))
            .await
    }
    async fn publish(
        &self,
        sub: &Value,
        source: Value,
        messages: Vec<Value>,
        raw: Value,
        state: Value,
        cancel: Arc<AtomicBool>,
    ) -> Result<Value> {
        let bytes = string(&raw).len() + messages.iter().map(|m| string(m).len()).sum::<usize>();
        if bytes > MAX_BATCH_BYTES {
            return Err(Error::invalid(
                "Source page exceeds the 2 MiB publication bound; retained checkpoint has not advanced",
            ));
        }
        let _permit = self
            .store
            .imports
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::busy())?;
        struct Batch {
            counter: Arc<AtomicUsize>,
            bytes: usize,
        }
        impl Drop for Batch {
            fn drop(&mut self) {
                self.counter.fetch_sub(self.bytes, Ordering::Relaxed);
            }
        }
        let current = self.batch_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.peak_batch_bytes.fetch_max(current, Ordering::Relaxed);
        let batch = Batch {
            counter: self.batch_bytes.clone(),
            bytes,
        };
        let saved = self.saved_pages.clone();
        let id = text(sub, "id");
        let version = sub["version"].as_i64().ok_or_else(Error::internal)?;
        let result = self
            .block(move |s| {
                // These guards live with the blocking write even if its awaiting
                // network task is cancelled. No second publication bypasses it.
                let (_batch, _writer_permit) = (batch, _permit);
                let result =
                    s.publish_collection_page(&id, version, source, messages, raw, state, cancel);
                if result.is_ok() {
                    saved.fetch_add(1, Ordering::Relaxed);
                }
                result
            })
            .await;
        result.map(|v| v["state"].clone())
    }
    async fn cancellable<T>(
        &self,
        cancel: &AtomicBool,
        future: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        tokio::pin!(future);
        loop {
            tokio::select! {r=&mut future=>return r,_=tokio::time::sleep(Duration::from_millis(50))=>{if cancel.load(Ordering::Acquire)||self.store.shutdown.load(Ordering::Acquire){return Err(Error::cancelled());}}}
        }
    }
}

#[cfg(test)]
mod cursor_recovery_tests {
    use super::*;
    #[test]
    fn only_repeated_cursor_state_gets_three_bounded_restarts() {
        let mut state = json!({"provider":"x","continuation":{"cursor":"same","original_watermark":"42"},"detail":"X repeated a cursor. Acquired pages are retained; the original watermark has not advanced.","catchup_auto_resume":false});
        assert!(auto_continue(&state));
        state["cursor_restart_attempts"] = json!(3);
        assert!(!auto_continue(&state));
        assert_eq!(state["continuation"]["original_watermark"], "42");
        state["cursor_restart_attempts"] = json!(0);
        state["detail"] = json!("An unknown source failure");
        assert!(!auto_continue(&state));
        state["cursor_restart_pending"] = json!(true);
        assert!(auto_continue(&state));
        state["provider"] = json!("telegram");
        assert!(!auto_continue(&state));
    }
}
