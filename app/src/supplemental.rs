//! Independent supplementary lane. No code here writes readings, media links,
//! coordinates, feed eligibility, or acquisition cursors.
use crate::{
    error::{Error, Result},
    model::*,
    store::{Store, edge, node},
    supplemental_ai::{self as ai, Context},
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

pub const CONCURRENCY: usize = 16;
pub const RESERVE_USD: f64 = 0.002;
pub struct Work {
    pub rowid: i64,
    pub publication: String,
    pub phase: String,
    pub context: Context,
    pub request: Value,
}
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| Error::internal())?
}
impl Store {
    pub fn supplemental_enabled(&self) -> Result<bool> {
        Ok(self
            .connect()?
            .query_row(
                "SELECT value FROM metadata WHERE key='supplemental_enabled'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .as_deref()
            == Some(ai::RECIPE))
    }
    pub fn set_supplemental_enabled(&self, enabled: bool) -> Result<Value> {
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        self.connect()?.execute("INSERT INTO metadata VALUES('supplemental_enabled',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [if enabled {ai::RECIPE} else {""}])?;
        drop(_lock);
        self.supplemental_status()
    }
    pub fn supplemental_status(&self) -> Result<Value> {
        let c = self.connect()?;
        let day = now()[..10].to_string();
        let (calls, cost, unknown, reserved):(i64,f64,i64,f64) = c.query_row("SELECT count(*),coalesce(sum(cost_usd),0),coalesce(sum(cost_usd IS NULL),0),coalesce(sum(coalesce(cost_usd,reserved_usd)),0) FROM supplemental_attempts WHERE day=?1", [&day], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        let size_excluded: i64 = c.query_row("SELECT count(*) FROM evidence e JOIN membership m ON m.evidence_rowid=e.rowid WHERE m.publication_id=(SELECT value FROM metadata WHERE key='current_publication') AND (length(e.text)>16000 OR length(cast(e.text AS BLOB))>32000)", [], |r|r.get(0))?;
        let request_excluded: i64 = c.query_row(
            "SELECT count(*) FROM supplemental_exclusions WHERE recipe=?1",
            [ai::RECIPE],
            |r| r.get(0),
        )?;
        // One pass over the attempts: with a single max(), SQLite takes the bare
        // `error` from the latest attempt (NULL when there is none).
        let (complete, last): (i64, Option<String>) = c.query_row(
            "SELECT coalesce(sum(status='completed'),0),max(created_at),error FROM supplemental_attempts",
            [],
            |r| Ok((r.get(0)?, r.get(2)?)),
        )?;
        let notice: Option<String> = c
            .query_row(
                "SELECT value FROM metadata WHERE key='supplemental_error'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let running = self
            .running
            .lock()
            .map_err(|_| Error::internal())?
            .keys()
            .any(|id| id.starts_with("processing:supplemental:"));
        Ok(
            json!({"enabled":self.supplemental_enabled()?,"running":running,"completed":complete,"text_size_excluded":size_excluded,"request_excluded":request_excluded,"today":{"calls":calls,"limit":null,"known_cost_usd":cost,"unknown_cost_requests":unknown,"admission_reserved_usd":reserved,"spend_limit_usd":null},"last_error":notice.or(last),"concurrency":CONCURRENCY,"mode":"supplementary_only","model":ai::MODEL,"recipe":ai::RECIPE}),
        )
    }
    pub fn next_supplemental(&self) -> Result<Option<Work>> {
        if !self.supplemental_enabled()? || self.shutdown.load(Ordering::Acquire) {
            return Ok(None);
        }
        let policy = self.processing_policy()?;
        if !policy.enabled {
            return Ok(None);
        }
        let c = self.connect()?;
        let publication = Self::latest(&c)?.unwrap_or_default();
        // Provider failures pause dispatch; a paid, invalid classification affects
        // only that exact input. Never retry either context automatically.
        let cooling:bool = c.query_row("SELECT EXISTS(SELECT 1 FROM metadata WHERE key='supplemental_backoff_until' AND julianday(value)>julianday(?1))", [now()], |r|r.get(0))?;
        if cooling {
            return Ok(None);
        }
        let count: i64 = c.query_row("SELECT count(*) FROM supplemental_attempts", [], |r| {
            r.get(0)
        })?;
        let order = if count % 5 == 4 { "ASC" } else { "DESC" };
        let sql = format!(
            "SELECT e.rowid,e.json,i.json FROM evidence e JOIN membership m ON m.evidence_rowid=e.rowid LEFT JOIN interpretation_intervals pi ON pi.evidence_rowid=e.rowid AND pi.from_seq<=(SELECT seq FROM publication_order WHERE id=m.publication_id) AND (pi.until_seq IS NULL OR pi.until_seq>(SELECT seq FROM publication_order WHERE id=m.publication_id)) LEFT JOIN interpretations i ON i.id=pi.interpretation_id WHERE m.publication_id=?1 AND e.source IN (SELECT value FROM json_each(?2)) AND length(e.text)<=16000 AND length(cast(e.text AS BLOB))<=32000 AND trim(e.text,' '||char(9,10,13))!='' AND coalesce(json_extract(e.json,'$.upstream_state'),'available')!='removed' AND julianday(e.acquired_at)<=julianday(?3)-3.0/1440.0 AND julianday(coalesce(e.published_at,e.acquired_at))>=julianday(?3)-coalesce((SELECT CAST(value AS REAL) FROM metadata WHERE key='processing_max_age_days'),1e9) AND NOT EXISTS(SELECT 1 FROM supplemental_attempts a WHERE a.evidence_rowid=e.rowid AND a.recipe=?4 AND a.phase=CASE WHEN i.id IS NULL THEN 'initial' ELSE 'enriched' END) AND NOT EXISTS(SELECT 1 FROM supplemental_exclusions x WHERE x.evidence_rowid=e.rowid AND x.recipe=?4 AND x.phase=CASE WHEN i.id IS NULL THEN 'initial' ELSE 'enriched' END) ORDER BY coalesce(e.published_at,e.acquired_at) {order},e.rowid {order} LIMIT 1"
        );
        let row: Option<(i64, String, Option<String>)> = c
            .query_row(
                &sql,
                params![
                    publication,
                    string(&value(policy.eligible_source_ids)),
                    now(),
                    ai::RECIPE
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((rowid, raw, interpretation)) = row else {
            return Ok(None);
        };
        let e: Value = serde_json::from_str(&raw)?;
        let reading: Value = interpretation
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or(Value::Null);
        let mut neighbours = vec![];
        let mut stmt = c.prepare("SELECT e.json FROM evidence e JOIN membership m ON m.evidence_rowid=e.rowid WHERE m.publication_id=?1 AND e.source=?2 AND e.rowid!=?3 AND length(e.text)<=4000 AND coalesce(json_extract(e.json,'$.upstream_state'),'available')!='removed' AND abs(unixepoch(e.published_at)-unixepoch(?4))<=180 ORDER BY abs(unixepoch(e.published_at)-unixepoch(?4)),e.rowid LIMIT 3")?;
        for raw in stmt.query_map(
            params![
                publication,
                e["source"].as_str(),
                rowid,
                e["published_at"].as_str()
            ],
            |r| r.get::<_, String>(0),
        )? {
            neighbours.push(message(&serde_json::from_str::<Value>(&raw?)?));
        }
        let mut locations = vec![];
        let mut all = reading["locations"].as_array().cloned().unwrap_or_default();
        for event in reading["events"].as_array().into_iter().flatten() {
            all.extend(event["locations"].as_array().cloned().unwrap_or_default());
        }
        for loc in all {
            if locations.len() == 4 {
                break;
            }
            if loc["selected_candidate_id"].is_string()
                || locations
                    .iter()
                    .any(|p: &Value| p["mention"] == loc["mention"] && p["quote"] == loc["quote"])
            {
                continue;
            }
            let candidates:Vec<Value> = loc["candidates"].as_array().into_iter().flatten().take(20).map(|p| json!({"id":p["id"],"name":p["name"],"kind":p["kind"],"country_code":p["country_code"],"country_name":p["country_name"],"reference":p["reference"],"matched_alias":p["matched_alias"]})).collect();
            if !candidates.is_empty() {
                locations.push(json!({"mention":loc["mention"],"quote":loc["quote"],"role":loc["role"],"status":loc["status"],"candidates_truncated":loc["candidates_truncated"],"candidates":candidates}));
            }
        }
        let mut context = Context {
            message: message(&e),
            neighbours,
            locations,
            interpretation_id: reading["id"].as_str().map(str::to_string),
            omitted_locations: 0,
            omitted_neighbours: 0,
        };
        // Drop whole optional context items, never truncate the source text or
        // claim that unseen items were assessed. Omission counts are retained.
        let mut request = ai::request(&context);
        while request.is_err() && (!context.neighbours.is_empty() || !context.locations.is_empty())
        {
            if !context.locations.is_empty() {
                context.locations.pop();
                context.omitted_locations += 1;
            } else {
                context.neighbours.pop();
                context.omitted_neighbours += 1;
            }
            request = ai::request(&context);
        }
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                let _lock = self.writer.lock().map_err(|_| Error::internal())?;
                c.execute(
                    "INSERT OR IGNORE INTO supplemental_exclusions VALUES(?1,?2,?3,?4)",
                    params![
                        rowid,
                        ai::RECIPE,
                        if context.interpretation_id.is_some() {
                            "enriched"
                        } else {
                            "initial"
                        },
                        error.message
                    ],
                )?;
                return Ok(None);
            }
        };
        Ok(Some(Work {
            rowid,
            publication,
            phase: if context.interpretation_id.is_some() {
                "enriched"
            } else {
                "initial"
            }
            .into(),
            context,
            request,
        }))
    }
    pub fn reserve_supplemental(&self, work: &Work) -> Result<String> {
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        if !self.supplemental_enabled()?
            || !self.processing_policy()?.enabled
            || !self
                .processing_policy()?
                .eligible_source_ids
                .iter()
                .any(|id| Some(id.as_str()) == work.context.message["source"].as_str())
        {
            return Err(Error::conflict(
                "Supplemental processing is paused or source is ineligible",
            ));
        }
        let retained: Value = serde_json::from_str(&tx.query_row(
            "SELECT json FROM evidence WHERE rowid=?1",
            [work.rowid],
            |r| r.get::<_, String>(0),
        )?)?;
        if ["id", "revision", "source", "text"]
            .iter()
            .any(|key| retained[*key] != work.context.message[*key])
            || ai::request(&work.context)? != work.request
        {
            return Err(Error::conflict(
                "Supplemental input does not match its retained source revision",
            ));
        }
        let current:bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM membership WHERE publication_id=(SELECT value FROM metadata WHERE key='current_publication') AND evidence_rowid=?1)",[work.rowid],|r|r.get(0))?;
        if !current {
            return Err(Error::conflict("Supplemental input is no longer current"));
        }
        let day = now()[..10].to_string();
        let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM supplemental_attempts WHERE evidence_rowid=?1 AND recipe=?2 AND phase=?3)",params![work.rowid,ai::RECIPE,work.phase],|r|r.get(0))?;
        if exists {
            return Err(Error::conflict(
                "Supplemental context already attempted; no automatic retry",
            ));
        }
        let request = serde_json::to_vec(&work.request)?;
        let h = self.write_object(&request)?;
        self.sync_object_directories(std::iter::once(h.clone()), &AtomicBool::new(false))?;
        tx.execute(
            "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
            params![h, request.len() as i64],
        )?;
        let id = ident("supplement");
        tx.execute("INSERT INTO supplemental_attempts(id,evidence_rowid,recipe,phase,context_hash,publication_id,interpretation_id,created_at,day,status,reserved_usd,request_hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'reserved',?10,?11)",params![id,work.rowid,ai::RECIPE,work.phase,hash(&serde_json::to_vec(&work.context)?),work.publication,work.context.interpretation_id,now(),day,RESERVE_USD,h])?;
        tx.commit()?;
        Ok(id)
    }
    pub fn finish_supplemental(
        &self,
        id: &str,
        audit: &Value,
        failure: Option<&str>,
    ) -> Result<()> {
        let _lock = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let (h,rowid,publication):(String,i64,String)=tx.query_row("SELECT request_hash,evidence_rowid,publication_id FROM supplemental_attempts WHERE id=?1 AND status='reserved'",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        let request: Value = serde_json::from_slice(&self.original_object(&h)?)?;
        let checked = if let Some(error) = failure {
            Err(Error::invalid(error))
        } else {
            ai::accepted(&request, audit)
        };
        let cost = ai::cost(&audit["response"]);
        let bytes = serde_json::to_vec(audit)?;
        let audit_hash = self.write_object(&bytes)?;
        self.sync_object_directories(std::iter::once(audit_hash.clone()), &AtomicBool::new(false))?;
        tx.execute(
            "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
            params![audit_hash, bytes.len() as i64],
        )?;
        if checked.is_ok() {
            tx.execute("DELETE FROM metadata WHERE key='supplemental_error'", [])?;
        }
        let provider_failure = match failure {
            Some(message) => message != Error::cancelled().message,
            None => audit["http_status"] != 200 || audit["response"]["model"] != ai::SERVED_MODEL,
        };
        if provider_failure {
            let until = (chrono::Utc::now() + chrono::Duration::minutes(5))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            tx.execute("INSERT INTO metadata VALUES('supplemental_backoff_until',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [until])?;
        }
        let result = checked.as_ref().ok().map(string);
        let error = checked.as_ref().err().map(|e| e.message.clone());
        let status = if result.is_some() {
            "completed"
        } else if cost.is_some() {
            "failed"
        } else {
            "unknown"
        };
        tx.execute("UPDATE supplemental_attempts SET status=?2,cost_usd=?3,audit_hash=?4,result_json=?5,error=?6 WHERE id=?1 AND status='reserved'",params![id,status,cost,audit_hash,result,error])?;
        if status == "completed" {
            crate::jev_hints::refresh(&tx, Some(rowid))?;
        }
        let input: Value = serde_json::from_str(&tx.query_row(
            "SELECT json FROM evidence WHERE rowid=?1",
            [rowid],
            |r| r.get::<_, String>(0),
        )?)?;
        node(
            &tx,
            id,
            "supplementary",
            "Jev supplementary decisions",
            &json!({"model":ai::MODEL,"recipe":ai::RECIPE,"status":status,"request_hash":h,"audit_hash":audit_hash,"input_publication_id":publication,"answers":checked.ok(),"effect":"Advisory; a qualifying event type can add a marked possible event to Event reports. No evidence, reading, media link or coordinate changes"}),
        )?;
        edge(
            &tx,
            &format!(
                "{}@{}",
                required(&input, "id")?,
                required(&input, "revision")?
            ),
            id,
            "supplementary input",
            &json!({"exact_revision":true}),
        )?;
        for neighbour in request["state"]["neighbours"]
            .as_array()
            .into_iter()
            .flatten()
        {
            edge(
                &tx,
                &format!(
                    "{}@{}",
                    required(neighbour, "id")?,
                    required(neighbour, "revision")?
                ),
                id,
                "comparison input",
                &json!({"exact_revision":true}),
            )?;
        }
        if let Some(reading) = request["state"]["interpretation_id"].as_str() {
            edge(
                &tx,
                reading,
                id,
                "candidate context",
                &json!({"advisory_only":true}),
            )?;
        }
        let event = Self::event(
            &tx,
            "supplemental",
            Some(&publication),
            vec![format!(
                "{}@{}",
                required(&input, "id")?,
                required(&input, "revision")?
            )],
        )?;
        tx.commit()?;
        let _ = self.events.send(event);
        Ok(())
    }
    pub fn supplemental_detail(
        &self,
        id: &str,
        revision: &str,
        interpretation: Option<&str>,
        historical: bool,
        publication: &str,
    ) -> Result<Value> {
        let c = self.connect()?;
        let raw:Option<(String,String,String,String,String,String)>=c.query_row("SELECT a.id,a.result_json,a.request_hash,a.audit_hash,a.created_at,a.phase FROM supplemental_attempts a JOIN evidence e ON e.rowid=a.evidence_rowid WHERE e.id=?1 AND e.revision=?2 AND a.status='completed' AND (a.interpretation_id=?3 OR a.interpretation_id IS NULL) AND (?4=0 OR a.created_at<=(SELECT created_at FROM publications WHERE id=?5)) ORDER BY a.created_at DESC LIMIT 1",params![id,revision,interpretation,historical,publication],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
        let Some((id, answers, request_hash, audit_hash, created_at, phase)) = raw else {
            return Ok(Value::Null);
        };
        let request: Value = serde_json::from_slice(&self.original_object(&request_hash)?)?;
        Ok(
            json!({"id":id,"answers":serde_json::from_str::<Value>(&answers)?,"context":request["state"],"request_hash":request_hash,"audit_hash":audit_hash,"created_at":created_at,"phase":phase,"model":ai::MODEL,"mode":"supplementary_only"}),
        )
    }
}
fn message(e: &Value) -> Value {
    json!({"id":e["id"],"revision":e["revision"],"source":e["source"],"text":e["text"],"published_at":e["published_at"],"media_count":e["media"].as_array().map_or(0,Vec::len),"relationships":e["relationships"]})
}
/// Independent request pool. Reserve each input before selecting another.
pub fn start(store: Store) {
    tokio::spawn(async move {
        loop {
            if store.shutdown.load(Ordering::Acquire) {
                return;
            }
            let s = store.clone();
            if blocking(move || Ok(s.supplemental_enabled()? && s.processing_policy()?.enabled))
                .await
                .unwrap_or(false)
            {
                match ai::Client::new(&store.config.source_values) {
                    Ok(client) => {
                        run_pool(store, Arc::new(client)).await;
                        return;
                    }
                    Err(error) => save_notice(&store, error).await,
                }
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });
}
async fn run_pool(store: Store, client: Arc<ai::Client>) {
    loop {
        if store.shutdown.load(Ordering::Acquire) {
            break;
        }
        let job_id = format!("processing:supplemental:{}", ident("pool"));
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut running) = store.running.lock() {
            running.insert(job_id.clone(), cancel.clone());
        } else {
            break;
        }
        let mut active = tokio::task::JoinSet::new();
        loop {
            while active.len() < CONCURRENCY
                && !cancel.load(Ordering::Acquire)
                && !store.shutdown.load(Ordering::Acquire)
            {
                let s = store.clone();
                let flag = cancel.clone();
                let selected = blocking(move || {
                    if flag.load(Ordering::Acquire) {
                        return Ok(None);
                    }
                    let Some(work) = s.next_supplemental()? else {
                        return Ok(None);
                    };
                    if flag.load(Ordering::Acquire) {
                        return Ok(None);
                    }
                    let id = s.reserve_supplemental(&work)?;
                    Ok(Some((work, id)))
                })
                .await;
                let (work, id) = match selected {
                    Ok(Some(work)) => work,
                    Ok(None) => break,
                    Err(error) => {
                        save_notice(&store, error).await;
                        break;
                    }
                };
                let store = store.clone();
                let client = client.clone();
                let cancel = cancel.clone();
                active.spawn(async move {
                    if let Err(error) = run_one(&store, &client, &work, &id, &cancel).await {
                        save_notice(&store, error).await;
                    }
                });
            }
            match active.join_next().await {
                None => break,
                Some(Err(_)) => {
                    cancel.store(true, Ordering::Release);
                    save_notice(&store, Error::internal()).await;
                }
                Some(Ok(())) => {}
            }
        }
        if let Ok(mut running) = store.running.lock() {
            running.remove(&job_id);
        }
        tokio::time::sleep(Duration::from_secs(15)).await;
    }
}
async fn save_notice(store: &Store, error: Error) {
    let s = store.clone();
    let _ = blocking(move || {
        s.connect()?.execute("INSERT INTO metadata VALUES('supplemental_error',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [error.message])?;
        Ok(())
    }).await;
}
async fn run_one(
    store: &Store,
    client: &ai::Client,
    work: &Work,
    attempt: &str,
    cancel: &AtomicBool,
) -> Result<()> {
    let source = work.context.message["source"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    let stop = async {
        loop {
            if cancel.load(Ordering::Acquire) || store.halt.load(Ordering::Acquire) {
                break;
            }
            let s = store.clone();
            let source = source.clone();
            if !blocking(move || {
                Ok(s.supplemental_enabled()?
                    && s.processing_policy()?.eligible_source_ids.contains(&source))
            })
            .await
            .unwrap_or(false)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    };
    let result = tokio::select! {biased; _=stop=>Err(Error::cancelled()), result=tokio::time::timeout(ai::DEADLINE,client.decide(&work.request))=>match result{Ok(r)=>r,Err(_)=>Err(Error::invalid("Jev exceeded Relic's 30-second deadline; no retry sent"))}};
    let (audit, error) = match result {
        Ok(audit) => (audit, None),
        Err(e) => (Value::Null, Some(e.message)),
    };
    let s = store.clone();
    let id = attempt.to_owned();
    blocking(move || s.finish_supplemental(&id, &audit, error.as_deref())).await
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    #[derive(Clone)]
    struct Mock {
        calls: Arc<AtomicUsize>,
        gate: Arc<tokio::sync::Semaphore>,
    }
    async fn reply(
        axum::extract::State(mock): axum::extract::State<Mock>,
        axum::Json(body): axum::Json<Value>,
    ) -> axum::Json<Value> {
        mock.calls.fetch_add(1, Ordering::SeqCst);
        mock.gate.acquire().await.unwrap().forget();
        let mut answers = json!({});
        for (key, q) in body["questions"].as_object().unwrap() {
            let criteria = q["criteria"].as_object().unwrap();
            let chosen = if criteria.contains_key("uncertain") {
                "uncertain"
            } else {
                criteria.keys().next().unwrap()
            };
            let probabilities: serde_json::Map<String, Value> = criteria
                .keys()
                .map(|k| (k.clone(), json!(if k == chosen { 1.0 } else { 0.0 })))
                .collect();
            answers[key] = json!({"type":"choice","choice":chosen,"probabilities":probabilities,"confidence":0.8});
        }
        axum::Json(json!({"model":ai::SERVED_MODEL,"answers":answers,"usage":{"cost":0.0001}}))
    }
    async fn wait(mut ready: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(20), async {
            while !ready() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn supplementary_pool_overlaps_refills_and_honors_stop_without_duplicates() {
        for cancel in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .to_path_buf();
            let s = Store::open(crate::config::Config::local(
                root,
                tmp.path().join("data"),
                "127.0.0.1:8793".parse().unwrap(),
            ))
            .unwrap();
            let rows:Vec<Value>=(0..40).map(|n|json!({"id":format!("fiction:{n}"),"revision":"v1","source":"fiction:alpha","published_at":"2026-09-10T12:00:00Z","acquired_at":"2026-09-10T12:01:00Z","text":"Fictional report.","geometry":null,"location_precision":"unresolved","origin_group":null,"dependence":"unknown","language":"en","fictional":true,"availability":"available","citation":{"evidence_id":format!("fiction:{n}"),"revision":"v1","start_character":0,"end_character":17}})).collect();
            s.import_job(json!({"schema_version":1,"fictional":true,"title":"Concurrent supplementary fixture","sources":[{"id":"fiction:alpha","label":"Fictional Alpha","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
            s.set_processing_policy(crate::processing_store::Policy {
                enabled: true,
                eligible_source_ids: vec!["fiction:alpha".into()],
            })
            .unwrap();
            s.set_supplemental_enabled(true).unwrap();
            let mock = Mock {
                calls: Arc::new(AtomicUsize::new(0)),
                gate: Arc::new(tokio::sync::Semaphore::new(0)),
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = ai::Client::fixture(
                format!("http://{}/decisions", listener.local_addr().unwrap())
                    .parse()
                    .unwrap(),
            );
            let app = axum::Router::new()
                .route("/decisions", axum::routing::post(reply))
                .with_state(mock.clone());
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let worker = tokio::spawn(run_pool(s.clone(), Arc::new(client)));
            wait(|| mock.calls.load(Ordering::SeqCst) == CONCURRENCY).await;
            if cancel {
                crate::processing::Runtime::new(s.clone())
                    .stop()
                    .await
                    .unwrap();
                wait(|| {
                    !s.supplemental_status().unwrap()["running"]
                        .as_bool()
                        .unwrap()
                })
                .await;
                assert_eq!(mock.calls.load(Ordering::SeqCst), CONCURRENCY);
                assert!(
                    s.next_supplemental().unwrap().is_some(),
                    "manual cancellation must not impose provider backoff"
                );
            } else {
                mock.gate.add_permits(40);
                wait(|| s.supplemental_status().unwrap()["completed"] == 40).await;
                assert_eq!(mock.calls.load(Ordering::SeqCst), 40);
            }
            let c = s.connect().unwrap();
            let reserved: i64 = c
                .query_row(
                    "SELECT count(*) FROM supplemental_attempts WHERE status='reserved'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(reserved, 0);
            let duplicates:i64=c.query_row("SELECT count(*) FROM (SELECT evidence_rowid,recipe,phase FROM supplemental_attempts GROUP BY 1,2,3 HAVING count(*)>1)",[],|r|r.get(0)).unwrap();
            assert_eq!(duplicates, 0);
            s.shutdown.store(true, Ordering::Release);
            worker.abort();
            mock.gate.add_permits(40);
            server.abort();
        }
    }
}
