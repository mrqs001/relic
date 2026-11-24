use relic_app::{
    advisories,
    config::Config,
    live_layers::LiveLayers,
    store::Store,
    vessel_batch::VesselBatch,
    vessel_finder::{self, evaluation},
};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};

fn workspace() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let s = Store::open(Config::local(root, tmp.path().join("data"), "127.0.0.1:8797".parse().unwrap())).unwrap();
    (tmp, s)
}

/// A completed lookup for a fictional message, written as a finished attempt.
fn completed_lookup(s: &Store, id: &str, mentions: serde_json::Value) -> (String, String) {
    let c = s.connect().unwrap();
    let (rowid, revision): (i64, String) = c.query_row("SELECT rowid,revision FROM evidence WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    c.execute("INSERT OR IGNORE INTO objects VALUES('sha256:fictional-request',2,'application/json')", []).unwrap();
    let lookup = format!("vessels:{rowid}");
    c.execute(
        "INSERT INTO vessel_lookups(id,evidence_rowid,profile,status,created_at,request_hash,result_json,cost_usd) VALUES(?1,?2,?3,'completed','2026-09-30T12:00:00Z','sha256:fictional-request',?4,0.0001)",
        rusqlite::params![lookup, rowid, vessel_finder::profile_key(), mentions.to_string()],
    )
    .unwrap();
    (lookup, revision)
}

#[test]
fn feed_marks_show_decisions_over_suggestions_and_the_vessel_lists_its_messages() {
    let (_tmp, s) = workspace();
    let body = json!({"data":[
        {"AlertId":"201","Latitude":"12.6","Longitude":"43.3","RegistrationDate":"2026-09-20T10:00:00.000","ThreatDescription":"Fictional tanker FICTION STAR reported an approach.","AlertType":"8"},
        {"AlertId":"202","Latitude":"12.7","Longitude":"43.2","RegistrationDate":"2026-09-21T10:00:00.000","ThreatDescription":"Fictional bulk carrier PAPER MOON was boarded.","AlertType":"2"}
    ]});
    let (warnings, _) = advisories::parse(&body).unwrap();
    s.retain_warnings(&warnings, &serde_json::to_vec(&body).unwrap()).unwrap();
    let (first, r1) = completed_lookup(&s, "mscio:alert:201", json!([{"name":"FICTION STAR","quote":"FICTION STAR","span":[16,28]}]));
    let (second, r2) = completed_lookup(&s, "mscio:alert:202", json!([{"name":"PAPER MOON","quote":"PAPER MOON","span":[29,39]}]));
    let candidate = |key: &str, name: &str, mmsi: u64| json!({"key":key,"name":name,"category":"tanker","flag":"PA","mmsi":mmsi,"imo":null});
    s.save_vessel_matches(&first, &[json!({"candidates":[candidate("kw-1","FICTION STAR",111), candidate("kw-2","FICTION STAR",222)],"suggested":true,"verdict":"Only one was near."})]).unwrap();
    s.save_vessel_matches(&second, &[json!({"candidates":[candidate("kw-3","PAPER MOON",333)],"suggested":false,"verdict":"Several could be meant."})]).unwrap();
    let items = vec![("mscio:alert:201".to_string(), r1.clone()), ("mscio:alert:202".to_string(), r2.clone())];

    // A suggestion shows; an unsuggested match does not.
    let marks = s.vessel_marks(&items).unwrap()["marks"].clone();
    let key1 = format!("mscio:alert:201|{r1}");
    assert_eq!(marks[&key1][0]["state"], "suggested");
    assert_eq!(marks[&key1][0]["name"], "FICTION STAR");
    assert_eq!(marks[&key1][0]["quoted"], "FICTION STAR");
    assert!(marks.get(format!("mscio:alert:202|{r2}")).is_none());

    // Rejecting the suggestion hides it; confirming another vessel shows that one.
    s.decide_vessel(&first, 0, "kw-1", Some("rejected")).unwrap();
    assert!(s.vessel_marks(&items).unwrap()["marks"].get(&key1).is_none());
    s.decide_vessel(&first, 0, "kw-2", Some("confirmed")).unwrap();
    let marks = s.vessel_marks(&items).unwrap()["marks"].clone();
    assert_eq!(marks[&key1][0]["state"], "confirmed");
    assert_eq!(marks[&key1][0]["vessel_key"], "kw-2");
    assert_eq!(marks[&key1][0]["mmsi"], "222");

    // The vessel card finds its messages by key or, from a tile, by MMSI.
    assert_eq!(s.vessel_messages("kw-2", None).unwrap()["items"][0]["state"], "confirmed");
    let by_mmsi = s.vessel_messages("mmsi:222", Some("222")).unwrap();
    assert_eq!(by_mmsi["items"].as_array().unwrap().len(), 1);
    assert_eq!(by_mmsi["items"][0]["id"], "mscio:alert:201");
    assert_eq!(s.vessel_messages("kw-1", None).unwrap()["items"], json!([]));
}

#[test]
fn the_backlog_holds_flagged_messages_from_read_sources_without_an_attempt() {
    let (_tmp, s) = workspace();
    let body = json!({"data":[
        {"AlertId":"301","Latitude":"12.6","Longitude":"43.3","RegistrationDate":"2026-09-20T10:00:00.000","ThreatDescription":"Fictional tanker reported an approach.","AlertType":"8"},
        {"AlertId":"302","Latitude":"12.7","Longitude":"43.2","RegistrationDate":"2026-09-21T10:00:00.000","ThreatDescription":"Fictional drones seen over the coast.","AlertType":"8"}
    ]});
    let (warnings, _) = advisories::parse(&body).unwrap();
    s.retain_warnings(&warnings, &serde_json::to_vec(&body).unwrap()).unwrap();
    // Warnings are not read until their source is turned on.
    assert_eq!(s.vessel_backlog().unwrap(), (vec![], 0));
    let source: String = s.connect().unwrap().query_row("SELECT source FROM evidence WHERE id='mscio:alert:301'", [], |r| r.get(0)).unwrap();
    s.set_source_reading(&source, true).unwrap();
    let (todo, flagged) = s.vessel_backlog().unwrap();
    assert_eq!(flagged, 1, "only the message naming a vessel word is flagged");
    assert_eq!(todo.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["mscio:alert:301"]);
    assert!(todo[0].acquired_ms.is_some());
    completed_lookup(&s, "mscio:alert:301", json!([]));
    assert_eq!(s.vessel_backlog().unwrap(), (vec![], 1));
    // Detection is cached per revision and picks up new evidence incrementally.
    let more = json!({"data":[{"AlertId":"303","Latitude":"12.6","Longitude":"43.3","RegistrationDate":"2026-09-22T10:00:00.000","ThreatDescription":"Fictional vessel boarded.","AlertType":"2"}]});
    let (warnings, _) = advisories::parse(&more).unwrap();
    s.retain_warnings(&warnings, &serde_json::to_vec(&more).unwrap()).unwrap();
    let (todo, flagged) = s.vessel_backlog().unwrap();
    assert_eq!((todo.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), flagged), (vec!["mscio:alert:303"], 2));
    // A cache from another detector version is rebuilt with the same answer.
    s.connect().unwrap().execute("UPDATE metadata SET value='relic.vessel-detect.v0 999999' WHERE key='vessel_finder:detected_through'", []).unwrap();
    assert_eq!(s.vessel_backlog().unwrap().1, 2);
}

