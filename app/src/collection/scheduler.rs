use super::*;
use crate::model::ident;
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;

/// Waits after consecutive temporary source errors; the last value repeats.
const TRANSIENT_WAIT_MINUTES: [i64; 5] = [2, 5, 15, 30, 60];

/// "50 minutes", "5 hours", "3 days": how long ago `since` was.
fn elapsed(since: chrono::DateTime<Utc>) -> String {
    let minutes = (Utc::now() - since).num_minutes().max(0);
    if minutes < 120 {
        format!("{minutes} minutes")
    } else if minutes < 48 * 60 {
        format!("{} hours", minutes / 60)
    } else {
        format!("{} days", minutes / (24 * 60))
    }
}

fn due(sub: &Value, state: Option<&Value>, at: &str) -> bool {
    let valid = state.is_some_and(|state| state["configuration_version"] == sub["version"]);
    let requested = valid && state.is_some_and(|state| state["requested_action"].is_string());
    if sub["enabled"] != true && !requested {
        return false;
    }
    if requested {
        return true;
    }
    if valid
        && state.is_some_and(|state| {
            state["stopped"] == true
                || state["status"] == "blocked"
                || (state["continuation"].is_object() && !auto_continue(state))
        })
    {
        return false;
    }
    !valid
        || !state.is_some_and(|state| state["next_due_at"].as_str().is_some_and(|time| time > at))
}

