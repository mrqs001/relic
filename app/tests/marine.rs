//! Fictional polygons deliberately do not reproduce real marine boundaries.
use relic_app::{
    config::Config,
    model::{Query, Scope, hash, ident, string, value},
    processing_ai::validate_output,
    processing_geo::{GeoResolver, LocationProposal, LocationRole},
    processing_store::prepare,
    store::Store,
};
use serde_json::json;
use std::{
    fs,
    sync::{Arc, atomic::AtomicBool},
};

#[test]
fn selected_marine_areas_preserve_roles_holes_history_and_restore() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let tmp = tempfile::Builder::new()
        .prefix("fictional-marine-")
        .tempdir_in(root.join("data"))
        .unwrap();
    let s = Store::open(Config::local(
        root.clone(),
        tmp.path().join("workspace"),
        "127.0.0.1:8794".parse().unwrap(),
    ))
    .unwrap();
    let dir = tmp.path().join("candidate");
    fs::create_dir(&dir).unwrap();
    let catalog = relic_app::marine::selections();
    let mut assets = json!({});
    let mut requests = vec![];
    for selection in catalog["areas"].as_array().unwrap() {
        let id = selection["mrgid"].as_str().unwrap();
        let raw = json!({"type":"FeatureCollection","numberMatched":1,"numberReturned":1,"totalFeatures":1,"crs":{"properties":{"name":"urn:ogc:def:crs:EPSG::4326"}},"features":[{"type":"Feature","properties":{"mrgid_sr":id,"sub_region":selection["publisher_name"],"skos_url":selection["vocabulary_uri"],"minlong":50,"minlat":10,"maxlong":54,"maxlat":14},"geometry":{"type":"Polygon","coordinates":[[[50,10],[54,10],[54,14],[50,14],[50,10]],[[51,11],[51,12],[52,12],[52,11],[51,11]]]}}]});
        let bytes = serde_json::to_vec(&raw).unwrap();
        let filename = format!("{id}.geojson");
        fs::write(dir.join(&filename), &bytes).unwrap();
        assets[&filename] = json!({"sha256":hash(&bytes),"bytes":bytes.len()});
        requests.push(json!({"url":relic_app::marine::request_url(id).unwrap(),"acquired_at":"2026-09-23T00:00:00Z"}));
    }
    let manifest = json!({"schema_version":1,"status":"candidate","publisher":"Marine Regions / VLIZ","product":"SeaVoX v19","license":"CC-BY-4.0","selections_sha256":hash(string(&catalog).as_bytes()),"selection_review":catalog,"assets":assets,"requests":requests,"fictional":true});
    fs::write(dir.join("manifest.json"), string(&manifest)).unwrap();
    assert!(s.activate_marine(&dir, &AtomicBool::new(true)).is_err());
    assert!(s.marine().unwrap().is_none());
    let active = s.activate_marine(&dir, &AtomicBool::new(false)).unwrap();
    assert_eq!(active["features"], catalog["areas"].as_array().unwrap().len());
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    for name in [
        "Strait of Hormuz",
        "مضيق هرمز",
        "تنگه هرمز",
        "Arabian Sea",
        "بحر العرب",
        "دریای عرب",
    ] {
        let text = format!("Fictional: fire reported in {name}.");
        let proposal = LocationProposal {
            mention: name.into(),
            quote: text.clone(),
            role: LocationRole::Event,
            canonical_name: None,
            country_code: None,
        };
        let r = resolver
            .resolve(&text, std::slice::from_ref(&proposal), true)
            .unwrap()
            .remove(0);
        assert_eq!(r.status, "resolved_area");
        assert!(r.geometry.is_none());
        assert!(r.area.is_some());
        assert!(r.candidates[0].country_code.is_none());
        for role in [
            LocationRole::Mentioned,
            LocationRole::Issuer,
            LocationRole::Destination,
            LocationRole::Origin,
        ] {
            let p = LocationProposal {
                role,
                ..proposal.clone()
            };
            let r = resolver.resolve(&text, &[p], true).unwrap().remove(0);
            assert!(r.area.is_none());
            assert!(!r.occurrence_eligible);
        }
        assert!(
            resolver.resolve(&text, &[proposal], false).unwrap()[0]
                .area
                .is_none()
        );
    }
    for (mention, canonical) in [
        ("the sea", Some("Arabian Sea")),
        ("Gulf of Oman", Some("Arabian Sea")),
        ("Strait of Hormuz", Some("Arabian Sea")),
    ] {
        let text = format!("Fictional: fire reported in {mention}.");
        let p = LocationProposal {
            mention: mention.into(),
            quote: text.clone(),
            role: LocationRole::Event,
            canonical_name: canonical.map(str::to_owned),
            country_code: None,
        };
        assert!(
            resolver.resolve(&text, &[p], true).unwrap()[0]
                .area
                .is_none()
        );
    }
    let text = "Fictional: fire reported in Strait of Hormuz.";
    let evidence = json!({"id":"fiction:marine","revision":"v1","source":"fiction:marine","text":text,"published_at":"2026-09-23T00:00:00Z","acquired_at":"2026-09-23T00:01:00Z","geometry":null,"location_precision":"unresolved","language":"en","origin_group":null,"fictional":true,"availability":"available","citation":{"evidence_id":"fiction:marine","revision":"v1","start_character":0,"end_character":text.chars().count()}});
    let imported=s.import_job(json!({"schema_version":1,"fictional":true,"sources":[{"id":"fiction:marine","label":"Fictional Marine Wire","family":"authored_report"}],"evidence_revisions":[evidence]}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
    let gold = json!({"language":"en","translation":null,"headline":text,"kind":"event","claim_status":"reported","topic":"safety","claims":[{"text":text,"quote":text,"state":"reported","event_type":"fire","time_text":null,"location_indices":[0]}],"locations":[{"mention":"Strait of Hormuz","quote":text,"role":"event","canonical_name":"Strait of Hormuz","country_code":null}]});
    let e = s.detail("fiction:marine", "v1").unwrap();
    let checked = validate_output(text, &gold).unwrap();
    let prepared = prepare(
        e["message_key"].as_str().unwrap().parse().unwrap(),
        &e,
        &checked,
        &resolver,
        "authored/gold",
    )
    .unwrap();
    // Activation can happen while a reading is prepared. Publication still
    // cites its retained old context rather than silently switching editions.
    let old_edition = prepared.interpretation["reference_edition"].clone();
    let mut replacement = manifest.clone();
    replacement["requests"][0]["acquired_at"] = json!("2026-09-23T01:00:00Z");
    fs::write(dir.join("manifest.json"), string(&replacement)).unwrap();
    let active = s.activate_marine(&dir, &AtomicBool::new(false)).unwrap();
    assert_ne!(
        old_edition.as_str(),
        GeoResolver::load_for_store(&s).unwrap().edition()
    );
    s.publish_processing(
        vec![prepared],
        &ident("processing"),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        s.detail_with_publication("fiction:marine", "v1", None)
            .unwrap()["interpretation"]["reference_edition"],
        old_edition
    );
    let query = |store: &Store, scope: Scope| {
        store
            .query(Query {
                scope: value(scope),
                generation: 1,
                cursor: None,
                limit: Some(100),
            })
            .unwrap()
    };
    let all = query(&s, Scope::default());
    assert_eq!(all["located"], 1);
    assert_eq!(all["map_loaded"], 0);
    assert_eq!(
        all["map_areas"]["features"][0]["properties"]["level"],
        "MARINE"
    );
    assert_eq!(
        query(
            &s,
            Scope {
                include_unlocated: false,
                bbox: Some([51.1, 11.1, 51.9, 11.9]),
                ..Scope::default()
            }
        )["total"],
        0,
        "analytical holes remain empty"
    );
    assert_eq!(
        query(
            &s,
            Scope {
                publication_id: Some(imported["publication_id"].as_str().unwrap().into()),
                ..Scope::default()
            }
        )["located"],
        0
    );
    let backup = s.backup().unwrap();
    let verified = s.verify_backup(backup["id"].as_str().unwrap()).unwrap();
    let mut config = s.config.clone();
    config.data = verified["restored_directory"].as_str().unwrap().into();
    let restored = Store::open(config).unwrap();
    assert_eq!(
        query(&restored, Scope::default())["map_areas"],
        all["map_areas"]
    );
    let fresh_config = Config::local(
        root,
        tmp.path().join("fresh"),
        "127.0.0.1:8795".parse().unwrap(),
    );
    relic_app::fresh_workspace::prepare(fresh_config.clone(), &s.config.data).unwrap();
    let fresh = Store::open(fresh_config).unwrap();
    assert_eq!(fresh.marine().unwrap(), s.marine().unwrap());
    assert_eq!(fresh.area_records().unwrap().len(), catalog["areas"].as_array().unwrap().len());
    fs::write(dir.join("24076.geojson"), b"corrupt").unwrap();
    assert!(s.activate_marine(&dir, &AtomicBool::new(false)).is_err());
    assert_eq!(s.marine().unwrap().unwrap()["edition"], active["edition"]);
}
