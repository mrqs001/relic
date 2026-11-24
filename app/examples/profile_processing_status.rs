//! Timing of the processing status that the Processing and Sources views poll.
//!
//! Use only a disposable copy of a workspace, never a live data directory:
//! `Store::open` performs its startup maintenance there, and the churn step
//! temporarily rewrites attempt JSON (restoring the exact text afterwards).
//! No network, credentials or model calls. A placeholder AI key selects the
//! configured branch of the status so its timing matches a configured
//! workspace; it is never sent anywhere.
//!
//! cargo run --release --example profile_processing_status -- \
//!     <copied data dir> [runs] [changed attempts per poll] [status.json]
use relic_app::{config::Config, store::Store};
use serde_json::Value;
use std::{path::PathBuf, time::Instant};

fn summary(label: &str, mut samples: Vec<f64>) {
    if samples.is_empty() {
        return;
    }
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((samples.len() as f64 - 1.0) * q).round() as usize];
    println!(
        "{label:<40} n={:<3} p50={:>8.2} ms  p95={:>8.2} ms  max={:>8.2} ms",
        samples.len(),
        at(0.5),
        at(0.95),
        samples[samples.len() - 1]
    );
}

fn time<T>(
    samples: &mut Vec<f64>,
    f: impl FnOnce() -> relic_app::error::Result<T>,
) -> Result<T, String> {
    let start = Instant::now();
    let out = f().map_err(|e| e.message)?;
    samples.push(start.elapsed().as_secs_f64() * 1000.0);
    Ok(out)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let data = PathBuf::from(
        args.next()
            .expect("Pass a disposable copy of a workspace data directory"),
    );
    let runs: usize = args.next().map_or(Ok(20), |n| n.parse())?;
    let churn: usize = args.next().map_or(Ok(64), |n| n.parse())?;
    let dump = args.next().map(PathBuf::from);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    assert!(
        data.join("relic.sqlite3").is_file(),
        "The directory must contain a copied relic.sqlite3"
    );
    assert!(
        !data.canonicalize()?.starts_with(root.join("data").canonicalize()?),
        "Refusing a workspace inside this checkout's data directory; profile a copy"
    );
    let mut config = Config::local(root, data, "127.0.0.1:8797".parse()?);
    config.source_values.insert(
        "OPENROUTER_API_KEY".into(),
        "profile-placeholder-not-a-key".into(),
    );
    let open = || Store::open(config.clone()).map_err(|e| e.message);

    // First call on a newly opened store: every message is classified.
    let mut cold = vec![];
    let mut first: Option<Value> = None;
    for _ in 0..runs.clamp(1, 5) {
        let s = open()?;
        let status = time(&mut cold, || s.processing_status(false))?;
        if let Some(first) = &first {
            assert_eq!(first, &status, "Status changed without a write");
        }
        first = Some(status);
    }
    let first = first.unwrap();
    let s = open()?;
    s.processing_status(false).map_err(|e| e.message)?;

    let (mut warm, mut links, mut supplemental, mut interrupted, mut failures) =
        (vec![], vec![], vec![], vec![], vec![]);
    for _ in 0..runs {
        let status = time(&mut warm, || s.processing_status(false))?;
        assert_eq!(first, status, "A repeated poll must return the same status");
        time(&mut links, || s.event_link_status())?;
        time(&mut supplemental, || s.supplemental_status())?;
        time(&mut interrupted, || s.processing_interrupted())?;
        time(&mut failures, || s.processing_failures(None))?;
    }

    // Running-batch churn: `churn` attempts change between polls. The edit
    // adds an unused JSON key, so every count must stay the same while each
    // touched message is reclassified.
    let mut changed = vec![];
    if churn > 0 {
        let c = s.connect().map_err(|e| e.message)?;
        let originals: Vec<(String, String)> = c
            .prepare("SELECT id,json FROM processing_attempts ORDER BY created_at DESC LIMIT ?1")?
            .query_map([churn as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for run in 0..runs {
            {
                let _writer = s.writer.lock().map_err(|_| "writer lock")?;
                for (id, _) in &originals {
                    c.execute(
                        "UPDATE processing_attempts SET json=json_set(json,'$.profile_touch',?2) WHERE id=?1",
                        rusqlite::params![id, run as i64],
                    )?;
                }
            }
            let status = time(&mut changed, || s.processing_status(false))?;
            assert_eq!(first, status, "An unused attempt key changed a count");
        }
        let _writer = s.writer.lock().map_err(|_| "writer lock")?;
        for (id, json) in &originals {
            c.execute(
                "UPDATE processing_attempts SET json=?2 WHERE id=?1",
                rusqlite::params![id, json],
            )?;
        }
    }
    summary("processing_status (new store)", cold);
    summary("processing_status (repeat poll)", warm);
    summary(
        &format!("processing_status ({churn} attempts changed)"),
        changed,
    );
    summary("  event_link_status", links);
    summary("  supplemental_status", supplemental);
    summary("processing_interrupted", interrupted);
    summary("processing_failures (first page)", failures);
    if let Some(dump) = dump {
        std::fs::write(dump, serde_json::to_vec_pretty(&first)?)?;
    }
    println!(
        "pending={} queued={} in_flight={} failed={} processed={} interrupted={}",
        first["pending"],
        first["queued"],
        first["in_flight"],
        first["failed"],
        first["processed"],
        first["interrupted"]
    );
    Ok(())
}
