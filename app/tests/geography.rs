use relic_app::{
    config::Config,
    geonames,
    model::{ident, value},
    processing_ai::{self, validate_output},
    processing_geo::{GeoResolver, LocationProposal, LocationRole},
    processing_store::{self, Policy},
    store::Store,
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};
/// Fictional GeoNames rows shared by most fixtures. Bab Fiction lists Yemen
/// as an alternate country (cc2), as straits between countries often do.
const ROWS: &str = concat!(
    "900001\tFictional Cove\tFictional Cove\t\t14\t43\tP\tPPL\tYE\t\t01\t02\t\t\t1000\t\t10\tAsia/Aden\t2026-09-12\n",
    "900002\tAmbiguous Bay\tAmbiguous Bay\t\t15\t44\tA\tADM1\tYE\t\t01\t\t\t\t0\t\t10\tAsia/Aden\t2026-09-12\n",
    "900003\tAmbiguous Bay\tAmbiguous Bay\t\t16\t45\tP\tPPL\tYE\t\t01\t02\t\t\t1000\t\t10\tAsia/Aden\t2026-09-12\n",
    "900004\tDistant Cove\tDistant Cove\t\t24\t46\tP\tPPL\tSA\t\t01\t02\t\t\t1000\t\t10\tAsia/Riyadh\t2026-09-12\n",
    "900005\tFictional Peak\tFictional Peak\t\t14\t43\tT\tMT\tYE\t\t01\t\t\t\t0\t\t10\tAsia/Aden\t2026-09-12\n",
    "900006\tUnclassified Cove\tUnclassified Cove\t\t14\t43\tP\tPPL\tYE\t\t\t\t\t\t1000\t\t10\tAsia/Aden\t2026-09-12\n",
    "900007\tOther Region\tOther Region\t\t14\t43\tA\tADM1\tYE\t\t02\t\t\t\t0\t\t10\tAsia/Aden\t2026-09-12\n",
    "900008\tOther Cove\tOther Cove\t\t14\t43\tP\tPPL\tYE\t\t02\t\t\t\t1000\t\t10\tAsia/Aden\t2026-09-12\n",
    "900009\tFictional Strait\tFictional Strait\t\t12.6\t43.3\tH\tSTRT\tYE\t\t00\t\t\t\t0\t\t0\tAsia/Aden\t2026-10-01\n",
    "900010\tFictional Shoal\tFictional Shoal\t\t15.15\t42.76\tT\tATOL\tYE\t\t00\t\t\t\t0\t\t0\tAsia/Aden\t2026-10-01\n",
    "900011\tFictional Bank\tFictional Bank\t\t13.1\t42.9\tU\tBNKU\tYE\t\t00\t\t\t\t0\t\t0\tAsia/Aden\t2026-10-01\n",
    "900012\tBab Fiction\tBab Fiction\t\t12.5\t43.4\tH\tSTRT\tDJ\tYE\t00\t\t\t\t0\t\t0\tAfrica/Djibouti\t2026-10-01\n");
