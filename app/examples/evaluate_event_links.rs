//! Explicit fictional relationship gate. Request admission precedes network I/O;
//! reruns replay receipts, and an interrupted/failed admission never rebills.
use futures_util::{StreamExt, stream};
use relic_app::{
    config::Config,
    event_links::{self, Client},
    event_similarity::Claim,
    model::{hash, string},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Deserialize)]
struct Case {
    id: String,
    split: String,
    expected: String,
    related: bool,
    a: Claim,
    b: Claim,
}
#[derive(Serialize, Deserialize)]
struct SavedReceipt {
    request_sha256: String,
    status: u16,
    elapsed_ms: u128,
    body: Vec<u8>,
}
fn save_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::File::open(path.parent().unwrap())?.sync_all()
}
async fn evaluate(c: Case, dir: &Path, client: &Client, cancel: Arc<AtomicBool>) -> Value {
    let mut result = json!({"id":c.id,"split":c.split,"expected":c.expected,"new_calls":0,"known_cost_usd":0.0,"unknown_cost_requests":0});
    if !event_links::reviewable(&c.a, &c.b) {
        result["actual"] = json!("ineligible");
        result["pass"] = json!(!c.related);
        return result;
    }
    let attempt = async {
        let request = event_links::request(&c.a, &c.b).map_err(|e| e.message)?;
        let key = hash(string(&request).as_bytes());
        result["request_sha256"] = json!(key);
        let admission = dir.join(format!("{key}.request.json"));
        let receiptpath = dir.join(format!("{key}.receipt.json"));
        let receipt: SavedReceipt = if receiptpath.exists() {
            // A corrupt/missing audit is unknown billing, never a free request.
            result["unknown_cost_requests"] = json!(1);
            if serde_json::from_slice::<Value>(
                &fs::read(&admission).map_err(|_| "Cannot read retained admission")?,
            )
            .map_err(|_| "Invalid retained admission")?
                != request
            {
                return Err("Retained request differs from evaluated input".into());
            }
            serde_json::from_slice(
                &fs::read(&receiptpath).map_err(|_| "Cannot read retained receipt")?,
            )
            .map_err(|_| "Invalid retained receipt; no retry")?
        } else {
            if admission.exists() {
                result["unknown_cost_requests"] = json!(1);
                return Err("Prior admission lacks a complete receipt; no retry".into());
            }
            if cancel.load(Ordering::Acquire) {
                return Err("Evaluation cancelled before admission".into());
            }
            save_new(&admission, string(&request).as_bytes())
                .map_err(|_| "Cannot durably admit evaluation request")?;
            result["new_calls"] = json!(1);
            result["unknown_cost_requests"] = json!(1);
            let raw = match client.review(&c.a, &c.b, cancel).await {
                Ok(raw) => raw,
                Err(e) if e.code == "model_not_sent" => {
                    result["new_calls"] = json!(0);
                    result["unknown_cost_requests"] = json!(0);
                    fs::remove_file(&admission).map_err(|_| "Cannot clear unsent admission")?;
                    fs::File::open(dir)
                        .and_then(|f| f.sync_all())
                        .map_err(|_| "Cannot synchronize cancelled admission")?;
                    return Err(e.message);
                }
                Err(e) => return Err(e.message),
            };
            let saved = SavedReceipt {
                request_sha256: key.clone(),
                status: raw.status,
                elapsed_ms: raw.elapsed_ms,
                body: raw.body,
            };
            // A single atomic rename commits status, timing and exact raw bytes.
            let staged = receiptpath.with_extension("staging");
            save_new(
                &staged,
                &serde_json::to_vec(&saved).map_err(|_| "Cannot encode receipt")?,
            )
            .map_err(|_| "Cannot retain receipt")?;
            fs::rename(staged, &receiptpath).map_err(|_| "Cannot commit receipt")?;
            fs::File::open(dir)
                .and_then(|f| f.sync_all())
                .map_err(|_| "Cannot synchronize receipt")?;
            saved
        };
        result["elapsed_ms"] = json!(receipt.elapsed_ms);
        result["unknown_cost_requests"] = json!(1);
        if receipt.request_sha256 != key {
            return Err("Receipt request identity mismatch".into());
        }
        let response: Value = serde_json::from_slice(&receipt.body)
            .map_err(|_| "Retained model response is not JSON")?;
        if let Some(cost) = response["usage"]["cost"]
            .as_f64()
            .filter(|v| v.is_finite() && *v >= 0.0)
        {
            result["known_cost_usd"] = json!(cost);
            result["unknown_cost_requests"] = json!(0);
        }
        if receipt.status != 200 {
            return Err(format!(
                "Provider returned HTTP {}; exact response retained privately",
                receipt.status
            ));
        }
        let review = event_links::decode(&c.a, &c.b, &response).map_err(|e| e.message)?;
        result["actual"] = serde_json::to_value(&review.decision.relation).unwrap();
        result["actual_link"] = json!(review.decision.relation.is_link());
        result["review"] = serde_json::to_value(review).unwrap();
        Ok::<(), String>(())
    }
    .await;
    if let Err(error) = attempt {
        result["error"] = json!(error);
    }
    result["pass"] = json!(result.get("error").is_none() && result["actual"] == c.expected);
    result
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().map_err(|e| e.message)?;
    let dir = config.data.join("event-link-evaluation");
    fs::create_dir_all(&dir)?;
    let fixture: Value =
        serde_json::from_str(include_str!("../../examples/event-link-evaluation.json"))?;
    let cases: Vec<Case> = serde_json::from_value(fixture["cases"].clone())?;
    // A fixture must not race two cases for the same paid admission. Reject
    // duplicate requests before any calls rather than miscount one as unknown.
    let mut requests = std::collections::HashSet::new();
    let mut ids = std::collections::HashSet::new();
    for c in &cases {
        if !ids.insert(&c.id) {
            return Err("Duplicate evaluation case identity".into());
        }
        if event_links::reviewable(&c.a, &c.b)
            && !requests.insert(string(
                &event_links::request(&c.a, &c.b).map_err(|e| e.message)?,
            ))
        {
            return Err("Duplicate evaluation request; reuse one case instead".into());
        }
    }
    let client = Client::new(&config.source_values).map_err(|e| e.message)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal.store(true, Ordering::Release);
    });
    let mut results = stream::iter(
        cases
            .into_iter()
            .map(|c| evaluate(c, &dir, &client, cancel.clone())),
    )
    .buffer_unordered(8)
    .collect::<Vec<_>>()
    .await;
    results.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let passed = results.iter().filter(|r| r["pass"] == true).count();
    let unsafe_links = results
        .iter()
        .filter(|r| {
            matches!(r["expected"].as_str(), Some("distinct" | "uncertain"))
                && r["actual_link"] == true
        })
        .count();
    // Costs describe unique retained requests, not case counts or this run alone.
    let mut receipts = std::collections::BTreeMap::new();
    for r in &results {
        if let Some(key) = r["request_sha256"].as_str() {
            receipts.entry(key).or_insert(r);
        }
    }
    let report = json!({"fictional":true,"profile":event_links::profile(),"fixture_sha256":hash(include_bytes!("../../examples/event-link-evaluation.json")),
        "passed":passed==results.len(),"passed_cases":passed,"total_cases":results.len(),"unsafe_links":unsafe_links,
        "new_calls":results.iter().filter_map(|r|r["new_calls"].as_u64()).sum::<u64>(),
        "known_cost_usd":receipts.values().filter_map(|r|r["known_cost_usd"].as_f64()).sum::<f64>(),
        "unknown_cost_requests":receipts.values().filter_map(|r|r["unknown_cost_requests"].as_u64()).sum::<u64>(),
        "admission_rejections":results.iter().filter(|r|r["actual"]=="ineligible").count(),
        "model_reviewed":results.iter().filter(|r|r.get("review").is_some()).count(),
        "model_exact_labels":results.iter().filter(|r|r.get("review").is_some() && r["pass"]==true).count(),
        "safe_abstentions":results.iter().filter(|r|r["actual"]=="uncertain" && r["expected"]=="distinct").count(),
        "results":results});
    fs::write(dir.join("report.json"), serde_json::to_vec_pretty(&report)?)?;
    println!(
        "{}",
        json!({"passed":passed,"total":report["total_cases"],"unsafe_links":unsafe_links,"new_calls":report["new_calls"],"known_cost_usd":report["known_cost_usd"],"unknown_cost_requests":report["unknown_cost_requests"]})
    );
    for r in report["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["pass"] != true)
    {
        println!(
            "{}",
            json!({"id":r["id"],"expected":r["expected"],"actual":r["actual"],"error":r["error"]})
        );
    }
    if report["passed"] != true {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, Client, Case) {
        let dir = tempfile::tempdir().unwrap();
        let values = [("OPENROUTER_API_KEY".into(), "fictional-not-a-key".into())]
            .into_iter()
            .collect();
        let client = Client::new(&values).unwrap();
        let fixture: Value =
            serde_json::from_str(include_str!("../../examples/event-link-evaluation.json"))
                .unwrap();
        let case = serde_json::from_value(fixture["cases"][0].clone()).unwrap();
        (dir, client, case)
    }
    #[tokio::test]
    async fn completed_receipt_replays_without_network_or_new_admission() {
        let (dir, client, case) = setup();
        let request = event_links::request(&case.a, &case.b).unwrap();
        let key = hash(string(&request).as_bytes());
        save_new(
            &dir.path().join(format!("{key}.request.json")),
            string(&request).as_bytes(),
        )
        .unwrap();
        let output = json!({"relation":"same_occurrence","a_quote":case.a.text,"b_quote":case.b.text,"reason":"Both identify the North Pier grain warehouse fire."});
        let response = json!({"model":event_links::MODEL,"provider":"OpenAI","usage":{"cost":0.001},"choices":[{"finish_reason":"stop","message":{"content":string(&output)}}]});
        let receipt = SavedReceipt {
            request_sha256: key.clone(),
            status: 200,
            elapsed_ms: 1,
            body: serde_json::to_vec(&response).unwrap(),
        };
        save_new(
            &dir.path().join(format!("{key}.receipt.json")),
            &serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        let r = evaluate(case, dir.path(), &client, Arc::new(AtomicBool::new(true))).await;
        assert_eq!(r["pass"], true);
        assert_eq!(r["new_calls"], 0);
        assert_eq!(r["known_cost_usd"], 0.001);
        assert_eq!(r["unknown_cost_requests"], 0);
    }
    #[tokio::test]
    async fn interrupted_admission_never_rebills_and_cancel_prevents_admission() {
        let (dir, client, case) = setup();
        let r = evaluate(
            case.clone(),
            dir.path(),
            &client,
            Arc::new(AtomicBool::new(true)),
        )
        .await;
        assert_eq!(r["new_calls"], 0);
        assert_eq!(r["unknown_cost_requests"], 0);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        let request = event_links::request(&case.a, &case.b).unwrap();
        let key = hash(string(&request).as_bytes());
        save_new(
            &dir.path().join(format!("{key}.request.json")),
            string(&request).as_bytes(),
        )
        .unwrap();
        let r = evaluate(case, dir.path(), &client, Arc::new(AtomicBool::new(false))).await;
        assert_eq!(r["new_calls"], 0);
        assert_eq!(r["unknown_cost_requests"], 1);
        assert!(r["error"].as_str().unwrap().contains("no retry"));
    }
    #[tokio::test]
    async fn corrupt_or_missing_retained_audit_is_unknown_cost_without_retry() {
        let (dir, client, case) = setup();
        let request = event_links::request(&case.a, &case.b).unwrap();
        let key = hash(string(&request).as_bytes());
        let receipt = dir.path().join(format!("{key}.receipt.json"));
        save_new(&receipt, b"corrupt").unwrap();
        // Missing admission is not evidence that a retained paid receipt was free.
        let r = evaluate(
            case.clone(),
            dir.path(),
            &client,
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        assert_eq!(r["new_calls"], 0);
        assert_eq!(r["unknown_cost_requests"], 1);
        save_new(
            &dir.path().join(format!("{key}.request.json")),
            string(&request).as_bytes(),
        )
        .unwrap();
        let r = evaluate(case, dir.path(), &client, Arc::new(AtomicBool::new(false))).await;
        assert_eq!(r["new_calls"], 0);
        assert_eq!(r["unknown_cost_requests"], 1);
        assert!(
            r["error"]
                .as_str()
                .unwrap()
                .contains("Invalid retained receipt")
        );
    }
}