#[test]
fn attempts_that_cost_nothing_return_to_the_backlog() {
    let (_tmp, s) = workspace();
    let body = json!({"data":[
        {"AlertId":"401","Latitude":"12.6","Longitude":"43.3","RegistrationDate":"2026-09-20T10:00:00.000","ThreatDescription":"Fictional tanker one.","AlertType":"8"},
        {"AlertId":"402","Latitude":"12.6","Longitude":"43.3","RegistrationDate":"2026-09-21T10:00:00.000","ThreatDescription":"Fictional tanker two.","AlertType":"8"},
        {"AlertId":"403","Latitude":"12.6","Longitude":"43.3","RegistrationDate":"2026-09-22T10:00:00.000","ThreatDescription":"Fictional tanker three.","AlertType":"8"},
        {"AlertId":"404","Latitude":"12.6","Longitude":"43.3","RegistrationDate":"2026-09-23T10:00:00.000","ThreatDescription":"Fictional tanker four.","AlertType":"8"}
    ]});
    let (warnings, _) = advisories::parse(&body).unwrap();
    s.retain_warnings(&warnings, &serde_json::to_vec(&body).unwrap()).unwrap();
    let source: String = s.connect().unwrap().query_row("SELECT source FROM evidence WHERE id='mscio:alert:401'", [], |r| r.get(0)).unwrap();
    s.set_source_reading(&source, true).unwrap();
    // Never sent, refused for credits, failed after a billed response, unknown.
    for (id, status, billing, cost) in [("401", "failed", "not_sent", Some(0.0)), ("402", "failed", "rejected", Some(0.0)), ("403", "failed", "sent", Some(0.002)), ("404", "unknown", "sent", None)] {
        let c = s.connect().unwrap();
        let rowid: i64 = c.query_row("SELECT rowid FROM evidence WHERE id=?1", [format!("mscio:alert:{id}")], |r| r.get(0)).unwrap();
        c.execute("INSERT OR IGNORE INTO objects VALUES('sha256:fictional-request',2,'application/json')", []).unwrap();
        c.execute(
            "INSERT INTO vessel_lookups(id,evidence_rowid,profile,status,created_at,request_hash,cost_usd,billing) VALUES(?1,?2,?3,?4,'2026-09-30T12:00:00Z','sha256:fictional-request',?5,?6)",
            rusqlite::params![format!("vessels:{id}"), rowid, vessel_finder::profile_key(), status, cost, billing],
        )
        .unwrap();
    }
    let (todo, flagged) = s.vessel_backlog().unwrap();
    let mut waiting = todo.iter().map(|t| t.id.as_str()).collect::<Vec<_>>();
    waiting.sort();
    assert_eq!((waiting, flagged), (vec!["mscio:alert:401", "mscio:alert:402"], 4));
    let totals = s.vessel_totals().unwrap();
    assert_eq!((totals["attempted"].clone(), totals["failed"].clone(), totals["requests"].clone(), totals["unknown_cost_requests"].clone()), (json!(2), json!(2), json!(3), json!(1)));
    assert_eq!(totals["known_cost_usd"], 0.002);
}

