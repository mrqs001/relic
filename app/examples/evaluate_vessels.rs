//! The ship finder's evaluation: the authored fictional messages in
//! `examples/vessel-mentions-evaluation.json` through the exact pinned
//! extraction request (`vessel_finder::request`), scored against the
//! fixture's thresholds. Neither mode opens a workspace; a passed record is
//! installed separately with `activate_vessel_evaluation`.
//!
//! Paid, explicit: `--out <dir> --env <private configuration>` (OPENROUTER_*).
//! One request per case, eight at a time. Each request is written to <dir>
//! before it is sent, and each raw reply is kept there with a receipt. A
//! rerun into the same directory reuses every kept reply and never resends a
//! request whose billing is unknown. Ctrl-C once stops sending (requests in
//! flight finish and are kept); twice cuts them off (billing unknown).
//! The record is `<dir>/evaluation.json`.
//!
//! Replay, free: `--replay <dir>` rescores the kept replies with no model
//! call and writes `<dir>/replay-evaluation.json`. It refuses replies whose
//! request differs from the current profile.
//!
//! Exit status: 0 passed, 1 failed the thresholds, 2 could not evaluate.
use relic_app::{config::Config, vessel_finder::evaluation};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Written whole: a staging file renamed over the record.
fn save(path: &Path, record: &Value) -> Result<(), String> {
    let staged = path.with_extension("staging");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let bytes = serde_json::to_vec_pretty(record).map_err(|_| "Could not encode the evaluation record")?;
    let mut file = options.open(&staged).map_err(|_| "Could not write the evaluation record")?;
    file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|_| "Could not write the evaluation record")?;
    std::fs::rename(&staged, path).map_err(|_| "Could not write the evaluation record".into())
}
/// The outcome without the per-case detail, which stays in the record file.
fn report(record: &Value, path: &Path) {
    let failed: Vec<Value> = record["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["passed"] != true)
        .map(|r| json!({"id":r["id"],"error":r["error"],"missing":r["missing"],"extra":r["extra"],"duplicates":r["duplicates"],"identifier_errors":r["identifier_errors"]}))
        .collect();
    println!(
        "{}",
        json!({"passed":record["passed"],"mode":record["mode"],"cases":record["cases"],"new_calls":record["new_calls"],"fingerprint_sha256":record["fingerprint_sha256"],
            "summary":record["summary"],"failures":record["failures"],"acquisition_errors":record["acquisition_errors"],"cases_not_passed":failed,"record":path.display().to_string()})
    );
}
async fn evaluate(args: Vec<String>) -> Result<bool, String> {
    if args.first().map(String::as_str) == Some("--replay") {
        let [_, dir] = args.as_slice() else { return Err("Use --replay <directory>".into()) };
        let dir = Path::new(dir);
        let record = evaluation::replay(dir).map_err(|e| e.message)?;
        let path = dir.join("replay-evaluation.json");
        save(&path, &record)?;
        report(&record, &path);
        return Ok(record["passed"] == true);
    }
    let mut rest = vec![];
    let mut out: Option<PathBuf> = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--out" {
            out = Some(args.next().ok_or("--out needs a directory")?.into());
        } else {
            rest.push(arg);
        }
    }
    let dir = out.ok_or("Use --out <directory> --env <private configuration> for the paid evaluation, or --replay <directory>")?;
    let config = Config::from_args(rest).map_err(|e| e.message)?;
    let (stop, halt) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let (s, h) = (stop.clone(), halt.clone());
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("Stopping: no new request is sent; requests in flight finish and are kept. Press Ctrl-C again to cut them off.");
            s.store(true, Ordering::Release);
        }
        if tokio::signal::ctrl_c().await.is_ok() {
            h.store(true, Ordering::Release);
        }
    });
    let record = evaluation::run(&dir, &config.source_values, stop, halt).await.map_err(|e| e.message)?;
    let path = dir.join("evaluation.json");
    save(&path, &record)?;
    report(&record, &path);
    Ok(record["passed"] == true)
}
#[tokio::main]
async fn main() {
    let code = match evaluate(std::env::args().skip(1).collect()).await {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(message) => {
            eprintln!("{message}");
            2
        }
    };
    std::process::exit(code);
}
