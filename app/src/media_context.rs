//! Publication-pinned presentation relationships. Never changes evidence or location.
use crate::{error::Result, model::required};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

const WINDOW: i64 = 180;
const MAX_CONTEXT: usize = 400;
fn text(e: &Value) -> &str {
    e["text"].as_str().unwrap_or("").trim()
}
fn time(e: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(e["published_at"].as_str()?)
        .ok()
        .map(|t| t.timestamp())
}
fn visual(e: &Value) -> bool {
    e["media"].as_array().is_some_and(|ms| {
        ms.iter().any(|m| {
            matches!(m["kind"].as_str(), Some("photo" | "video" | "animated_gif"))
                || m["mime_type"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("image/") || s.starts_with("video/"))
        })
    })
}
fn caption(e: &Value) -> bool {
    text(e).chars().count() <= 160 && text(e).split_whitespace().count() <= 24
}
/// Inference needs more than a short caption. Short self-contained posts
/// ("IDF strikes in …", "Footage shows …") are separate messages even when
/// they follow seconds later, so only empty, link/handle/tag/emoji-only, or at
/// most three-word captions ("Additional footage") may attach without a reply.
fn continuation(e: &Value) -> bool {
    !substantive(e) || (text(e).chars().count() <= 40 && text(e).split_whitespace().count() <= 3)
}
fn substantive(e: &Value) -> bool {
    let s = text(e);
    s.chars().count() >= 12
        && s.split_whitespace().any(|w| {
            !w.starts_with("https://")
                && !w.starts_with("http://")
                && !w.starts_with('@')
                && !w.starts_with('#')
        })
}
fn album(e: &Value) -> Option<&str> {
    e["relationships"]["grouped_id"]
        .as_str()
        .filter(|s| !s.is_empty())
}
fn same_author(a: &Value, b: &Value) -> bool {
    let author = |e: &Value| {
        e["relationships"]["sender_id"]
            .as_str()
            .or(e["relationships"]["author"]["id"].as_str())
            .map(str::to_owned)
    };
    match (author(a), author(b)) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}
fn independent(e: &Value) -> bool {
    ["forward", "repost_of", "quote_of"]
        .iter()
        .all(|key| e["relationships"][key].is_null())
}
fn reply(e: &Value) -> Option<String> {
    let r = &e["relationships"];
    if let Some(id) = r["reply_to"].as_str() {
        return Some(format!("x:post:{id}"));
    }
    let id = r["reply"]["message_id"].as_str()?;
    Some(format!(
        "telegram:{}:{}:{id}",
        r["account_id"].as_str()?,
        r["reply"]["peer_id"].as_str()?
    ))
}
fn relation_roots(rows: &[Value]) -> Vec<(usize, &'static str)> {
    let mut roots: Vec<_> = (0..rows.len()).map(|i| (i, "original")).collect();
    // Native album membership takes precedence over temporal inference.
    for (i, e) in rows.iter().enumerate() {
        if let Some(group) = album(e) {
            let members: Vec<_> = rows
                .iter()
                .enumerate()
                .filter(|(_, r)| album(r) == Some(group) && same_author(e, r))
                .collect();
            let root = members
                .iter()
                .find(|(_, r)| !text(r).is_empty())
                .or(members.first())
                .map(|(n, _)| *n)
                .unwrap_or(i);
            if root != i {
                roots[i] = (root, "album");
            }
        }
    }
    for (i, e) in rows.iter().enumerate() {
        if roots[i].0 != i || !visual(e) || !caption(e) || !independent(e) {
            continue;
        }
        if let Some(target) = reply(e) {
            if let Some((j, _)) = rows
                .iter()
                .enumerate()
                .find(|(j, r)| *j < i && r["id"] == target && same_author(e, r))
            {
                // One explicit reply hop. Never infer a chain of event identity.
                roots[i] = (roots[j].0, "reply");
            }
            continue;
        }
        if !continuation(e) {
            continue;
        }
        let Some(at) = time(e) else {
            continue;
        };
        for j in (0..i).rev() {
            let p = &rows[j];
            if roots[j].0 != j || !substantive(p) {
                continue;
            }
            let Some(before) = time(p) else {
                continue;
            };
            if at - before > WINDOW {
                break;
            }
            if at >= before && same_author(e, p) && independent(p) {
                roots[i] = (j, "nearby");
            }
            // A new substantive post, even by another author, is a boundary.
            break;
        }
    }
    // If a captioned album itself follows a text post, move all its media together.
    for i in 0..rows.len() {
        if roots[i].1 == "album" {
            let parent = roots[i].0;
            if roots[parent].0 != parent {
                roots[i] = roots[parent];
            }
        }
    }
    roots
}

