//! Durable adapter output. No network calls, credentials, or source scheduling.
//!
//! A bounded provider page is staged and synced before one transaction commits
//! evidence, executed lineage, cumulative publication, and its next checkpoint.
//! Configuration versions reject late responses after edits; state generations
//! reject competing workers and replay after an ambiguous commit acknowledgement.
use crate::{
    error::{Error, Result},
    model::{bounded_text, hash, ident, now, required, string, timestamp},
    store::{MAX_IMPORT_BYTES, MAX_IMPORT_RECORDS, Store, edge, node},
    subscriptions::MAX_SUBSCRIPTIONS,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub const MAX_COLLECTION_STATE_BYTES: usize = 64 * 1024;

impl Store {
    /// Call before opening a provider connection. Publication repeats this
    /// check under the writer transaction to close races with fixture import.
    pub fn assert_real_collection_allowed(&self) -> Result<()> {
        assert_real(&*self.connect()?)
    }

    pub fn collection_state(&self, id: &str) -> Result<Value> {
        let c = self.connect()?;
        let (version, _) = configuration(&c, id, None)?;
        Ok(state_row(&c, id)?.unwrap_or_else(|| {
            json!({
                "subscription_id":id,"configuration_version":version,"generation":0
            })
        }))
    }

    pub fn collection_states(&self) -> Result<Vec<Value>> {
        let c = self.connect()?;
        let rows = c
            .prepare("SELECT json FROM collection_states ORDER BY subscription_id LIMIT ?1")?
            .query_map([(MAX_SUBSCRIPTIONS + 1) as i64], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if rows.len() > MAX_SUBSCRIPTIONS {
            return Err(Error::internal());
        }
        rows.into_iter()
            .map(|row| Ok(serde_json::from_str(&row)?))
            .collect()
    }

    /// Status/backoff changes may not advance an acquisition checkpoint.
    pub fn save_collection_state(
        &self,
        id: &str,
        expected_configuration_version: i64,
        state: Value,
    ) -> Result<Value> {
        validate_state(&state)?;
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        configuration(&tx, id, Some(expected_configuration_version))?;
        let previous = state_row(&tx, id)?;
        let generation = check_generation(previous.as_ref(), &state)?;
        for field in [
            "watermark",
            "continuation",
            "last_success_at",
            "messages_acquired",
        ] {
            let old = previous.as_ref().map(|p| &p[field]).unwrap_or(&Value::Null);
            if &state[field] != old {
                let resetting_changed_config = previous
                    .as_ref()
                    .is_some_and(|p| p["configuration_version"] != expected_configuration_version)
                    && state[field].is_null();
                if !resetting_changed_config {
                    return Err(Error::conflict(
                        "Acquisition progress advances only with a durable collection page",
                    ));
                }
            }
        }
        let result = commit_state(&tx, id, expected_configuration_version, generation, state)?;
        let event = Self::event(
            &tx,
            "collection_state",
            Self::latest(&tx)?.as_deref(),
            vec![id.into()],
        )?;
        tx.commit()?;
        let _ = self.events.send(event);
        Ok(result)
    }

    /// Owner-requested new baseline. Retain the abandoned interval as an exact
    /// durable gap before resetting cursors; all acquired evidence stays intact.
    pub fn rebaseline_collection(&self, id: &str, expected: i64, state: Value) -> Result<Value> {
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        configuration(&tx, id, Some(expected))?;
        let prior = state_row(&tx, id)?
            .ok_or_else(|| Error::conflict("No collection interval to restart"))?;
        let generation = check_generation(Some(&prior), &state)?;
        if prior["provider"] != "x" || !prior["continuation"].is_object() {
            return Err(Error::conflict(
                "Only an incomplete X interval can be rebaselined",
            ));
        }
        let gap = crate::model::ident("collection-gap");
        tx.execute(
            "INSERT INTO collection_gaps VALUES(?1,?2,?3,?4)",
            params![
                gap,
                id,
                now(),
                string(&json!({"reason":"explicit_recent_history_restart","prior_state":prior}))
            ],
        )?;
        let mut next = prior;
        for field in [
            "watermark",
            "continuation",
            "initial_history",
            "last_success_at",
            "last_error_code",
        ] {
            next[field] = Value::Null;
        }
        next["last_gap_id"] = json!(gap);
        next["status"] = json!("waiting");
        next["detail"] = json!(
            "Restarting the newest 100 messages or last month. The abandoned interval is recorded as a gap; retained messages stay available."
        );
        next["requested_action"] = json!("baseline");
        next["next_due_at"] = json!(now());
        next["stopped"] = json!(false);
        next["catchup_auto_resume"] = json!(true);
        next["checkpoint_missing"] = json!(false);
        next["cursor_restart_pending"] = json!(false);
        next["cursor_restart_attempts"] = json!(0);
        let result = commit_state(&tx, id, expected, generation, next)?;
        let event = Self::event(
            &tx,
            "collection_state",
            Self::latest(&tx)?.as_deref(),
            vec![id.into()],
        )?;
        tx.commit()?;
        let _ = self.events.send(event);
        Ok(result)
    }

    /// `source.id` is adapter-resolved provider identity, never a mutable handle.
    /// Input media references are metadata only; this method never downloads them.
    #[allow(clippy::too_many_arguments)]
    pub fn publish_collection_page(
        &self,
        id: &str,
        expected_configuration_version: i64,
        source: Value,
        messages: Vec<Value>,
        raw: Value,
        next_state: Value,
        cancel: Arc<AtomicBool>,
    ) -> Result<Value> {
        self.assert_real_collection_allowed()?;
        validate_state(&next_state)?;
        if let Some(run_id) = next_state.get("run_id").filter(|v| !v.is_null()) {
            let run_id = run_id
                .as_str()
                .ok_or_else(|| Error::invalid("Collection operation identity must be text"))?;
            if !run_id.starts_with("collection:")
                || run_id.len() > 100
                || !run_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b':' || b == b'-')
            {
                return Err(Error::invalid("Invalid collection operation identity"));
            }
        }
        if messages.len() > MAX_IMPORT_RECORDS {
            return Err(Error::invalid("Collection page exceeds 2000 messages"));
        }
        let raw_bytes = serde_json::to_vec(&raw)?;
        let normalized_bytes = serde_json::to_vec(&messages)?;
        if raw_bytes.len() > MAX_IMPORT_BYTES || normalized_bytes.len() > MAX_IMPORT_BYTES {
            return Err(Error::invalid(
                "Collection page exceeds the 2 MiB raw or normalized bound",
            ));
        }
        check_cancel(&cancel)?;
        let (provider, previous) = {
            let c = self.connect()?;
            let (_, provider) = configuration(&c, id, Some(expected_configuration_version))?;
            let previous = state_row(&c, id)?;
            check_generation(previous.as_ref(), &next_state)?;
            (provider, previous)
        };
        let mut source = normalize_source(source, &provider)?;
        let source_id = required(&source, "id")?.to_string();
        if previous.as_ref().is_some_and(|p| {
            p["configuration_version"] == expected_configuration_version
                && p["source_id"].is_string()
                && p["source_id"] != source_id
        }) {
            return Err(Error::conflict(
                "Resolved source identity changed without a collection configuration revision",
            ));
        }
        let acquired = now();
        source["last_acquired_at"] = json!(acquired);
        let c = self.connect()?;
        let mut seen = HashSet::new();
        let mut entries = Vec::with_capacity(messages.len());
        // Validate every input before writing a retained object.
        for input in messages {
            check_cancel(&cancel)?;
            let e = normalize_message(&c, input, &source, &acquired)?;
            if !seen.insert((
                required(&e, "id")?.to_string(),
                required(&e, "revision")?.to_string(),
            )) {
                continue; // duplicate entries within one provider page share one exact revision
            }
            let bytes = serde_json::to_vec(&e)?;
            if bytes.len() > MAX_IMPORT_BYTES {
                return Err(Error::invalid(
                    "Normalized collection record exceeds the retained-object read bound",
                ));
            }
            entries.push((e, bytes));
        }
        drop(c);
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        assert_real(&tx)?;
        configuration(&tx, id, Some(expected_configuration_version))?;
        let prior_state = state_row(&tx, id)?;
        let generation = check_generation(prior_state.as_ref(), &next_state)?;
        if let Some(old) = json_row(&tx, "SELECT json FROM sources WHERE id=?1", [&source_id])?
            && (old["provider"] != provider
                || old["fictional"] != false
                || old["family"] != source["family"])
        {
            return Err(Error::conflict(
                "A retained source identity cannot change provider, family or fictional status",
            ));
        }
        tx.execute("INSERT INTO sources(id,json) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET json=excluded.json", params![source_id,string(&source)])?;
        node(
            &tx,
            &source_id,
            "source_instance",
            required(&source, "label")?,
            &source,
        )?;
        tx.execute(
            "UPDATE lineage_nodes SET label=?1,detail=?2 WHERE id=?3 AND kind='source_instance'",
            params![source["label"].as_str(), string(&source), source_id],
        )?;
        // A successful unchanged check advances only the durable checkpoint.
        // Existing evidence already has exact acquisition/original provenance.
        let prior_publication = Self::latest(&tx)?;
        let unchanged = entries.iter().try_fold(true, |same, (e, _)| -> Result<bool> {
            let retained: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM evidence WHERE id=?1 AND revision=?2 AND source=?3)", params![e["id"].as_str(),e["revision"].as_str(),source_id], |r| r.get(0))?;
            Ok(same && retained)
        })?;
        if unchanged && let Some(publication) = prior_publication {
            let mut state = next_state;
            state["source_id"] = json!(source_id);
            state["publication_id"] = json!(publication);
            state["last_check"] = json!({"at":acquired,"unchanged":true,"records":entries.len()});
            let state = commit_state(&tx, id, expected_configuration_version, generation, state)?;
            let event = Self::event(&tx, "collection_state", Some(&publication), vec![id.into()])?;
            check_cancel(&cancel)?;
            tx.commit()?;
            let _ = self.events.send(event);
            return Ok(
                json!({"publication_id":publication,"job_id":null,"accepted":0,"reused":entries.len(),"original_input_hash":null,"unchanged":true,"state":state}),
            );
        }
        let raw_hash = self.write_object(&raw_bytes)?;
        for (e, bytes) in &mut entries {
            check_cancel(&cancel)?;
            if e["object_hash"].is_null() {
                e["object_hash"] = json!(self.write_object(bytes)?);
            }
        }
        self.sync_object_directories(
            entries
                .iter()
                .map(|(e, _)| e["object_hash"].as_str().unwrap().to_string())
                .chain(std::iter::once(raw_hash.clone())),
            &cancel,
        )?;
        check_cancel(&cancel)?;
        tx.execute(
            "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
            params![raw_hash, raw_bytes.len() as i64],
        )?;
        let run_id = ident("collection");
        if let Some(operation_id) = next_state["run_id"].as_str() {
            node(
                &tx,
                operation_id,
                "run_step",
                &format!("Collect {provider} messages"),
                &json!({"operation":"collection_attempt_v1","provider":provider,"subscription_id":id,"configuration_version":expected_configuration_version,"model_calls":0}),
            )?;
        }
        node(
            &tx,
            &run_id,
            "run_step",
            &format!("Retain {provider} messages"),
            &json!({
                "status":"succeeded","operation":"collection_page_v1","provider":provider,
                "subscription_id":id,"configuration_version":expected_configuration_version,
                "input_hash":raw_hash,"input_records":entries.len(),"model_calls":0,
                "started_at":acquired,"coverage":source["coverage"],
                "field_mappings":[{"input":"text","output":"text","operation":"preserve exact Unicode text"},{"input":"geometry","output":"geometry","operation":"preserve source geometry or unresolved"}]
            }),
        )?;
        if let Some(operation_id) = next_state["run_id"].as_str() {
            edge(
                &tx,
                operation_id,
                &run_id,
                "executes_page",
                &json!({"input_hash":raw_hash}),
            )?;
        }
        edge(
            &tx,
            &source_id,
            &run_id,
            "collected_by",
            &json!({"input_hash":raw_hash}),
        )?;
        let mut accepted = 0;
        let mut changed = Vec::new();
        let mut retained = Vec::with_capacity(entries.len());
        for (e, bytes) in &entries {
            check_cancel(&cancel)?;
            let evidence_id = required(e, "id")?;
            let revision = required(e, "revision")?;
            if let Some(old_source) = tx
                .query_row(
                    "SELECT source FROM evidence WHERE id=?1 LIMIT 1",
                    [evidence_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                && old_source != source_id
            {
                return Err(Error::conflict("An evidence identity cannot change source"));
            }
            let rowid = if let Some(rowid) = tx
                .query_row(
                    "SELECT rowid FROM evidence WHERE id=?1 AND revision=?2",
                    params![evidence_id, revision],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
            {
                rowid
            } else {
                tx.execute(
                    "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
                    params![e["object_hash"].as_str(), bytes.len() as i64],
                )?;
                tx.execute("INSERT INTO evidence(id,revision,source,acquired_at,published_at,lon,lat,origin_group,text,object_hash,json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![evidence_id,revision,source_id,e["acquired_at"].as_str(),e["published_at"].as_str(),e["geometry"]["coordinates"][0].as_f64(),e["geometry"]["coordinates"][1].as_f64(),e["origin_group"].as_str(),e["text"].as_str(),e["object_hash"].as_str(),string(e)])?;
                let rowid = tx.last_insert_rowid();
                let root = format!("{evidence_id}@{revision}");
                node(
                    &tx,
                    &root,
                    "evidence_revision",
                    &root,
                    &json!({"evidence_id":evidence_id,"revision":revision,"object_hash":e["object_hash"],"acquired_at":acquired,"upstream_state":e["upstream_state"],"fictional":false}),
                )?;
                edge(
                    &tx,
                    &source_id,
                    &root,
                    "acquired_from",
                    &json!({"retained_original":e["object_hash"]}),
                )?;
                if let Some(previous) = e["supersedes"].as_object() {
                    let old_root = format!(
                        "{}@{}",
                        previous["id"].as_str().unwrap(),
                        previous["revision"].as_str().unwrap()
                    );
                    edge(
                        &tx,
                        &old_root,
                        &root,
                        "superseded_by",
                        &json!({"meaning":"Earlier exact content and citations remain retained"}),
                    )?;
                }
                edge(
                    &tx,
                    &format!("{evidence_id}@{revision}"),
                    &run_id,
                    "input_record",
                    &json!({"evidence_id":evidence_id,"revision":revision}),
                )?;
                accepted += 1;
                changed.push(evidence_id.to_string());
                rowid
            };
            retained.push(rowid);
        }
        let prior_publication = Self::latest(&tx)?;
        let is_new_publication = accepted > 0 || prior_publication.is_none();
        let publication_id = if is_new_publication {
            ident("publication")
        } else {
            prior_publication.clone().unwrap()
        };
        if is_new_publication {
            tx.execute(
                "INSERT INTO publications VALUES(?1,?2,?3,'complete',?4)",
                params![
                    publication_id,
                    format!("{} · retained messages", required(&source, "label")?),
                    acquired,
                    raw_hash
                ],
            )?;
            crate::publication_history::inherit(
                &tx,
                &publication_id,
                prior_publication.as_deref(),
            )?;
            for evidence_id in changed.iter().collect::<HashSet<_>>() {
                // Late history must not remove/re-add an unchanged current row:
                // its exact chosen reading may be older than another retained one.
                let selected: i64 = tx.query_row("SELECT rowid FROM evidence WHERE id=?1 ORDER BY CASE WHEN json_extract(json,'$.upstream_state')='removed' THEN coalesce(json_extract(json,'$.source_edited_at'),acquired_at) ELSE coalesce(json_extract(json,'$.source_edited_at'),published_at,acquired_at) END DESC,acquired_at DESC,revision DESC LIMIT 1",[evidence_id],|r|r.get(0))?;
                tx.execute("DELETE FROM membership WHERE publication_id=?1 AND evidence_rowid!=?3 AND evidence_rowid IN (SELECT rowid FROM evidence WHERE id=?2)",params![publication_id,evidence_id,selected])?;
                tx.execute(
                    "INSERT INTO membership VALUES(?1,?2)",
                    params![publication_id, selected],
                )?;
            }
            node(
                &tx,
                &publication_id,
                "publication",
                &format!("{} · retained messages", required(&source, "label")?),
                &json!({"status":"complete","created_at":acquired,"input_hash":raw_hash,"recipe_revision":"collection-page-v1","coverage":source["coverage"],"complete_meaning":"Coherent retained evidence; not exhaustive upstream history"}),
            )?;
            if let Some(prior) = &prior_publication {
                edge(
                    &tx,
                    prior,
                    &publication_id,
                    "extends_publication",
                    &json!({"meaning":"Cumulative retained latest revisions; earlier publication remains available"}),
                )?;
            }
            tx.execute("INSERT INTO metadata VALUES('current_publication',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[&publication_id])?;
            crate::processing_store::carry_interpretations(&tx, &publication_id)?;
            self.evaluate_watches(&tx, &publication_id)?;
        }
        edge(
            &tx,
            &run_id,
            &publication_id,
            if is_new_publication {
                "published"
            } else {
                "reused_publication"
            },
            &json!({"accepted":accepted,"reused":entries.len()-accepted}),
        )?;
        let semantic_hash = hash(&serde_json::to_vec(
            &entries
                .iter()
                .map(|(e, _)| json!({"id":e["id"],"revision":e["revision"]}))
                .collect::<Vec<_>>(),
        )?);
        tx.execute(
            "INSERT INTO acquisitions VALUES(?1,?2,?3,?4,?5)",
            params![run_id, semantic_hash, raw_hash, publication_id, acquired],
        )?;
        for rowid in retained {
            tx.execute(
                "INSERT INTO evidence_acquisitions VALUES(?1,?2)",
                params![rowid, run_id],
            )?;
        }
        let mut next_state = next_state;
        next_state["source_id"] = json!(source_id);
        let acquired_total = prior_state
            .as_ref()
            .and_then(|state| state["messages_acquired"].as_u64())
            .unwrap_or(0);
        next_state["messages_acquired"] = json!(
            acquired_total
                .checked_add(accepted as u64)
                .ok_or_else(Error::internal)?
        );
        let attempt_accepted = next_state["attempt_accepted"].as_u64().unwrap_or(0);
        next_state["attempt_accepted"] = json!(
            attempt_accepted
                .checked_add(accepted as u64)
                .ok_or_else(Error::internal)?
        );
        next_state["publication_id"] = json!(publication_id);
        next_state["last_acquisition_id"] = json!(run_id);
        next_state["last_input_hash"] = json!(raw_hash);
        let state = commit_state(
            &tx,
            id,
            expected_configuration_version,
            generation,
            next_state,
        )?;
        let job = json!({"id":run_id,"kind":"collection","provider":provider,"subscription_id":id,"status":"completed","accepted":accepted,"total":entries.len(),"created_at":acquired,"detail":"Provider page retained; evidence, publication and acquisition checkpoint committed together.","publication_id":publication_id});
        tx.execute(
            "INSERT INTO jobs VALUES(?1,?2)",
            params![run_id, string(&job)],
        )?;
        let event = Self::event(
            &tx,
            if is_new_publication {
                "publication"
            } else {
                "acquisition"
            },
            Some(&publication_id),
            changed.into_iter().take(100).collect(),
        )?;
        let state_event = Self::event(
            &tx,
            "collection_state",
            Some(&publication_id),
            vec![id.into()],
        )?;
        check_cancel(&cancel)?;
        tx.commit()?;
        let _ = self.events.send(event);
        let _ = self.events.send(state_event);
        Ok(
            json!({"publication_id":publication_id,"job_id":run_id,"accepted":accepted,"reused":entries.len()-accepted,"original_input_hash":raw_hash,"state":state}),
        )
    }
}

fn assert_real(c: &Connection) -> Result<()> {
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sources WHERE json_extract(json,'$.fictional')=1)",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(Error::conflict(
            "Live collection requires a separate real-evidence workspace; this directory contains fictional fixtures",
        ));
    }
    Ok(())
}

fn configuration(c: &Connection, id: &str, expected: Option<i64>) -> Result<(i64, String)> {
    let (version, provider) = c
        .query_row(
            "SELECT version,provider FROM subscriptions WHERE id=?1",
            [id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
        .ok_or_else(|| Error::missing("Collection configuration is unavailable"))?;
    if expected.is_some_and(|expected| expected != version) {
        return Err(Error::conflict(
            "Collection configuration changed while acquisition was running",
        ));
    }
    Ok((version, provider))
}

fn state_row(c: &Connection, id: &str) -> Result<Option<Value>> {
    json_row(
        c,
        "SELECT json FROM collection_states WHERE subscription_id=?1",
        [id],
    )
}

fn json_row<P: rusqlite::Params>(c: &Connection, sql: &str, params: P) -> Result<Option<Value>> {
    c.query_row(sql, params, |r| r.get::<_, String>(0))
        .optional()?
        .map(|raw| Ok(serde_json::from_str(&raw)?))
        .transpose()
}

fn validate_state(state: &Value) -> Result<()> {
    if !state.is_object() || serde_json::to_vec(state)?.len() > MAX_COLLECTION_STATE_BYTES {
        return Err(Error::invalid(
            "Collection state must be an object bounded to 64 KiB",
        ));
    }
    if state
        .get("generation")
        .is_some_and(|v| v.as_u64().is_none())
    {
        return Err(Error::invalid(
            "Collection generation must be a nonnegative integer",
        ));
    }
    for field in ["next_due_at", "last_success_at"] {
        if !state[field].is_null() {
            timestamp(required(state, field)?)?;
        }
    }
    Ok(())
}

fn check_generation(previous: Option<&Value>, next: &Value) -> Result<i64> {
    let current = previous.and_then(|p| p["generation"].as_i64()).unwrap_or(0);
    if next["generation"].as_i64().unwrap_or(0) != current {
        return Err(Error::conflict(
            "Collection progress changed; reload the durable checkpoint before continuing",
        ));
    }
    current.checked_add(1).ok_or_else(Error::internal)
}

fn commit_state(
    c: &Connection,
    id: &str,
    version: i64,
    generation: i64,
    mut state: Value,
) -> Result<Value> {
    state["subscription_id"] = json!(id);
    state["configuration_version"] = json!(version);
    state["generation"] = json!(generation);
    validate_state(&state)?;
    c.execute("INSERT INTO collection_states VALUES(?1,?2,?3,?4) ON CONFLICT(subscription_id) DO UPDATE SET configuration_version=excluded.configuration_version,generation=excluded.generation,json=excluded.json",params![id,version,generation,string(&state)])?;
    Ok(state)
}

fn normalize_source(mut source: Value, provider: &str) -> Result<Value> {
    let id = bounded_text(&source, "id", 200)?;
    if !id.starts_with(&format!("{provider}:")) || id.contains('@') || id.contains('/') {
        return Err(Error::invalid(
            "Resolved source identity must be provider-qualified and independent of its public handle",
        ));
    }
    bounded_text(&source, "label", 200)?;
    if source.get("fictional").is_some_and(|v| v != false) {
        return Err(Error::invalid("Collection source cannot be fictional"));
    }
    if source.get("provider").is_some_and(|v| v != provider) {
        return Err(Error::invalid(
            "Collection source provider differs from its configuration",
        ));
    }
    if !source.is_object() || serde_json::to_vec(&source)?.len() > 16 * 1024 {
        return Err(Error::invalid("Collection source metadata exceeds 16 KiB"));
    }
    source["provider"] = json!(provider);
    source["fictional"] = json!(false);
    if source["family"].is_null() {
        source["family"] = json!("social_reporting");
    }
    bounded_text(&source, "family", 100)?;
    if source["status"].is_null() {
        source["status"] = json!("available");
    }
    if !["available", "partial", "unavailable", "stale"].contains(&required(&source, "status")?) {
        return Err(Error::invalid("Invalid collection source availability"));
    }
    if source["coverage"].is_null() {
        source["coverage"] = json!(
            "Bounded acquired messages only; upstream history, deletions and media coverage may be incomplete"
        );
    }
    bounded_text(&source, "coverage", 4000)?;
    if source["detail"].is_null() {
        source["detail"] = json!(
            "Messages retained from the configured provider; references do not imply downloaded media."
        );
    }
    bounded_text(&source, "detail", 4000)?;
    Ok(source)
}

pub(crate) fn normalize_message(
    c: &Connection,
    mut e: Value,
    source: &Value,
    acquired: &str,
) -> Result<Value> {
    let id = bounded_text(&e, "id", 240)?;
    let source_id = required(source, "id")?;
    let provider = required(source, "provider")?;
    if !id.starts_with(&format!("{provider}:")) {
        return Err(Error::invalid(
            "Collected evidence identity must be provider-qualified",
        ));
    }
    if e.get("source").is_some_and(|v| v != source_id) {
        return Err(Error::invalid(
            "Collected message belongs to another source",
        ));
    }
    if e.get("fictional").is_some_and(|v| v != false) {
        return Err(Error::invalid(
            "Collected messages cannot be fictional fixtures",
        ));
    }
    let text = e["text"]
        .as_str()
        .filter(|text| text.len() <= 64 * 1024)
        .ok_or_else(|| {
            Error::invalid("Collected text must be exact text up to 64 KiB; empty text is allowed")
        })?
        .to_string();
    e["text"] = json!(text);
    if e["upstream_state"].is_null() {
        e["upstream_state"] = json!("present");
    }
    if !["present", "removed"].contains(&required(&e, "upstream_state")?) {
        return Err(Error::invalid("Invalid message upstream state"));
    }
    for field in [
        "published_at",
        "source_edited_at",
        "event_at",
        "upstream_observed_at",
    ] {
        e[field] = if e[field].is_null() {
            Value::Null
        } else {
            json!(timestamp(required(&e, field)?)?)
        };
    }
    if e["media"].is_null() {
        e["media"] = json!([]);
    }
    if e["relationships"].is_null() {
        e["relationships"] = json!({});
    }
    if e["media"].as_array().is_none_or(|v| v.len() > 100) || !e["relationships"].is_object() {
        return Err(Error::invalid(
            "Media must be a bounded array and relationships an object",
        ));
    }
    if e["geometry"].is_null() {
        e["geometry"] = Value::Null;
        e["location_precision"] = json!("unresolved");
    } else {
        let geometry = &e["geometry"];
        let coordinates = geometry["coordinates"]
            .as_array()
            .filter(|v| v.len() == 2)
            .ok_or_else(|| Error::invalid("Collection geometry must be a two-dimensional point"))?;
        let lon = coordinates[0]
            .as_f64()
            .ok_or_else(|| Error::invalid("Invalid longitude"))?;
        let lat = coordinates[1]
            .as_f64()
            .ok_or_else(|| Error::invalid("Invalid latitude"))?;
        if geometry["type"] != "Point"
            || !lon.is_finite()
            || !lat.is_finite()
            || lon.abs() > 180.
            || lat.abs() > 90.
        {
            return Err(Error::invalid(
                "Collection geometry is outside WGS84 point bounds",
            ));
        }
        if e["location_precision"].is_null() {
            e["location_precision"] = json!("source_asserted");
        }
        bounded_text(&e, "location_precision", 100)?;
    }
    let previous = json_row(
        c,
        "SELECT json FROM evidence WHERE id=?1 ORDER BY CASE WHEN json_extract(json,'$.upstream_state')='removed' THEN coalesce(json_extract(json,'$.source_edited_at'),acquired_at) ELSE coalesce(json_extract(json,'$.source_edited_at'),published_at,acquired_at) END DESC,acquired_at DESC,revision DESC LIMIT 1",
        [&id],
    )?;
    // Deletion notifications have no text. Retain the last exact locally held
    // text and label why it is visible, without claiming it is still upstream.
    if e["upstream_state"] == "removed"
        && text.is_empty()
        && let Some(previous) = &previous
    {
        e["text"] = previous["text"].clone();
        e["retained_text_from"] = json!({"id":id,"revision":previous["revision"]});
        if e["published_at"].is_null() {
            e["published_at"] = previous["published_at"].clone();
        }
    }
    e["source"] = json!(source_id);
    e["fictional"] = json!(false);
    if e["origin_group"].is_null() {
        e["origin_group"] = Value::Null;
    }
    if !e["origin_group"].is_null() {
        bounded_text(&e, "origin_group", 500)?;
    }
    // Include content-bearing optional metadata while excluding measurement and
    // transport volatility. Full decoded provider pages retain those values.
    let mut content = e.clone();
    scrub_volatile(&mut content);
    if content["upstream_state"] == "removed" {
        // Repeated confirmed absence is an observation of the same tombstone,
        // not a fresh content edit every time a reconciliation sweep runs.
        content.as_object_mut().unwrap().remove("source_edited_at");
    }
    let revision = format!("sha256:{}", hash(&serde_json::to_vec(&content)?));
    e["revision"] = json!(revision);
    e["acquired_at"] = json!(acquired);
    e["source_label"] = source["label"].clone();
    e["family"] = source["family"].clone();
    e["availability"] = json!("available");
    if e["language"].is_null() {
        e["language"] = json!("und");
    }
    let length = e["text"].as_str().unwrap().chars().count();
    e["citation"] = if length == 0 {
        Value::Null
    } else {
        json!({"evidence_id":id,"revision":revision,"start_character":0,"end_character":length})
    };
    if let Some(previous) = previous
        && previous["revision"] != revision
        && effective_time(&e) >= effective_time(&previous)
    {
        e["supersedes"] = json!({"id":id,"revision":previous["revision"]});
    }
    if let Some(retained) = json_row(
        c,
        "SELECT json FROM evidence WHERE id=?1 AND revision=?2",
        params![id, revision],
    )? {
        return Ok(retained);
    }
    Ok(e)
}

fn scrub_volatile(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for key in [
                "acquired_at",
                "fetched_at",
                "observed_at",
                "upstream_observed_at",
                "view_count",
                "views",
                "retweet_count",
                "favorite_count",
                "reply_count",
                "quote_count",
                "bookmark_count",
                "forward_count",
                "forwards",
                "like_count",
                "likes",
                "counters",
                "observations",
                "metrics",
                "raw",
                "object_hash",
                "revision",
                "citation",
                "source_label",
                "family",
                "supersedes",
                "retained_text_from",
                "file_reference",
                "access_hash",
            ] {
                object.remove(key);
            }
            for value in object.values_mut() {
                scrub_volatile(value);
            }
        }
        Value::Array(array) => {
            for value in array {
                scrub_volatile(value);
            }
        }
        _ => {}
    }
}

fn effective_time(e: &Value) -> &str {
    e["source_edited_at"]
        .as_str()
        .or_else(|| {
            if e["upstream_state"] == "removed" {
                None
            } else {
                e["published_at"].as_str()
            }
        })
        .or_else(|| e["acquired_at"].as_str())
        .unwrap_or("")
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::cancelled())
    } else {
        Ok(())
    }
}

