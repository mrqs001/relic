CREATE TABLE publication_order(seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE REFERENCES publications(id));
INSERT INTO publication_order(id) SELECT id FROM publications ORDER BY rowid;
CREATE TRIGGER publication_ordinal AFTER INSERT ON publications BEGIN
 INSERT INTO publication_order(id) VALUES(NEW.id);
END;
CREATE TABLE membership_intervals(
 evidence_rowid INTEGER NOT NULL REFERENCES evidence(rowid),
 from_seq INTEGER NOT NULL REFERENCES publication_order(seq),
 until_seq INTEGER REFERENCES publication_order(seq),
 PRIMARY KEY(evidence_rowid,from_seq), CHECK(until_seq IS NULL OR until_seq>from_seq)
);
CREATE INDEX membership_interval_range ON membership_intervals(from_seq,until_seq,evidence_rowid);
CREATE UNIQUE INDEX membership_interval_current ON membership_intervals(evidence_rowid) WHERE until_seq IS NULL;
CREATE TABLE interpretation_intervals(
 evidence_rowid INTEGER NOT NULL REFERENCES evidence(rowid),
 interpretation_id TEXT NOT NULL REFERENCES interpretations(id),
 from_seq INTEGER NOT NULL REFERENCES publication_order(seq),
 until_seq INTEGER REFERENCES publication_order(seq),
 PRIMARY KEY(evidence_rowid,from_seq), CHECK(until_seq IS NULL OR until_seq>from_seq)
);
CREATE INDEX interpretation_interval_range ON interpretation_intervals(from_seq,until_seq,evidence_rowid);
CREATE UNIQUE INDEX interpretation_interval_current ON interpretation_intervals(evidence_rowid) WHERE until_seq IS NULL;
-- Islands of consecutive publication ordinals preserve removals, re-additions,
-- branches and interpretation replacements without assuming timestamp ordering.
INSERT INTO membership_intervals
 SELECT evidence_rowid,min(seq),CASE WHEN max(seq)=(SELECT max(seq) FROM publication_order) THEN NULL ELSE max(seq)+1 END
 FROM (SELECT m.evidence_rowid,p.seq,p.seq-row_number() OVER(PARTITION BY m.evidence_rowid ORDER BY p.seq) AS island FROM membership m JOIN publication_order p ON p.id=m.publication_id)
 GROUP BY evidence_rowid,island;
INSERT INTO interpretation_intervals
 SELECT evidence_rowid,interpretation_id,min(seq),CASE WHEN max(seq)=(SELECT max(seq) FROM publication_order) THEN NULL ELSE max(seq)+1 END
 FROM (SELECT m.evidence_rowid,m.interpretation_id,p.seq,p.seq-row_number() OVER(PARTITION BY m.evidence_rowid,m.interpretation_id ORDER BY p.seq) AS island FROM publication_interpretations m JOIN publication_order p ON p.id=m.publication_id)
 GROUP BY evidence_rowid,interpretation_id,island;
DROP TABLE publication_interpretations;
DROP TABLE membership;
CREATE VIEW membership AS
 SELECT p.id AS publication_id,m.evidence_rowid FROM publication_order p JOIN membership_intervals m ON m.from_seq<=p.seq AND (m.until_seq IS NULL OR m.until_seq>p.seq);
CREATE VIEW publication_interpretations AS
 SELECT p.id AS publication_id,m.evidence_rowid,m.interpretation_id FROM publication_order p JOIN interpretation_intervals m ON m.from_seq<=p.seq AND (m.until_seq IS NULL OR m.until_seq>p.seq);
-- Writes may only affect the publication currently being assembled. Earlier
-- intervals close at its ordinal; their historical meaning never changes.
CREATE TRIGGER membership_add INSTEAD OF INSERT ON membership BEGIN
 SELECT CASE WHEN NEW.publication_id!=(SELECT id FROM publication_order ORDER BY seq DESC LIMIT 1) THEN RAISE(ABORT,'Historical publication is immutable') END;
 INSERT INTO membership_intervals SELECT NEW.evidence_rowid,(SELECT seq FROM publication_order WHERE id=NEW.publication_id),NULL
 WHERE NOT EXISTS(SELECT 1 FROM membership_intervals WHERE evidence_rowid=NEW.evidence_rowid AND until_seq IS NULL);
END;
CREATE TRIGGER membership_remove INSTEAD OF DELETE ON membership BEGIN
 SELECT CASE WHEN OLD.publication_id!=(SELECT id FROM publication_order ORDER BY seq DESC LIMIT 1) THEN RAISE(ABORT,'Historical publication is immutable') END;
 DELETE FROM membership_intervals WHERE evidence_rowid=OLD.evidence_rowid AND from_seq=(SELECT seq FROM publication_order WHERE id=OLD.publication_id);
 UPDATE membership_intervals SET until_seq=(SELECT seq FROM publication_order WHERE id=OLD.publication_id) WHERE evidence_rowid=OLD.evidence_rowid AND until_seq IS NULL;
 DELETE FROM interpretation_intervals WHERE evidence_rowid=OLD.evidence_rowid AND from_seq=(SELECT seq FROM publication_order WHERE id=OLD.publication_id);
 UPDATE interpretation_intervals SET until_seq=(SELECT seq FROM publication_order WHERE id=OLD.publication_id) WHERE evidence_rowid=OLD.evidence_rowid AND until_seq IS NULL;
END;
CREATE TRIGGER interpretation_add INSTEAD OF INSERT ON publication_interpretations BEGIN
 SELECT CASE WHEN NEW.publication_id!=(SELECT id FROM publication_order ORDER BY seq DESC LIMIT 1) THEN RAISE(ABORT,'Historical publication is immutable') END;
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM membership WHERE publication_id=NEW.publication_id AND evidence_rowid=NEW.evidence_rowid) THEN RAISE(ABORT,'Interpretation must belong to publication') END;
 DELETE FROM interpretation_intervals WHERE evidence_rowid=NEW.evidence_rowid AND interpretation_id!=NEW.interpretation_id AND from_seq=(SELECT seq FROM publication_order WHERE id=NEW.publication_id);
 UPDATE interpretation_intervals SET until_seq=(SELECT seq FROM publication_order WHERE id=NEW.publication_id) WHERE evidence_rowid=NEW.evidence_rowid AND interpretation_id!=NEW.interpretation_id AND until_seq IS NULL;
 INSERT INTO interpretation_intervals SELECT NEW.evidence_rowid,NEW.interpretation_id,(SELECT seq FROM publication_order WHERE id=NEW.publication_id),NULL
 WHERE NOT EXISTS(SELECT 1 FROM interpretation_intervals WHERE evidence_rowid=NEW.evidence_rowid AND until_seq IS NULL);
END;
INSERT OR REPLACE INTO metadata VALUES('publication_storage','interval-v1');
