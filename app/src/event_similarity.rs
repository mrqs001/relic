//! Conservative pair suggestions, not incident merges or confirmation.
use crate::{embeddings, error::Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub const POLICY: &str = "relic.possible-event-links.v1";
pub const MIN_COSINE: f64 = 0.88;
pub const WINDOW_SECONDS: i64 = 6 * 60 * 60;
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Claim {
    pub text: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub state: String,
    pub place: Option<String>,
    pub published_at: Option<String>,
    pub time_text: Option<String>,
}
pub fn profile() -> Value {
    json!({"policy":POLICY,"embedding":embeddings::profile(),"minimum_cosine":MIN_COSINE,"publication_window_seconds":WINDOW_SECONDS,"place":"same selected reference identity","time":"publication window; differing explicit time phrases abstain","meaning":"possibly related, never confirmation or a merged incident"})
}
pub fn compatible(a: &Claim, b: &Claim) -> bool {
    if a.event_type != b.event_type
        || a.place.is_none()
        || a.place != b.place
        || !["reported", "denied"].contains(&a.state.as_str())
        || !["reported", "denied"].contains(&b.state.as_str())
    {
        return false;
    }
    if let (Some(a), Some(b)) = (&a.time_text, &b.time_text)
        && a.trim().to_lowercase() != b.trim().to_lowercase()
    {
        return false;
    }
    let time = |s: &Option<String>| {
        s.as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
    };
    match (time(&a.published_at), time(&b.published_at)) {
        (Some(a), Some(b)) => (a - b).num_seconds().abs() <= WINDOW_SECONDS,
        _ => false,
    }
}
pub fn related(a: &Claim, b: &Claim, av: &[f32], bv: &[f32]) -> Result<bool> {
    Ok(compatible(a, b) && embeddings::cosine(av, bv)? >= MIN_COSINE)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_vectors_cannot_override_time_place_type_and_state() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../examples/semantic-evaluation.json")).unwrap();
        let mut vector = vec![0.0; embeddings::DIMENSIONS];
        vector[0] = 1.0;
        for id in [
            "different-city",
            "different-day",
            "historical",
            "planned-drill",
            "unresolved",
            "missing-time",
            "different-type",
            "different-occurrence-time",
        ] {
            let case = fixture["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["id"] == id)
                .unwrap();
            let a: Claim = serde_json::from_value(case["a"].clone()).unwrap();
            let b: Claim = serde_json::from_value(case["b"].clone()).unwrap();
            assert!(!related(&a, &b, &vector, &vector).unwrap(), "{id}");
        }
    }
}
