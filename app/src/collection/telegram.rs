use super::*;
use relic_telegram::{AuthState, Message, MessageEvent, Status};

pub(super) fn telegram_error(error: relic_telegram::Error) -> Error {
    Error {
        status: axum::http::StatusCode::BAD_GATEWAY,
        code: if matches!(
            error,
            relic_telegram::Error::PeerUnavailable
                | relic_telegram::Error::InvalidTarget
                | relic_telegram::Error::UnsupportedPeer
                | relic_telegram::Error::AccessDenied
        ) {
            "source_unavailable"
        } else {
            "telegram_unavailable"
        },
        message: if error == relic_telegram::Error::PeerUnavailable {
            "This Telegram channel could not be found or accessed. Check its handle and account access.".into()
        } else {
            error.to_string()
        },
    }
}
pub(super) fn reset_connection(error: &relic_telegram::Error) -> bool {
    matches!(
        error,
        relic_telegram::Error::Network
            | relic_telegram::Error::UpdateOverflow
            | relic_telegram::Error::Timeout
            | relic_telegram::Error::AuthenticationRequired
            | relic_telegram::Error::SessionUnavailable
            | relic_telegram::Error::SessionCorrupt
            | relic_telegram::Error::FloodWait(_)
    )
}
fn tombstone(account: &str, peer: &str, id: &str, observed: &str) -> Value {
    json!({"id":format!("telegram:{account}:{peer}:{id}"),"text":"","upstream_state":"removed","upstream_observed_at":observed,"relationships":{"account_id":account,"peer_id":peer,"message_id":id},"media":[]})
}
pub(super) fn normalized_message(message: &Message) -> Value {
    let origin = message
        .forward
        .as_ref()
        .and_then(|f| f.peer_id.as_ref().zip(f.message_id.as_ref()))
        .map(|(peer, id)| format!("telegram:{}:{peer}:{id}", message.account_id));
    json!({"id":format!("telegram:{}:{}:{}",message.account_id,message.peer_id,message.message_id),"text":message.text,"published_at":message.published_at,"source_edited_at":message.edited_at,"upstream_state":"present","language":"und","geometry":message.point.as_ref().map(|p|json!({"type":"Point","coordinates":[p.longitude,p.latitude]})),"location_precision":if message.point.is_some(){"source supplied"}else{"unresolved"},"origin_group":origin,"dependence":if message.forward.is_some(){"forward"}else{"unknown"},"relationships":{"account_id":message.account_id,"peer_id":message.peer_id,"message_id":message.message_id,"sender_id":message.sender_id,"reply":message.reply,"forward":message.forward,"grouped_id":message.grouped_id},"media":message.media.as_ref().map(|m|vec![serde_json::to_value(m).unwrap_or(Value::Null)]).unwrap_or_default()})
}