impl Runtime {
    /// Durable account backoff applies before startup, reconnect, and every sweep.
    /// The connect endpoint calls this same gate before starting any networking.
    pub(super) async fn retry_gate(&self, provider: &str) -> Result<bool> {
        index(provider)?;
        let key = format!("collection:{provider}:retry_at");
        let persisted = self
            .block(move |store| {
                Ok(store
                    .connect()?
                    .query_row("SELECT value FROM metadata WHERE key=?1", [key], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?)
            })
            .await?;
        let current = self.connection(provider)?;
        let retry = persisted.into_iter().chain(current.retry_at.clone()).map(|value| {
            chrono::DateTime::parse_from_rfc3339(&value).map(|time| (time.with_timezone(&Utc), value)).map_err(|_| Error::invalid("Stored collection retry time is invalid; repair it before reconnecting"))
        }).collect::<Result<Vec<_>>>()?.into_iter().max_by_key(|(time,_)| *time);
        if let Some((time, value)) = retry.filter(|(time, _)| *time > Utc::now()) {
            let _ = time;
            if current.retry_at.as_ref() != Some(&value) {
                self.set_connection(
                    provider,
                    &current.status,
                    &current.detail,
                    current.account_label,
                    Some(value),
                )?;
            }
            return Ok(false);
        }
        if current.retry_at.is_some() {
            self.set_connection(
                provider,
                &current.status,
                current.detail,
                current.account_label,
                None,
            )?;
        }
        Ok(true)
    }

    async fn provider_active(&self, provider: &str) -> Result<bool> {
        let key = format!("collection:{provider}:active");
        self.block(move |store| {
            Ok(store
                .connect()?
                .query_row("SELECT value FROM metadata WHERE key=?1", [key], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
                .is_some_and(|value| value == "true"))
        })
        .await
    }

    pub fn start(self: &Arc<Self>) {
        for provider in ["x", "telegram"] {
            let runtime = self.clone();
            tokio::spawn(async move {
                loop {
                    if runtime.store.shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    let outcome = AssertUnwindSafe(runtime.scheduler_tick(provider))
                        .catch_unwind()
                        .await
                        .unwrap_or_else(|_| Err(Error::internal()));
                    if let Err(error) = outcome
                        && error.code != "cancelled"
                        && error.code != "capacity_exceeded"
                    {
                        let current = runtime.connection(provider).ok();
                        let retry = current
                            .as_ref()
                            .and_then(|connection| connection.retry_at.clone())
                            .filter(|value| value > &now())
                            .unwrap_or_else(|| later(60));
                        let persisted = runtime.persist_retry(provider, retry.clone()).await;
                        let detail = if persisted.is_err() {
                            "Collection failed and its retry time could not be saved. Last committed evidence is retained.".into()
                        } else {
                            error.message
                        };
                        let _ = runtime.set_connection(
                            provider,
                            "error",
                            detail,
                            current.and_then(|connection| connection.account_label),
                            Some(retry),
                        );
                    }
                    tokio::select! { _=tokio::time::sleep(Duration::from_secs(2))=>{}, _=runtime.wake.notified()=>{} }
                }
            });
        }
    }

    async fn scheduler_tick(self: &Arc<Self>, provider: &str) -> Result<()> {
        if !self.retry_gate(provider).await? {
            return Ok(());
        }
        let connection = self.connection(provider)?;
        if ["disconnected", "error"].contains(&connection.status.as_str())
            && self.provider_active(provider).await?
        {
            self.connect(provider).await?;
        }
        self.sweep(provider).await
    }

    // Source lookup/content failures are already durably recorded on that
    // subscription. They must not disconnect an otherwise usable account.
    // The status says whether `due` can fire again without the operator.
    async fn record_attempt_failure(
        &self,
        sub: &Value,
        mut state: Value,
        error: &Error,
        cancelled: bool,
        run_id: &str,
    ) -> Result<()> {
        state["requested_action"] = Value::Null;
        // Whether the scheduler can ever pick this state up again by itself.
        let resumes = |state: &Value| due(sub, Some(state), "9999-12-31T23:59:59.999Z");
        let hint = if state["continuation"].is_object() {
            "It does not retry by itself: Continue the saved interval to try again."
        } else {
            "It does not retry by itself: Check now to try again."
        };
        // Shutdown cancels every run; only an operator Stop holds the source,
        // including one whose own save lost a race with this attempt.
        let operator_stop = state["stopped"] == true
            || self.stopped_runs.lock().map_err(|_| Error::internal())?.contains(run_id);
        if cancelled && self.store.shutdown.load(Ordering::Acquire) && !operator_stop {
            state["status"] = json!(if state["continuation"].is_object() {
                "catching_up"
            } else {
                "waiting"
            });
            state["detail"] = json!(
                "Interrupted when Relic stopped. Retained pages and the last checkpoint are kept; collection resumes on the next start."
            );
            state["last_error_code"] = Value::Null;
            state["next_due_at"] = json!(now());
            if !resumes(&state) {
                state["status"] = json!("needs_attention");
                state["detail"] = json!(format!(
                    "Interrupted when Relic stopped. Retained pages and the last checkpoint are kept. {hint}"
                ));
                state["next_due_at"] = Value::Null;
            }
            self.save_state(sub, state).await?;
            return Ok(());
        }
        let blocked = error.code == "source_unavailable";
        let transient = error.code == "source_transient";
        let failures = state["transient_failures"].as_u64().unwrap_or(0) + 1;
        let step = (failures as usize - 1).min(TRANSIENT_WAIT_MINUTES.len() - 1);
        let wait = TRANSIENT_WAIT_MINUTES[step];
        state["status"] = json!(if cancelled {
            "paused"
        } else if blocked {
            "blocked"
        } else {
            "needs_attention"
        });
        state["last_error_code"] = json!(error.code);
        state["next_due_at"] = if cancelled || blocked {
            Value::Null
        } else if transient {
            json!(later(wait * 60))
        } else {
            json!(later(60))
        };
        if transient && !cancelled {
            state["transient_failures"] = json!(failures);
            // When the consecutive errors began, so a fault that outlasts the
            // backoff is visible. Cleared by the next retained page.
            if failures == 1 || !state["transient_since"].is_string() {
                state["transient_since"] = json!(now());
            }
        }
        if cancelled {
            state["stopped"] = json!(true);
        }
        // Judged with the new detail in place: legacy details feed `auto_continue`.
        let retries = !cancelled && !blocked && {
            let mut saved = state.clone();
            saved["detail"] = json!(error.message);
            resumes(&saved)
        };
        state["detail"] = json!(if cancelled || blocked {
            error.message.clone()
        } else if !retries {
            format!("{} {hint}", error.message)
        } else if transient && step == TRANSIENT_WAIT_MINUTES.len() - 1 {
            let provider = if sub["provider"] == "telegram" { "Telegram" } else { "X" };
            let since = state["transient_since"].as_str().and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok()).map(|at| at.with_timezone(&Utc));
            match since {
                Some(at) => format!(
                    "{} {provider} has returned errors for {}, since {}. Retrying automatically every {wait} minutes.",
                    error.message,
                    elapsed(at),
                    at.format("%Y-%m-%d %H:%M UTC")
                ),
                None => format!("{} Retrying automatically every {wait} minutes.", error.message),
            }
        } else if transient {
            format!("{} Retrying automatically in {wait} minutes.", error.message)
        } else {
            error.message.clone()
        });
        if !cancelled && !blocked && !retries {
            state["next_due_at"] = Value::Null;
        }
        self.save_state(sub, state).await?;
        Ok(())
    }

    async fn drain_telegram_pass(
        &self,
        configs: &[Value],
        count: &mut usize,
        deadline: Instant,
    ) -> Result<()> {
        while *count < 64
            && Instant::now() < deadline
            && !self.store.shutdown.load(Ordering::Acquire)
        {
            if !self.drain_telegram_update(configs).await? {
                break;
            }
            *count += 1;
        }
        Ok(())
    }

    async fn sweep(self: &Arc<Self>, provider: &str) -> Result<()> {
        if !self.retry_gate(provider).await? || !self.connected(provider) {
            return Ok(());
        }
        let _permit = match self.operations[index(provider)?].try_acquire() {
            Ok(permit) => permit,
            Err(_) => return Ok(()),
        };
        let configs = self.configurations().await?;
        let mut drained = 0;
        let drain_deadline = Instant::now() + Duration::from_secs(2);
        if provider == "telegram" {
            self.drain_telegram_pass(&configs, &mut drained, drain_deadline)
                .await?;
        }
        if !self.retry_gate(provider).await? || !self.connected(provider) {
            return Ok(());
        }
        let states = self.block(|store| store.collection_states()).await?;
        let by_id = states
            .into_iter()
            .map(|state| (text(&state, "subscription_id"), state))
            .collect::<HashMap<_, _>>();
        let at = now();
        let mut candidates = configs
            .iter()
            .filter(|sub| sub["provider"] == provider)
            .filter(|sub| due(sub, by_id.get(&text(sub, "id")), &at))
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by_key(|sub| {
            by_id
                .get(&text(sub, "id"))
                .map(|state| text(state, "next_due_at"))
                .unwrap_or_default()
        });
        if let Some(sub) = candidates.into_iter().next()
            && let Err(error) = self.run_subscription(sub).await
            && error.code != "cancelled"
        {
            return Err(error);
        }
        if provider == "telegram" && self.connected(provider) && self.retry_gate(provider).await? {
            self.drain_telegram_pass(&configs, &mut drained, drain_deadline)
                .await?;
        }
        Ok(())
    }

    async fn run_subscription(self: &Arc<Self>, sub: Value) -> Result<()> {
        let id = text(&sub, "id");
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut active = self.active.lock().await;
            if active.contains_key(&id) {
                return Err(Error::busy());
            }
            active.insert(id.clone(), cancel.clone());
        }
        let run_id = ident("collection");
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        // All recoverable errors and unwinds share cleanup, including the initial job save.
        let outcome = AssertUnwindSafe(async {
            self.store
                .running
                .lock()
                .map_err(|_| Error::internal())?
                .insert(run_id.clone(), cancel.clone());
            self.execute_subscription(&sub, &run_id, cancel.clone())
                .await
        })
        .catch_unwind()
        .await
        .unwrap_or_else(|_| Err(Error::internal()));
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
        let mut active = self.active.lock().await;
        if active
            .get(&id)
            .is_some_and(|registered| Arc::ptr_eq(registered, &cancel))
        {
            active.remove(&id);
        }
        drop(active);
        // Any Stop of this attempt has been recorded by its finish path.
        if let Ok(mut stopped) = self.stopped_runs.lock() {
            stopped.remove(&run_id);
        }
        match self.store.running.lock() {
            Ok(mut running) => {
                running.remove(&run_id);
            }
            Err(poisoned) => {
                poisoned.into_inner().remove(&run_id);
            }
        }
        outcome
    }

