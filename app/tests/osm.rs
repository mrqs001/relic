use relic_app::osm;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};
#[test]
fn pbf_retains_identity_aliases_support_nodes_holes_and_incomplete_geometry() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("candidate");
    let result = osm::build(
        &root.join("examples/osm-facilities.osm.pbf"),
        &out,
        json!({"id":"fictional","fictional":true}),
        &Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert_eq!(result["selected_features"], 6);
    assert_eq!(result["geometry_status"]["complete"], 4);
    assert_eq!(result["geometry_status"]["missing_node"], 1);
    assert_eq!(result["geometry_status"]["unsupported_member"], 1);
    let c = Connection::open(out.join("index.sqlite3")).unwrap();
    let raw: String = c
        .query_row(
            "SELECT geometry FROM features WHERE id='osm:relation:201'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let g: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(g["type"], "MultiPolygon");
    assert_eq!(g["coordinates"][0].as_array().unwrap().len(), 2);
    assert_eq!(g["coordinates"][0][0].as_array().unwrap().len(), 5);
    let v: (String, Option<i32>, Option<i64>) = c
        .query_row(
            "SELECT osm_id,version,timestamp FROM features WHERE id='osm:node:20'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(v.0, "20");
    assert_eq!(v.1, Some(1));
    assert!(v.2.is_some());
    let names: i64 = c
        .query_row(
            "SELECT count(*) FROM aliases WHERE feature='osm:node:20'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(names, 4);
    let line: String = c
        .query_row(
            "SELECT json_extract(geometry,'$.type') FROM features WHERE id='osm:way:110'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(line, "LineString");
    assert!(
        osm::build(
            &root.join("examples/osm-facilities.osm.pbf"),
            &out,
            json!({}),
            &Arc::new(AtomicBool::new(false))
        )
        .is_err()
    );
    let cancelled = tmp.path().join("cancelled");
    assert!(
        osm::build(
            &root.join("examples/osm-facilities.osm.pbf"),
            &cancelled,
            json!({}),
            &Arc::new(AtomicBool::new(true))
        )
        .is_err()
    );
    assert!(!cancelled.exists());
}

#[test]
fn truncated_snapshot_never_gets_an_activation_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let pbf = tmp.path().join("broken.osm.pbf");
    let bytes = include_bytes!("../../examples/osm-facilities.osm.pbf");
    std::fs::write(&pbf, &bytes[..bytes.len() - 20]).unwrap();
    let dest = tmp.path().join("candidate");
    assert!(
        osm::build(
            &pbf,
            &dest,
            json!({"fictional":true}),
            &Arc::new(AtomicBool::new(false))
        )
        .is_err()
    );
    assert!(!dest.join("manifest.json").exists());
}

#[test]
fn public_dense_nodes_keep_versions_without_contributor_metadata() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("candidate");
    osm::build(
        &root.join("examples/osm-facilities-public.osm.pbf"),
        &out,
        json!({"fictional":true}),
        &Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    let c = Connection::open(out.join("index.sqlite3")).unwrap();
    let (version, timestamp): (Option<i32>, Option<i64>) = c
        .query_row(
            "SELECT version,timestamp FROM features WHERE id='osm:node:20'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(version, Some(1));
    assert_eq!(timestamp, Some(1767225600000));
}

fn stored_candidate(
    root: &std::path::Path,
    dir: &std::path::Path,
    region: &str,
) -> std::path::PathBuf {
    let dest = dir.join(region);
    let mut m=osm::build(&root.join("examples/osm-facilities.osm.pbf"),&dest,json!({"feature":{"properties":{"id":region}},"pbf":{"resolved_url":"https://example.invalid/fictional.osm.pbf"}}),&Arc::new(AtomicBool::new(false))).unwrap();
    std::fs::copy(
        root.join("examples/osm-facilities.osm.pbf"),
        dest.join("source.osm.pbf"),
    )
    .unwrap();
    for name in ["index.json", "source.md5"] {
        let bytes = b"Fictional acquisition asset";
        std::fs::write(dest.join(name), bytes).unwrap();
        let field = if name == "index.json" {
            "index"
        } else {
            "checksum"
        };
        m["selection"][field] = json!({"sha256":relic_app::model::hash(bytes),"bytes":bytes.len()});
    }
    // Authored acquisition metadata exercises overlapping release provenance.
    m["header"]["replication_timestamp"] = json!(if region == "first" {
        1767225600_i64
    } else {
        1767312000_i64
    });
    std::fs::write(dest.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    dest
}
#[test]
fn activation_exact_lookup_overlap_failed_refresh_and_backup() {
    use relic_app::{
        config::Config,
        processing_geo::{GeoResolver, LocationProposal, LocationRole},
        store::Store,
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let tmp = tempfile::tempdir_in(root.join("data")).unwrap();
    let s = Store::open(Config::local(
        root.clone(),
        tmp.path().join("workspace"),
        "127.0.0.1:8793".parse().unwrap(),
    ))
    .unwrap();
    // A fictional accepted country surrounds the fictional facilities. Its
    // extract label alone deliberately supplies no country attribution.
    let c = s.connect().unwrap();
    let bytes = b"Fictional country reference";
    let fixture_hash = relic_app::model::hash(bytes);
    let path = s.config.object_path(&fixture_hash);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    c.execute(
        "INSERT INTO objects VALUES(?1,?2,'text/plain')",
        rusqlite::params![fixture_hash, bytes.len() as i64],
    )
    .unwrap();

    let shape = json!({"type":"Polygon","coordinates":[[[49.,29.],[51.,29.],[51.,31.],[49.,31.],[49.,29.]]]});
    c.execute("INSERT INTO boundary_shapes VALUES('fictional-country','fictional','IR','IRN','ADM0','fixture','Iran',?1,?1,?2,?3,?4)",rusqlite::params![shape.to_string(),"[49,29,51,31]",json!({"boundaryYearRepresented":"fictional","relic_source_sha256":"fixture","relic_record_sha256":"fixture"}).to_string(),fixture_hash]).unwrap();
    c.execute(
        "INSERT INTO metadata VALUES('active_boundaries',?1)",
        [json!({"edition":"fictional","editions":[]}).to_string()],
    )
    .unwrap();
    c.execute("INSERT INTO lineage_nodes VALUES('boundaries:fictional','reference_edition','Fictional country','{}')",[]).unwrap();
    drop(c);
    let a = stored_candidate(&root, tmp.path(), "first");
    let b = stored_candidate(&root, tmp.path(), "overlap");
    let paths = vec![
        b.to_str().unwrap().to_owned(),
        a.to_str().unwrap().to_owned(),
    ];
    let active = s.activate_osm(&paths, &AtomicBool::new(false)).unwrap();
    let r = GeoResolver::load_for_store(&s).unwrap();
    let proposal = |name: &str, country: Option<&str>| LocationProposal {
        mention: name.into(),
        quote: format!("Fire at {name}."),
        role: LocationRole::Event,
        canonical_name: None,
        country_code: country.map(str::to_owned),
    };
    let p = proposal("Fictional Cedar Airport", Some("IR"));
    let resolved = r
        .resolve(&p.quote, std::slice::from_ref(&p), true)
        .unwrap()
        .remove(0);
    assert_eq!(
        resolved.candidate_count, 1,
        "identical overlap is one identity"
    );
    assert!(resolved.geometry.is_some());
    assert_eq!(resolved.precision, "approximate_facility");
    assert!(
        r.resolve(&p.quote, std::slice::from_ref(&p), false)
            .unwrap()[0]
            .geometry
            .is_none()
    );
    // Without a supplied country, a name no gazetteer knows that matches one
    // OSM feature is placed; its country comes from the accepted boundary that
    // contains it (the extract label still attributes nothing).
    let free = proposal("Fictional Cedar Airport", None);
    let free = r.resolve(&free.quote, std::slice::from_ref(&free), true).unwrap().remove(0);
    assert!(free.geometry.is_some(), "{free:?}");
    assert!(free.reason.contains("accepted boundary containing it"));
    assert_eq!(
        free.candidates.iter().find(|c| Some(&c.id) == free.selected_candidate_id.as_ref()).unwrap().country_code.as_deref(),
        Some("IR")
    );
    for (name, country) in [
        ("Fictional Cedar Airport", Some("LB")),
        ("Old Cedar Field", Some("IR")),
        ("ZZZ", Some("IR")),
    ] {
        let p = proposal(name, country);
        assert!(
            r.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap()[0]
                .geometry
                .is_none()
        );
    }
    let index = s.osm_indexes().unwrap();
    assert_eq!(index[0].matches("فرودگاه خیالی سدر").unwrap().len(), 1);
    let line = proposal("Fictional Closed Rail Loop", Some("IR"));
    let line = r
        .resolve(&line.quote, std::slice::from_ref(&line), true)
        .unwrap()
        .remove(0);
    assert_eq!(line.precision, "facility_line");
    assert_eq!(line.status, "context_only");
    assert!(!line.occurrence_eligible);
    let name = "Fictional Lakeside Hospital";
    let p = proposal(name, Some("IR"));
    let result = r
        .resolve(&p.quote, std::slice::from_ref(&p), true)
        .unwrap()
        .remove(0);
    assert!(result.geometry.is_none());
    assert!(result.area.is_some(), "{result:?}");
    let area = result.area.unwrap();
    let shapeid = area["id"].as_str().unwrap().to_owned();
    let (saved_time,saved_manifest):(String,String)=s.connect().unwrap().query_row("SELECT json_extract(metadata,'$.boundaryYearRepresented'),manifest_hash FROM boundary_shapes WHERE id=?1",[&shapeid],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(saved_time, area["represented_time"].as_str().unwrap());
    assert_eq!(
        saved_manifest,
        result.candidates[0].reference.acceptance_sha256
    );
    assert_eq!(saved_time, "OSM snapshot 2026-01-01 00:00 UTC");

    assert!(
        s.connect()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM boundary_shapes WHERE id=?1)",
                [shapeid],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
    // Complete fictional publication: the hospital keeps its hole, the airport
    // is an approximate reference point, and originals/citations remain exact.
    let names = ["Fictional Cedar Airport", "Fictional Lakeside Hospital"];
    let rows=names.iter().enumerate().map(|(i,name)| {
        let text=format!("Fictional: fire reported at {name}.");
        let id=format!("fiction:osm:{i}");
        json!({"id":id,"revision":"v1","source":"fiction:osm","text":text,"published_at":"2026-09-23T01:00:00Z","acquired_at":"2026-09-23T01:01:00Z","geometry":null,"location_precision":"unresolved","language":"en","origin_group":null,"fictional":true,"availability":"available","citation":{"evidence_id":id,"revision":"v1","start_character":0,"end_character":text.chars().count()}})
    }).collect::<Vec<_>>();
    s.import_job(json!({"schema_version":1,"fictional":true,"sources":[{"id":"fiction:osm","label":"Fictional Facility Wire","family":"authored_report"}],"evidence_revisions":rows}),relic_app::model::ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
    let mut prepared = vec![];
    for (i, name) in names.iter().enumerate() {
        let e = s.detail(&format!("fiction:osm:{i}"), "v1").unwrap();
        let text = e["text"].as_str().unwrap();
        let gold = json!({"language":"en","translation":null,"headline":text,"kind":"event","claim_status":"reported","topic":"safety","claims":[{"text":text,"quote":text,"state":"reported","event_type":"fire","time_text":null,"location_indices":[0]}],"locations":[{"mention":name,"quote":text,"role":"event","canonical_name":name,"country_code":"IR"}]});
        let checked = relic_app::processing_ai::validate_output(text, &gold).unwrap();
        prepared.push(
            relic_app::processing_store::prepare(
                e["message_key"].as_str().unwrap().parse().unwrap(),
                &e,
                &checked,
                &r,
                "authored/gold",
            )
            .unwrap(),
        );
    }
    s.publish_processing(
        prepared,
        &relic_app::model::ident("processing"),
        &AtomicBool::new(false),
    )
    .unwrap();
    let query = |scope| {
        s.query(relic_app::model::Query {
            scope,
            generation: 1,
            cursor: None,
            limit: Some(100),
        })
        .unwrap()
    };
    let view = query(json!({}));
    assert_eq!(view["located"], 2);
    assert_eq!(view["map_areas"]["features"].as_array().unwrap().len(), 1);
    let area = &view["map_areas"]["features"][0];
    assert_eq!(area["properties"]["level"], "FACILITY");
    assert_eq!(
        area["geometry"]["coordinates"][0].as_array().unwrap().len(),
        2
    );
    let hole = query(json!({"bbox":[50.025,30.025,50.035,30.035],"include_unlocated":false}));
    assert_eq!(hole["total"], 0);
    assert_eq!(
        s.detail("fiction:osm:0", "v1").unwrap()["text"],
        rows[0]["text"]
    );
    assert_eq!(
        s.detail_with_publication("fiction:osm:0", "v1", None)
            .unwrap()["interpretation"]["location_precision"],
        "approximate_facility"
    );
    std::fs::write(a.join("index.sqlite3"), b"broken").unwrap();
    assert!(s.activate_osm(&paths, &AtomicBool::new(false)).is_err());
    assert_eq!(s.osm().unwrap().unwrap(), active);
    assert_eq!(
        s.activate_osm(&paths, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        "cancelled"
    );
    let backup = s.backup().unwrap();
    assert_eq!(
        s.verify_backup(backup["id"].as_str().unwrap()).unwrap()["verified"],
        true
    );
    let backup_dir = s
        .config
        .data
        .join("backups")
        .join(backup["id"].as_str().unwrap());
    let restored_dir = tmp.path().join("restored");
    std::fs::create_dir_all(restored_dir.join("objects")).unwrap();
    std::fs::copy(
        backup_dir.join("relic.sqlite3"),
        restored_dir.join("relic.sqlite3"),
    )
    .unwrap();
    std::fs::rename(
        backup_dir.join("objects"),
        restored_dir.join("objects/sha256"),
    )
    .unwrap();
    let restored = Store::open(Config::local(
        root.clone(),
        restored_dir,
        "127.0.0.1:8794".parse().unwrap(),
    ))
    .unwrap();
    assert_eq!(
        restored
            .query(relic_app::model::Query {
                scope: json!({}),
                generation: 1,
                cursor: None,
                limit: Some(100)
            })
            .unwrap()["located"],
        2
    );
    assert_eq!(
        restored.osm_indexes().unwrap()[0]
            .matches("Fictional Cedar Airport")
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_dir_all(a).unwrap();
    std::fs::remove_dir_all(b).unwrap();
    assert_eq!(
        s.osm_indexes().unwrap()[0]
            .matches("Fictional Cedar Airport")
            .unwrap()
            .len(),
        1
    );
    if std::env::var_os("RELIC_KEEP_OSM_FIXTURE").is_some() {
        println!(
            "Fictional OSM workspace: {}",
            tmp.keep().join("workspace").display()
        );
    }
}
