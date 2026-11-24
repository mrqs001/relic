//! Offline reference/retained-claim comparison in a fresh isolated workspace.
//! No provider client, source collection or publication changes in the source.
use relic_app::{
    config::Config,
    error::{Error, Result},
    processing_geo::{GeoResolver, LocationProposal},
    store::Store,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::atomic::AtomicBool};
fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        return Err(Error::invalid(
            "Usage: preview_osm SOURCE_WORKSPACE NEW_WORKSPACE CANDIDATE",
        ));
    }
    let root = std::env::current_dir()?;
    let source = root.join(&args[0]);
    let destination = root.join(&args[1]);
    let config = Config::local(
        root.clone(),
        destination.clone(),
        "127.0.0.1:8795".parse().unwrap(),
    );
    relic_app::fresh_workspace::prepare(config.clone(), &source)?;
    let store = Store::open(config)?;
    let before = GeoResolver::load_for_store(&store)?;
    let started = std::time::Instant::now();
    store.activate_osm(
        &[PathBuf::from(&args[2]).to_string_lossy().into_owned()],
        &AtomicBool::new(false),
    )?;
    let activation_ms = started.elapsed().as_millis();
    let after = GeoResolver::load_for_store(&store)?;
    let c = rusqlite::Connection::open_with_flags(
        source.join("relic.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    c.execute_batch("BEGIN;")?;
    let publication = Store::latest(&c)?;
    let mut q=c.prepare("SELECT e.id,e.revision,e.text,i.json,(SELECT json_extract(a.json,'$.audit_hash') FROM processing_attempts a WHERE a.evidence_rowid=e.rowid AND a.status='completed' ORDER BY a.created_at DESC LIMIT 1) FROM publication_interpretations pi JOIN evidence e ON e.rowid=pi.evidence_rowid JOIN interpretations i ON i.id=pi.interpretation_id WHERE pi.publication_id=?1 ORDER BY e.rowid LIMIT 50001")?;
    let mut rows = q.query([publication])?;
    let mut checked = 0;
    let mut gained = 0;
    let mut lost = 0;
    let mut details = vec![];
    let start = std::time::Instant::now();
    while let Some(row) = rows.next()? {
        checked += 1;
        if checked > 50000 {
            return Err(Error::invalid("Comparison exceeds message bound"));
        }
        let old: Value = serde_json::from_str(&row.get::<_, String>(3)?)?;
        let text: String = row.get(2)?;
        let Some(audit_hash) = row.get::<_, Option<String>>(4)? else {
            continue;
        };
        if audit_hash.len() != 64 || !audit_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::invalid("Invalid retained audit hash"));
        }
        let analysis: relic_app::processing_ai::Analysis = serde_json::from_slice(&std::fs::read(
            source
                .join("objects/sha256")
                .join(&audit_hash[..2])
                .join(&audit_hash),
        )?)?;
        if analysis.audit.source_input_hash != relic_app::model::hash(text.as_bytes()) {
            return Err(Error::invalid("Retained audit/source mismatch"));
        }
        let original: Vec<LocationProposal> =
            serde_json::from_value(serde_json::to_value(&analysis.extracted.output.locations)?)?;

        for event in old["events"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["state"] == "reported")
        {
            let proposals = event["locations"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|l| {
                    original
                        .iter()
                        .find(|p| {
                            p.mention == l["mention"]
                                && p.quote == l["quote"]
                                && serde_json::to_value(&p.role).ok().as_ref() == Some(&l["role"])
                        })
                        .cloned()
                        .ok_or_else(|| {
                            Error::invalid("Saved claim location has no retained original proposal")
                        })
                })
                .collect::<Result<Vec<_>>>()?;
            let a = before.resolve(&text, &proposals, true)?;
            let b = after.resolve(&text, &proposals, true)?;
            for (a, b) in a.iter().zip(&b) {
                let old = a.geometry.is_some() || a.area.is_some();
                let new = b.geometry.is_some() || b.area.is_some();
                gained += usize::from(!old && new);
                lost += usize::from(old && !new);
                if !b.candidates.iter().any(|c| c.kind == "facility") {
                    continue;
                }
                details.push(json!({"id":row.get::<_,String>(0)?,"revision":row.get::<_,String>(1)?,"before":a,"after":b}));
            }
        }
    }
    let summary = json!({"messages":checked,"gained_locations":gained,"lost_locations":lost,"facility_matches":details.len(),"activation_ms":activation_ms,"comparison_ms":start.elapsed().as_millis(),"model_calls":0,"source_changed":false});
    std::fs::write(
        destination.join("osm-evaluation.json"),
        serde_json::to_vec_pretty(&json!({"summary":summary,"details":details}))?,
    )?;
    println!("{summary}");
    Ok(())
}
