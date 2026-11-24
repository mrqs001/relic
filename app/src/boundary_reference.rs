//! Explicit, versioned gbOpen acquisition. No source messages are sent upstream.
use crate::{
    area_geometry::{AreaGeometry, MAX_VERTICES},
    error::{Error, Result},
    model::{hash, now, required},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
pub fn invalid() -> Error {
    Error::invalid(
        "Boundary edition is unavailable, invalid or unsupported; prior areas remain active",
    )
}
fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::cancelled())
    } else {
        Ok(())
    }
}
pub fn country(code: &str) -> bool {
    code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase())
}
pub fn level(level: &str) -> bool {
    matches!(level, "ADM0" | "ADM1")
}
fn permitted(url: &url::Url) -> bool {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
    {
        return false;
    }
    match url.host_str() {
        Some("www.geoboundaries.org") => url.path().starts_with("/api/current/gbOpen/"),
        Some("github.com") => url.path().starts_with("/wmgeolab/geoBoundaries/raw/"),
        Some("raw.githubusercontent.com") => url.path().starts_with("/wmgeolab/geoBoundaries/"),
        Some("media.githubusercontent.com") => {
            url.path().starts_with("/media/wmgeolab/geoBoundaries/")
        }
        _ => false,
    }
}
async fn download(
    client: &wreq::Client,
    start: &str,
    max: usize,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, Value)> {
    let operation = async {
        let mut url = url::Url::parse(start).map_err(|_| invalid())?;
        for _ in 0..5 {
            check(cancel)?;
            if !permitted(&url) {
                return Err(invalid());
            }
            let response = client
                .get(url.as_str())
                .send()
                .await
                .map_err(|_| invalid())?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(wreq::header::LOCATION)
                    .and_then(|h| h.to_str().ok())
                    .ok_or_else(invalid)?;
                url = url.join(location).map_err(|_| invalid())?;
                continue;
            }
            if !response.status().is_success()
                || response.content_length().is_some_and(|n| n > max as u64)
            {
                return Err(invalid());
            }
            let modified = response
                .headers()
                .get(wreq::header::LAST_MODIFIED)
                .and_then(|h| h.to_str().ok())
                .map(str::to_owned);
            let mut stream = response.bytes_stream();
            let mut bytes = vec![];
            while let Some(chunk) = stream.next().await {
                check(cancel)?;
                let chunk = chunk.map_err(|_| invalid())?;
                if bytes.len() + chunk.len() > max {
                    return Err(invalid());
                }
                bytes.extend_from_slice(&chunk);
            }
            let receipt = json!({"requested_url":start,"resolved_url":url.as_str(),"acquired_at":now(),"publisher_last_modified":modified,"bytes":bytes.len(),"sha256":hash(&bytes)});
            return Ok((bytes, receipt));
        }
        Err(invalid())
    };
    tokio::select! {r=operation=>r,_=async {loop {if cancel.load(Ordering::Acquire){break}tokio::time::sleep(Duration::from_millis(50)).await;}}=>Err(Error::cancelled())}
}
#[derive(Clone)]
pub struct Shape {
    pub id: String,
    pub name: String,
    pub full: Value,
    pub display: Value,
    pub bounds: [f64; 4],
    pub vertices: usize,
    pub record_sha256: String,
}
pub fn validate(
    metadata: &Value,
    full: &Value,
    display: &Value,
    cancel: &AtomicBool,
) -> Result<Vec<Shape>> {
    let iso = required(metadata, "boundaryISO")?;
    let tier = required(metadata, "boundaryType")?;
    if !country(iso) || !level(tier) || required(metadata, "boundaryID")?.len() > 200 {
        return Err(invalid());
    }
    for field in [
        "boundaryYearRepresented",
        "boundarySource",
        "boundaryLicense",
        "licenseSource",
        "sourceDataUpdateDate",
        "buildDate",
    ] {
        if required(metadata, field)?.is_empty() {
            return Err(invalid());
        }
    }
    let count = metadata["admUnitCount"]
        .as_str()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .ok_or_else(invalid)?;
    let features = |v: &Value| -> Result<Vec<Value>> {
        if v["type"] != "FeatureCollection" {
            return Err(invalid());
        }
        let features = v["features"].as_array().ok_or_else(invalid)?;
        if features.len() != count {
            return Err(Error::invalid(format!(
                "Publisher metadata declares {count} boundary features but the asset contains {}; edition remains unaccepted",
                features.len()
            )));
        }
        Ok(features.clone())
    };
    let full = features(full)?;
    let display = features(display)?;
    let mut smaller = BTreeMap::new();
    for f in display {
        let id = required(&f["properties"], "shapeID")?.to_owned();
        if smaller.insert(id, f).is_some() {
            return Err(invalid());
        }
    }
    let mut shapes = vec![];
    let mut seen = std::collections::BTreeSet::new();
    for f in full {
        check(cancel)?;
        let p = &f["properties"];
        let id = required(p, "shapeID")?;
        let name = required(p, "shapeName")?;
        if id.len() > 200
            || name.len() > 240
            || p["shapeGroup"] != iso
            || p["shapeType"] != tier
            || !seen.insert(id.to_owned())
        {
            return Err(invalid());
        }
        let d = smaller.remove(id).ok_or_else(invalid)?;
        if d["properties"]["shapeName"] != name
            || d["properties"]["shapeGroup"] != iso
            || d["properties"]["shapeType"] != tier
        {
            return Err(invalid());
        }
        let analytical =
            AreaGeometry::parse(&f["geometry"], MAX_VERTICES, cancel).map_err(|mut e| {
                if e.code != "cancelled" {
                    e.message = format!("Full boundary {id}: {}", e.message);
                }
                e
            })?;
        AreaGeometry::parse(&d["geometry"], MAX_VERTICES, cancel).map_err(|mut e| {
            if e.code != "cancelled" {
                e.message = format!("Display boundary {id}: {}", e.message);
            }
            e
        })?;
        shapes.push(Shape {
            id: id.into(),
            name: name.into(),
            full: f["geometry"].clone(),
            display: d["geometry"].clone(),
            bounds: analytical.bounds,
            vertices: analytical.vertices,
            record_sha256: hash(&serde_json::to_vec(&f)?),
        });
    }
    Ok(shapes)
}
/// A reviewed publisher discrepancy is bounded to exact retained bytes. It is
/// never a general permission to accept metadata/geometry count mismatches.
pub(crate) fn reconciliation(assets: &[Vec<u8>]) -> Result<Option<Value>> {
    let review: Value = serde_json::from_str(include_str!(
        "../../reference/reconciliations/geoboundaries-irn-adm1-17685810.json"
    ))?;
    let names = ["metadata.json", "full.geojson", "display.geojson"];
    if assets.len() == names.len()
        && assets.iter().zip(names).all(|(bytes, name)| {
            review["assets"][name]["sha256"] == hash(bytes)
                && review["assets"][name]["bytes"].as_u64() == Some(bytes.len() as u64)
        })
    {
        Ok(Some(review))
    } else {
        Ok(None)
    }
}
pub(crate) fn validate_assets(assets: &[Vec<u8>], cancel: &AtomicBool) -> Result<Vec<Shape>> {
    if assets.len() != 3 {
        return Err(invalid());
    }
    let mut metadata: Value = serde_json::from_slice(&assets[0])?;
    if let Some(review) = reconciliation(assets)? {
        // Only the validation expectation changes; original metadata remains in CAS.
        metadata["admUnitCount"] = json!(
            review["accepted_features"]
                .as_u64()
                .ok_or_else(invalid)?
                .to_string()
        );
    }
    validate(
        &metadata,
        &serde_json::from_slice(&assets[1])?,
        &serde_json::from_slice(&assets[2])?,
        cancel,
    )
}
pub async fn acquire(iso: &str, tier: &str, dest: &Path, cancel: Arc<AtomicBool>) -> Result<Value> {
    if !country(iso) || !level(tier) {
        return Err(invalid());
    }
    if dest.exists() {
        return review(iso, tier, dest, &cancel);
    }
    let client = wreq::Client::builder()
        .redirect(wreq::redirect::Policy::none())
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|_| invalid())?;
    let endpoint = format!("https://www.geoboundaries.org/api/current/gbOpen/{iso}/{tier}/");
    let (meta, mr) = download(&client, &endpoint, 1024 * 1024, &cancel).await?;
    let metadata: Value = serde_json::from_slice(&meta)?;
    if metadata["boundaryISO"] != iso || metadata["boundaryType"] != tier {
        return Err(invalid());
    }
    let mut assets = BTreeMap::new();
    let mut receipts = vec![mr];
    let mut data = vec![("metadata.json", meta)];
    for (field, name) in [
        ("gjDownloadURL", "full.geojson"),
        ("simplifiedGeometryGeoJSON", "display.geojson"),
    ] {
        let (bytes, receipt) =
            download(&client, required(&metadata, field)?, usize::MAX, &cancel).await?;
        data.push((name, bytes));
        receipts.push(receipt);
    }
    check(&cancel)?;
    fs::create_dir_all(dest.parent().ok_or_else(invalid)?)?;
    fs::create_dir(dest)?;
    for (name, bytes) in data {
        check(&cancel)?;
        assets.insert(name, json!({"sha256":hash(&bytes),"bytes":bytes.len()}));
        fs::write(dest.join(name), bytes)?;
    }
    let manifest = json!({"schema_version":1,"status":"downloaded","publisher":"geoBoundaries","collection":"gbOpen","country":iso,"level":tier,"boundary_id":metadata["boundaryID"],"represented_time":metadata["boundaryYearRepresented"],"source_license":metadata["boundaryLicense"],"attribution":"geoBoundaries / William & Mary; gbOpen CC BY 4.0-compatible; original publisher and licence retained in metadata","assets":assets,"requests":receipts});
    fs::write(
        dest.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    let iso = iso.to_owned();
    let tier = tier.to_owned();
    let dest = dest.to_owned();
    tokio::task::spawn_blocking(move || review(&iso, &tier, &dest, &cancel))
        .await
        .map_err(|_| Error::internal())?
}
/// Revalidate retained raw downloads without another publisher request.
pub fn review(iso: &str, tier: &str, dest: &Path, cancel: &AtomicBool) -> Result<Value> {
    if fs::metadata(dest.join("manifest.json"))?.len() > 64 * 1024 {
        return Err(invalid());
    }
    let mut manifest: Value = serde_json::from_slice(&fs::read(dest.join("manifest.json"))?)?;
    if manifest["country"] != iso
        || manifest["level"] != tier
        || manifest["publisher"] != "geoBoundaries"
        || manifest["collection"] != "gbOpen"
    {
        return Err(invalid());
    }
    let mut assets = vec![];
    for name in ["metadata.json", "full.geojson", "display.geojson"] {
        check(cancel)?;
        let bytes = fs::read(dest.join(name))?;
        if hash(&bytes) != required(&manifest["assets"][name], "sha256")?
            || manifest["assets"][name]["bytes"].as_u64() != Some(bytes.len() as u64)
        {
            return Err(invalid());
        }
        assets.push(bytes);
    }
    let metadata: Value = serde_json::from_slice(&assets[0])?;
    if metadata["boundaryISO"] != iso || metadata["boundaryType"] != tier {
        return Err(invalid());
    }
    let shapes = validate_assets(&assets, cancel)?;
    if let Some(review) = reconciliation(&assets)? {
        manifest["reconciliation"] = review;
    }
    check(cancel)?;
    manifest["status"] = json!("candidate");
    manifest["features"] = json!(shapes.len());
    manifest["vertices"] = json!(shapes.iter().map(|s| s.vertices).sum::<usize>());
    fs::write(
        dest.join("manifest.json.tmp"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::rename(dest.join("manifest.json.tmp"), dest.join("manifest.json"))?;
    Ok(manifest)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edition_counts_and_shape_identity_cannot_drift() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../examples/processing-areas.json")).unwrap();
        let e = &fixture["editions"][0];
        let cancel = AtomicBool::new(false);
        assert!(validate(&e["metadata"], &e["full"], &e["display"], &cancel).is_ok());
        let mut metadata = e["metadata"].clone();
        metadata["admUnitCount"] = json!("2");
        assert!(validate(&metadata, &e["full"], &e["display"], &cancel).is_err());
        let mut display = e["display"].clone();
        display["features"][0]["properties"]["shapeID"] = json!("different");
        assert!(validate(&e["metadata"], &e["full"], &display, &cancel).is_err());
    }
    #[test]
    fn count_exception_is_not_a_general_validation_override() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../examples/processing-areas.json")).unwrap();
        let e = &fixture["editions"][0];
        let mut metadata = e["metadata"].clone();
        metadata["admUnitCount"] = json!("33");
        metadata["boundaryISO"] = json!("IRN");
        metadata["boundaryID"] = json!("IRN-ADM1-17685810");
        let assets = vec![
            serde_json::to_vec(&metadata).unwrap(),
            serde_json::to_vec(&e["full"]).unwrap(),
            serde_json::to_vec(&e["display"]).unwrap(),
        ];
        assert!(reconciliation(&assets).unwrap().is_none());
        assert!(validate_assets(&assets, &AtomicBool::new(false)).is_err());
        assert!(validate_assets(&[], &AtomicBool::new(false)).is_err());
    }
    #[test]
    fn asset_redirects_are_publisher_scoped() {
        for u in [
            "http://github.com/wmgeolab/geoBoundaries/raw/x",
            "https://github.com/other/raw/x",
            "https://127.0.0.1/x",
            "https://www.geoboundaries.org/api/private/",
            "https://github.com:8443/wmgeolab/geoBoundaries/raw/x",
        ] {
            assert!(!permitted(&url::Url::parse(u).unwrap()));
        }
        assert!(permitted(&url::Url::parse("https://raw.githubusercontent.com/wmgeolab/geoBoundaries/123/releaseData/gbOpen/LBN/ADM1/x.geojson").unwrap()));
    }
}
