PRAGMA foreign_keys=ON;
CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT OR IGNORE INTO metadata VALUES ('schema_version','1');
CREATE TABLE IF NOT EXISTS sources(id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS objects(hash TEXT PRIMARY KEY, bytes INTEGER NOT NULL, media_type TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS evidence(
 rowid INTEGER PRIMARY KEY, id TEXT NOT NULL, revision TEXT NOT NULL, source TEXT NOT NULL REFERENCES sources(id),
 acquired_at TEXT NOT NULL, published_at TEXT, lon REAL, lat REAL, origin_group TEXT, text TEXT NOT NULL,
 object_hash TEXT NOT NULL REFERENCES objects(hash), json TEXT NOT NULL, UNIQUE(id,revision)
);
CREATE INDEX IF NOT EXISTS evidence_identity ON evidence(id,acquired_at DESC,revision DESC);
CREATE INDEX IF NOT EXISTS evidence_source ON evidence(source);
CREATE VIRTUAL TABLE IF NOT EXISTS evidence_geo USING rtree(rowid,min_lon,max_lon,min_lat,max_lat);
-- The viewer contract is literal substring search across source + translation.
-- The old token index served no queries; retire only this rebuildable projection.
DROP TRIGGER IF EXISTS evidence_insert;
DROP TABLE IF EXISTS evidence_fts;
CREATE TRIGGER IF NOT EXISTS evidence_insert AFTER INSERT ON evidence BEGIN
 INSERT INTO evidence_geo SELECT new.rowid,new.lon,new.lon,new.lat,new.lat WHERE new.lon IS NOT NULL;
END;
CREATE TABLE IF NOT EXISTS publications(id TEXT PRIMARY KEY,label TEXT NOT NULL,created_at TEXT NOT NULL,status TEXT NOT NULL,object_hash TEXT REFERENCES objects(hash));
CREATE TABLE IF NOT EXISTS membership(publication_id TEXT NOT NULL REFERENCES publications(id), evidence_rowid INTEGER NOT NULL REFERENCES evidence(rowid), PRIMARY KEY(publication_id,evidence_rowid));

CREATE TABLE IF NOT EXISTS investigations(id TEXT PRIMARY KEY,json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS briefs(id TEXT NOT NULL,revision TEXT NOT NULL,version INTEGER NOT NULL,json TEXT NOT NULL,PRIMARY KEY(id,revision));
CREATE TABLE IF NOT EXISTS lineage_nodes(id TEXT PRIMARY KEY,kind TEXT NOT NULL,label TEXT NOT NULL,detail TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS lineage_edges(id TEXT PRIMARY KEY,source TEXT NOT NULL REFERENCES lineage_nodes(id),target TEXT NOT NULL REFERENCES lineage_nodes(id),relationship TEXT NOT NULL,detail TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS lineage_source ON lineage_edges(source);
CREATE INDEX IF NOT EXISTS lineage_target ON lineage_edges(target);
CREATE TABLE IF NOT EXISTS watches(id TEXT PRIMARY KEY,json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS watch_seen(watch_id TEXT NOT NULL REFERENCES watches(id),identity TEXT NOT NULL,revision TEXT NOT NULL,origin_group TEXT,PRIMARY KEY(watch_id,identity));
CREATE TABLE IF NOT EXISTS alerts(id TEXT PRIMARY KEY,watch_id TEXT NOT NULL REFERENCES watches(id),group_key TEXT NOT NULL,json TEXT NOT NULL,UNIQUE(watch_id,group_key));
CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY,json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS import_bundles(hash TEXT PRIMARY KEY,publication_id TEXT NOT NULL REFERENCES publications(id));
CREATE TABLE IF NOT EXISTS events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS acquisitions(id TEXT PRIMARY KEY,semantic_hash TEXT NOT NULL,object_hash TEXT NOT NULL REFERENCES objects(hash),publication_id TEXT NOT NULL REFERENCES publications(id),created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS evidence_acquisitions(evidence_rowid INTEGER NOT NULL REFERENCES evidence(rowid),acquisition_id TEXT NOT NULL REFERENCES acquisitions(id),PRIMARY KEY(evidence_rowid,acquisition_id));
CREATE TABLE IF NOT EXISTS subscriptions(
 rowid INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, version INTEGER NOT NULL CHECK(version > 0),
 provider TEXT NOT NULL CHECK(provider IN ('telegram','x')), label TEXT NOT NULL, target TEXT NOT NULL,
 mode TEXT NOT NULL CHECK(mode IN ('stream','poll')), poll_interval_seconds INTEGER,
 enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 search_text TEXT NOT NULL, UNIQUE(provider,target),
 CHECK((provider='telegram' AND mode='stream' AND poll_interval_seconds IS NULL)
    OR (provider='x' AND mode='poll' AND poll_interval_seconds IS NOT NULL AND poll_interval_seconds BETWEEN 30 AND 86400))
);
CREATE TABLE IF NOT EXISTS collection_states(
 subscription_id TEXT PRIMARY KEY REFERENCES subscriptions(id),
 configuration_version INTEGER NOT NULL CHECK(configuration_version > 0),
 generation INTEGER NOT NULL CHECK(generation > 0),
 json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS media_assets(
 evidence_id TEXT NOT NULL, revision TEXT NOT NULL, asset_index INTEGER NOT NULL CHECK(asset_index >= 0 AND asset_index < 100),
 hash TEXT NOT NULL REFERENCES objects(hash), media_type TEXT NOT NULL, bytes INTEGER NOT NULL CHECK(bytes > 0 AND bytes <= 536870912),
 PRIMARY KEY(evidence_id,revision,asset_index), FOREIGN KEY(evidence_id,revision) REFERENCES evidence(id,revision)
);
CREATE TABLE IF NOT EXISTS interpretations(
 id TEXT PRIMARY KEY, evidence_rowid INTEGER NOT NULL REFERENCES evidence(rowid),
 recipe TEXT NOT NULL, created_at TEXT NOT NULL, kind TEXT NOT NULL,
 lon REAL, lat REAL, related_key TEXT, json TEXT NOT NULL,
 object_hash TEXT NOT NULL REFERENCES objects(hash), UNIQUE(evidence_rowid,recipe)
);
CREATE INDEX IF NOT EXISTS interpretation_related ON interpretations(related_key);
CREATE TABLE IF NOT EXISTS publication_interpretations(
 publication_id TEXT NOT NULL, evidence_rowid INTEGER NOT NULL,
 interpretation_id TEXT NOT NULL REFERENCES interpretations(id),
 PRIMARY KEY(publication_id,evidence_rowid),
 FOREIGN KEY(publication_id,evidence_rowid) REFERENCES membership(publication_id,evidence_rowid)
);
CREATE TABLE IF NOT EXISTS processing_attempts(
 id TEXT PRIMARY KEY, evidence_rowid INTEGER NOT NULL REFERENCES evidence(rowid),
 day TEXT NOT NULL, created_at TEXT NOT NULL, status TEXT NOT NULL,
 reserved_usd REAL NOT NULL, cost_usd REAL, json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS processing_attempt_day ON processing_attempts(day);
CREATE INDEX IF NOT EXISTS processing_attempt_evidence ON processing_attempts(evidence_rowid,created_at);
CREATE VIEW IF NOT EXISTS reading_evidence AS
 SELECT e.rowid,e.id,e.revision,e.source,e.acquired_at,e.published_at,
 coalesce(e.lon,i.lon) AS lon,coalesce(e.lat,i.lat) AS lat,e.origin_group,e.text,
 e.text||char(10)||coalesce(json_extract(i.json,'$.translation'),'') AS search_text,
 coalesce(i.kind,'pending') AS kind,i.related_key,m.publication_id,i.json AS reading_json,
 CASE WHEN i.id IS NULL THEN e.json ELSE
 json_set(e.json,'$.interpretation',json(i.json),
 '$.geometry',CASE WHEN e.lon IS NOT NULL THEN json_extract(e.json,'$.geometry') ELSE json_extract(i.json,'$.geometry') END,
 '$.location_precision',CASE WHEN e.lon IS NOT NULL THEN json_extract(e.json,'$.location_precision') ELSE coalesce(json_extract(i.json,'$.location_precision'),'unresolved') END) END AS json
 FROM membership m JOIN evidence e ON e.rowid=m.evidence_rowid
 LEFT JOIN interpretation_intervals pi ON pi.evidence_rowid=e.rowid AND pi.from_seq<=(SELECT seq FROM publication_order WHERE id=m.publication_id) AND (pi.until_seq IS NULL OR pi.until_seq>(SELECT seq FROM publication_order WHERE id=m.publication_id))
 LEFT JOIN interpretations i ON i.id=pi.interpretation_id;

-- Rebuildable projection of immutable interpretations. Source coordinates stay
-- on evidence; these rows describe only supported reported event locations.
CREATE TABLE IF NOT EXISTS interpretation_sites(
 interpretation_id TEXT NOT NULL REFERENCES interpretations(id),
 site_key TEXT NOT NULL,event_type TEXT NOT NULL,
 lon REAL NOT NULL,lat REAL NOT NULL,label TEXT,
 PRIMARY KEY(interpretation_id,site_key,event_type)
);
CREATE TABLE IF NOT EXISTS interpretation_event_states(
 interpretation_id TEXT NOT NULL REFERENCES interpretations(id),event_type TEXT NOT NULL,state TEXT NOT NULL,
 PRIMARY KEY(interpretation_id,event_type,state)
);
CREATE VIEW IF NOT EXISTS projected_interpretation_event_states AS
 SELECT i.id AS interpretation_id,json_extract(ev.value,'$.type') AS event_type,json_extract(ev.value,'$.state') AS state
 FROM interpretations i,json_each(i.json,'$.events') ev
 WHERE json_extract(ev.value,'$.type') IS NOT NULL;
CREATE VIEW IF NOT EXISTS reading_event_states AS
 SELECT pi.publication_id,pi.evidence_rowid,s.event_type,s.state
 FROM publication_interpretations pi JOIN interpretation_event_states s ON s.interpretation_id=pi.interpretation_id;
CREATE TABLE IF NOT EXISTS interpretation_geography_indexed(
 interpretation_id TEXT PRIMARY KEY REFERENCES interpretations(id)
);
CREATE VIEW IF NOT EXISTS projected_interpretation_sites AS
 SELECT i.id AS interpretation_id,
 json_extract(loc.value,'$.selected_candidate_id') AS site_key,
 json_extract(ev.value,'$.type') AS event_type,
 json_extract(loc.value,'$.geometry.coordinates[0]') AS lon,
 json_extract(loc.value,'$.geometry.coordinates[1]') AS lat,
 coalesce((SELECT json_extract(c.value,'$.name') FROM json_each(loc.value,'$.candidates') c WHERE json_extract(c.value,'$.id')=json_extract(loc.value,'$.selected_candidate_id') LIMIT 1),json_extract(loc.value,'$.mention')) AS label
 FROM interpretations i,json_each(i.json,'$.events') ev,json_each(ev.value,'$.locations') loc
 WHERE json_extract(ev.value,'$.state')='reported'
 AND json_extract(loc.value,'$.role')='event'
 AND json_extract(loc.value,'$.occurrence_eligible')=1
 AND json_extract(loc.value,'$.selected_candidate_id') IS NOT NULL
 AND json_extract(loc.value,'$.geometry.type')='Point';
DROP TRIGGER IF EXISTS index_interpretation_sites;
CREATE TRIGGER index_interpretation_sites AFTER INSERT ON interpretations
BEGIN
 INSERT OR IGNORE INTO interpretation_sites SELECT * FROM projected_interpretation_sites WHERE interpretation_id=NEW.id;
 INSERT OR IGNORE INTO interpretation_event_states SELECT * FROM projected_interpretation_event_states WHERE interpretation_id=NEW.id;
 INSERT INTO interpretation_geography_indexed VALUES(NEW.id);
END;
-- Rebuildable preview of each immutable reading for map points and area
-- choices: its displayed title (headline, else translation), the type/state of
-- each event in reading order, its location label and its event count.
-- `title` and `location_label` have no column affinity, so they keep the value
-- json_extract returns. Map, area and site views read this instead of parsing
-- the whole reading document once per point, area member or located message.
CREATE TABLE IF NOT EXISTS interpretation_previews(
 interpretation_id TEXT PRIMARY KEY REFERENCES interpretations(id),
 title, events TEXT, location_label, event_count INTEGER NOT NULL
);
CREATE VIEW IF NOT EXISTS projected_interpretation_previews AS
 SELECT i.id AS interpretation_id,
 coalesce(nullif(json_extract(i.json,'$.headline'),''),nullif(json_extract(i.json,'$.translation'),'')) AS title,
 CASE WHEN json_type(i.json,'$.events')='array' THEN (SELECT json_group_array(CASE WHEN ev.type='object' THEN json_object('type',ev.value->'$.type','state',ev.value->'$.state') ELSE json_object('type',NULL,'state',NULL) END ORDER BY ev.key) FROM json_each(i.json,'$.events') ev) END AS events,
 json_extract(i.json,'$.location_label') AS location_label,
 coalesce(json_array_length(i.json,'$.events'),0) AS event_count
 FROM interpretations i;
CREATE TRIGGER IF NOT EXISTS index_interpretation_previews AFTER INSERT ON interpretations
BEGIN
 INSERT OR REPLACE INTO interpretation_previews SELECT * FROM projected_interpretation_previews WHERE interpretation_id=NEW.id;
END;
CREATE TRIGGER IF NOT EXISTS reindex_interpretation_previews AFTER UPDATE OF json ON interpretations
BEGIN
 INSERT OR REPLACE INTO interpretation_previews SELECT * FROM projected_interpretation_previews WHERE interpretation_id=NEW.id;
END;
-- Match UNION column affinities so SQLite pushes publication/message predicates
-- into both branches instead of scanning historical publications per message.
CREATE VIEW IF NOT EXISTS reading_message_sites AS
 SELECT pi.evidence_rowid,pi.publication_id,s.site_key,s.lon,s.lat,s.label,1 AS approximate,s.event_type
 FROM publication_interpretations pi JOIN interpretation_sites s ON s.interpretation_id=pi.interpretation_id
 UNION ALL
 SELECT e.rowid,m.publication_id,CAST('single' AS TEXT),CAST(coalesce(e.lon,i.lon) AS REAL),CAST(coalesce(e.lat,i.lat) AS REAL),
 CAST(CASE WHEN e.lon IS NOT NULL THEN json_extract(e.json,'$.location_precision') ELSE ip.location_label END AS TEXT),
 e.lon IS NULL,CAST(NULL AS TEXT)
 FROM membership m JOIN evidence e ON e.rowid=m.evidence_rowid
 LEFT JOIN interpretation_intervals pi ON pi.evidence_rowid=e.rowid AND pi.from_seq<=(SELECT seq FROM publication_order WHERE id=m.publication_id) AND (pi.until_seq IS NULL OR pi.until_seq>(SELECT seq FROM publication_order WHERE id=m.publication_id))
 LEFT JOIN interpretations i ON i.id=pi.interpretation_id
 LEFT JOIN interpretation_previews ip ON ip.interpretation_id=i.id
 WHERE e.lon IS NOT NULL OR (i.lon IS NOT NULL AND ip.event_count=0);

-- Immutable accepted analytical/display boundary pairs, distinct from evidence.
CREATE TABLE IF NOT EXISTS boundary_shapes(
 id TEXT PRIMARY KEY,edition TEXT NOT NULL,iso2 TEXT NOT NULL,iso3 TEXT NOT NULL,level TEXT NOT NULL,
 shape_id TEXT NOT NULL,name TEXT NOT NULL,full_json TEXT NOT NULL,display_json TEXT NOT NULL,
 bounds TEXT NOT NULL,metadata TEXT NOT NULL,manifest_hash TEXT NOT NULL REFERENCES objects(hash)
);
CREATE INDEX IF NOT EXISTS boundary_edition ON boundary_shapes(edition,iso2,level);
CREATE TABLE IF NOT EXISTS interpretation_areas(
 interpretation_id TEXT NOT NULL REFERENCES interpretations(id),area_id TEXT NOT NULL REFERENCES boundary_shapes(id),event_type TEXT NOT NULL,
 PRIMARY KEY(interpretation_id,area_id,event_type)
);
CREATE TABLE IF NOT EXISTS interpretation_areas_indexed(interpretation_id TEXT PRIMARY KEY REFERENCES interpretations(id));
CREATE VIEW IF NOT EXISTS projected_interpretation_areas AS
 SELECT i.id AS interpretation_id,json_extract(loc.value,'$.area.id') AS area_id,json_extract(ev.value,'$.type') AS event_type
 FROM interpretations i,json_each(i.json,'$.events') ev,json_each(ev.value,'$.locations') loc
 WHERE json_extract(ev.value,'$.state')='reported' AND json_extract(loc.value,'$.role')='event'
 AND json_extract(loc.value,'$.occurrence_eligible')=1 AND json_extract(loc.value,'$.area.id') IS NOT NULL;
CREATE TRIGGER IF NOT EXISTS index_interpretation_areas AFTER INSERT ON interpretations
BEGIN
 INSERT OR IGNORE INTO interpretation_areas SELECT * FROM projected_interpretation_areas WHERE interpretation_id=NEW.id;
 INSERT INTO interpretation_areas_indexed VALUES(NEW.id);
END;
CREATE VIEW IF NOT EXISTS reading_message_areas AS
 SELECT pi.publication_id,pi.evidence_rowid,a.area_id,a.event_type FROM publication_interpretations pi JOIN interpretation_areas a ON a.interpretation_id=pi.interpretation_id;

-- Advisory outputs never enter interpretations, membership or map/filter indexes.
CREATE TABLE IF NOT EXISTS supplemental_attempts(
 id TEXT PRIMARY KEY,evidence_rowid INTEGER NOT NULL REFERENCES evidence(rowid),
 recipe TEXT NOT NULL,phase TEXT NOT NULL,context_hash TEXT NOT NULL,
 publication_id TEXT NOT NULL REFERENCES publications(id),interpretation_id TEXT,
 created_at TEXT NOT NULL,day TEXT NOT NULL,status TEXT NOT NULL,
 reserved_usd REAL NOT NULL,cost_usd REAL,
 request_hash TEXT NOT NULL REFERENCES objects(hash),audit_hash TEXT REFERENCES objects(hash),
 result_json TEXT,error TEXT,
 UNIQUE(evidence_rowid,recipe,phase)
);
CREATE INDEX IF NOT EXISTS supplemental_day ON supplemental_attempts(day);
CREATE TABLE IF NOT EXISTS supplemental_exclusions(
 evidence_rowid INTEGER NOT NULL REFERENCES evidence(rowid),recipe TEXT NOT NULL,phase TEXT NOT NULL,reason TEXT NOT NULL,
 PRIMARY KEY(evidence_rowid,recipe,phase)
);

CREATE TABLE IF NOT EXISTS collection_gaps (
 id TEXT PRIMARY KEY, subscription_id TEXT NOT NULL REFERENCES subscriptions(id),
 created_at TEXT NOT NULL, json TEXT NOT NULL
);
