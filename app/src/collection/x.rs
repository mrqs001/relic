use super::*;
use crate::sources::{FailureKind, SourceFailure, TimelinePage, TimelinePost};
use std::collections::BTreeSet;

pub(super) fn x_error(error: SourceFailure) -> Error {
    Error {
        status: axum::http::StatusCode::BAD_GATEWAY,
        code: match error.kind {
            FailureKind::Unavailable => "source_unavailable",
            // X reports temporary timeline faults this way; the source retries with backoff.
            FailureKind::Transient => "source_transient",
            _ => "x_unavailable",
        },
        message: error.message,
    }
}

/// An exact decimal post or profile identity from saved collection state.
fn parse_id(value: &str) -> Result<u64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(x_error(SourceFailure::new(FailureKind::Other, "invalid decimal X identity")));
    }
    value.parse().map_err(|_| x_error(SourceFailure::new(FailureKind::Other, "X identity exceeds its integer range")))
}

// Timeline modules can contain other authors. Only confirmed monitored-author
// posts are published under this source; complete raw pages remain retained.
fn eligible_page(page: &TimelinePage, user_id: u64) -> (Vec<&TimelinePost>, BTreeSet<u64>, usize) {
    let own = page.posts.iter().filter(|p| p.author_id == Some(user_id)).collect::<Vec<_>>();
    let ordinary = own.iter().filter(|p| p.primary).map(|p| p.id).collect();
    let omitted = page.posts.len() - own.len();
    (own, ordinary, omitted)
}
// Crossing an ID boundary is a stop signal, never proof of a complete interval.
fn missing_watermark(ordinary: &BTreeSet<u64>, watermark: Option<u64>) -> bool {
    watermark.is_some_and(|w| !ordinary.contains(&w) && ordinary.iter().any(|id| *id < w))
}
fn repeats_cursor(next: Option<&str>, current: Option<&str>, seen: &BTreeSet<String>) -> bool {
    next.is_some_and(|next| Some(next) == current || seen.contains(next))
}

impl Runtime {
    pub(super) async fn x_failure(&self, error: &SourceFailure) -> Result<()> {
        let current = self.connection("x")?;
        if error.kind == FailureKind::RateLimited {
            let until = later(900);
            self.persist_retry("x", until.clone()).await?;
            self.set_connection(
                "x",
                if current.status == "connected" {
                    "connected"
                } else {
                    "error"
                },
                "X rate limited this account. Collection will resume after the cooldown.",
                current.account_label,
                Some(until),
            )?;
        } else if error.kind == FailureKind::Unauthorized {
            self.set_connection(
                "x",
                "error",
                "X access failed. Check the private account/proxy configuration and reconnect.",
                None,
                None,
            )?;
        }
        Ok(())
    }

