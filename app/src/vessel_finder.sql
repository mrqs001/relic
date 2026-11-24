-- Ship finder: one retained, paid extraction per message revision and prompt
-- profile, and the analyst's decisions on KeelWatch candidates. Suggestions
-- are separate from readings; nothing here changes a reading.
-- `billing` says what an attempt could have cost: 'sent' (the request may have
-- reached the provider; cost_usd is its known cost, or NULL when unknown),
-- 'not_sent' (it never left Relic) or 'rejected' (OpenRouter refused it for
-- credits before any generation). The last two are known $0 and the message
-- returns to the ship finder's backlog.
CREATE TABLE IF NOT EXISTS vessel_lookups(
 id TEXT PRIMARY KEY, evidence_rowid INTEGER NOT NULL REFERENCES evidence(rowid),
 profile TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, cost_usd REAL,
 request_hash TEXT NOT NULL REFERENCES objects(hash), receipt_hash TEXT REFERENCES objects(hash),
 result_json TEXT, error TEXT, billing TEXT NOT NULL DEFAULT 'sent', UNIQUE(evidence_rowid, profile)
);
-- Attempts a retry replaced, kept as they were: a retry never erases what an
-- earlier request cost, or that its cost is unknown.
CREATE TABLE IF NOT EXISTS vessel_lookup_attempts(
 id TEXT PRIMARY KEY, evidence_rowid INTEGER NOT NULL REFERENCES evidence(rowid),
 profile TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, cost_usd REAL, billing TEXT NOT NULL,
 request_hash TEXT NOT NULL REFERENCES objects(hash), receipt_hash TEXT REFERENCES objects(hash),
 error TEXT, replaced_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS vessel_lookup_attempts_evidence ON vessel_lookup_attempts(evidence_rowid, profile);
-- Free detection per evidence revision ("may name a ship", within the size
-- limit), filled incrementally so status polls never rescan message text.
-- Rebuilt when the detector changes; metadata `vessel_finder:detected_through`
-- holds the detector version and the last evidence rowid checked.
CREATE TABLE IF NOT EXISTS vessel_detections(
 evidence_rowid INTEGER PRIMARY KEY REFERENCES evidence(rowid), flagged INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS vessel_detections_flagged ON vessel_detections(evidence_rowid) WHERE flagged=1;
CREATE TABLE IF NOT EXISTS vessel_decisions(
 lookup_id TEXT NOT NULL REFERENCES vessel_lookups(id) ON DELETE CASCADE,
 mention INTEGER NOT NULL, vessel_key TEXT NOT NULL, decision TEXT NOT NULL, decided_at TEXT NOT NULL,
 PRIMARY KEY(lookup_id, mention, vessel_key)
);
-- The candidate each mention points to at its latest match, so the feed and
-- the vessel card need no KeelWatch call. Refreshed whenever matches are made;
-- a suggestion is never a decision.
CREATE TABLE IF NOT EXISTS vessel_suggestions(
 lookup_id TEXT NOT NULL REFERENCES vessel_lookups(id) ON DELETE CASCADE,
 mention INTEGER NOT NULL, vessel_key TEXT, suggested INTEGER NOT NULL, verdict TEXT NOT NULL, matched_at TEXT NOT NULL,
 PRIMARY KEY(lookup_id, mention)
);
-- Display identity of every candidate shown, for labels without KeelWatch.
CREATE TABLE IF NOT EXISTS vessel_labels(
 vessel_key TEXT PRIMARY KEY, name TEXT, category TEXT, flag TEXT, mmsi TEXT, imo TEXT, updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS vessel_labels_mmsi ON vessel_labels(mmsi);
CREATE INDEX IF NOT EXISTS vessel_suggestions_vessel ON vessel_suggestions(vessel_key);
CREATE INDEX IF NOT EXISTS vessel_decisions_vessel ON vessel_decisions(vessel_key);
