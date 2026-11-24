use crate::{Counters, Error, Forward, MediaRef, Message, MessageRef, Point};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn body(value: &Value) -> &Value {
    if let Some(object) = value.as_object()
        && object.len() == 1
        && let Some(inner) = object.values().next().filter(|inner| inner.is_object())
    {
        return inner;
    }
    value
}
fn number(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}
fn id(value: &Value) -> Option<String> {
    number(value).map(|n| n.to_string())
}
fn time(value: &Value) -> Result<String, Error> {
    DateTime::from_timestamp(number(value).ok_or(Error::Upstream)?, 0)
        .map(|v| v.to_rfc3339_opts(SecondsFormat::Secs, true))
        .ok_or(Error::Upstream)
}
fn peer_id(value: &Value) -> Option<String> {
    let peer = body(value);
    if let Some(value) = number(&peer["channel_id"]).filter(|v| *v > 0) {
        (-1_000_000_000_000i64)
            .checked_sub(value)
            .map(|v| v.to_string())
    } else if let Some(value) = number(&peer["chat_id"]).filter(|v| *v > 0) {
        value.checked_neg().map(|v| v.to_string())
    } else {
        number(&peer["user_id"])
            .filter(|v| *v > 0)
            .map(|v| v.to_string())
    }
}

fn clean_private(value: &mut Value, shared_contact: bool) {
    match value {
        Value::Object(object) => {
            for key in [
                "access_hash",
                "file_reference",
                "auth_key",
                "phone_code_hash",
                "phone_number",
                "phone",
                "api_hash",
            ] {
                if !shared_contact || !matches!(key, "phone" | "phone_number") {
                    object.remove(key);
                }
            }
            for (key, value) in object.iter_mut() {
                clean_private(value, key == "Contact");
            }
        }
        Value::Array(values) => {
            for value in values {
                clean_private(value, false);
            }
        }
        _ => {}
    }
}

fn media(value: &Value) -> Option<MediaRef> {
    if value.is_null() {
        return None;
    }
    let kind = value.as_object()?.keys().next()?.to_lowercase();
    if kind == "empty" {
        return None;
    }
    let contents = body(value);
    let native = if kind == "photo" {
        body(&contents["photo"])
    } else if kind == "document" {
        body(&contents["document"])
    } else {
        contents
    };
    Some(MediaRef {
        kind,
        id: id(&native["id"]),
        mime_type: native["mime_type"].as_str().map(str::to_owned),
        size: number(&native["size"]).and_then(|size| u64::try_from(size).ok()),
        archive_status: "not_requested".into(),
    })
}

/// Normalize a serialized grammers TL message. This is also the fixture/protocol boundary.
/// Retains its content representation but removes authorization and expiring file references.
pub fn normalize_raw(account_id: &str, raw: &Value, acquired_at: &str) -> Result<Message, Error> {
    if account_id.parse::<i64>().ok().filter(|v| *v > 0).is_none() {
        return Err(Error::AuthenticationRequired);
    }
    if DateTime::parse_from_rfc3339(acquired_at).is_err() {
        return Err(Error::Upstream);
    }
    let native = body(raw);
    let message_id = number(&native["id"])
        .filter(|v| *v > 0 && *v <= i32::MAX as i64)
        .ok_or(Error::Upstream)?
        .to_string();
    let peer_id = peer_id(&native["peer_id"]).ok_or(Error::Upstream)?;
    let reply_native = body(&native["reply_to"]);
    let reply = id(&reply_native["reply_to_msg_id"]).map(|message_id| MessageRef {
        peer_id: self::peer_id(&reply_native["reply_to_peer_id"])
            .unwrap_or_else(|| peer_id.clone()),
        message_id,
    });
    let forward = if native["fwd_from"].is_null() {
        None
    } else {
        let value = body(&native["fwd_from"]);
        Some(Forward {
            peer_id: self::peer_id(&value["from_id"]),
            message_id: id(&value["channel_post"]),
            date: time(&value["date"])?,
            author: value["from_name"]
                .as_str()
                .or_else(|| value["post_author"].as_str())
                .map(str::to_owned),
            saved_from: self::peer_id(&value["saved_from_peer"])
                .zip(id(&value["saved_from_msg_id"]))
                .map(|(peer_id, message_id)| MessageRef {
                    peer_id,
                    message_id,
                }),
        })
    };
    let geo = body(&body(&native["media"])["geo"]);
    let point = geo["long"]
        .as_f64()
        .zip(geo["lat"].as_f64())
        .filter(|(longitude, latitude)| {
            longitude.is_finite()
                && latitude.is_finite()
                && (-180.0..=180.0).contains(longitude)
                && (-90.0..=90.0).contains(latitude)
        })
        .map(|(longitude, latitude)| Point {
            longitude,
            latitude,
        });
    let mut retained = raw.clone();
    clean_private(&mut retained, false);
    let mut content = retained.clone();
    if let Some(native) = content
        .as_object_mut()
        .and_then(|object| object.values_mut().next())
        .and_then(Value::as_object_mut)
    {
        for key in [
            "views",
            "forwards",
            "replies",
            "reactions",
            "edit_hide",
            "pinned",
        ] {
            native.remove(key);
        }
    }
    let content_fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&content).map_err(|_| Error::Upstream)?)
    );
    Ok(Message {
        account_id: account_id.to_owned(),
        peer_id,
        message_id,
        text: native["message"].as_str().unwrap_or("").to_owned(),
        published_at: time(&native["date"])?,
        edited_at: if native["edit_date"].is_null() {
            None
        } else {
            Some(time(&native["edit_date"])?)
        },
        acquired_at: acquired_at.to_owned(),
        sender_id: self::peer_id(&native["from_id"]),
        reply,
        forward,
        media: media(&native["media"]),
        grouped_id: id(&native["grouped_id"]),
        point,
        counters: Counters {
            views: number(&native["views"]).and_then(|n| n.try_into().ok()),
            forwards: number(&native["forwards"]).and_then(|n| n.try_into().ok()),
            replies: number(&body(&native["replies"])["replies"]).and_then(|n| n.try_into().ok()),
        },
        content_fingerprint,
        raw: retained,
    })
}

