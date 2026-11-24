//! Area identity selection supplements the name resolver without making points.
use crate::{
    boundary_store::AreaRecord,
    processing_geo::{
        LocationCandidate, LocationProposal, LocationResolution, LocationRole, ReferenceIdentity,
    },
};
use serde_json::json;
fn normalized(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn administrative_name(s: &str) -> Option<String> {
    let n = normalized(s);
    for suffix in [" province", " governorate", " region", " state"] {
        if let Some(name) = n.strip_suffix(suffix) {
            return Some(name.to_owned());
        }
    }
    for prefix in ["province of ", "governorate of ", "region of ", "state of "] {
        if let Some(name) = n.strip_prefix(prefix) {
            return Some(name.to_owned());
        }
    }
    None
}
pub fn resolve(
    areas: &[AreaRecord],
    p: &LocationProposal,
    allowed: bool,
    mut base: LocationResolution,
) -> LocationResolution {
    if areas.is_empty()
        || [
            "unsupported_quote",
            "country_conflict",
            "canonical_conflict",
        ]
        .contains(&base.status.as_str())
    {
        return base;
    }
    let country = base
        .candidates
        .iter()
        .find(|c| c.kind == "country" && Some(&c.id) == base.selected_candidate_id.as_ref())
        .and_then(|c| c.country_code.as_deref());
    let qualifier = administrative_name(&p.mention);
    let name = qualifier.clone().unwrap_or_else(|| normalized(&p.mention));
    if country.is_none()
        && qualifier.is_none()
        && !["unresolved", "proposal_only"].contains(&base.status.as_str())
    {
        return base;
    }
    let matches = areas
        .iter()
        .filter(|a| {
            if let Some(country) = country {
                a.level == "ADM0" && a.iso2 == country
            } else {
                a.level == "ADM1"
                    && normalized(&a.name) == name
                    && p.country_code
                        .as_ref()
                        .is_none_or(|c| c.eq_ignore_ascii_case(&a.iso2))
            }
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return base;
    }
    let name_support = base
        .candidates
        .iter()
        .find(|c| Some(&c.id) == base.selected_candidate_id.as_ref())
        .cloned();
    base.candidates = matches
        .iter()
        .take(20)
        .map(|a| LocationCandidate {
            id: a.id.clone(),
            name: a.name.clone(),
            kind: if a.level == "ADM0" {
                "country"
            } else {
                "administrative_area"
            }
            .into(),
            country_code: Some(a.iso2.clone()),
            country_name: a.metadata["boundaryName"].as_str().map(str::to_owned),
            geometry: None,
            precision: if a.level == "ADM0" {
                "country_area"
            } else {
                "administrative_area"
            }
            .into(),
            reference: ReferenceIdentity {
                dataset_id: "geoboundaries-gbOpen".into(),
                publisher: "geoBoundaries".into(),
                edition: a.edition.clone(),
                product_version: a.metadata["boundaryID"].as_str().unwrap_or("").into(),
                source_path: a.metadata["gjDownloadURL"].as_str().unwrap_or("").into(),
                source_sha256: a.source_sha256.clone(),
                publisher_record_id: a.shape_id.clone(),
                record_sha256: a.record_sha256.clone(),
                acceptance_id: format!("boundaries:{}", a.edition),
                acceptance_sha256: a.edition.clone(),
                represented_time: a.metadata["boundaryYearRepresented"]
                    .as_str()
                    .unwrap_or("")
                    .into(),
            },
            match_basis: if country.is_some() {
                "accepted_country_code_identity"
            } else {
                "literal_administrative_name"
            }
            .into(),
            matched_alias: Some(p.mention.clone()),
            alias_reference: name_support
                .as_ref()
                .map(|c| json!({"id":c.id,"reference":c.reference})),
            country_constraint_match: p
                .country_code
                .as_ref()
                .map(|c| c.eq_ignore_ascii_case(&a.iso2)),
        })
        .collect();
    base.candidate_count = matches.len();
    base.candidates_truncated = matches.len() > 20;
    base.geometry = None;
    base.area = None;
    base.selected_candidate_id = None;
    base.occurrence_eligible = false;
    if matches.len() != 1 {
        base.status = "ambiguous".into();
        base.reason = "Several accepted administrative shapes match; no area was selected.".into();
        return base;
    }
    let a = matches[0];
    base.selected_candidate_id = Some(a.id.clone());
    base.precision = if a.level == "ADM0" {
        "country_area"
    } else {
        "administrative_area"
    }
    .into();
    if allowed && p.role == LocationRole::Event {
        base.area = Some(
            json!({"id":a.id,"label":a.name,"bounds":a.bounds,"level":a.level,"represented_time":a.metadata["boundaryYearRepresented"]}),
        );
        base.occurrence_eligible = true;
        base.status = "resolved_area".into();
        base.reason="The source supports an occurrence somewhere in this publisher area. No city point or exact incident footprint is implied.".into();
    } else {
        base.status = "context_only".into();
        base.reason =
            "The named area is retained as context without asserting a reported occurrence there."
                .into();
    }
    base
}
