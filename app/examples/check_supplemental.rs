//! Explicit bounded fictional compatibility check; private receipts stay outside Git.
//! Arguments: ROOT ENV OUTPUT_DIR [--execute]. Existing attempts never resend.
use relic_app::supplemental_ai::{self as ai, Context};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        return Err("Use ROOT ENV OUTPUT_DIR [--execute]".into());
    }
    let root = PathBuf::from(&args[0]);
    let out = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&out)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o700))?;
    }
    let values = dotenvy::from_path_iter(&args[1])?
        .collect::<std::result::Result<std::collections::BTreeMap<String, String>, _>>()?;
    let client = ai::Client::new(&values).map_err(|_| "OpenRouter not configured")?;
    let cases: Vec<Value> = serde_json::from_slice(&std::fs::read(
        root.join("app/tests/fixtures/supplemental.json"),
    )?)?;
    let mut results = vec![];
    let mut total = 0.;
    for case in cases {
        let id = case["id"].as_str().unwrap();
        let context = Context {
            message: json!({"id":format!("fiction:{id}"),"revision":"v1","source":"fiction:jev","text":case["text"],"media_count":if id=="media"{1}else{0}}),
            neighbours: case["neighbours"].as_array().cloned().unwrap_or_default(),
            locations: case["locations"].as_array().cloned().unwrap_or_default(),
            interpretation_id: None,
            omitted_neighbours: 0,
            omitted_locations: 0,
        };
        let request = ai::request(&context).map_err(|_| "Invalid fictional request")?;
        let path = out.join(format!("{id}.json"));
        let audit = if path.exists() {
            serde_json::from_slice(&std::fs::read(path)?)?
        } else {
            if !args.iter().any(|a| a == "--execute") {
                return Err("Missing cached receipt; explicit --execute required".into());
            }
            // Durable attempted marker before sending: interruption cannot rebill.
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(out.join(format!("{id}.attempted")))?
                .sync_all()?;
            std::fs::write(
                out.join(format!("{id}.request.json")),
                serde_json::to_vec_pretty(&request)?,
            )?;
            let result = client
                .decide(&request)
                .await
                .map_err(|_| "Jev request failed; do not retry automatically")?;
            std::fs::write(path, serde_json::to_vec_pretty(&result)?)?;
            result
        };
        let accepted = ai::accepted(&request, &audit).map_err(|e| e.message)?;
        let cost = ai::cost(&audit["response"]).ok_or("Unknown billing; stop")?;
        total += cost;
        let mut checks = BTreeMap::new();
        for (key, expected) in case["expected"].as_object().unwrap() {
            checks.insert(
                key.clone(),
                expected
                    .as_array()
                    .unwrap()
                    .contains(&accepted[key]["choice"]),
            );
        }
        if id != "media" && id != "followup" {
            for event_type in relic_app::processing_ai::EVENT_TYPES {
                let key = format!("event_{event_type}");
                if !case["expected"].as_object().unwrap().contains_key(&key) {
                    checks.insert(key.clone(), accepted[&key]["choice"] == "absent");
                }
            }
        }
        let passed = checks.values().all(|ok| *ok);
        results.push(json!({"id":id,"passed":passed,"checks":checks,"elapsed_ms":audit["elapsed_ms"],"cost_usd":cost,"served_model":audit["response"]["model"]}));
        if cost > 0.002 || total > 0.02 {
            return Err("Fictional check spending bound reached".into());
        }
    }
    let report = json!({"recipe":ai::RECIPE,"passed":results.iter().all(|r|r["passed"]==true),"results":results,"cost_usd":total,"meaning":"Development smoke cases, not held-out semantic accuracy or promotion authority"});
    std::fs::write(out.join("report.json"), serde_json::to_vec_pretty(&report)?)?;
    println!("{report}");
    Ok(())
}
