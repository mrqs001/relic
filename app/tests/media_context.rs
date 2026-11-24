use relic_app::{config::Config, store::Store};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};
fn workspace() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let s = Store::open(Config::local(
        root,
        tmp.path().join("data"),
        "127.0.0.1:8795".parse().unwrap(),
    ))
    .unwrap();
    (tmp, s)
}
fn row(id: u64, second: i64, text: &str, media: bool) -> Value {
    let text = if text.is_empty() { "📷" } else { text };
    let date = chrono::DateTime::parse_from_rfc3339("2026-09-20T12:00:00Z").unwrap()
        + chrono::Duration::seconds(second);
    let identity = format!("telegram:fixture:peer:{id}");
    json!({"id":identity,"revision":"v1","source":"fixture:channel","published_at":date.to_rfc3339_opts(chrono::SecondsFormat::Secs,true),"acquired_at":"2026-09-20T13:00:00Z","text":text,"geometry":null,"location_precision":"unresolved","origin_group":null,"dependence":"unknown","language":"en","fictional":true,"availability":"available","upstream_state":"present","relationships":{"account_id":"fixture","peer_id":"peer","message_id":id.to_string(),"sender_id":"one"},"media":if media{json!([{"kind":"photo"}])}else{json!([])},"citation":{"evidence_id":identity,"revision":"v1","start_character":0,"end_character":text.chars().count()}})
}
fn import(s: &Store, rows: Vec<Value>, label: &str) -> String {
    let v=s.import_job(json!({"schema_version":1,"fictional":true,"title":label,"sources":[{"id":"fixture:channel","label":"Fictional channel","family":"telegram"},{"id":"fixture:other","label":"Other channel","family":"telegram"}],"evidence_revisions":rows}),label.into(),Arc::new(AtomicBool::new(false))).unwrap();
    v["publication_id"].as_str().unwrap().to_owned()
}
fn detail(s: &Store, id: u64, pubid: Option<&str>) -> Value {
    s.detail_with_publication(&format!("telegram:fixture:peer:{id}"), "v1", pubid)
        .unwrap()
}
fn members(d: &Value) -> Vec<&str> {
    d["media_context"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect()
}
#[test]
fn late_media_joins_exact_publication_without_changing_original_or_chaining_window() {
    let (_tmp, s) = workspace();
    let first = import(
        &s,
        vec![row(
            1,
            0,
            "Fictional crews report flooding in the harbour.",
            true,
        )],
        "initial",
    );
    import(
        &s,
        vec![
            row(4, 181, "", true),
            row(3, 180, "Additional footage", true),
            row(2, 120, "", true),
        ],
        "late-out-of-order",
    );
    let current = detail(&s, 1, None);
    assert_eq!(
        members(&current),
        vec!["telegram:fixture:peer:2", "telegram:fixture:peer:3"]
    );
    assert!(members(&detail(&s, 1, Some(&first))).is_empty());
    assert_eq!(current["media_context"]["members"][0]["reason"], "nearby");
    assert_eq!(
        detail(&s, 2, None)["media_context"]["anchor"]["id"],
        "telegram:fixture:peer:1"
    );
    assert!(detail(&s, 4, None)["media_context"]["anchor"].is_null());
    assert_eq!(current["media"].as_array().unwrap().len(), 1);
    assert_eq!(
        current["text"],
        "Fictional crews report flooding in the harbour."
    );
    assert!(
        detail(&s, 2, None)["map_locations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let frozen = s
        .connect()
        .unwrap()
        .query_row(
            "SELECT id FROM publications WHERE status='complete' ORDER BY rowid DESC LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    let mut edit = row(
        2,
        120,
        "This is a separate unrelated report, with a long caption. "
            .repeat(5)
            .as_str(),
        true,
    );
    edit["revision"] = json!("v2");
    edit["citation"]["revision"] = json!("v2");
    import(&s, vec![edit], "edit");
    assert!(!members(&detail(&s, 1, None)).contains(&"telegram:fixture:peer:2"));
    assert_eq!(
        detail(&s, 1, Some(&frozen))["media_context"]["members"][0]["revision"],
        "v1"
    );
    assert!(members(&detail(&s, 2, None)).is_empty());
}
#[test]
fn boundaries_reply_album_and_author_take_precedence_over_nearby_heuristic() {
    let (_tmp, s) = workspace();
    let mut other = row(4, 80, "", true);
    other["source"] = json!("fixture:other");
    let mut author = row(5, 90, "", true);
    author["relationships"]["sender_id"] = json!("two");
    let mut reply = row(6, 100, "", true);
    reply["relationships"]["reply"] = json!({"peer_id":"peer","message_id":"999"});
    let mut forward = row(7, 110, "", true);
    forward["relationships"]["forward"] = json!({"peer_id":"foreign"});
    let mut explicit = row(9, 900, "", true);
    explicit["relationships"]["reply"] = json!({"peer_id":"peer","message_id":"1"});
    let mut album = row(10, 1000, "A fictional captioned album.", true);
    album["relationships"]["grouped_id"] = json!("album");
    let mut sibling = row(11, 1900, "", true);
    sibling["relationships"]["grouped_id"] = json!("album");
    import(
        &s,
        vec![
            row(1, 0, "First fictional event report.", false),
            row(2, 60, "A different fictional event.", false),
            row(3, 70, "", true),
            other,
            author,
            reply,
            forward,
            row(8, 120, "long ".repeat(40).as_str(), true),
            explicit,
            album,
            sibling,
        ],
        "boundaries",
    );
    assert_eq!(
        members(&detail(&s, 1, None)),
        vec!["telegram:fixture:peer:9"]
    );
    assert_eq!(
        members(&detail(&s, 2, None)),
        vec!["telegram:fixture:peer:3"]
    );
    assert_eq!(
        detail(&s, 9, None)["media_context"]["anchor"]["id"],
        "telegram:fixture:peer:1"
    );
    assert_eq!(
        members(&detail(&s, 10, None)),
        vec!["telegram:fixture:peer:11"]
    );
    assert_eq!(
        detail(&s, 11, None)["media_context"]["anchor"]["id"],
        "telegram:fixture:peer:10"
    );
}
#[test]
fn x_reply_and_numeric_post_order_are_preserved() {
    let (_tmp, s) = workspace();
    let mut rows = vec![
        row(9, 0, "Fictional report from the harbour.", false),
        row(10, 0, "", true),
        row(11, 30, "", true),
    ];
    for (i, r) in rows.iter_mut().enumerate() {
        let id = format!("x:post:{}", i + 9);
        r["id"] = json!(id);
        r["citation"]["evidence_id"] = json!(id);
        r["relationships"] = json!({"author":{"id":"author"},"provider":"x"});
    }
    rows[2]["relationships"]["reply_to"] = json!("other");
    import(&s, rows, "x-fixture");
    let d = s.detail_with_publication("x:post:9", "v1", None).unwrap();
    assert_eq!(members(&d), vec!["x:post:10"]);
}

#[test]
fn busy_channels_disclose_bounds_without_returning_a_misleading_partial_group() {
    for (followers, expected) in [(80, 50), (405, 0)] {
        let (_tmp, s) = workspace();
        let mut rows = vec![row(1, 0, "Fictional report from the harbour.", false)];
        rows.extend((0..followers).map(|i| row(i + 2, 60, "📷", true)));
        import(&s, rows, "busy-channel");
        let d = detail(&s, 1, None);
        assert_eq!(members(&d).len(), expected);
        assert_eq!(d["media_context"]["limited"], true);
    }
}
#[test]
fn short_self_contained_reports_are_not_absorbed_as_follow_up_media() {
    let (_tmp, s) = workspace();
    import(
        &s,
        vec![
            row(1, 0, "Fictional officials describe a new harbour policy.", false),
            row(2, 7, "Fictional strikes reported near the northern ridge, per local reports.", true),
            row(3, 20, "", true),
            row(4, 30, "Additional footage", true),
        ],
        "short reports",
    );
    // The seven-second report stands alone; only captionless or tiny follow-ups join.
    assert!(detail(&s, 2, None)["media_context"]["anchor"].is_null());
    assert_eq!(
        members(&detail(&s, 2, None)),
        vec!["telegram:fixture:peer:3", "telegram:fixture:peer:4"]
    );
    assert!(members(&detail(&s, 1, None)).is_empty());
}
#[test]
fn explicit_short_reply_keeps_its_link_when_inference_would_not() {
    let (_tmp, s) = workspace();
    let mut reply = row(2, 20, "Officials confirm the fictional closure, per local reports.", true);
    reply["relationships"]["reply"] = json!({"peer_id":"peer","message_id":"1"});
    import(&s, vec![row(1, 0, "Fictional harbour closure reported this morning.", false), reply], "reply");
    assert_eq!(detail(&s, 2, None)["media_context"]["anchor"]["reason"], "reply");
}