#[test]
fn earlier_attempts_are_reclassified_only_by_what_their_errors_prove() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let config = || Config::local(root.clone(), tmp.path().join("data"), "127.0.0.1:8797".parse().unwrap());
    {
        let s = Store::open(config()).unwrap();
        let alerts = (0..6).map(|n| json!({"AlertId":format!("50{n}"),"Latitude":"12.6","Longitude":"43.3","RegistrationDate":format!("2026-09-2{n}T10:00:00.000"),"ThreatDescription":format!("Fictional tanker {n}."),"AlertType":"8"})).collect::<Vec<_>>();
        let body = json!({"data":alerts});
        let (warnings, _) = advisories::parse(&body).unwrap();
        s.retain_warnings(&warnings, &serde_json::to_vec(&body).unwrap()).unwrap();
        let c = s.connect().unwrap();
        c.execute("INSERT OR IGNORE INTO objects VALUES('fictional-object',1,'application/json')", []).unwrap();
        // The table as it was before attempts recorded their billing.
        c.execute("ALTER TABLE vessel_lookups DROP COLUMN billing", []).unwrap();
        let attempts = [
            ("failed", Some(0.0), "Cancelled before model dispatch; no request sent"),
            ("failed", Some(0.0), "OpenRouter credits are exhausted; nothing was generated or billed."),
            ("unknown", None, "AI processing is not configured"),
            ("unknown", None, "OpenRouter request failed; billing is unknown, no automatic retry"),
            ("failed", Some(0.003), "Model provider HTTP 500"),
            ("reserved", None, ""),
        ];
        for (n, (status, cost, error)) in attempts.iter().enumerate() {
            let rowid: i64 = c.query_row("SELECT rowid FROM evidence WHERE id=?1", [format!("mscio:alert:50{n}")], |r| r.get(0)).unwrap();
            c.execute("INSERT INTO vessel_lookups(id,evidence_rowid,profile,status,created_at,cost_usd,request_hash,error) VALUES(?1,?2,'p',?3,'2026-09-30T12:00:00Z',?4,'fictional-object',?5)", rusqlite::params![format!("vessels:{n}"), rowid, status, cost, error]).unwrap();
        }
    }
    let s = Store::open(config()).unwrap();
    let rows = s.connect().unwrap().prepare("SELECT status,billing,cost_usd FROM vessel_lookups ORDER BY id").unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<f64>>(2)?))).unwrap()
        .collect::<Result<Vec<_>, _>>().unwrap();
    let expected = [("failed", "not_sent", Some(0.0)), ("failed", "rejected", Some(0.0)), ("failed", "not_sent", Some(0.0)), ("unknown", "sent", None), ("failed", "sent", Some(0.003)), ("unknown", "sent", None)];
    assert_eq!(rows, expected.iter().map(|(a, b, c)| (a.to_string(), b.to_string(), *c)).collect::<Vec<_>>());
}