pub fn normalize_message(
    account_id: &str,
    message: &grammers_client::message::Message,
) -> Result<Message, Error> {
    normalize_raw(
        account_id,
        &serde_json::to_value(&message.raw).map_err(|_| Error::Upstream)?,
        &Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> Value {
        json!({"Message": {"id":42,"peer_id":{"Channel":{"channel_id":90000001}},"from_id":{"User":{"user_id":99}},"date":1770000000,"message":"Fictional report","views":1,"forwards":2,"reply_to":{"Header":{"reply_to_msg_id":40}},"fwd_from":{"Header":{"from_id":{"Channel":{"channel_id":80000001}},"channel_post":10,"date":1769990000}},"media":{"Document":{"document":{"Document":{"id":1234,"access_hash":999,"file_reference":[1,2],"mime_type":"image/png","size":1000}}}}}})
    }
    #[test]
    fn identity_relationships_and_private_fields() {
        let normalized = normalize_raw("100", &fixture(), "2026-09-10T00:00:00Z").unwrap();
        assert_eq!(normalized.peer_id, "-1000090000001");
        assert_eq!(normalized.message_id, "42");
        assert_eq!(normalized.reply.unwrap().message_id, "40");
        assert_eq!(
            normalized.forward.unwrap().message_id.as_deref(),
            Some("10")
        );
        assert_eq!(normalized.media.unwrap().id.as_deref(), Some("1234"));
        let raw = normalized.raw.to_string();
        assert!(!raw.contains("access_hash"));
        assert!(!raw.contains("file_reference"));
        assert!(normalized.point.is_none());
    }
    #[test]
    fn counters_and_file_reference_refresh_do_not_change_content_revision() {
        let original = fixture();
        let mut changed = original.clone();
        changed["Message"]["views"] = json!(10000);
        changed["Message"]["forwards"] = json!(400);
        changed["Message"]["media"]["Document"]["document"]["Document"]["file_reference"] =
            json!([8, 9]);
        let first = normalize_raw("100", &original, "2026-09-10T00:00:00Z").unwrap();
        let second = normalize_raw("100", &changed, "2026-09-10T00:00:01Z").unwrap();
        assert_eq!(first.content_fingerprint, second.content_fingerprint);
        assert_eq!(second.counters.views, Some(10000));
        changed["Message"]["message"] = json!("Edited fictional report");
        assert_ne!(
            first.content_fingerprint,
            normalize_raw("100", &changed, "2026-09-10T00:00:00Z")
                .unwrap()
                .content_fingerprint
        );
    }
    #[test]
    fn invalid_identity_is_rejected_and_media_only_text_stays_empty() {
        let mut fixture = fixture();
        fixture["Message"]["id"] = json!(0);
        assert!(normalize_raw("100", &fixture, "2026-09-10T00:00:00Z").is_err());
        fixture["Message"]["id"] = json!(42);
        fixture["Message"]["message"] = json!("");
        assert!(
            normalize_raw("100", &fixture, "2026-09-10T00:00:00Z")
                .unwrap()
                .text
                .is_empty()
        );
    }

    #[test]
    fn actual_grammers_tl_message_serialization_matches_normalization_contract() {
        use grammers_client::{
            Client,
            message::{InputMessage, Message as NativeMessage},
        };
        use grammers_session::{storages::MemorySession, types::PeerId};
        let session = std::sync::Arc::new(MemorySession::default());
        let pool = grammers_mtsender::SenderPool::new(session, 1);
        let client = Client::new(pool.handle);
        let message = NativeMessage::from_raw_short_updates(
            &client,
            grammers_tl_types::types::UpdateShortSentMessage {
                out: false,
                id: 42,
                pts: 2,
                pts_count: 1,
                date: 1770000000,
                media: None,
                entities: None,
                ttl_period: None,
            },
            InputMessage::new().text("Fictional typed message"),
            PeerId::channel(90000001).unwrap().to_ambient_ref(),
        );
        let normalized = normalize_message("100", &message).unwrap();
        assert_eq!(normalized.peer_id, "-1000090000001");
        assert_eq!(normalized.text, "Fictional typed message");
        assert_eq!(normalized.message_id, "42");
    }
}

#[cfg(test)]
mod contact_tests {
    use super::*;
    #[test]
    fn shared_contact_phone_is_evidence_but_account_fields_are_not() {
        let mut raw = serde_json::json!({"phone_number":"private-account","auth_key":"secret","media":{"Contact":{"phone_number":"+0000000000","first_name":"Fictional","access_hash":"secret"}}});
        clean_private(&mut raw, false);
        assert!(raw.get("phone_number").is_none());
        assert!(raw.get("auth_key").is_none());
        assert_eq!(raw["media"]["Contact"]["phone_number"], "+0000000000");
        assert!(raw["media"]["Contact"].get("access_hash").is_none());
    }
}
