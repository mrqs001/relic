//! Immutable message interpretations, usage accounting and coherent publication.
use crate::{
    error::{Error, Result},
    model::*,
    processing_ai::{self, ValidatedOutput},
    processing_geo::{GeoResolver, LocationProposal},
    store::{Store, edge, node},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

pub const RECIPE: &str = "message-events-v6";
// Accounting estimate for unknown billing, retained across restarts.
// Cumulative request counts and costs never gate processing.
pub const REQUEST_RESERVE_USD: f64 = 0.05;
pub const BATCH: usize = 256;
const TRANSLATION_REPAIR: &str = "message-events-v6/translation-echo-repair-v1";
const RECOVERY_VERSION: &str = "relic.response-recovery.v8";
const CORRECTION_RECIPE: &str = "message-events-v6/correction-v8";

// One eligibility definition for dispatch and status. Counts describe work, not
// subtraction of overlapping historical failures from retained-message totals.
// An interrupted (unknown-outcome) correction whose retry the owner explicitly
// authorized does not consume the once-only correction allowance; that retry
// is the correction. A retry cut off again is held again until re-authorized.
// Messages published more than `processing_max_age_days` ago (workspace metadata;
// absent means no limit) are never read: they count as ineligible, not pending.
// The owner set five days for the live workspace on 23 October 2026. The same
// limit applies to Jev (supplemental.rs) and event-link review
// (event_link_store.rs, event_semantic.rs).
const WORK_CTE: &str = r#"
WITH work AS (
 SELECT e.rowid,e.json,e.published_at,e.acquired_at,pi.interpretation_id,
 relic_needs_translation(e.text,json_extract(i.json,'$.language'),json_extract(i.json,'$.translation')) AS needs_translation,
 (SELECT a.id FROM processing_attempts a WHERE a.evidence_rowid=e.rowid
   AND a.status='failed' AND a.cost_usd IS NOT NULL
   AND json_type(a.json,'$.audit_hash')='text'
   AND (json_extract(a.json,'$.recipe_key')=?4 OR json_extract(a.json,'$.recipe_key') LIKE ?4 || '/geo:%')
   AND (json_extract(a.json,'$.error') IN (
    'The model response was incomplete; no result was accepted',
    'The model output did not match the bounded analysis schema',
    'The model cited text that does not occur exactly in the original evidence',
    'The model copied the original text instead of translating it into English',
    'The model quote matches multiple passages; its exact support is ambiguous')
    OR (json_extract(a.json,'$.error')='Relic stopped waiting after its 120-second model request deadline'
        AND json_type(a.json,'$.billing_receipt_hash')='text'))
   ORDER BY a.created_at DESC,a.id DESC LIMIT 1) AS correction_of,
 CASE
 WHEN i.recipe IN (?2,?2 || '/translation-repair-v1') AND NOT relic_needs_translation(e.text,json_extract(i.json,'$.language'),json_extract(i.json,'$.translation')) THEN 'current'
 WHEN coalesce(json_extract(e.json,'$.upstream_state'),'available')='removed' THEN 'removed'
 WHEN trim(e.text)='' THEN 'without_text'
 WHEN e.source NOT IN (SELECT value FROM json_each(?3)) OR julianday(coalesce(e.published_at,e.acquired_at))<julianday('now')-coalesce((SELECT CAST(value AS REAL) FROM metadata WHERE key='processing_max_age_days'),1e9) THEN 'ineligible'
 WHEN length(e.text)>16000 THEN 'oversized'
 WHEN EXISTS(SELECT 1 FROM processing_attempts a WHERE a.evidence_rowid=e.rowid AND a.status='reserved') THEN 'in_flight'
 WHEN EXISTS(SELECT 1 FROM processing_attempts a WHERE a.evidence_rowid=e.rowid AND a.status='unknown' AND json_type(a.json,'$.retry_authorized_at') IS NULL) THEN 'failed'
 WHEN EXISTS(SELECT 1 FROM processing_attempts a WHERE a.evidence_rowid=e.rowid AND a.status NOT IN ('completed','rate_limited') AND json_extract(a.json,'$.recipe_key')=?4 || '/translation-echo-repair-v1') THEN 'failed'
 WHEN EXISTS(SELECT 1 FROM processing_attempts a WHERE a.evidence_rowid=e.rowid AND a.status='completed'
   AND (json_extract(a.json,'$.recipe_key')=?5 OR
      (json_type(a.json,'$.recovery')='object' AND (json_extract(a.json,'$.recipe_key')=?4 OR json_extract(a.json,'$.recipe_key') LIKE ?4 || '/geo:%')))) THEN 'queued'
 WHEN EXISTS(SELECT 1 FROM processing_attempts a WHERE a.evidence_rowid=e.rowid AND a.status NOT IN ('completed','rate_limited')
   AND NOT (a.status='unknown' AND json_type(a.json,'$.retry_authorized_at') IS NOT NULL)
   AND (json_extract(a.json,'$.recipe_key')=?5 OR json_extract(a.json,'$.recipe_key')=?4 OR json_extract(a.json,'$.recipe_key') LIKE ?4 || '/geo:%')) THEN 'rejected'
 ELSE 'queued' END AS base_state,
 EXISTS(SELECT 1 FROM processing_attempts a WHERE a.evidence_rowid=e.rowid AND a.status!='rate_limited'
   AND NOT (a.status='unknown' AND json_type(a.json,'$.retry_authorized_at') IS NOT NULL)
   AND json_extract(a.json,'$.recipe_key')=?5) AS correction_attempted
 FROM membership m JOIN evidence e ON e.rowid=m.evidence_rowid
 LEFT JOIN interpretation_intervals pi ON pi.evidence_rowid=m.evidence_rowid AND pi.from_seq<=(SELECT seq FROM publication_order WHERE id=m.publication_id) AND (pi.until_seq IS NULL OR pi.until_seq>(SELECT seq FROM publication_order WHERE id=m.publication_id))
 LEFT JOIN interpretations i ON i.id=pi.interpretation_id
 WHERE m.publication_id=?1
), classified AS (
 SELECT *,CASE WHEN base_state='rejected' THEN
   CASE WHEN correction_of IS NOT NULL AND NOT correction_attempted AND ?6 THEN 'correctable' ELSE 'failed' END
 ELSE base_state END AS state FROM work
)
"#;

fn register_reading_functions(c: &Connection) -> Result<()> {
    c.create_scalar_function(
        "relic_needs_translation",
        3,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let original = ctx.get::<String>(0)?;
            let language = ctx.get::<Option<String>>(1)?;
            let translation = ctx.get::<Option<String>>(2)?;
            Ok(language.is_some_and(|language| {
                processing_ai::reading_needs_translation(
                    &original,
                    &language,
                    translation.as_deref(),
                )
            }))
        },
    )?;
    Ok(())
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub enabled: bool,
    pub eligible_source_ids: Vec<String>,
}
pub struct Pending {
    pub rowid: i64,
    pub evidence: Value,
    pub repair_of: Option<String>,
    pub correction_of: Option<String>,
}
pub struct Prepared {
    pub rowid: i64,
    pub interpretation: Value,
    pub audit: Value,
    retained_object: Option<String>,
    expected_prior: Option<String>,
}

pub fn carry_interpretations(c: &Connection, publication: &str) -> Result<()> {
    c.execute("INSERT OR IGNORE INTO publication_interpretations SELECT m.publication_id,m.evidence_rowid,i.id FROM membership m JOIN interpretations i ON i.evidence_rowid=m.evidence_rowid WHERE m.publication_id=?1 AND i.id=(SELECT x.id FROM interpretations x WHERE x.evidence_rowid=m.evidence_rowid ORDER BY x.created_at DESC,x.id DESC LIMIT 1) AND NOT EXISTS(SELECT 1 FROM publication_interpretations old WHERE old.publication_id=m.publication_id AND old.evidence_rowid=m.evidence_rowid)",[publication])?;
    Ok(())
}

pub fn prepare(
    rowid: i64,
    e: &Value,
    extracted: &ValidatedOutput,
    resolver: &GeoResolver,
    model: &str,
) -> Result<Prepared> {
    let text = required(e, "text")?;
    // Validate again at the publication boundary; public DTOs are not authority.
    let checked = processing_ai::validate_output(text, &value(&extracted.output))
        .map_err(|x| Error::invalid(x.to_string()))?;
    let out = &checked.output;
    let proposals: Vec<LocationProposal> = serde_json::from_value(value(&out.locations))?;
    let occurrence = out.kind == processing_ai::Kind::Event
        && out.claim_status == processing_ai::ClaimStatus::Reported;
    let (locations, message_country) = resolver
        .resolve_message(text, &proposals, occurrence)
        .map_err(|_| {
            Error::invalid("Geographic reference validation failed; no interpretation published")
        })?;
    let mut claims = value(&out.claims);
    for (index, claim) in claims
        .as_array_mut()
        .ok_or_else(Error::internal)?
        .iter_mut()
        .enumerate()
    {
        let span = &checked.claim_spans[index];
        claim["citation"] = json!({"evidence_id":e["id"],"revision":e["revision"],"start_character":span.start_character,"end_character":span.end_character});
    }
    let mut resolved = value(&locations);
    for (index, location) in resolved
        .as_array_mut()
        .ok_or_else(Error::internal)?
        .iter_mut()
        .enumerate()
    {
        let span = &checked.location_spans[index];
        location["citation"] = json!({"evidence_id":e["id"],"revision":e["revision"],"start_character":span.start_character,"end_character":span.end_character});
    }
    let mapped: Vec<&Value> = resolved
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["geometry"].is_object())
        .collect();
    // Multiple different event places cannot be silently collapsed to the first mention.
    let unique: std::collections::BTreeSet<&str> = mapped
        .iter()
        .filter_map(|l| l["selected_candidate_id"].as_str())
        .collect();
    let selected = if unique.len() == 1 {
        mapped.first().copied()
    } else {
        None
    };
    let geometry = selected
        .map(|l| l["geometry"].clone())
        .unwrap_or(Value::Null);
    let label = selected
        .and_then(|l| {
            l["candidates"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["id"] == l["selected_candidate_id"]))
        })
        .map(|c| c["name"].clone())
        .unwrap_or(Value::Null);
    let related_key = if let (Some(place), Some(date)) = (
        unique.iter().next().filter(|_| unique.len() == 1),
        e["published_at"].as_str().filter(|s| s.len() >= 10),
    ) {
        if occurrence && out.topic != processing_ai::Topic::Other {
            Some(hash(
                format!("{}|{}|{}", place, string(&value(&out.topic)), &date[..10]).as_bytes(),
            ))
        } else {
            None
        }
    } else {
        None
    };
    let mut interpretation = json!({"id":ident("interpretation"),"recipe":RECIPE,"evidence_id":e["id"],"revision":e["revision"],"language":out.language,"translation":out.translation,"headline":out.headline,"kind":out.kind,"claim_status":out.claim_status,"topic":out.topic,"claims":claims,"locations":resolved,"geometry":geometry,"location_label":label,"location_precision":mapped_precision(&geometry,&resolved),"related_key":related_key,"related_count":0,"related_meaning":"Same city, topic and publication day; not a merged incident or independent confirmation","model":model,"created_at":now(),"source_input_hash":hash(text.as_bytes())});
    interpretation["reference_edition"] = value(resolver.edition());
    let mut events = Vec::new();
    for (index, claim) in out.claims.iter().enumerate() {
        let Some(event_type) = &claim.event_type else {
            continue;
        };
        let chosen = claim
            .location_indices
            .iter()
            .map(|i| proposals[*i].clone())
            .collect::<Vec<_>>();
        let mut resolved_places = resolver.resolve_in(
            text,
            &chosen,
            claim.state == processing_ai::ClaimStatus::Reported,
            message_country.as_deref(),
        )?;
        crate::processing_geo::follow_message(&locations, &claim.location_indices, &mut resolved_places);
        let mut places = value(&resolved_places);
        for (position, place) in places.as_array_mut().unwrap().iter_mut().enumerate() {
            let span = &checked.location_spans[claim.location_indices[position]];
            place["citation"] = json!({"evidence_id":e["id"],"revision":e["revision"],"start_character":span.start_character,"end_character":span.end_character});
        }
        let (point, label) = single_event_place(places.as_array().unwrap().iter());
        events.push(json!({"id":format!("{}:event:{index}",interpretation["id"].as_str().unwrap()),"type":event_type,"state":claim.state,"text":claim.text,"quote":claim.quote,"citation":claims[index]["citation"],"time_text":claim.time_text,"locations":places,"geometry":point,"location_label":label,"location_precision":mapped_precision(&point,&places)}));
    }
    if !events.is_empty() {
        // A message dot can represent one common reported place. Keep different
        // event sites separate in the retained claims instead of choosing one.
        let (mut point, mut label) = single_event_place(
            events
                .iter()
                .filter(|e| e["state"] == "reported")
                .flat_map(|event| event["locations"].as_array().unwrap()),
        );
        if events
            .iter()
            .any(|event| event["state"] == "reported" && event["geometry"].is_null())
        {
            point = Value::Null;
            label = Value::Null;
        }
        interpretation["geometry"] = point.clone();
        interpretation["location_label"] = label;
        interpretation["location_precision"] =
            mapped_precision(&point, &interpretation["locations"]);
        if point != geometry {
            interpretation["related_key"] = Value::Null;
        }
        interpretation["event_schema"] = json!("relic.event-claims.v1");
        interpretation["events"] = json!(events);
    }
    interpretation["support_alignment"] =
        json!({"claims":extracted.claim_spans,"locations":extracted.location_spans});
    Ok(Prepared {
        rowid,
        interpretation,
        audit: Value::Null,
        retained_object: None,
        expected_prior: None,
    })
}

