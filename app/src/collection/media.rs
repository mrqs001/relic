//! Only exact retained media references can initiate provider requests.
use super::*;

impl Runtime {
    pub async fn load_media(
        &self,
        id: String,
        revision: String,
        asset_index: usize,
    ) -> Result<Value> {
        let (progress, _) = tokio::sync::watch::channel(Value::Null);
        self.load_media_with_progress(
            id,
            revision,
            asset_index,
            progress,
            self.store.shutdown.clone(),
        )
        .await
    }

    pub async fn load_media_with_progress(
        &self,
        id: String,
        revision: String,
        asset_index: usize,
        progress: tokio::sync::watch::Sender<Value>,
        cancel: Arc<AtomicBool>,
    ) -> Result<Value> {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        let mut resolved_revision = revision.clone();
        let _media = self.media.try_acquire().map_err(|_| Error::busy())?;
        let read_id = id.clone();
        let read_revision = revision.clone();
        let retained = self
            .block(move |s| s.retained_media(&read_id, &read_revision, asset_index))
            .await?;
        let metadata = if let Some(metadata) = retained {
            metadata
        } else {
            let read_id = id.clone();
            let read_revision = revision.clone();
            let evidence = self
                .block(move |s| Store::evidence(&*s.connect()?, &read_id, &read_revision))
                .await?;
            let provider = if id.starts_with("x:post:") {
                "x"
            } else if id.starts_with("telegram:") {
                "telegram"
            } else {
                return Err(Error::invalid(
                    "This evidence has no supported source media adapter",
                ));
            };
            if !self.connected(provider) {
                return Err(Error::conflict(
                    "Connect the source account before loading this media",
                ));
            }
            if !self.retry_gate(provider).await? {
                return Err(self.retry_error(provider)?);
            }
            let _operation = self.operations[index(provider)?]
                .try_acquire()
                .map_err(|_| Error::busy())?;
            let epoch = self.epoch(provider).load(Ordering::Acquire);
            let request = async {
                if provider == "x" {
                    let asset = &evidence["media"][asset_index];
                    let url = if asset["kind"] == "photo" {
                        asset["preview_url"].as_str()
                    } else {
                        asset["variants"]
                            .as_array()
                            .and_then(|variants| {
                                variants
                                    .iter()
                                    .filter(|v| {
                                        v["content_type"] == "video/mp4" && v["url"].is_string()
                                    })
                                    .min_by_key(|v| v["bitrate"].as_u64().unwrap_or(u64::MAX))
                            })
                            .and_then(|v| v["url"].as_str())
                    }
                    .ok_or_else(|| {
                        Error::invalid(
                            "This source media has no supported bounded image or MP4 reference",
                        )
                    })?;
                    let manager = self
                        .x
                        .as_ref()
                        .ok_or_else(|| Error::invalid("X is not configured"))?;
                    progress.send_replace(crate::media_stream::progress("downloading", 0, None));
                    let response = match manager.fetch_asset(url, MAX_MEDIA_BYTES).await {
                        Ok(response) => response,
                        Err(error) => {
                            if error.kind == crate::sources::FailureKind::RateLimited {
                                let until = later(900);
                                self.persist_retry("x", until.clone()).await?;
                                self.set_connection("x","connected","X rate limited this account; collection and media wait before retrying.",Some("Configured X account".into()),Some(until))?;
                            }
                            return Err(x::x_error(error));
                        }
                    };
                    let mime = response
                        .content_type
                        .split(';')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .to_ascii_lowercase();
                    let file = self.store.media_staging()?;
                    let mut output = tokio::fs::File::from_std(file.reopen()?);
                    tokio::io::AsyncWriteExt::write_all(&mut output, &response.body).await?;
                    tokio::io::AsyncWriteExt::flush(&mut output).await?;
                    progress.send_replace(crate::media_stream::progress(
                        "downloading",
                        response.body.len(),
                        Some(response.body.len()),
                    ));
                    Ok((file, mime, None))
                } else {
                    let mut guard = self.telegram.lock().await;
                    let client = guard
                        .as_mut()
                        .ok_or_else(|| Error::conflict("Telegram is disconnected"))?;
                    if client.status().account_id.as_deref()
                        != evidence["relationships"]["account_id"].as_str()
                    {
                        return Err(Error::conflict(
                            "This media belongs to another Telegram account",
                        ));
                    }
                    let peer = evidence["relationships"]["peer_id"]
                        .as_str()
                        .ok_or_else(|| Error::invalid("Media peer identity is unavailable"))?;
                    let message_id = evidence["relationships"]["message_id"]
                        .as_str()
                        .and_then(|v| v.parse::<i32>().ok())
                        .ok_or_else(|| Error::invalid("Media message identity is unavailable"))?;
                    let file = self.store.media_staging()?;
                    let mut output = tokio::fs::File::from_std(file.reopen()?);
                    let media = match client
                        .media_to_with_progress(
                            peer,
                            message_id,
                            crate::media_store::MAX_MEDIA_FILE_BYTES,
                            &mut output,
                            |bytes, total| {
                                progress.send_replace(crate::media_stream::progress(
                                    "downloading",
                                    bytes,
                                    total,
                                ));
                            },
                        )
                        .await
                    {
                        Ok(media) => media,
                        Err(error) => {
                            if error == relic_telegram::Error::MediaTooLarge {
                                return Err(Error::invalid(
                                    "This Telegram file exceeds Relic's 512 MiB download limit",
                                ));
                            }
                            if telegram::reset_connection(&error) {
                                client.disconnect();
                            }
                            self.telegram_failure(&error).await?;
                            return Err(telegram::telegram_error(error));
                        }
                    };
                    let mime = media.mime_type.unwrap_or_else(|| {
                        detected_media_type(&media.prefix)
                            .unwrap_or_default()
                            .into()
                    });
                    Ok((file, mime, Some(media.origin)))
                }
            };
            tokio::pin!(request);
            let (file, mime, origin) = loop {
                tokio::select! {
                    result=&mut request=>break result?,
                    _=tokio::time::sleep(Duration::from_millis(50))=>{
                        if cancel.load(Ordering::Acquire)||self.epoch(provider).load(Ordering::Acquire)!=epoch||self.store.shutdown.load(Ordering::Acquire){return Err(Error::cancelled());}
                    }
                }
            };
            if cancel.load(Ordering::Acquire)
                || self.store.shutdown.load(Ordering::Acquire)
                || self.epoch(provider).load(Ordering::Acquire) != epoch
            {
                return Err(Error::cancelled());
            }
            let write_id = id.clone();
            let write_revision = revision.clone();
            let size = file.as_file().metadata()?.len() as usize;
            progress.send_replace(crate::media_stream::progress("verifying", size, Some(size)));
            if let Some(origin) = origin {
                let current = telegram::normalized_message(&origin);
                let evidence = evidence.clone();
                let (metadata, revision) = self
                    .block(move |s| {
                        s.reconcile_telegram_media_file_cancellable(
                            &evidence,
                            asset_index,
                            current,
                            origin.raw,
                            file,
                            &mime,
                            cancel,
                        )
                    })
                    .await?;
                resolved_revision = revision;
                metadata
            } else {
                self.block(move |s| {
                    s.retain_media_file_cancellable(
                        &write_id,
                        &write_revision,
                        asset_index,
                        file,
                        &mime,
                        &cancel,
                    )
                })
                .await?
            }
        };
        // Only a Relic route is returned, never an upstream URL.
        let encode = |value: &str| {
            value
                .bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                        (b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect::<String>()
        };
        Ok(
            json!({"url":format!("/api/v1/media?id={}&revision={}&index={asset_index}",encode(&id),encode(&resolved_revision)),"revision":resolved_revision,"media_type":metadata["media_type"],"bytes":metadata["bytes"]}),
        )
    }
}

// Telegram photos may omit a MIME label. Infer only the supported binary
// signatures; the store then checks the complete bounded header and type.
fn detected_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.get(4..8) == Some(b"ftyp") {
        Some("video/mp4")
    } else {
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unlabeled_telegram_photo_needs_a_supported_signature() {
        assert_eq!(
            detected_media_type(&[0xff, 0xd8, 0xff, 0xe0]),
            Some("image/jpeg")
        );
        assert_eq!(detected_media_type(b"<svg></svg>"), None);
        assert_eq!(detected_media_type(b"<!doctype html>"), None);
        assert_eq!(detected_media_type(b""), None);
    }
}
