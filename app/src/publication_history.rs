//! Publication membership is a validity interval, not a copied snapshot.
//! The legacy read views preserve exact publication IDs, including frozen views.
use crate::error::Result;
use rusqlite::{Connection, params};

pub(crate) fn migrate(c: &Connection) -> Result<()> {
    let legacy: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='membership' AND type='table')",
        [],
        |r| r.get(0),
    )?;
    if !legacy {
        return Ok(());
    }
    // No references remain to the old membership table after its companion is
    // dropped. SQLite DDL and interval conversion roll back together on failure.
    c.execute_batch("BEGIN IMMEDIATE;")?;
    let result = (|| -> Result<()> {
        c.execute_batch(include_str!("publication_history.sql"))?;
        c.execute_batch("COMMIT")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = c.execute_batch("ROLLBACK");
    }
    result
}

/// Usually the prior publication is the immediately preceding ordinal: no work.
/// Explicit frozen snapshots are branches; reconcile only their differences.
pub(crate) fn inherit(c: &Connection, publication: &str, prior: Option<&str>) -> Result<()> {
    let (seq, previous): (i64, Option<String>) = c.query_row(
        "SELECT seq,(SELECT id FROM publication_order WHERE seq=p.seq-1) FROM publication_order p WHERE id=?1", [publication], |r| Ok((r.get(0)?,r.get(1)?)))?;
    if previous.as_deref() == prior {
        return Ok(());
    }
    c.execute("UPDATE membership_intervals SET until_seq=?1 WHERE until_seq IS NULL AND evidence_rowid NOT IN (SELECT evidence_rowid FROM membership WHERE publication_id=?2)", params![seq,prior])?;
    c.execute(
        "INSERT INTO membership SELECT ?1,evidence_rowid FROM membership WHERE publication_id=?2",
        params![publication, prior],
    )?;
    c.execute("UPDATE interpretation_intervals SET until_seq=?1 WHERE until_seq IS NULL AND NOT EXISTS(SELECT 1 FROM publication_interpretations p WHERE p.publication_id=?2 AND p.evidence_rowid=interpretation_intervals.evidence_rowid AND p.interpretation_id=interpretation_intervals.interpretation_id)",params![seq,prior])?;
    c.execute("INSERT INTO publication_interpretations SELECT ?1,evidence_rowid,interpretation_id FROM publication_interpretations WHERE publication_id=?2",params![publication,prior])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_islands_preserve_removal_and_readdition_and_reopen() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("schema.sql")).unwrap();
        c.execute_batch("INSERT INTO sources VALUES('source','{}'); INSERT INTO objects VALUES('object',1,'text/plain'); INSERT INTO evidence VALUES(1,'e','v1','source','2026-01-01',NULL,NULL,NULL,NULL,'text','object','{}'); INSERT INTO publications VALUES('p1','one','2026-01-01','complete',NULL),('p2','two','2026-01-01','complete',NULL),('p3','three','2026-01-01','complete',NULL); INSERT INTO membership VALUES('p1',1),('p3',1);").unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM membership_intervals", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM membership WHERE publication_id='p2'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert!(
            c.execute("DELETE FROM membership WHERE publication_id='p1'", [])
                .is_err()
        );
        c.execute_batch(include_str!("schema.sql")).unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM membership", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}
