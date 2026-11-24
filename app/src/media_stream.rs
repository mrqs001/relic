//! A response-owned transfer: disconnect drops the provider future, while the
//! cancellation flag also stops validation running on a blocking storage worker.
use crate::error::Result;
use axum::body::Bytes;
use futures_util::Stream;
use serde_json::{Value, json};
use std::{
    convert::Infallible,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;

pub(crate) fn progress(phase: &str, downloaded: usize, total: Option<usize>) -> Value {
    json!({"kind":"progress", "phase":phase, "downloaded":downloaded, "total":total})
}

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(crate) fn download<F, Fut>(
    start: F,
) -> impl Stream<Item = std::result::Result<Bytes, Infallible>> + Send
where
    F: FnOnce(watch::Sender<Value>, Arc<AtomicBool>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<Value>> + Send,
{
    let guard = CancelOnDrop(Arc::new(AtomicBool::new(false)));
    let initial = progress("connecting", 0, None);
    let (tx, mut rx) = watch::channel(initial.clone());
    async_stream::stream! {
        let guard = guard;
        yield Ok(Bytes::from(format!("{initial}\n")));
        let transfer = start(tx, guard.0.clone());
        tokio::pin!(transfer);
        let mut ticks = tokio::time::interval(Duration::from_millis(100));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut heartbeat = 0;
        loop {
            tokio::select! {
                result = &mut transfer => {
                    let value = match result {
                        Ok(receipt) => json!({"kind":"ready", "receipt":receipt}),
                        Err(error) => json!({"kind":"error", "code":error.code, "message":error.message}),
                    };
                    yield Ok(Bytes::from(format!("{value}\n")));
                    break;
                }
                _ = ticks.tick() => {
                    heartbeat += 1;
                    // One latest value, at most ten progress updates per second;
                    // heartbeats detect abandoned connections during stalled RPCs.
                    if rx.has_changed().unwrap_or(false) || heartbeat >= 10 {
                        let value = rx.borrow_and_update().clone();
                        heartbeat = 0;
                        yield Ok(Bytes::from(format!("{value}\n")));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, routing::post};
    use futures_util::StreamExt;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        sync::Semaphore,
        time::timeout,
    };

    #[tokio::test]
    async fn progress_does_not_publish_until_ready_and_errors_are_terminal() {
        let stream = download(|tx, _| async move {
            tx.send_replace(progress("downloading", 512, Some(1024)));
            tokio::time::sleep(Duration::from_millis(150)).await;
            tx.send_replace(progress("verifying", 1024, Some(1024)));
            tokio::time::sleep(Duration::from_millis(150)).await;
            Ok(json!({"url":"/api/v1/media", "bytes":1024}))
        });
        tokio::pin!(stream);
        let mut events = Vec::new();
        while let Some(chunk) = stream.next().await {
            events.push(serde_json::from_slice::<Value>(&chunk.unwrap()).unwrap());
        }
        assert_eq!(events[0]["phase"], "connecting");
        assert!(
            events
                .iter()
                .any(|v| v["downloaded"] == 512 && v["total"] == 1024)
        );
        assert!(events.iter().any(|v| v["phase"] == "verifying"));
        assert_eq!(events.last().unwrap()["kind"], "ready");
        assert_eq!(events.iter().filter(|v| v["kind"] == "ready").count(), 1);
        let errors: Vec<_> =
            download(|_, _| async { Err(crate::error::Error::missing("Unavailable")) })
                .collect()
                .await;
        assert_eq!(errors.len(), 2);
        assert_eq!(
            serde_json::from_slice::<Value>(errors[1].as_ref().unwrap()).unwrap()["code"],
            "not_found"
        );
    }

    #[tokio::test]
    async fn actual_http_disconnect_cancels_future_removes_staging_and_releases_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_owned();
        let capacity = Arc::new(Semaphore::new(1));
        let observed = Arc::new(std::sync::Mutex::new(None::<Arc<AtomicBool>>));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/download",
            post({
                let capacity = capacity.clone();
                let observed = observed.clone();
                let path = path.clone();
                move || {
                    let capacity = capacity.clone();
                    let observed = observed.clone();
                    let path = path.clone();
                    async move {
                        Body::from_stream(download(move |tx, cancel| async move {
                            *observed.lock().unwrap() = Some(cancel);
                            let _permit = capacity.acquire_owned().await.unwrap();
                            let mut file = tempfile::NamedTempFile::new_in(path).unwrap();
                            std::io::Write::write_all(&mut file, &[1; 512]).unwrap();
                            tx.send_replace(progress("downloading", 512, Some(1024)));
                            std::future::pending::<()>().await;
                            drop(file);
                            Ok(Value::Null)
                        }))
                    }
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut socket = TcpStream::connect(address).await.unwrap();
        socket
            .write_all(b"POST /download HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        let mut received = String::new();
        timeout(Duration::from_secs(3), async {
            while !received.contains("\"downloaded\":512") {
                let mut buffer = [0; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                received.push_str(&String::from_utf8_lossy(&buffer[..n]));
            }
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 1);
        assert_eq!(capacity.available_permits(), 0);
        drop(socket);
        timeout(Duration::from_secs(3), async {
            loop {
                if observed
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .load(Ordering::Acquire)
                    && capacity.available_permits() == 1
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
        server.abort();
    }
}
