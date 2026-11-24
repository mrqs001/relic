//! Explicit official regional download; failed staging never becomes active.
use crate::{
    error::{Error, Result},
    model::now,
    osm,
};
use futures_util::StreamExt;
use md5::Md5;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncWriteExt;
const INDEX: &str = "https://download.geofabrik.de/index-v1.json";
fn invalid() -> Error {
    Error::invalid("Geofabrik acquisition failed validation; the active reference is unchanged")
}
fn official(url: &str) -> Result<url::Url> {
    let u = url::Url::parse(url).map_err(|_| invalid())?;
    if u.scheme() != "https"
        || u.host_str() != Some("download.geofabrik.de")
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(u)
}
async fn fetch(
    client: &wreq::Client,
    url: &str,
    path: &Path,
    bound: u64,
    cancel: &AtomicBool,
) -> Result<Value> {
    let mut current = official(url)?;
    let mut redirects = vec![];
    let operation = async {
        let response = loop {
            let r = client
                .get(current.as_str())
                .send()
                .await
                .map_err(|_| invalid())?;
            if r.status().is_redirection() {
                if redirects.len() >= 5 {
                    return Err(invalid());
                }
                let next = r
                    .headers()
                    .get(wreq::header::LOCATION)
                    .and_then(|h| h.to_str().ok())
                    .ok_or_else(invalid)?;
                let next = official(current.join(next).map_err(|_| invalid())?.as_str())?;
                redirects.push(current.to_string());
                current = next;
                continue;
            }
            break r;
        };
        if response.status().as_u16() != 200 || response.content_length().is_some_and(|n| n > bound)
        {
            return Err(invalid());
        }
        let length = response.content_length();
        // The extract itself plus its on-disk build staging.
        if let (Some(n), Some(dir)) = (length, path.parent()) {
            crate::disk::ensure(dir, n.saturating_mul(2), "Downloading and building this OSM extract")?;
        }
        let modified = response
            .headers()
            .get(wreq::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut stream = response.bytes_stream();
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await?;
        let mut total = 0_u64;
        let mut sha = Sha256::new();
        let mut md5 = Md5::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| invalid())?;
            total += chunk.len() as u64;
            if total > bound {
                return Err(invalid());
            }
            file.write_all(&chunk).await?;
            sha.update(&chunk);
            md5.update(&chunk);
        }
        if length.is_some_and(|n| n != total) {
            return Err(invalid());
        }
        file.sync_all().await?;
        Ok(
            json!({"requested_url":url,"resolved_url":current.as_str(),"redirects":redirects,"acquired_at":now(),"publisher_last_modified":modified,"bytes":total,"sha256":format!("{:x}",sha.finalize()),"md5":format!("{:x}",md5.finalize())}),
        )
    };
    tokio::select! {r=operation=>r,_=async {while !cancel.load(Ordering::Acquire){tokio::time::sleep(Duration::from_millis(50)).await;}}=>Err(Error::cancelled())}
}
pub async fn acquire(region: &str, destination: &Path, cancel: Arc<AtomicBool>) -> Result<Value> {
    if region.is_empty()
        || region.len() > 100
        || !region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'-' || b.is_ascii_digit())
    {
        return Err(invalid());
    }
    if destination.exists() {
        return Err(Error::conflict(
            "Geofabrik candidate already exists; use a new directory",
        ));
    }
    if cancel.load(Ordering::Acquire) {
        return Err(Error::cancelled());
    }
    let parent = destination.parent().ok_or_else(invalid)?;
    tokio::fs::create_dir_all(parent).await?;
    let staging = tempfile::Builder::new()
        .prefix("osm-download-")
        .tempdir_in(parent)?;
    let client = wreq::Client::builder()
        .no_proxy()
        .redirect(wreq::redirect::Policy::none())
        .retry(wreq::retry::Policy::never())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(1200))
        .build()
        .map_err(|_| invalid())?;
    let index_receipt = fetch(
        &client,
        INDEX,
        &staging.path().join("index.json"),
        32 * 1024 * 1024,
        &cancel,
    )
    .await?;
    let index: Value =
        serde_json::from_slice(&tokio::fs::read(staging.path().join("index.json")).await?)?;
    if index["type"] != "FeatureCollection" {
        return Err(invalid());
    }
    let choices = index["features"]
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .filter(|f| f["properties"]["id"] == region)
        .collect::<Vec<_>>();
    if choices.len() != 1 {
        return Err(invalid());
    }
    let selected = choices[0].clone();
    let url = selected["properties"]["urls"]["pbf"]
        .as_str()
        .ok_or_else(invalid)?;
    if !url.ends_with(".osm.pbf") || selected["geometry"].is_null() {
        return Err(invalid());
    }
    // No extract size cap; free disk space is checked once the size is known.
    let pbf_receipt = fetch(
        &client,
        url,
        &staging.path().join("source.osm.pbf"),
        u64::MAX,
        &cancel,
    )
    .await?;
    // The checksum follows the resolved asset. A mutable release race fails checksum validation.
    let checksum_url = format!(
        "{}.md5",
        pbf_receipt["resolved_url"].as_str().ok_or_else(invalid)?
    );
    let checksum_receipt = fetch(
        &client,
        &checksum_url,
        &staging.path().join("source.md5"),
        4096,
        &cancel,
    )
    .await?;
    let checksum = tokio::fs::read_to_string(staging.path().join("source.md5")).await?;
    let expected = checksum.split_whitespace().next().ok_or_else(invalid)?;
    if expected.len() != 32
        || !expected.bytes().all(|b| b.is_ascii_hexdigit())
        || !expected.eq_ignore_ascii_case(pbf_receipt["md5"].as_str().ok_or_else(invalid)?)
    {
        return Err(invalid());
    }
    let selection = json!({"feature":selected,"index":index_receipt,"pbf":pbf_receipt,"checksum":checksum_receipt,"publisher_checksum_verified":true,"social_data_sent":false});
    let destination = destination.to_owned();
    // Await blocking work even when cancelled, so staging cannot disappear under it.
    tokio::task::spawn_blocking(move || {
        let candidate = staging.path().join("candidate");
        let mut report = osm::build(
            &staging.path().join("source.osm.pbf"),
            &candidate,
            selection,
            &cancel,
        )?;
        for name in ["source.osm.pbf", "index.json", "source.md5"] {
            std::fs::rename(staging.path().join(name), candidate.join(name))?;
        }
        report["acquired_at"] = json!(now());
        std::fs::write(
            candidate.join("manifest.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        std::fs::File::open(candidate.join("manifest.json"))?.sync_all()?;
        std::fs::File::open(&candidate)?.sync_all()?;
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        std::fs::rename(&candidate, &destination)?;
        std::fs::File::open(destination.parent().ok_or_else(invalid)?)?.sync_all()?;
        Ok(report)
    })
    .await
    .map_err(|_| Error::internal())?
}
