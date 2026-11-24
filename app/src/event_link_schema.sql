-- Rebuildable claim projection; immutable pairs retain exact interpretation IDs.
CREATE TABLE IF NOT EXISTS event_link_indexed(
 interpretation_id TEXT PRIMARY KEY REFERENCES interpretations(id)
);
CREATE TABLE IF NOT EXISTS event_link_claims(
 id TEXT PRIMARY KEY, evidence_rowid INTEGER NOT NULL REFERENCES evidence(rowid),
 interpretation_id TEXT NOT NULL REFERENCES interpretations(id),
 place TEXT NOT NULL, event_type TEXT NOT NULL, published_millis INTEGER NOT NULL,
 json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS event_claim_candidates ON event_link_claims(place,event_type,published_millis,id);
CREATE INDEX IF NOT EXISTS event_claim_place_time ON event_link_claims(place,published_millis,id);
CREATE INDEX IF NOT EXISTS event_claim_reading ON event_link_claims(interpretation_id,evidence_rowid);
CREATE TABLE IF NOT EXISTS event_link_attempts(
 id TEXT PRIMARY KEY, profile TEXT NOT NULL, status TEXT NOT NULL,
 created_at TEXT NOT NULL, cost_usd REAL, request_hash TEXT NOT NULL REFERENCES objects(hash),
 receipt_hash TEXT REFERENCES objects(hash), review_json TEXT, error TEXT, retry_at TEXT
);
CREATE TABLE IF NOT EXISTS event_link_pairs(
 a TEXT NOT NULL REFERENCES event_link_claims(id),b TEXT NOT NULL REFERENCES event_link_claims(id),
 profile TEXT NOT NULL, attempt_id TEXT REFERENCES event_link_attempts(id),
 state TEXT NOT NULL, PRIMARY KEY(a,b,profile)
);
-- Pairs carry the review + retrieval admission identity; attempts retain the
-- paid review identity. Old rows remain inspectable and exact requests reusable.
CREATE TABLE IF NOT EXISTS event_links(
 id TEXT PRIMARY KEY, a TEXT NOT NULL REFERENCES event_link_claims(id),
 b TEXT NOT NULL REFERENCES event_link_claims(id), attempt_id TEXT NOT NULL REFERENCES event_link_attempts(id),
 publication_id TEXT NOT NULL REFERENCES publications(id), relation TEXT NOT NULL,
 json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS event_link_a ON event_links(a);
CREATE INDEX IF NOT EXISTS event_link_b ON event_links(b);
CREATE TABLE IF NOT EXISTS event_link_receipts(
 attempt_id TEXT NOT NULL REFERENCES event_link_attempts(id),
 object_hash TEXT NOT NULL REFERENCES objects(hash), PRIMARY KEY(attempt_id,object_hash)
);
CREATE TABLE IF NOT EXISTS event_link_projection_errors(
 interpretation_id TEXT PRIMARY KEY REFERENCES interpretations(id), error TEXT NOT NULL
);
-- Semantic candidate retrieval. Vectors are keyed by the prepared claim text,
-- so re-readings with unchanged claims reuse them. Retrieval only: the paid
-- pair review above still decides every relationship.
CREATE TABLE IF NOT EXISTS event_embedding_attempts(
 id TEXT PRIMARY KEY, status TEXT NOT NULL, created_at TEXT NOT NULL, cost_usd REAL,
 request_hash TEXT NOT NULL REFERENCES objects(hash), receipt_hash TEXT REFERENCES objects(hash), error TEXT
);
CREATE TABLE IF NOT EXISTS event_claim_text(
 claim_id TEXT PRIMARY KEY REFERENCES event_link_claims(id), text_hash TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS event_claim_text_hash ON event_claim_text(text_hash);
CREATE TABLE IF NOT EXISTS event_claim_vectors(
 text_hash TEXT PRIMARY KEY, attempt_id TEXT NOT NULL REFERENCES event_embedding_attempts(id), vector BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS event_semantic_scanned(claim_id TEXT PRIMARY KEY REFERENCES event_link_claims(id));
CREATE TABLE IF NOT EXISTS event_semantic_candidates(
 a TEXT NOT NULL REFERENCES event_link_claims(id), b TEXT NOT NULL REFERENCES event_link_claims(id),
 cosine REAL NOT NULL, PRIMARY KEY(a,b)
);
