//! Bounded official GeoNames archive acquisition. No social text is sent here.
use crate::{
    error::{Error, Result},
    geonames,
    media_store::file_hash,
    model::now,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncWriteExt;

fn invalid() -> Error {
    Error::invalid(
        "Reference archive is unavailable, malformed or exceeds its explicit bound; active geography is unchanged",
    )
}
async fn download(
    client: &wreq::Client,
    url: &str,
    path: &Path,
    cancel: &AtomicBool,
) -> Result<Value> {
    let operation = async {
        let response = client.get(url).send().await.map_err(|_| invalid())?;
        if !response.status().is_success() {
            return Err(invalid());
        }
        // No archive size cap; the archive and its extraction must fit on disk.
        if let (Some(n), Some(dir)) = (response.content_length(), path.parent()) {
            crate::disk::ensure(dir, n.saturating_mul(8), "Downloading this GeoNames archive")?;
        }
        let modified = response
            .headers()
            .get(wreq::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut stream = response.bytes_stream();
        let mut output = tokio::fs::File::create(path).await?;
        let mut total = 0;
        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let chunk = chunk.map_err(|_| invalid())?;
            total += chunk.len() as u64;
            output.write_all(&chunk).await?;
        }
        output.sync_all().await?;
        Ok(json!({"url":url,"acquired_at":now(),"publisher_last_modified":modified,"bytes":total}))
    };
    tokio::select! {
        result = operation => result,
        _ = async { loop { if cancel.load(Ordering::Acquire) { break; } tokio::time::sleep(Duration::from_millis(50)).await; } } => Err(Error::cancelled())
    }
}
pub(crate) fn extract(
    archive: &Path,
    member: &str,
    output: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?).map_err(|_| invalid())?;
    if zip.len() > 8 {
        return Err(invalid());
    }
    let mut input = zip.by_name(member).map_err(|_| invalid())?;
    if input.is_dir() || input.enclosed_name().is_none() {
        return Err(invalid());
    }
    crate::disk::ensure(output.parent().unwrap_or(Path::new(".")), input.size(), "Extracting this GeoNames file")?;
    let mut dest = fs::File::create(output)?;
    let mut total = 0;
    let mut buf = [0; 64 * 1024];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        let n = input.read(&mut buf).map_err(|_| invalid())?;
        if n == 0 {
            break;
        }
        total += n as u64;
        // Never write past the declared size (a zip bomb lies about it).
        if total > input.size() {
            return Err(invalid());
        }
        dest.write_all(&buf[..n])?;
    }
    if total != input.size() {
        return Err(invalid());
    }
    dest.sync_all()?;
    Ok(())
}

/// One explicitly selected country or global cities500 edition, always NEW.
pub async fn acquire(product: &str, destination: &Path, cancel: Arc<AtomicBool>) -> Result<Value> {
    if product != "cities500"
        && (product.len() != 2 || !product.bytes().all(|b| b.is_ascii_uppercase()))
    {
        return Err(invalid());
    }
    if destination.exists() {
        return Err(Error::conflict("Reference edition already exists"));
    }
    let parent = destination.parent().ok_or_else(invalid)?;
    fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix("reference-download-")
        .tempdir_in(parent)?;
    let client = wreq::Client::builder()
        .redirect(wreq::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| invalid())?;
    let mut receipts = vec![];
    let mut retained = vec!["places.zip", "readme.txt"];
    for (url, file) in [
        (
            format!("https://download.geonames.org/export/dump/{product}.zip"),
            "places.zip",
        ),
        (
            "https://download.geonames.org/export/dump/readme.txt".into(),
            "readme.txt",
        ),
    ] {
        receipts.push(download(&client, &url, &staging.path().join(file), &cancel).await?);
    }
    // Language-tagged alternate names: the country's own file, or for the
    // global city list the worldwide file, joined to its places at build.
    let aliases_url = if product == "cities500" {
        "https://download.geonames.org/export/dump/alternateNamesV2.zip".to_owned()
    } else {
        format!("https://download.geonames.org/export/dump/alternatenames/{product}.zip")
    };
    receipts.push(download(&client, &aliases_url, &staging.path().join("aliases.zip"), &cancel).await?);
    retained.push("aliases.zip");
    let product = product.to_owned();
    let destination = destination.to_owned();
    tokio::task::spawn_blocking(move || {
        extract(&staging.path().join("places.zip"),&format!("{product}.txt"),&staging.path().join("places.txt"),&cancel)?;
        let member = if product=="cities500" { "alternateNamesV2.txt".to_owned() } else { format!("{product}.txt") };
        extract(&staging.path().join("aliases.zip"),&member,&staging.path().join("aliases.txt"),&cancel)?;
        let candidate=staging.path().join("candidate");
        let mut manifest=geonames::build(&candidate,&staging.path().join("places.txt"),&staging.path().join("aliases.txt"),json!({"product":product,"requests":receipts,"method":"official bounded HTTPS archives, exact named ZIP member, CRC validation","social_data_sent":false}),&cancel)?;
        for name in retained {
            fs::copy(staging.path().join(name),candidate.join(name))?;
            fs::File::open(candidate.join(name))?.sync_all()?;
            let (sha,bytes)=file_hash(&candidate.join(name))?;
            manifest["assets"][name]=json!({"sha256":sha,"bytes":bytes});
        }
        fs::write(candidate.join("manifest.json"),serde_json::to_vec_pretty(&manifest)?)?;
        fs::File::open(candidate.join("manifest.json"))?.sync_all()?;
        fs::File::open(&candidate)?.sync_all()?;
        if cancel.load(Ordering::Acquire) { return Err(Error::cancelled()); }
        fs::rename(&candidate,&destination)?;
        fs::File::open(destination.parent().ok_or_else(invalid)?)?.sync_all()?;
        Ok(manifest)
    }).await.map_err(|_|Error::internal())?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn archive(path: &Path, names: &[&str]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for name in names {
            zip.start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
            zip.write_all(b"fictional-place-row").unwrap();
        }
        zip.finish().unwrap();
    }
    #[test]
    fn archives_require_expected_safe_member_valid_crc_and_bounded_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("input.zip");
        let out = tmp.path().join("output.txt");
        archive(&path, &["YE.txt"]);
        extract(&path, "YE.txt", &out, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"fictional-place-row");
        assert!(extract(&path, "IR.txt", &out, &AtomicBool::new(false)).is_err());
        assert!(extract(&path, "YE.txt", &out, &AtomicBool::new(true)).is_err());
        let mut bytes = fs::read(&path).unwrap();
        let at = bytes
            .windows(b"fictional-place-row".len())
            .position(|v| v == b"fictional-place-row")
            .unwrap();
        bytes[at] ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(extract(&path, "YE.txt", &out, &AtomicBool::new(false)).is_err());
        archive(&path, &["../YE.txt"]);
        assert!(extract(&path, "../YE.txt", &out, &AtomicBool::new(false)).is_err());
        archive(&path, &["YE.txt", "a", "b", "c", "d", "e", "f", "g", "h"]);
        assert!(extract(&path, "YE.txt", &out, &AtomicBool::new(false)).is_err());
    }
}