    async fn execute_subscription(
        &self,
        sub: &Value,
        run_id: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        let provider = text(sub, "provider");
        let job = json!({"id":run_id,"kind":format!("{provider}_collection"),"status":"running","accepted":0,"total":0,"created_at":now(),"detail":format!("Collecting {}. Each page is retained before its checkpoint advances.",text(sub,"label"))});
        let initial = job.clone();
        self.block(move |store| store.save_job(&initial)).await?;
        let outcome = self.attempt(sub, run_id, cancel.clone()).await;
        if !self
            .finish_subscription(sub, job, outcome, cancel.load(Ordering::Acquire))
            .await?
        {
            return Ok(());
        }
        if provider == "x"
            && self.x_rotate
            && self.connected("x")
            && self.retry_gate("x").await?
            && let Some(x) = &self.x
            && let Err(error) = x.rotate().await
        {
            self.set_connection(
                "x",
                "error",
                format!("Proxy rotation failed. {error}"),
                None,
                None,
            )?;
        }
        Ok(())
    }

    async fn finish_subscription(
        &self,
        sub: &Value,
        mut job: Value,
        outcome: Result<Value>,
        cancelled: bool,
    ) -> Result<bool> {
        let id = text(sub, "id");
        let run_id = text(&job, "id");
        let mut deferred_error = None;
        let mut isolated_failure = false;
        match outcome {
            Ok(state) => {
                job["status"] = json!(if state["continuation"].is_object() {
                    "partial"
                } else {
                    "completed"
                });
                job["detail"] = state["detail"].clone();
                job["accepted"] = json!(state["attempt_accepted"].as_u64().unwrap_or(0));
                job["total"] = json!(state["attempt_records"].as_u64().unwrap_or(0));
            }
            Err(error) => {
                let cancelled = error.code == "cancelled" || cancelled;
                job["status"] = json!(if cancelled { "cancelled" } else { "failed" });
                job["detail"] = json!(if cancelled {
                    "Collection stopped. Retained evidence and the last durable checkpoint remain available.".to_owned()
                } else {
                    error.message.clone()
                });
                let error = if cancelled { Error::cancelled() } else { error };
                let source_failure =
                    matches!(error.code, "source_unavailable" | "source_transient");
                isolated_failure = source_failure;
                let read_id = id.clone();
                match self
                    .block(move |store| store.collection_state(&read_id))
                    .await
                {
                    Ok(state) if state["configuration_version"] == sub["version"] => {
                        job["accepted"] = json!(state["attempt_accepted"].as_u64().unwrap_or(0));
                        job["total"] = json!(state["attempt_records"].as_u64().unwrap_or(0));
                        if let Err(error) = self
                            .record_attempt_failure(sub, state, &error, cancelled, &run_id)
                            .await
                        {
                            deferred_error = Some(error);
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        deferred_error = Some(error);
                    }
                }
                if !source_failure && deferred_error.is_none() {
                    deferred_error = Some(error);
                }
            }
        }
        // Final job status is attempted even if persisting recovery state failed.
        self.block(move |store| {
            if job["status"] == "completed" && job["accepted"] == 0 {
                // Successful unchanged polling is summarized by collection state.
                // Keep any operation with provenance, errors or partial coverage.
                let _guard = store.writer.lock().map_err(|_| Error::internal())?;
                store.connect()?.execute("DELETE FROM jobs WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM lineage_nodes WHERE id=?1)", [job["id"].as_str()])?;
                Ok(())
            } else { store.save_job(&job) }
        }).await?;
        if let Some(error) = deferred_error {
            return Err(error);
        }
        Ok(!isolated_failure)
    }

    async fn attempt(&self, sub: &Value, run_id: &str, cancel: Arc<AtomicBool>) -> Result<Value> {
        let mut state = self.prepare_state(sub).await?;
        if repeated_cursor_retry(&state) {
            state["cursor_restart_pending"] = json!(true);
        }
        if auto_continue(&state) {
            state["catchup_auto_resume"] = json!(true);
        }
        state["run_id"] = json!(run_id);
        state["status"] = json!("running");
        state["detail"] = json!("Reading a bounded source interval…");
        state["attempt_records"] = json!(0);
        state["attempt_accepted"] = json!(0);
        state["last_error_code"] = Value::Null;
        initial_history::prepare(&mut state, &now())?;
        state = self.save_state(sub, state).await?;
        if sub["provider"] == "x" {
            self.cancellable(&cancel, self.collect_x(sub, state, cancel.clone()))
                .await
        } else {
            self.cancellable(&cancel, self.collect_telegram(sub, state, cancel.clone()))
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> (tempfile::TempDir, Arc<Runtime>, Value) {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::config::Config::local(
            std::env::current_dir().unwrap(),
            directory.path().join("data"),
            "127.0.0.1:8792".parse().unwrap(),
        );
        let store = Store::open(config).unwrap();
        let sub = store.create_subscription(json!({"provider":"x","label":"Synthetic scheduler source","target":"@synthetic_qa","enabled":true})).unwrap();
        (directory, Runtime::new(store), sub)
    }

    #[tokio::test]
    async fn stored_provider_retry_is_respected_after_runtime_restart() {
        let (_directory, runtime, _sub) = workspace();
        let retry = later(600);
        runtime
            .persist_retry("telegram", retry.clone())
            .await
            .unwrap();
        let restarted = Runtime::new(runtime.store.clone());
        assert!(!restarted.retry_gate("telegram").await.unwrap());
        assert_eq!(
            restarted
                .connection("telegram")
                .unwrap()
                .retry_at
                .as_deref(),
            Some(retry.as_str())
        );
        assert!(restarted.telegram.lock().await.is_none());
        runtime.persist_retry("telegram", later(-1)).await.unwrap();
        assert!(
            Runtime::new(runtime.store.clone())
                .retry_gate("telegram")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn unavailable_sources_do_not_disconnect_accounts_or_create_retry_deadlines() {
        for provider in ["telegram", "x"] {
            let (_directory, runtime, _) = workspace();
            let sub = runtime.store.create_subscription(json!({"provider":provider,"label":"Unavailable test source","target":"@missing_qa","enabled":true})).unwrap();
            let good = runtime.store.create_subscription(json!({"provider":provider,"label":"Usable test source","target":"@working_qa","enabled":true})).unwrap();
            runtime
                .set_connection(provider, "connected", "Connected", None, None)
                .unwrap();
            let mut state = runtime.prepare_state(&sub).await.unwrap();
            state["watermark"] = json!("123");
            let messages: Vec<_> = (112..124).map(|id| json!({"id":if provider == "x" {format!("x:post:{id}")} else {format!("telegram:100:-1000000000010:{id}")},"text":"Synthetic retained report","media":[],"relationships":{}})).collect();
            runtime.store.publish_collection_page(sub["id"].as_str().unwrap(),1,
                json!({"id":format!("{provider}:synthetic:100"),"label":"Synthetic source","coverage":"Test only"}),
                messages.clone(),json!({"synthetic":messages}),state,Arc::new(AtomicBool::new(false))).unwrap();
            let error = if provider == "telegram" {
                telegram::telegram_error(relic_telegram::Error::PeerUnavailable)
            } else {
                x::x_error(crate::sources::SourceFailure::new(
                    crate::sources::FailureKind::Unavailable,
                    "X handle did not resolve to a readable user",
                ))
            };
            let job = json!({"id":"collection:source-failure","kind":format!("{provider}_collection"),"status":"running","accepted":0,"total":0,"created_at":now(),"detail":"Synthetic source failure"});
            assert!(
                !runtime
                    .finish_subscription(&sub, job, Err(error), false)
                    .await
                    .unwrap()
            );
            let state = runtime
                .store
                .collection_state(sub["id"].as_str().unwrap())
                .unwrap();
            assert_eq!(state["status"], "blocked");
            assert_eq!(state["watermark"], "123");
            assert_eq!(state["messages_acquired"], 12);
            assert!(state["next_due_at"].is_null());
            assert!(!due(&sub, Some(&state), "2030-01-01T00:00:00Z"));
            assert!(due(&good, None, "2030-01-01T00:00:00Z"));
            assert!(runtime.connected(provider));
            assert!(runtime.retry_gate(provider).await.unwrap());
            assert!(runtime.connection(provider).unwrap().retry_at.is_none());
            let count: i64 = runtime
                .store
                .connect()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM metadata WHERE key LIKE 'collection:%:retry_at'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 0);
            let restarted = Runtime::new(runtime.store.clone());
            assert!(restarted.retry_gate(provider).await.unwrap());
            // Explicit Check now clears the source hold without discarding history.
            runtime
                .request(
                    sub["id"].as_str().unwrap().into(),
                    sub["version"].as_i64().unwrap(),
                    "poll",
                )
                .await
                .unwrap();
            let requested = runtime
                .store
                .collection_state(sub["id"].as_str().unwrap())
                .unwrap();
            assert!(due(&sub, Some(&requested), &now()));
            assert_eq!(requested["watermark"], "123");
        }
    }

    #[tokio::test]
    async fn actual_provider_limits_still_gate_reconnect_after_restart() {
        let (_directory, runtime, _) = workspace();
        runtime
            .x_failure(&crate::sources::SourceFailure::new(crate::sources::FailureKind::RateLimited, "HTTP 429"))
            .await
            .unwrap();
        runtime
            .telegram_failure(&relic_telegram::Error::FloodWait(120))
            .await
            .unwrap();
        for provider in ["x", "telegram"] {
            let restarted = Runtime::new(runtime.store.clone());
            assert!(!restarted.retry_gate(provider).await.unwrap());
            assert!(restarted.connection(provider).unwrap().retry_at.is_some());
            let error = restarted
                .connect(provider)
                .await
                .err()
                .expect("cooldown must reject reconnect");
            assert_eq!(error.code, "conflict");
            assert!(error.message.contains("UTC"));
            assert!(restarted.telegram.lock().await.is_none());
        }
    }

    #[test]
    fn correcting_a_blocked_source_reenables_its_next_attempt() {
        let sub = json!({"version":3,"enabled":true});
        let state = json!({"configuration_version":2,"status":"blocked","next_due_at":null});
        assert!(due(&sub, Some(&state), &now()));
    }

    #[tokio::test]
    async fn initial_and_final_job_write_failures_release_runtime_registration() {
        for trigger in [
            "CREATE TRIGGER fail_job BEFORE INSERT ON jobs BEGIN SELECT RAISE(ABORT,'synthetic initial job failure'); END;",
            "CREATE TRIGGER fail_job BEFORE INSERT ON jobs WHEN json_extract(NEW.json,'$.status')='failed' BEGIN SELECT RAISE(ABORT,'synthetic final job failure'); END;",
        ] {
            let (_directory, runtime, sub) = workspace();
            runtime
                .store
                .connect()
                .unwrap()
                .execute_batch(trigger)
                .unwrap();
            let error = runtime.run_subscription(sub).await.unwrap_err();
            assert_eq!(error.code, "storage_failure");
            assert!(runtime.active.lock().await.is_empty());
            assert!(runtime.store.running.lock().unwrap().is_empty());
            assert_eq!(runtime.in_flight.load(Ordering::Relaxed), 0);
        }
    }

    #[tokio::test]
    async fn cancelled_attempt_persists_a_stop_instead_of_a_timed_restart() {
        let (_directory, runtime, sub) = workspace();
        let result = runtime
            .execute_subscription(
                &sub,
                "collection:synthetic-stop",
                Arc::new(AtomicBool::new(true)),
            )
            .await;
        assert_eq!(result.unwrap_err().code, "cancelled");
        let state = runtime
            .store
            .collection_state(sub["id"].as_str().unwrap())
            .unwrap();
        assert_eq!(state["stopped"], true);
        assert_eq!(state["status"], "paused");
        assert!(state["next_due_at"].is_null());
        assert!(!due(&sub, Some(&state), "2030-01-01T00:00:00Z"));
    }

    #[tokio::test]
    async fn shutdown_interrupts_a_run_without_stopping_its_source() {
        let (_directory, runtime, sub) = workspace();
        runtime.store.shutdown.store(true, Ordering::Release);
        let result = runtime
            .execute_subscription(
                &sub,
                "collection:synthetic-shutdown",
                Arc::new(AtomicBool::new(true)),
            )
            .await;
        assert_eq!(result.unwrap_err().code, "cancelled");
        let id = sub["id"].as_str().unwrap();
        let state = runtime.store.collection_state(id).unwrap();
        assert_ne!(state["stopped"], true);
        assert_eq!(state["status"], "waiting");
        assert!(state["last_error_code"].is_null());
        assert!(due(&sub, Some(&state), &now()));
        // An operator Stop saved before the shutdown still holds.
        let mut stopped = state;
        stopped["stopped"] = json!(true);
        runtime
            .store
            .save_collection_state(id, sub["version"].as_i64().unwrap(), stopped)
            .unwrap();
        let _ = runtime
            .execute_subscription(
                &sub,
                "collection:synthetic-stopped-shutdown",
                Arc::new(AtomicBool::new(true)),
            )
            .await;
        let state = runtime.store.collection_state(id).unwrap();
        assert_eq!(state["stopped"], true);
        assert_eq!(state["status"], "paused");
        assert!(!due(&sub, Some(&state), "2030-01-01T00:00:00Z"));
    }

    #[tokio::test]
    async fn temporary_x_errors_retry_with_backoff_instead_of_blocking() {
        let (_directory, runtime, sub) = workspace();
        runtime
            .set_connection("x", "connected", "Connected", None, None)
            .unwrap();
        runtime.prepare_state(&sub).await.unwrap();
        let id = sub["id"].as_str().unwrap();
        let mut waits = Vec::new();
        for _ in 0..6 {
            let before = Utc::now();
            let error = x::x_error(crate::sources::SourceFailure::new(crate::sources::FailureKind::Transient, 
                "code 0: Internal error",
            ));
            let job = json!({"id":"collection:transient","kind":"x_collection","status":"running","accepted":0,"total":0,"created_at":now(),"detail":"Synthetic transient failure"});
            assert!(
                !runtime
                    .finish_subscription(&sub, job, Err(error), false)
                    .await
                    .unwrap()
            );
            let state = runtime.store.collection_state(id).unwrap();
            assert_eq!(state["status"], "needs_attention");
            assert_eq!(state["last_error_code"], "source_transient");
            assert!(state["detail"].as_str().unwrap().contains("Internal error"));
            let next = chrono::DateTime::parse_from_rfc3339(state["next_due_at"].as_str().unwrap())
                .unwrap()
                .with_timezone(&Utc);
            waits.push(((next - before).num_seconds() + 30) / 60);
            assert!(!due(&sub, Some(&state), &now()));
            assert!(due(&sub, Some(&state), &later(2 * 3600)));
        }
        assert_eq!(waits, [2, 5, 15, 30, 60, 60]);
        assert!(runtime.connected("x"));
        assert!(runtime.connection("x").unwrap().retry_at.is_none());
    }

    /// A transient failure on the shared fictional source; returns its state.
    async fn fail_transiently(runtime: &Arc<Runtime>, sub: &Value) -> Value {
        let error = x::x_error(crate::sources::SourceFailure::new(crate::sources::FailureKind::Transient, "code 0: Internal error"));
        let job = json!({"id":"collection:transient","kind":"x_collection","status":"running","accepted":0,"total":0,"created_at":now(),"detail":"Synthetic transient failure"});
        assert!(!runtime.finish_subscription(sub, job, Err(error), false).await.unwrap());
        runtime.store.collection_state(sub["id"].as_str().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn a_persistent_x_fault_says_how_long_it_has_lasted() {
        let (_directory, runtime, sub) = workspace();
        runtime.prepare_state(&sub).await.unwrap();
        let first = fail_transiently(&runtime, &sub).await;
        let since = first["transient_since"].as_str().expect("first failure time is kept").to_owned();
        assert!(!first["detail"].as_str().unwrap().contains("since"));
        for failure in 2..=6 {
            let state = fail_transiently(&runtime, &sub).await;
            assert_eq!(state["transient_since"], since, "the first consecutive failure stays");
            let detail = state["detail"].as_str().unwrap();
            // Once the wait reaches its last step, the detail says for how long.
            assert_eq!(detail.contains("X has returned errors"), failure >= 5, "{failure}: {detail}");
            assert!(detail.contains("Retrying automatically"), "{detail}");
            if failure >= 5 {
                let time = chrono::DateTime::parse_from_rfc3339(&since).unwrap().with_timezone(&Utc);
                assert!(detail.contains(&time.format("%Y-%m-%d %H:%M UTC").to_string()), "{detail}");
            }
            assert!(due(&sub, Some(&state), &later(2 * 3600)));
        }
    }

    /// A saved catch-up interval with `catchup_auto_resume` false (the 30-page
    /// cursor bound, a repeated cursor) and a retained page.
    fn held_interval(runtime: &Arc<Runtime>, sub: &Value, state: Value, auto: bool) {
        let mut state = state;
        state["status"] = json!("needs_attention");
        state["continuation"] = json!({"cursor":"next","highest":"130","original_watermark":"100","seen_cursors":["a"],"pages":30});
        state["catchup_auto_resume"] = json!(auto);
        state["initial_history"] = json!({"complete":true});
        let messages: Vec<_> = (120..122).map(|id| json!({"id":format!("x:post:{id}"),"text":"Synthetic retained report","media":[],"relationships":{}})).collect();
        runtime.store.publish_collection_page(sub["id"].as_str().unwrap(), sub["version"].as_i64().unwrap(),
            json!({"id":"x:synthetic:held","label":"Synthetic source","coverage":"Test only"}),
            messages.clone(), json!({"synthetic":messages}), state, Arc::new(AtomicBool::new(false))).unwrap();
    }

    #[tokio::test]
    async fn a_held_interval_never_promises_a_retry_it_will_not_make() {
        let (_directory, runtime, sub) = workspace();
        let state = runtime.prepare_state(&sub).await.unwrap();
        held_interval(&runtime, &sub, state, false);
        let state = fail_transiently(&runtime, &sub).await;
        let detail = state["detail"].as_str().unwrap();
        assert!(!due(&sub, Some(&state), "9999-12-31T23:59:59Z"), "nothing fires by itself");
        assert!(!detail.contains("Retrying automatically"), "{detail}");
        assert!(detail.contains("Continue"), "{detail}");
        assert_eq!(state["status"], "needs_attention");
        assert!(state["next_due_at"].is_null());
        assert_eq!(state["continuation"]["cursor"], "next", "the saved interval is kept");
        // An interval that does continue by itself keeps its timed retry.
        let (_directory, runtime, sub) = workspace();
        let state = runtime.prepare_state(&sub).await.unwrap();
        held_interval(&runtime, &sub, state, true);
        let state = fail_transiently(&runtime, &sub).await;
        assert!(state["detail"].as_str().unwrap().contains("Retrying automatically in 2 minutes"));
        assert!(!due(&sub, Some(&state), &now()));
        assert!(due(&sub, Some(&state), &later(600)));
    }

    #[tokio::test]
    async fn shutdown_promises_a_resume_only_when_one_will_happen() {
        let (_directory, runtime, sub) = workspace();
        let state = runtime.prepare_state(&sub).await.unwrap();
        held_interval(&runtime, &sub, state, false);
        runtime.store.shutdown.store(true, Ordering::Release);
        let _ = runtime
            .execute_subscription(&sub, "collection:held-shutdown", Arc::new(AtomicBool::new(true)))
            .await;
        let state = runtime.store.collection_state(sub["id"].as_str().unwrap()).unwrap();
        let detail = state["detail"].as_str().unwrap();
        assert!(!due(&sub, Some(&state), "9999-12-31T23:59:59Z"));
        assert_ne!(state["status"], "catching_up");
        assert!(!detail.contains("resumes on the next start"), "{detail}");
        assert!(detail.contains("Continue"), "{detail}");
        assert_ne!(state["stopped"], true, "shutdown is not an operator Stop");
    }

    #[tokio::test]
    async fn an_operator_stop_survives_a_shutdown_that_beats_its_save() {
        let (_directory, runtime, sub) = workspace();
        let id = sub["id"].as_str().unwrap().to_owned();
        let mut state = runtime.prepare_state(&sub).await.unwrap();
        state["run_id"] = json!("collection:stopped-run");
        runtime.store.save_collection_state(&id, sub["version"].as_i64().unwrap(), state).unwrap();
        // Stop signals the running attempt and records the request in memory.
        let cancel = Arc::new(AtomicBool::new(false));
        runtime.active.lock().await.insert(id.clone(), cancel.clone());
        runtime.stop_job("collection:stopped-run").await.unwrap();
        assert!(cancel.load(Ordering::Acquire));
        assert!(runtime.stopped_runs.lock().unwrap().contains("collection:stopped-run"));
        runtime.active.lock().await.clear();
        // Its own save lost a race with a page: the saved state is not stopped.
        let mut lost = runtime.store.collection_state(&id).unwrap();
        lost["stopped"] = json!(false);
        lost["status"] = json!("running");
        runtime.store.save_collection_state(&id, sub["version"].as_i64().unwrap(), lost).unwrap();
        // Shutdown then interrupts the attempt before it records the Stop.
        runtime.store.shutdown.store(true, Ordering::Release);
        let job = json!({"id":"collection:stopped-run","kind":"x_collection","status":"running","accepted":0,"total":0,"created_at":now(),"detail":"Synthetic stopped run"});
        let _ = runtime.finish_subscription(&sub, job, Err(Error::cancelled()), true).await;
        let state = runtime.store.collection_state(&id).unwrap();
        assert_eq!(state["stopped"], true);
        assert_eq!(state["status"], "paused");
        assert!(!due(&sub, Some(&state), "2030-01-01T00:00:00Z"));
    }
    #[test]
    fn stopped_source_stays_paused_until_explicit_request_or_configuration_change() {
        let sub = json!({"id":"source","version":2,"enabled":true});
        let mut state =
            json!({"configuration_version":2,"stopped":true,"next_due_at":"2020-01-01T00:00:00Z"});
        assert!(!due(&sub, Some(&state), "2026-09-10T00:00:00Z"));
        state["requested_action"] = json!("poll");
        assert!(due(&sub, Some(&state), "2026-09-10T00:00:00Z"));
        state["requested_action"] = Value::Null;
        state["configuration_version"] = json!(1);
        assert!(due(&sub, Some(&state), "2026-09-10T00:00:00Z"));
    }
    #[test]
    fn initial_history_resumes_but_respects_stop_backoff_and_safety_limit() {
        let sub = json!({"version":2,"enabled":true});
        let at = "2026-09-20T00:00:00Z";
        let base = json!({"configuration_version":2,"status":"catching_up","continuation":{"cursor":"saved"},"initial_history":{"complete":false,"pages":3}});
        assert!(due(&sub, Some(&base), at));
        for (key, val) in [
            ("stopped", json!(true)),
            ("status", json!("blocked")),
            ("next_due_at", json!("2026-09-21T00:00:00Z")),
        ] {
            let mut state = base.clone();
            state[key] = val;
            assert!(!due(&sub, Some(&state), at), "{key}");
        }
        let mut state = base.clone();
        state["initial_history"]["pages"] = json!(30);
        assert!(!due(&sub, Some(&state), at));
        state = base;
        state["status"] = json!("needs_attention");
        state["last_error_code"] = json!("transport_failure");
        assert!(due(&sub, Some(&state), at));
        state["last_error_code"] = json!("source_unavailable");
        assert!(!due(&sub, Some(&state), at));
        assert!(!due(
            &json!({"version":2,"enabled":false}),
            Some(&state),
            at
        ));
    }

    #[test]
    fn normal_catchup_resumes_after_restart_and_retry_without_ignoring_holds() {
        let sub = json!({"version":2,"enabled":true});
        let at = "2026-09-20T00:00:00Z";
        let base = json!({"configuration_version":2,"status":"catching_up","catchup_auto_resume":true,"continuation":{"cursor":"next"},"initial_history":{"complete":true}});
        assert!(due(&sub, Some(&base), at));
        let restarted: Value = serde_json::from_str(&base.to_string()).unwrap();
        assert!(due(&sub, Some(&restarted), at));
        for (key, val) in [
            ("stopped", json!(true)),
            ("status", json!("blocked")),
            ("next_due_at", json!("2026-09-21T00:00:00Z")),
            ("catchup_auto_resume", json!(false)),
            ("last_error_code", json!("conflict")),
            ("last_error_code", json!("source_unavailable")),
        ] {
            let mut state = base.clone();
            state[key] = val;
            assert!(!due(&sub, Some(&state), at), "{key}");
        }
        for provider_error in ["x_unavailable", "telegram_unavailable", "source_transient"] {
            let mut state = base.clone();
            state["status"] = json!("needs_attention");
            state["last_error_code"] = json!(provider_error);
            assert!(due(&sub, Some(&state), at));
        }
        assert!(!due(&json!({"version":2,"enabled":false}), Some(&base), at));
    }

    #[test]
    fn legacy_page_pauses_resume_but_repeated_cursors_and_unknown_states_do_not() {
        let sub = json!({"version":2,"enabled":true});
        let at = "2026-09-20T00:00:00Z";
        let mut state = json!({"provider":"x","configuration_version":2,"status":"needs_attention","detail":"Catch-up is incomplete. Acquired pages are retained; continue the saved interval.","continuation":{"cursor":"next","seen_cursors":["old"]}});
        assert!(due(&sub, Some(&state), at));
        state["continuation"]["cursor"] = json!("old");
        assert!(!due(&sub, Some(&state), at));
        state["continuation"]["cursor"] = json!("next");
        state["detail"] = json!("X repeated a cursor.");
        assert!(!due(&sub, Some(&state), at));
        state["provider"] = json!("telegram");
        state["detail"] = json!(
            "Telegram catch-up is incomplete. Continue the saved interval; the original watermark is unchanged."
        );
        state["continuation"] = json!({"before":110,"original_watermark":"100"});
        assert!(due(&sub, Some(&state), at));
        state["continuation"]["before"] = json!(100);
        assert!(!due(&sub, Some(&state), at));
    }

    #[test]
    fn incomplete_interval_needs_explicit_continuation_and_stale_requests_do_not_enable_a_source() {
        let sub = json!({"id":"source","version":2,"enabled":true});
        let mut state = json!({"configuration_version":2,"continuation":{"cursor":"saved"}});
        assert!(!due(&sub, Some(&state), "2026-09-10T00:00:00Z"));
        state["requested_action"] = json!("continue");
        assert!(due(&sub, Some(&state), "2026-09-10T00:00:00Z"));
        let disabled = json!({"id":"source","version":3,"enabled":false});
        assert!(!due(&disabled, Some(&state), "2026-09-10T00:00:00Z"));
    }
}
