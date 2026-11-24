//! Compact viewing DTO. Full readings and originals stay on /evidence.
use crate::{
    error::{Error, Result},
    model::Query,
    store::Store,
};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
impl Store {
    pub fn query_preview(&self, input: Query) -> Result<Value> {
        let result = self.query_without_area_shapes(input)?;
        Ok(compact_preview(result))
    }
    pub fn query_page_preview(&self, input: Query) -> Result<Value> {
        Ok(compact_preview(self.query_page(input)?))
    }
    pub fn area_shape(&self, id: &str) -> Result<Value> {
        area_shape(self, id)
    }
}
fn compact_preview(mut result: Value) -> Value {
    if let Some(items) = result["items"].as_array_mut() {
        for item in items {
            if let Some(reading) = item["interpretation"].as_object_mut() {
                for key in [
                    "claims",
                    "model",
                    "created_at",
                    "source_input_hash",
                    "processing_mode",
                    "recipe",
                    "prompt_version",
                    "reference_edition",
                    "reading_revision",
                    // Processing internals; the reader loads full readings from /evidence.
                    "support_alignment",
                    "related_meaning",
                    "event_schema",
                    "prior_interpretation_id",
                ] {
                    reading.remove(key);
                }
                if let Some(events) = reading.get_mut("events").and_then(Value::as_array_mut) {
                    for event in events {
                        if let Some(event) = event.as_object_mut() {
                            event.retain(|key, _| {
                                ["type", "state", "locations"].contains(&key.as_str())
                            });
                        }
                        compact_locations(&mut event["locations"]);
                    }
                }
                if let Some(locations) = reading.get_mut("locations") {
                    compact_locations(locations);
                }
            }
        }
    }
    // The map draws, groups and lists points by evidence identity, site, event
    // type and group; feed rows carry their own locations. These per-point
    // fields are not read by the map and stay available from /query.
    if let Some(points) = result
        .get_mut("map")
        .and_then(|v| v.get_mut("features"))
        .and_then(Value::as_array_mut)
    {
        for point in points {
            if let Some(properties) = point["properties"].as_object_mut() {
                for key in ["message_key", "location_label", "approximate", "event_reports"] {
                    properties.remove(key);
                }
            }
        }
    }
    if let Some(areas) = result
        .get_mut("map_areas")
        .and_then(|v| v.get_mut("features"))
        .and_then(Value::as_array_mut)
    {
        for area in areas {
            area["geometry"] = Value::Null;
        }
    }
    result["representation"] = json!("preview-v1");
    result
}
fn area_shape(store: &Store, id: &str) -> Result<Value> {
    if id.len() > 500 {
        return Err(Error::invalid("Invalid area identity"));
    }
    let c = store.connect()?;
    let geometry: Option<String> = c
        .query_row(
            "SELECT display_json FROM boundary_shapes WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let geometry = geometry.ok_or_else(|| Error::missing("Retained area shape is unavailable"))?;
    Ok(json!({"id":id,"geometry":serde_json::from_str::<Value>(&geometry)?}))
}
fn compact_locations(value: &mut Value) {
    if let Some(locations) = value.as_array_mut() {
        for location in locations {
            let selected = location["selected_candidate_id"].clone();
            if let Some(candidates) = location["candidates"].as_array_mut() {
                candidates.retain(|c| !selected.is_null() && c["id"] == selected);
                for candidate in candidates {
                    if let Some(c) = candidate.as_object_mut() {
                        c.retain(|k, _| ["id", "name", "country_code"].contains(&k.as_str()));
                    }
                }
            }
            if let Some(l) = location.as_object_mut() {
                l.retain(|k, _| ["selected_candidate_id", "candidates"].contains(&k.as_str()));
            }
        }
    }
}