    pub(super) async fn connect_x(&self) -> Result<Connection> {
        let manager = self
            .x
            .as_ref()
            .ok_or_else(|| Error::invalid("X token and proxy are not configured"))?;
        // Validates access without importing a timeline.
        let result = manager.check().await;
        if let Err(error) = result {
            self.x_failure(&error).await?;
            return Err(x_error(error));
        }
        self.set_connection(
            "x",
            "connected",
            "Connected through the configured proxy. Enabled profiles can now collect.",
            Some("Configured X account".into()),
            None,
        )
    }
    pub(super) async fn collect_x(
        &self,
        sub: &Value,
        mut state: Value,
        cancel: Arc<AtomicBool>,
    ) -> Result<Value> {
        let manager = self
            .x
            .as_ref()
            .ok_or_else(|| Error::invalid("X is not configured"))?;
        if state["account_key"]
            .as_str()
            .is_some_and(|k| k != self.x_account)
        {
            return Err(Error::conflict(
                "X credentials changed. The saved interval belongs to the previous session; reconnect/review this source before resuming.",
            ));
        }
        let user_id = if let Some(id) = state["peer_id"].as_str() {
            parse_id(id)?
        } else {
            match manager
                .resolve(text(sub, "target").trim_start_matches('@'))
                .await
            {
                Ok(user) => user,
                Err(error) => {
                    self.x_failure(&error).await?;
                    return Err(x_error(error));
                }
            }
            .ok_or_else(|| Error::invalid("X profile has no readable stable identity"))?
        };
        state["peer_id"] = json!(user_id.to_string());
        state["account_key"] = json!(self.x_account);
        state["bound_target"] = sub["target"].clone();
        state["provider"] = json!("x");
        let source = json!({"id":format!("x:user:{user_id}"),"label":sub["label"],"family":"social_reporting","coverage":"Account-visible timeline only. Recent first baseline; subsequent exact-watermark catch-up. Unreadable posts and historical edits/deletions may be unavailable.","detail":"Custom mobile GraphQL through the configured proxy; media is a reference until explicitly retained."});
        let watermark = state["watermark"]
            .as_str()
            .map(parse_id)
            .transpose()?;
        let baseline = initial_history::active(&state);
        let continuation = state["continuation"].clone();
        let mut cursor = continuation["cursor"].as_str().map(str::to_owned);
        let mut highest = continuation["highest"]
            .as_str()
            .map(parse_id)
            .transpose()?
            .or(watermark);
        let mut seen: BTreeSet<String> = continuation["seen_cursors"]
            .as_array()
            .map(|v| {
                v.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if state["cursor_restart_pending"] == true {
            // Recheck from the head against the SAME durable watermark. Retain
            // already acquired pages; never declare a gap caught up by ID order.
            cursor = None;
            seen.clear();
            state["cursor_restart_pending"] = json!(false);
            state["cursor_restart_attempts"] =
                json!(state["cursor_restart_attempts"].as_u64().unwrap_or(0) + 1);
        }
        let mut pages = if state["requested_action"] == "continue" {
            0
        } else {
            continuation["pages"].as_u64().unwrap_or(0)
        };
        let mut unreadable = continuation["unreadable"].as_u64().unwrap_or(0);
        let mut context_only = continuation["context_only"].as_u64().unwrap_or(0);
        for _ in 0..ATTEMPT_PAGES {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let page = match manager.page(user_id, cursor.as_deref()).await {
                Ok(page) => page,
                Err(error) => {
                    self.x_failure(&error).await?;
                    return Err(x_error(error));
                }
            };
            unreadable += page.unreadable as u64;
            let (mut eligible, ordinary, omitted) = eligible_page(&page, user_id);
            context_only += omitted as u64;
            for id in &ordinary {
                highest = Some(highest.map_or(*id, |h| h.max(*id)));
            }
            let overlap = watermark.is_some_and(|id| ordinary.contains(&id));
            pages += 1;
            let missing = !baseline && missing_watermark(&ordinary, watermark);
            if !baseline && let Some(w) = watermark {
                // The full raw page is retained, but new catch-up cannot turn into
                // an old-history import when an upstream checkpoint disappears.
                eligible.retain(|p| p.id >= w);
            }
            let mut messages: Vec<Value> = eligible.into_iter().map(|p| p.message.clone()).collect();
            let complete = if baseline {
                // Pinned and embedded context posts cannot spend the initial
                // 100-message allowance or prematurely hit the age boundary.
                messages = page
                    .posts
                    .iter()
                    .filter(|p| ordinary.contains(&p.id))
                    .map(|p| p.message.clone())
                    .collect();
                let (retained, done) =
                    initial_history::page(&mut state, messages, page.next_cursor.is_none())?;
                messages = retained;
                done
            } else {
                overlap || page.next_cursor.is_none()
            };
            let repeated = repeats_cursor(page.next_cursor.as_deref(), cursor.as_deref(), &seen);
            if let Some(current) = cursor.as_ref() {
                seen.insert(current.clone());
            }
            state["attempt_records"] =
                json!(state["attempt_records"].as_u64().unwrap_or(0) + messages.len() as u64);
            state["unreadable"] = json!(unreadable);
            state["context_only"] = json!(context_only);
            state["requested_action"] = Value::Null;
            state["transient_failures"] = json!(0);
            state["transient_since"] = Value::Null;
            if complete {
                state["checkpoint_missing"] = json!(false);
                state["cursor_restart_attempts"] = json!(0);
                state["cursor_restart_pending"] = json!(false);
                state["watermark"] = json!(highest.map(|h| h.to_string()));
                state["continuation"] = Value::Null;
                state["baseline_established"] = json!(highest.is_some());
                state["last_success_at"] = json!(now());
                state["next_due_at"] =
                    json!(later(sub["poll_interval_seconds"].as_i64().unwrap_or(120)));
                state["status"] = json!("waiting");
                state["detail"] = json!(if baseline {
                    initial_history::description(&state, true)
                } else {
                    format!(
                        "Account-visible interval caught up. {unreadable} unreadable entries; {context_only} context posts kept in raw pages only. Historical edits/deletions are not fully reconciled."
                    )
                });
                if highest.is_none() {
                    state["detail"] = json!(
                        "Recent page retained, but it contained no readable ordinary posts by this profile. The next poll will check a recent page again; no history boundary has been invented."
                    );
                }
            } else {
                state["continuation"] = json!({"cursor":page.next_cursor,"highest":highest.map(|h| h.to_string()),"original_watermark":watermark.map(|w| w.to_string()),"seen_cursors":seen,"unreadable":unreadable,"context_only":context_only,"pages":pages,"account_key":self.x_account,"peer_id":user_id.to_string()});
                let cursor_limit = pages >= 30
                    || seen.len() >= 1024
                    || serde_json::to_vec(&seen)?.len() >= 32 * 1024;
                state["catchup_auto_resume"] = json!(!missing && !repeated && !cursor_limit);
                state["status"] = json!(if missing
                    || repeated
                    || cursor_limit
                    || (baseline && state["initial_history"]["pages"].as_u64().unwrap_or(30) >= 30)
                {
                    "needs_attention"
                } else {
                    "catching_up"
                });
                state["checkpoint_missing"] = json!(missing);
                state["detail"] = json!(if missing {
                    "The previous X checkpoint was not visible before older posts. New messages are retained; the interval is incomplete. Retry or restart recent history with a recorded gap.".to_owned()
                } else if repeated {
                    "X repeated a cursor. Acquired pages are retained; the original watermark has not advanced.".to_owned()
                } else if cursor_limit {
                    "X catch-up reached its cursor-history safety bound. Retained messages are available; the interval needs review.".to_owned()
                } else if baseline {
                    initial_history::description(&state, false)
                } else {
                    "Catching up on new posts; the next batch continues automatically.".to_owned()
                });
                let restart = !missing
                    && repeated
                    && state["cursor_restart_attempts"].as_u64().unwrap_or(0) < 3;
                state["cursor_restart_pending"] = json!(restart);
                state["next_due_at"] = if restart {
                    json!(later(
                        match state["cursor_restart_attempts"].as_u64().unwrap_or(0) {
                            0 => 60,
                            1 => 300,
                            _ => 900,
                        }
                    ))
                } else if missing || repeated || cursor_limit {
                    Value::Null
                } else {
                    json!(later(if baseline {
                        2
                    } else {
                        sub["poll_interval_seconds"].as_i64().unwrap_or(120).max(30)
                    }))
                };
            }
            state=self.publish(sub,source.clone(),messages,json!({"provider":"x","representation":"decoded provider JSON","page":page.raw_payload}),state,cancel.clone()).await?;
            if complete || missing || repeated || pages >= 30 {
                return Ok(state);
            }
            cursor = page.next_cursor;
        }
        Ok(state)
    }
    pub(super) async fn persist_retry(&self, provider: &str, until: String) -> Result<()> {
        let key = format!("collection:{provider}:retry_at");
        self.block(move|s|{let _g=s.writer.lock().map_err(|_|Error::internal())?;s.connect()?.execute("INSERT INTO metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,until])?;Ok(())}).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn post(id: u64, author: Option<u64>, primary: bool) -> TimelinePost {
        TimelinePost { id, author_id: author, primary, message: json!({"id":format!("x:post:{id}"),"text":"Synthetic original"}) }
    }
    fn page(posts: Vec<TimelinePost>) -> TimelinePage {
        TimelinePage { posts, unreadable: 0, next_cursor: Some("next".into()), raw_payload: json!({}) }
    }
    #[test]
    fn foreign_and_unknown_authors_stay_raw_and_pins_never_anchor() {
        let page = page(vec![post(10, Some(1), true), post(99, Some(1), false), post(100, Some(2), true), post(101, None, true)]);
        let (messages, ordinary, omitted) = eligible_page(&page, 1);
        assert_eq!(messages.len(), 2);
        assert_eq!(omitted, 2);
        assert_eq!(ordinary, BTreeSet::from([10]));
        assert_eq!(eligible_page(&page, 55).1.len(), 0);
    }
    #[test]
    fn missing_checkpoint_only_uses_ordinary_own_posts_and_never_claims_overlap() {
        let w = Some(100);
        assert!(!missing_watermark(&BTreeSet::new(), w));
        assert!(!missing_watermark(&[101].into(), w));
        assert!(!missing_watermark(&[99, 100].into(), w));
        assert!(missing_watermark(&[99, 101].into(), w));
        let page = page(vec![post(99, Some(2), true), post(98, Some(1), false), post(101, Some(1), true)]);
        assert!(!missing_watermark(&eligible_page(&page, 1).1, w));
    }
    #[test]
    fn saved_identities_are_exact_decimals() {
        assert_eq!(parse_id("18446744073709551000").unwrap(), 18446744073709551000);
        assert!(parse_id("").is_err() && parse_id("-1").is_err() && parse_id("18446744073709551616").is_err());
    }
    #[test]
    fn cursor_repetition_detected_before_another_request() {
        let seen = BTreeSet::from(["earlier".to_owned()]);
        assert!(repeats_cursor(Some("current"), Some("current"), &seen));
        assert!(repeats_cursor(Some("earlier"), Some("current"), &seen));
        assert!(!repeats_cursor(Some("next"), Some("current"), &seen));
        assert!(!repeats_cursor(None, Some("current"), &seen));
    }
}
