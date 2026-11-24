//! The ship finder over many messages: the same single paid extraction per
//! message revision as the reader's button, then free KeelWatch matching.
//! Runs over the whole backlog when the owner asks, or, in automatic mode,
//! over flagged messages acquired since automatic mode was switched on.
//! Stop here or in Processing, and Pause in Processing, take effect between
//! requests; requests already sent finish and are recorded. A run that stops
//! early (failures, exhausted credits, Stop or Pause) switches automatic mode
//! off with its reason until the analyst switches it on again. No request or
//! spending cap. Without a passed evaluation of the current profile, a run
//! does not start, automatic mode cannot be switched on, and automatic mode
//! already on switches itself off with that reason.
use crate::{
    error::{Error, Result},
    live_layers::LiveLayers,
    model::now,
    store::Store,
    vessel_finder::{self, Extraction, Sender, Waiting},
};
use futures_util::StreamExt;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// Extractions in flight at once.
const CONCURRENCY: usize = 8;
/// Consecutive failed extractions that stop a run (a provider outage).
const FAILURE_STOP: u32 = 5;
/// Automatic mode looks for new messages this often.
const AUTO_EVERY: std::time::Duration = std::time::Duration::from_secs(120);
pub(crate) const AUTO_KEY: &str = "vessel_finder:auto";
/// When automatic mode was switched on: it takes messages acquired since.
pub(crate) const AUTO_SINCE: &str = "vessel_finder:auto_since";
/// Why automatic mode switched itself off.
pub(crate) const AUTO_PAUSED: &str = "vessel_finder:auto_paused";

