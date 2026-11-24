//! Bounded OpenRouter embedding transport; no scheduling, implicit retries or
//! evidence publication. Callers persist admission before making a paid request.
use crate::error::{Error, Result};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};

pub const MODEL: &str = "openai/text-embedding-3-small";
pub const DIMENSIONS: usize = 1536;
pub const PREPARATION: &str = "relic.event-claim-text.v1";
pub const BATCH: usize = 16;
pub const MAX_INPUT_BYTES: usize = 4096;

pub fn profile() -> Value {
    json!({"model":MODEL,"dimensions":DIMENSIONS,"preparation":PREPARATION,"encoding_format":"float","provider":{"only":["openai"],"allow_fallbacks":false}})
}
pub fn request(texts: &[String]) -> Result<Value> {
    if texts.is_empty()
        || texts.len() > BATCH
        || texts
            .iter()
            .any(|s| s.trim().is_empty() || s.len() > MAX_INPUT_BYTES)
    {
        return Err(Error::invalid(
            "Embedding inputs exceed the bounded claim batch",
        ));
    }
    Ok(
        json!({"model":MODEL,"dimensions":DIMENSIONS,"encoding_format":"float","provider":profile()["provider"],"input":texts}),
    )
}
pub fn decode(response: &Value, count: usize) -> Result<Vec<Vec<f32>>> {
    if count == 0
        || count > BATCH
        || ![MODEL, "text-embedding-3-small"]
            .iter()
            .any(|m| response["model"] == *m)
        || response["provider"] != "OpenAI"
        || response.get("error").is_some()
    {
        return Err(Error::invalid(
            "Embedding response model or status differs from the evaluated profile",
        ));
    }
    let data = response["data"]
        .as_array()
        .filter(|r| r.len() == count)
        .ok_or_else(|| Error::invalid("Incomplete embedding response"))?;
    let mut vectors: Vec<Option<Vec<f32>>> = vec![None; count];
    for row in data {
        let index = row["index"]
            .as_u64()
            .filter(|i| *i < count as u64)
            .ok_or_else(|| Error::invalid("Invalid embedding index"))? as usize;
        if vectors[index].is_some() {
            return Err(Error::invalid("Duplicate embedding index"));
        }
        let raw = row["embedding"]
            .as_array()
            .filter(|v| v.len() == DIMENSIONS)
            .ok_or_else(|| {
                Error::invalid("Embedding dimension differs from the evaluated profile")
            })?;
        let mut v = Vec::with_capacity(DIMENSIONS);
        let mut norm = 0.0_f64;
        for value in raw {
            let n = value
                .as_f64()
                .filter(|n| n.is_finite() && n.abs() <= 1.0)
                .ok_or_else(|| Error::invalid("Invalid embedding coordinate"))?;
            norm += n * n;
            v.push(n as f32);
        }
        if norm < 0.9 || norm > 1.1 {
            return Err(Error::invalid("Embedding is not a normalized vector"));
        }
        let norm = norm.sqrt() as f32;
        for n in &mut v {
            *n /= norm;
        }
        vectors[index] = Some(v);
    }
    vectors
        .into_iter()
        .map(|v| v.ok_or_else(|| Error::invalid("Missing embedding index")))
        .collect()
}
pub fn cosine(a: &[f32], b: &[f32]) -> Result<f64> {
    if a.len() != DIMENSIONS || b.len() != DIMENSIONS {
        return Err(Error::invalid("Incompatible embedding dimensions"));
    }
    let n: f64 = a
        .iter()
        .zip(b)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum();
    if !n.is_finite() {
        return Err(Error::invalid("Invalid embedding similarity"));
    }
    Ok(n.clamp(-1.0, 1.0))
}
pub struct Client(crate::model_transport::Client);
pub use crate::model_transport::Receipt;
impl Client {
    pub fn new(values: &BTreeMap<String, String>) -> Result<Self> {
        if values.get("AI_EMBEDDING_MODEL").is_some_and(|m| m != MODEL) {
            return Err(Error::invalid(
                "Embedding model requires a separate evaluated profile",
            ));
        }
        Ok(Self(crate::model_transport::Client::embeddings(values)?))
    }
    pub async fn embed(&self, texts: &[String], cancel: Arc<AtomicBool>) -> Result<Receipt> {
        self.0.post(&request(texts)?, cancel).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedding_protocol_requires_complete_indexed_compatible_vectors() {
        let mut v = vec![0.0; DIMENSIONS];
        v[0] = 1.0;
        let mut r = json!({"model":MODEL,"provider":"OpenAI","data":[{"index":1,"embedding":v},{"index":0,"embedding":v}]});
        let decoded = decode(&r, 2).unwrap();
        assert_eq!(cosine(&decoded[0], &decoded[1]).unwrap(), 1.0);
        r["data"][1]["index"] = json!(1);
        assert!(decode(&r, 2).is_err());
        r["data"][1]["index"] = json!(0);
        r["model"] = json!("other/model");
        assert!(decode(&r, 2).is_err());
        r["model"] = json!(MODEL);
        r["data"][0]["embedding"] = json!([0.0]);
        assert!(decode(&r, 2).is_err());
        assert!(request(&vec!["ok".into(); BATCH + 1]).is_err());
        assert!(request(&["".into()]).is_err());
        assert_eq!(
            request(&["Fictional fire at North Pier".into()]).unwrap()["provider"]["allow_fallbacks"],
            false
        );
    }
}