#[tokio::test]
async fn processing_stop_cancels_a_ship_finder_run_without_a_job_record() {
    let (_tmp, s) = workspace();
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    s.running.lock().unwrap().insert("processing:vessels:test".into(), flag.clone());
    let runtime = relic_app::processing::Runtime::new(s.clone());
    runtime.stop().await.unwrap();
    assert!(flag.load(std::sync::atomic::Ordering::Acquire));
}

fn case(id: &str) -> Value {
    evaluation::fixture().unwrap()["cases"].as_array().unwrap().iter().find(|c| c["id"] == id).cloned().unwrap()
}
/// Keep a reply for one case exactly as a paid run does: the request is
/// admitted first, then the raw reply and its receipt.
fn keep(dir: &Path, c: &Value, body: &[u8]) {
    assert!(evaluation::admit(dir, c).unwrap().is_some());
    evaluation::retain(dir, c, 200, body, 1, Some("gen-fictional")).unwrap();
}
/// A reply of the pinned profile listing these vessels.
fn reply(vessels: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"id":"gen-fictional","model":vessel_finder::MODEL,"provider":"OpenAI","usage":{"cost":0.0001},
        "choices":[{"finish_reason":"stop","message":{"role":"assistant","content":json!({"vessels":vessels}).to_string()}}]})).unwrap()
}
fn vessel(name: &str, quote: &str, latin: &str, imo: Option<&str>, mmsi: Option<&str>, flag: Option<&str>, vessel_type: Option<&str>) -> Value {
    json!({"name":name,"quote":quote,"name_latin":latin,"imo":imo,"mmsi":mmsi,"callsign":null,"hull_number":null,"flag":flag,"vessel_type":vessel_type})
}
/// A vessel invented for a message that names none.
fn najaf() -> Vec<u8> {
    reply(json!([vessel("النجف", "مؤتمر في النجف", "NAJAF", None, None, None, None)]))
}
/// Every case's gold reply kept in `dir`, except those `except` lists.
fn keep_gold(dir: &Path, except: &[&str]) {
    for c in evaluation::fixture().unwrap()["cases"].as_array().unwrap() {
        if !except.contains(&c["id"].as_str().unwrap()) {
            keep(dir, c, &serde_json::to_vec(&evaluation::gold_reply(c)).unwrap());
        }
    }
}
fn without_time(mut record: Value) -> Value {
    record.as_object_mut().unwrap().remove("checked_at");
    record
}

