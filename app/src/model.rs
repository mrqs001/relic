use crate::error::{Error, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
pub fn ident(prefix: &str) -> String {
    format!("{}:{}", prefix, uuid::Uuid::new_v4())
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn required<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| Error::invalid(format!("{key} must be a nonempty string")))
}
pub fn bounded_text(v: &Value, key: &str, max: usize) -> Result<String> {
    let s = required(v, key)?;
    if s.len() > max {
        return Err(Error::invalid(format!("{key} exceeds {max} bytes")));
    }
    Ok(s.into())
}
pub fn timestamp(s: &str) -> Result<String> {
    DateTime::parse_from_rfc3339(s)
        .map(|x| {
            x.with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        })
        .map_err(|_| Error::invalid("Timestamp requires RFC3339 with a timezone"))
}
pub fn value<T: Serialize>(v: T) -> Value {
    serde_json::to_value(v).expect("serializable domain value")
}
pub fn string(v: &Value) -> String {
    serde_json::to_string(v).expect("JSON value")
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Citation {
    pub evidence_id: String,
    pub revision: String,
    pub start_character: usize,
    pub end_character: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scope {
    pub source_ids: Vec<String>,
    pub include_unlocated: bool,
    pub query: String,
    pub time_basis: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub bbox: Option<[f64; 4]>,
    pub publication_id: Option<String>,
    pub content_view: String,
    pub event_type: Option<String>,
    pub related_key: Option<String>,
}
impl Default for Scope {
    fn default() -> Self {
        Self {
            source_ids: vec![],
            include_unlocated: true,
            query: String::new(),
            time_basis: "publication_time".into(),
            from: None,
            to: None,
            bbox: None,
            publication_id: None,
            content_view: "all".into(),
            event_type: None,
            related_key: None,
        }
    }
}
impl Scope {
    pub fn parse(v: &Value) -> Result<Self> {
        let mut s: Self = serde_json::from_value(v.clone())?;
        s.source_ids.sort();
        s.source_ids.dedup();
        if !["all", "signals", "events"].contains(&s.content_view.as_str()) {
            return Err(Error::invalid(
                "Content view must be all, signals or events",
            ));
        }
        if s.event_type
            .as_ref()
            .is_some_and(|t| !crate::processing_ai::EVENT_TYPES.contains(&t.as_str()))
        {
            return Err(Error::invalid("Unsupported event type"));
        }
        if s.related_key
            .as_ref()
            .is_some_and(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(Error::invalid("Invalid related-reporting key"));
        }
        if s.source_ids.len() > 100 || s.query.len() > 500 {
            return Err(Error::invalid("Scope exceeds source or query limit"));
        }
        if !["publication_time", "acquisition_time"].contains(&s.time_basis.as_str()) {
            return Err(Error::invalid("Unsupported scope time basis"));
        }
        s.from = s.from.as_deref().map(timestamp).transpose()?;
        s.to = s.to.as_deref().map(timestamp).transpose()?;
        if s.from
            .as_ref()
            .zip(s.to.as_ref())
            .is_some_and(|(f, t)| f > t)
        {
            return Err(Error::invalid("Scope start is after end"));
        }
        if let Some([w, south, e, n]) = s.bbox
            && (![w, south, e, n].iter().all(|x| x.is_finite())
                || w.abs() > 180.
                || e.abs() > 180.
                || south.abs() > 90.
                || n.abs() > 90.
                || south > n)
        {
            return Err(Error::invalid("Invalid geographic bounds"));
        }
        Ok(s)
    }
    pub fn fixture(v: &Value) -> Result<Self> {
        let mut o = json!({"source_ids":v["source_ids"],"include_unlocated":v["include_unlocated"],"time_basis":v["time_basis"]});
        if let Some(a) = v["time_interval"].as_array() {
            if a.len() != 2 {
                return Err(Error::invalid(
                    "Fixture time_interval requires exactly two endpoints",
                ));
            }
            o["from"] = a[0].clone();
            o["to"] = a[1].clone()
        }
        Self::parse(&o)
    }
}
#[derive(Debug, Deserialize)]
pub struct Query {
    pub scope: Value,
    #[serde(default)]
    pub generation: u64,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}
