//! Read-only audit of saved failed responses against the current decoder.
//! Safe beside the running app: deliberately never opens Store or sends traffic.
//! Writes source excerpts only into a private local review file, not stdout.
use relic_app::{config::Config, processing_ai, store::Store};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::{fs, io::Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().map_err(|e| e.message)?;
    let c = Connection::open_with_flags(config.db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.execute_batch("BEGIN")?;
    let publication = Store::latest(&c)
        .map_err(|e| e.message)?
        .ok_or("No publication")?;
    let mut query = c.prepare("SELECT a.id,e.rowid,e.text,a.json FROM processing_attempts a JOIN evidence e ON e.rowid=a.evidence_rowid JOIN membership m ON m.evidence_rowid=e.rowid WHERE m.publication_id=?1 AND a.status='failed' AND a.cost_usd IS NOT NULL AND json_type(a.json,'$.audit_hash')='text' ORDER BY a.created_at")?;
    let rows = query.query_map([publication], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut reviewed = 0;
    let mut accepted = vec![];
    let mut rejected = vec![];
    for row in rows {
        let (attempt, rowid, original, raw) = row?;
        let record: Value = serde_json::from_str(&raw)?;
        let h = record["audit_hash"].as_str().ok_or("Missing audit hash")?;
        if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid audit hash".into());
        }
        let path = config.object_path(h);
        if fs::metadata(&path)?.len() > 2 * 1024 * 1024 {
            return Err("Audit exceeds bound".into());
        }
        let audit: Option<processing_ai::ResponseAudit> = serde_json::from_slice(&fs::read(path)?)?;
        let Some(audit) = audit else {
            continue;
        };
        if audit.prompt_version != processing_ai::PROMPT_VERSION {
            continue;
        }
        reviewed += 1;
        match processing_ai::replay_audit(&original, &audit) {
            Ok(analysis) => accepted.push(json!({"attempt":attempt,"rowid":rowid,"source":original,"reading":analysis.extracted.output,"changes":analysis.audit.conservative_normalizations,"original_audit_hash":h})),
            Err(error) => rejected.push(json!({"attempt":attempt,"rowid":rowid,"kind":error.kind,"detail":error.detail,"original_audit_hash":h})),
        }
    }
    c.execute_batch("ROLLBACK")?;
    let path = config.data.join(format!(
        "reading-recovery-preview-{}.json",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%fZ")
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(&serde_json::to_vec_pretty(
        &json!({"model_calls":0,"reviewed":reviewed,"recoverable":accepted,"still_rejected":rejected}),
    )?)?;
    file.sync_all()?;
    println!(
        "{}",
        json!({"reviewed":reviewed,"recoverable_attempts":accepted.len(),"still_rejected_attempts":rejected.len(),"private_report":path,"model_calls":0,"database_writes":0})
    );
    Ok(())
}
