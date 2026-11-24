//! Times the map/feed scope preview (`POST /api/v1/query/preview`) on an
//! existing workspace copy. `Store::open` runs startup maintenance, so point it
//! at a copy, never at a live data directory.
//!
//! Usage: profile_query WORKSPACE_COPY [RUNS] [DUMP_DIRECTORY]
//! RUNS 0 only opens the workspace (startup maintenance and migrations).
//! Set RELIC_QUERY_PROFILE=1 for per-step timings. With DUMP_DIRECTORY each
//! case's last response is written there as `<case>.json` for comparison.
use relic_app::{config::Config, model::Query, store::Store};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Instant};

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[derive(Clone, Copy)]
enum Kind {
    /// The map view request.
    Preview,
    /// A feed continuation page after the named case's first page.
    Page(&'static str),
    /// `/query`: area outlines included, nothing compacted.
    Full,
}

fn main() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let Some(workspace) = args.first() else {
        return Err("Usage: profile_query WORKSPACE_COPY [RUNS] [DUMP_DIRECTORY]".into());
    };
    let runs = args.get(1).map_or(Ok(7), |r| r.parse::<usize>()).map_err(|e| e.to_string())?;
    let dump = args.get(2).map(std::path::PathBuf::from);
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let opened = Instant::now();
    let store = Store::open(Config::local(root, workspace.into(), "127.0.0.1:8797".parse().unwrap())).map_err(|e| e.message)?;
    println!("open (startup maintenance) {:.0} ms", opened.elapsed().as_secs_f64() * 1000.0);
    if runs == 0 {
        return Ok(());
    }
    let request = |scope: &Value, cursor: Option<String>| Query { scope: scope.clone(), generation: 1, cursor, limit: Some(100) };
    // The 24-hour window ends at the newest published message of the default scope.
    let default = store.query_preview(request(&json!({}), None)).map_err(|e| e.message)?;
    let end = default["timeline"]["to"].as_str().ok_or("Default scope has no dated messages")?;
    let end = chrono::DateTime::parse_from_rfc3339(end).map_err(|e| e.to_string())?;
    let start = (end - chrono::Duration::hours(24)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let end = end.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let source = default["items"][0]["source"].as_str().ok_or("Default scope has no messages")?.to_owned();
    let cases = [
        ("default", Kind::Preview, json!({})),
        ("events", Kind::Preview, json!({"content_view":"events"})),
        ("day", Kind::Preview, json!({"from":start,"to":end})),
        ("bbox", Kind::Preview, json!({"bbox":[30.0,25.0,60.0,40.0]})),
        ("source", Kind::Preview, json!({"source_ids":[source]})),
        ("located", Kind::Preview, json!({"include_unlocated":false})),
        ("signals", Kind::Preview, json!({"content_view":"signals"})),
        ("strike", Kind::Preview, json!({"content_view":"events","event_type":"strike"})),
        ("search", Kind::Preview, json!({"query":"drone"})),
        ("acquired", Kind::Preview, json!({"time_basis":"acquisition_time","from":start})),
        ("bbox_located", Kind::Preview, json!({"bbox":[30.0,25.0,60.0,40.0],"include_unlocated":false})),
        ("dateline", Kind::Preview, json!({"bbox":[100.0,-60.0,-100.0,60.0]})),
        ("page2", Kind::Page("default"), json!({})),
        ("events_page2", Kind::Page("events"), json!({"content_view":"events"})),
        ("full", Kind::Full, json!({})),
    ];
    let mut first_pages = BTreeMap::<&str, Value>::new();
    for (name, kind, scope) in &cases {
        let run = || -> Result<Value, String> {
            match kind {
                Kind::Preview => store.query_preview(request(scope, None)),
                Kind::Page(of) => {
                    let first: &Value = &first_pages[of];
                    let cursor = first["next_cursor"].as_str().map(str::to_owned);
                    store.query_page_preview(request(&first["scope"], cursor))
                }
                Kind::Full => store.query(request(scope, None)),
            }
            .map_err(|e| e.message)
        };
        let mut totals = vec![];
        let mut steps: BTreeMap<&'static str, Vec<f64>> = BTreeMap::new();
        let mut order: Vec<&'static str> = vec![];
        let mut last = Value::Null;
        let mut bytes = 0;
        relic_app::store::take_query_steps();
        for attempt in 0..=runs {
            let started = Instant::now();
            let result = run()?;
            let query_ms = started.elapsed().as_secs_f64() * 1000.0;
            let serialised = Instant::now();
            bytes = serde_json::to_vec(&result).map_err(|e| e.to_string())?.len();
            let serialise_ms = serialised.elapsed().as_secs_f64() * 1000.0;
            let recorded = relic_app::store::take_query_steps();
            // The first run warms the page cache and is not counted.
            if attempt > 0 {
                totals.push(query_ms + serialise_ms);
                if !recorded.is_empty() {
                    // Remaining time: response assembly and the compact preview projection.
                    let rest = query_ms - recorded.iter().map(|(_, ms)| ms).sum::<f64>();
                    for (step, ms) in recorded.into_iter().chain([("compact", rest.max(0.0)), ("serialise", serialise_ms)]) {
                        if !order.contains(&step) {
                            order.push(step);
                        }
                        steps.entry(step).or_default().push(ms);
                    }
                }
            }
            last = result;
        }
        totals.sort_by(f64::total_cmp);
        let size = |v: &Value| serde_json::to_vec(v).map(|b| b.len()).unwrap_or(0);
        let count = |v: &Value| v["features"].as_array().map_or(0, Vec::len);
        println!(
            "{name:12} p50 {:7.1} ms  p95 {:7.1} ms  bytes {bytes} (map {} / {} features, areas {} / {}, items {} / {})  total {} located {} unread {} map_truncated {}",
            percentile(&totals, 50.0),
            percentile(&totals, 95.0),
            size(&last["map"]),
            count(&last["map"]),
            size(&last["map_areas"]),
            count(&last["map_areas"]),
            size(&last["items"]),
            last["items"].as_array().map_or(0, Vec::len),
            last["total"],
            last["located"],
            last["unread"],
            last["map_truncated"],
        );
        if order.len() > 1 {
            let line = order
                .iter()
                .map(|step| {
                    let mut v = steps[step].clone();
                    v.sort_by(f64::total_cmp);
                    format!("{step} {:.1}/{:.1}", percentile(&v, 50.0), percentile(&v, 95.0))
                })
                .collect::<Vec<_>>()
                .join("  ");
            println!("             steps p50/p95 ms: {line}");
        }
        if let Some(dir) = &dump {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let text = serde_json::to_string_pretty(&last).map_err(|e| e.to_string())?;
            std::fs::write(dir.join(format!("{name}.json")), text).map_err(|e| e.to_string())?;
        }
        first_pages.insert(name, last);
    }
    Ok(())
}
