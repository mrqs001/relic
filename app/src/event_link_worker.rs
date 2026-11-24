//! One bounded optional relationship lane sharing source eligibility and Pause.
use crate::{
    error::{Error, Result},
    embeddings, event_link_store, event_links,
    store::Store,
};
use futures_util::{StreamExt, stream};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| Error::internal())?
}
pub fn start(store: Store) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut idle_version = None;
        loop {
            tick.tick().await;
            if store.shutdown.load(Ordering::Acquire) {
                break;
            }
            let s = store.clone();
            if !blocking(move || s.event_links_ready())
                .await
                .unwrap_or(false)
            {
                idle_version = None;
                continue;
            }
            let s = store.clone();
            let Ok(version) = blocking(move || s.event_link_work_version()).await else {
                continue;
            };
            if idle_version.as_ref() == Some(&version) {
                continue;
            }
            let clients = event_links::Client::new(&store.config.source_values).and_then(|c| {
                Ok((c, embeddings::Client::new(&store.config.source_values)?))
            });
            let (client, embedder) = match clients {
                Ok(c) => c,
                Err(e) => {
                    let s = store.clone();
                    let _ = blocking(move || s.record_event_link_error(&e.message)).await;
                    continue;
                }
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let id = crate::model::ident("processing:relationships");
            if let Ok(mut running) = store.running.lock() {
                running.insert(id.clone(), cancel.clone());
            }
            let work = run_page(store.clone(), client, embedder, cancel.clone());
            tokio::pin!(work);
            // Drain dispatched work after shutdown. The same flag is reachable by Stop
            // and policy changes; HTTP cancellation never implies a free provider call.
            let mut check = tokio::time::interval(Duration::from_millis(100));
            loop {
                tokio::select! {
                 _=check.tick()=>{if store.halt.load(Ordering::Acquire){cancel.store(true,Ordering::Release);}},
                 result=&mut work=>{if let Err(e)=result{let s=store.clone();let _=blocking(move||s.record_event_link_error(&e.message)).await;}break;}
                }
            }
            if let Ok(mut running) = store.running.lock() {
                running.remove(&id);
            }
            let s = store.clone();
            if blocking(move || s.event_link_work_version())
                .await
                .is_ok_and(|after| after == version)
            {
                idle_version = Some(version);
            }
        }
    });
}
async fn run_page(
    store: Store,
    client: event_links::Client,
    embedder: embeddings::Client,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
    let s = store.clone();
    let flag = cancel.clone();
    blocking(move || s.index_event_claims(&flag)).await?;
    // Semantic retrieval: embed new claim texts (sequential bounded requests;
    // any failure stops before the next dispatch), then scan neighbours.
    let s = store.clone();
    let flag = cancel.clone();
    let batches = blocking(move || s.reserve_claim_embeddings(&flag)).await?;
    let mut stopped = false;
    for batch in batches {
        let response = if stopped {
            Err(Error {
                code: "model_not_sent",
                ..Error::invalid("Batch stopped before dispatch")
            })
        } else {
            embedder.embed(&batch.texts, cancel.clone()).await
        };
        stopped |= response.as_ref().is_err() || response.as_ref().is_ok_and(|r| r.status != 200);
        let s = store.clone();
        blocking(move || match response {
            Ok(r) => s.finish_claim_embeddings(&batch, Some(r.status), &r.body, None),
            Err(e) => s.finish_claim_embeddings(&batch, None, &[], Some((e.code, &e.message))),
        })
        .await?;
    }
    if stopped || cancel.load(Ordering::Acquire) {
        return Ok(());
    }
    let s = store.clone();
    let flag = cancel.clone();
    blocking(move || s.scan_semantic_neighbours(&flag)).await?;
    let s = store.clone();
    let flag = cancel.clone();
    let pairs = blocking(move || s.admit_event_pairs(&flag)).await?;
    let stop_refill = AtomicBool::new(false);
    let mut pending = stream::iter(pairs.into_iter().map(|pair| {
        let s = store.clone();
        let client = &client;
        let flag = cancel.clone();
        let stop_refill = &stop_refill;
        async move {
            if stop_refill.load(Ordering::Acquire) {
                return blocking(move || {
                    s.finish_event_pair(
                        &pair,
                        None,
                        &[],
                        Some(("model_not_sent", "Batch stopped before dispatch")),
                        None,
                    )
                })
                .await;
            }
            let response = client.review(&pair.a.claim, &pair.b.claim, flag).await;
            if response.as_ref().is_err() || response.as_ref().is_ok_and(|r| r.status != 200) {
                stop_refill.store(true, Ordering::Release);
            }
            blocking(move || match response {
                Ok(r) => s.finish_event_pair(
                    &pair,
                    Some(r.status),
                    &r.body,
                    r.generation_id.as_deref().map(|id| ("generation_id", id)),
                    r.retry_after.as_deref(),
                ),
                Err(e) => s.finish_event_pair(&pair, None, &[], Some((e.code, &e.message)), None),
            })
            .await
        }
    }))
    .buffer_unordered(event_link_store::CONCURRENCY);
    let mut failure = None;
    while let Some(result) = pending.next().await {
        if let Err(e) = result {
            stop_refill.store(true, Ordering::Release);
            failure = Some(e);
        }
    }
    drop(pending);
    if let Some(e) = failure {
        return Err(e);
    }
    if cancel.load(Ordering::Acquire) {
        return Ok(());
    }
    let s = store;
    blocking(move || s.publish_event_links(&cancel)).await?;
    Ok(())
}
