//! Offline geography diagnosis over already-paid readings. Replays the retained
//! place proposals of every current reading through the resolver exactly as a
//! reading does (per event, reported state as the occurrence gate). No model,
//! source or network calls. Run against a disposable copy of a workspace:
//! Store::open performs startup maintenance on the directory it is given.
//! Arguments: PROJECT_ROOT WORKSPACE_COPY OUTPUT_JSONL [--activate DIR,DIR,...]
//! [--boundaries DIR,DIR,...]. They first activate those GeoNames or boundary
//! editions in the copy (each flag replaces that whole active set). Per-mention
//! rows go to the private output file; stdout carries aggregate counts only.
use relic_app::{
    config::Config,
    model::hash,
    processing_ai::{Analysis, ClaimStatus},
    processing_geo::{GeoResolver, LocationProposal},
    store::Store,
};
use serde_json::json;
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf};

fn main() -> Result<(), String> {
    let a = std::env::args().collect::<Vec<_>>();
    let flags = a.get(4..).unwrap_or_default();
    if a.len() < 4 || flags.len() % 2 != 0 || flags.chunks(2).any(|f| !["--activate", "--boundaries"].contains(&f[0].as_str())) {
        return Err("Usage: diagnose_geography PROJECT_ROOT WORKSPACE_COPY OUTPUT_JSONL [--activate DIR,DIR,...] [--boundaries DIR,DIR,...]".into());
    }
    let root = PathBuf::from(&a[1]);
    let data = PathBuf::from(&a[2]);
    let store = Store::open(Config::local(root, data.clone(), "127.0.0.1:8799".parse().unwrap()))
        .map_err(|e| e.message)?;
    for flag in flags.chunks(2) {
        let paths = flag[1].split(',').map(str::to_owned).collect::<Vec<_>>();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        if flag[0] == "--activate" {
            store.activate_geography(&paths, &cancel).map_err(|e| e.message)?;
        } else {
            store.activate_boundaries(&paths, &cancel).map_err(|e| e.message)?;
        }
    }
    let resolver = GeoResolver::load_for_store(&store).map_err(|e| e.message)?;
    let c = store.connect().map_err(|e| e.message)?;
    let mut stmt = c.prepare("SELECT e.id,e.text,json_extract(p.json,'$.audit_hash') FROM membership m JOIN evidence e ON e.rowid=m.evidence_rowid JOIN processing_attempts p ON p.evidence_rowid=e.rowid WHERE m.publication_id=(SELECT value FROM metadata WHERE key='current_publication') AND p.status='completed' AND json_type(p.json,'$.audit_hash')='text' AND p.id=(SELECT q.id FROM processing_attempts q WHERE q.evidence_rowid=e.rowid AND q.status='completed' AND json_type(q.json,'$.audit_hash')='text' ORDER BY q.created_at DESC,q.id DESC LIMIT 1)").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut out = fs::File::create(&a[3]).map_err(|e| e.to_string())?;
    let mut statuses = BTreeMap::<String, usize>::new();
    let (mut messages, mut reported_messages, mut located_messages, mut point_messages) = (0, 0, 0, 0);
    for (id, text, h) in rows {
        let Ok(bytes) = fs::read(data.join("objects/sha256").join(&h[..2]).join(&h)) else {
            continue;
        };
        if hash(&bytes) != h {
            return Err("Invalid audit object".into());
        }
        let Ok(audit) = serde_json::from_slice::<Analysis>(&bytes) else {
            continue;
        };
        messages += 1;
        let output = &audit.extracted.output;
        let proposals: Vec<LocationProposal> =
            serde_json::from_value(serde_json::to_value(&output.locations).unwrap()).unwrap();
        let (_, message_country) = resolver.resolve_message(&text, &proposals, true).map_err(|e| e.message)?;
        let (mut reported, mut located, mut point) = (false, false, false);
        for (claim_index, claim) in output.claims.iter().enumerate() {
            let Some(event_type) = &claim.event_type else { continue };
            if claim.state != ClaimStatus::Reported {
                continue;
            }
            reported = true;
            let chosen = claim
                .location_indices
                .iter()
                .filter_map(|i| proposals.get(*i).cloned())
                .collect::<Vec<_>>();
            let places = resolver.resolve_in(&text, &chosen, true, message_country.as_deref()).map_err(|e| e.message)?;
            for (place, proposal) in places.iter().zip(&chosen) {
                *statuses.entry(format!("{} | {}", place.status, place.precision)).or_default() += 1;
                located |= place.geometry.is_some() || place.area.is_some();
                point |= place.geometry.is_some();
                let candidates = place.candidates.iter().take(12).map(|c| {
                    let r = c.alias_reference.as_ref();
                    json!({"id":c.id,"name":c.name,"cc":c.country_code,"kind":c.kind,"class":r.and_then(|r| r["feature_class"].as_str()),"code":r.and_then(|r| r["feature_code"].as_str()),"population":r.and_then(|r| r["publisher_record"].as_str()).and_then(|p| p.split('\t').nth(14)).and_then(|p| p.parse::<u64>().ok()),"basis":c.match_basis})
                }).collect::<Vec<_>>();
                let row = json!({"message_country":message_country,"evidence":id,"claim":claim_index,"event_type":event_type,"mention":proposal.mention,"canonical":proposal.canonical_name,"country":proposal.country_code,"role":proposal.role,"quote":proposal.quote,"status":place.status,"precision":place.precision,"reason":place.reason,"selected":place.selected_candidate_id,"point":place.geometry.is_some(),"area":place.area.as_ref().and_then(|a| a["id"].as_str().or(a["label"].as_str())),"candidate_count":place.candidate_count,"candidates":candidates});
                writeln!(out, "{row}").map_err(|e| e.to_string())?;
            }
        }
        reported_messages += reported as usize;
        located_messages += located as usize;
        point_messages += point as usize;
    }
    println!("{}", serde_json::to_string_pretty(&json!({"model_calls":0,"readings":messages,"messages_with_reported_events":reported_messages,"located_messages":located_messages,"point_messages":point_messages,"event_place_outcomes":statuses})).unwrap());
    Ok(())
}