fn candidate(dir: &Path, product: &str) -> PathBuf {
    edition(dir, product, ROWS)
}
fn edition(dir: &Path, product: &str, rows: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let places = dir.join("input.txt");
    let aliases = dir.join("aliases.txt");
    fs::write(&places, rows).unwrap();
    fs::write(&aliases, "1\t900001\tar\tخليج خيالي\t1\t\t\t\t\t\n2\t900002\tar\tخيالية\t1\t\t\t\t\t\n3\t900002\tfa\tساختگی\t1\t\t\t\t\t\n").unwrap();
    let dest = dir.join("edition");
    let mut requests = json!([
        {"url":format!("https://download.geonames.org/export/dump/{product}.zip"),"acquired_at":"2026-09-12T00:00:00Z"},
        {"url":"https://download.geonames.org/export/dump/readme.txt","acquired_at":"2026-09-12T00:00:00Z"}
    ]);
    if product != "cities500" {
        requests.as_array_mut().unwrap().push(json!({"url":format!("https://download.geonames.org/export/dump/alternatenames/{product}.zip"),"acquired_at":"2026-09-12T00:00:00Z"}));
    }
    let mut m = geonames::build(
        &dest,
        &places,
        &aliases,
        json!({"product":product,"requests":requests,"fictional":true}),
        &AtomicBool::new(false),
    )
    .unwrap();
    let mut assets = vec!["places.zip", "readme.txt"];
    if product != "cities500" {
        assets.push("aliases.zip");
    }
    for name in assets {
        let data = b"Fictional retained publisher input; no request made";
        fs::write(dest.join(name), data).unwrap();
        m["assets"][name] = json!({"sha256":relic_app::model::hash(data),"bytes":data.len()});
    }
    fs::write(dest.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    dest
}
fn workspace() -> (tempfile::TempDir, Store) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let tmp = tempfile::Builder::new()
        .prefix("geography-fixture-")
        .tempdir_in(root.join("data"))
        .unwrap();
    let s = Store::open(Config::local(
        root,
        tmp.path().join("workspace"),
        "127.0.0.1:8793".parse().unwrap(),
    ))
    .unwrap();
    (tmp, s)
}
fn proposal(name: &str, role: LocationRole) -> LocationProposal {
    LocationProposal {
        mention: name.into(),
        quote: format!("Fictional fire in {name}."),
        role,
        canonical_name: None,
        country_code: None,
    }
}
#[test]
fn twelve_countries_and_global_baseline_survive_reload_and_rejected_expansion() {
    let (tmp, s) = workspace();
    let products = [
        "IR",
        "SA",
        "YE",
        "IQ",
        "IL",
        "PS",
        "KW",
        "JO",
        "LB",
        "SY",
        "RU",
        "UA",
        "cities500",
    ];
    let paths = products
        .iter()
        .map(|product| {
            candidate(&tmp.path().join(product), product)
                .to_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    let active = s
        .activate_geography(&paths, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(active["editions"].as_array().unwrap().len(), 13);
    assert_eq!(s.geography_indexes().unwrap().len(), 13);
    assert_eq!(
        s.geography_indexes_cancellable(&AtomicBool::new(true))
            .err()
            .unwrap()
            .code,
        "cancelled"
    );
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    let p = proposal("Fictional Cove", LocationRole::Event);
    let r = resolver
        .resolve(&p.quote, std::slice::from_ref(&p), true)
        .unwrap();
    assert_eq!(r[0].candidate_count, 1);
    assert!(r[0].geometry.is_some());
    // A rejected selection (the same edition twice) keeps the active one.
    assert!(
        s.activate_geography(&vec![paths[0].clone(); 2], &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(
        s.geography().unwrap().unwrap()["edition"],
        active["edition"]
    );
    assert_eq!(s.geography_indexes().unwrap().len(), 13);
}
#[test]
fn offline_reindex_keeps_raw_assets_and_rejects_changed_or_cancelled_inputs() {
    let (tmp, s) = workspace();
    let original = candidate(&tmp.path().join("original"), "cities500");
    let manifest = fs::read(original.join("manifest.json")).unwrap();
    let next = tmp.path().join("rebuilt");
    let rebuilt = geonames::rebuild(&original, &next, &AtomicBool::new(false)).unwrap();
    let prior: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    for file in [
        "places.txt",
        "alternateNamesV2.txt",
        "places.zip",
        "readme.txt",
    ] {
        assert_eq!(rebuilt["assets"][file], prior["assets"][file]);
        assert_eq!(
            fs::read(next.join(file)).unwrap(),
            fs::read(original.join(file)).unwrap()
        );
    }
    s.activate_geography(&[next.to_str().unwrap().into()], &AtomicBool::new(false))
        .unwrap();
    let before = s.geography().unwrap();
    let mut changed = rebuilt.clone();
    changed["possible_name_count"] = json!(999999);
    fs::write(
        next.join("manifest.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    assert!(
        s.activate_geography(&[next.to_str().unwrap().into()], &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(s.geography().unwrap(), before);
    assert!(
        geonames::rebuild(
            &original,
            &tmp.path().join("cancelled"),
            &AtomicBool::new(true)
        )
        .is_err()
    );
    assert!(!tmp.path().join("cancelled").exists());
    fs::write(original.join("readme.txt"), b"changed publisher bytes").unwrap();
    assert!(
        geonames::rebuild(
            &original,
            &tmp.path().join("changed"),
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(!tmp.path().join("changed").exists());
    assert_eq!(fs::read(original.join("manifest.json")).unwrap(), manifest);
}

#[test]
fn reviewed_marine_features_get_an_approximate_point_and_other_features_do_not() {
    let (tmp, s) = workspace();
    let dir = candidate(&tmp.path().join("marine"), "YE");
    s.activate_geography(&[dir.to_str().unwrap().into()], &AtomicBool::new(false))
        .unwrap();
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    let place = |name: &str, role: LocationRole, occurrence: bool| {
        let text = format!("Fictional tanker struck near {name}, Yemen.");
        let p = LocationProposal { mention: name.into(), quote: text.clone(), role, canonical_name: None, country_code: None };
        resolver.resolve(&text, &[p], occurrence).unwrap().remove(0)
    };
    for (name, at) in [("Fictional Strait", [43.3, 12.6]), ("Fictional Shoal", [42.76, 15.15])] {
        let r = place(name, LocationRole::Event, true);
        assert_eq!(r.status, "resolved", "{name}: {}", r.reason);
        assert_eq!(r.precision, relic_app::processing_geo::MARINE_FEATURE_PRECISION);
        assert_eq!(r.geometry.unwrap()["coordinates"], json!(at));
        assert!(r.occurrence_eligible);
        // Not an event place, or not a reported occurrence: identity only.
        assert_eq!(place(name, LocationRole::Mentioned, true).geometry, None);
        assert_eq!(place(name, LocationRole::Event, false).geometry, None);
    }
    // A strait filed under a neighbouring country, named with or without the word.
    for name in ["Bab Fiction", "Bab Fiction Strait", "the Bab Fiction strait", "Strait of Bab Fiction"] {
        let r = place(name, LocationRole::Event, true);
        assert_eq!(r.status, "resolved", "{name}: {}", r.reason);
        assert_eq!(r.geometry.unwrap()["coordinates"], json!([43.4, 12.5]), "{name}");
    }
    // "Strait of" never turns a name into a non-marine place of that name.
    let r = place("Strait of Fictional Peak", LocationRole::Event, true);
    assert_eq!(r.geometry, None, "{}", r.reason);
    // Undersea banks and mountains are not reviewed marine features.
    for name in ["Fictional Bank", "Fictional Peak"] {
        let r = place(name, LocationRole::Event, true);
        assert_eq!(r.geometry, None, "{name}: {}", r.reason);
        assert_ne!(r.status, "resolved");
    }
}

/// The fictional Lebanon outline from the shared area fixture (34..37 E,
/// 32..35 N), activated as boundaries.
fn lebanon_boundaries(tmp: &Path, s: &Store) {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../examples/processing-areas.json")).unwrap();
    let mut paths = vec![];
    for e in fixture["editions"].as_array().unwrap() {
        let level = e["metadata"]["boundaryType"].as_str().unwrap();
        let path = tmp.join(level);
        fs::create_dir(&path).unwrap();
        let mut assets = json!({});
        for (key, file) in [
            ("metadata", "metadata.json"),
            ("full", "full.geojson"),
            ("display", "display.geojson"),
        ] {
            let bytes = serde_json::to_vec(&e[key]).unwrap();
            fs::write(path.join(file), &bytes).unwrap();
            assets[file] = json!({"sha256":relic_app::model::hash(&bytes),"bytes":bytes.len()});
        }
        fs::write(path.join("manifest.json"),serde_json::to_vec(&json!({"schema_version":1,"status":"candidate","publisher":"geoBoundaries","collection":"gbOpen","country":"LBN","level":level,"boundary_id":e["metadata"]["boundaryID"],"features":1,"assets":assets,"fictional":true})).unwrap()).unwrap();
        paths.push(path.to_str().unwrap().into());
    }
    s.activate_boundaries(&paths, &AtomicBool::new(false))
        .unwrap();
}
/// Every reviewed marine selection as one fictional sea, 50..54 E and 10..14 N
/// with a hole at 51..52 E, 11..12 N. Not a real Marine Regions boundary.
fn fictional_seas(tmp: &Path, s: &Store) {
    let dir = tmp.join("seas");
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
        assets[&filename] = json!({"sha256":relic_app::model::hash(&bytes),"bytes":bytes.len()});
        requests.push(json!({"url":relic_app::marine::request_url(id).unwrap(),"acquired_at":"2026-09-23T00:00:00Z"}));
    }
    let manifest = json!({"schema_version":1,"status":"candidate","publisher":"Marine Regions / VLIZ","product":"SeaVoX v19","license":"CC-BY-4.0","selections_sha256":relic_app::model::hash(relic_app::model::string(&catalog).as_bytes()),"selection_review":catalog,"assets":assets,"requests":requests,"fictional":true});
    fs::write(dir.join("manifest.json"), relic_app::model::string(&manifest)).unwrap();
    s.activate_marine(&dir, &AtomicBool::new(false)).unwrap();
}
fn in_quote(resolver: &GeoResolver, text: &str, names: &[&str]) -> Vec<relic_app::processing_geo::LocationResolution> {
    let proposals = names
        .iter()
        .map(|name| LocationProposal { mention: (*name).into(), quote: text.into(), role: LocationRole::Event, canonical_name: None, country_code: None })
        .collect::<Vec<_>>();
    resolver.resolve(text, &proposals, true).unwrap()
}

#[test]
fn a_sea_feature_filed_under_a_neighbour_is_no_country_evidence() {
    let (tmp, s) = workspace();
    // Quiet Harbour is only in the national edition, so alone it needs a country.
    let national = edition(&tmp.path().join("YE"), "YE", &format!("{ROWS}900013\tQuiet Harbour\tQuiet Harbour\t\t13.3\t43.25\tP\tPPL\tYE\t\t01\t\t\t\t0\t\t0\tAsia/Aden\t2026-10-01\n"));
    let global = candidate(&tmp.path().join("global"), "cities500");
    s.activate_geography(&[national.to_str().unwrap().into(), global.to_str().unwrap().into()], &AtomicBool::new(false))
        .unwrap();
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    // Bab Fiction is filed under DJ; the town beside it is in YE.
    let text = "Fictional tanker struck in Bab Fiction near Fictional Cove and Quiet Harbour.";
    let r = in_quote(&resolver, text, &["Bab Fiction", "Fictional Cove"]);
    for place in &r {
        assert_eq!(place.status, "resolved", "{}: {}", place.mention, place.reason);
        assert!(place.geometry.is_some(), "{}", place.mention);
    }
    assert_eq!(r[0].precision, relic_app::processing_geo::MARINE_FEATURE_PRECISION);
    assert_eq!(relic_app::processing_geo::message_country(&r).as_deref(), Some("YE"));
    // So the message's one country still places a name that needs it.
    let proposals = ["Bab Fiction", "Fictional Cove", "Quiet Harbour"]
        .map(|name| LocationProposal { mention: name.into(), quote: text.into(), role: LocationRole::Event, canonical_name: None, country_code: None });
    let (r, country) = resolver.resolve_message(text, &proposals, true).unwrap();
    assert_eq!(country.as_deref(), Some("YE"));
    assert_eq!(r[2].selected_candidate_id.as_deref(), Some("geonames:900013"), "{} {}", r[2].status, r[2].reason);
    assert!(r[2].geometry.is_some());
    assert_eq!(r[2].literal_qualifier, Some(json!({"message_country":"YE"})));
    assert!(r[0].geometry.is_some() && r[1].geometry.is_some());
}

#[test]
fn a_written_country_keeps_a_sea_feature_only_when_it_borders_that_country() {
    let (tmp, s) = workspace();
    let rows = format!("{ROWS}900014\tFictional Reef\tFictional Reef\t\t34.0\t33.5\tH\tRF\tCY\t\t00\t\t\t\t0\t\t0\tAsia/Nicosia\t2026-10-01\n");
    let dir = edition(&tmp.path().join("YE"), "YE", &rows);
    s.activate_geography(&[dir.to_str().unwrap().into()], &AtomicBool::new(false))
        .unwrap();
    let near = |resolver: &GeoResolver, name: &str, country: &str, supplied: bool| {
        let text = if supplied { format!("Fictional tanker struck near {name}.") } else { format!("Fictional tanker struck near {name}, {country}.") };
        let code = if country == "Lebanon" { "LB" } else { "YE" };
        let p = LocationProposal { mention: name.into(), quote: text.clone(), role: LocationRole::Event, canonical_name: None, country_code: supplied.then(|| code.into()) };
        resolver.resolve(&text, &[p], true).unwrap().remove(0)
    };
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    // A Yemeni strait or a Cypriot reef is not "near X, Lebanon".
    for supplied in [false, true] {
        for name in ["Fictional Strait", "Fictional Reef", "Bab Fiction"] {
            let r = near(&resolver, name, "Lebanon", supplied);
            assert_eq!(r.status, "country_conflict", "{name}: {}", r.reason);
            assert_eq!(r.geometry, None, "{name}");
            assert!(!r.occurrence_eligible);
        }
        // The publisher lists Yemen as Bab Fiction's other country.
        let r = near(&resolver, "Bab Fiction", "Yemen", supplied);
        assert_eq!(r.status, "resolved", "{}", r.reason);
        assert!(r.reason.contains("DJ"), "{}", r.reason);
    }
    // With Lebanon's outline loaded, a reef within about 100 km of it counts.
    lebanon_boundaries(tmp.path(), &s);
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    for supplied in [false, true] {
        let r = near(&resolver, "Fictional Reef", "Lebanon", supplied);
        assert_eq!(r.status, "resolved", "{}", r.reason);
        assert_eq!(r.geometry.unwrap()["coordinates"], json!([33.5, 34.0]));
        let r = near(&resolver, "Fictional Strait", "Lebanon", supplied);
        assert_eq!(r.geometry, None, "{}", r.reason);
        assert_ne!(r.status, "resolved");
    }
}

#[test]
fn a_sea_feature_named_without_a_country_needs_a_unique_name() {
    let (tmp, s) = workspace();
    let national = candidate(&tmp.path().join("YE"), "YE");
    s.activate_geography(&[national.to_str().unwrap().into()], &AtomicBool::new(false))
        .unwrap();
    let text = "Fictional tanker struck in Fictional Strait.";
    // Without the global baseline, uniqueness cannot be established.
    let r = in_quote(&GeoResolver::load_for_store(&s).unwrap(), text, &["Fictional Strait"]).remove(0);
    assert_eq!(r.status, "country_context_required", "{}", r.reason);
    assert_eq!(r.geometry, None);
    assert_eq!(r.selected_candidate_id.as_deref(), Some("geonames:900009"));
    // With a global baseline that knows no other place of that name, it is placed.
    let clean = candidate(&tmp.path().join("clean"), "cities500");
    s.activate_geography(&[national.to_str().unwrap().into(), clean.to_str().unwrap().into()], &AtomicBool::new(false))
        .unwrap();
    let r = in_quote(&GeoResolver::load_for_store(&s).unwrap(), text, &["Fictional Strait"]).remove(0);
    assert_eq!(r.status, "resolved", "{}", r.reason);
    assert_eq!(r.precision, relic_app::processing_geo::MARINE_FEATURE_PRECISION);
    // A same-name place elsewhere, even only as an alternate name, blocks it.
    let rival = edition(&tmp.path().join("rival"), "cities500", &format!("{ROWS}900015\tHarbor Town\tHarbor Town\tFictional Strait\t23.6\t58.5\tP\tPPL\tOM\t\t01\t\t\t\t5000\t\t0\tAsia/Muscat\t2026-10-01\n"));
    s.activate_geography(&[national.to_str().unwrap().into(), rival.to_str().unwrap().into()], &AtomicBool::new(false))
        .unwrap();
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    let r = in_quote(&resolver, text, &["Fictional Strait"]).remove(0);
    assert_eq!(r.status, "country_context_required", "{}", r.reason);
    assert_eq!(r.geometry, None);
    // A written country still places it.
    let text = "Fictional tanker struck in Fictional Strait, Yemen.";
    let r = in_quote(&resolver, text, &["Fictional Strait"]).remove(0);
    assert_eq!(r.status, "resolved", "{}", r.reason);
}

#[test]
fn channels_narrows_and_shoals_need_a_loaded_sea_nearby() {
    let (tmp, s) = workspace();
    let rows = format!(
        "{ROWS}{}{}{}{}",
        "900016\tFictional Channel\tFictional Channel\t\t12.5\t53.0\tH\tCHN\tYE\t\t00\t\t\t\t0\t\t0\tAsia/Aden\t2026-10-01\n",
        "900017\tEdge Shoal\tEdge Shoal\t\t13.0\t54.15\tH\tSHOL\tYE\t\t00\t\t\t\t0\t\t0\tAsia/Aden\t2026-10-01\n",
        "900018\tInland Narrows\tInland Narrows\t\t15.5\t44.2\tH\tNRWS\tYE\t\t00\t\t\t\t0\t\t0\tAsia/Aden\t2026-10-01\n",
        "900019\tHole Channel\tHole Channel\t\t11.5\t51.5\tH\tCHN\tYE\t\t00\t\t\t\t0\t\t0\tAsia/Aden\t2026-10-01\n",
    );
    let dir = edition(&tmp.path().join("YE"), "YE", &rows);
    s.activate_geography(&[dir.to_str().unwrap().into()], &AtomicBool::new(false))
        .unwrap();
    let place = |resolver: &GeoResolver, name: &str| {
        in_quote(resolver, &format!("Fictional tanker struck in {name}, Yemen."), &[name]).remove(0)
    };
    // GeoNames also uses these codes for river channels and sandbars: with no
    // sea loaded, none is placed. Straits and atolls are unaffected.
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    for name in ["Fictional Channel", "Edge Shoal", "Inland Narrows", "Hole Channel"] {
        let r = place(&resolver, name);
        assert_eq!(r.geometry, None, "{name}: {}", r.reason);
        assert!(r.selected_candidate_id.is_some(), "{name}");
    }
    assert!(place(&resolver, "Fictional Strait").geometry.is_some());
    fictional_seas(tmp.path(), &s);
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    // Inside the sea, or about 16 km outside its edge: placed.
    for (name, at) in [("Fictional Channel", [53.0, 12.5]), ("Edge Shoal", [54.15, 13.0])] {
        let r = place(&resolver, name);
        assert_eq!(r.status, "resolved", "{name}: {}", r.reason);
        assert_eq!(r.geometry.unwrap()["coordinates"], json!(at));
    }
    // Far inland, or in the middle of the sea outline's hole: identity only.
    for name in ["Inland Narrows", "Hole Channel"] {
        let r = place(&resolver, name);
        assert_eq!(r.geometry, None, "{name}: {}", r.reason);
        assert_ne!(r.status, "resolved");
    }
    assert!(place(&resolver, "Fictional Strait").geometry.is_some());
}

#[test]
fn attached_context_outside_excerpt_keeps_exact_support_and_rejects_unrelated_places() {
    let (tmp, s) = workspace();
    let dir = candidate(&tmp.path().join("attached"), "YE");
    s.activate_geography(&[dir.to_str().unwrap().into()], &AtomicBool::new(false))
        .unwrap();
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    let mention = "خليج خيالي";
    let quote = format!("Fictional fire at {mention}");
    for suffix in [
        ", Ambiguous Bay province.",
        " بمحافظة خيالية.",
        " در استان ساختگی.",
        " in Yemen.",
        "، اليمن.",
    ] {
        let text = format!("🧪 Test header. {quote}{suffix} Later unrelated mention of France.");
        let p = LocationProposal {
            mention: mention.into(),
            quote: quote.clone(),
            role: LocationRole::Event,
            canonical_name: None,
            country_code: None,
        };
        let r = resolver
            .resolve(&text, std::slice::from_ref(&p), true)
            .unwrap()
            .remove(0);
        assert_eq!(r.status, "resolved", "{suffix}: {}", r.reason);
        assert_eq!(r.geometry.unwrap()["coordinates"], json!([43., 14.]));
        assert_eq!(r.quote, quote);
        assert_eq!(r.mention, mention);
        let support = &r.literal_qualifier.as_ref().unwrap()["support"];
        let a = support["start_character"].as_u64().unwrap() as usize;
        let b = support["end_character"].as_u64().unwrap() as usize;
        assert_eq!(
            text.chars().skip(a).take(b - a).collect::<String>(),
            support["quote"]
        );
        assert!(!support["quote"].as_str().unwrap().contains("France"));
        for (role, reported) in [
            (LocationRole::Issuer, true),
            (LocationRole::Origin, true),
            (LocationRole::Destination, true),
            (LocationRole::Event, false),
        ] {
            let p = LocationProposal { role, ..p.clone() };
            let r = resolver.resolve(&text, &[p], reported).unwrap().remove(0);
            assert!(r.geometry.is_none() && r.area.is_none(), "{suffix}");
            assert_eq!(r.status, "context_only");
        }
    }
    for suffix in [
        ". Officials in Yemen commented.",
        "\n\nAmbiguous Bay province",
        " #Yemen",
        ", #Yemen.",
        ", Ambiguous Bay province and Saudi Arabia.",
        " while an official spoke in Yemen.",
        ", province of Distant Cove.",
    ] {
        let text = format!("{quote}{suffix}");
        let p = LocationProposal {
            mention: mention.into(),
            quote: quote.clone(),
            role: LocationRole::Event,
            canonical_name: None,
            country_code: None,
        };
        let r = resolver.resolve(&text, &[p], true).unwrap().remove(0);
        assert!(
            r.geometry.is_none() && r.area.is_none(),
            "{suffix}: {}",
            r.status
        );
    }
    let p = LocationProposal {
        mention: mention.into(),
        quote: quote.clone(),
        role: LocationRole::Event,
        canonical_name: None,
        country_code: Some("SA".into()),
    };
    let r = resolver
        .resolve(&format!("{quote}, Ambiguous Bay province."), &[p], true)
        .unwrap()
        .remove(0);
    assert!(r.geometry.is_none());
    let p = LocationProposal {
        mention: mention.into(),
        quote: quote.clone(),
        role: LocationRole::Event,
        canonical_name: None,
        country_code: None,
    };
    let r = resolver
        .resolve(&format!("{quote}. {quote} in Yemen."), &[p], true)
        .unwrap()
        .remove(0);
    assert_eq!(r.status, "unsupported_quote");
    // A contradictory province must veto a unique city rather than silently
    // dropping the qualifier and restoring its otherwise plausible point.
    let p = proposal("Distant Cove", LocationRole::Event);
    let text = "Fictional fire in Distant Cove, Ambiguous Bay province.";
    let r = resolver
        .resolve(
            text,
            &[LocationProposal {
                quote: "Fictional fire in Distant Cove".into(),
                ..p
            }],
            true,
        )
        .unwrap()
        .remove(0);
    assert_eq!(r.status, "country_conflict");
    assert!(r.geometry.is_none() && r.area.is_none());
    let p = LocationProposal {
        mention: mention.into(),
        quote: format!("hit {mention}"),
        role: LocationRole::Event,
        canonical_name: None,
        country_code: None,
    };
    let text = format!("Fictional flight from {mention}, Yemen, hit {mention}.");
    let r = resolver.resolve(&text, &[p], true).unwrap().remove(0);
    assert!(r.geometry.is_none() && r.area.is_none());
    assert!(r.literal_qualifier.is_none());
    let p = LocationProposal {
        mention: "Yemen".into(),
        quote: "Yemen, Ambiguous Bay province".into(),
        role: LocationRole::Event,
        canonical_name: None,
        country_code: None,
    };
    let r = resolver
        .resolve(&p.quote, std::slice::from_ref(&p), true)
        .unwrap()
        .remove(0);
    assert_eq!(r.status, "broad");
    assert_eq!(r.candidates[0].kind, "country");
    assert!(r.literal_qualifier.is_none());
    for (name, expected) in [
        ("Other Cove", "country_conflict"),
        ("Fictional Peak", "broad"),
        ("Unclassified Cove", "ambiguous"),
    ] {
        for compound in [false, true] {
            let mention = if compound {
                format!("{name}, Ambiguous Bay province")
            } else {
                name.to_owned()
            };
            let text = format!("Fictional report at {name}, Ambiguous Bay province.");
            let p = LocationProposal {
                mention,
                quote: text.clone(),
                role: LocationRole::Event,
                canonical_name: None,
                country_code: None,
            };
            let result = resolver.resolve(&text, &[p], true).unwrap().remove(0);
            assert_eq!(result.status, expected, "{name}, compound={compound}");
            assert!(result.geometry.is_none() && result.area.is_none());
            assert!(!result.occurrence_eligible);
            if expected != "broad" {
                assert_eq!(result.precision, "unresolved");
            }
            if name != "Other Cove" {
                assert_eq!(result.candidate_count, 1);
            }
        }
    }
    let text = "Fictional fire in Other Cove, Other Region province.";
    let p = LocationProposal {
        mention: "Other Cove".into(),
        quote: text.into(),
        role: LocationRole::Event,
        canonical_name: None,
        country_code: None,
    };
    assert_eq!(
        resolver.resolve(text, &[p], true).unwrap()[0].status,
        "resolved"
    );
}

#[test]
fn compound_names_require_literal_country_or_publisher_province_support() {
    let (tmp, s) = workspace();
    let dir = candidate(&tmp.path().join("compound"), "cities500");
    s.activate_geography(&[dir.to_str().unwrap().into()], &AtomicBool::new(false))
        .unwrap();
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    for name in [
        "Fictional Cove, Yemen",
        "Fictional Cove (Yemen)",
        "Fictional Cove, Ambiguous Bay province",
    ] {
        let mut p = proposal(name, LocationRole::Event);
        let result = resolver
            .resolve(&p.quote, std::slice::from_ref(&p), true)
            .unwrap();
        assert_eq!(result[0].status, "resolved", "{name}");
        assert_eq!(result[0].mention, name);
        assert_eq!(
            result[0].geometry.as_ref().unwrap()["coordinates"],
            json!([43.0, 14.0])
        );
        assert!(result[0].literal_qualifier.is_some());
        p.role = LocationRole::Mentioned;
        assert!(
            resolver
                .resolve(&p.quote, std::slice::from_ref(&p), true)
                .unwrap()[0]
                .geometry
                .is_none()
        );
        p.role = LocationRole::Event;
        assert!(
            resolver
                .resolve(&p.quote, std::slice::from_ref(&p), false)
                .unwrap()[0]
                .geometry
                .is_none()
        );
    }
    let quote = "Fictional fires in Fictional Cove, Yemen and Distant Cove, Saudi Arabia.";
    let proposals =
        ["Fictional Cove, Yemen", "Distant Cove, Saudi Arabia"].map(|name| LocationProposal {
            quote: quote.into(),
            ..proposal(name, LocationRole::Event)
        });
    let places = resolver.resolve(quote, &proposals, true).unwrap();
    assert!(
        places
            .iter()
            .all(|p| p.geometry.is_some() && p.status == "resolved"),
        "{places:?}"
    );
    assert!(places.iter().all(|p| p.candidates.iter().all(|c| {
        c.alias_reference
            .as_ref()
            .is_none_or(|a| a.get("literal_qualifier").is_none())
    })));
    for name in [
        "Fictional Cove, Saudi Arabia",
        "Distant Cove, Ambiguous Bay province",
        "Fictional Cove, Ambiguous Bay",
        "Fictional Cove, Yemen, Saudi Arabia",
        "Unknown Cove, Yemen",
    ] {
        let p = proposal(name, LocationRole::Event);
        assert!(
            resolver
                .resolve(&p.quote, std::slice::from_ref(&p), true)
                .unwrap()[0]
                .geometry
                .is_none(),
            "{name}"
        );
    }
}

#[test]
fn qualified_ambiguity_retains_exact_candidates_without_choosing_a_point() {
    let (tmp, s) = workspace();
    let dir = candidate(&tmp.path().join("qualified-ambiguity"), "cities500");
    s.activate_geography(&[dir.to_str().unwrap().into()], &AtomicBool::new(false))
        .unwrap();
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    for name in ["Ambiguous Bay, Yemen", "Ambiguous Bay (Yemen)"] {
        let p = proposal(name, LocationRole::Event);
        let result = resolver
            .resolve(&p.quote, std::slice::from_ref(&p), true)
            .unwrap();
        let place = &result[0];
        assert_eq!(place.status, "ambiguous");
        assert_eq!(place.candidate_count, 2);
        assert_eq!(place.mention, name);
        assert!(
            place
                .candidates
                .iter()
                .all(|c| c.country_code.as_deref() == Some("YE"))
        );
        assert!(place.literal_qualifier.is_some());
        assert!(place.selected_candidate_id.is_none());
        assert!(place.geometry.is_none() && place.area.is_none());
    }
    let mut p = proposal("Ambiguous Bay, Yemen", LocationRole::Event);
    p.country_code = Some("SA".into());
    let result = resolver
        .resolve(&p.quote, std::slice::from_ref(&p), true)
        .unwrap();
    assert!(result[0].geometry.is_none() && result[0].area.is_none());
    p.country_code = None;
    p.quote = "A different literal location.".into();
    let result = resolver
        .resolve(&p.quote, std::slice::from_ref(&p), true)
        .unwrap();
    assert_eq!(result[0].status, "unsupported_quote");
    assert!(result[0].literal_qualifier.is_none());
}

#[test]
fn activated_geography_is_versioned_cancellable_and_restorable() {
    let (tmp, s) = workspace();
    let dir = candidate(&tmp.path().join("ref"), "cities500");
    let paths = vec![dir.to_str().unwrap().to_owned()];
    assert!(
        s.activate_geography(&paths, &AtomicBool::new(true))
            .is_err()
    );
    assert!(s.geography().unwrap().is_none());
    let active = s
        .activate_geography(&paths, &AtomicBool::new(false))
        .unwrap();
    let fresh_config = Config::local(
        s.config.root.clone(),
        tmp.path().join("fresh"),
        "127.0.0.1:0".parse().unwrap(),
    );
    let report = relic_app::fresh_workspace::prepare(fresh_config.clone(), &s.config.data).unwrap();
    assert_eq!(report["messages"], 0);
    let fresh = Store::open(fresh_config).unwrap();
    let fresh_resolver = GeoResolver::load_for_store(&fresh).unwrap();
    let place = proposal("Fictional Cove", LocationRole::Event);
    assert!(
        fresh_resolver
            .resolve(&place.quote, std::slice::from_ref(&place), true)
            .unwrap()[0]
            .geometry
            .is_some()
    );
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    for (name, status) in [
        ("Fictional Cove", "resolved"),
        ("خليج خيالي", "resolved"),
        ("Ambiguous Bay", "ambiguous"),
        ("city of Ambiguous Bay", "resolved"),
        ("province of Ambiguous Bay", "broad"),
    ] {
        let p = proposal(name, LocationRole::Event);
        let r = resolver
            .resolve(&p.quote, std::slice::from_ref(&p), true)
            .unwrap();
        assert_eq!(r[0].status, status, "{name}");
        assert_eq!(r[0].geometry.is_some(), status == "resolved");
        assert_eq!(r[0].quote, p.quote);
    }
    for role in [
        LocationRole::Issuer,
        LocationRole::Destination,
        LocationRole::Mentioned,
    ] {
        let p = proposal("Fictional Cove", role);
        assert!(
            resolver
                .resolve(&p.quote, std::slice::from_ref(&p), true)
                .unwrap()[0]
                .geometry
                .is_none()
        );
    }
    let mut p = proposal("Fictional Cove", LocationRole::Event);
    p.country_code = Some("FR".into());
    assert_eq!(
        resolver
            .resolve(&p.quote, std::slice::from_ref(&p), true)
            .unwrap()[0]
            .status,
        "country_conflict"
    );
    p.country_code = None;
    assert!(
        resolver
            .resolve(&p.quote, std::slice::from_ref(&p), false)
            .unwrap()[0]
            .geometry
            .is_none()
    );
    fs::write(dir.join("index.sqlite3"), b"broken").unwrap();
    assert!(
        s.activate_geography(&paths, &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(s.geography().unwrap().unwrap(), active);
    let backup = s.backup().unwrap();
    let id = backup["id"].as_str().unwrap();
    assert_eq!(s.verify_backup(id).unwrap()["verified"], true);
    // Open only the independent backup. Its accepted reference assets survive
    // loss of the acquisition directory and do not depend on developer paths.
    fs::remove_dir_all(&dir).unwrap();
    let restore_dir = tmp.path().join("restored");
    fs::create_dir_all(restore_dir.join("objects")).unwrap();
    let backup_dir = s.config.data.join("backups").join(id);
    fs::copy(
        backup_dir.join("relic.sqlite3"),
        restore_dir.join("relic.sqlite3"),
    )
    .unwrap();
    fs::rename(
        backup_dir.join("objects"),
        restore_dir.join("objects/sha256"),
    )
    .unwrap();
    let restored = Store::open(Config::local(
        s.config.root.clone(),
        restore_dir,
        "127.0.0.1:8794".parse().unwrap(),
    ))
    .unwrap();
    let r = GeoResolver::load_for_store(&restored).unwrap();
    let p = proposal("Fictional Cove", LocationRole::Event);
    assert!(
        r.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap()[0]
            .geometry
            .is_some()
    );
}
#[test]
fn geography_reuses_paid_output_without_rewriting_old_publications_or_recipe() {
    for legacy in [true, false] {
        let (tmp, s) = workspace();
        let text = "Fictional fire in Fictional Cove.";
        let e = json!({"id":"fiction:geo","revision":"v1","source":"fiction:source","published_at":"2026-09-12T00:00:00Z","acquired_at":"2026-09-12T00:01:00Z","text":text,"geometry":null,"location_precision":"unresolved","fictional":true,"availability":"available","language":"en","dependence":"unknown","origin_group":null,"citation":{"evidence_id":"fiction:geo","revision":"v1","start_character":0,"end_character":text.len()}});
        s.import_job(json!({"schema_version":1,"fictional":true,"title":"Fictional geography","sources":[{"id":"fiction:source","label":"Fictional source","family":"authored_report"}],"evidence_revisions":[e]}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
        s.set_processing_policy(Policy {
            enabled: false,
            eligible_source_ids: vec!["fiction:source".into()],
        })
        .unwrap();
        let row = s
            .processing_pending(&s.processing_policy().unwrap())
            .unwrap()
            .remove(0);
        let out=validate_output(text,&json!({"language":"en","translation":null,"headline":"Fictional fire","kind":"event","claim_status":"reported","topic":"disaster","claims":[{"text":"Fictional fire","quote":text,"state":"reported","event_type":"fire","time_text":null,"location_indices":[0]}],"locations":[{"mention":"Fictional Cove","quote":text,"role":"event","canonical_name":"Fictional Cove","country_code":null}]})).unwrap();
        let mut p = processing_store::prepare(
            row.rowid,
            &row.evidence,
            &out,
            &GeoResolver::load(&s.config.root).unwrap(),
            processing_ai::LEGACY_MODEL,
        )
        .unwrap();
        p.interpretation["recipe"] = json!("message-reading-v2");
        if legacy {
            p.interpretation.as_object_mut().unwrap().remove("events");
        } else {
            p.interpretation["reading_revision"] = json!("translation-repair-v1");
        }
        let attempt = s.reserve_processing(row.rowid).unwrap();
        let audit = processing_ai::ResponseAudit {
            requested_model: processing_ai::LEGACY_MODEL.into(),
            served_model: None,
            provider: None,
            generation_id: None,
            usage: Default::default(),
            source_input_hash: relic_app::model::hash(text.as_bytes()),
            response_hash: relic_app::model::hash(b"fiction"),
            prompt_version: "old".into(),
            parameters: json!({}),
            elapsed_ms: 0,
            http_status: 200,
            raw_response: vec![],
            conservative_normalizations: vec![],
            rejection: None,
        };
        p.audit = value(&audit);
        s.finish_processing_attempt(
            &attempt,
            "completed",
            Some(0.),
            &value(processing_ai::Analysis {
                extracted: out.clone(),
                audit: audit.clone(),
            }),
            None,
        )
        .unwrap();
        let old = s
            .publish_processing(vec![p], &ident("processing"), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        let dir = candidate(&tmp.path().join("ref"), "cities500");
        s.activate_geography(&[dir.to_str().unwrap().into()], &AtomicBool::new(false))
            .unwrap();
        let result = s
            .reresolve_geography(&ident("geo"), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(result["resolved_readings"], 1);
        assert_eq!(result["model_calls"], 0);
        assert_eq!(
            s.reresolve_geography(&ident("geo"), &AtomicBool::new(false))
                .unwrap()["resolved_readings"],
            0
        );
        let current = s
            .detail_with_publication("fiction:geo", "v1", None)
            .unwrap();
        assert!(current["interpretation"]["geometry"].is_object());
        assert_eq!(current["interpretation"]["recipe"], "message-reading-v2");
        assert_eq!(current["interpretation"].get("events").is_none(), legacy);
        if !legacy {
            assert_eq!(
                current["interpretation"]["reading_revision"],
                "translation-repair-v1"
            );
        }
        let previous = s
            .detail_with_publication("fiction:geo", "v1", Some(&old))
            .unwrap();
        assert!(previous["interpretation"]["geometry"].is_null());
        assert_eq!(current["text"], previous["text"]);
        assert_eq!(s.processing_status(false).unwrap()["needs_upgrade"], 1);
        // Returning to edition A must reuse its immutable interpretation rather
        // than collide on (evidence, recipe), and later imports must preserve A.
        let a = current["interpretation"].clone();
        let publication_a: String = s
            .connect()
            .unwrap()
            .query_row(
                "SELECT value FROM metadata WHERE key='current_publication'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let regional = candidate(&tmp.path().join("regional"), "YE");
        s.activate_geography(
            &[
                dir.to_str().unwrap().into(),
                regional.to_str().unwrap().into(),
            ],
            &AtomicBool::new(false),
        )
        .unwrap();
        s.reresolve_geography(&ident("geo"), &AtomicBool::new(false))
            .unwrap();
        let b = s
            .detail_with_publication("fiction:geo", "v1", None)
            .unwrap()["interpretation"]
            .clone();
        assert_ne!(a["id"], b["id"]);
        let publication_b: String = s
            .connect()
            .unwrap()
            .query_row(
                "SELECT value FROM metadata WHERE key='current_publication'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        s.activate_geography(&[dir.to_str().unwrap().into()], &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            s.reresolve_geography(&ident("geo"), &AtomicBool::new(false))
                .unwrap()["resolved_readings"],
            1
        );
        assert_eq!(
            s.detail_with_publication("fiction:geo", "v1", None)
                .unwrap()["interpretation"],
            a
        );
        let mut added = row.evidence.clone();
        added["id"] = json!("fiction:next");
        added["citation"]["evidence_id"] = added["id"].clone();
        s.import_job(json!({"schema_version":1,"fictional":true,"title":"Later import","sources":[{"id":"fiction:source","label":"Fictional source","family":"authored_report"}],"evidence_revisions":[added]}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(
            s.detail_with_publication("fiction:geo", "v1", None)
                .unwrap()["interpretation"],
            a
        );
        assert_eq!(
            s.detail_with_publication("fiction:geo", "v1", Some(&publication_a))
                .unwrap()["interpretation"],
            a
        );
        assert_eq!(
            s.detail_with_publication("fiction:geo", "v1", Some(&publication_b))
                .unwrap()["interpretation"],
            b
        );
        assert_eq!(
            s.detail_with_publication("fiction:geo", "v1", Some(&old))
                .unwrap()["interpretation"],
            previous["interpretation"]
        );
        assert_eq!(
            s.reresolve_geography(&ident("geo"), &AtomicBool::new(false))
                .unwrap()["resolved_readings"],
            0
        );
        // Identical-text revision reuse has no new paid attempt. Its retained
        // analysis reference must still participate in geographic replay.
        let audit_hash: String = s
            .connect()
            .unwrap()
            .query_row(
                "SELECT json_extract(json,'$.audit_hash') FROM processing_attempts WHERE id=?1",
                [&attempt],
                |r| r.get(0),
            )
            .unwrap();
        let mut replacement = row.evidence.clone();
        replacement["revision"] = json!("v2");
        replacement["published_at"] = json!("2026-09-12T00:02:00Z");
        replacement["citation"]["revision"] = json!("v2");
        s.import_job(json!({"schema_version":1,"fictional":true,"title":"Unchanged-text revision","sources":[{"id":"fiction:source","label":"Fictional source","family":"authored_report"}],"evidence_revisions":[replacement]}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
        let reused_row: i64 = s
            .connect()
            .unwrap()
            .query_row(
                "SELECT rowid FROM evidence WHERE id='fiction:geo' AND revision='v2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut reuse = processing_store::prepare(
            reused_row,
            &replacement,
            &out,
            &GeoResolver::load_for_store(&s).unwrap(),
            processing_ai::LEGACY_MODEL,
        )
        .unwrap();
        reuse.audit = value(&audit);
        reuse.interpretation["reused_from"] = json!({"evidence_id":"fiction:geo","revision":"v1","audit_hash":audit_hash,"model_calls":0});
        s.publish_processing(vec![reuse], &ident("processing"), &AtomicBool::new(false))
            .unwrap();
        s.activate_geography(
            &[
                dir.to_str().unwrap().into(),
                regional.to_str().unwrap().into(),
            ],
            &AtomicBool::new(false),
        )
        .unwrap();
        let result = s
            .reresolve_geography(&ident("geo"), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(result["resolved_readings"], 1);
        assert_eq!(result["complete"], true);
        assert_eq!(
            s.detail_with_publication("fiction:geo", "v2", None)
                .unwrap()["interpretation"]["reference_edition"],
            result["reference_edition"]
        );
        assert_eq!(
            s.connect()
                .unwrap()
                .query_row("SELECT count(*) FROM processing_attempts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn conflicting_receipt_does_not_block_other_geographic_readings() {
    let (tmp, s) = workspace();
    let text = "Fictional fire in Fictional Cove.";
    let rows = ["fiction:conflicting","fiction:usable"].map(|id| json!({"id":id,"revision":"v1","source":"fiction:source","published_at":"2026-09-12T00:00:00Z","acquired_at":"2026-09-12T00:01:00Z","text":text,"geometry":null,"location_precision":"unresolved","fictional":true,"availability":"available","language":"en","dependence":"unknown","origin_group":null,"citation":{"evidence_id":id,"revision":"v1","start_character":0,"end_character":text.len()}}));
    s.import_job(json!({"schema_version":1,"fictional":true,"title":"Conflicting receipt","sources":[{"id":"fiction:source","label":"Fictional source","family":"authored_report"}],"evidence_revisions":rows}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
    let out = validate_output(text,&json!({"language":"en","translation":null,"headline":text,"kind":"event","claim_status":"reported","topic":"safety","claims":[{"text":text,"quote":text,"state":"reported","event_type":"fire","time_text":null,"location_indices":[0]}],"locations":[{"mention":"Fictional Cove","quote":text,"role":"event","canonical_name":"Fictional Cove","country_code":null}]})).unwrap();
    let resolver = GeoResolver::load(&s.config.root).unwrap();
    let mut prepared = vec![];
    for (index, e) in rows.iter().enumerate() {
        let rowid: i64 = s
            .connect()
            .unwrap()
            .query_row(
                "SELECT rowid FROM evidence WHERE id=?1",
                [e["id"].as_str().unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        let mut p = processing_store::prepare(rowid, e, &out, &resolver, "fixture/gold").unwrap();
        let mut audit = processing_ai::ResponseAudit {
            requested_model: "fixture/gold".into(),
            served_model: None,
            provider: None,
            generation_id: None,
            usage: Default::default(),
            source_input_hash: relic_app::model::hash(text.as_bytes()),
            response_hash: relic_app::model::hash(b"retained response"),
            prompt_version: "fixture".into(),
            parameters: json!({}),
            elapsed_ms: 0,
            http_status: 200,
            raw_response: vec![],
            conservative_normalizations: vec![],
            rejection: None,
        };
        p.audit = value(&audit);
        if index == 0 {
            audit.response_hash = relic_app::model::hash(b"different response");
        }
        let attempt = s.reserve_processing(rowid).unwrap();
        s.finish_processing_attempt(
            &attempt,
            "completed",
            Some(0.),
            &value(processing_ai::Analysis {
                extracted: out.clone(),
                audit,
            }),
            None,
        )
        .unwrap();
        prepared.push(p);
    }
    s.publish_processing(prepared, &ident("processing"), &AtomicBool::new(false))
        .unwrap();
    let dir = candidate(&tmp.path().join("ref"), "cities500");
    s.activate_geography(&[dir.to_str().unwrap().into()], &AtomicBool::new(false))
        .unwrap();
    let result = s
        .reresolve_geography(&ident("geo"), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(result["resolved_readings"], 1);
    assert_eq!(result["conflicting_readings"], 1);
    assert_eq!(
        result["conflict_examples"][0]["evidence_id"],
        "fiction:conflicting"
    );
    assert_eq!(result["complete"], false);
    assert!(
        s.detail_with_publication("fiction:usable", "v1", None)
            .unwrap()["interpretation"]["geometry"]
            .is_object()
    );
    assert!(
        s.detail_with_publication("fiction:conflicting", "v1", None)
            .unwrap()["interpretation"]["geometry"]
            .is_null()
    );
    let again = s
        .reresolve_geography(&ident("geo"), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(again["resolved_readings"], 0);
    assert_eq!(again["conflicting_readings"], 1);
    assert_eq!(
        s.connect()
            .unwrap()
            .query_row("SELECT count(*) FROM processing_attempts", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn geography_jobs_keep_the_shared_workspace_contract_and_migrate_old_results() {
    let (_tmp, store) = workspace();
    let response = relic_app::geography_store::start_job(store.clone(), "geography", |_, _, _| {
        Ok(json!({"resolved_readings":3,"model_calls":0}))
    })
    .await
    .unwrap();
    assert_eq!(response["accepted"], 0);
    assert_eq!(response["total"], 0);
    let id = response["id"].as_str().unwrap();
    let completed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let text: String = store
                .connect()
                .unwrap()
                .query_row("SELECT json FROM jobs WHERE id=?1", [id], |r| r.get(0))
                .unwrap();
            let job: serde_json::Value = serde_json::from_str(&text).unwrap();
            if job["status"] == "completed" {
                break job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(completed["accepted"], 3);
    assert_eq!(completed["total"], 3);
    store
        .connect()
        .unwrap()
        .execute(
            "UPDATE jobs SET json=json_remove(json,'$.accepted','$.total') WHERE id=?1",
            [id],
        )
        .unwrap();
    let reopened = Store::open(store.config.clone()).unwrap();
    let text: String = reopened
        .connect()
        .unwrap()
        .query_row("SELECT json FROM jobs WHERE id=?1", [id], |r| r.get(0))
        .unwrap();
    let restored: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(restored, completed);
    reopened.connect().unwrap().execute_batch("CREATE TRIGGER reject_job BEFORE INSERT ON jobs BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
    assert!(
        relic_app::geography_store::start_job(reopened.clone(), "geography", |_, _, _| Ok(json!(
            {}
        )))
        .await
        .is_err()
    );
    assert!(reopened.running.lock().unwrap().is_empty());
    assert_eq!(reopened.imports.available_permits(), 1);
}

#[tokio::test]
async fn partial_geography_is_not_reported_as_complete() {
    let (_tmp, store) = workspace();
    let response = relic_app::geography_store::start_job(store.clone(),"geography",|_,_,_| Ok(json!({"resolved_readings":3,"conflicting_readings":2,"changed_during_run":1,"complete":false,"model_calls":0}))).await.unwrap();
    let job = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let raw: String = store
                .connect()
                .unwrap()
                .query_row(
                    "SELECT json FROM jobs WHERE id=?1",
                    [response["id"].as_str().unwrap()],
                    |r| r.get(0),
                )
                .unwrap();
            let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
            if value["status"] != "running" {
                break value;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(job["status"], "partial");
    assert_eq!(job["accepted"], 3);
    assert_eq!(job["total"], 6);
    assert!(
        job["detail"]
            .as_str()
            .unwrap()
            .contains("retained their previous reading")
    );
}

#[test]
fn conflicting_countries_in_one_occurrence_do_not_create_distant_neighborhood_dots() {
    let (tmp, s) = workspace();
    let dir = candidate(&tmp.path().join("references"), "cities500");
    s.activate_geography(&[dir.to_str().unwrap().to_owned()], &AtomicBool::new(false))
        .unwrap();
    let resolver = GeoResolver::load_for_store(&s).unwrap();
    let quote = "Fictional outages affected Fictional Cove and its neighborhood Distant Cove.";
    let mut places = [
        proposal("Fictional Cove", LocationRole::Event),
        proposal("Distant Cove", LocationRole::Event),
    ];
    for p in &mut places {
        p.quote = quote.into();
    }
    let ambiguous = resolver.resolve(quote, &places, true).unwrap();
    assert!(
        ambiguous.iter().all(|p| p.status == "context_conflict"
            && p.geometry.is_none()
            && !p.occurrence_eligible)
    );
    // Explicit source country context can describe real cross-border events.
    let cross_border = "Fictional fires in Fictional Cove, Yemen, and Distant Cove, Saudi Arabia.";
    for p in &mut places {
        p.quote = cross_border.into();
    }
    places[0].country_code = Some("YE".into());
    places[1].country_code = Some("SA".into());
    assert!(
        resolver
            .resolve(cross_border, &places, true)
            .unwrap()
            .iter()
            .all(|p| p.geometry.is_some())
    );
    // An issuing authority elsewhere must not erase a valid event location.
    places[0].country_code = None;
    places[1].role = LocationRole::Issuer;
    let result = resolver.resolve(cross_border, &places, true).unwrap();
    assert!(result[0].geometry.is_some());
    assert!(result[1].geometry.is_none());
}

#[test]
fn supported_context_qualifiers_and_aliases_do_not_invent_city_points() {
    let (tmp, s) = workspace();
    let dir = tmp.path().join("context-reference");
    fs::create_dir_all(&dir).unwrap();
    let places = dir.join("places.txt");
    let aliases = dir.join("aliases.txt");
    fs::write(&places, concat!(
        "910001\tRegional Hollow\tRegional Hollow\t\t32.5\t34.5\tA\tADM1\tLB\t\t01\t\t\t\t0\t\t0\tAsia/Beirut\t2026-09-21\n",
        "910002\tHollow\tHollow\t\t32.5\t34.5\tP\tPPLA\tLB\t\t01\t\t\t\t1000\t\t0\tAsia/Beirut\t2026-09-21\n",
        "910003\tForeign School\tForeign School\t\t15\t44\tS\tSCH\tYE\t\t01\t\t\t\t0\t\t0\tAsia/Aden\t2026-09-21\n",
        "910004\tFictional Capital\tFictional Capital\t\t32.5\t34.5\tP\tPPLC\tLB\t\t01\t\t\t\t1000\t\t0\tAsia/Beirut\t2026-09-21\n",
        "910005\tFictional Capital\tFictional Capital\t\t32.5\t34.5\tA\tADM1\tLB\t\t02\t\t\t\t0\t\t0\tAsia/Beirut\t2026-09-21\n",
        "910006\tFictional Capital\tFictional Capital\t\t33\t35\tP\tPPL\tLB\t\t02\t\t\t\t10\t\t0\tAsia/Beirut\t2026-09-21\n"
    )).unwrap();
    fs::write(
        &aliases,
        concat!(
            "91\t910001\tar\tخيالي\t\t\t\t\t\t\n",
            "92\t910002\tar\tخيالي\t\t\t\t\t\t\n",
            "93\t910003\tar\tخيالي\t\t\t\t\t\t\n",
            "94\t910001\ten\tHollow\t\t\t\t\t\t\n"
        ),
    )
    .unwrap();
    let dest = dir.join("edition");
    let mut m=geonames::build(&dest,&places,&aliases,json!({"product":"cities500","fictional":true,"requests":[{"url":"https://download.geonames.org/export/dump/cities500.zip","acquired_at":"2026-09-21T00:00:00Z"},{"url":"https://download.geonames.org/export/dump/readme.txt","acquired_at":"2026-09-21T00:00:00Z"}]}),&AtomicBool::new(false)).unwrap();
    for name in ["places.zip", "readme.txt"] {
        fs::write(dest.join(name), b"fictional").unwrap();
        m["assets"][name] = json!({"sha256":relic_app::model::hash(b"fictional"),"bytes":9});
    }
    fs::write(dest.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    s.activate_geography(&[dest.to_str().unwrap().into()], &AtomicBool::new(false))
        .unwrap();
    lebanon_boundaries(tmp.path(), &s);
    let r = GeoResolver::load_for_store(&s).unwrap();
    // A bare name shared by a province and its own seat names the seat unless
    // the province dominates it by population (v14); explicit province wording
    // below still selects the province area.
    let mut p = proposal("خيالي", LocationRole::Event);
    p.canonical_name = Some("Hollow".into());
    let a = r.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
    assert_eq!(a[0].status, "resolved", "{}", a[0].reason);
    assert_eq!(a[0].selected_candidate_id.as_deref(), Some("geonames:910002"));
    assert_eq!(a[0].mention, p.mention);
    for name in [
        "province of Hollow",
        "Hollow governorate",
        "#Hollow province",
    ] {
        let mut named = proposal(name, LocationRole::Event);
        named.country_code = Some("LB".into());
        let area = r.resolve(&named.quote, &[named.clone()], true).unwrap();
        assert_eq!(area[0].status, "resolved_area", "{name}");
        assert!(area[0].geometry.is_none());
        assert_eq!(area[0].mention, name);
    }
    // Without canonical support the shared alias names a provincial seat, its
    // own province and a foreign school: the seat is the principal settlement.
    // An unmatched canonical name carries no contrary evidence.
    for canonical in [None, Some("Unknown Translation".to_string())] {
        p.canonical_name = canonical;
        let r = r.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap().remove(0);
        assert_eq!(r.status, "resolved", "{}", r.reason);
        assert_eq!(r.selected_candidate_id.as_deref(), Some("geonames:910002"));
    }
    // A national capital dominates its own province and a 10-person village,
    // with or without a literal "capital" qualifier.
    p = proposal("Fictional Capital", LocationRole::Event);
    p.quote = "Fictional explosion in the capital Fictional Capital.".into();
    assert_eq!(
        r.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap()[0].selected_candidate_id.as_deref(),
        Some("geonames:910004")
    );
    p.quote = "Fictional explosion in the national capital Fictional Capital.".into();
    assert!(
        r.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap()[0]
            .geometry
            .is_some()
    );
    p.quote = "Fictional explosion in Fictional Capital.".into();
    assert_eq!(
        r.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap()[0].selected_candidate_id.as_deref(),
        Some("geonames:910004")
    );
    p = proposal("Unknown Ridge", LocationRole::Event);
    p.quote = "Fictional fire on Unknown Ridge, southern Lebanon.".into();
    let a = r.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
    assert_eq!(a[0].status, "resolved_area");
    assert_eq!(a[0].area.as_ref().unwrap()["level"], "ADM0");
    for quote in [
        "Fictional fire on Unknown Ridge, Lebanon.",
        "Fictional fire on Unknown Ridge, Lebanon, today.",
        "Fictional fire on Unknown Ridge in Lebanon. Officials in Syria commented.",
    ] {
        p.quote = quote.into();
        assert_eq!(
            r.resolve(quote, std::slice::from_ref(&p), true).unwrap()[0].status,
            "resolved_area"
        );
    }
    for role in [
        LocationRole::Issuer,
        LocationRole::Mentioned,
        LocationRole::Destination,
    ] {
        p.role = role;
        assert!(
            r.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap()[0]
                .area
                .is_none()
        );
    }
    p.role = LocationRole::Event;
    assert!(
        r.resolve(&p.quote, std::slice::from_ref(&p), false)
            .unwrap()[0]
            .area
            .is_none()
    );
    p.country_code = Some("SY".into());
    assert!(
        r.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap()[0]
            .area
            .is_none()
    );
    p.country_code = None;
    for text in [
        "Fictional strikes hit Unknown Ridge, Lebanon, and Syria.",
        "Fictional strikes hit Unknown Ridge, Lebanon, Syria.",
        "Fictional fire on Unknown Ridge. Officials responded from Lebanon.",
        "Fictional fire on Unknown Ridge while officials met in southern Lebanon.",
        "Fictional fire on Unknown Ridge, Yemen, reported by an office in Lebanon.",
    ] {
        p.quote = text.into();
        assert!(
            r.resolve(text, std::slice::from_ref(&p), true).unwrap()[0]
                .area
                .is_none()
        );
    }
    // A city in the full polygon's hole must not be assigned to that province.
    s.connect().unwrap().execute("UPDATE boundary_shapes SET full_json=?1 WHERE level='ADM1'",[serde_json::to_string(&json!({"type":"Polygon","coordinates":[[[34.,32.],[37.,32.],[37.,35.],[34.,35.],[34.,32.]],[[34.25,32.25],[34.75,32.25],[34.75,32.75],[34.25,32.75],[34.25,32.25]]]})).unwrap()]).unwrap();
    // The seat itself may still be placed; the province area is not assigned.
    p = proposal("خيالي", LocationRole::Event);
    p.canonical_name = Some("Hollow".into());
    let hole = r.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap().remove(0);
    assert!(hole.area.is_none());
    assert_ne!(hole.selected_candidate_id.as_deref(), Some("geonames:910001"));
}