#[derive(Default)]
struct Run {
    running: bool,
    started_at: Option<String>,
    finished_at: Option<String>,
    planned: usize,
    done: usize,
    found: usize,
    failed: usize,
    cost_usd: f64,
    unknown_cost: usize,
    stop_reason: Option<String>,
    automatic: bool,
}
/// How a run ended.
enum End {
    Complete,
    /// Stopped early for this reason; automatic mode switches off.
    Stopped(String),
    /// Relic is stopping; automatic mode stays as it was.
    Shutdown,
}
/// Automatic mode as stored.
#[derive(Clone, Debug, Default, PartialEq)]
struct Auto {
    on: bool,
    since: Option<String>,
    paused: Option<String>,
}
fn read_auto(c: &Connection) -> Result<Auto> {
    let get = |key: &str| c.query_row("SELECT value FROM metadata WHERE key=?1", [key], |r| r.get::<_, String>(0)).optional();
    Ok(Auto { on: get(AUTO_KEY)?.is_some_and(|v| v == "true"), since: get(AUTO_SINCE)?, paused: get(AUTO_PAUSED)? })
}
fn put(c: &Connection, key: &str, value: Option<&str>) -> Result<()> {
    match value {
        Some(v) => c.execute("INSERT INTO metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, v])?,
        None => c.execute("DELETE FROM metadata WHERE key=?1", [key])?,
    };
    Ok(())
}
fn millis(t: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(t).ok().map(|t| t.timestamp_millis())
}
/// Messages automatic mode takes: acquired at or after it was switched on.
fn since(todo: Vec<Waiting>, auto: &Auto) -> Vec<Waiting> {
    match auto.since.as_deref().and_then(millis).filter(|_| auto.on) {
        Some(from) => todo.into_iter().filter(|w| w.acquired_ms.is_some_and(|t| t >= from)).collect(),
        None => vec![],
    }
}

type Senders = Box<dyn Fn(&Store) -> Result<Sender> + Send + Sync>;
pub struct VesselBatch {
    store: Store,
    layers: Arc<LiveLayers>,
    run: Mutex<Run>,
    cancel: Mutex<Option<Arc<AtomicBool>>>,
    sender: Senders,
}

impl VesselBatch {
    pub fn new(store: Store, layers: Arc<LiveLayers>) -> Arc<Self> {
        Self::with_sender(store, layers, Box::new(Sender::configured))
    }
    fn with_sender(store: Store, layers: Arc<LiveLayers>, sender: Senders) -> Arc<Self> {
        Arc::new(Self { store, layers, run: Mutex::new(Run::default()), cancel: Mutex::new(None), sender })
    }
    /// Automatic mode: messages acquired since it was switched on are checked
    /// every two minutes.
    pub fn start(self: &Arc<Self>) {
        let batch = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(AUTO_EVERY).await;
                if batch.store.shutdown.load(Ordering::Acquire) {
                    break;
                }
                if batch.auto().await.is_ok_and(|a| a.on) && !batch.is_running() {
                    let _ = batch.clone().begin(true).await;
                }
            }
        });
    }
    fn is_running(&self) -> bool {
        self.run.lock().is_ok_and(|r| r.running)
    }
    async fn auto(&self) -> Result<Auto> {
        let s = self.store.clone();
        tokio::task::spawn_blocking(move || read_auto(&*s.connect()?)).await.map_err(|_| Error::internal())?
    }
    /// Switching on records the moment, and only messages acquired from then
    /// on are taken automatically; the backlog waits for an explicit start.
    /// Switching on again while on keeps that moment. Either change clears
    /// the reason automatic mode last switched itself off. Switching on needs
    /// a passed evaluation of the current profile; switching off never does.
    pub fn set_auto(&self, on: bool) -> Result<()> {
        if on {
            self.store.vessel_gate()?;
        }
        let _lock = self.store.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.store.connect()?;
        let tx = c.transaction()?;
        if on && read_auto(&tx)?.on {
            return Ok(());
        }
        put(&tx, AUTO_KEY, Some(if on { "true" } else { "false" }))?;
        put(&tx, AUTO_SINCE, on.then(now).as_deref())?;
        put(&tx, AUTO_PAUSED, None)?;
        tx.commit()?;
        Ok(())
    }
    /// Switch automatic mode off after a run stopped early, keeping the reason.
    /// Not when the analyst switched it on again since the run started.
    fn pause_auto(&self, reason: &str, started: &Auto) -> Result<()> {
        let _lock = self.store.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.store.connect()?;
        let tx = c.transaction()?;
        let current = read_auto(&tx)?;
        if !current.on || current.since != started.since {
            return Ok(());
        }
        put(&tx, AUTO_KEY, Some("false"))?;
        put(&tx, AUTO_SINCE, None)?;
        put(&tx, AUTO_PAUSED, Some(reason))?;
        tx.commit()?;
        Ok(())
    }
    /// Backlog, automatic mode, totals and the current or last run.
    pub async fn status(&self) -> Result<Value> {
        let s = self.store.clone();
        let (todo, flagged, totals, auto, (evaluated, evaluation)) = tokio::task::spawn_blocking(move || {
            let (todo, flagged) = s.vessel_backlog()?;
            Ok::<_, Error>((todo, flagged, s.vessel_totals()?, read_auto(&*s.connect()?)?, s.vessel_evaluation()?))
        })
        .await
        .map_err(|_| Error::internal())??;
        let waiting = todo.len();
        let auto_waiting = since(todo, &auto).len();
        let r = self.run.lock().map_err(|_| Error::internal())?;
        Ok(json!({"flagged":flagged,"waiting":waiting,"totals":totals,"auto":auto.on,"auto_since":auto.since.filter(|_| auto.on),"auto_paused_reason":auto.paused,"auto_waiting":auto_waiting,"model":vessel_finder::MODEL,
            "evaluated":evaluated,"evaluation":evaluation,
            "run":{"running":r.running,"automatic":r.automatic,"started_at":r.started_at,"finished_at":r.finished_at,"planned":r.planned,"done":r.done,"found":r.found,"failed":r.failed,
                "cost_usd":r.cost_usd,"unknown_cost_requests":r.unknown_cost,"stop_reason":r.stop_reason}}))
    }
    pub fn stop(&self) {
        if let Ok(Some(cancel)) = self.cancel.lock().map(|c| c.clone()) {
            cancel.store(true, Ordering::Release);
        }
    }
    /// Start a run: every waiting message, or with `automatic` only those
    /// acquired since automatic mode was switched on.
    pub async fn begin(self: Arc<Self>, automatic: bool) -> Result<()> {
        if self.store.shutdown.load(Ordering::Acquire) {
            return Err(Error::conflict("Relic is stopping"));
        }
        let s = self.store.clone();
        let (policy_on, evaluated, todo, auto) = tokio::task::spawn_blocking(move || {
            let policy_on = s.processing_policy()?.enabled;
            Ok::<_, Error>((policy_on, s.vessel_evaluation()?.0, s.vessel_backlog()?.0, read_auto(&*s.connect()?)?))
        })
        .await
        .map_err(|_| Error::internal())??;
        if !policy_on {
            return Err(Error::conflict("Processing is paused; resume it to find ships"));
        }
        // Like a configuration error: nothing starts, and automatic mode
        // switches off with the reason, so switching it on again after an
        // evaluation is installed takes only messages from then on.
        if !evaluated {
            let batch = self.clone();
            tokio::task::spawn_blocking(move || batch.pause_auto(vessel_finder::NEEDS_EVALUATION, &auto)).await.map_err(|_| Error::internal())??;
            return Err(Error::conflict(vessel_finder::NEEDS_EVALUATION));
        }
        let todo = if automatic { since(todo, &auto) } else { todo };
        let cancel = Arc::new(AtomicBool::new(false));
        let sender = {
            let mut run = self.run.lock().map_err(|_| Error::internal())?;
            if run.running {
                return Err(Error::conflict("The ship finder is already running"));
            }
            if todo.is_empty() {
                return Ok(());
            }
            // Built before anything is reserved: a configuration error sends nothing.
            let sender = (self.sender)(&self.store);
            match &sender {
                Ok(_) => {
                    *run = Run { running: true, started_at: Some(now()), planned: todo.len(), automatic, ..Run::default() };
                    *self.cancel.lock().map_err(|_| Error::internal())? = Some(cancel.clone());
                }
                Err(e) => *run = Run { started_at: Some(now()), finished_at: Some(now()), planned: todo.len(), automatic, stop_reason: Some(e.message.clone()), ..Run::default() },
            }
            sender
        };
        let sender = match sender {
            Ok(sender) => Arc::new(sender),
            Err(e) => {
                let (batch, reason) = (self.clone(), e.message.clone());
                let _ = tokio::task::spawn_blocking(move || batch.pause_auto(&reason, &auto)).await;
                return Err(e);
            }
        };
        let job = format!("processing:vessels:{}", crate::model::ident("run"));
        self.store.running.lock().map_err(|_| Error::internal())?.insert(job.clone(), cancel.clone());
        let batch = self.clone();
        tokio::spawn(async move {
            let end = batch.work(todo, cancel.clone(), sender).await;
            if let Ok(mut running) = batch.store.running.lock() {
                running.remove(&job);
            }
            let reason = match &end {
                End::Complete => None,
                End::Stopped(r) => Some(r.clone()),
                End::Shutdown => Some("Relic stopped; the rest are found on the next run".to_owned()),
            };
            // Before the run reads finished, so the loop cannot restart it.
            if let End::Stopped(r) = end {
                let (b, r) = (batch.clone(), r);
                let _ = tokio::task::spawn_blocking(move || b.pause_auto(&r, &auto)).await;
            }
            if let Ok(mut run) = batch.run.lock() {
                run.running = false;
                run.finished_at = Some(now());
                run.stop_reason = reason;
            }
            // A newer run may already hold the slot; only this run's flag is cleared.
            if let Ok(mut c) = batch.cancel.lock()
                && c.as_ref().is_some_and(|c| Arc::ptr_eq(c, &cancel))
            {
                *c = None;
            }
        });
        Ok(())
    }
    /// Extract and match each message; says how the run ended. A stop lets
    /// requests already sent finish and be recorded; no new one is sent.
    /// Matching runs inside each message's slot, so a slow KeelWatch never
    /// holds up the requests in flight.
    async fn work(&self, todo: Vec<Waiting>, cancel: Arc<AtomicBool>, sender: Arc<Sender>) -> End {
        let stopped = || cancel.load(Ordering::Acquire) || self.store.shutdown.load(Ordering::Acquire);
        let paused = AtomicBool::new(false);
        let mut failures = 0u32;
        let mut reason = None;
        let mut stream = futures_util::stream::iter(todo.into_iter().map(|w| {
            let (paused, stopped, cancel, sender) = (&paused, &stopped, cancel.clone(), sender.clone());
            async move {
                if stopped() {
                    return None;
                }
                // Pause in Processing holds the run: no new request starts.
                let s = self.store.clone();
                let enabled = tokio::task::spawn_blocking(move || s.processing_policy().map(|p| p.enabled)).await.ok().and_then(|r| r.ok()).unwrap_or(false);
                if !enabled {
                    paused.store(true, Ordering::Release);
                    cancel.store(true, Ordering::Release);
                    return None;
                }
                let outcome = vessel_finder::extract_with(&self.store, sender, &w.id, &w.revision, cancel).await;
                if let Ok(Extraction::Recorded(a)) = &outcome
                    && a.status == "completed"
                {
                    // Free KeelWatch matching keeps the suggestion for the feed.
                    let _ = vessel_finder::suggest(&self.store, &self.layers, &w.id, &w.revision).await;
                }
                Some(outcome)
            }
        }))
        .buffer_unordered(CONCURRENCY);
        while let Some(outcome) = stream.next().await {
            let attempt = match outcome {
                // Not started, or stopped before sending: nothing to count.
                None | Some(Ok(Extraction::Withheld)) => continue,
                // Completed earlier (the reader's button); nothing sent now.
                Some(Ok(Extraction::Reused)) => {
                    if let Ok(mut run) = self.run.lock() {
                        run.done += 1;
                    }
                    continue;
                }
                Some(Ok(Extraction::Recorded(a))) => a,
                Some(Err(e)) => {
                    if let Ok(mut run) = self.run.lock() {
                        run.done += 1;
                        run.failed += 1;
                    }
                    failures += 1;
                    if reason.is_none() && failures >= FAILURE_STOP {
                        cancel.store(true, Ordering::Release);
                        reason = Some(format!("Stopped after {FAILURE_STOP} failed requests in a row. Latest: {}", e.message));
                    }
                    continue;
                }
            };
            if let Ok(mut run) = self.run.lock() {
                run.done += 1;
                match attempt.cost_usd {
                    Some(cost) => run.cost_usd += cost,
                    None => run.unknown_cost += 1,
                }
                if attempt.status == "completed" {
                    run.found += attempt.mentions;
                } else {
                    run.failed += 1;
                }
            }
            if attempt.status == "completed" {
                failures = 0;
                continue;
            }
            failures += 1;
            let message = attempt.error.unwrap_or_else(|| "The ship finder request failed".into());
            if reason.is_none() && attempt.billing == "rejected" {
                cancel.store(true, Ordering::Release);
                reason = Some(message);
            } else if reason.is_none() && failures >= FAILURE_STOP {
                cancel.store(true, Ordering::Release);
                reason = Some(format!("Stopped after {FAILURE_STOP} failed requests in a row. Latest: {message}"));
            }
        }
        if let Some(r) = reason {
            End::Stopped(r)
        } else if paused.load(Ordering::Acquire) {
            End::Stopped("Processing was paused; the rest wait for the next run".into())
        } else if self.store.shutdown.load(Ordering::Acquire) {
            End::Shutdown
        } else if cancel.load(Ordering::Acquire) {
            End::Stopped("Stopped".into())
        } else {
            End::Complete
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vessel_finder::fixture;

    const TEXT: &str = "The tanker FICTION STAR was hit.";
    fn batch(s: &Store, model: &fixture::Model) -> Arc<VesselBatch> {
        let url = model.url();
        VesselBatch::with_sender(s.clone(), Arc::new(LiveLayers::new(&Default::default())), Box::new(move |_| Ok(Sender::Local(url.clone()))))
    }
    async fn finished(b: &VesselBatch) -> Value {
        fixture::wait(|| !b.is_running()).await;
        b.status().await.unwrap()
    }
    fn many(n: usize) -> Vec<(&'static str, &'static str)> {
        vec![("2026-09-20T10:00:00Z", TEXT); n]
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn failures_stop_the_run_and_switch_automatic_mode_off() {
        let (_tmp, s) = fixture::workspace(&many(20));
        let model = fixture::model(500, json!({"error":{"message":"Fictional outage"}})).await;
        let b = batch(&s, &model);
        b.set_auto(true).unwrap();
        b.clone().begin(false).await.unwrap();
        let status = finished(&b).await;
        let reason = status["run"]["stop_reason"].as_str().unwrap().to_owned();
        assert!(reason.starts_with("Stopped after 5 failed requests in a row. Latest: Model provider HTTP 500"), "{reason}");
        let sent = model.calls();
        assert!((5..20).contains(&sent), "{sent}");
        // Each failure was sent and stated no cost: unknown, never resent by a run.
        assert_eq!(status["totals"]["unknown_cost_requests"], sent);
        assert_eq!(status["run"]["unknown_cost_requests"], sent);
        assert_eq!(status["waiting"], 20 - sent, "requests withheld or never started wait again");
        assert_eq!((status["auto"].clone(), status["auto_paused_reason"].clone()), (json!(false), json!(reason)));
        // Automatic mode stays off until switched on again.
        b.set_auto(true).unwrap();
        assert_eq!(b.status().await.unwrap()["auto_paused_reason"], Value::Null);
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn exhausted_credits_stop_the_run_and_every_message_waits_again() {
        let (_tmp, s) = fixture::workspace(&many(20));
        let model = fixture::model(402, fixture::credits()).await;
        let b = batch(&s, &model);
        b.set_auto(true).unwrap();
        b.clone().begin(false).await.unwrap();
        let status = finished(&b).await;
        assert_eq!(status["run"]["stop_reason"], "OpenRouter credits are exhausted; nothing was generated or billed.");
        assert!(model.calls() >= 1);
        // Nothing was billed: no message is consumed and no cost is unknown.
        assert_eq!(status["waiting"], 20);
        assert_eq!(status["totals"]["unknown_cost_requests"], 0);
        assert_eq!(status["totals"]["known_cost_usd"], 0.0);
        assert_eq!(status["totals"]["attempted"], 0);
        assert_eq!(status["auto"], false);
        assert_eq!(status["auto_paused_reason"], status["run"]["stop_reason"]);
        // With credits again, a run finds them all.
        model.reply(200, fixture::answer(0.001));
        b.clone().begin(false).await.unwrap();
        let status = finished(&b).await;
        assert_eq!((status["waiting"].clone(), status["run"]["stop_reason"].clone()), (json!(0), Value::Null));
        assert_eq!(status["totals"]["completed"], 20);
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn automatic_mode_takes_only_messages_acquired_since_it_was_switched_on() {
        let (_tmp, s) = fixture::workspace(&[("2026-09-20T10:00:00Z", TEXT), ("2026-09-21T10:00:00Z", TEXT), ("2026-10-05T10:00:00Z", TEXT)]);
        let model = fixture::model(200, fixture::answer(0.001)).await;
        let b = batch(&s, &model);
        // Switching on starts no run over the backlog.
        b.set_auto(true).unwrap();
        let status = b.status().await.unwrap();
        assert_eq!((status["auto"].clone(), status["waiting"].clone(), status["auto_waiting"].clone()), (json!(true), json!(3), json!(0)));
        assert!(status["auto_since"].is_string());
        assert!(!b.is_running());
        // As if switched on on 1 October: one message is new since.
        put(&s.connect().unwrap(), AUTO_SINCE, Some("2026-10-01T00:00:00.000Z")).unwrap();
        assert_eq!(b.status().await.unwrap()["auto_waiting"], 1);
        b.clone().begin(true).await.unwrap();
        let status = finished(&b).await;
        assert_eq!((model.calls(), status["waiting"].clone(), status["auto"].clone()), (1, json!(2), json!(true)));
        assert_eq!(status["run"]["automatic"], true);
        // The explicit start covers the whole backlog.
        b.clone().begin(false).await.unwrap();
        let waiting = finished(&b).await["waiting"].clone();
        assert_eq!((model.calls(), waiting), (3, json!(0)));
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stop_lets_sent_requests_finish_and_sends_no_more() {
        let (_tmp, s) = fixture::workspace(&many(20));
        let model = fixture::model(200, fixture::answer(0.001)).await;
        model.hold();
        let b = batch(&s, &model);
        b.set_auto(true).unwrap();
        b.clone().begin(false).await.unwrap();
        fixture::wait(|| model.calls() == CONCURRENCY).await;
        b.stop();
        model.release();
        let status = finished(&b).await;
        assert_eq!(model.calls(), CONCURRENCY);
        assert_eq!((status["run"]["done"].clone(), status["totals"]["completed"].clone()), (json!(CONCURRENCY), json!(CONCURRENCY)));
        assert_eq!(status["waiting"], 20 - CONCURRENCY);
        assert_eq!(status["run"]["stop_reason"], "Stopped");
        assert_eq!((status["auto"].clone(), status["auto_paused_reason"].clone()), (json!(false), json!("Stopped")));
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_a_passed_evaluation_nothing_starts_and_automatic_mode_switches_off() {
        let (_tmp, s) = fixture::workspace(&many(3));
        let model = fixture::model(200, fixture::answer(0.001)).await;
        let b = batch(&s, &model);
        b.set_auto(true).unwrap();
        // The installed evaluation is gone, as for a workspace never evaluated.
        s.connect().unwrap().execute("DELETE FROM metadata WHERE key=?1", [vessel_finder::EVALUATION_KEY]).unwrap();
        let status = b.status().await.unwrap();
        assert_eq!((status["evaluated"].clone(), status["evaluation"].clone()), (json!(false), Value::Null));
        // Automatic mode already on: its next check starts nothing and switches it off.
        assert_eq!(b.clone().begin(true).await.unwrap_err().message, vessel_finder::NEEDS_EVALUATION);
        let status = b.status().await.unwrap();
        assert_eq!((status["auto"].clone(), status["auto_paused_reason"].clone()), (json!(false), json!(vessel_finder::NEEDS_EVALUATION)));
        // It cannot be switched on again, and an explicit start is refused too.
        assert_eq!(b.set_auto(true).unwrap_err().message, vessel_finder::NEEDS_EVALUATION);
        assert_eq!(b.clone().begin(false).await.unwrap_err().message, vessel_finder::NEEDS_EVALUATION);
        assert!(!b.is_running());
        assert_eq!((model.calls(), status["waiting"].clone(), status["totals"]["attempted"].clone(), status["run"]["started_at"].clone()), (0, json!(3), json!(0), Value::Null));
        // Switching off is always allowed.
        b.set_auto(false).unwrap();
        // With a passed evaluation of the current profile, the run starts.
        fixture::evaluated(&s);
        b.set_auto(true).unwrap();
        b.clone().begin(false).await.unwrap();
        let status = finished(&b).await;
        assert_eq!((model.calls(), status["waiting"].clone(), status["evaluated"].clone(), status["evaluation"]["passed"].clone()), (3, json!(0), json!(true), json!(true)));
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_configuration_error_starts_nothing_and_pauses_automatic_mode() {
        let (_tmp, s) = fixture::workspace(&many(2));
        let b = VesselBatch::new(s.clone(), Arc::new(LiveLayers::new(&Default::default())));
        b.set_auto(true).unwrap();
        assert_eq!(b.clone().begin(false).await.err().unwrap().message, "AI processing is not configured");
        let status = b.status().await.unwrap();
        assert_eq!((status["waiting"].clone(), status["totals"]["attempted"].clone()), (json!(2), json!(0)));
        assert_eq!((status["auto"].clone(), status["auto_paused_reason"].clone()), (json!(false), json!("AI processing is not configured")));
    }
}