/// Older snapshots have no collection state table. A checkpoint is meaningful
/// only together with the bounded configuration and referenced publication data.
pub(crate) fn verify_collection_states(c: &Connection) -> Result<usize> {
    if !c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='collection_states')",[],|r|r.get::<_,bool>(0))? { return Ok(0); }
    let rows = c.prepare("SELECT subscription_id,configuration_version,generation,json FROM collection_states LIMIT ?1")?
        .query_map([(MAX_SUBSCRIPTIONS+1) as i64],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?)))?
        .collect::<std::result::Result<Vec<_>,_>>()?;
    if rows.len() > MAX_SUBSCRIPTIONS {
        return Err(Error::invalid(
            "Restored collection state exceeds its bound",
        ));
    }
    for (id, version, generation, raw) in &rows {
        let value: Value = serde_json::from_str(raw)?;
        validate_state(&value)?;
        let (current, _) = configuration(c, id, None)?;
        if *version < 1
            || *version > current
            || *generation < 1
            || value["subscription_id"] != *id
            || value["configuration_version"] != *version
            || value["generation"] != *generation
        {
            return Err(Error::invalid("Restored acquisition checkpoint is invalid"));
        }
        if let Some(source_id) = value["source_id"].as_str()
            && !c.query_row(
                "SELECT EXISTS(SELECT 1 FROM sources WHERE id=?1)",
                [source_id],
                |r| r.get::<_, bool>(0),
            )?
        {
            // Resolution may be saved before the first page. It does not yet
            // claim a durable acquisition until last_success_at exists.
            if !value["last_success_at"].is_null() {
                return Err(Error::invalid("Restored acquisition source is unavailable"));
            }
        }
        if let Some(acquisition_id) = value["last_acquisition_id"].as_str() {
            // An unchanged check moves the checkpoint to the latest publication,
            // which another source may have made; its acquisition stays the
            // earlier one. The acquisition must exist with the same input and
            // be published no later than the checkpoint's publication.
            // Snapshots older than publication_order are ordered by publication rowid.
            let ordered = c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='publication_order')", [], |r| r.get::<_, bool>(0))?;
            let sql = if ordered {
                "SELECT EXISTS(SELECT 1 FROM acquisitions a JOIN publication_order pa ON pa.id=a.publication_id JOIN publication_order ps ON ps.id=?2 WHERE a.id=?1 AND a.object_hash=?3 AND pa.seq<=ps.seq)"
            } else {
                "SELECT EXISTS(SELECT 1 FROM acquisitions a JOIN publications pa ON pa.id=a.publication_id JOIN publications ps ON ps.id=?2 WHERE a.id=?1 AND a.object_hash=?3 AND pa.rowid<=ps.rowid)"
            };
            let matches:bool = c.query_row(sql,params![acquisition_id,value["publication_id"].as_str(),value["last_input_hash"].as_str()],|r|r.get(0))?;
            if !matches {
                return Err(Error::invalid(
                    "Restored checkpoint is detached from its durable acquisition",
                ));
            }
        }
    }
    Ok(rows.len())
}
