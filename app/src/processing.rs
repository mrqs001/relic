//! Concurrent requests with one coherent publication coordinator.
use crate::{
    error::{Error, Result},
    model::{ident, now, value},
    processing_ai::{self, Client},
    processing_geo::GeoResolver,
    processing_store::{self, Policy},
    store::Store,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::{sync::Semaphore, task::JoinSet};

pub const CONCURRENCY: usize = 64;

pub struct Runtime {
    store: Store,
    worker: Arc<Semaphore>,
    ready: tokio::sync::Notify,
}
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| Error::internal())?
}
// A paid response rejected by content validation concerns this message only.
// Unknown billing, provider/transport failures and wrong models stop the batch.
fn skippable_rejection(error: &processing_ai::AiError) -> bool {
    use processing_ai::ErrorKind;
    error
        .audit
        .as_ref()
        .is_some_and(|audit| audit.usage.cost_credits.is_some())
        && matches!(
            error.kind,
            ErrorKind::Incomplete
                | ErrorKind::InvalidOutput
                | ErrorKind::UnsupportedQuote
                | ErrorKind::UntranslatedEcho
                | ErrorKind::AmbiguousQuote
                | ErrorKind::Refused
                | ErrorKind::UnexpectedAction
        )
}
impl Runtime {
    pub fn new(store: Store) -> Arc<Self> {
        Arc::new(Self {
            store,
            worker: Arc::new(Semaphore::new(1)),
            ready: tokio::sync::Notify::new(),
        })
    }
    pub fn is_running(&self) -> bool {
        self.worker.available_permits() == 0
            || self.store.running.lock().is_ok_and(|r| {
                r.keys().any(|id| {
                    id.starts_with("processing:supplemental:")
                        || id.starts_with("processing:relationships:")
                })
            })
    }
    pub fn status(&self) -> Result<Value> {
        self.store.processing_status(self.is_running())
    }
    pub fn start(self: &Arc<Self>) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(15));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! { _ = tick.tick() => {}, _ = runtime.ready.notified() => {} }
                if runtime.store.shutdown.load(Ordering::Acquire) {
                    break;
                }
                let s = runtime.store.clone();
                if blocking(move || s.processing_policy())
                    .await
                    .is_ok_and(|p| p.enabled)
                {
                    // Admission failures are surfaced by status; they never trigger retries of a source request.
                    let _ = runtime.run().await;
                }
            }
        });
    }
    pub async fn set_policy(&self, policy: Policy) -> Result<Value> {
        let s = self.store.clone();
        blocking(move || s.set_processing_policy(policy)).await?;
        self.ready.notify_one();
        self.status()
    }
    pub async fn set_source_reading(&self, source: String, reading: bool) -> Result<Value> {
        let s = self.store.clone();
        blocking(move || s.set_source_reading(&source, reading)).await?;
        self.ready.notify_one();
        self.status()
    }
    pub async fn stop(&self) -> Result<Value> {
        let ids: Vec<String> = self
            .store
            .running
            .lock()
            .map_err(|_| Error::internal())?
            .keys()
            .filter(|id| id.starts_with("processing:"))
            .cloned()
            .collect();
        for id in ids {
            // Lanes without a job record (Jev, event links, the ship finder)
            // are stopped by their flag alone.
            if id.starts_with("processing:supplemental:")
                || id.starts_with("processing:relationships:")
                || id.starts_with("processing:vessels:")
            {
                if let Some(flag) = self
                    .store
                    .running
                    .lock()
                    .map_err(|_| Error::internal())?
                    .get(&id)
                {
                    flag.store(true, Ordering::Release);
                }
                continue;
            }
            let s = self.store.clone();
            blocking(move || s.cancel_job(&id)).await?;
        }
        self.status()
    }
    pub async fn reresolve(self: &Arc<Self>) -> Result<Value> {
        let permit = self
            .worker
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::busy())?;
        let s = self.store.clone();
        crate::geography_store::start_job(s, "geography", move |s, id, cancel| {
            let _permit = permit;
            s.reresolve_geography(id, cancel)
        })
        .await
    }
    pub async fn recover(self: &Arc<Self>) -> Result<Value> {
        let permit = self
            .worker
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::busy())?;
        crate::geography_store::start_job(
            self.store.clone(),
            "processing-recovery",
            move |s, _, cancel| {
                let _permit = permit;
                s.recover_processing(cancel)
            },
        )
        .await
    }
    pub async fn run(self: &Arc<Self>) -> Result<Value> {
        if self.store.shutdown.load(Ordering::Acquire) {
            return Err(Error::busy());
        }
        let permit = self
            .worker
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::busy())?;
        let config = processing_ai::Config::from_mapping(&self.store.config.source_values)
            .map_err(|e| Error::invalid(e.to_string()))?;
        let client = Client::new(config).map_err(|e| Error::invalid(e.to_string()))?;
        self.run_with_client(client, permit).await
    }
    async fn run_with_client(
        self: &Arc<Self>,
        client: Client,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<Value> {
        let model = client.model().to_owned();
        let s = self.store.clone();
        let backoff = blocking(move || s.processing_backoff()).await?;
        if backoff["retry_at"]
            .as_str()
            .is_some_and(|at| at > now().as_str())
        {
            return Err(Error::conflict(if backoff["reason"] == "credits" {
                "OpenRouter credits are exhausted; reading resumes after credits are added"
            } else {
                "Rate limited; processing resumes after the provider cooldown"
            }));
        }
        let concurrency = backoff["concurrency"]
            .as_u64()
            .unwrap_or(CONCURRENCY as u64)
            .clamp(1, CONCURRENCY as u64) as usize;
        let s = self.store.clone();
        let (policy, evaluation) =
            blocking(move || Ok((s.processing_policy()?, s.processing_evaluation()?))).await?;
        if policy.eligible_source_ids.is_empty() {
            return Err(Error::invalid(
                "No source is eligible for external processing yet",
            ));
        }
        if !evaluation.is_some_and(|e| {
            e["recipe"] == processing_store::RECIPE
                && processing_ai::evaluation_matches(&e, &model, false)
        }) {
            return Err(Error::conflict(
                "The configured processing recipe needs a passed fictional evaluation before live use",
            ));
        }
        let s = self.store.clone();
        if let Some(receipt_request) = blocking(move || s.claim_processing_receipt()).await? {
            // Read-only recovery cannot regenerate the failed message. Missing,
            // delayed or rejected receipts retain their unknown-cost estimate.
            if let Ok(receipt) = client
                .generation_receipt(&receipt_request.generation_id)
                .await
            {
                let s = self.store.clone();
                blocking(move || s.apply_processing_receipt(&receipt_request, &receipt)).await?;
            }
        }
        let s = self.store.clone();
        let p = policy.clone();
        let pending = blocking(move || s.processing_pending(&p)).await?;
        if pending.is_empty() {
            drop(permit);
            return self.status();
        }
        let resolver_store = self.store.clone();
        let resolver = blocking(move || {
            GeoResolver::load_for_store_cancellable(&resolver_store, &resolver_store.shutdown)
                .map_err(|error| {
                    if error.code == "cancelled" {
                        error
                    } else {
                        Error::invalid("Accepted city reference is unavailable")
                    }
                })
        })
        .await?;
        let id = ident("processing");
        let cancel = Arc::new(AtomicBool::new(false));
        self.store
            .running
            .lock()
            .map_err(|_| Error::internal())?
            .insert(id.clone(), cancel.clone());
        let initial = json!({"id":id,"kind":"processing","status":"running","total":pending.len(),"accepted":0,"created_at":now(),"detail":"Translating messages and checking exact support; prior publication remains usable."});
        let s = self.store.clone();
        let j = initial.clone();
        if let Err(error) = blocking(move || s.save_job(&j)).await {
            self.store
                .running
                .lock()
                .map_err(|_| Error::internal())?
                .remove(&id);
            return Err(error);
        }
        let runtime = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let mut job = initial;
            let mut prepared = vec![];
            let mut failure = None;
            let mut rejected = 0usize;
            let client = Arc::new(client);
            let resolver = Arc::new(resolver);
            let mut rows = pending.into_iter();
            let mut requests = JoinSet::new();
            let mut progress_at = std::time::Instant::now();
            loop {
                while failure.is_none() && requests.len() < concurrency {
                    if cancel.load(Ordering::Acquire)
                        || runtime.store.shutdown.load(Ordering::Acquire)
                    {
                        failure = Some(Error::cancelled());
                        break;
                    }
                    let Some(row) = rows.next() else {
                        break;
                    };
                    let store = runtime.store.clone();
                    let client = client.clone();
                    let cancel = cancel.clone();
                    requests.spawn(async move {
                        let result = analyze_pending(&store, &client, &row, &cancel).await;
                        (row, result)
                    });
                }
                let Some(done) = requests.join_next().await else {
                    break;
                };
                let (row, analysis) = match done {
                    Ok((row, Ok(Some(analysis)))) => (row, analysis),
                    Ok((_, Ok(None))) => {
                        rejected += 1;
                        continue;
                    }
                    Ok((_, Err(e))) => {
                        failure.get_or_insert(e);
                        continue;
                    }
                    Err(_) => {
                        failure.get_or_insert_with(Error::internal);
                        continue;
                    }
                };
                let resolver = resolver.clone();
                let model = model.clone();
                match blocking(move || {
                    let mut candidate = processing_store::prepare(
                        row.rowid,
                        &row.evidence,
                        &analysis.extracted,
                        &resolver,
                        &model,
                    )?;
                    candidate.audit = value(&analysis.audit);
                    if analysis.audit.parameters["reused_from"].is_object() {
                        candidate.interpretation["reused_from"] =
                            analysis.audit.parameters["reused_from"].clone();
                    }
                    if let Some(prior) = &row.repair_of {
                        candidate.interpretation["reading_revision"] =
                            json!("translation-repair-v1");
                        candidate.interpretation["replaces_reading"] = json!(prior);
                    }
                    Ok(candidate)
                })
                .await
                {
                    Ok(candidate) => prepared.push(candidate),
                    Err(e) => {
                        failure.get_or_insert(e);
                    }
                }
                job["accepted"] = json!(prepared.len());
                job["detail"] = json!(format!(
                    "{} ready · {} rejected · {} requests in flight.",
                    prepared.len(),
                    rejected,
                    requests.len()
                ));
                if progress_at.elapsed() >= Duration::from_secs(1) {
                    let s = runtime.store.clone();
                    let j = job.clone();
                    if let Err(e) = blocking(move || s.save_job(&j)).await {
                        failure.get_or_insert(e);
                    }
                    progress_at = std::time::Instant::now();
                }
                // On failure, stop refilling but drain already paid requests and
                // persist their receipts before publishing the validated subset.
            }
            let count = prepared.len();
            let s = runtime.store.clone();
            let run = id.clone();
            let flag = cancel.clone();
            let published = blocking(move || s.publish_processing(prepared, &run, &flag)).await;
            match published {
                Ok(publication) => job["publication_id"] = value(publication),
                Err(e) => failure = Some(e),
            };
            job["status"] = json!(match &failure {
                None if rejected > 0 => "partial",
                None => "completed",
                Some(e) if e.code == "cancelled" => "cancelled",
                Some(_) => "failed",
            });
            job["accepted"] = if job["publication_id"].is_string() {
                json!(count)
            } else {
                json!(0)
            };
            let continue_work = failure.is_none();
            job["detail"] = json!(match failure {
                Some(e) => format!(
                    "{} validated messages published. {}",
                    job["accepted"], e.message
                ),
                None if rejected > 0 => format!(
                    "{count} messages processed; {rejected} skipped because the response did not pass validation. Originals remain available."
                ),
                None =>
                    format!("{count} messages processed. Originals and exact support retained."),
            });
            if let Ok(mut running) = runtime.store.running.lock() {
                running.remove(&id);
            }
            drop(_permit);
            let s = runtime.store.clone();
            // The durable job event is emitted after the worker is released, so
            // the UI's status refresh observes the completed worker state.
            let _ = blocking(move || s.save_job(&job)).await;
            if continue_work {
                let s = runtime.store.clone();
                let _ = blocking(move || s.relax_processing_backoff()).await;
                runtime.ready.notify_one();
            }
        });
        self.status()
    }
}