fn single_event_place<'a>(places: impl Iterator<Item = &'a Value>) -> (Value, Value) {
    let mapped = places
        .filter(|place| place["geometry"].is_object())
        .collect::<Vec<_>>();
    let identities = mapped
        .iter()
        .filter_map(|place| place["selected_candidate_id"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if identities.len() != 1 {
        return (Value::Null, Value::Null);
    }
    let place = mapped[0];
    let label = place["candidates"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|c| c["id"] == place["selected_candidate_id"])
        })
        .map(|candidate| candidate["name"].clone())
        .unwrap_or(Value::Null);
    (place["geometry"].clone(), label)
}

fn reproject(
    rowid: i64,
    e: &Value,
    old: &Value,
    out: &ValidatedOutput,
    resolver: &GeoResolver,
) -> Result<Prepared> {
    let text = required(e, "text")?;
    let proposals: Vec<LocationProposal> = serde_json::from_value(value(&out.output.locations))?;
    let mut i = old.clone();
    let interpretation_id = ident("interpretation");
    i["id"] = json!(interpretation_id);
    i["created_at"] = json!(now());
    i["reference_edition"] = value(resolver.edition());
    i["processing_mode"] = json!("geography_only");
    i["prior_interpretation_id"] = old["id"].clone();
    let (resolved_locations, message_country) = resolver.resolve_message(
        text,
        &proposals,
        old["kind"] == "event" && old["claim_status"] == "reported",
    )?;
    let mut locations = value(&resolved_locations);
    for (index, location) in locations.as_array_mut().unwrap().iter_mut().enumerate() {
        location["citation"] = old["locations"][index]["citation"].clone();
    }
    i["locations"] = locations;
    // Legacy readings may omit event records. Indexing mutably would insert
    // null and break the typed reader; preserve absence and its legacy meaning.
    if i.get("events").is_some_and(Value::is_null) {
        i.as_object_mut()
            .ok_or_else(Error::internal)?
            .remove("events");
    }
    if let Some(events) = i.get_mut("events").and_then(Value::as_array_mut) {
        for (index, event) in events.iter_mut().enumerate() {
            let indices = event["locations"]
                .as_array()
                .ok_or_else(Error::internal)?
                .iter()
                .filter_map(|l| {
                    proposals.iter().position(|p| {
                        p.mention == l["mention"] && p.quote == l["quote"] && value(&p.role) == l["role"]
                    })
                })
                .collect::<Vec<_>>();
            let chosen = indices.iter().map(|i| proposals[*i].clone()).collect::<Vec<_>>();
            let mut resolved_places = resolver.resolve_in(
                text,
                &chosen,
                event["state"] == "reported",
                message_country.as_deref(),
            )?;
            crate::processing_geo::follow_message(&resolved_locations, &indices, &mut resolved_places);
            let mut places = value(resolved_places);
            for (n, place) in places.as_array_mut().unwrap().iter_mut().enumerate() {
                place["citation"] = event["locations"][n]["citation"].clone();
            }
            let (point, label) = single_event_place(places.as_array().unwrap().iter());
            event["id"] = json!(format!("{}:event:{index}", interpretation_id));
            event["locations"] = places;
            event["geometry"] = point.clone();
            event["location_label"] = label;
            event["location_precision"] = mapped_precision(&point, &event["locations"]);
        }
    }
    let (mut geometry, mut label) = single_event_place(i["locations"].as_array().unwrap().iter());
    if let Some(events) = i["events"].as_array().filter(|e| !e.is_empty()) {
        (geometry, label) = single_event_place(
            events
                .iter()
                .filter(|e| e["state"] == "reported")
                .flat_map(|e| e["locations"].as_array().unwrap()),
        );
        if events
            .iter()
            .any(|e| e["state"] == "reported" && e["geometry"].is_null())
        {
            geometry = Value::Null;
            label = Value::Null;
        }
    }
    i["geometry"] = geometry.clone();
    i["location_label"] = label;
    i["location_precision"] = mapped_precision(&geometry, &i["locations"]);
    i["related_key"] = Value::Null;
    if geometry.is_object()
        && old["topic"] != "other"
        && old["claim_status"] == "reported"
        && let (Some(place), Some(date)) = (
            i["locations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|l| l["geometry"] == geometry)
                .and_then(|l| l["selected_candidate_id"].as_str()),
            e["published_at"].as_str().filter(|d| d.len() >= 10),
        )
    {
        i["related_key"] = json!(hash(
            format!("{}|{}|{}", place, string(&old["topic"]), &date[..10]).as_bytes()
        ));
    }
    Ok(Prepared {
        rowid,
        interpretation: i,
        audit: Value::Null,
        retained_object: None,
        expected_prior: Some(required(old, "id")?.to_owned()),
    })
}

fn mapped_precision(point: &Value, places: &Value) -> Value {
    if point.is_null() {
        return Value::Null;
    }
    let at = |precision: &str| {
        places.as_array().is_some_and(|ls| ls.iter().any(|l| l["geometry"] == *point && l["precision"] == precision))
    };
    json!(if at("approximate_facility") {
        "approximate_facility"
    } else if at(crate::processing_geo::MARINE_FEATURE_PRECISION) {
        crate::processing_geo::MARINE_FEATURE_PRECISION
    } else {
        "approximate_city"
    })
}

fn recipe_key(recipe: &str, edition: Option<&str>) -> String {
    edition.map_or_else(
        || recipe.to_string(),
        |edition| format!("{recipe}/geo:{edition}"),
    )
}

fn interpretation_recipe_key(i: &Value, edition: Option<&str>) -> Result<String> {
    let key = recipe_key(required(i, "recipe")?, edition);
    Ok(if i["reading_revision"] == "translation-repair-v1" {
        format!("{key}/translation-repair-v1")
    } else {
        key
    })
}

// Compare the retained semantic reading, excluding only geography and its
// version metadata. Top-level/event extension fields remain in the comparison;
// locations compare source mention, quote, role and citation, not resolver output.
fn reading_content(i: &Value) -> Value {
    let mut content = i.clone();
    if let Some(fields) = content.as_object_mut() {
        for key in [
            "id",
            "created_at",
            "reference_edition",
            "processing_mode",
            "prior_interpretation_id",
            "geometry",
            "location_label",
            "location_precision",
            "related_key",
            "related_count",
        ] {
            fields.remove(key);
        }
    }
    fn places(record: &mut Value) {
        if let Some(locations) = record.get_mut("locations").and_then(Value::as_array_mut) {
            for location in locations {
                *location = json!({"mention":location["mention"],"quote":location["quote"],"role":location["role"],"citation":location["citation"]});
            }
        }
    }
    places(&mut content);
    if let Some(events) = content.get_mut("events").and_then(Value::as_array_mut) {
        for event in events {
            if let Some(fields) = event.as_object_mut() {
                for key in ["id", "geometry", "location_label", "location_precision"] {
                    fields.remove(key);
                }
            }
            places(event);
        }
    }
    content
}

impl Store {
    pub(crate) fn processing_recipe_key(&self) -> Result<String> {
        Ok(recipe_key(
            RECIPE,
            crate::boundary_store::combined_edition(
                self.geography()?.as_ref(),
                self.boundaries()?.as_ref(),
                self.marine()?.as_ref(),
                self.osm()?.as_ref(),
            )
            .as_deref(),
        ))
    }
    pub fn processing_policy(&self) -> Result<Policy> {
        let c = self.connect()?;
        let raw: Option<String> = c
            .query_row(
                "SELECT value FROM metadata WHERE key='processing_policy'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        raw.map(|v| serde_json::from_str(&v).map_err(Error::from))
            .unwrap_or_else(|| Ok(Policy::default()))
    }
    pub fn set_processing_policy(&self, policy: Policy) -> Result<()> {
        self.cancel_event_links();
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut connection = self.connect()?;
        let c = connection.transaction()?;
        self.write_policy(c, policy)
    }
    /// Select or deselect one source for reading, reading and writing the
    /// policy under the writer lock so concurrent changes are not lost.
    /// Deselecting the last source pauses reading; nothing is left to read.
    pub fn set_source_reading(&self, source: &str, reading: bool) -> Result<Policy> {
        self.cancel_event_links();
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut connection = self.connect()?;
        let c = connection.transaction()?;
        let raw: Option<String> = c
            .query_row("SELECT value FROM metadata WHERE key='processing_policy'", [], |r| r.get(0))
            .optional()?;
        let mut policy: Policy = raw.map(|v| serde_json::from_str(&v)).transpose()?.unwrap_or_default();
        policy.eligible_source_ids.retain(|id| id != source);
        if reading {
            policy.eligible_source_ids.push(source.to_owned());
        } else if policy.eligible_source_ids.is_empty() {
            policy.enabled = false;
        }
        self.write_policy(c, policy.clone())?;
        Ok(policy)
    }
    fn write_policy(&self, c: rusqlite::Transaction<'_>, mut policy: Policy) -> Result<()> {
        policy.eligible_source_ids.sort();
        policy.eligible_source_ids.dedup();
        for id in &policy.eligible_source_ids {
            if !c.query_row(
                "SELECT EXISTS(SELECT 1 FROM sources WHERE id=?1)",
                [id],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(Error::invalid(
                    "Processing source is not retained in this workspace",
                ));
            }
        }
        if policy.enabled && policy.eligible_source_ids.is_empty() {
            return Err(Error::invalid(
                "Select eligible sources before enabling processing",
            ));
        }
        c.execute("INSERT INTO metadata VALUES('processing_policy',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[string(&value(&policy))])?;
        if policy.enabled {
            c.execute("DELETE FROM metadata WHERE key='event_links_error'", [])?;
        }
        // Choosing eligible IDs is the operator's explicit declaration, separate
        // from source login. Keep the reference/date in authoritative backup state.
        let previous: Option<String> = c
            .query_row(
                "SELECT value FROM metadata WHERE key='processing_eligibility'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let same = previous
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .is_some_and(|p| p["source_ids"] == value(&policy.eligible_source_ids));
        if !same {
            let declaration = json!({"source_ids":policy.eligible_source_ids,"checked_at":now(),"policy_reference":"Workspace owner authorization through explicit processing-source selection","operation":"external text processing","destination":"OpenRouter"});
            c.execute("INSERT INTO metadata VALUES('processing_eligibility',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[string(&declaration)])?;
        }
        let ev = Self::event(&c, "processing", Self::latest(&c)?.as_deref(), vec![])?;
        c.commit()?;
        let _ = self.events.send(ev);
        Ok(())
    }
    pub fn processing_evaluation(&self) -> Result<Option<Value>> {
        let c = self.connect()?;
        let raw: Option<String> = c
            .query_row(
                "SELECT value FROM metadata WHERE key='processing_evaluation'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        raw.map(|s| serde_json::from_str(&s).map_err(Error::from))
            .transpose()
    }
    pub fn record_processing_evaluation(&self, report: &Value) -> Result<()> {
        let model = self
            .config
            .source_values
            .get("AI_DEFAULT_MODEL")
            .map(String::as_str)
            .unwrap_or(processing_ai::DEFAULT_MODEL);
        let correction = report["correction_version"].is_string();
        if report["recipe"] != RECIPE
            || !processing_ai::evaluation_matches(report, model, correction)
        {
            return Err(Error::invalid(
                "A passed fictional evaluation of this model, provider and request profile is required",
            ));
        }
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let key = if correction {
            "processing_correction_evaluation"
        } else {
            "processing_evaluation"
        };
        self.connect()?.execute("INSERT INTO metadata VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[key.to_owned(), string(report)])?;
        Ok(())
    }
    pub fn processing_pending(&self, policy: &Policy) -> Result<Vec<Pending>> {
        if policy.eligible_source_ids.is_empty() {
            return Ok(vec![]);
        }
        let c = self.connect()?;
        register_reading_functions(&c)?;
        let Some(publication) = Self::latest(&c)? else {
            return Ok(vec![]);
        };
        let sql = format!(
            "{WORK_CTE} SELECT rowid,json,CASE WHEN state='correctable' THEN correction_of END,CASE WHEN needs_translation THEN interpretation_id END FROM classified WHERE state IN ('queued','correctable') ORDER BY state='correctable',coalesce(published_at,acquired_at) DESC,rowid DESC LIMIT ?7"
        );
        let mut stmt = c.prepare(&sql)?;
        let rows = stmt
            .query_map(
                params![
                    &publication,
                    self.processing_recipe_key()?,
                    string(&value(&policy.eligible_source_ids)),
                    RECIPE,
                    CORRECTION_RECIPE,
                    self.processing_correction_evaluated()?,
                    BATCH as i64
                ],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let pending: Vec<Pending> = rows
            .into_iter()
            .map(|(rowid, s, correction_of, repair_of)| {
                Ok(Pending {
                    rowid,
                    evidence: serde_json::from_str(&s)?,
                    repair_of,
                    correction_of,
                })
            })
            .collect::<Result<_>>()?;
        Ok(pending)
    }

    /// A corrective prompt changes generation behavior and requires its own
    /// reviewed fictional gate. A baseline gate never implicitly enables it.
    pub fn processing_correction_evaluated(&self) -> Result<bool> {
        let Some(config) = processing_ai::Config::from_mapping(&self.config.source_values).ok()
        else {
            return Ok(false);
        };
        let raw: Option<String> = self
            .connect()?
            .query_row(
                "SELECT value FROM metadata WHERE key='processing_correction_evaluation'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let report: Option<Value> = raw.as_deref().map(serde_json::from_str).transpose()?;
        Ok(report.is_some_and(|r| {
            r["recipe"] == RECIPE && processing_ai::evaluation_matches(&r, &config.model, true)
        }))
    }
    /// Messages held only because a request was cut off (unknown outcome). The
    /// provider may have billed it, so nothing retries automatically.
    pub fn processing_interrupted(&self) -> Result<i64> {
        let policy = self.processing_policy()?;
        let c = self.connect()?;
        register_reading_functions(&c)?;
        let publication = Self::latest(&c)?.unwrap_or_default();
        Ok(c.query_row(
            &format!("{WORK_CTE} SELECT count(*) FROM classified w WHERE w.state='failed' AND EXISTS(SELECT 1 FROM processing_attempts a WHERE a.evidence_rowid=w.rowid AND a.status='unknown' AND json_type(a.json,'$.retry_authorized_at') IS NULL)"),
            params![
                &publication,
                self.processing_recipe_key()?,
                string(&value(&policy.eligible_source_ids)),
                RECIPE,
                CORRECTION_RECIPE,
                self.processing_correction_evaluated()?
            ],
            |r| r.get(0),
        )?)
    }
    /// Explicitly allow one new paid attempt for each message held by an
    /// unknown outcome. The original attempt, its reservation and any
    /// reconciled cost stay in the ledger; a retry cut off again is held again.
    pub fn authorize_interrupted_retry(&self) -> Result<i64> {
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let c = self.connect()?;
        let publication = Self::latest(&c)?.unwrap_or_default();
        Ok(c.execute(
            "UPDATE processing_attempts SET json=json_set(json,'$.retry_authorized_at',?1) WHERE status='unknown' AND json_type(json,'$.retry_authorized_at') IS NULL AND evidence_rowid IN (SELECT evidence_rowid FROM membership WHERE publication_id=?2) AND NOT EXISTS(SELECT 1 FROM processing_attempts later WHERE later.evidence_rowid=processing_attempts.evidence_rowid AND later.status='completed' AND later.created_at>processing_attempts.created_at)",
            params![now(), publication],
        )? as i64)
    }
    pub fn processing_failures(&self, before: Option<i64>) -> Result<Value> {
        let c = self.connect()?;
        register_reading_functions(&c)?;
        let policy = self.processing_policy()?;
        let publication = Self::latest(&c)?.unwrap_or_default();
        let sql = format!(
            "{WORK_CTE} SELECT w.rowid,w.json,w.interpretation_id,a.json,a.status FROM classified w JOIN processing_attempts a ON a.id=(SELECT x.id FROM processing_attempts x WHERE x.evidence_rowid=w.rowid AND x.status IN ('failed','unknown') ORDER BY x.created_at DESC,x.id DESC LIMIT 1) WHERE w.state='failed' AND w.rowid<?7 ORDER BY w.rowid DESC LIMIT 51"
        );
        let mut rows = c
            .prepare(&sql)?
            .query_map(
                params![
                    publication,
                    self.processing_recipe_key()?,
                    string(&value(&policy.eligible_source_ids)),
                    RECIPE,
                    CORRECTION_RECIPE,
                    self.processing_correction_evaluated()?,
                    before.unwrap_or(i64::MAX)
                ],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let more = rows.len() > 50;
        rows.truncate(50);
        let next = more.then(|| rows.last().unwrap().0.to_string());
        let items = rows.into_iter().map(|(rowid,e,reading,a,status)| -> Result<Value> {
            let e:Value=serde_json::from_str(&e)?;
            let a:Value=serde_json::from_str(&a)?;
            Ok(json!({"message_key":rowid.to_string(),"id":e["id"],"revision":e["revision"],"source_label":e["source_label"],"published_at":e["published_at"],"preview":e["text"].as_str().unwrap_or("").chars().take(200).collect::<String>(),"reason":a["error"].as_str().unwrap_or(if status=="unknown" {"Request outcome is unknown; no automatic retry"} else {"Reading validation failed"}),"retained_reading":reading.is_some()}))
        }).collect::<Result<Vec<_>>>()?;
        Ok(json!({"publication_id":publication,"items":items,"next_cursor":next}))
    }
    /// Polled by the Processing and Sources views. Message counts come from
    /// this store's `StatusMemo`, refreshed against one read snapshot on every
    /// call: a repeated poll reclassifies only the messages whose inputs
    /// changed and never reuses a class computed from different inputs.
    pub fn processing_status(&self, running: bool) -> Result<Value> {
        let policy = self.processing_policy()?;
        let basis = StatusParams {
            recipe_key: self.processing_recipe_key()?,
            sources: string(&value(&policy.eligible_source_ids)),
            correction: self.processing_correction_evaluated()?,
        };
        let mut connection = self.connect()?;
        register_reading_functions(&connection)?;
        // One snapshot for memo inputs, recomputed classes, the latest failure
        // and today's usage; dropping the transaction ends the read.
        let c = connection.transaction()?;
        let pubid = Self::latest(&c)?.unwrap_or_default();
        let (groups, interrupted, failed_rows) = {
            let shared = status_memo(&self.writer)?;
            let mut memo = shared.lock().unwrap_or_else(|poisoned| {
                // A panic mid-refresh leaves no trustworthy entries; recount.
                let mut memo = poisoned.into_inner();
                *memo = StatusMemo::default();
                shared.clear_poison();
                memo
            });
            memo.refresh(&c, &pubid, &basis)?;
            memo.counts()
        };
        let total: i64 = groups.iter().map(|(_, count, _)| count).sum();
        let processed: i64 = groups.iter().map(|(_, _, readings)| readings).sum();
        let count = |state: &str| {
            groups
                .iter()
                .find(|(s, _, _)| s == state)
                .map_or(0, |(_, n, _)| *n)
        };
        let unread = |state: &str| {
            groups
                .iter()
                .find(|(s, _, _)| s == state)
                .map_or(0, |(_, n, p)| n - p)
        };
        let queued = count("queued") + count("correctable");
        let in_flight = count("in_flight");
        let failed = count("failed");
        let without_text = unread("without_text");
        let needs_upgrade: i64 = groups
            .iter()
            .filter(|(s, _, _)| matches!(s.as_str(), "queued" | "correctable" | "in_flight"))
            .map(|(_, _, p)| p)
            .sum();
        let unavailable = json!({"removed":unread("removed"),"without_text":without_text,"ineligible":unread("ineligible"),"oversized":unread("oversized")});
        let day = now()[..10].to_string();
        let (calls,cost,unknown,reserved):(i64,f64,i64,f64)=c.query_row("SELECT count(*),coalesce(sum(cost_usd),0),coalesce(sum(cost_usd IS NULL),0),coalesce(sum(coalesce(cost_usd,reserved_usd)),0) FROM processing_attempts WHERE day=?1",[&day],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        // The latest failed or unknown attempt of a message classified failed.
        let last_error: Option<String> = c
            .prepare_cached("SELECT json_extract(a.json,'$.error') FROM processing_attempts a WHERE a.evidence_rowid IN (SELECT value FROM json_each(?1)) AND a.status IN ('failed','unknown') ORDER BY a.created_at DESC LIMIT 1")?
            .query_row([string(&value(&failed_rows))], |r| r.get(0))
            .optional()?
            .flatten();
        let supplemental_spent:f64=c.query_row("SELECT coalesce(sum(coalesce(cost_usd,reserved_usd)),0) FROM supplemental_attempts WHERE day=?1",[&day],|r|r.get(0))?;
        drop(c);
        let configured = processing_ai::Config::from_mapping(&self.config.source_values).ok();
        let evaluation = self.processing_evaluation()?;
        let evaluated = evaluation.as_ref().is_some_and(|e| {
            e["recipe"] == RECIPE
                && configured.as_ref().is_some_and(|config| {
                    processing_ai::evaluation_matches(e, &config.model, false)
                })
        });
        let quality_notice = if evaluation.as_ref().is_some_and(|e| e["passed"] == false) {
            Some(
                "The latest model quality check failed. New readings are paused; collection and existing readings remain available.",
            )
        } else if evaluation.is_some() && !evaluated {
            Some(
                "The configured model/provider settings need a matching quality evaluation. Collection and existing readings remain available.",
            )
        } else {
            None
        };
        let backoff = self.processing_backoff()?;
        Ok(
            json!({"relationships":self.event_link_status()?,"supplemental":self.supplemental_status()?,"configured":configured.is_some(),"evaluated":evaluated,"quality_notice":quality_notice,"policy":policy,"pending":total-processed,"queued":queued,"in_flight":in_flight,"corrections_queued":count("correctable"),"interrupted":interrupted,"unavailable":unavailable,"without_text":without_text,"processed":processed,"needs_upgrade":needs_upgrade,"failed":failed,"running":running,"last_error":last_error,"today":{"calls":calls,"limit":null,"cost_usd":if unknown>0 {Value::Null}else{json!(cost)},"known_cost_usd":cost,"unknown_cost_requests":unknown,"spend_limit_usd":null,"admission_reserved_usd":reserved+supplemental_spent,"next_request_reserve_usd":REQUEST_RESERVE_USD},"retry_at":backoff["retry_at"],"retry_reason":backoff["reason"],"limits":{"batch":BATCH,"concurrency":backoff["concurrency"]},"jobs":[],"coverage":"Text-only reading aids; city-level locations are approximate proposals. Unsupported and ambiguous places remain unlocated."}),
        )
    }
    pub fn reserve_processing(&self, rowid: i64) -> Result<String> {
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let day = now()[..10].to_string();
        let current = Self::latest(&tx)?.unwrap_or_default();
        let reading: Option<(String,String,Option<String>)> = tx.query_row("SELECT e.text,json_extract(i.json,'$.language'),json_extract(i.json,'$.translation') FROM publication_interpretations pi JOIN interpretations i ON i.id=pi.interpretation_id JOIN evidence e ON e.rowid=pi.evidence_rowid WHERE pi.publication_id=?1 AND pi.evidence_rowid=?2",params![current,rowid], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let repair = reading.is_some_and(|(text, language, translation)| {
            processing_ai::reading_needs_translation(&text, &language, translation.as_deref())
        });
        let attempt_recipe = if repair { TRANSLATION_REPAIR } else { RECIPE };
        if tx.query_row("SELECT EXISTS(SELECT 1 FROM processing_attempts WHERE evidence_rowid=?1 AND status!='rate_limited' AND NOT (status='unknown' AND json_type(json,'$.retry_authorized_at') IS NOT NULL) AND (status='reserved' OR json_extract(json,'$.recipe_key')=?2 OR json_extract(json,'$.recipe_key') LIKE ?2 || '/geo:%'))",params![rowid,attempt_recipe],|r|r.get::<_,bool>(0))? { return Err(Error::conflict("This message already has a processing attempt; reuse its saved response or review its outcome.")); }
        let id = ident("attempt");
        tx.execute(
            "INSERT INTO processing_attempts VALUES(?1,?2,?3,?4,'reserved',?5,NULL,?6)",
            params![
                id,
                rowid,
                day,
                now(),
                REQUEST_RESERVE_USD,
                string(&json!({"recipe_key":if repair {TRANSLATION_REPAIR.to_owned()} else {self.processing_recipe_key()?},"repair":if repair {Some("incomplete_translation")} else {None}}))
            ],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// A distinct, once-only paid phase. Recheck eligibility and the exact saved
    /// failure under the writer lock; callers cannot turn arbitrary failures or
    /// unknown billing into retry permission.
    pub fn reserve_processing_correction(&self, rowid: i64, prior: &str) -> Result<String> {
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let policy = self.processing_policy()?;
        let mut c = self.connect()?;
        register_reading_functions(&c)?;
        let tx = c.transaction()?;
        let current = Self::latest(&tx)?.unwrap_or_default();
        let sql = format!(
            "{WORK_CTE} SELECT correction_of FROM classified WHERE rowid=?7 AND state='correctable'"
        );
        let selected: Option<String> = tx
            .query_row(
                &sql,
                params![
                    current,
                    self.processing_recipe_key()?,
                    string(&value(&policy.eligible_source_ids)),
                    RECIPE,
                    CORRECTION_RECIPE,
                    self.processing_correction_evaluated()?,
                    rowid
                ],
                |r| r.get(0),
            )
            .optional()?;
        if selected.as_deref() != Some(prior) {
            return Err(Error::conflict(
                "This reading is not eligible for another correction request",
            ));
        }
        let (raw, original): (String,String) = tx.query_row("SELECT a.json,e.text FROM processing_attempts a JOIN evidence e ON e.rowid=a.evidence_rowid WHERE a.id=?1 AND e.rowid=?2",params![prior,rowid],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let record: Value = serde_json::from_str(&raw)?;
        let audit: Option<processing_ai::ResponseAudit> =
            serde_json::from_slice(&self.original_object(required(&record, "audit_hash")?)?)?;
        let config = processing_ai::Config::from_mapping(&self.config.source_values)
            .map_err(|e| Error::invalid(e.to_string()))?;
        if let Some(audit) = audit {
            if audit.requested_model != config.model
                || ![
                    processing_ai::PROMPT_VERSION,
                    processing_ai::PREVIOUS_PROMPT_VERSION,
                ]
                .contains(&audit.prompt_version.as_str())
                || audit.source_input_hash != hash(original.as_bytes())
                || audit.response_hash != hash(&audit.raw_response)
                || audit.usage.cost_credits.is_none()
                || audit.http_status != 200
            {
                return Err(Error::conflict(
                    "The saved response does not authorize a correction for this exact input and model",
                ));
            }
            match processing_ai::replay_audit(&original, &audit) {
                Ok(_) => {
                    return Err(Error::conflict(
                        "This saved response can be recovered without another model request; recover it first",
                    ));
                }
                Err(error)
                    if matches!(
                        error.kind,
                        processing_ai::ErrorKind::Incomplete
                            | processing_ai::ErrorKind::InvalidOutput
                            | processing_ai::ErrorKind::UnsupportedQuote
                            | processing_ai::ErrorKind::AmbiguousQuote
                            | processing_ai::ErrorKind::UntranslatedEcho
                    ) => {}
                Err(_) => {
                    return Err(Error::conflict(
                        "This response failure is not eligible for a reading correction",
                    ));
                }
            }
        } else {
            if record["error"]
                != "Relic stopped waiting after its 120-second model request deadline"
                || record["receipt_model"] != config.model
            {
                return Err(Error::conflict(
                    "Only an accounted model deadline is eligible for this recovery",
                ));
            }
            let receipt: Value = serde_json::from_slice(
                &self.original_object(required(&record, "billing_receipt_hash")?)?,
            )?;
            let cost = processing_ai::receipt_cost(
                &receipt,
                required(&record, "receipt_generation_id")?,
                &config.model,
            )
            .map_err(|_| {
                Error::conflict("The retained deadline receipt does not identify this request")
            })?;
            let recorded: Option<f64> = tx.query_row(
                "SELECT cost_usd FROM processing_attempts WHERE id=?1",
                [prior],
                |r| r.get(0),
            )?;
            if recorded != Some(cost) {
                return Err(Error::conflict("Deadline cost is not reconciled"));
            }
        }
        let id = ident("attempt");
        tx.execute("INSERT INTO processing_attempts VALUES(?1,?2,?3,?4,'reserved',?5,NULL,?6)",params![id,rowid,&now()[..10],now(),REQUEST_RESERVE_USD,string(&json!({"recipe_key":CORRECTION_RECIPE,"correction_of":prior,"correction_version":processing_ai::CORRECTION_VERSION,"prior_audit_hash":record["audit_hash"]}))])?;
        tx.commit()?;
        Ok(id)
    }
    /// Atomically retain an explicit not-admitted response and its shared cooldown.
    /// Unknown billing and identified generations never enter this retry path.
    pub fn defer_processing_attempt(&self, id: &str, error: &processing_ai::AiError) -> Result<()> {
        if !error.rejected_before_generation() {
            return Err(Error::invalid(
                "This response does not establish a retryable rejection",
            ));
        }
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let bytes = serde_json::to_vec(&error.audit)?;
        let h = self.write_object(&bytes)?;
        self.sync_object_directories(std::iter::once(h.clone()), &AtomicBool::new(false))?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let prior: Value = tx
            .query_row(
                "SELECT value FROM metadata WHERE key='processing_backoff'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|v| serde_json::from_str(&v))
            .transpose()?
            .unwrap_or(json!({}));
        // A credit rejection waits longer: it clears only when the owner adds credits.
        let credits = error.audit.as_ref().is_some_and(|a| a.http_status == 402);
        let delay = error.retry_after_seconds.unwrap_or(if credits { 300 } else { 60 }).max(1);
        let until = chrono::Utc::now()
            .checked_add_signed(chrono::Duration::seconds(
                delay.min(i64::MAX as u64 / 1000) as i64
            ))
            .ok_or_else(|| Error::invalid("Invalid provider retry deadline"))?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let until = prior["retry_at"]
            .as_str()
            .filter(|v| *v > until.as_str())
            .unwrap_or(&until)
            .to_owned();
        let previous_concurrency = prior["concurrency"]
            .as_u64()
            .unwrap_or(crate::processing::CONCURRENCY as u64);
        // In-flight responses share one cooldown; reduce admission only once.
        let cooling_down = prior["retry_at"]
            .as_str()
            .is_some_and(|v| v > now().as_str());
        let concurrency = if cooling_down {
            previous_concurrency
        } else {
            (previous_concurrency / 2).max(4)
        };
        tx.execute(
            "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
            params![h, bytes.len() as i64],
        )?;
        // OpenRouter assigns a generation id header even to a pre-flight credit
        // rejection; for 402 the body check above is the proof of no generation.
        let (message, basis) = if credits {
            ("OpenRouter credits are exhausted; queued until credits are added", "explicit_402_credit_rejection_without_generation")
        } else {
            ("Rate limited; queued for retry after the provider cooldown", "explicit_429_rejection_without_generation")
        };
        let changed = tx.execute("UPDATE processing_attempts SET status='rate_limited',cost_usd=0,json=json_patch(json,?2) WHERE id=?1 AND status='reserved' AND (?3 OR json_extract(json,'$.receipt_generation_id') IS NULL)",params![id,string(&json!({"audit_hash":h,"error":message,"retry_at":until,"cost_basis":basis})),credits])?;
        if changed != 1 {
            return Err(Error::conflict(
                "Processing attempt is no longer eligible for deferred admission",
            ));
        }
        tx.execute("INSERT INTO metadata VALUES('processing_backoff',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[string(&json!({"retry_at":until,"concurrency":concurrency,"reason":if credits {"credits"} else {"rate_limit"}}))])?;
        tx.commit()?;
        Ok(())
    }
    pub fn processing_backoff(&self) -> Result<Value> {
        let saved: Option<String> = self
            .connect()?
            .query_row(
                "SELECT value FROM metadata WHERE key='processing_backoff'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(saved
            .map(|v| serde_json::from_str(&v))
            .transpose()?
            .unwrap_or(json!({"retry_at":null,"concurrency":crate::processing::CONCURRENCY})))
    }
    pub fn relax_processing_backoff(&self) -> Result<()> {
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        self.connect()?.execute("UPDATE metadata SET value=json_set(value,'$.concurrency',min(?2,coalesce(json_extract(value,'$.concurrency'),?2)+4)) WHERE key='processing_backoff' AND json_extract(value,'$.retry_at')<=?1",params![now(),crate::processing::CONCURRENCY as i64])?;
        Ok(())
    }
    pub fn finish_processing_attempt(
        &self,
        id: &str,
        status: &str,
        cost: Option<f64>,
        audit: &Value,
        error: Option<&str>,
    ) -> Result<()> {
        if !["completed", "failed", "unknown"].contains(&status)
            || cost.is_some_and(|c| !c.is_finite() || c < 0.)
        {
            return Err(Error::invalid("Invalid processing attempt outcome"));
        }
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let bytes = serde_json::to_vec(audit)?;
        let h = self.write_object(&bytes)?;
        self.sync_object_directories(std::iter::once(h.clone()), &AtomicBool::new(false))?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
            params![h, bytes.len() as i64],
        )?;
        tx.execute(
            "UPDATE processing_attempts SET status=?2,cost_usd=?3,json=json_patch(json,?4) WHERE id=?1",
            params![
                id,
                status,
                cost,
                string(&json!({"audit_hash":h,"error":error}))
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn cached_processing(&self, rowid: i64) -> Result<Option<processing_ai::Analysis>> {
        let c = self.connect()?;
        let h:Option<String>=c.query_row("SELECT json_extract(json,'$.audit_hash') FROM processing_attempts WHERE evidence_rowid=?1 AND status='completed' ORDER BY created_at DESC LIMIT 1",[rowid],|r|r.get(0)).optional()?;
        let Some(h) = h else {
            return self.reuse_unchanged_text(rowid);
        };
        let original: String =
            c.query_row("SELECT text FROM evidence WHERE rowid=?1", [rowid], |r| {
                r.get(0)
            })?;
        let analysis: processing_ai::Analysis = serde_json::from_slice(&self.original_object(&h)?)?;
        let usable = !processing_ai::reading_needs_translation(
            &original,
            &analysis.extracted.output.language,
            analysis.extracted.output.translation.as_deref(),
        ) && analysis.audit.prompt_version == processing_ai::PROMPT_VERSION
            && processing_ai::Config::from_mapping(&self.config.source_values)
                .is_ok_and(|config| analysis.audit.requested_model == config.model);
        Ok(usable.then_some(analysis))
    }

    fn reuse_unchanged_text(&self, rowid: i64) -> Result<Option<processing_ai::Analysis>> {
        let c = self.connect()?;
        let Ok(config) = processing_ai::Config::from_mapping(&self.config.source_values) else {
            return Ok(None);
        };
        // Same provider identity and source, exact text, earlier revision only.
        // No cross-message cache and no borrowed contextual/relationship decisions.
        let saved: Option<(String,String,String,String)> = c.query_row(
            "SELECT json_extract(a.json,'$.audit_hash'),old.id,old.revision,current.text FROM evidence current JOIN evidence old ON old.id=current.id AND old.source=current.source AND old.text=current.text AND old.rowid<current.rowid JOIN processing_attempts a ON a.evidence_rowid=old.rowid WHERE current.rowid=?1 AND a.status='completed' ORDER BY old.rowid DESC,a.created_at DESC LIMIT 1",[rowid],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let Some((h, id, revision, original)) = saved else {
            return Ok(None);
        };
        let saved: processing_ai::Analysis = serde_json::from_slice(&self.original_object(&h)?)?;
        let audit = &saved.audit;
        let correction = audit.parameters.get("correction_version").is_some();
        let mut recorded = audit.parameters["request_profile"].clone();
        let mut expected = processing_ai::evaluation_profile(&config.model, correction);
        // Current replay below validates decoder upgrades without repaying for
        // an unchanged generation request. Provider/prompt/schema still match.
        if let Some(profile) = recorded.as_object_mut() {
            profile.remove("decoder_version");
        }
        expected.as_object_mut().unwrap().remove("decoder_version");
        if audit.requested_model != config.model
            || audit.source_input_hash != hash(original.as_bytes())
            || recorded != expected
        {
            return Ok(None);
        }
        let Ok(mut analysis) = processing_ai::replay_audit(&original, audit) else {
            return Ok(None);
        };
        if processing_ai::reading_needs_translation(
            &original,
            &analysis.extracted.output.language,
            analysis.extracted.output.translation.as_deref(),
        ) {
            return Ok(None);
        }
        analysis.audit.parameters["reused_from"] = json!({"evidence_id":id,"revision":revision,"audit_hash":h,"model_calls":0,"basis":"exact_same_message_text_and_request_profile"});
        Ok(Some(analysis))
    }

    /// Revalidate already-paid responses once per decoder version. The original
    /// rejected response, error and cost remain retained; no request is sent.
    pub fn recover_processing(&self, cancel: &AtomicBool) -> Result<Value> {
        let config = processing_ai::Config::from_mapping(&self.config.source_values)
            .map_err(|e| Error::invalid(e.to_string()))?;
        let policy = self.processing_policy()?;
        let c = self.connect()?;
        let current = Self::latest(&c)?.unwrap_or_default();
        let rows = c.prepare("SELECT a.id,a.json,e.text FROM processing_attempts a JOIN evidence e ON e.rowid=a.evidence_rowid JOIN membership m ON m.evidence_rowid=e.rowid WHERE m.publication_id=?1 AND a.status='failed' AND a.cost_usd IS NOT NULL AND coalesce(json_extract(a.json,'$.recovery_review.decision'),'')!='hold' AND (json_extract(a.json,'$.recipe_key')=?2 OR json_extract(a.json,'$.recipe_key') LIKE ?2 || '/geo:%' OR json_extract(a.json,'$.recipe_key')=?6) AND coalesce(json_extract(a.json,'$.replay_version'),'')!=?3 AND e.source IN (SELECT value FROM json_each(?4)) ORDER BY a.created_at LIMIT ?5")?.query_map(params![current,RECIPE,RECOVERY_VERSION,string(&value(&policy.eligible_source_ids)),BATCH as i64,CORRECTION_RECIPE], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut recovered = 0;
        let examined = rows.len();
        for (id, prior, original) in rows {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let mut record: Value = serde_json::from_str(&prior)?;
            let old_hash = required(&record, "audit_hash")?.to_owned();
            // A reconciled timeout can have known cost but no response body.
            // Exact JSON null is absence, not corruption; malformed audits still fail.
            let audit: Option<processing_ai::ResponseAudit> =
                serde_json::from_slice(&self.original_object(&old_hash)?)?;
            let replay = audit
                .as_ref()
                .filter(|audit| {
                    audit.prompt_version == processing_ai::PROMPT_VERSION
                        && audit.requested_model == config.model
                        && audit.source_input_hash == hash(original.as_bytes())
                        && audit.response_hash == hash(&audit.raw_response)
                })
                .map(|audit| processing_ai::replay_audit(&original, audit));
            let _guard = self.writer.lock().map_err(|_| Error::internal())?;
            let mut c = self.connect()?;
            let tx = c.transaction()?;
            record["replay_version"] = json!(RECOVERY_VERSION);
            // The original error stays authoritative; this records why this
            // recovery version's decoder still rejects the retained response.
            let replay = match replay {
                Some(Err(error)) => {
                    record["replay_rejection"] =
                        json!({"version":RECOVERY_VERSION,"kind":error.kind,"detail":error.detail});
                    None
                }
                other => other.and_then(|result| result.ok()),
            };
            if let Some(analysis) = replay {
                let bytes = serde_json::to_vec(&analysis)?;
                let h = self.write_object(&bytes)?;
                self.sync_object_directories(std::iter::once(h.clone()), cancel)?;
                tx.execute(
                    "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
                    params![h, bytes.len() as i64],
                )?;
                record["recovery"] = json!({"at":now(),"original_status":"failed","original_error":record["error"],"original_audit_hash":old_hash,"model_calls":0});
                record["audit_hash"] = json!(h);
                record["error"] = Value::Null;
                tx.execute("UPDATE processing_attempts SET status='completed',json=?2 WHERE id=?1 AND status='failed'",params![id,string(&record)])?;
                recovered += 1;
            } else {
                tx.execute(
                    "UPDATE processing_attempts SET json=?2 WHERE id=?1 AND status='failed'",
                    params![id, string(&record)],
                )?;
            }
            tx.commit()?;
        }
        Ok(
            json!({"examined":examined,"recovered":recovered,"still_rejected":examined-recovered,"model_calls":0}),
        )
    }
    pub fn publish_processing(
        &self,
        candidates: Vec<Prepared>,
        run: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<String>> {
        Ok(self
            .publish_processing_count(candidates, run, cancel)?
            .map(|(id, _)| id))
    }
    fn publish_processing_count(
        &self,
        candidates: Vec<Prepared>,
        run: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<(String, usize)>> {
        if candidates.is_empty() {
            return Ok(None);
        }
        if candidates.len() > BATCH {
            return Err(Error::invalid("Processing publication exceeds batch bound"));
        }
        let _guard = self.writer.lock().map_err(|_| Error::internal())?;
        let mut retained = vec![];
        for candidate in candidates {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            if let Some(object_hash) = candidate.retained_object.as_ref() {
                let bytes = self.original_object(object_hash)?;
                let wrapper: Value = serde_json::from_slice(&bytes)?;
                if wrapper["interpretation"] != candidate.interpretation
                    || wrapper["audit"] != candidate.audit
                {
                    return Err(Error::conflict(
                        "The retained reading object differs from the selected reading",
                    ));
                }
                retained.push((candidate, hash(&bytes), bytes.len()));
            } else {
                let bytes = serde_json::to_vec(
                    &json!({"interpretation":candidate.interpretation,"audit":candidate.audit}),
                )?;
                let hash = self.write_object(&bytes)?;
                retained.push((candidate, hash, bytes.len()));
            }
        }
        self.sync_object_directories(retained.iter().map(|(_, h, _)| h.clone()), cancel)?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let prior = Self::latest(&tx)?
            .ok_or_else(|| Error::missing("No evidence publication to process"))?;
        let mut current = Vec::with_capacity(retained.len());
        for (candidate, object_hash, bytes) in retained {
            let i = &candidate.interpretation;
            let exact: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM evidence WHERE rowid=?1 AND id=?2 AND revision=?3 AND text IS NOT NULL)",params![candidate.rowid,i["evidence_id"].as_str(),i["revision"].as_str()],|r|r.get(0))?;
            if !exact {
                return Err(Error::conflict(
                    "Interpretation does not identify an exact retained revision",
                ));
            }
            let member: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM membership WHERE publication_id=?1 AND evidence_rowid=?2)",params![prior,candidate.rowid],|r|r.get(0))?;
            if !member {
                continue;
            }
            if let Some(expected) = candidate.expected_prior.as_deref() {
                let still_current: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM publication_interpretations WHERE publication_id=?1 AND evidence_rowid=?2 AND interpretation_id=?3)",
                    params![prior,candidate.rowid,expected], |r| r.get(0),
                )?;
                if !still_current {
                    continue;
                }
            }
            current.push((candidate, object_hash, bytes));
        }
        let retained = current;
        let selected_count = retained.len();
        if selected_count == 0 {
            return Ok(None);
        }
        let geography = self.geography()?;
        let boundaries = self.boundaries()?;
        let marine = self.marine()?;
        let osm = self.osm()?;
        if let Some(edition) = crate::boundary_store::combined_edition(
            geography.as_ref(),
            boundaries.as_ref(),
            marine.as_ref(),
            osm.as_ref(),
        ) && !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM lineage_nodes WHERE id=?1)",
            [format!("geography:{edition}")],
            |r| r.get::<_, bool>(0),
        )? {
            crate::boundary_store::record_context(
                &tx,
                geography.as_ref(),
                boundaries.as_ref(),
                marine.as_ref(),
                osm.as_ref(),
            )?;
        }
        // A resolver can outlive activation. Historical accepted contexts remain
        // valid, but never substitute today's edition for a missing old context.
        for (candidate, _, _) in &retained {
            if let Some(edition) = candidate.interpretation["reference_edition"].as_str()
                && !tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM lineage_nodes WHERE id=?1)",
                    [format!("geography:{edition}")],
                    |r| r.get::<_, bool>(0),
                )?
            {
                return Err(Error::conflict(
                    "The reading's geographic reference context is unavailable; re-resolve its saved output against an accepted edition",
                ));
            }
        }
        let publication = ident("publication");
        let created = now();
        tx.execute(
            "INSERT INTO publications VALUES(?1,'Processed messages',?2,'complete',NULL)",
            params![publication, created],
        )?;
        crate::publication_history::inherit(&tx, &publication, Some(&prior))?;
        let geography_only = retained.iter().all(|(p, _, _)| {
            p.retained_object.is_some() || p.interpretation["processing_mode"] == "geography_only"
        });
        node(
            &tx,
            run,
            "run_step",
            if geography_only {
                "Re-resolve retained place mentions"
            } else {
                "Translate and extract supported reporting"
            },
            &json!({"recipe":RECIPE,"model":processing_ai::DEFAULT_MODEL,"batch":retained.len(),"geography_only":geography_only}),
        )?;
        node(
            &tx,
            &publication,
            "publication",
            "Processed messages",
            &json!({"status":"complete","recipe":RECIPE,"created_at":created,"meaning":"Validated interpretations; unprocessed messages remain in the same evidence scope"}),
        )?;
        let mut selected = Vec::with_capacity(retained.len());
        for (candidate, h, bytes) in retained {
            let i = &candidate.interpretation;
            let id = required(i, "id")?;
            selected.push((candidate.rowid, id.to_owned()));
            if candidate.retained_object.is_some() {
                let stored: Option<String> = tx.query_row(
                    "SELECT json FROM interpretations WHERE id=?1 AND evidence_rowid=?2 AND object_hash=?3",
                    params![id, candidate.rowid, h], |r| r.get(0),
                ).optional()?;
                if stored
                    .map(|raw| serde_json::from_str::<Value>(&raw))
                    .transpose()?
                    .as_ref()
                    != Some(i)
                {
                    return Err(Error::conflict(
                        "The retained reading changed; no geography publication was committed",
                    ));
                }
                edge(&tx, id, run, "reused_reading", &json!({"object_hash":h}))?;
                edge(
                    &tx,
                    id,
                    &publication,
                    "published",
                    &json!({"exact_revision":i["revision"]}),
                )?;
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO objects VALUES(?1,?2,'application/json')",
                params![h, bytes as i64],
            )?;
            tx.execute(
                "INSERT INTO interpretations VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    id,
                    candidate.rowid,
                    interpretation_recipe_key(i, i["reference_edition"].as_str())?,
                    created,
                    required(i, "kind")?,
                    i["geometry"]["coordinates"][0].as_f64(),
                    i["geometry"]["coordinates"][1].as_f64(),
                    i["related_key"].as_str(),
                    string(i),
                    h
                ],
            )?;
            let evidence = format!(
                "{}@{}",
                required(i, "evidence_id")?,
                required(i, "revision")?
            );
            node(
                &tx,
                id,
                "interpretation",
                required(i, "headline")?,
                &json!({"interpretation_id":id,"evidence_id":i["evidence_id"],"revision":i["revision"],"audit_hash":h,"recipe":i["recipe"]}),
            )?;
            if let Some(reused) = i.get("reused_from") {
                let prior = format!(
                    "{}@{}",
                    required(reused, "evidence_id")?,
                    required(reused, "revision")?
                );
                edge(&tx, &prior, id, "reading_reused", reused)?;
            }
            if let Some(prior) = i["replaces_reading"].as_str() {
                edge(
                    &tx,
                    prior,
                    id,
                    "translation_repaired",
                    &json!({"original_preserved":true,"full_reading_revalidated":true}),
                )?;
            }
            if let Some(prior) = i["prior_interpretation_id"].as_str() {
                edge(
                    &tx,
                    prior,
                    id,
                    "geography_reinterpreted",
                    &json!({"source_reading_unchanged":true}),
                )?;
            }
            if let Some(edition) = i["reference_edition"].as_str() {
                edge(
                    &tx,
                    &format!("geography:{edition}"),
                    id,
                    "reference_used",
                    &json!({"edition":edition}),
                )?;
            }
            edge(
                &tx,
                &evidence,
                run,
                "input_record",
                &json!({"source_input_hash":i["source_input_hash"]}),
            )?;
            edge(&tx, run, id, "produced", &json!({"object_hash":h}))?;
            edge(
                &tx,
                &evidence,
                id,
                "supports",
                &json!({"claims":i["claims"],"locations":i["locations"]}),
            )?;
            edge(
                &tx,
                id,
                &publication,
                "published",
                &json!({"exact_revision":i["revision"]}),
            )?;
        }
        carry_interpretations(&tx, &publication)?;
        // Explicit choices win over creation order, including a return to an
        // older geography edition. Future imports inherit this exact selection.
        for (rowid, id) in selected {
            tx.execute(
                "INSERT INTO publication_interpretations VALUES(?1,?2,?3)",
                params![publication, rowid, id],
            )?;
        }
        edge(
            &tx,
            run,
            &publication,
            "published",
            &json!({"geography_only":geography_only}),
        )?;
        edge(
            &tx,
            &prior,
            &publication,
            "extends_publication",
            &json!({"meaning":"Raw evidence unchanged; exact interpretations pinned"}),
        )?;
        self.evaluate_watches(&tx, &publication)?;
        tx.execute(
            "UPDATE metadata SET value=?1 WHERE key='current_publication'",
            [&publication],
        )?;
        let ev = Self::event(&tx, "processing", Some(&publication), vec![run.into()])?;
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        tx.commit()?;
        let _ = self.events.send(ev);
        Ok(Some((publication, selected_count)))
    }
    /// Reuse validated, paid source readings. Preserve their original recipe and
    /// model identity; geography-only work does not pretend to upgrade semantics.
    pub fn reresolve_geography(&self, run: &str, cancel: &AtomicBool) -> Result<Value> {
        let resolver = GeoResolver::load_for_store_cancellable(self, cancel)?;
        let edition = resolver
            .edition()
            .ok_or_else(|| Error::conflict("Activate a geography edition first"))?
            .to_owned();
        let upper: i64 = self.connect()?.query_row("SELECT coalesce(max(evidence_rowid),0) FROM publication_interpretations WHERE publication_id=(SELECT value FROM metadata WHERE key='current_publication')", [], |r|r.get(0))?;
        let mut count = 0;
        let mut cursor = 0_i64;
        let mut conflicts = 0_usize;
        let mut changed_during_run = 0_usize;
        let mut examples = Vec::new();
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::cancelled());
            }
            let c = self.connect()?;
            let current = Self::latest(&c)?.unwrap_or_default();
            let rows=c.prepare("SELECT e.rowid,e.json,i.json,i.object_hash,coalesce((SELECT json_extract(a.json,'$.audit_hash') FROM processing_attempts a WHERE a.evidence_rowid=e.rowid AND a.status='completed' ORDER BY a.created_at DESC LIMIT 1),json_extract(i.json,'$.reused_from.audit_hash')) FROM publication_interpretations pi JOIN evidence e ON e.rowid=pi.evidence_rowid JOIN interpretations i ON i.id=pi.interpretation_id WHERE pi.publication_id=?1 AND coalesce(json_extract(i.json,'$.reference_edition'),'')!=?2 AND e.rowid>?4 AND e.rowid<=?5 ORDER BY e.rowid LIMIT ?3")?.query_map(params![current,edition,BATCH as i64,cursor,upper],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<String>>(4)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            if rows.is_empty() {
                break;
            }
            let mut prepared = vec![];
            for (rowid, e, old, interpretation_hash, audit_hash) in rows {
                cursor = rowid;
                if cancel.load(Ordering::Acquire) {
                    return Err(Error::cancelled());
                }
                let e: Value = serde_json::from_str(&e)?;
                let old: Value = serde_json::from_str(&old)?;
                let Some(audit_hash) = audit_hash else {
                    conflicts += 1;
                    if examples.len() < 20 {
                        examples.push(json!({"evidence_id":e["id"],"revision":e["revision"],"reason":"No retained analysis receipt supports geographic replay"}));
                    }
                    continue;
                };
                let analysis: processing_ai::Analysis =
                    serde_json::from_slice(&self.original_object(&audit_hash)?)?;
                let retained: Value =
                    serde_json::from_slice(&self.original_object(&interpretation_hash)?)?;
                if retained["interpretation"] != old
                    || retained["audit"]["response_hash"] != analysis.audit.response_hash
                    || analysis.audit.source_input_hash != hash(required(&e, "text")?.as_bytes())
                {
                    conflicts += 1;
                    if examples.len() < 20 {
                        examples.push(json!({"evidence_id":e["id"],"revision":e["revision"],"reason":"Retained reading and cached response differ; evaluated processing upgrade required"}));
                    }
                    continue;
                }
                let key = interpretation_recipe_key(&old, Some(&edition))?;
                let cached: Option<(String, String)> = c.query_row(
                    "SELECT json,object_hash FROM interpretations WHERE evidence_rowid=?1 AND recipe=?2",
                    params![rowid,key], |r| Ok((r.get(0)?,r.get(1)?)),
                ).optional()?;
                if let Some((cached, object_hash)) = cached {
                    let cached: Value = serde_json::from_str(&cached)?;
                    let wrapper: Value =
                        serde_json::from_slice(&self.original_object(&object_hash)?)?;
                    if wrapper["interpretation"] != cached
                        || wrapper["audit"]["response_hash"] != analysis.audit.response_hash
                        || reading_content(&cached) != reading_content(&old)
                    {
                        conflicts += 1;
                        if examples.len() < 20 {
                            examples.push(json!({"evidence_id":e["id"],"revision":e["revision"],"reason":"Earlier geography edition contains a different source reading; previous semantics retained"}));
                        }
                        continue;
                    }
                    prepared.push(Prepared {
                        rowid,
                        interpretation: cached,
                        audit: wrapper["audit"].clone(),
                        retained_object: Some(object_hash),
                        expected_prior: Some(required(&old, "id")?.to_owned()),
                    });
                    continue;
                }
                let mut p = reproject(rowid, &e, &old, &analysis.extracted, &resolver)?;
                p.audit = value(&analysis.audit);
                prepared.push(p);
            }
            let n = prepared.len();
            let published = self
                .publish_processing_count(prepared, &format!("{run}:batch:{cursor}"), cancel)?
                .map_or(0, |(_, n)| n);
            count += published;
            changed_during_run += n - published;
        }
        Ok(
            json!({"resolved_readings":count,"model_calls":0,"reference_edition":edition,"conflicting_readings":conflicts,"conflict_examples":examples,"changed_during_run":changed_during_run,"complete":conflicts==0 && changed_during_run==0}),
        )
    }

    pub fn detail_with_publication(
        &self,
        id: &str,
        revision: &str,
        publication: Option<&str>,
    ) -> Result<Value> {
        let mut e = self.detail(id, revision)?;
        let c = self.connect()?;
        let latest = Self::latest(&c)?;
        let historical = publication.is_some_and(|id| Some(id) != latest.as_deref());
        let publication = publication.map(str::to_string).or(latest);
        if let Some(publication) = publication {
            crate::media_context::attach(&c, &publication, &mut e)?;
            let raw:Option<String>=c.query_row("SELECT i.json FROM publication_interpretations pi JOIN evidence e ON e.rowid=pi.evidence_rowid JOIN interpretations i ON i.id=pi.interpretation_id WHERE pi.publication_id=?1 AND e.id=?2 AND e.revision=?3",params![publication,id,revision],|r|r.get(0)).optional()?;
            e["interpretation"] = raw
                .map(|s| serde_json::from_str(&s))
                .transpose()?
                .unwrap_or(Value::Null);
            e["supplemental"] = self.supplemental_detail(
                id,
                revision,
                e["interpretation"]["id"].as_str(),
                historical,
                &publication,
            )?;
            e["interpretation_publication_id"] = json!(publication);
            let scope = Scope {
                publication_id: Some(publication.clone()),
                ..Scope::default()
            };
            let vals = vec![
                rusqlite::types::Value::Text(publication),
                rusqlite::types::Value::Text(id.into()),
                rusqlite::types::Value::Text(revision.into()),
            ];
            let sites = crate::geographic_view::sites(
                &c,
                &scope,
                "e.publication_id=? AND e.id=? AND e.revision=?",
                &vals,
                25,
            )?;
            crate::geographic_view::attach_sites(&mut e, &sites);
            crate::area_view::attach(&c, &scope, std::slice::from_mut(&mut e))?;
            // Hints follow Jev's newest answers, so past views do not show them.
            if !historical {
                crate::jev_hints::attach(&c, std::slice::from_mut(&mut e), None)?;
            }
        }
        Ok(e)
    }
}

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};

/// WORK_CTE parameters other than the publication. A change reclassifies
/// every message.
#[derive(Clone, PartialEq)]
struct StatusParams {
    recipe_key: String,
    sources: String,
    correction: bool,
}
/// What WORK_CTE reads for one message besides its evidence row, which is
/// never updated: the reading(s) its `work` join selects and a digest of every
/// column of every processing attempt of that message.
#[derive(Default, PartialEq)]
struct StatusInputs {
    readings: Vec<Option<String>>,
    attempts: Option<[u8; 32]>,
}
/// One `classified` row; a message has one per `work` row.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct StatusClass {
    state: String,
    reading: bool,
    interrupted: bool,
}
/// Classes of each message in the current publication, valid for `params`.
/// WORK_CTE classifies a message only from its evidence revision, the reading
/// its `work` join selects, its own attempts and the bound parameters. An
/// entry is kept only while its recorded inputs equal the snapshot's, so the
/// counts equal a full WORK_CTE pass over the same snapshot; debug builds
/// check this on every refresh.
#[derive(Default)]
struct StatusMemo {
    params: Option<StatusParams>,
    rows: HashMap<i64, (StatusInputs, Vec<StatusClass>)>,
}
type SharedStatusMemo = Arc<Mutex<StatusMemo>>;
/// A store's writer lock, which all clones of that `Store` share.
type StoreIdentity = Weak<Mutex<()>>;
/// One memo per open store. The weak reference keeps the writer lock's
/// allocation alive, so a later store never matches a dropped one.
static STATUS_MEMOS: LazyLock<Mutex<Vec<(StoreIdentity, SharedStatusMemo)>>> =
    LazyLock::new(Default::default);
fn status_memo(owner: &Arc<Mutex<()>>) -> Result<SharedStatusMemo> {
    let owner = Arc::downgrade(owner);
    let mut memos = STATUS_MEMOS.lock().map_err(|_| Error::internal())?;
    memos.retain(|(store, _)| store.strong_count() > 0);
    if let Some((_, memo)) = memos.iter().find(|(store, _)| store.ptr_eq(&owner)) {
        return Ok(memo.clone());
    }
    let memo = SharedStatusMemo::default();
    memos.push((owner, memo.clone()));
    Ok(memo)
}
fn status_digest(h: &mut Sha256, value: rusqlite::types::ValueRef<'_>) {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => h.update([0]),
        ValueRef::Integer(n) => {
            h.update([1]);
            h.update(n.to_le_bytes());
        }
        ValueRef::Real(n) => {
            h.update([2]);
            h.update(n.to_bits().to_le_bytes());
        }
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            h.update([if matches!(value, ValueRef::Text(_)) { 3 } else { 4 }]);
            h.update((bytes.len() as u64).to_le_bytes());
            h.update(bytes);
        }
    }
}
/// Classes from WORK_CTE, for the listed messages or for all of them.
fn status_classes(
    c: &Connection,
    pubid: &str,
    p: &StatusParams,
    only: Option<&[i64]>,
) -> Result<HashMap<i64, Vec<StatusClass>>> {
    let select = format!(
        "{WORK_CTE} SELECT w.rowid,w.state,w.interpretation_id IS NOT NULL,w.state='failed' AND EXISTS(SELECT 1 FROM processing_attempts a WHERE a.evidence_rowid=w.rowid AND a.status='unknown' AND json_type(a.json,'$.retry_authorized_at') IS NULL) FROM classified w"
    );
    let sql = match only {
        Some(_) => format!("{select} WHERE w.rowid IN (SELECT value FROM json_each(?7))"),
        None => select,
    };
    let mut stmt = c.prepare_cached(&sql)?;
    let base = params![pubid, p.recipe_key, p.sources, RECIPE, CORRECTION_RECIPE, p.correction];
    let mut rows = match only {
        Some(ids) => {
            let ids = string(&value(ids));
            let mut bound: Vec<&dyn rusqlite::ToSql> = base.to_vec();
            bound.push(&ids);
            stmt.query(bound.as_slice())?
        }
        None => stmt.query(base)?,
    };
    let mut classes: HashMap<i64, Vec<StatusClass>> = HashMap::new();
    while let Some(r) = rows.next()? {
        classes.entry(r.get(0)?).or_default().push(StatusClass {
            state: r.get(1)?,
            reading: r.get(2)?,
            interrupted: r.get(3)?,
        });
    }
    for list in classes.values_mut() {
        list.sort();
    }
    Ok(classes)
}
impl StatusMemo {
    fn refresh(&mut self, c: &Connection, pubid: &str, p: &StatusParams) -> Result<()> {
        if self.params.as_ref() != Some(p) {
            self.rows.clear();
            self.params = Some(p.clone());
        }
        // The rows of `work`: its membership join LEFT JOIN the reading
        // intervals covering the publication, read as two range scans instead
        // of one lookup per message. Its other joins match exactly one
        // evidence row (a foreign key) and at most one interpretation (a
        // primary key), so they add no rows.
        let mut covering: HashMap<i64, Vec<Option<String>>> = HashMap::new();
        {
            let mut stmt = c.prepare_cached("SELECT pi.evidence_rowid,pi.interpretation_id FROM publication_order p JOIN interpretation_intervals pi ON pi.from_seq<=p.seq AND (pi.until_seq IS NULL OR pi.until_seq>p.seq) WHERE p.id=?1")?;
            let mut rows = stmt.query([pubid])?;
            while let Some(r) = rows.next()? {
                covering.entry(r.get(0)?).or_default().push(r.get(1)?);
            }
        }
        let mut inputs: HashMap<i64, StatusInputs> = HashMap::new();
        {
            let mut stmt = c.prepare_cached(
                "SELECT evidence_rowid FROM membership WHERE publication_id=?1",
            )?;
            let mut rows = stmt.query([pubid])?;
            while let Some(r) = rows.next()? {
                let rowid: i64 = r.get(0)?;
                let readings = &mut inputs.entry(rowid).or_default().readings;
                match covering.get(&rowid) {
                    Some(found) => readings.extend(found.iter().cloned()),
                    None => readings.push(None),
                }
            }
        }
        for input in inputs.values_mut() {
            input.readings.sort();
        }
        {
            let mut stmt = c.prepare_cached("SELECT evidence_rowid,id,day,created_at,status,reserved_usd,cost_usd,json FROM processing_attempts ORDER BY evidence_rowid,id")?;
            let mut rows = stmt.query([])?;
            let mut current: Option<(i64, Sha256)> = None;
            let finish = |current: Option<(i64, Sha256)>,
                              inputs: &mut HashMap<i64, StatusInputs>| {
                if let Some((rowid, h)) = current
                    && let Some(input) = inputs.get_mut(&rowid)
                {
                    input.attempts = Some(h.finalize().into());
                }
            };
            while let Some(r) = rows.next()? {
                let rowid: i64 = r.get(0)?;
                if current.as_ref().is_none_or(|(id, _)| *id != rowid) {
                    finish(current.take(), &mut inputs);
                    current = inputs.contains_key(&rowid).then(|| (rowid, Sha256::new()));
                }
                if let Some((_, h)) = current.as_mut() {
                    for column in 1..8 {
                        status_digest(h, r.get_ref(column)?);
                    }
                }
            }
            finish(current, &mut inputs);
        }
        self.rows
            .retain(|rowid, (kept, _)| inputs.get(rowid).is_some_and(|now| now == kept));
        let changed: Vec<i64> = inputs
            .keys()
            .filter(|rowid| !self.rows.contains_key(rowid))
            .copied()
            .collect();
        if !changed.is_empty() {
            let only = (changed.len() * 2 < inputs.len()).then_some(changed.as_slice());
            let mut classes = status_classes(c, pubid, p, only)?;
            for rowid in changed {
                let input = inputs.remove(&rowid).ok_or_else(Error::internal)?;
                // Exactly what WORK_CTE yields for these inputs, possibly no row.
                let class = classes.remove(&rowid).unwrap_or_default();
                self.rows.insert(rowid, (input, class));
            }
        }
        #[cfg(debug_assertions)]
        {
            let full = status_classes(c, pubid, p, None)?;
            let memo: HashMap<i64, Vec<StatusClass>> = self
                .rows
                .iter()
                .filter(|(_, (_, class))| !class.is_empty())
                .map(|(rowid, (_, class))| (*rowid, class.clone()))
                .collect();
            assert!(
                full == memo,
                "Memoized processing status differs from a full WORK_CTE pass"
            );
        }
        Ok(())
    }
    /// `(state, messages, with a reading)` per state, the interrupted count
    /// and the messages classified failed.
    fn counts(&self) -> (Vec<(String, i64, i64)>, i64, Vec<i64>) {
        let mut groups: Vec<(String, i64, i64)> = vec![];
        let mut interrupted = 0;
        let mut failed = vec![];
        for (rowid, (_, classes)) in &self.rows {
            for class in classes {
                let at = match groups.iter().position(|(s, _, _)| *s == class.state) {
                    Some(at) => at,
                    None => {
                        groups.push((class.state.clone(), 0, 0));
                        groups.len() - 1
                    }
                };
                groups[at].1 += 1;
                groups[at].2 += i64::from(class.reading);
                interrupted += i64::from(class.interrupted);
            }
            if classes.iter().any(|class| class.state == "failed") {
                failed.push(*rowid);
            }
        }
        failed.sort();
        (groups, interrupted, failed)
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use crate::config::Config;
    use std::sync::Arc;

    #[test]
    fn delayed_geography_never_overwrites_a_newer_reading() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let store = Store::open(Config::local(
            root,
            temp.path().join("data"),
            "127.0.0.1:0".parse().unwrap(),
        ))
        .unwrap();
        let text = "Fictional fire in London.";
        let evidence = json!({"id":"fiction:race","revision":"v1","source":"fiction:source","published_at":"2026-09-23T12:00:00Z","acquired_at":"2026-09-23T12:01:00Z","text":text,"geometry":null,"location_precision":"unresolved","fictional":true,"availability":"available","language":"en","dependence":"unknown","origin_group":null,"citation":{"evidence_id":"fiction:race","revision":"v1","start_character":0,"end_character":text.len()}});
        store.import_job(json!({"schema_version":1,"fictional":true,"title":"Concurrent reading","sources":[{"id":"fiction:source","label":"Fictional source","family":"authored_report"}],"evidence_revisions":[evidence]}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
        let rowid: i64 = store
            .connect()
            .unwrap()
            .query_row("SELECT rowid FROM evidence", [], |r| r.get(0))
            .unwrap();
        let output = processing_ai::validate_output(text,&json!({"language":"en","translation":null,"headline":text,"kind":"event","claim_status":"reported","topic":"safety","claims":[{"text":text,"quote":text,"state":"reported"}],"locations":[{"mention":"London","quote":text,"role":"event","canonical_name":"London","country_code":"GB"}]})).unwrap();
        let resolver = GeoResolver::load(&store.config.root).unwrap();
        let first = prepare(rowid, &evidence, &output, &resolver, "fixture/gold").unwrap();
        let old = first.interpretation.clone();
        let stop = AtomicBool::new(false);
        store
            .publish_processing(vec![first], &ident("processing"), &stop)
            .unwrap();
        let delayed = reproject(rowid, &evidence, &old, &output, &resolver).unwrap();
        let mut newer = prepare(rowid, &evidence, &output, &resolver, "fixture/gold").unwrap();
        newer.interpretation["recipe"] = json!("fixture/newer-reading");
        let new = newer.interpretation.clone();
        let publication = store
            .publish_processing(vec![newer], &ident("processing"), &stop)
            .unwrap()
            .unwrap();
        let object_hash: String = store
            .connect()
            .unwrap()
            .query_row(
                "SELECT object_hash FROM interpretations WHERE id=?1",
                [required(&old, "id").unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        let reused = Prepared {
            rowid,
            interpretation: old.clone(),
            audit: Value::Null,
            retained_object: Some(object_hash),
            expected_prior: Some(required(&old, "id").unwrap().into()),
        };
        for stale in [delayed, reused] {
            assert!(
                store
                    .publish_processing(vec![stale], &ident("geography"), &stop)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                Store::latest(&store.connect().unwrap()).unwrap().as_deref(),
                Some(publication.as_str())
            );
            assert_eq!(
                store
                    .detail_with_publication("fiction:race", "v1", None)
                    .unwrap()["interpretation"],
                new
            );
        }
        let stale = reproject(rowid, &evidence, &new, &output, &resolver).unwrap();
        let mut revision = evidence.clone();
        revision["revision"] = json!("v2");
        revision["published_at"] = json!("2026-09-23T13:00:00Z");
        revision["citation"]["revision"] = json!("v2");
        let mut peer = evidence.clone();
        peer["id"] = json!("fiction:peer");
        peer["citation"]["evidence_id"] = peer["id"].clone();
        store.import_job(json!({"schema_version":1,"fictional":true,"title":"Concurrent revisions","sources":[{"id":"fiction:source","label":"Fictional source","family":"authored_report"}],"evidence_revisions":[revision,peer]}),ident("job"),Arc::new(AtomicBool::new(false))).unwrap();
        let peer_rowid: i64 = store
            .connect()
            .unwrap()
            .query_row(
                "SELECT rowid FROM evidence WHERE id='fiction:peer'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let valid = prepare(peer_rowid, &peer, &output, &resolver, "fixture/gold").unwrap();
        let result = store
            .publish_processing_count(vec![stale, valid], &ident("processing"), &stop)
            .unwrap()
            .unwrap();
        assert_eq!(result.1, 1);
        assert!(
            store
                .detail_with_publication("fiction:peer", "v1", None)
                .unwrap()["interpretation"]
                .is_object()
        );
        assert!(
            store
                .detail_with_publication("fiction:race", "v2", None)
                .unwrap()["interpretation"]
                .is_null()
        );
    }
    #[test]
    fn late_collection_preserves_reselected_reading_and_exact_legacy_object() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let store = Store::open(Config::local(
            root,
            temp.path().join("data"),
            "127.0.0.1:0".parse().unwrap(),
        ))
        .unwrap();
        let subscription = store.create_subscription(json!({"provider":"x","label":"Synthetic history","target":"@synthetic_qa","enabled":true})).unwrap();
        let id = required(&subscription, "id").unwrap();
        let publish = |text: &str, at: &str| {
            let messages = vec![
                json!({"id":"x:post:1","text":text,"published_at":at,"media":[],"relationships":{}}),
            ];
            store
                .publish_collection_page(
                    id,
                    1,
                    json!({"id":"x:user:1","label":"Synthetic source"}),
                    messages.clone(),
                    json!({"messages":messages,"synthetic":true}),
                    store.collection_state(id).unwrap(),
                    Arc::new(AtomicBool::new(false)),
                )
                .unwrap();
        };
        let text = "Synthetic fire in London.";
        publish(text, "2026-09-23T12:00:00Z");
        let c = store.connect().unwrap();
        let (rowid, raw): (i64, String) = c
            .query_row("SELECT rowid,json FROM evidence", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        let e: Value = serde_json::from_str(&raw).unwrap();
        let output = processing_ai::validate_output(text,&json!({"language":"en","translation":null,"headline":text,"kind":"event","claim_status":"reported","topic":"safety","claims":[{"text":text,"quote":text,"state":"reported","event_type":"fire","time_text":null,"location_indices":[0]}],"locations":[{"mention":"London","quote":text,"role":"event","canonical_name":"London","country_code":"GB"}]})).unwrap();
        let resolver = GeoResolver::load(&store.config.root).unwrap();
        let a = prepare(rowid, &e, &output, &resolver, "fixture/gold").unwrap();
        let a_value = a.interpretation.clone();
        let stop = AtomicBool::new(false);
        store
            .publish_processing(vec![a], &ident("processing"), &stop)
            .unwrap();
        let mut b = prepare(rowid, &e, &output, &resolver, "fixture/gold").unwrap();
        b.interpretation["recipe"] = json!("fixture/geography-b");
        let b_value = b.interpretation.clone();
        store
            .publish_processing(vec![b], &ident("processing"), &stop)
            .unwrap();
        // Simulate a retained legacy wrapper: different whitespace/extra audit
        // metadata must not force a new object or a false hash conflict.
        let bytes = serde_json::to_vec_pretty(
            &json!({"interpretation":a_value,"audit":null,"legacy_wrapper":"preserve me"}),
        )
        .unwrap();
        let object = store.write_object(&bytes).unwrap();
        c.execute(
            "INSERT INTO objects VALUES(?1,?2,'application/json')",
            params![object, bytes.len() as i64],
        )
        .unwrap();
        c.execute(
            "UPDATE interpretations SET object_hash=?2,json=?3 WHERE id=?1",
            params![
                required(&a_value, "id").unwrap(),
                object,
                serde_json::to_string_pretty(&a_value).unwrap()
            ],
        )
        .unwrap();
        let again = Prepared {
            rowid,
            interpretation: a_value.clone(),
            audit: Value::Null,
            retained_object: Some(object.clone()),
            expected_prior: Some(required(&b_value, "id").unwrap().into()),
        };
        store
            .publish_processing(vec![again], &ident("geography"), &stop)
            .unwrap();
        publish(
            "Older synthetic fire report from London.",
            "2026-09-23T11:00:00Z",
        );
        let current = store
            .detail_with_publication(
                required(&e, "id").unwrap(),
                required(&e, "revision").unwrap(),
                None,
            )
            .unwrap();
        assert_eq!(current["interpretation"], a_value);
        let retained: String = c
            .query_row(
                "SELECT object_hash FROM interpretations WHERE id=?1",
                [required(&a_value, "id").unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(retained, object);
        assert_eq!(store.original_object(&retained).unwrap(), bytes);
    }
}