pub(crate) fn attach(c: &Connection, publication: &str, e: &mut Value) -> Result<()> {
    e["media_context"] = json!({"members":[],"anchor":null,"limited":false});
    let id = required(e, "id")?;
    if !(id.starts_with("telegram:") || id.starts_with("x:post:"))
        || e["upstream_state"] == "removed"
    {
        return Ok(());
    }
    let source = required(e, "source")?;
    let native = &e["relationships"];
    let target = reply(e).unwrap_or_default();
    let mut stmt = c.prepare("SELECT e.json FROM evidence e JOIN membership m ON m.evidence_rowid=e.rowid WHERE m.publication_id=?1 AND e.source=?2 AND (e.id=?3 OR e.id=?4 OR abs(unixepoch(e.published_at)-unixepoch(?5))<=360 OR (?6!='' AND json_extract(e.json,'$.relationships.grouped_id')=?6) OR (?7!='' AND json_extract(e.json,'$.relationships.reply.message_id')=?7 AND json_extract(e.json,'$.relationships.reply.peer_id')=?8) OR (?9!='' AND json_extract(e.json,'$.relationships.reply_to')=?9)) ORDER BY e.published_at,length(e.id),e.id LIMIT 401")?;
    let raw: Vec<String> = stmt
        .query_map(
            params![
                publication,
                source,
                id,
                target,
                e["published_at"].as_str(),
                album(e).unwrap_or(""),
                native["message_id"].as_str().unwrap_or(""),
                native["peer_id"].as_str().unwrap_or(""),
                id.strip_prefix("x:post:").unwrap_or("")
            ],
            |r| r.get(0),
        )?
        .collect::<std::result::Result<_, _>>()?;
    if raw.len() > MAX_CONTEXT {
        e["media_context"]["limited"] = json!(true);
        return Ok(());
    }
    let rows: Vec<Value> = raw
        .into_iter()
        .map(|r| serde_json::from_str(&r))
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|r: &Value| r["upstream_state"] != "removed")
        .collect();
    // An old exact revision outside this publication must never gain current media.
    let Some(selected) = rows
        .iter()
        .position(|r| r["id"] == e["id"] && r["revision"] == e["revision"])
    else {
        return Ok(());
    };
    let roots = relation_roots(&rows);
    let root = roots[selected].0;
    let member = |i: usize| json!({"id":rows[i]["id"],"revision":rows[i]["revision"],"published_at":rows[i]["published_at"],"reason":roots[i].1,"media":rows[i]["media"].as_array().cloned().unwrap_or_default()});
    let mut members: Vec<_> = (0..rows.len())
        .filter(|i| {
            *i != selected
                && (roots[*i].0 == root || *i == root)
                && rows[*i]["media"].as_array().is_some_and(|a| !a.is_empty())
        })
        .map(member)
        .collect();
    let limited = members.len() > 50;
    members.truncate(50);
    e["media_context"] = json!({"members":members,"anchor":if root != selected { Some(json!({"id":rows[root]["id"],"revision":rows[root]["revision"],"reason":roots[selected].1})) }else{None},"limited":limited});
    Ok(())
}
