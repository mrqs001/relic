//! Explicit fictional embedding evaluation. Never opens the live workspace or
//! installs a gate. Re-running replays saved responses; it never repeats a request.
use relic_app::{
    config::Config,
    embeddings,
    event_similarity::{self, Claim},
    model::{hash, string},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    sync::{Arc, atomic::AtomicBool},
};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().map_err(|e| e.message)?;
    let dir = config.data.join("semantic-evaluation");
    fs::create_dir_all(&dir)?;
    let fixture: Value =
        serde_json::from_str(include_str!("../../examples/semantic-evaluation.json"))?;
    let mut texts = BTreeMap::new();
    for case in fixture["cases"].as_array().unwrap() {
        for side in ["a", "b"] {
            let text = case[side]["text"].as_str().unwrap().to_owned();
            texts.insert(text, Vec::new());
        }
    }
    let inputs = texts.keys().cloned().collect::<Vec<_>>();
    let client = embeddings::Client::new(&config.source_values).map_err(|e| e.message)?;
    let mut calls = 0;
    let mut costs = Vec::new();
    let mut elapsed = 0;
    for batch in inputs.chunks(embeddings::BATCH) {
        let request = embeddings::request(batch).map_err(|e| e.message)?;
        let key = hash(string(&request).as_bytes());
        let reqpath = dir.join(format!("{key}.request.json"));
        let responsepath = dir.join(format!("{key}.response.json"));
        if !responsepath.exists() {
            use std::io::Write;
            let mut f = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&reqpath)?;
            f.write_all(string(&request).as_bytes())?;
            f.sync_all()?;
            let receipt = client
                .embed(batch, Arc::new(AtomicBool::new(false)))
                .await
                .map_err(|e| e.message)?;
            calls += 1;
            fs::write(&responsepath, &receipt.body)?;
            fs::write(
                dir.join(format!("{key}.receipt.json")),
                string(&json!({"status":receipt.status,"elapsed_ms":receipt.elapsed_ms})),
            )?;
            if receipt.status != 200 {
                return Err(format!(
                    "Embedding evaluation returned HTTP {}; response retained privately",
                    receipt.status
                )
                .into());
            }
        }
        if serde_json::from_slice::<Value>(&fs::read(&reqpath)?)? != request {
            return Err("Retained embedding request mismatch".into());
        }
        let receipt: Value =
            serde_json::from_slice(&fs::read(dir.join(format!("{key}.receipt.json")))?)?;
        if receipt["status"] != 200 {
            return Err("Prior failed request retained; no automatic retry".into());
        }
        elapsed += receipt["elapsed_ms"].as_u64().unwrap_or(0);
        let response: Value = serde_json::from_slice(&fs::read(&responsepath)?)?;
        costs.push(response["usage"]["cost"].as_f64());
        let vectors = embeddings::decode(&response, batch.len()).map_err(|e| e.message)?;
        for (text, vector) in batch.iter().zip(vectors) {
            texts.insert(text.clone(), vector);
        }
    }
    let mut results = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let a: Claim = serde_json::from_value(case["a"].clone())?;
        let b: Claim = serde_json::from_value(case["b"].clone())?;
        let similarity =
            embeddings::cosine(&texts[&a.text], &texts[&b.text]).map_err(|e| e.message)?;
        let related = event_similarity::related(&a, &b, &texts[&a.text], &texts[&b.text])
            .map_err(|e| e.message)?;
        results.push(json!({"id":case["id"],"expected":case["related"],"actual":related,"pass":case["related"]==related,"cosine":similarity}));
    }
    let passed = results.iter().filter(|r| r["pass"] == true).count();
    let report = json!({"fictional":true,"fixture_sha256":hash(include_bytes!("../../examples/semantic-evaluation.json")),"profile":event_similarity::profile(),"passed":passed==results.len(),"passed_cases":passed,"total_cases":results.len(),"new_calls":calls,"retained_request_elapsed_ms":elapsed,"known_cost_usd":costs.iter().flatten().sum::<f64>(),"unknown_cost_requests":costs.iter().filter(|v|v.is_none()).count(),"results":results});
    fs::write(dir.join("report.json"), serde_json::to_vec_pretty(&report)?)?;
    println!(
        "{}",
        json!({"passed":passed,"total":results.len(),"new_calls":calls,"known_cost_usd":report["known_cost_usd"],"unknown_cost_requests":report["unknown_cost_requests"]})
    );
    for r in results.iter().filter(|r| r["pass"] != true) {
        println!("{}", r);
    }
    Ok(())
}