impl Runtime {
    async fn open_telegram(&self) -> Result<TelegramClient> {
        let values = &self.store.config.source_values;
        let api_id = values
            .get("TELEGRAM_API_ID")
            .and_then(|s| s.parse::<i32>().ok())
            .filter(|id| *id > 0)
            .ok_or_else(|| Error::invalid("Telegram application ID is not configured"))?;
        let api_hash = values
            .get("TELEGRAM_API_HASH")
            .filter(|s| !s.is_empty())
            .cloned()
            .ok_or_else(|| Error::invalid("Telegram application hash is not configured"))?;
        let name = values
            .get("TELEGRAM_SESSION")
            .map(String::as_str)
            .unwrap_or("relic");
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(Error::invalid(
                "TELEGRAM_SESSION must be a simple private session name",
            ));
        }
        TelegramClient::open(relic_telegram::Config {
            api_id,
            api_hash: api_hash.into(),
            session_path: self
                .store
                .config
                .session_dir
                .join(format!("{name}.session.json")),
            request_timeout: Duration::from_secs(20),
        })
        .await
        .map_err(telegram_error)
    }
    fn telegram_status(&self, status: Status) -> Result<Connection> {
        let (state, detail) = match status.state {
            AuthState::Connected => (
                "connected",
                "Telegram connected. Channel updates are received where account access permits; periodic history and revision checks cover interruptions.",
            ),
            AuthState::CodeRequired => (
                "awaiting_code",
                "Enter the login code delivered by Telegram.",
            ),
            AuthState::PasswordRequired => (
                "awaiting_password",
                "Enter your Telegram two-step verification password.",
            ),
            AuthState::Disconnected => (
                "awaiting_phone",
                "Sign in to create Relic's private Telegram session. No existing external session is imported.",
            ),
        };
        self.set_connection(
            "telegram",
            state,
            detail,
            status
                .account_id
                .map(|_| "Authorized Telegram account".into()),
            None,
        )
    }
    pub(super) async fn connect_telegram(&self) -> Result<Connection> {
        let mut client = self.telegram.lock().await;
        if client.is_none() {
            *client = Some(self.open_telegram().await?);
        }
        let client = client.as_mut().ok_or_else(Error::internal)?;
        client.disconnect();
        match client.connect().await {
            Ok(status) => self.telegram_status(status),
            Err(relic_telegram::Error::AuthenticationRequired) => {
                client.disconnect();
                self.telegram_status(client.status())
            }
            Err(error) => {
                if reset_connection(&error) {
                    client.disconnect();
                }
                self.telegram_failure(&error).await?;
                Err(telegram_error(error))
            }
        }
    }

    pub async fn telegram_auth(self: &Arc<Self>, input: Value) -> Result<Connection> {
        self.block(|s| s.assert_real_collection_allowed()).await?;
        let _permit = self.operations[1]
            .try_acquire()
            .map_err(|_| Error::busy())?;
        if !self.retry_gate("telegram").await? {
            return Err(self.retry_error("telegram")?);
        }
        let object = input
            .as_object()
            .filter(|o| o.len() == 1)
            .ok_or_else(|| Error::invalid("Supply exactly one Telegram login field"))?;
        let (field, value) = object.iter().next().ok_or_else(Error::internal)?;
        let value = value
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 1024)
            .ok_or_else(|| Error::invalid("Invalid Telegram login field"))?;
        let connection = self.connection("telegram")?;
        let permitted = match field.as_str() {
            "phone" => connection.status == "awaiting_phone",
            "code" => connection.status == "awaiting_code",
            "password" => connection.status == "awaiting_password",
            _ => false,
        };
        if !permitted {
            return Err(Error::conflict(
                "Telegram login state changed. Reopen the connection panel.",
            ));
        }
        let epoch = self.telegram_epoch.load(Ordering::Acquire);
        let mut guard = self.telegram.lock().await;
        let client = guard
            .as_mut()
            .ok_or_else(|| Error::conflict("Connect Telegram before signing in"))?;
        let result = {
            let operation = async {
                match field.as_str() {
                    "phone" => client.request_code(value).await,
                    "code" => client.submit_code(value).await,
                    "password" => client.submit_password(value).await,
                    _ => unreachable!(),
                }
            };
            self.telegram_auth_until_epoch(epoch, operation).await
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                client.disconnect();
                return Err(error);
            }
        };
        if self.telegram_epoch.load(Ordering::Acquire) != epoch {
            client.disconnect();
            return Err(Error::cancelled());
        }
        match result {
            Ok(status) => {
                drop(guard);
                let result = self.telegram_status(status)?;
                if result.status == "connected" {
                    self.activation_if_epoch("telegram", epoch).await?;
                }
                Ok(result)
            }
            Err(error) => {
                if reset_connection(&error) {
                    client.disconnect();
                }
                drop(guard);
                self.telegram_failure(&error).await?;
                Err(telegram_error(error))
            }
        }
    }
    async fn telegram_auth_until_epoch<T>(
        &self,
        epoch: u64,
        operation: impl Future<Output = T>,
    ) -> Result<T> {
        tokio::pin!(operation);
        loop {
            tokio::select! {
                result = &mut operation => return Ok(result),
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if self.telegram_epoch.load(Ordering::Acquire) != epoch || self.store.shutdown.load(Ordering::Acquire) {
                        return Err(Error::cancelled());
                    }
                }
            }
        }
    }
    pub(super) async fn telegram_failure(&self, error: &relic_telegram::Error) -> Result<()> {
        if reset_connection(error) {
            let delay = match error {
                relic_telegram::Error::FloodWait(seconds) => i64::from(*seconds),
                _ => 60,
            };
            let until = later(delay);
            self.persist_retry("telegram", until.clone()).await?;
            let old = self.connection("telegram")?;
            self.set_connection(
                "telegram",
                if matches!(error, relic_telegram::Error::AuthenticationRequired) {
                    "awaiting_phone"
                } else {
                    "error"
                },
                error.to_string(),
                old.account_label,
                Some(until),
            )?;
        }
        Ok(())
    }
    pub(super) async fn collect_telegram(
        &self,
        sub: &Value,
        mut state: Value,
        cancel: Arc<AtomicBool>,
    ) -> Result<Value> {
        let mut guard = self.telegram.lock().await;
        let client = guard
            .as_mut()
            .ok_or_else(|| Error::conflict("Telegram is not connected"))?;
        let account = client
            .status()
            .account_id
            .ok_or_else(|| Error::conflict("Telegram account authorization is required"))?;
        if state["account_key"].as_str().is_some_and(|a| a != account) {
            return Err(Error::conflict(
                "The saved Telegram interval belongs to a different account; review this source before resuming.",
            ));
        }
        let peer = if let Some(peer) = state["peer_id"].as_str() {
            peer.to_owned()
        } else {
            match client.resolve_channel(&text(sub, "target")).await {
                Ok(peer) => peer.peer_id,
                Err(error) => {
                    if reset_connection(&error) {
                        client.disconnect();
                    }
                    self.telegram_failure(&error).await?;
                    return Err(telegram_error(error));
                }
            }
        };
        state["peer_id"] = json!(peer);
        state["account_key"] = json!(account);
        state["bound_target"] = sub["target"].clone();
        state["provider"] = json!("telegram");
        let source = json!({"id":format!("telegram:{account}:{peer}"),"label":sub["label"],"family":"social_reporting","coverage":"Recent first baseline; account-visible updates plus two-minute history recovery and paced exact-ID reconciliation. Public channels not joined by this account may not deliver push updates.","detail":"MTProto account session. Missing access is not message deletion; media remains a reference until explicitly retained."});
        let after = state["watermark"]
            .as_str()
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);
        let baseline = initial_history::active(&state);
        let mut before = state["continuation"]["before"]
            .as_i64()
            .and_then(|v| i32::try_from(v).ok());
        let mut highest = state["continuation"]["highest"]
            .as_i64()
            .and_then(|v| i32::try_from(v).ok())
            .unwrap_or(after);
        for _ in 0..ATTEMPT_PAGES {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let page = match client
                .history(&peer, if baseline { 0 } else { after }, before, 100)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    if reset_connection(&e) {
                        client.disconnect();
                    }
                    self.telegram_failure(&e).await?;
                    return Err(telegram_error(e));
                }
            };
            highest = highest.max(page.candidate_high_water.unwrap_or(after));
            let mut messages = page
                .messages
                .iter()
                .map(normalized_message)
                .collect::<Vec<_>>();
            let complete = if baseline {
                let (retained, done) =
                    initial_history::page(&mut state, messages, page.interval_drained)?;
                messages = retained;
                done
            } else {
                page.interval_drained
            };
            state["attempt_records"] =
                json!(state["attempt_records"].as_u64().unwrap_or(0) + messages.len() as u64);
            state["requested_action"] = Value::Null;
            if complete {
                state["watermark"] = if highest > 0 {
                    json!(highest.to_string())
                } else {
                    Value::Null
                };
                state["baseline_established"] = json!(true);
                state["continuation"] = Value::Null;
                state["last_success_at"] = json!(now());
                state["next_due_at"] = json!(later(120));
                state["status"] = json!("waiting");
                state["detail"] = json!(if baseline {
                    initial_history::description(&state, true)
                } else {
                    "Visible history interval caught up; listening for new messages and edits."
                        .to_owned()
                });
            } else {
                state["continuation"] = json!({"before":page.resume_before,"highest":highest,"original_watermark":after.to_string(),"account_key":account,"peer_id":peer});
                // The adapter rejects non-decreasing offsets; this saved page
                // therefore represents durable progress, not a retry loop.
                state["catchup_auto_resume"] = json!(true);
                state["status"] = json!(if baseline
                    && state["initial_history"]["pages"].as_u64().unwrap_or(30) >= 30
                {
                    "needs_attention"
                } else {
                    "catching_up"
                });
                state["detail"] = json!(if baseline {
                    initial_history::description(&state, false)
                } else {
                    "Catching up on new messages; the next batch continues automatically."
                        .to_owned()
                });
                state["next_due_at"] = json!(later(if baseline { 2 } else { 120 }));
            }
            state=self.publish(sub,source.clone(),messages,json!({"provider":"telegram","representation":"sanitized decoded TL messages","messages":page.messages.iter().map(|m|&m.raw).collect::<Vec<_>>()}),state,cancel.clone()).await?;
            if complete {
                break;
            }
            before = page.resume_before;
        }
        // Revisit a paced slice of retained IDs for edits/removals missed while
        // disconnected. Access errors never become successful missing messages.
        if !state["continuation"].is_object() {
            let source_id = text(&state, "source_id");
            let marker = state["reconcile_after"].as_i64().unwrap_or(0);
            let ids=self.block(move|s|{let c=s.connect()?;let mut stmt=c.prepare("SELECT DISTINCT CAST(json_extract(json,'$.relationships.message_id') AS INTEGER) AS mid FROM evidence WHERE source=?1 AND mid>?2 ORDER BY mid LIMIT 20")?;let ids=stmt.query_map(params![source_id,marker],|r|r.get::<_,i32>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;Ok(ids)}).await?;
            if !ids.is_empty() {
                let exact = match client.exact_messages(&peer, &ids).await {
                    Ok(v) => v,
                    Err(e) => {
                        if reset_connection(&e) {
                            client.disconnect();
                        }
                        self.telegram_failure(&e).await?;
                        return Err(telegram_error(e));
                    }
                };
                let mut messages = exact
                    .messages
                    .iter()
                    .map(normalized_message)
                    .collect::<Vec<_>>();
                let observed = now();
                messages.extend(
                    exact
                        .unavailable_ids
                        .iter()
                        .map(|id| tombstone(&account, &peer, id, &observed)),
                );
                state["attempt_records"] =
                    json!(state["attempt_records"].as_u64().unwrap_or(0) + messages.len() as u64);
                state["reconcile_after"] = json!(ids.last());
                state=self.publish(sub,source,messages,json!({"provider":"telegram","operation":"exact_id_reconciliation","requested_ids":ids,"response":exact}),state,cancel).await?;
            } else {
                state["reconcile_after"] = json!(0);
                state = self.save_state(sub, state).await?;
            }
        }
        Ok(state)
    }
    pub(super) async fn drain_telegram_update(&self, configs: &[Value]) -> Result<bool> {
        let mut guard = self.telegram.lock().await;
        let Some(client) = guard.as_mut() else {
            return Ok(false);
        };
        let delivery = match client.next_update(Duration::from_millis(20)).await {
            Ok(Some(delivery)) => delivery,
            Ok(None) => return Ok(false),
            Err(error) => {
                if reset_connection(&error) {
                    client.disconnect();
                }
                self.telegram_failure(&error).await?;
                return Err(telegram_error(error));
            }
        };
        let result = async {
            let peer = match &delivery.event {
                MessageEvent::New { message } | MessageEvent::Edited { message } => Some((message.account_id.as_str(), message.peer_id.as_str())),
                MessageEvent::Deleted { account_id, peer_id, .. } => Some((account_id.as_str(),peer_id.as_str())), _ => None,
            };
            if let Some((account, peer)) = peer {
                let states = self.block(|s| s.collection_states()).await?;
                for state in states.iter().filter(|s| s["provider"] == "telegram" && s["account_key"].as_str() == Some(account) && s["peer_id"].as_str() == Some(peer) && s["stopped"] != true) {
                    let Some(sub) = configs.iter().find(|s| s["id"] == state["subscription_id"] && s["version"] == state["configuration_version"] && s["enabled"] == true) else { continue; };
                    let messages = match &delivery.event {
                        MessageEvent::New { message } | MessageEvent::Edited { message } => vec![normalized_message(message)],
                        MessageEvent::Deleted { account_id, peer_id, message_ids, acquired_at } => message_ids.iter().map(|id| tombstone(account_id,peer_id,id,acquired_at)).collect(),
                        _ => vec![],
                    };
                    let source = json!({"id":state["source_id"],"label":sub["label"],"family":"social_reporting","coverage":"Account-visible updates; durable history watermark advances only after a complete recovery interval.","detail":"Retained Telegram update. Periodic history and exact-ID reconciliation remain active."});
                    let mut next = state.clone(); next["last_update_at"] = json!(now());
                    if let Some(object) = next.as_object_mut() { object.remove("run_id"); }
                    self.publish(sub,source,messages,json!({"provider":"telegram","operation":"update","update":delivery.event}),next,Arc::new(AtomicBool::new(false))).await?;
                }
            }
            client.acknowledge_update(delivery.sequence).await.map_err(telegram_error)?;
            Ok(true)
        }.await;
        if result.is_err() {
            client.disconnect();
            let until = later(60);
            self.persist_retry("telegram", until.clone()).await?;
            self.set_connection("telegram","error","Telegram update was not acknowledged. Retained data is safe; reconnect will replay from the durable checkpoint.",None,Some(until))?;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deletion_uses_observation_time_without_fabricating_an_edit_timestamp() {
        let removed = tombstone("123", "-1000000012345", "9", "2026-09-10T12:00:00Z");
        assert_eq!(removed["id"], "telegram:123:-1000000012345:9");
        assert_eq!(removed["upstream_observed_at"], "2026-09-10T12:00:00Z");
        assert!(removed.get("source_edited_at").is_none());
        assert_eq!(removed["upstream_state"], "removed");
    }
    #[tokio::test]
    async fn pending_authentication_is_cancelled_when_connection_epoch_changes() {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::config::Config::local(
            std::env::current_dir().unwrap(),
            directory.path().join("data"),
            "127.0.0.1:8792".parse().unwrap(),
        );
        let runtime = Runtime::new(Store::open(config).unwrap());
        runtime.telegram_epoch.store(2, Ordering::Release);
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            runtime.telegram_auth_until_epoch(1, std::future::pending::<()>()),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, "cancelled");
    }
}
