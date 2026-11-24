//! One message's recorded path: source, exact revision chain, readings, and the
//! runs, references, publications and advisory records attached to them.
//! Shared runs, publications and references appear as this message's steps;
//! their other outputs are never expanded, so the result stays bounded.
use crate::{
    error::{Error, Result},
    store::Store,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const MAX_NODES: usize = 160;
const ADJACENT_EDGES: usize = 200;
const MAX_VERSIONS: usize = 50;
const MAX_READINGS: usize = 40;

struct Edge {
    id: String,
    source: String,
    target: String,
    relationship: String,
    detail: String,
}
impl Edge {
    fn other(&self, id: &str) -> &str {
        if self.source == id { &self.target } else { &self.source }
    }
}

#[derive(Default)]
struct Graph {
    nodes: BTreeMap<String, Value>,
    edges: BTreeMap<String, Edge>,
    hidden: BTreeSet<String>,
}
impl Graph {
    /// Adds a node unless the bound is reached; returns whether it is present.
    fn add(&mut self, c: &Connection, id: &str) -> Result<bool> {
        if self.nodes.contains_key(id) {
            return Ok(true);
        }
        if self.nodes.len() >= MAX_NODES {
            self.hidden.insert(id.to_owned());
            return Ok(false);
        }
        let row = c
            .query_row(
                "SELECT kind,label,detail FROM lineage_nodes WHERE id=?1",
                [id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
            )
            .optional()?;
        let Some((kind, label, detail)) = row else {
            return Ok(false);
        };
        self.nodes.insert(
            id.to_owned(),
            json!({"id":id,"kind":kind,"label":label,"detail":serde_json::from_str::<Value>(&detail)?}),
        );
        Ok(true)
    }
    fn kind(&self, id: &str) -> Option<&str> {
        self.nodes.get(id).and_then(|n| n["kind"].as_str())
    }
    fn link(&mut self, c: &Connection, edge: Edge, from: &str) -> Result<()> {
        if self.add(c, edge.other(from))? {
            self.edges.insert(edge.id.clone(), edge);
        }
        Ok(())
    }
}

/// Both directions of `id`, optionally one relationship, each direction bounded.
fn adjacent(c: &Connection, id: &str, relationship: Option<&str>, limit: usize) -> Result<Vec<Edge>> {
    let mut out = vec![];
    for column in ["source", "target"] {
        let sql = format!(
            "SELECT id,source,target,relationship,detail FROM lineage_edges WHERE {column}=?1 AND (?2 IS NULL OR relationship=?2) ORDER BY id LIMIT ?3"
        );
        let mut stmt = c.prepare(&sql)?;
        let rows = stmt.query_map(params![id, relationship, limit as i64], |r| {
            Ok(Edge {
                id: r.get(0)?,
                source: r.get(1)?,
                target: r.get(2)?,
                relationship: r.get(3)?,
                detail: r.get(4)?,
            })
        })?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

fn reading_summary(c: &Connection, id: &str) -> Result<Option<Value>> {
    let Some(raw) = c
        .query_row("SELECT json FROM interpretations WHERE id=?1", [id], |r| r.get::<_, String>(0))
        .optional()?
    else {
        return Ok(None);
    };
    let i: Value = serde_json::from_str(&raw)?;
    let count = |key: &str| i[key].as_array().map_or(0, Vec::len);
    Ok(Some(json!({
        "id": id,
        "evidence_id": i["evidence_id"],
        "revision": i["revision"],
        "created_at": i["created_at"],
        "model": i["model"],
        "recipe": i["recipe"],
        "processing_mode": i.get("processing_mode").cloned().unwrap_or(json!("reading")),
        "prior_interpretation_id": i["prior_interpretation_id"],
        "reference_edition": i["reference_edition"],
        "headline": i["headline"],
        "language": i["language"],
        "kind": i["kind"],
        "claim_status": i["claim_status"],
        "location_label": i["location_label"],
        "location_precision": i["location_precision"],
        "locations": count("locations"),
        "claims": count("claims"),
        "events": count("events"),
    })))
}

impl Store {
    /// The recorded path of one exact message revision (`id@revision`).
    pub fn message_lineage(&self, root: &str) -> Result<Value> {
        let c = self.connect()?;
        let kind: Option<String> = c
            .query_row("SELECT kind FROM lineage_nodes WHERE id=?1", [root], |r| r.get(0))
            .optional()?;
        match kind.as_deref() {
            None => return Err(Error::missing("Message lineage unavailable")),
            Some("evidence_revision") => {}
            Some(_) => return Err(Error::invalid("Message lineage needs an exact message revision")),
        }
        let mut g = Graph::default();
        g.add(&c, root)?;

        // Earlier and later exact revisions of the same message.
        let mut queue = VecDeque::from([root.to_owned()]);
        let mut versions = 1;
        while let Some(id) = queue.pop_front() {
            for e in adjacent(&c, &id, Some("superseded_by"), MAX_VERSIONS)? {
                let next = e.other(&id).to_owned();
                if !g.nodes.contains_key(&next) {
                    if versions >= MAX_VERSIONS {
                        g.hidden.insert(next);
                        continue;
                    }
                    if !g.add(&c, &next)? {
                        continue;
                    }
                    versions += 1;
                    queue.push_back(next);
                }
                g.edges.insert(e.id.clone(), e);
            }
        }

        // Every reading of this exact revision has one `supports` edge from it.
        // A `reading_reused` edge points at a newer revision's reading instead.
        let mut readings: Vec<String> = adjacent(&c, root, Some("supports"), ADJACENT_EDGES)?
            .into_iter()
            .filter(|e| e.source == root)
            .map(|e| e.target)
            .collect();
        if readings.len() > MAX_READINGS {
            g.hidden.extend(readings.split_off(MAX_READINGS));
        }

        // Direct connections of the message and its readings. Neighbors are not expanded.
        for owned in std::iter::once(root.to_owned()).chain(readings.iter().cloned()) {
            if !g.add(&c, &owned)? {
                continue;
            }
            for e in adjacent(&c, &owned, None, ADJACENT_EDGES)? {
                g.link(&c, e, &owned)?;
            }
        }
        // A retaining run's source and publication, without its other records.
        let runs: Vec<String> = g
            .edges
            .values()
            .filter(|e| e.source == root && e.relationship == "input_record")
            .map(|e| e.target.clone())
            .collect();
        for run in runs {
            if g.kind(&run) != Some("run_step") {
                continue;
            }
            for relationship in ["collected_by", "published"] {
                for e in adjacent(&c, &run, Some(relationship), 3)? {
                    g.link(&c, e, &run)?;
                }
            }
        }

        let mut summaries = vec![];
        for id in &readings {
            if g.nodes.contains_key(id)
                && let Some(summary) = reading_summary(&c, id)?
            {
                summaries.push(summary);
            }
        }
        summaries.sort_by(|a, b| {
            a["created_at"].as_str().cmp(&b["created_at"].as_str()).then(a["id"].as_str().cmp(&b["id"].as_str()))
        });
        let mut edges = vec![];
        for e in g.edges.values() {
            if g.nodes.contains_key(&e.source) && g.nodes.contains_key(&e.target) {
                edges.push(json!({"id":e.id,"source":e.source,"target":e.target,"relationship":e.relationship,"detail":serde_json::from_str::<Value>(&e.detail)?}));
            }
        }
        g.hidden.retain(|id| !g.nodes.contains_key(id));
        Ok(json!({
            "root": root,
            "nodes": g.nodes.into_values().collect::<Vec<_>>(),
            "edges": edges,
            "readings": summaries,
            "hidden_nodes": g.hidden.len(),
            "hidden_count_is_lower_bound": true,
            "snapshot": Self::sequence(&c)?.to_string(),
            "limits": {"nodes":MAX_NODES,"adjacent_edges_per_node":ADJACENT_EDGES,"versions":MAX_VERSIONS,"readings":MAX_READINGS},
        }))
    }
}
