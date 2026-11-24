//! Raw bounded OpenRouter transport for evaluated embedding/link capabilities.
//! Retain the raw reply before capability-specific decoding. No implicit retry.
use crate::{
    error::{Error, Result},
    processing_ai,
};
use futures_util::StreamExt;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
pub struct Client {
    key: String,
    endpoint: url::Url,
    http: wreq::Client,
}
pub struct Receipt {
    pub status: u16,
    pub body: Vec<u8>,
    pub elapsed_ms: u128,
    pub retry_after: Option<String>,
    pub generation_id: Option<String>,
}
impl Client {
    pub fn embeddings(values: &BTreeMap<String, String>) -> Result<Self> {
        Self::build(values, true)
    }
    pub fn chat(values: &BTreeMap<String, String>) -> Result<Self> {
        Self::build(values, false)
    }
    fn build(values: &BTreeMap<String, String>, embeddings: bool) -> Result<Self> {
        let config = processing_ai::Config::from_mapping(values)
            .map_err(|e| Error::invalid(e.to_string()))?;
        let mut endpoint = config.endpoint;
        if embeddings {
            endpoint.set_path(&format!(
                "{}/embeddings",
                endpoint.path().trim_end_matches("/chat/completions")
            ));
        }
        let http = wreq::Client::builder()
            .no_proxy()
            .retry(wreq::retry::Policy::never())
            .redirect(wreq::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|_| Error::internal())?;
        Ok(Self {
            key: config.api_key,
            endpoint,
            http,
        })
    }
    pub async fn post(&self, payload: &Value, cancel: Arc<AtomicBool>) -> Result<Receipt> {
        let payload = payload.to_string();
        if payload.len() > 256 * 1024 {
            return Err(Error {
                code: "model_not_sent",
                ..Error::invalid("Model request exceeds byte limit; no request sent")
            });
        }
        let mut authorization =
            wreq::header::HeaderValue::from_str(&format!("Bearer {}", self.key)).map_err(|_| {
                Error {
                    code: "model_not_sent",
                    ..Error::invalid("Invalid OpenRouter configuration; no request sent")
                }
            })?;
        authorization.set_sensitive(true);
        if cancel.load(Ordering::Acquire) {
            return Err(not_sent());
        }
        let started = Instant::now();
        let dispatched = AtomicBool::new(false);
        let work = async {
            if cancel.load(Ordering::Acquire) {
                return Err(not_sent());
            }
            // After this point conservatively treat interrupted outcomes as
            // possibly billed, even if the HTTP stack never reaches the wire.
            dispatched.store(true, Ordering::Release);
            let response = self
                .http
                .post(self.endpoint.as_str())
                .header(wreq::header::AUTHORIZATION, authorization)
                .header("Content-Type", "application/json")
                .body(payload)
                .send()
                .await
                .map_err(|_| {
                    Error::invalid(
                        "OpenRouter request failed; billing is unknown, no automatic retry",
                    )
                })?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get(wreq::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .filter(|v| v.len() <= 128)
                .map(str::to_owned);
            let generation_id = ["x-generation-id", "x-openrouter-generation-id"]
                .iter()
                .find_map(|key| {
                    response
                        .headers()
                        .get(*key)
                        .and_then(|v| v.to_str().ok())
                        .filter(|v| !v.is_empty())
                        .map(|v| v.chars().take(256).collect())
                });
            if response
                .content_length()
                .is_some_and(|n| n > (2 * 1024 * 1024) as u64)
            {
                return Err(Error::invalid("OpenRouter response exceeds byte limit"));
            }
            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| {
                    Error::invalid("OpenRouter response interrupted; billing is unknown")
                })?;
                if bytes.len() + chunk.len() > (2 * 1024 * 1024) {
                    return Err(Error::invalid("OpenRouter response exceeds byte limit"));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(Receipt {
                status,
                body: bytes,
                elapsed_ms: started.elapsed().as_millis(),
                retry_after,
                generation_id,
            })
        };
        tokio::select! {
            result = work => result,
            _ = async { while !cancel.load(Ordering::Acquire) {tokio::time::sleep(Duration::from_millis(50)).await;} } => Err(if dispatched.load(Ordering::Acquire) { Error::cancelled() } else { not_sent() }),
        }
    }
}
fn not_sent() -> Error {
    Error {
        code: "model_not_sent",
        message: "Cancelled before model dispatch; no request sent".into(),
        ..Error::cancelled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn endpoint_selection_and_cancellation_do_not_send_traffic() {
        let values = [("OPENROUTER_API_KEY".into(), "fictional-not-a-key".into())]
            .into_iter()
            .collect();
        let embeddings = Client::embeddings(&values).unwrap();
        let chat = Client::chat(&values).unwrap();
        assert_eq!(
            embeddings.endpoint.as_str(),
            "https://openrouter.ai/api/v1/embeddings"
        );
        assert_eq!(
            chat.endpoint.as_str(),
            "https://openrouter.ai/api/v1/chat/completions"
        );
        match chat
            .post(&serde_json::json!({}), Arc::new(AtomicBool::new(true)))
            .await
        {
            Err(e) => assert_eq!(e.code, "model_not_sent"),
            Ok(_) => panic!("Cancelled request dispatched"),
        }
        match chat
            .post(
                &serde_json::json!({"input":"x".repeat(256*1024)}),
                Arc::new(AtomicBool::new(false)),
            )
            .await
        {
            Err(e) => assert_eq!(e.code, "model_not_sent"),
            Ok(_) => panic!("Oversized request dispatched"),
        }
        let mut invalid = chat;
        invalid.key = "fictional\ninvalid".into();
        match invalid
            .post(&serde_json::json!({}), Arc::new(AtomicBool::new(false)))
            .await
        {
            Err(e) => assert_eq!(e.code, "model_not_sent"),
            Ok(_) => panic!("Invalid header dispatched"),
        }
    }
}