#[test]
fn replay_scores_retained_replies_deterministically_and_without_model_calls() {
    let dir = tempfile::tempdir().unwrap();
    // Four synthetic replies: exact, a vessel invented for a message naming
    // none, a quote not copied exactly, and an IMO left out.
    keep(dir.path(), &case("en_mt_imo"), &reply(json!([vessel("M/T Coral Meridian", "the tanker M/T Coral Meridian (IMO 3456781)", "CORAL MERIDIAN", Some("3456781"), None, None, Some("tanker"))])));
    keep(dir.path(), &case("ar_najaf"), &najaf());
    keep(dir.path(), &case("en_three_ships"), &reply(json!([vessel("Velvet Harrow", "the oil tanker Velvet Harrow", "VELVET HARROW", None, None, None, Some("tanker"))])));
    keep(dir.path(), &case("en_imo_mmsi_flag"), &reply(json!([vessel("Lumen Spire", "The Panama-flagged tanker Lumen Spire (IMO 3104522, MMSI 352001847)", "LUMEN SPIRE", None, Some("352001847"), Some("PA"), Some("tanker"))])));
    let files = std::fs::read_dir(dir.path()).unwrap().count();
    let first = evaluation::replay(dir.path()).unwrap();
    let second = evaluation::replay(dir.path()).unwrap();
    assert_eq!(without_time(first.clone()), without_time(second));
    // Replay writes nothing and calls nothing.
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), files);
    assert_eq!((first["mode"].clone(), first["new_calls"].clone()), (json!("replay"), json!(0)));
    let result = |id: &str| first["results"].as_array().unwrap().iter().find(|r| r["id"] == id).cloned().unwrap();
    assert_eq!(result("en_mt_imo")["passed"], true);
    assert_eq!((result("ar_najaf")["returned"].clone(), result("ar_najaf")["passed"].clone()), (json!(1), json!(false)));
    assert_eq!((result("en_three_ships")["decoded"].clone(), result("en_three_ships")["error"].clone()), (json!(false), json!("A vessel quote is not copied exactly from the message")));
    assert_eq!(result("en_imo_mmsi_flag")["identifier_errors"], json!([{"vessel":"Lumen Spire","field":"imo","expected":"3104522","returned":null}]));
    assert_eq!(result("en_hms")["attempted"], false);
    let s = &first["summary"];
    assert_eq!((s["replies"].clone(), s["decoded"].clone(), s["negative_vessels"].clone(), s["identifier_errors"].clone()), (json!(4), json!(3), json!(1), json!(1)));
    assert!((s["known_cost_usd"].as_f64().unwrap() - 0.0004).abs() < 1e-12);
    assert_eq!(first["passed"], false);
    assert_eq!(first["failures"][0], "37 of 40 cases have no reply that decodes with exact quotes");
    assert!(first["failures"].as_array().unwrap().iter().any(|f| f == "Vessels returned for messages that name none is 1, above the maximum 0"), "{}", first["failures"]);
    // A reply kept for another request (a changed prompt, schema or model) is
    // never scored as the current profile's.
    std::fs::write(dir.path().join("en_mt_imo.request.json"), b"{\"model\":\"another\"}").unwrap();
    assert!(evaluation::replay(dir.path()).unwrap_err().message.contains("differs from the current profile"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paid_extraction_needs_a_passed_evaluation_of_the_current_profile() {
    let (_tmp, s) = workspace();
    let body = json!({"data":[{"AlertId":"601","Latitude":"12.6","Longitude":"43.3","RegistrationDate":"2026-09-20T10:00:00.000","ThreatDescription":"Fictional tanker FICTION STAR reported an approach.","AlertType":"8"},
        {"AlertId":"602","Latitude":"12.6","Longitude":"43.3","RegistrationDate":"2026-09-21T10:00:00.000","ThreatDescription":"Fictional tanker PAPER MOON reported an approach.","AlertType":"8"}]});
    let (warnings, _) = advisories::parse(&body).unwrap();
    s.retain_warnings(&warnings, &serde_json::to_vec(&body).unwrap()).unwrap();
    let source: String = s.connect().unwrap().query_row("SELECT source FROM evidence WHERE id='mscio:alert:601'", [], |r| r.get(0)).unwrap();
    s.set_source_reading(&source, true).unwrap();
    let (lookup, revision) = completed_lookup(&s, "mscio:alert:601", json!([{"name":"FICTION STAR","quote":"FICTION STAR","span":[16,28]}]));
    s.save_vessel_matches(&lookup, &[json!({"candidates":[{"key":"kw-1","name":"FICTION STAR"}],"suggested":true,"verdict":"Only one."})]).unwrap();
    let other: String = s.connect().unwrap().query_row("SELECT revision FROM evidence WHERE id='mscio:alert:602'", [], |r| r.get(0)).unwrap();

    // No evaluation installed: Find ships and automatic mode refuse, and nothing is reserved.
    let view = s.vessel_lookup("mscio:alert:601", &revision).unwrap();
    assert_eq!((view["evaluated"].clone(), view["evaluation"].clone()), (json!(false), Value::Null));
    let refused = vessel_finder::extract(&s, "mscio:alert:602", &other).await.unwrap_err();
    assert_eq!((refused.message.as_str(), refused.status.as_u16()), (vessel_finder::NEEDS_EVALUATION, 409));
    let batch = VesselBatch::new(s.clone(), Arc::new(LiveLayers::new(&Default::default())));
    assert_eq!(batch.set_auto(true).unwrap_err().message, vessel_finder::NEEDS_EVALUATION);
    let status = batch.status().await.unwrap();
    assert_eq!((status["evaluated"].clone(), status["evaluation"].clone(), status["auto"].clone()), (json!(false), Value::Null, json!(false)));
    let attempts: i64 = s.connect().unwrap().query_row("SELECT count(*) FROM vessel_lookups", [], |r| r.get(0)).unwrap();
    assert_eq!(attempts, 1, "only the retained lookup");
    // The retained lookup stays readable and its decisions editable.
    assert_eq!(view["lookup"]["status"], "completed");
    s.decide_vessel(&lookup, 0, "kw-1", Some("confirmed")).unwrap();
    assert_eq!(s.vessel_lookup("mscio:alert:601", &revision).unwrap()["decisions"][0]["decision"], "confirmed");

    // A failed evaluation is not installed.
    let failing = tempfile::tempdir().unwrap();
    keep_gold(failing.path(), &["ar_najaf"]);
    keep(failing.path(), &case("ar_najaf"), &najaf());
    let failed = evaluation::replay(failing.path()).unwrap();
    assert_eq!(failed["passed"], false);
    assert!(s.record_vessel_evaluation(&failed).unwrap_err().message.contains("did not pass"));
    // A passed one is checked again on install.
    let passing = tempfile::tempdir().unwrap();
    keep_gold(passing.path(), &[]);
    let passed = evaluation::replay(passing.path()).unwrap();
    assert_eq!(passed["passed"], true, "{}", passed["failures"]);
    let mut edited = passed.clone();
    edited["summary"]["negative_vessels"] = json!(2);
    assert!(s.record_vessel_evaluation(&edited).unwrap_err().message.contains("does not meet"));
    let mut another = passed.clone();
    another["fingerprint"]["model"] = json!("openai/another-model");
    assert!(s.record_vessel_evaluation(&another).unwrap_err().message.contains("another model profile"));
    assert!(!s.vessel_evaluation().unwrap().0);

    // Installed: the gate opens. With no OpenRouter key configured, the next
    // refusal is the configuration, still before anything is reserved.
    s.record_vessel_evaluation(&passed).unwrap();
    let view = s.vessel_lookup("mscio:alert:601", &revision).unwrap();
    assert_eq!((view["evaluated"].clone(), view["evaluation"]["passed"].clone(), view["evaluation"]["cases"].clone()), (json!(true), json!(true), json!(40)));
    assert_eq!(view["evaluation"]["summary"]["recall"], 1.0);
    assert!(view["evaluation"]["checked_at"].is_string());
    assert_eq!(vessel_finder::extract(&s, "mscio:alert:602", &other).await.unwrap_err().message, "AI processing is not configured");
    let attempts: i64 = s.connect().unwrap().query_row("SELECT count(*) FROM vessel_lookups", [], |r| r.get(0)).unwrap();
    assert_eq!(attempts, 1);
    batch.set_auto(true).unwrap();
    assert_eq!(batch.status().await.unwrap()["evaluated"], true);
}
