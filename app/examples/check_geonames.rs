//! Read-only comparison of a candidate reference against already-paid extractions.
//! Prints aggregate counts only. Never publishes, changes policy or calls a model.
use relic_app::{
    geonames::Index,
    model::hash,
    processing_ai::{Analysis, ClaimStatus, Kind},
    processing_geo::{GeoResolver, LocationProposal},
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path, time::Instant};
fn main() -> Result<(), String> {
    let a = std::env::args().collect::<Vec<_>>();
    if a.len() != 4 {
        return Err("Usage: check_geonames PROJECT_ROOT WORKSPACE_DATA GEONAMES_EDITION".into());
    }
    let index = Index::inspect(Path::new(&a[3])).map_err(|e| e.message)?;
    let baseline = GeoResolver::load(Path::new(&a[1])).map_err(|e| e.message)?;
    let db = Connection::open_with_flags(
        Path::new(&a[2]).join("relic.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| e.to_string())?;
    let mut stmt=db.prepare("SELECT e.text,json_extract(p.json,'$.audit_hash') FROM membership m JOIN evidence e ON e.rowid=m.evidence_rowid JOIN processing_attempts p ON p.evidence_rowid=e.rowid WHERE m.publication_id=(SELECT value FROM metadata WHERE key='current_publication') AND p.status='completed' AND p.id=(SELECT q.id FROM processing_attempts q WHERE q.evidence_rowid=e.rowid AND q.status='completed' ORDER BY q.created_at DESC,q.id DESC LIMIT 1) LIMIT 500").map_err(|e|e.to_string())?;
    let mut statuses = BTreeMap::<String, usize>::new();
    let mut calls = 0;
    let mut old = 0;
    let mut candidate_points = 0;
    let mut samples = vec![];
    for row in stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?
    {
        let (text, h) = row.map_err(|e| e.to_string())?;
        if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid audit hash".into());
        }
        let bytes = fs::read(
            Path::new(&a[2])
                .join("objects/sha256")
                .join(&h[..2])
                .join(&h),
        )
        .map_err(|e| e.to_string())?;
        if bytes.len() > 2 * 1024 * 1024 || hash(&bytes) != h {
            return Err("Invalid audit object".into());
        }
        let audit: Analysis =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid extraction audit")?;
        let out = &audit.extracted.output;
        let occurrence = out.kind == Kind::Event && out.claim_status == ClaimStatus::Reported;
        let proposals: Vec<LocationProposal> =
            serde_json::from_value(serde_json::to_value(&out.locations).unwrap()).unwrap();
        old += baseline
            .resolve(&text, &proposals, occurrence)
            .map_err(|e| e.message)?
            .iter()
            .filter(|r| r.geometry.is_some())
            .count();
        for p in proposals {
            let began = Instant::now();
            let resolved = index
                .resolve(&text, &p, occurrence)
                .map_err(|e| e.message)?;
            samples.push(began.elapsed().as_secs_f64() * 1000.0);
            calls += 1;
            if let Some(r) = resolved {
                *statuses.entry(r.status.clone()).or_default() += 1;
                if r.geometry.is_some() {
                    candidate_points += 1;
                }
            } else {
                *statuses.entry("no_match_in_pilot".into()).or_default() += 1;
            }
        }
    }
    samples.sort_by(f64::total_cmp);
    let p95 = samples
        .get(samples.len().saturating_sub(1) * 95 / 100)
        .copied()
        .unwrap_or(0.0);
    let output: Value = json!({"candidate_only":true,"model_calls":0,"place_mentions":calls,"baseline_occurrence_mentions":old,"pilot_occurrence_mentions":candidate_points,"status_counts":statuses,"lookup_p95_ms":p95,"limits":"Yemen reference only; unchanged retained extraction semantics; counts are mentions, not incidents; no publication changed"});
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    Ok(())
}