// Network requests overlap; each attempt is durably reserved before sending.
async fn analyze_pending(
    store: &Store,
    client: &Client,
    row: &processing_store::Pending,
    cancel: &AtomicBool,
) -> Result<Option<processing_ai::Analysis>> {
    if cancel.load(Ordering::Acquire) || store.shutdown.load(Ordering::Acquire) {
        return Err(Error::cancelled());
    }
    let s = store.clone();
    let source = row.evidence["source"].as_str().unwrap_or("").to_owned();
    let rowid = row.rowid;
    let cached = blocking(move || {
        if !s.processing_policy()?.eligible_source_ids.contains(&source) {
            return Err(Error::conflict(
                "Source processing eligibility changed; remaining work stopped",
            ));
        }
        s.cached_processing(rowid)
    })
    .await?;
    if let Some(analysis) = cached {
        return Ok(Some(analysis));
    }
    if cancel.load(Ordering::Acquire) || store.shutdown.load(Ordering::Acquire) {
        return Err(Error::cancelled());
    }
    let s = store.clone();
    let correction_of = row.correction_of.clone();
    let attempt = blocking(move || match correction_of {
        Some(prior) => s.reserve_processing_correction(rowid, &prior),
        None => s.reserve_processing(rowid),
    })
    .await?;
    let receipt_store = store.clone();
    let receipt_attempt = attempt.clone();
    let receipt_model = client.model().to_owned();
    let request = client.analyze_reading_with_receipt(
        row.evidence["text"].as_str().unwrap_or(""),
        row.correction_of.is_some(),
        move |generation| async move {
            blocking(move || {
                receipt_store.save_processing_generation(
                    &receipt_attempt,
                    &generation,
                    &receipt_model,
                )
            })
            .await
            .map_err(|_| ())
        },
    );
    let cancel_watch = async {
        loop {
            if cancel.load(Ordering::Acquire) || store.halt.load(Ordering::Acquire) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    let result = tokio::select! { biased; _=cancel_watch=>None, r=request=>Some(r) };
    if result.as_ref().is_some_and(|r| {
        r.as_ref()
            .err()
            .is_some_and(|e| e.rejected_before_generation())
    }) {
        let Some(Err(error)) = result else {
            unreachable!()
        };
        let credits = error.audit.as_ref().is_some_and(|a| a.http_status == 402);
        let s = store.clone();
        blocking(move || s.defer_processing_attempt(&attempt, &error)).await?;
        return Err(Error::conflict(if credits {
            "OpenRouter credits are exhausted; reading resumes after credits are added"
        } else {
            "Rate limited; queued for retry after the provider cooldown"
        }));
    }
    let (state, cost, audit, error) = match &result {
        Some(Ok(v)) => ("completed", v.audit.usage.cost_credits, value(v), None),
        Some(Err(e)) => (
            "failed",
            e.audit.as_ref().and_then(|a| a.usage.cost_credits),
            e.audit.as_ref().map(value).unwrap_or(Value::Null),
            Some(e.to_string()),
        ),
        None => (
            "unknown",
            None,
            Value::Null,
            Some(
                "Processing cancelled during a request; billing is unknown and no retry was sent"
                    .into(),
            ),
        ),
    };
    let s = store.clone();
    blocking(move || s.finish_processing_attempt(&attempt, state, cost, &audit, error.as_deref()))
        .await?;
    match result {
        Some(Ok(v)) => Ok(Some(v)),
        Some(Err(e)) if skippable_rejection(&e) => Ok(None),
        Some(Err(e)) => Err(Error::invalid(e.to_string())),
        None => Err(Error::cancelled()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    #[derive(Clone)]
    struct Mock {
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        calls: Arc<AtomicUsize>,
        gate: Arc<Semaphore>,
        fail: bool,
        response: Value,
    }
    async fn reply(
        axum::extract::State(mock): axum::extract::State<Mock>,
    ) -> (axum::http::StatusCode, axum::Json<Value>) {
        let n = mock.calls.fetch_add(1, Ordering::SeqCst);
        let active = mock.active.fetch_add(1, Ordering::SeqCst) + 1;
        mock.peak.fetch_max(active, Ordering::SeqCst);
        mock.gate.acquire().await.unwrap().forget();
        if mock.fail && n != 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        mock.active.fetch_sub(1, Ordering::SeqCst);
        if mock.fail && n == 0 {
            (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                axum::Json(json!({"error":{"message":"Fictional provider rate limit"}})),
            )
        } else {
            (axum::http::StatusCode::OK, axum::Json(mock.response))
        }
    }
    async fn wait_until(mut ready: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(20), async {
            while !ready() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fictional worker did not reach expected state");
    }
    async fn concurrent_fixture(
        fail: bool,
    ) -> (
        tempfile::TempDir,
        Arc<Runtime>,
        Mock,
        tokio::task::JoinHandle<()>,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let store = Store::open(crate::config::Config::local(
            root,
            tmp.path().join("data"),
            "127.0.0.1:8793".parse().unwrap(),
        ))
        .unwrap();
        let fixture: Value =
            serde_json::from_str(include_str!("../../examples/processing-ai.json")).unwrap();
        let text = fixture["cases"][0]["original"].as_str().unwrap();
        let rows:Vec<Value> = (0..140).map(|n|json!({"id":format!("fiction:{n}"),"revision":"v1","source":"fiction:alpha","published_at":"2026-09-10T12:00:00Z","acquired_at":"2026-09-10T12:01:00Z","text":text,"geometry":null,"location_precision":"unresolved","origin_group":null,"dependence":"unknown","language":"en","fictional":true,"availability":"available","citation":{"evidence_id":format!("fiction:{n}"),"revision":"v1","start_character":0,"end_character":text.chars().count()}})).collect();
        store.import_job(json!({"schema_version":1,"fictional":true,"title":"Concurrent fictional processing","sources":[{"id":"fiction:alpha","label":"Fictional Alpha","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
        store
            .set_processing_policy(Policy {
                enabled: true,
                eligible_source_ids: vec!["fiction:alpha".into()],
            })
            .unwrap();
        store.record_processing_evaluation(&json!({"passed":true,"fictional":true,"recipe":processing_store::RECIPE,"prompt_version":processing_ai::PROMPT_VERSION,"model":processing_ai::DEFAULT_MODEL,"request_profile":processing_ai::evaluation_profile(processing_ai::DEFAULT_MODEL,false)})).unwrap();
        let mock = Mock {
            active: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(AtomicUsize::new(0)),
            gate: Arc::new(Semaphore::new(0)),
            fail,
            response: json!({"model":processing_ai::DEFAULT_MODEL,"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":fixture["cases"][0]["gold"].to_string()}}],"usage":{"cost":0.001}}),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new()
            .route("/chat/completions", axum::routing::post(reply))
            .with_state(mock.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = Client::new(processing_ai::Config {
            api_key: "fictional-not-secret".into(),
            endpoint: format!("http://{address}/chat/completions")
                .parse()
                .unwrap(),
            model: processing_ai::DEFAULT_MODEL.into(),
        })
        .unwrap();
        let runtime = Runtime::new(store);
        let permit = runtime.worker.clone().acquire_owned().await.unwrap();
        runtime.run_with_client(client, permit).await.unwrap();
        wait_until(|| mock.calls.load(Ordering::SeqCst) == CONCURRENCY).await;
        (tmp, runtime, mock, server)
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn requests_overlap_refill_and_publish_exactly_once() {
        let (_tmp, runtime, mock, server) = concurrent_fixture(false).await;
        assert_eq!(mock.peak.load(Ordering::SeqCst), CONCURRENCY);
        mock.gate.add_permits(140);
        wait_until(|| !runtime.is_running()).await;
        assert_eq!(mock.calls.load(Ordering::SeqCst), 140);
        assert!(mock.peak.load(Ordering::SeqCst) <= CONCURRENCY);
        let status = runtime.status().unwrap();
        assert_eq!(status["processed"], 140);
        assert_eq!(status["today"]["calls"], 140);
        assert_eq!(status["today"]["unknown_cost_requests"], 0);
        assert!(
            runtime
                .store
                .processing_pending(&runtime.store.processing_policy().unwrap())
                .unwrap()
                .is_empty()
        );
        server.abort();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancellation_drains_all_started_attempts_without_dispatching_more() {
        let (_tmp, runtime, mock, server) = concurrent_fixture(false).await;
        runtime.stop().await.unwrap();
        wait_until(|| !runtime.is_running()).await;
        assert_eq!(mock.calls.load(Ordering::SeqCst), CONCURRENCY);
        let c = runtime.store.connect().unwrap();
        let reserved: i64 = c
            .query_row(
                "SELECT count(*) FROM processing_attempts WHERE status='reserved'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reserved, 0);
        assert_eq!(
            runtime.status().unwrap()["today"]["unknown_cost_requests"],
            CONCURRENCY
        );
        assert_eq!(runtime.status().unwrap()["processed"], 0);
        mock.gate.add_permits(140);
        server.abort();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn provider_failure_stops_refill_but_saves_inflight_successes() {
        let (_tmp, runtime, mock, server) = concurrent_fixture(true).await;
        mock.gate.add_permits(140);
        wait_until(|| !runtime.is_running()).await;
        assert_eq!(mock.calls.load(Ordering::SeqCst), CONCURRENCY);
        assert_eq!(runtime.status().unwrap()["processed"], CONCURRENCY - 1);
        let c = runtime.store.connect().unwrap();
        let reserved: i64 = c
            .query_row(
                "SELECT count(*) FROM processing_attempts WHERE status='reserved'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reserved, 0);
        server.abort();
    }
    #[test]
    fn only_audited_content_rejections_can_skip_without_stopping_the_batch() {
        // Decode an actual response shape so this checks the same audit boundary
        // as paid runtime responses, without a network request.
        let response = serde_json::to_vec(&json!({"model":processing_ai::DEFAULT_MODEL,"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"{}"}}],"usage":{"cost":0.001}})).unwrap();
        let mut error = match processing_ai::replay_response(
            "A source message.",
            processing_ai::DEFAULT_MODEL,
            200,
            response,
            1,
        ) {
            Err(error) => error,
            Ok(_) => panic!("invalid schema accepted"),
        };
        assert!(skippable_rejection(&error));
        error.kind = processing_ai::ErrorKind::Incomplete;
        assert!(skippable_rejection(&error));
        error.audit.as_mut().unwrap().usage.cost_credits = None;
        assert!(!skippable_rejection(&error));
        error.audit.as_mut().unwrap().usage.cost_credits = Some(0.001);
        error.kind = processing_ai::ErrorKind::UnexpectedModel;
        assert!(!skippable_rejection(&error));
        error.kind = processing_ai::ErrorKind::HttpFailure;
        assert!(!skippable_rejection(&error));
    }
}
