//! Conservative, offline location candidates from specifically accepted releases.
//! Source text and roles remain separate from publisher place identity. This
//! module never uses model coordinates or promotes a country label to a site.
use crate::{
    error::{Error, Result},
    model::{hash, required},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

const MAX_REFERENCE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PROPOSALS: usize = 32;
const MAX_CANDIDATES: usize = 100;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocationRole {
    Event,
    Origin,
    Destination,
    Issuer,
    Mentioned,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocationProposal {
    pub mention: String,
    pub quote: String,
    pub role: LocationRole,
    #[serde(default)]
    pub canonical_name: Option<String>,
    #[serde(default)]
    pub country_code: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReferenceIdentity {
    pub dataset_id: String,
    pub publisher: String,
    pub edition: String,
    pub product_version: String,
    pub source_path: String,
    pub source_sha256: String,
    pub publisher_record_id: String,
    pub record_sha256: String,
    pub acceptance_id: String,
    pub acceptance_sha256: String,
    pub represented_time: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LocationCandidate {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub country_code: Option<String>,
    pub country_name: Option<String>,
    pub geometry: Option<Value>,
    pub precision: String,
    pub reference: ReferenceIdentity,
    pub match_basis: String,
    pub matched_alias: Option<String>,
    pub alias_reference: Option<Value>,
    pub country_constraint_match: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LocationResolution {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub literal_qualifier: Option<Value>,
    pub mention: String,
    pub quote: String,
    pub role: LocationRole,
    pub status: String,
    pub candidates: Vec<LocationCandidate>,
    pub candidate_count: usize,
    pub candidates_truncated: bool,
    pub selected_candidate_id: Option<String>,
    pub geometry: Option<Value>,
    pub precision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area: Option<Value>,
    pub occurrence_eligible: bool,
    pub reason: String,
}

#[derive(Clone)]
struct Entry {
    candidate: LocationCandidate,
    aliases: BTreeMap<String, Alias>,
    /// Natural Earth city size and capital status, for homonym dominance.
    population: u64,
    capital: bool,
}
#[derive(Clone)]
struct Alias {
    original: String,
    reference: Option<Value>,
}
#[derive(Clone)]
pub struct GeoResolver {
    entries: Vec<Entry>,
    index: BTreeMap<String, Vec<usize>>,
    reference_summary: Value,
    indexes: Vec<crate::geonames::Index>,
    facilities: Vec<crate::osm_store::Index>,
    facility_countries: crate::osm_resolution::CountryCache,
    areas: Vec<crate::boundary_store::AreaRecord>,
    store: Option<crate::store::Store>,
    spelling: std::sync::Arc<crate::geonames::SpellingCache>,
}

impl GeoResolver {
    pub fn load(project_root: &Path) -> Result<Self> {
        let root = project_root.join("reference/accepted/natural-earth");
        let manifest_bytes = bounded_read(&root.join("processing-manifest.json"))?;
        let manifest: Value = serde_json::from_slice(&manifest_bytes)?;
        if manifest["schema_version"] != 1
            || manifest["status"] != "accepted_for_city_level_location_candidates"
            || manifest["edition"] != "v5.1.2"
            || manifest["source_record_count"] != 243
            || manifest["product_version"] != "5.1.2"
        {
            return Err(Error::invalid(
                "City reference acceptance is unavailable or unsupported",
            ));
        }
        let acceptance_hash = hash(&manifest_bytes);
        let city_path = "raw/ne_110m_populated_places.geojson";
        if manifest["source_path"] != city_path {
            return Err(Error::invalid("Unsupported city reference path"));
        }
        let city_bytes =
            verified_read(&root.join(city_path), required(&manifest, "source_sha256")?)?;
        if manifest["source_bytes"].as_u64() != Some(city_bytes.len() as u64) {
            return Err(Error::invalid("Accepted city reference byte count changed"));
        }
        let cities: Value = serde_json::from_slice(&city_bytes)?;
        let mut entries = Vec::new();
        let mut identities = BTreeSet::new();
        for feature in features(&cities, 243)? {
            let properties = &feature["properties"];
            let publisher_id = publisher_id(properties)?;
            let id = format!("natural-earth:v5.1.2:city:{publisher_id}");
            if !identities.insert(id.clone()) {
                return Err(Error::invalid("Duplicate publisher city identity"));
            }
            validate_point(&feature["geometry"])?;
            let name = preferred_name(properties)?;
            let reference = ReferenceIdentity {
                dataset_id: required(&manifest, "id")?.into(),
                publisher: "Natural Earth".into(),
                edition: "v5.1.2".into(),
                product_version: required(&manifest, "product_version")?.into(),
                source_path: format!("reference/accepted/natural-earth/{city_path}"),
                source_sha256: required(&manifest, "source_sha256")?.into(),
                publisher_record_id: publisher_id,
                record_sha256: hash(&serde_json::to_vec(feature)?),
                acceptance_id: required(&manifest, "acceptance_id")?.into(),
                acceptance_sha256: acceptance_hash.clone(),
                represented_time:
                    "2022 publisher release; individual observation date not supplied".into(),
            };
            entries.push(Entry {
                candidate: LocationCandidate {
                    id,
                    name: name.clone(),
                    kind: "city".into(),
                    country_code: iso2(&properties["ISO_A2"]),
                    country_name: optional_text(&properties["ADM0NAME"]),
                    geometry: Some(feature["geometry"].clone()),
                    precision: "city_level_approximate".into(),
                    reference,
                    match_basis: "unmatched".into(),
                    matched_alias: None,
                    alias_reference: None,
                    country_constraint_match: None,
                },
                aliases: publisher_aliases(properties, &name),
                population: properties["POP_MAX"].as_u64().unwrap_or(0),
                capital: properties["FEATURECLA"].as_str().is_some_and(|f| f.starts_with("Admin-0 capital")),
            });
        }
        if manifest["aliases"]["path"] != "processing-aliases.json" {
            return Err(Error::invalid("Unsupported city alias path"));
        }
        let aliases_bytes = verified_read(
            &root.join("processing-aliases.json"),
            required(&manifest["aliases"], "sha256")?,
        )?;
        let aliases: Value = serde_json::from_slice(&aliases_bytes)?;
        if aliases["schema_version"] != 1
            || aliases["status"] != "accepted_for_bounded_exact_city_matching"
        {
            return Err(Error::invalid("City aliases lack their bounded acceptance"));
        }
        let rules = aliases["rules"]
            .as_array()
            .filter(|rules| rules.len() <= 100)
            .ok_or_else(|| Error::invalid("City alias rules exceed their bound"))?;
        if manifest["aliases"]["count"].as_u64() != Some(rules.len() as u64) {
            return Err(Error::invalid("Accepted city alias count changed"));
        }
        for rule in rules {
            let target = required(rule, "target_id")?;
            let alias = required(rule, "alias")?;
            if alias.len() > 240 {
                return Err(Error::invalid("Reviewed city alias exceeds its bound"));
            }
            let entry = entries
                .iter_mut()
                .find(|entry| entry.candidate.id == target)
                .ok_or_else(|| {
                    Error::invalid("Reviewed alias refers to an unavailable city identity")
                })?;
            let publisher_field = required(rule, "publisher_name_field")?;
            let publisher_value = required(rule, "publisher_value")?;
            let original = features(&cities, 243)?
                .iter()
                .find(|feature| {
                    feature["properties"]["NE_ID"]
                        .as_u64()
                        .map(|id| id.to_string())
                        .as_deref()
                        == Some(entry.candidate.reference.publisher_record_id.as_str())
                })
                .ok_or_else(|| Error::invalid("Reviewed alias original record is unavailable"))?;
            if original["properties"][publisher_field].as_str() != Some(publisher_value) {
                return Err(Error::invalid(
                    "Reviewed alias does not match its retained publisher name",
                ));
            }
            entry.aliases.insert(normalize(alias),Alias { original:alias.into(),reference:Some(json!({"id":aliases["id"],"sha256":hash(&aliases_bytes),"language":rule["language"],"rationale":rule["rationale"],"publisher_name_field":rule["publisher_name_field"],"publisher_value":rule["publisher_value"]})) });
        }

        // This separate, narrow country-name acceptance carries no geometry.
        // The previous camera-navigation acceptance is not geocoding authority.
        let country_manifest = &manifest["country_identity"];
        if country_manifest["status"] != "accepted_for_broad_name_identity_only"
            || country_manifest["product_version"] != "5.1.1"
            || country_manifest["unique_record_count"] != 210
        {
            return Err(Error::invalid(
                "Country-name reference acceptance is unavailable",
            ));
        }
        let source_files = country_manifest["source_files"]
            .as_array()
            .ok_or_else(|| Error::invalid("Country reference source identities are missing"))?;
        let mut country_ids = BTreeSet::new();
        for (path, count) in [
            ("raw/ne_110m_admin_0_countries.geojson", 177),
            ("raw/ne_110m_admin_0_tiny_countries.geojson", 37),
        ] {
            let file = source_files
                .iter()
                .find(|file| file["path"] == path)
                .ok_or_else(|| Error::invalid("Country reference source is not accepted"))?;
            let bytes = verified_read(&root.join(path), required(file, "sha256")?)?;
            if file["record_count"].as_u64() != Some(count as u64)
                || file["bytes"].as_u64() != Some(bytes.len() as u64)
            {
                return Err(Error::invalid("Accepted country reference counts changed"));
            }
            let countries: Value = serde_json::from_slice(&bytes)?;
            for feature in features(&countries, count)? {
                let properties = &feature["properties"];
                let publisher_id = publisher_id(properties)?;
                if !country_ids.insert(publisher_id.clone()) {
                    continue;
                }
                let name = preferred_name(properties)?;
                entries.push(Entry { candidate: LocationCandidate {
                    id:format!("natural-earth:v5.1.2:country:{publisher_id}"),name:name.clone(),kind:"country".into(),
                    country_code:iso2(&properties["ISO_A2"]).or_else(||iso2(&properties["ISO_A2_EH"])),country_name:Some(name.clone()),
                    geometry:None,precision:"country_level_broad".into(),reference:ReferenceIdentity {
                        dataset_id:required(country_manifest,"id")?.into(),publisher:"Natural Earth".into(),edition:"v5.1.2".into(),product_version:"5.1.1".into(),
                        source_path:format!("reference/accepted/natural-earth/{path}"),source_sha256:required(file,"sha256")?.into(),publisher_record_id:publisher_id,
                        record_sha256:hash(&serde_json::to_vec(feature)?),acceptance_id:required(country_manifest,"acceptance_id")?.into(),acceptance_sha256:acceptance_hash.clone(),
                        represented_time:"2022 publisher release; country names/classifications are publisher viewpoints, not current legal determinations".into(),
                    },match_basis:"unmatched".into(),matched_alias:None,alias_reference:None,country_constraint_match:None,
                }, aliases:publisher_aliases(properties,&name), population: 0, capital: false });
            }
        }
        if country_ids.len() != 210 {
            return Err(Error::invalid(
                "Accepted unique country identity count changed",
            ));
        }
        let mut index: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (position, entry) in entries.iter().enumerate() {
            for alias in entry.aliases.keys() {
                index.entry(alias.clone()).or_default().push(position);
            }
        }
        let reference_summary = json!({"cities":243,"countries":country_ids.len(),"reviewed_aliases":rules.len(),"city_acceptance_id":manifest["acceptance_id"],"city_acceptance_sha256":acceptance_hash,"source_sha256":manifest["source_sha256"],"limits":manifest["limits"],"normalization":"case and whitespace only; no fuzzy matching or model coordinates"});
        Ok(Self {
            entries,
            index,
            reference_summary,
            indexes: vec![],
            areas: vec![],
            facilities: vec![],
            facility_countries: Default::default(),
            store: None,
            spelling: Default::default(),
        })
    }

    pub fn load_for_store(store: &crate::store::Store) -> Result<Self> {
        Self::load_for_store_cancellable(store, &std::sync::atomic::AtomicBool::new(false))
    }
    pub fn load_for_store_cancellable(
        store: &crate::store::Store,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Self> {
        let mut resolver = Self::load(&store.config.root)?;
        let geography = store.geography()?;
        let boundaries = store.boundaries()?;
        let marine = store.marine()?;
        let osm = store.osm()?;
        resolver.facilities = store.osm_indexes()?;
        resolver.indexes = store.geography_indexes_cancellable(cancel)?;
        resolver.areas = store.area_records()?;
        resolver.store = Some(store.clone());
        if geography != store.geography()?
            || boundaries != store.boundaries()?
            || marine != store.marine()?
            || osm != store.osm()?
        {
            return Err(Error::busy());
        }
        if let Some(edition) = crate::boundary_store::combined_edition(
            geography.as_ref(),
            boundaries.as_ref(),
            marine.as_ref(),
            osm.as_ref(),
        ) {
            resolver.reference_summary["active_geography"] = geography.unwrap_or(json!({}));
            resolver.reference_summary["active_geography"]["edition"] = json!(edition);
            resolver.reference_summary["active_boundaries"] = json!(boundaries);
            resolver.reference_summary["active_marine"] = json!(marine);
            resolver.reference_summary["active_osm"] = json!(osm);
        }
        Ok(resolver)
    }
    /// Whether a complete national GeoNames edition is loaded for `code`.
    /// Only then is "the only such name in this country" meaningful; the
    /// global city list alone holds just the larger places.
    fn national_gazetteer(&self, code: &str) -> bool {
        self.indexes
            .iter()
            .any(|i| i.product().eq_ignore_ascii_case(code))
    }

    /// Whether a loaded area that `select` accepts meets a box reaching `km`
    /// north, south, east and west of a point, by its full publisher geometry.
    fn area_near(
        &self,
        (lon, lat): (f64, f64),
        km: f64,
        select: impl Fn(&crate::boundary_store::AreaRecord) -> bool,
    ) -> Result<bool> {
        let Some(store) = &self.store else {
            return Ok(false);
        };
        let dlat = km / 111.32;
        let dlon = (km / (111.32 * lat.to_radians().cos().max(0.01))).min(180.0);
        let (south, north) = ((lat - dlat).max(-90.0), (lat + dlat).min(90.0));
        let wrap = |x: f64| if x < -180.0 { x + 360.0 } else if x > 180.0 { x - 360.0 } else { x };
        let (west, east) = (wrap(lon - dlon), wrap(lon + dlon));
        let nearby = self
            .areas
            .iter()
            .filter(|a| select(a))
            .filter(|a| {
                let b = a.bounds.as_array().and_then(|b| Some((b.first()?.as_f64()?, b.get(1)?.as_f64()?, b.get(2)?.as_f64()?, b.get(3)?.as_f64()?)));
                // A box across the antimeridian is left to the exact test.
                west > east || b.is_none_or(|(w, s, e, n)| w <= east && e >= west && s <= north && n >= south)
            })
            .collect::<Vec<_>>();
        if nearby.is_empty() {
            return Ok(false);
        }
        let c = store.connect()?;
        for area in nearby {
            let near: bool = c.query_row(
                "SELECT relic_area_intersects(full_json,?2,?3,?4,?5) FROM boundary_shapes WHERE id=?1",
                rusqlite::params![area.id, west, south, east, north],
                |r| r.get(0),
            )?;
            if near {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// How a supplied or written country other than a sea feature's filing
    /// country is supported: the record's alternate country codes (cc2) list
    /// it, or the point lies within about 100 km of its loaded outline.
    fn marine_borders(&self, feature: &LocationCandidate, code: &str) -> Result<Option<&'static str>> {
        let listed = feature
            .alias_reference
            .as_ref()
            .and_then(|a| a["publisher_record"].as_str())
            .and_then(|row| row.split('\t').nth(9))
            .is_some_and(|cc2| cc2.split(',').any(|c| c.trim().eq_ignore_ascii_case(code)));
        if listed {
            return Ok(Some("is listed as its alternate country"));
        }
        let Some(point) = publisher_point(feature) else {
            return Ok(None);
        };
        let near = self.area_near(point, BORDERING_KM, |a| a.level == "ADM0" && a.iso2.eq_ignore_ascii_case(code))?;
        Ok(near.then_some("lies within about 100 km"))
    }

    /// Why a sea feature named with no country at all cannot be placed: the
    /// global baseline is not loaded to show its exact name is unique, or it
    /// names another place there (also as an alternate name).
    fn marine_name_collision(&self, name: &str, id: &str) -> Result<Option<String>> {
        if !self.indexes.iter().any(|i| i.product() == "cities500") {
            return Ok(Some("No country is given for this sea feature, and without the global baseline its name cannot be shown to be unique. The identity is kept without a location until a country is known.".into()));
        }
        let rivals = crate::geonames::global_alternatives(&self.indexes, name, id)?;
        Ok((!rivals.is_empty()).then(|| format!("No country is given for this sea feature, and the global baseline has {} other place(s) with this name. The identity is kept without a location until a country is known.", rivals.len())))
    }

    /// Give a broad gazetteer identity of a reviewed marine feature type its
    /// publisher point, marked approximate. A code GeoNames also uses inland
    /// needs a loaded sea nearby. Without `country_supported` (a supplied or
    /// written country) the exact name must be unique; otherwise the identity
    /// waits for a country, which a message's one country can then supply
    /// through its own constrained lookup. Anything else is left unchanged.
    fn place_marine_feature(&self, r: &mut LocationResolution, country_supported: bool) -> Result<()> {
        if r.status != "broad" {
            return Ok(());
        }
        let Some(c) = r.candidates.iter().find(|c| Some(&c.id) == r.selected_candidate_id.as_ref()) else {
            return Ok(());
        };
        let Some(near_sea) = marine_entry(c) else {
            return Ok(());
        };
        let Some(point) = publisher_point(c) else {
            return Ok(());
        };
        if let Some(km) = near_sea
            && !self.area_near(point, km, |a| a.level == "MARINE")?
        {
            r.reason = format!("GeoNames also uses this feature type for rivers and lakes. No loaded Marine Regions sea lies within about {km} km of its point, so the identity is kept without a location.");
            return Ok(());
        }
        if !country_supported {
            let id = c.id.clone();
            let name = r.mention.strip_prefix('#').unwrap_or(&r.mention).to_owned();
            if let Some(reason) = self.marine_name_collision(&name, &id)? {
                r.status = "country_context_required".into();
                r.reason = reason;
                return Ok(());
            }
        }
        r.geometry = Some(json!({"type":"Point","coordinates":[point.0,point.1]}));
        r.status = "resolved".into();
        r.precision = MARINE_FEATURE_PRECISION.into();
        r.occurrence_eligible = true;
        r.reason = "Reviewed marine feature type (strait, channel, shoal, reef or atoll). The gazetteer point stands for a feature that may span tens of kilometres; approximate, not an observed incident coordinate.".into();
        Ok(())
    }

    pub fn edition(&self) -> Option<&str> {
        self.reference_summary["active_geography"]["edition"].as_str()
    }

    pub fn reference_summary(&self) -> Value {
        self.reference_summary.clone()
    }

    /// `occurrence_allowed` is the caller's independently validated
    /// kind==event && claim_status==reported gate. False still returns context.
    pub fn resolve(
        &self,
        source_text: &str,
        proposals: &[LocationProposal],
        occurrence_allowed: bool,
    ) -> Result<Vec<LocationResolution>> {
        self.resolve_in(source_text, proposals, occurrence_allowed, None)
    }

    /// Resolve every place of one message, then retry names that stayed
    /// unresolved in the one country all its other located places share.
    /// Returns that country for the message's per-claim resolutions.
    pub fn resolve_message(
        &self,
        source_text: &str,
        proposals: &[LocationProposal],
        occurrence_allowed: bool,
    ) -> Result<(Vec<LocationResolution>, Option<String>)> {
        let first = self.resolve_in(source_text, proposals, occurrence_allowed, None)?;
        let Some(country) = message_country(&first) else {
            return Ok((first, None));
        };
        if !first.iter().any(|r| RETRY.contains(&r.status.as_str())) {
            return Ok((first, Some(country)));
        }
        let second = self.resolve_in(source_text, proposals, occurrence_allowed, Some(&country))?;
        Ok((second, Some(country)))
    }

    /// `message_country`: the one country of the message's other located
    /// places ([`message_country`]); only unresolved names without any
    /// country of their own are looked up there.
    pub fn resolve_in(
        &self,
        source_text: &str,
        proposals: &[LocationProposal],
        occurrence_allowed: bool,
        message_country: Option<&str>,
    ) -> Result<Vec<LocationResolution>> {
        self.resolve_in_with(source_text, proposals, occurrence_allowed, message_country, false)
    }

    /// `division`: the source called the place a division ("منطقة جيزان",
    /// "Bekaa region"), so a same-name province is not deferred to its seat.
    fn resolve_in_with(
        &self,
        source_text: &str,
        proposals: &[LocationProposal],
        occurrence_allowed: bool,
        message_country: Option<&str>,
        division: bool,
    ) -> Result<Vec<LocationResolution>> {
        if source_text.len() > 64 * 1024 || proposals.len() > MAX_PROPOSALS {
            return Err(Error::invalid(
                "Location resolution exceeds the text or 32-proposal bound",
            ));
        }
        let mut resolved = Vec::with_capacity(proposals.len());
        for original in proposals {
            let proposal = original;
            if acronym(&proposal.mention) && !self.country_name(&proposal.mention) {
                let mut r = self.resolve_one(source_text, proposal, false)?;
                if r.status != "unsupported_quote" {
                    r.status = "unresolved".into();
                    r.candidates.clear();
                    r.candidate_count = 0;
                    r.selected_candidate_id = None;
                    r.geometry = None;
                    r.area = None;
                    r.precision = "unresolved".into();
                    r.occurrence_eligible = false;
                    r.reason = "An all-capital abbreviation is not matched to same-letter place names (UN is not the village Un). Only accepted country abbreviations resolve.".into();
                }
                resolved.push(r);
                continue;
            }
            let expanded = self.surrounding_quote(source_text, proposal);
            let context_proposal = expanded.as_ref().unwrap_or(proposal);
            let context = self.country_suffix(context_proposal);
            let mut constrained = if context.is_some() {
                context_proposal.clone()
            } else {
                proposal.clone()
            };
            let attached =
                self.resolve_attached_context(source_text, context_proposal, occurrence_allowed)?;
            if constrained.country_code.is_none() {
                constrained.country_code = context.as_ref().map(|(_, code)| code.clone());
            }
            let mut base = self.resolve_one(source_text, &constrained, occurrence_allowed)?;
            if ["unresolved", "proposal_only"].contains(&base.status.as_str())
                && let Some(qualified) =
                    self.resolve_qualified(source_text, &constrained, occurrence_allowed)?
            {
                base = qualified;
            }
            if base.status != "unsupported_quote"
                && let Some(qualified) = attached
            {
                base = qualified;
            }
            // A strait or shoal lies between countries (Bab el-Mandeb is filed
            // under Djibouti): a supplied or written country does not exclude
            // the one reviewed marine feature with this exact name when the
            // publisher also lists that country or the feature lies within
            // about 100 km of its loaded outline. Otherwise the conflict stays:
            // "near X, Lebanon" is not a Yemeni reef X.
            let mut bordering = None;
            if base.status == "country_conflict"
                && base.candidate_count == 1
                && let Some(feature) = base.candidates.first().filter(|c| marine_type(c))
                && let Some(code) = constrained.country_code.clone().or_else(|| {
                    base.literal_qualifier.as_ref().and_then(|q| q["country_code"].as_str()).map(str::to_owned)
                })
                && let Some(basis) = self.marine_borders(feature, &code)?
            {
                let filed = feature.country_code.clone().unwrap_or_else(|| "another country".into());
                let open = LocationProposal { country_code: None, ..proposal.clone() };
                let within = self.resolve_one(source_text, &open, occurrence_allowed)?;
                if marine_feature(&within) {
                    base = within;
                    constrained.country_code = None;
                    bordering = Some(format!("The publisher files it under {filed}; {} {basis}.", code.to_ascii_uppercase()));
                }
            }
            // Downstream area adoption retains the cited excerpt, independently
            // of the separately recorded context inspected around it.
            base.quote = proposal.quote.clone();
            if let Some(store) = &self.store {
                // A gazetteer airport, base or port is an identity without a
                // location; an OpenStreetMap outline of it is kept if found.
                let feature = base.status == "broad"
                    && base.candidates.iter().any(|c| {
                        Some(&c.id) == base.selected_candidate_id.as_ref()
                            && c.alias_reference.as_ref().is_some_and(|a| ["H", "L", "R", "S", "T", "U", "V"].iter().any(|class| a["feature_class"] == *class))
                    });
                if feature {
                    let mut open = base.clone();
                    open.status = "unresolved".into();
                    open.candidates.clear();
                    open.candidate_count = 0;
                    open.candidates_truncated = false;
                    open.selected_candidate_id = None;
                    open.precision = "unresolved".into();
                    let outlined = crate::osm_resolution::resolve(&self.facilities, store, &self.facility_countries, &self.areas, &constrained, occurrence_allowed, open)?;
                    if outlined.geometry.is_some() || outlined.area.is_some() {
                        base = outlined;
                    } else if occurrence_allowed && proposal.role == LocationRole::Event {
                        // A reviewed strait, shoal or reef: its gazetteer point.
                        let supported = constrained.country_code.is_some()
                            || bordering.is_some()
                            || base.literal_qualifier.as_ref().is_some_and(|q| q["country_code"].is_string());
                        self.place_marine_feature(&mut base, supported)?;
                        if let Some(note) = &bordering
                            && base.geometry.is_some()
                        {
                            base.reason = format!("{} {note}", base.reason);
                        }
                    }
                } else {
                    base = crate::osm_resolution::resolve(
                        &self.facilities,
                        store,
                        &self.facility_countries,
                        &self.areas,
                        &constrained,
                        occurrence_allowed,
                        base,
                    )?;
                }
            }
            let base = crate::marine::resolve(&self.areas, &constrained, occurrence_allowed, base);
            let mut base = crate::area_resolution::resolve(
                &self.areas,
                &constrained,
                occurrence_allowed,
                base,
            );
            if let Some((_, code)) = &context
                && proposal
                    .country_code
                    .as_ref()
                    .is_some_and(|given| !given.eq_ignore_ascii_case(code))
            {
                base.status = "country_conflict".into();
                base.reason = "The supplied country constraint conflicts with the attached literal country phrase; no occurrence geometry was assigned.".into();
                base.geometry = None;
                base.area = None;
                base.selected_candidate_id = None;
                base.occurrence_eligible = false;
                base.precision = "unresolved".into();
            }
            self.common_administrative_area(&constrained, occurrence_allowed, &mut base, division)?;
            self.principal_settlement(occurrence_allowed, &mut base)?;
            // Exact country support can locate an otherwise unknown feature at
            // country precision. Never turn a terrain/route mention into a city.
            // A name fallback below may still find the local place itself.
            let mut context_area = false;
            if ["unresolved", "proposal_only", "ambiguous"].contains(&base.status.as_str())
                && let Some((mention, country)) = context.as_ref()
                && proposal
                    .country_code
                    .as_ref()
                    .is_none_or(|c| c.eq_ignore_ascii_case(country))
            {
                let country_proposal = LocationProposal {
                    mention: mention.clone(),
                    quote: context_proposal.quote.clone(),
                    role: proposal.role.clone(),
                    canonical_name: None,
                    country_code: Some(country.clone()),
                };
                let broad = self.resolve_one(source_text, &country_proposal, occurrence_allowed)?;
                let broad = crate::area_resolution::resolve(
                    &self.areas,
                    &country_proposal,
                    occurrence_allowed,
                    broad,
                );
                if broad.area.is_some() {
                    adopt_area(
                        &mut base,
                        broad,
                        "The occurrence quote explicitly locates this unresolved feature in this country. Only country-area precision is supported; the local site remains unresolved.",
                    );
                    context_area = true;
                }
            }
            if (base.literal_qualifier.is_some() || context.is_some()) && expanded.is_some() {
                let support = &context_proposal.quote;
                let byte_start = source_text.find(support).ok_or_else(Error::internal)?;
                let start = source_text[..byte_start].chars().count();
                let mut qualifier = base
                    .literal_qualifier
                    .take()
                    .unwrap_or_else(|| json!({"country_code":constrained.country_code}));
                qualifier["support"] = json!({"quote":support,"start_character":start,"end_character":start+support.chars().count()});
                base.literal_qualifier = Some(qualifier);
            }
            // "the Strait of Hormuz": when the whole phrase is not a name ("The
            // Hague" is), retry without the article. The shorter mention still
            // occurs literally inside the same quote.
            // A fallback replaces a country-area placement only with something
            // more precise than the country.
            let precise = |w: &LocationResolution| {
                w.geometry.is_some() || w.area.as_ref().is_some_and(|a| a["level"] != "ADM0")
            };
            if (["unresolved", "proposal_only"].contains(&base.status.as_str()) || context_area)
                && let Some(name) = article_free(&proposal.mention)
            {
                let part = LocationProposal { mention: name.into(), ..proposal.clone() };
                let within = self.resolve_in(source_text, std::slice::from_ref(&part), occurrence_allowed, message_country)?.remove(0);
                if if context_area { precise(&within) } else { within.status != "unresolved" } {
                    base = within;
                    context_area = false;
                }
            }
            // "southern Lebanon": a directional part of a named place. Only when
            // the whole phrase is not itself a name ("West Bank", "South Sudan").
            if (["unresolved", "proposal_only"].contains(&base.status.as_str()) || context_area)
                && let Some((direction, inner)) = directional(&proposal.mention)
            {
                let part = LocationProposal { mention: inner.into(), canonical_name: None, ..proposal.clone() };
                let mut within = self.resolve_in(source_text, std::slice::from_ref(&part), occurrence_allowed, message_country)?.remove(0);
                let found = within.geometry.is_some() || within.area.is_some() || within.selected_candidate_id.is_some();
                if if context_area { precise(&within) } else { found } {
                    within.literal_qualifier = Some(json!({"direction":direction,"within":inner}));
                    within.reason = format!("\"{}\" names part of {}. {}", proposal.mention, inner, within.reason);
                    base = within;
                    context_area = false;
                }
            }
            // "Mansouri, southern Lebanon": the place, constrained to the country
            // its qualifier names. Never a different country than one supplied.
            if (["unresolved", "proposal_only"].contains(&base.status.as_str()) || context_area)
                && let Some((name, qualifier)) = place_qualifier(&proposal.mention)
                && let Some(code) = self.country_code_of(directional(qualifier).map_or(qualifier, |(_, inner)| inner))
                && proposal.country_code.as_ref().is_none_or(|c| c.eq_ignore_ascii_case(&code))
            {
                let part = LocationProposal { mention: name.into(), canonical_name: None, country_code: Some(code.clone()), ..proposal.clone() };
                let mut within = self.resolve_in(source_text, std::slice::from_ref(&part), occurrence_allowed, message_country)?.remove(0);
                if precise(&within) {
                    within.literal_qualifier = Some(json!({"within":qualifier,"country_code":code}));
                    within.reason = format!("\"{}\" places {} in {}. {}", proposal.mention, name, qualifier, within.reason);
                    base = within;
                    context_area = false;
                }
            }
            // "Chitgar area": the place name without a generic trailing word.
            if (["unresolved", "proposal_only"].contains(&base.status.as_str()) || context_area)
                && let Some(name) = generic_suffix(&proposal.mention)
            {
                let part = LocationProposal { mention: name.into(), canonical_name: None, ..proposal.clone() };
                let within = self.resolve_in(source_text, std::slice::from_ref(&part), occurrence_allowed, message_country)?.remove(0);
                if precise(&within) {
                    base = within;
                    context_area = false;
                }
            }
            // "Riyadh airport", "مطار جدة": a facility missing from the loaded
            // references, named for a city that is present. The city point is
            // shown at approximate city precision; the facility is not located.
            if (base.status == "unresolved" || context_area)
                && let Some(city) = facility_city(&proposal.mention)
            {
                let part = LocationProposal { mention: city.into(), canonical_name: None, ..proposal.clone() };
                let at = self.resolve_in(source_text, std::slice::from_ref(&part), occurrence_allowed, message_country)?.remove(0);
                if at.geometry.is_some() && at.precision == "city_level_approximate" {
                    let mut at = at;
                    at.literal_qualifier = Some(json!({"facility":proposal.mention,"city":city}));
                    at.reason = format!("\"{}\" is not in the loaded references; the city it is named for is shown. {}", proposal.mention, at.reason);
                    base = at;
                }
            }
            // "مدينة سحاب الأردنية", "محافظة تعز", "Borno State": the name
            // without a generic word, in the country a nationality word names.
            // A division word ("governorate", "استان") accepts only an area.
            if (["unresolved", "proposal_only"].contains(&base.status.as_str()) || context_area)
                && let Some((name, named_country, word)) = generic_words(&proposal.mention)
                && named_country.is_none_or(|c| proposal.country_code.as_ref().is_none_or(|p| p.eq_ignore_ascii_case(c)))
            {
                // "the Bab el-Mandeb strait, Yemen": the written country also
                // applies to the bare strait name, where it must border it.
                let written = (word == Word::Marine).then(|| context.as_ref().map(|(_, code)| code.clone())).flatten();
                let country_code = proposal.country_code.clone().or(named_country.map(str::to_owned)).or(written);
                let part = LocationProposal { mention: name.into(), canonical_name: None, country_code, ..proposal.clone() };
                let division_word = matches!(word, Word::Division | Word::Area);
                let mut within = self.resolve_in_with(source_text, std::slice::from_ref(&part), occurrence_allowed, message_country, division_word)?.remove(0);
                if division_word
                    && within.geometry.is_some()
                    && let Some(division) = self.seat_division(&within, occurrence_allowed, &proposal.mention)?
                {
                    within = division;
                }
                let fits = match word {
                    Word::Division => within.geometry.is_none() && within.area.as_ref().is_some_and(|a| a["level"] != "ADM0"),
                    // Never the island or town a strait is named for.
                    Word::Marine => marine_feature(&within),
                    _ => precise(&within),
                };
                if fits {
                    base = within;
                }
            }
            // "Mayfadoun" in a message whose other places all lie in Lebanon:
            // the same name looked up in that one country. Never overrides a
            // supplied or literal country.
            let foreign_feature = base.status == "broad"
                && message_country.is_some_and(|code| {
                    base.candidates.iter().any(|c| Some(&c.id) == base.selected_candidate_id.as_ref() && c.country_code.as_deref().is_some_and(|cc| !cc.eq_ignore_ascii_case(code)))
                });
            if (RETRY.contains(&base.status.as_str()) || foreign_feature)
                && proposal.country_code.is_none()
                && context.is_none()
                && let Some(code) = message_country
                && self.national_gazetteer(code)
            {
                let part = LocationProposal { country_code: Some(code.into()), ..proposal.clone() };
                let mut within = self.resolve_in(source_text, std::slice::from_ref(&part), occurrence_allowed, None)?.remove(0);
                if precise(&within) && !section(&within) {
                    within.literal_qualifier = Some(json!({"message_country": code}));
                    within.reason = format!("The message's other places all lie in {code}, so the name was looked up there. {}", within.reason);
                    base = within;
                }
            }
            // "Barachit" for the gazetteer's "Braachit": one close spelling of
            // a settlement in the one country known for this place.
            // Only when no exact identity exists in the known country: an
            // ambiguous exact name is a choice between real places, not a
            // misspelling. An exact namesake abroad (an Eritrean "Barachit")
            // does not stop the Lebanese spelling.
            if ["unresolved", "proposal_only", "country_conflict"].contains(&base.status.as_str())
                && let Some(code) = proposal.country_code.as_deref().or(context.as_ref().map(|(_, c)| c.as_str())).or(message_country)
                && self.national_gazetteer(code)
                && let Some(mut variant) = crate::geonames::spelling_variant(&self.indexes, &self.spelling, source_text, &LocationProposal { country_code: Some(code.to_ascii_uppercase()), ..proposal.clone() }, occurrence_allowed)?
            {
                if proposal.country_code.is_none() && context.is_none() {
                    variant.literal_qualifier = Some(json!({"message_country": code}));
                }
                base = variant;
            }
            base.mention = original.mention.clone();
            base.quote = proposal.quote.clone();
            resolved.push(base);
        }
        // A globally unique name in a partial gazetteer is still a proposal.
        // Do not scatter one quoted occurrence across conflicting countries
        // when the source supplied no country context for that name. Explicit
        // country-supported cross-border reporting and issuer context survive.
        // A sea feature's filing country is not where the occurrence is: it
        // neither conflicts nor is conflicted, like a sea.
        let countries = resolved
            .iter()
            .map(|r| {
                r.candidates
                    .iter()
                    .find(|c| Some(&c.id) == r.selected_candidate_id.as_ref())
                    .filter(|c| !marine_type(c))
                    .and_then(|c| c.country_code.clone())
            })
            .collect::<Vec<_>>();
        let conflicts = resolved
            .iter()
            .enumerate()
            .map(|(i, r)| {
                (r.geometry.is_some() || r.area.is_some())
                    && proposals[i].country_code.is_none()
                    && r.literal_qualifier.is_none()
                    && self.country_suffix(&proposals[i]).is_none()
                    && countries[i].is_some()
                    && resolved.iter().enumerate().any(|(j, other)| {
                        i != j
                            && other.role == LocationRole::Event
                            && (other.geometry.is_some() || other.area.is_some())
                            && other.quote == r.quote
                            && countries[j].is_some()
                            && countries[j] != countries[i]
                    })
            })
            .collect::<Vec<_>>();
        for (r, conflict) in resolved.iter_mut().zip(conflicts) {
            if conflict {
                r.status = "context_conflict".into();
                r.reason = "Place candidates in this same occurrence quote imply different countries without supporting country context. Retain the names without assigning occurrence points.".into();
                r.geometry = None;
                r.area = None;
                r.selected_candidate_id = None;
                r.occurrence_eligible = false;
                r.precision = "unresolved".into();
            }
        }
        reject_foreign_namesakes(&mut resolved, message_country);
        Ok(resolved)
    }

    /// Expand only within the containing sentence. Additional support is kept
    /// separately; callers retain the original exact location citation.
    fn surrounding_quote(&self, text: &str, p: &LocationProposal) -> Option<LocationProposal> {
        if !p.quote.contains(&p.mention)
            || text.match_indices(&p.quote).count() != 1
            || p.quote.match_indices(&p.mention).count() != 1
        {
            return None;
        }
        let start = text.find(&p.quote)?;
        let end = start + p.quote.len();
        let boundary = |c: char| matches!(c, '.' | '!' | '?' | '؛' | ';' | '\n' | '。' | '؟');
        let from = text[..start]
            .rfind(boundary)
            .map_or(0, |n| n + text[n..].chars().next().unwrap().len_utf8());
        let to = text[end..].find(boundary).map_or(text.len(), |n| end + n);
        let quote = text[from..to].trim();
        if quote.len() > 4000
            || quote == p.quote
            || text.match_indices(quote).count() != 1
            || quote.match_indices(&p.mention).count() != 1
        {
            return None;
        }
        Some(LocationProposal {
            quote: quote.to_owned(),
            ..p.clone()
        })
    }

    /// Accept a directly attached country or explicitly marked province, not
    /// arbitrary co-mentioned geography elsewhere in the same source text.
    fn resolve_attached_context(
        &self,
        text: &str,
        p: &LocationProposal,
        allowed: bool,
    ) -> Result<Option<LocationResolution>> {
        if text.match_indices(&p.quote).count() != 1
            || p.quote.match_indices(&p.mention).count() != 1
        {
            return Ok(None);
        }
        let Some((_, tail)) = p.quote.split_once(&p.mention) else {
            return Ok(None);
        };
        let tail = tail.trim_end_matches(['.', '!', '?', '؟']);
        let qualifier = [", ", "، ", " in ", " في ", " در "]
            .iter()
            .find_map(|connector| tail.strip_prefix(connector))
            .or_else(|| tail.strip_prefix(" ب").filter(|q| q.starts_with("محافظة ")))
            .or_else(|| {
                tail.strip_prefix(' ')
                    .filter(|q| q.starts_with("محافظة ") || q.starts_with("استان "))
            });
        let Some(qualifier) = qualifier else {
            return Ok(None);
        };
        // Reject a second clause/list; it could refer to a different occurrence.
        let qualifier = qualifier.trim();
        if qualifier.is_empty()
            || qualifier.len() > 240
            || qualifier.starts_with('#')
            || qualifier.contains([',', '،', ';', '؛', '\n', ':'])
        {
            return Ok(None);
        }
        self.resolve_with_qualifier(text, p, &p.mention, qualifier, allowed)
    }

    /// A literal compound name can constrain a locality with a separately
    /// identified country/province. No token deletion or nearest-place guessing.
    fn resolve_qualified(
        &self,
        text: &str,
        p: &LocationProposal,
        allowed: bool,
    ) -> Result<Option<LocationResolution>> {
        let compound = p.mention.split_once([',', '،']).or_else(|| {
            let (name, rest) = p.mention.split_once('(')?;
            Some((name, rest.strip_suffix(')')?))
        });
        let Some((name, qualifier)) = compound else {
            return Ok(None);
        };
        let (name, qualifier) = (name.trim(), qualifier.trim());
        if name.is_empty() || qualifier.is_empty() || qualifier.contains([',', '،', '(', ')', ';'])
        {
            return Ok(None);
        }
        self.resolve_with_qualifier(text, p, name, qualifier, allowed)
    }

    fn resolve_with_qualifier(
        &self,
        text: &str,
        p: &LocationProposal,
        name: &str,
        qualifier: &str,
        allowed: bool,
    ) -> Result<Option<LocationResolution>> {
        let q = LocationProposal {
            mention: qualifier.into(),
            canonical_name: None,
            country_code: None,
            ..p.clone()
        };
        let context = self.resolve_one(text, &q, false)?;
        let Some(parent) = context
            .candidates
            .iter()
            .find(|c| Some(&c.id) == context.selected_candidate_id.as_ref())
        else {
            return Ok(None);
        };
        let admin = parent
            .alias_reference
            .as_ref()
            .filter(|a| a["feature_code"] == "ADM1");
        let qualifier_key = normalize(qualifier);
        if admin.is_some()
            && !["province of ", "governorate of ", "محافظة ", "استان "]
                .iter()
                .any(|prefix| qualifier_key.starts_with(prefix))
            && ![" province", " governorate"]
                .iter()
                .any(|suffix| qualifier_key.ends_with(suffix))
        {
            return Ok(None);
        }
        if parent.kind != "country" && admin.is_none() {
            return Ok(None);
        }
        let Some(country) = parent.country_code.as_ref() else {
            return Ok(None);
        };
        let country_conflict = p
            .country_code
            .as_ref()
            .is_some_and(|c| !c.eq_ignore_ascii_case(country));
        let local = LocationProposal {
            mention: name.into(),
            country_code: Some(country.clone()),
            ..p.clone()
        };
        let mut resolved = self.resolve_one(text, &local, allowed)?;
        let incompatible = [
            "unsupported_quote",
            "canonical_conflict",
            "country_conflict",
        ]
        .contains(&resolved.status.as_str());
        // A country followed by one of its provinces is a geographic list,
        // not a locality nested beneath that province. Keep the country match.
        if admin.is_some() && resolved.candidates.iter().any(|c| c.kind == "country") {
            return Ok(None);
        }
        let mut hierarchy_conflict = false;
        if !incompatible
            && !resolved.candidates_truncated
            && let Some(admin) = admin
        {
            let fields = admin["publisher_record"]
                .as_str()
                .unwrap_or("")
                .split('\t')
                .collect::<Vec<_>>();
            if fields.len() != 19 || fields[10].is_empty() {
                return Ok(None);
            }
            let mut unknown_hierarchy = false;
            let mut all_conflicting_cities = !resolved.candidates.is_empty();
            resolved.candidates.retain(|c| {
                let raw = c
                    .alias_reference
                    .as_ref()
                    .and_then(|a| a["publisher_record"].as_str())
                    .unwrap_or("");
                let local = raw.split('\t').collect::<Vec<_>>();
                if local.len() != 19 || local[10].is_empty() {
                    unknown_hierarchy = true;
                    all_conflicting_cities = false;
                    return true;
                }
                let matches = local[8] == fields[8] && local[10] == fields[10];
                if matches || c.kind != "city" {
                    all_conflicting_cities = false;
                }
                matches
            });
            hierarchy_conflict = all_conflicting_cities;
            resolved.candidate_count = resolved.candidates.len();
            resolved.geometry = None;
            resolved.area = None;
            resolved.occurrence_eligible = false;
            resolved.selected_candidate_id = None;
            resolved.precision = "unresolved".into();
            if resolved.candidates.len() == 1 && !unknown_hierarchy {
                select_unique(&mut resolved, allowed);
            } else if resolved.candidates.is_empty() {
                resolved.status = "unresolved".into();
                resolved.reason = "No candidate has confirmed membership in the literal province. The location remains unresolved.".into();
            } else {
                resolved.status = "ambiguous".into();
                resolved.reason =
                    "The literal province does not establish a unique locality identity.".into();
            }
            if unknown_hierarchy {
                resolved.reason = "Publisher administrative membership is incomplete; the literal province cannot establish a locality point or a conflict.".into();
            }
        }
        // A literal qualifier can narrow identities without choosing one. Keep
        // those alternatives for common-area resolution and inspection instead
        // of reverting to the unmatched, unsplit compound mention.
        if country_conflict || hierarchy_conflict {
            resolved.status = "country_conflict".into();
            resolved.geometry = None;
            resolved.area = None;
            resolved.selected_candidate_id = None;
            resolved.occurrence_eligible = false;
            resolved.precision = "unresolved".into();
            resolved.reason = "The literal qualifier conflicts with the supplied country or with the locality's publisher hierarchy.".into();
        }
        let selected = resolved.selected_candidate_id.clone();
        for candidate in &mut resolved.candidates {
            candidate.match_basis = "literal_compound_name_with_publisher_context".into();
        }
        resolved.literal_qualifier = Some(
            json!({"text":qualifier,"id":parent.id,"country_code":country,"reference":parent.reference,"publisher_context":parent.alias_reference,"locality_id":selected,"original_mention":p.mention}),
        );
        resolved.mention = p.mention.clone();
        resolved.reason = format!(
            "The literal qualifier matches the publisher country or province hierarchy. {}",
            resolved.reason
        );
        Ok(Some(resolved))
    }

    fn country_count(&self, text: &str) -> usize {
        let words = |s: &str| {
            normalize(s)
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let haystack = format!(" {} ", words(text));
        let mut hits = Vec::new();
        for entry in self
            .entries
            .iter()
            .filter(|e| e.candidate.kind == "country")
        {
            let Some(code) = entry.candidate.country_code.as_ref() else {
                continue;
            };
            for alias in entry.aliases.values() {
                let name = words(&alias.original);
                if name.is_empty() {
                    continue;
                }
                for (start, matched) in haystack.match_indices(&format!(" {name} ")) {
                    hits.push((start, start + matched.len(), code));
                }
            }
        }
        // A longer country name owns its embedded shorter alias: "South Sudan"
        // is one country, while "Sudan and South Sudan" still names two.
        hits.iter()
            .filter(|(start, end, _)| {
                !hits
                    .iter()
                    .any(|(s, e, _)| s <= start && e >= end && (s < start || e > end))
            })
            .map(|(_, _, code)| code)
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// Only an attached, trailing geographic phrase inside the exact quote.
    /// Flags, account location and unrelated country mentions are not context.
    fn country_suffix(&self, p: &LocationProposal) -> Option<(String, String)> {
        let (_, tail) = p.quote.split_once(&p.mention)?;
        // Immediate apposition remains geographic support even when the quote
        // continues after it ("Hollow, Lebanon, today"). Never search a distant
        // issuer/country clause or infer country from a source account.
        let attached = tail.trim_start();
        for connector in [", in ", ", ", " in "] {
            let rest = if connector == " in " {
                tail.strip_prefix(connector)
            } else {
                attached.strip_prefix(connector)
            };
            let Some(rest) = rest else { continue };
            // Commas also delimit lists of places. A second country in this
            // clause prevents treating the first as an attached qualifier.
            let clause = rest
                .split(['.', '!', '?', ';', '\n'])
                .next()
                .unwrap_or(rest);
            if self.country_count(clause) != 1 {
                continue;
            }
            let mut matches = BTreeMap::new();
            for entry in self
                .entries
                .iter()
                .filter(|e| e.candidate.kind == "country")
            {
                for alias in entry.aliases.values() {
                    let Some(prefix) = rest.get(..alias.original.len()) else {
                        continue;
                    };
                    let after = &rest[alias.original.len()..];
                    if prefix.eq_ignore_ascii_case(&alias.original)
                        && (after.is_empty() || after.starts_with([',', '.', '!', ';', '\n']))
                        && let Some(code) = &entry.candidate.country_code
                    {
                        matches.insert(code.clone(), prefix.to_owned());
                    }
                }
            }
            if matches.len() == 1 {
                return matches.into_iter().next().map(|(code, name)| (name, code));
            }
        }
        let tail =
            tail.trim_end_matches(|c: char| c.is_whitespace() || ['.', '!', '。'].contains(&c));
        if tail.contains(['.', '!', '?', ';', ':', '\n'])
            || normalize(tail).split_whitespace().any(|w| {
                [
                    "while",
                    "said",
                    "reported",
                    "reporting",
                    "according",
                    "officials",
                ]
                .contains(&w)
            })
        {
            return None;
        }
        if self.country_count(tail) != 1 {
            return None;
        }
        let mut found = BTreeMap::new();
        for entry in self
            .entries
            .iter()
            .filter(|e| e.candidate.kind == "country")
        {
            for alias in entry.aliases.values() {
                if alias.original.len() > tail.len() {
                    continue;
                }
                let start = tail.len() - alias.original.len();
                let Some(name) = tail.get(start..) else {
                    continue;
                };
                if !name.eq_ignore_ascii_case(&alias.original) {
                    continue;
                }
                let prefix = normalize(&tail[..start]);
                if [
                    ",",
                    "in",
                    ", in",
                    "in southern",
                    "in northern",
                    "in eastern",
                    "in western",
                    "in central",
                    ", southern",
                    ", northern",
                    ", eastern",
                    ", western",
                    ", central",
                ]
                .iter()
                .any(|x| prefix == *x)
                    && let Some(code) = &entry.candidate.country_code
                {
                    found.insert(code.clone(), name.to_owned());
                }
            }
        }
        if found.len() != 1 {
            return None;
        }
        found.into_iter().next().map(|(code, name)| (name, code))
    }

    /// Several identities share one name. Select the principal settlement only
    /// when it clearly dominates: a national capital, a first/second-order seat,
    /// a town of at least 20,000 people, or the only settlement among features;
    /// every other same-name settlement at most 10% of its population in the
    /// same country and 5% abroad (a national or first-order capital of unknown
    /// size blocks); any other identity a non-settlement feature or division of
    /// the same country, or a minor foreign one. Alias-only global collisions can block but are
    /// never chosen, and a neighbourhood is never the principal. A capital is a
    /// city point. A seat that shares its name with a province at least three
    /// times its size shows that province's area instead, because such mentions
    /// often mean the province ("the war in Gaza"); a literal "City" qualifier
    /// resolves before this.
    fn principal_settlement(&self, allowed: bool, base: &mut LocationResolution) -> Result<()> {
        if base.status != "ambiguous"
            || base.candidates_truncated
            || base.candidates.len() < 2
            || base.candidate_count != base.candidates.len()
        {
            return Ok(());
        }
        struct Facts { class: String, code: String, population: u64, collision: bool }
        // Accepted Natural Earth city names count even when a regional exact
        // match took over the lookup ("New York" and the Ukrainian Nyu-York).
        if let Some(positions) = self.index.get(&normalize(&base.mention)) {
            for position in positions {
                let entry = &self.entries[*position];
                let known = base.candidates.iter().any(|c| {
                    c.id == entry.candidate.id
                        || (c.country_code == entry.candidate.country_code
                            && located(c).zip(point(&entry.candidate.geometry)).is_some_and(|(a, b)| distance_km(a, b) <= 30.0)
                            && c.match_basis != "unverified_global_alias_collision")
                });
                if entry.candidate.kind == "city" && !known {
                    let mut candidate = entry.candidate.clone();
                    candidate.match_basis = "exact_publisher_name_or_reviewed_alias".into();
                    base.candidates.push(candidate);
                    base.candidate_count += 1;
                }
            }
        }
        if base.candidates.len() > MAX_CANDIDATES {
            return Ok(());
        }
        let mut facts = vec![];
        for c in &base.candidates {
            // A Natural Earth city: a major world city with its own size.
            if let Some(entry) = self.entries.iter().find(|e| e.candidate.id == c.id && e.candidate.kind == "city") {
                facts.push(Facts {
                    class: "P".into(),
                    code: if entry.capital { "PPLC" } else { "PPLA" }.into(),
                    population: entry.population,
                    collision: false,
                });
                continue;
            }
            let Some(record) = c.alias_reference.as_ref().and_then(|a| a["publisher_record"].as_str()) else {
                return Ok(());
            };
            let f = record.split('\t').collect::<Vec<_>>();
            if f.len() != 19 || c.country_code.is_none() {
                return Ok(());
            }
            facts.push(Facts {
                class: f[6].into(),
                code: f[7].into(),
                population: if population_not_used(&c.id) { 0 } else { f[14].parse().unwrap_or(0) },
                collision: c.match_basis == "unverified_global_alias_collision",
            });
        }
        const SEATS: [&str; 3] = ["PPLC", "PPLA", "PPLA2"];
        const PARTS: [&str; 8] = ["PPLX", "PPLL", "PPLQ", "PPLH", "PPLW", "PPLS", "PPLF", "PPLR"];
        let settlement = |i: usize| {
            let f = &facts[i];
            !f.collision && f.class == "P" && !PARTS.contains(&f.code.as_str()) && base.candidates[i].geometry.is_some()
        };
        let settlements = (0..facts.len()).filter(|i| settlement(*i)).collect::<Vec<_>>();
        // The only settlement among schools, stations and farms is what a bare
        // name means, unless a same-name division exists: then the division may
        // be meant and common_administrative_area decides.
        let sole = settlements.len() == 1
            && !facts.iter().any(|f| f.class == "A")
            && facts.iter().all(|f| f.class != "P" || !f.collision)
            && (SEATS.contains(&facts[settlements[0]].code.as_str()) || facts[settlements[0]].population >= 5_000);
        let eligible = |i: usize| {
            let f = &facts[i];
            settlement(i) && (sole || SEATS.contains(&f.code.as_str()) || f.population >= 20_000)
        };
        let Some(chosen) = (0..facts.len())
            .filter(|i| eligible(*i))
            .max_by_key(|i| (facts[*i].code == "PPLC", facts[*i].population))
        else {
            return Ok(());
        };
        // Natural Earth metro populations are larger than GeoNames city counts;
        // keep the GeoNames record when both describe the same place.
        let chosen = if base.candidates[chosen].id.starts_with("natural-earth:") {
            let at = point(&base.candidates[chosen].geometry);
            (0..facts.len())
                .find(|i| {
                    settlement(*i)
                        && base.candidates[*i].id.starts_with("geonames:")
                        && base.candidates[*i].country_code == base.candidates[chosen].country_code
                        && at.zip(point(&base.candidates[*i].geometry)).is_some_and(|(a, b)| distance_km(a, b) <= 30.0)
                })
                .unwrap_or(chosen)
        } else {
            chosen
        };
        let top = &facts[chosen];
        let country = base.candidates[chosen].country_code.clone();
        // Same-country villages sharing a town's name are common; a foreign
        // namesake must be much smaller. Unknown size blocks only for national
        // and first-order capitals.
        let minor = |f: &Facts, same_country: bool| {
            if f.population == 0 {
                !["PPLC", "PPLA"].contains(&f.code.as_str()) && (top.population > 0 || sole)
            } else {
                f.population * if same_country { 10 } else { 20 } <= top.population
            }
        };
        let at = point(&base.candidates[chosen].geometry);
        for (i, f) in facts.iter().enumerate() {
            let same_place = base.candidates[i].country_code == country
                && at.zip(located(&base.candidates[i])).is_some_and(|(a, b)| distance_km(a, b) <= 30.0);
            if i == chosen || same_place {
                continue;
            }
            let same_country = base.candidates[i].country_code == country;
            let dominated = if f.class == "P" {
                (same_country && PARTS.contains(&f.code.as_str())) || minor(f, same_country)
            } else if same_country {
                true
            } else {
                !(f.class == "A" && (f.code == "ADM1" || f.code.starts_with("PCL"))) && minor(f, false)
            };
            if !dominated {
                return Ok(());
            }
        }
        // The province reading wins only when the province is much larger than
        // its seat (Gaza Strip 5x Gaza City); a city that is most of its
        // province (Erbil, Tabuk) is what the name usually means.
        // Same-name places elsewhere in the world must also be minor: "Odessa"
        // is Odesa, Ukraine as well as the Texas city.
        let chosen_id = base.candidates[chosen].id.clone();
        let exclude = if chosen_id.starts_with("geonames:") { chosen_id.as_str() } else { "geonames:0" };
        for alternative in crate::geonames::global_alternatives(&self.indexes, &base.mention, exclude)? {
            // The same place in another dataset is not a rival.
            let same_place = alternative.country_code == country
                && at.zip(located(&alternative)).is_some_and(|(a, b)| distance_km(a, b) <= 30.0);
            if same_place || base.candidates.iter().any(|c| c.id == alternative.id) {
                continue;
            }
            let record = alternative.alias_reference.as_ref().and_then(|a| a["publisher_record"].as_str()).unwrap_or("");
            let f = record.split('\t').collect::<Vec<_>>();
            let other = Facts { class: f.get(6).unwrap_or(&"").to_string(), code: f.get(7).unwrap_or(&"").to_string(), population: if population_not_used(&alternative.id) { 0 } else { f.get(14).and_then(|p| p.parse().ok()).unwrap_or(0) }, collision: true };
            if other.class == "P" && !minor(&other, alternative.country_code == country) {
                return Ok(());
            }
        }
        let province = (0..facts.len()).any(|i| {
            facts[i].class == "A" && facts[i].code == "ADM1" && !facts[i].collision
                && base.candidates[i].country_code == country
                && top.population > 0
                && facts[i].population >= 3 * top.population
        });
        let reason = "Several identities share this name. The principal settlement clearly dominates the others (its own divisions, minor features, much smaller places); the alternatives are retained.";
        if top.code != "PPLC" && province
            && let Some(xy) = base.candidates[chosen].geometry.as_ref().and_then(|g| g["coordinates"].as_array()).cloned()
            && let Some(broad) = self.province_at(country.as_deref(), &xy, base, allowed)?
        {
            adopt_area(base, broad, "This name belongs both to a provincial seat and to its own province. The province area is shown, not a guessed city point; the alternatives are retained.");
            return Ok(());
        }
        let principal = base.candidates.remove(chosen);
        base.candidates.insert(0, principal);
        select_unique(base, allowed);
        if base.status == "resolved" || base.status == "context_only" {
            base.reason = format!("{reason} {}", base.reason);
        }
        Ok(())
    }

    /// The one accepted province (ADM1) of `country` containing a point, as
    /// an area resolution built on `base`.
    fn province_at(
        &self,
        country: Option<&str>,
        xy: &[Value],
        base: &LocationResolution,
        allowed: bool,
    ) -> Result<Option<LocationResolution>> {
        let Some(store) = &self.store else {
            return Ok(None);
        };
        let c = store.connect()?;
        let mut containing = vec![];
        for area in self.areas.iter().filter(|a| a.level == "ADM1" && Some(a.iso2.as_str()) == country) {
            let inside: bool = c.query_row("SELECT relic_area_intersects(full_json,?2,?3,?2,?3) FROM boundary_shapes WHERE id=?1", rusqlite::params![area.id, xy.first().and_then(Value::as_f64), xy.get(1).and_then(Value::as_f64)], |r| r.get(0))?;
            if inside {
                containing.push(area);
            }
        }
        if containing.len() != 1 {
            return Ok(None);
        }
        let mut area_base = base.clone();
        area_base.status = "unresolved".into();
        let area_proposal = LocationProposal {
            mention: format!("{} province", containing[0].name),
            quote: base.quote.clone(),
            role: base.role.clone(),
            canonical_name: None,
            country_code: country.map(str::to_owned),
        };
        let broad = crate::area_resolution::resolve(&self.areas, &area_proposal, allowed, area_base);
        Ok(broad.selected_candidate_id.is_some().then_some(broad))
    }

    /// "منطقة جيزان", "Taiz governorate": a division named for its seat. When
    /// the name resolved to that provincial seat, the province containing it.
    fn seat_division(&self, within: &LocationResolution, allowed: bool, mention: &str) -> Result<Option<LocationResolution>> {
        let Some(seat) = within.candidates.iter().find(|c| Some(&c.id) == within.selected_candidate_id.as_ref()) else {
            return Ok(None);
        };
        let seat_code = seat.alias_reference.as_ref().is_some_and(|a| ["PPLA", "PPLC"].iter().any(|code| a["feature_code"] == *code));
        let Some(xy) = seat.geometry.as_ref().and_then(|g| g["coordinates"].as_array()).cloned() else {
            return Ok(None);
        };
        if !seat_code {
            return Ok(None);
        }
        let Some(broad) = self.province_at(seat.country_code.as_deref(), &xy, within, allowed)? else {
            return Ok(None);
        };
        let mut out = within.clone();
        let reason = format!("\"{mention}\" names the division of its seat {}. The province area is shown, not the city point.", seat.name);
        adopt_area(&mut out, broad, &reason);
        Ok(Some(out))
    }

    /// The country an exact country name identifies, when it names exactly one.
    fn country_code_of(&self, mention: &str) -> Option<String> {
        let codes = self
            .index
            .get(&normalize(mention))?
            .iter()
            .filter(|i| self.entries[**i].candidate.kind == "country")
            .filter_map(|i| self.entries[*i].candidate.country_code.clone())
            .collect::<std::collections::BTreeSet<_>>();
        (codes.len() == 1).then(|| codes.into_iter().next().unwrap())
    }
    fn country_name(&self, mention: &str) -> bool {
        self.index
            .get(&normalize(mention))
            .is_some_and(|positions| positions.iter().any(|i| self.entries[*i].candidate.kind == "country"))
    }

    fn common_administrative_area(
        &self,
        p: &LocationProposal,
        allowed: bool,
        base: &mut LocationResolution,
        division: bool,
    ) -> Result<()> {
        if !["ambiguous", "broad"].contains(&base.status.as_str())
            || base.candidates_truncated
            || base.candidates.is_empty()
        {
            return Ok(());
        }
        let admins = base
            .candidates
            .iter()
            .filter(|c| {
                c.alias_reference
                    .as_ref()
                    .is_some_and(|a| a["feature_code"] == "ADM1")
            })
            .collect::<Vec<_>>();
        if admins.len() != 1 {
            return Ok(());
        }
        let admin = admins[0];
        let raw = admin
            .alias_reference
            .as_ref()
            .and_then(|a| a["publisher_record"].as_str())
            .unwrap_or("");
        let fields = raw.split('\t').collect::<Vec<_>>();
        if fields.len() != 19 || fields[10].is_empty() {
            return Ok(());
        }
        // Preserve true homonyms: only that administrative unit and its own
        // localities may share an enclosing region. No population ranking.
        if base.candidates.iter().any(|c| {
            if c.id == admin.id {
                return false;
            }
            let f = c
                .alias_reference
                .as_ref()
                .and_then(|a| a["publisher_record"].as_str())
                .unwrap_or("")
                .split('\t')
                .collect::<Vec<_>>();
            let subordinate_admin = c.alias_reference.as_ref().is_some_and(|a| {
                ["ADM2", "ADM3", "ADM4", "ADM5"]
                    .iter()
                    .any(|code| a["feature_code"] == *code)
            });
            // A bank, river, school or airport of the same name (a Philippine
            // undersea "West Bank") is not a rival reading of a division name.
            let physical_feature = f.len() == 19 && ["H", "L", "R", "S", "T", "U", "V"].contains(&f[6]);
            !physical_feature
                && ((c.kind != "city" && !subordinate_admin)
                    || f.len() != 19
                    || f[8] != fields[8]
                    || f[10] != fields[10])
        }) {
            return Ok(());
        }
        // A bare name shared by a province and its own seat ("Perm") usually
        // names the city. Defer to the principal-settlement rule unless the
        // province dominates the seat by population (that rule's 3x test), so
        // loading a country's provinces does not coarsen its city names.
        let population = |record: &str| {
            record.split('\t').nth(14).and_then(|p| p.parse::<u64>().ok()).unwrap_or(0)
        };
        let seat = base
            .candidates
            .iter()
            .filter(|c| c.id != admin.id)
            .filter_map(|c| {
                let a = c.alias_reference.as_ref()?;
                ["PPLC", "PPLA"]
                    .contains(&a["feature_code"].as_str()?)
                    .then(|| if population_not_used(&c.id) { 0 } else { population(a["publisher_record"].as_str().unwrap_or("")) })
            })
            .max();
        if !division && seat.is_some_and(|seat| seat > 0 && population(raw) < seat.saturating_mul(3)) {
            return Ok(());
        }
        let name = admin
            .name
            .strip_suffix(" Governorate")
            .or_else(|| admin.name.strip_suffix(" Province"))
            .unwrap_or(&admin.name);
        let mut candidates = vec![];
        let mut aliases: Option<BTreeSet<String>> = None;
        for area in self
            .areas
            .iter()
            .filter(|a| a.level == "ADM1" && Some(&a.iso2) == admin.country_code.as_ref())
        {
            let mut matched = normalize(&area.name) == normalize(name);
            if !matched {
                if aliases.is_none() {
                    let mut keys = BTreeSet::new();
                    for index in &self.indexes {
                        keys.extend(index.publisher_alias_keys(&admin.id, &area.iso2)?);
                    }
                    aliases = Some(keys);
                }
                matched = aliases
                    .as_ref()
                    .unwrap()
                    .contains(&crate::geonames::key(&area.name));
            }
            if matched {
                candidates.push(area);
            }
        }
        if candidates.len() != 1 {
            return Ok(());
        }
        let Some(store) = &self.store else {
            return Ok(());
        };
        let c = store.connect()?;
        for candidate in base.candidates.iter().filter(|c| c.kind == "city") {
            let Some(xy) = candidate
                .geometry
                .as_ref()
                .and_then(|g| g["coordinates"].as_array())
            else {
                return Ok(());
            };
            let inside: bool = c.query_row("SELECT relic_area_intersects(full_json,?2,?3,?2,?3) FROM boundary_shapes WHERE id=?1", rusqlite::params![candidates[0].id,xy[0].as_f64(),xy[1].as_f64()], |r|r.get(0))?;
            if !inside {
                return Ok(());
            }
        }
        let mut area_base = base.clone();
        area_base.status = "unresolved".into();
        let area_proposal = LocationProposal {
            mention: format!("{} province", candidates[0].name),
            country_code: admin.country_code.clone(),
            ..p.clone()
        };
        let broad =
            crate::area_resolution::resolve(&self.areas, &area_proposal, allowed, area_base);
        if broad.selected_candidate_id.is_some() {
            adopt_area(
                base,
                broad,
                "The publisher administrative identity and retained aliases match this province. Any ambiguous subordinate identities share its hierarchy, and locality points lie inside its full boundary. Show province-area precision, never a guessed city point.",
            );
        }
        Ok(())
    }

    fn resolve_one(
        &self,
        source_text: &str,
        proposal: &LocationProposal,
        occurrence_allowed: bool,
    ) -> Result<LocationResolution> {
        validate_proposal(proposal)?;
        let mut result = LocationResolution { literal_qualifier:None, mention:proposal.mention.clone(),quote:proposal.quote.clone(),role:proposal.role.clone(),status:"unresolved".into(),
            candidates:Vec::new(),candidate_count:0,candidates_truncated:false,selected_candidate_id:None,geometry:None,precision:"unresolved".into(),area:None,occurrence_eligible:false,
            reason:"No accepted exact place name matches this mention. Its location remains unresolved.".into() };
        if !proposal.quote.contains(&proposal.mention)
            || source_text.match_indices(&proposal.quote).count() != 1
        {
            result.status = "unsupported_quote".into();
            result.reason = "The exact quote must occur once in the source and contain the literal place mention; broaden or correct the supporting excerpt.".into();
            return Ok(result);
        }
        let exact_key = normalize(
            proposal
                .mention
                .strip_prefix('#')
                .unwrap_or(&proposal.mention),
        );
        let exact = self.index.get(&exact_key).cloned().unwrap_or_default();
        // A literal country name stays broad even when a small locality shares
        // that name. A literal city qualifier can independently select a city.
        let country_name = exact
            .iter()
            .any(|i| self.entries[*i].candidate.kind == "country");
        let canonical_key = proposal.canonical_name.as_deref().map(normalize);
        let canonical = canonical_key
            .as_ref()
            .and_then(|key| self.index.get(key))
            .cloned()
            .unwrap_or_default();
        if !country_name
            && !self.indexes.is_empty()
            && let Some(mut resolved) = crate::geonames::resolve_indexes(
                &self.indexes,
                source_text,
                proposal,
                occurrence_allowed,
            )?
        {
            let accepted = exact
                .iter()
                .copied()
                .filter(|position| {
                    let candidate = &self.entries[*position].candidate;
                    candidate.kind == "city"
                        && proposal
                            .country_code
                            .as_ref()
                            .is_none_or(|code| candidate.country_code.as_ref() == Some(code))
                        && (canonical.is_empty() || canonical.contains(position))
                })
                .collect::<Vec<_>>();
            // A new regional alias must not shadow an already accepted literal
            // match in another country. Preserve both sources' identities;
            // country-constrained or jointly supported canonical matches may
            // still use the existing Natural Earth resolution below.
            let use_accepted = !accepted.is_empty()
                && (resolved.status == "country_conflict"
                    || (resolved.status == "canonical_conflict" && !canonical.is_empty()));
            if !use_accepted {
                if proposal.country_code.is_none() {
                    let other = accepted
                        .into_iter()
                        .filter(|position| {
                            !resolved.candidates.iter().any(|candidate| {
                                candidate.country_code
                                    == self.entries[*position].candidate.country_code
                            })
                        })
                        .collect::<Vec<_>>();
                    if !other.is_empty() {
                        resolved.candidate_count += other.len();
                        resolved.candidates.extend(self.candidates(
                            &other,
                            &exact_key,
                            "exact_publisher_name_or_reviewed_alias",
                            None,
                        ));
                        resolved.candidates.truncate(MAX_CANDIDATES);
                        resolved.candidates_truncated = resolved.candidate_count > MAX_CANDIDATES;
                        resolved.status = "ambiguous".into();
                        resolved.reason = "Accepted references name different-country alternatives; regional aliases cannot replace the existing literal match.".into();
                        resolved.selected_candidate_id = None;
                        resolved.geometry = None;
                        resolved.area = None;
                        resolved.precision = "unresolved".into();
                        resolved.occurrence_eligible = false;
                    }
                }
                return Ok(resolved);
            }
        }
        if exact.is_empty() {
            if !canonical.is_empty() {
                result.candidates = self.candidates(
                    &canonical,
                    canonical_key.as_deref().unwrap(),
                    "canonical_name_proposal",
                    proposal.country_code.as_deref(),
                );
                result.candidate_count = canonical.len();
                result.status = "proposal_only".into();
                result.reason = "Only the proposed canonical name matches. The original mention needs a reviewed alias or another accepted reference; no occurrence point was assigned.".into();
            }
            result.candidates_truncated = result.candidate_count > MAX_CANDIDATES;
            return Ok(result);
        }
        if !canonical.is_empty() && !canonical.iter().any(|position| exact.contains(position)) {
            let positions = exact
                .iter()
                .chain(canonical.iter())
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            result.candidates = self.candidates(
                &positions,
                &exact_key,
                "conflicting_name_proposals",
                proposal.country_code.as_deref(),
            );
            result.candidate_count = positions.len();
            result.candidates_truncated = positions.len() > MAX_CANDIDATES;
            result.status = "canonical_conflict".into();
            result.reason = "The canonical name conflicts with the literal source mention. Neither was silently substituted.".into();
            return Ok(result);
        }
        let country = proposal
            .country_code
            .as_deref()
            .map(|code| code.trim().to_ascii_uppercase());
        let constrained = exact
            .iter()
            .copied()
            .filter(|position| {
                country.as_ref().is_none_or(|code| {
                    self.entries[*position].candidate.country_code.as_ref() == Some(code)
                })
            })
            .collect::<Vec<_>>();
        if constrained.is_empty() {
            result.candidates = self.candidates(
                &exact,
                &exact_key,
                "exact_name_country_conflict",
                country.as_deref(),
            );
            result.candidate_count = exact.len();
            result.candidates_truncated = exact.len() > MAX_CANDIDATES;
            result.status = "country_conflict".into();
            result.reason = "The exact name exists, but no retained candidate matches the supplied country constraint. Coverage is incomplete; no nearest or wrong-country substitution was made.".into();
            return Ok(result);
        }
        result.candidate_count = constrained.len();
        result.candidates_truncated = constrained.len() > MAX_CANDIDATES;
        result.candidates = self.candidates(
            &constrained,
            &exact_key,
            "exact_publisher_name_or_reviewed_alias",
            country.as_deref(),
        );
        let countries = constrained
            .iter()
            .copied()
            .filter(|i| self.entries[*i].candidate.kind == "country")
            .collect::<Vec<_>>();
        // "Kuwait" names the country, and Kuwait City lies inside it: the
        // country reading is safe. "عمان" is Oman and Amman, Jordan: ambiguous.
        let same_country = countries.len() == 1
            && constrained.iter().all(|i| {
                self.entries[*i].candidate.country_code == self.entries[countries[0]].candidate.country_code
            });
        let constrained = if constrained.len() > 1 && same_country {
            countries
        } else {
            constrained
        };
        result.candidate_count = constrained.len();
        result.candidates_truncated = constrained.len() > MAX_CANDIDATES;
        result.candidates = self.candidates(
            &constrained,
            &exact_key,
            "exact_publisher_name_or_reviewed_alias",
            country.as_deref(),
        );
        if constrained.len() != 1 {
            result.status = "ambiguous".into();
            result.reason = "Several accepted identities share this name and context. The alternatives remain available without an automatic selection.".into();
            return Ok(result);
        }
        select_unique(&mut result, occurrence_allowed);
        Ok(result)
    }

    fn candidates(
        &self,
        positions: &[usize],
        name_key: &str,
        basis: &str,
        country: Option<&str>,
    ) -> Vec<LocationCandidate> {
        let mut candidates = positions
            .iter()
            .map(|position| {
                let entry = &self.entries[*position];
                let mut candidate = entry.candidate.clone();
                candidate.match_basis = basis.into();
                if let Some(alias) = entry.aliases.get(name_key) {
                    candidate.matched_alias = Some(alias.original.clone());
                    candidate.alias_reference = alias.reference.clone();
                }
                candidate.country_constraint_match = country.map(|code| {
                    candidate.country_code.as_deref()
                        == Some(code.trim().to_ascii_uppercase().as_str())
                });
                candidate
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.id.cmp(&right.id));
        candidates.truncate(MAX_CANDIDATES);
        candidates
    }
}

fn select_unique(result: &mut LocationResolution, occurrence_allowed: bool) {
    let candidate = &result.candidates[0];
    result.selected_candidate_id = Some(candidate.id.clone());
    result.precision = candidate.precision.clone();
    if candidate.kind == "country" {
        result.status = "broad".into();
        result.reason = "This is a country-level mention. Country label points and centroids are not occurrence locations.".into();
    } else if candidate.geometry.is_none() {
        result.status = "broad".into();
        result.reason = "Publisher feature identity matches the literal context; its label point is not an occurrence site.".into();
    } else if result.role != LocationRole::Event {
        result.status = "context_only".into();
        result.reason = "This place has an origin, destination, issuer or mention role; it is preserved as context without asserting an occurrence there.".into();
    } else if !occurrence_allowed {
        result.status = "context_only".into();
        result.reason = "The report is not eligible as a reported event. A valid place name does not establish that an incident occurred.".into();
    } else {
        result.status = "resolved".into();
        result.geometry = candidate.geometry.clone();
        result.occurrence_eligible = true;
        result.reason = "Exact city identity matched the supplied context. The unchanged publisher point is city-level approximate, not an observed incident-site coordinate.".into();
    }
}

fn adopt_area(base: &mut LocationResolution, mut broad: LocationResolution, reason: &str) {
    let original = serde_json::to_value(&base.candidates).unwrap_or(Value::Null);
    for c in &mut broad.candidates {
        c.alias_reference = Some(
            json!({"name_support":c.alias_reference,"local_candidates":original,"original_mention":base.mention,"quote":base.quote}),
        );
        c.match_basis = "supported_enclosing_area".into();
    }
    broad.mention = base.mention.clone();
    broad.quote = base.quote.clone();
    broad.reason = reason.into();
    *base = broad;
}

fn point(geometry: &Option<Value>) -> Option<(f64, f64)> {
    let xy = geometry.as_ref()?["coordinates"].as_array()?;
    Some((xy.first()?.as_f64()?, xy.get(1)?.as_f64()?))
}
/// A candidate's location for same-place checks: its point, or the publisher
/// record's coordinates for identities shown without one (alias collisions).
fn located(c: &LocationCandidate) -> Option<(f64, f64)> {
    point(&c.geometry).or_else(|| {
        let record = c.alias_reference.as_ref()?["publisher_record"].as_str()?;
        let f = record.split('\t').collect::<Vec<_>>();
        Some((f.get(5)?.parse().ok()?, f.get(4)?.parse().ok()?))
    })
}
/// Great-circle distance between two lon/lat points.
fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, la2) = (a.1.to_radians(), b.1.to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2)
        + la1.cos() * la2.cos() * ((b.0 - a.0).to_radians() / 2.0).sin().powi(2);
    2.0 * 6371.0 * h.sqrt().asin()
}
/// "the Strait of Hormuz" → "Strait of Hormuz", "Al-Mansouri" → "Mansouri".
/// A leading English article or transliterated Arabic article only.
fn article_free(mention: &str) -> Option<&str> {
    ["the ", "al-", "el-", "al ", "el "].iter().find_map(|article| {
        let lower = mention.get(..article.len())?.to_lowercase();
        (lower == *article)
            .then(|| mention[article.len()..].trim())
            .filter(|rest| rest.chars().count() >= 2)
    })
}
/// "Chitgar area" → "Chitgar": a generic trailing word after a place name.
fn generic_suffix(mention: &str) -> Option<&str> {
    let lower = mention.to_lowercase();
    [" area", " district", " neighbourhood", " neighborhood", " village", " town"]
        .iter()
        .find(|s| lower.ends_with(**s))
        .map(|s| mention[..mention.len() - s.len()].trim())
        .filter(|rest| rest.chars().count() >= 2)
}
/// Statuses a later fallback may improve on.
const RETRY: [&str; 4] = ["unresolved", "proposal_only", "ambiguous", "country_context_required"];

/// A place matched only because its name is unique in the gazetteer.
pub const FOREIGN_NAMESAKE: &str = "foreign_namesake";
/// A small ordinary place (a populated place, not an administrative seat or
/// capital, under 15,000 people) given without a country and matched only
/// because one listed place worldwide has that name, is not placed when the
/// message's other event places point to other countries and nothing in the
/// message (issuers and its established country included) points to its own.
/// The Jordanian village of Hamoud is not in the gazetteer; the one Hamoud
/// listed is a Mauritanian village (26 Oct 2026). Capitals and seats abroad
/// (Paris, Hamburg, Tampa) and places beside mere mentions stay placed.
/// Sea features and other such lone matches are no evidence either way.
fn reject_foreign_namesakes(resolved: &mut [LocationResolution], message_country: Option<&str>) {
    let selected = |r: &LocationResolution| r.candidates.iter().find(|c| Some(&c.id) == r.selected_candidate_id.as_ref()).cloned();
    let lone = |r: &LocationResolution| r.status == "resolved" && selected(r).is_some_and(|c| c.match_basis == "exact_geonames_name" && c.country_constraint_match.is_none() && !marine_type(&c));
    let countries = |r: &LocationResolution| r.candidates.iter().filter(|c| !marine_type(c)).filter_map(|c| c.country_code.as_deref()).map(str::to_ascii_uppercase).collect::<BTreeSet<_>>();
    let rejected = resolved
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let candidate = selected(r)?;
            if !lone(r) || !minor_place(&candidate) {
                return None;
            }
            let code = candidate.country_code.as_deref()?.to_ascii_uppercase();
            let others = resolved.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, o)| o);
            let mut support = others.clone().flat_map(countries).collect::<BTreeSet<_>>();
            support.extend(message_country.map(str::to_ascii_uppercase));
            let contrary = others.filter(|o| o.role == LocationRole::Event && !lone(o)).flat_map(countries).collect::<BTreeSet<_>>();
            (!contrary.is_empty() && !support.contains(&code)).then_some((i, code))
        })
        .collect::<Vec<_>>();
    for (i, code) in rejected {
        let r = &mut resolved[i];
        r.status = FOREIGN_NAMESAKE.into();
        r.reason = format!("The only listed place with this name is a small place in {code}, a country nothing else in the message points to, while its other event places lie elsewhere. A namesake abroad is not placed.");
        r.selected_candidate_id = None;
        r.geometry = None;
        r.area = None;
        r.occurrence_eligible = false;
        r.precision = "unresolved".into();
    }
}
/// A GeoNames populated place that is neither a capital nor an administrative
/// seat and has fewer than 15,000 people (from its publisher record).
fn minor_place(c: &LocationCandidate) -> bool {
    let Some(record) = c.alias_reference.as_ref().and_then(|a| a["publisher_record"].as_str()) else {
        return false;
    };
    let fields = record.split('\t').collect::<Vec<_>>();
    let code = fields.get(7).copied().unwrap_or("");
    let population = fields.get(14).and_then(|p| p.parse::<u64>().ok()).unwrap_or(0);
    code.starts_with("PPL") && !code.starts_with("PPLA") && !matches!(code, "PPLC" | "PPLG") && population < 15_000
}

/// An event's places follow the message-level reading of the same mentions:
/// a name the whole message rejected as a foreign namesake is not placed for
/// one claim that lacks the message's other context.
pub fn follow_message(message: &[LocationResolution], indices: &[usize], places: &mut [LocationResolution]) {
    for (place, index) in places.iter_mut().zip(indices) {
        let Some(whole) = message.get(*index) else { continue };
        if whole.status == FOREIGN_NAMESAKE && place.status == "resolved" && place.mention == whole.mention {
            let role = place.role.clone();
            *place = whole.clone();
            place.role = role;
        }
    }
}

/// The one country every located or identified place of a message lies in.
/// Seas, sea features and places without a country are neutral; two
/// countries give none. GeoNames files a strait under one of its shores
/// (Bab el-Mandeb under Djibouti), so it says nothing about the message.
pub fn message_country(resolved: &[LocationResolution]) -> Option<String> {
    let mut found: Option<String> = None;
    for r in resolved {
        let Some(selected) = r.candidates.iter().find(|c| Some(&c.id) == r.selected_candidate_id.as_ref()) else {
            continue;
        };
        if marine_type(selected) {
            continue;
        }
        // Accepted places only: not a namesake awaiting country context, and
        // not a stray physical feature, but a country or division counts.
        let division = selected.kind == "country"
            || selected.alias_reference.as_ref().is_some_and(|a| a["feature_class"] == "A");
        if !(["resolved", "resolved_area", "context_only"].contains(&r.status.as_str()) || (r.status == "broad" && division)) {
            continue;
        }
        let Some(code) = selected.country_code.clone() else {
            continue;
        };
        match &found {
            Some(f) if !f.eq_ignore_ascii_case(&code) => return None,
            _ => found = Some(code.to_ascii_uppercase()),
        }
    }
    found
}

/// Reviewed publisher figures a resolver rule must not use. The reviewed file
/// is part of the location-context identity ([`corrections_hash`]).
const CORRECTIONS: &str = include_str!("../../reference/geography-corrections.json");
static POPULATION_NOT_USED: std::sync::LazyLock<BTreeSet<String>> = std::sync::LazyLock::new(|| {
    serde_json::from_str::<Value>(CORRECTIONS)
        .ok()
        .and_then(|v| v["population_not_used"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|c| c["id"].as_str().map(str::to_owned))
        .collect()
});
fn population_not_used(id: &str) -> bool {
    POPULATION_NOT_USED.contains(id)
}
pub fn corrections_hash() -> String {
    hash(CORRECTIONS.as_bytes())
}

/// Reviewed GeoNames types of small sea features (straits, shoals, reefs,
/// atolls) whose gazetteer point may stand for an event reported there. Part
/// of the location-context identity ([`marine_features_hash`]).
const MARINE_FEATURES: &str = include_str!("../../reference/marine-feature-types.json");
/// Reviewed (class, code) → the distance from a loaded Marine Regions area its
/// point must lie within, for codes GeoNames also uses inland.
static MARINE_FEATURE_TYPES: std::sync::LazyLock<BTreeMap<(String, String), Option<f64>>> = std::sync::LazyLock::new(|| {
    serde_json::from_str::<Value>(MARINE_FEATURES)
        .ok()
        .and_then(|v| v["types"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|t| Some(((t["class"].as_str()?.to_owned(), t["code"].as_str()?.to_owned()), t["near_marine_area_km"].as_f64())))
        .collect()
});
pub fn marine_features_hash() -> String {
    hash(MARINE_FEATURES.as_bytes())
}
/// How far a written country may lie from a sea feature filed elsewhere.
const BORDERING_KM: f64 = 100.0;
/// A GeoNames record's point (longitude, latitude): its fifth and sixth fields.
fn publisher_point(c: &LocationCandidate) -> Option<(f64, f64)> {
    let row = c.alias_reference.as_ref()?["publisher_record"].as_str()?.split('\t').collect::<Vec<_>>();
    let lat = row.get(4)?.parse::<f64>().ok().filter(|v| (-90.0..=90.0).contains(v))?;
    let lon = row.get(5)?.parse::<f64>().ok().filter(|v| (-180.0..=180.0).contains(v))?;
    Some((lon, lat))
}
pub const MARINE_FEATURE_PRECISION: &str = "approximate_marine_feature";
/// The reviewed entry of a gazetteer candidate's marine feature type, if any.
fn marine_entry(c: &LocationCandidate) -> Option<Option<f64>> {
    let a = c.alias_reference.as_ref()?;
    MARINE_FEATURE_TYPES.get(&(a["feature_class"].as_str()?.to_owned(), a["feature_code"].as_str()?.to_owned())).copied()
}
/// A gazetteer candidate of a reviewed marine feature type.
fn marine_type(c: &LocationCandidate) -> bool {
    marine_entry(c).is_some()
}
/// The selected identity is a reviewed marine feature (placed or not).
fn marine_feature(r: &LocationResolution) -> bool {
    r.candidates.iter().any(|c| Some(&c.id) == r.selected_candidate_id.as_ref() && marine_type(c))
}

/// What a generic word says the place is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Word {
    /// "governorate", "محافظة": only an administrative area fits.
    Division,
    /// "region", "منطقة": a same-name province is meant, else any place.
    Area,
    /// "city of", "مدينة", "village": a settlement.
    Settlement,
    /// "strait", "مضيق", "تنگه": only a reviewed marine feature fits.
    Marine,
}
/// A settlement section ("Middle East" in Baltimore is a PPLX): never placed
/// by country context alone.
fn section(r: &LocationResolution) -> bool {
    r.candidates.iter().any(|c| {
        Some(&c.id) == r.selected_candidate_id.as_ref()
            && c.alias_reference.as_ref().is_some_and(|a| {
                ["PPLX", "PPLL", "PPLQ", "PPLH", "PPLW", "PPLS", "PPLF", "PPLR"].iter().any(|code| a["feature_code"] == *code)
            })
    })
}
/// A generic word around a place name, and the country a trailing
/// nationality word names: "مدينة سحاب الأردنية" → ("سحاب", JO, Settlement).
/// Islands are not stripped: an island is not the town it is named for.
fn generic_words(mention: &str) -> Option<(&str, Option<&'static str>, Word)> {
    // Masculine forms; a feminine "ة" ending is folded away first.
    const NATIONAL: [(&str, &str); 23] = [
        ("اليمني", "YE"), ("الأردني", "JO"), ("السعودي", "SA"), ("الإيراني", "IR"), ("الايراني", "IR"),
        ("العراقي", "IQ"), ("السوري", "SY"), ("اللبناني", "LB"), ("الأفغاني", "AF"), ("الفلسطيني", "PS"),
        ("المصري", "EG"), ("العماني", "OM"), ("الإماراتي", "AE"), ("الكويتي", "KW"), ("القطري", "QA"),
        ("البحريني", "BH"), ("التركي", "TR"), ("الباكستاني", "PK"), ("الليبي", "LY"), ("السوداني", "SD"),
        ("الإسرائيلي", "IL"), ("الروسي", "RU"), ("الأوكراني", "UA"),
    ];
    // Divisions: governorate, province, district, county, state, region.
    const DIVISIONS: [&str; 9] = ["محافظة", "ولاية", "مديرية", "قضاء", "ناحية", "استان", "شهرستان", "بخش", "دهستان"];
    const AREAS: [&str; 1] = ["منطقة"];
    const SETTLEMENTS: [&str; 5] = ["مدينة", "بلدة", "قرية", "شهر", "روستای"];
    const MARINE: [&str; 2] = ["مضيق", "تنگه"];
    const SUFFIXES: [(&str, Word); 8] = [(" province", Word::Division), (" governorate", Word::Division), (" county", Word::Division), (" department", Word::Division), (" state", Word::Division), (" municipality", Word::Division), (" region", Word::Area), (" city", Word::Settlement)];
    const PREFIXES: [(&str, Word); 5] = [("city of ", Word::Settlement), ("town of ", Word::Settlement), ("village of ", Word::Settlement), ("province of ", Word::Division), ("governorate of ", Word::Division)];
    let fold = |w: &str| w.chars().filter(|c| !matches!(*c, '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{0640}')).collect::<String>();
    let mut name = mention.trim();
    let mut country = None;
    let mut division = None;
    if let Some((rest, last)) = name.rsplit_once(char::is_whitespace) {
        let last = fold(last);
        let last = last.strip_suffix('ة').unwrap_or(&last);
        if let Some((_, code)) = NATIONAL.iter().find(|(word, _)| last == *word) {
            country = Some(*code);
            name = rest.trim();
        }
    }
    if let Some((first, rest)) = name.split_once(char::is_whitespace) {
        let first = fold(first);
        let kind = if DIVISIONS.contains(&first.as_str()) {
            Some(Word::Division)
        } else if AREAS.contains(&first.as_str()) {
            Some(Word::Area)
        } else if SETTLEMENTS.contains(&first.as_str()) {
            Some(Word::Settlement)
        } else if MARINE.contains(&first.as_str()) {
            Some(Word::Marine)
        } else {
            None
        };
        if kind.is_some() {
            division = kind;
            name = rest.trim();
        }
    }
    if division.is_none() {
        // "the Bab el-Mandeb strait", "Strait of Tiran": the name without the word.
        let lower = name.to_lowercase();
        if lower.len() == name.len() {
            let bare = if lower.starts_with("the ") { &name[4..] } else { name };
            let bare_lower = bare.to_lowercase();
            for suffix in [" strait", " straits"] {
                if bare_lower.ends_with(suffix) {
                    return Some((bare[..bare.len() - suffix.len()].trim(), country, Word::Marine)).filter(|(n, _, _)| n.chars().count() >= 2);
                }
            }
            for prefix in ["strait of ", "straits of "] {
                if bare_lower.starts_with(prefix) {
                    return Some((bare[prefix.len()..].trim(), country, Word::Marine)).filter(|(n, _, _)| n.chars().count() >= 2);
                }
            }
        }
    }
    if division.is_none() {
        let lower = name.to_lowercase();
        if lower.len() == name.len() {
            if let Some((suffix, d)) = SUFFIXES.iter().find(|(s, _)| lower.ends_with(s)) {
                division = Some(*d);
                name = name[..name.len() - suffix.len()].trim();
            } else if let Some((prefix, d)) = PREFIXES.iter().find(|(p, _)| lower.starts_with(p)) {
                division = Some(*d);
                name = name[prefix.len()..].trim();
            }
        }
    }
    (name != mention.trim() && name.chars().count() >= 2).then_some((name, country, division.unwrap_or(Word::Settlement)))
}
/// "Mansouri, southern Lebanon", "Al-Mansouri in southern Lebanon": a place
/// followed by the country (or a compass part of it) it lies in.
fn place_qualifier(mention: &str) -> Option<(&str, &str)> {
    let (name, qualifier) = mention
        .rsplit_once(", ")
        .or_else(|| mention.rsplit_once("، "))
        .or_else(|| mention.rsplit_once(" in "))?;
    let (name, qualifier) = (name.trim(), qualifier.trim());
    (!name.is_empty() && !qualifier.is_empty()).then_some((name, qualifier))
}
/// The city named in a facility phrase: "Riyadh airport", "مطار الرياض الدولي".
fn facility_city(mention: &str) -> Option<&str> {
    const SUFFIXES: [&str; 9] = [" international airport", " airport", " air base", " airbase", " naval base", " seaport", " port", " railway station", " train station"];
    const PREFIXES: [&str; 6] = ["مطار ", "ميناء ", "فرودگاه ", "بندرگاه ", "port of ", "נמל התעופה "];
    let lower = mention.to_lowercase();
    let mut city = None;
    for suffix in SUFFIXES {
        if lower.ends_with(suffix) && lower.is_char_boundary(lower.len() - suffix.len()) && mention.len() == lower.len() {
            city = mention.get(..mention.len() - suffix.len());
            break;
        }
    }
    if city.is_none() {
        for prefix in PREFIXES {
            if lower.starts_with(prefix) && mention.len() == lower.len() {
                city = mention.get(prefix.len()..);
                break;
            }
        }
    }
    let mut city = city?.trim();
    for tail in [" الدولي", " الدولية", " بین‌المللی", " بین المللی"] {
        city = city.strip_suffix(tail).unwrap_or(city).trim();
    }
    (city.chars().count() >= 2).then_some(city)
}
/// UN, US, IDF, U.S.: two to five capitals, optionally dotted.
fn acronym(mention: &str) -> bool {
    let letters = mention.chars().filter(|c| *c != '.').collect::<String>();
    (2..=5).contains(&letters.chars().count())
        && letters.chars().all(|c| c.is_ascii_uppercase())
        && mention.chars().all(|c| c.is_ascii_uppercase() || c == '.')
}
/// A leading compass qualifier and the named place it qualifies.
fn directional(mention: &str) -> Option<(&str, &str)> {
    const WORDS: [&str; 40] = [
        "northeastern ", "northwestern ", "southeastern ", "southwestern ",
        "north-eastern ", "north-western ", "south-eastern ", "south-western ",
        "northeast ", "northwest ", "southeast ", "southwest ",
        "northern ", "southern ", "eastern ", "western ", "central ",
        "north ", "south ", "east ", "west ",
        "شمال شرق ", "شمال غرب ", "جنوب شرق ", "جنوب غرب ",
        "شمالي ", "جنوبي ", "شمال ", "جنوب ", "شرق ", "غرب ", "وسط ", "مرکز ",
        "شمال‌شرق ", "جنوب‌شرق ", "צפון ", "דרום ", "מזרח ", "מערב ", "מרכז ",
    ];
    let lower = mention.to_lowercase();
    for word in WORDS {
        if lower.starts_with(word) {
            let inner = mention.get(word.len()..)?.trim();
            if inner.chars().count() >= 2 {
                return Some((mention[..word.len()].trim(), inner));
            }
        }
    }
    None
}

fn validate_proposal(proposal: &LocationProposal) -> Result<()> {
    for (text, max) in [(&proposal.mention, 240), (&proposal.quote, 4000)] {
        if text.trim().is_empty() || text.len() > max {
            return Err(Error::invalid(
                "Location mention and exact quote must be bounded nonempty text",
            ));
        }
    }
    if proposal
        .canonical_name
        .as_ref()
        .is_some_and(|name| name.trim().is_empty() || name.len() > 240)
    {
        return Err(Error::invalid(
            "Canonical place name must be bounded text or null",
        ));
    }
    if proposal
        .country_code
        .as_ref()
        .is_some_and(|code| code.len() != 2 || !code.bytes().all(|byte| byte.is_ascii_alphabetic()))
    {
        return Err(Error::invalid(
            "Location country constraint must be a two-letter publisher ISO code or null",
        ));
    }
    Ok(())
}
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn optional_text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.trim().is_empty() && *text != "-99")
        .map(|text| text.trim().into())
}
fn iso2(value: &Value) -> Option<String> {
    optional_text(value)
        .filter(|code| code.len() == 2 && code.bytes().all(|byte| byte.is_ascii_alphabetic()))
        .map(|code| code.to_ascii_uppercase())
}
fn preferred_name(properties: &Value) -> Result<String> {
    optional_text(&properties["NAME_EN"])
        .or_else(|| optional_text(&properties["NAME"]))
        .ok_or_else(|| Error::invalid("Publisher place lacks a name"))
}
fn publisher_id(properties: &Value) -> Result<String> {
    properties["NE_ID"]
        .as_u64()
        .filter(|id| *id > 0)
        .map(|id| id.to_string())
        .ok_or_else(|| Error::invalid("Publisher place lacks a stable NE_ID"))
}
fn publisher_aliases(properties: &Value, name: &str) -> BTreeMap<String, Alias> {
    let mut aliases = BTreeMap::new();
    aliases.insert(
        normalize(name),
        Alias {
            original: name.into(),
            reference: None,
        },
    );
    if let Some(properties) = properties.as_object() {
        for (field, value) in properties {
            let language_name = field.strip_prefix("NAME_").is_some_and(|suffix| {
                (2..=3).contains(&suffix.len())
                    && suffix.bytes().all(|byte| byte.is_ascii_uppercase())
            });
            if (language_name
                || [
                    "NAME",
                    "NAMEASCII",
                    "NAMEALT",
                    "NAME_LONG",
                    "NAME_ALT",
                    "BRK_NAME",
                ]
                .contains(&field.as_str()))
                && let Some(text) = optional_text(value)
            {
                aliases.entry(normalize(&text)).or_insert(Alias {
                    original: text,
                    reference: None,
                });
            }
        }
    }
    aliases
}
fn bounded_read(path: &Path) -> Result<Vec<u8>> {
    if fs::metadata(path)?.len() > MAX_REFERENCE_BYTES {
        return Err(Error::invalid("Reference file exceeds the 2 MiB bound"));
    }
    let bytes = fs::read(path)?;
    if bytes.len() as u64 > MAX_REFERENCE_BYTES {
        return Err(Error::invalid("Reference file exceeds the 2 MiB bound"));
    }
    Ok(bytes)
}
fn verified_read(path: &Path, expected: &str) -> Result<Vec<u8>> {
    let bytes = bounded_read(path)?;
    if hash(&bytes) != expected {
        return Err(Error::invalid(
            "Accepted reference checksum mismatch; prior interpretations remain retained",
        ));
    }
    Ok(bytes)
}
fn features(collection: &Value, count: usize) -> Result<&Vec<Value>> {
    let features = collection["features"]
        .as_array()
        .filter(|features| features.len() == count)
        .ok_or_else(|| Error::invalid("Accepted reference feature count changed"))?;
    if collection["type"] != "FeatureCollection" {
        return Err(Error::invalid(
            "Reference is not a GeoJSON FeatureCollection",
        ));
    }
    Ok(features)
}
fn validate_point(geometry: &Value) -> Result<()> {
    let coordinates = geometry["coordinates"]
        .as_array()
        .filter(|coordinates| coordinates.len() == 2)
        .ok_or_else(|| Error::invalid("City reference requires a two-dimensional point"))?;
    let lon = coordinates[0]
        .as_f64()
        .ok_or_else(|| Error::invalid("Invalid city longitude"))?;
    let lat = coordinates[1]
        .as_f64()
        .ok_or_else(|| Error::invalid("Invalid city latitude"))?;
    if geometry["type"] != "Point"
        || !lon.is_finite()
        || !lat.is_finite()
        || lon.abs() > 180.
        || lat.abs() > 90.
    {
        return Err(Error::invalid("City point lies outside WGS84 bounds"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn resolver() -> GeoResolver {
        GeoResolver::load(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()).unwrap()
    }
    /// A fictional cities500-style edition from GeoNames rows; alternate names
    /// in field 4 become alias-only (possible-name) collision evidence.
    fn fictional_global(tmp: &Path, rows: &str) -> crate::geonames::Index {
        let places = tmp.join("places.txt");
        let aliases = tmp.join("aliases.txt");
        fs::write(&places, rows).unwrap();
        fs::write(&aliases, []).unwrap();
        let path = tmp.join("edition");
        crate::geonames::build(&path, &places, &aliases, json!({"product":"cities500","fictional":true}), &std::sync::atomic::AtomicBool::new(false)).unwrap();
        crate::geonames::Index::inspect(&path).unwrap()
    }
    fn fictional_event(mention: &str) -> LocationProposal {
        LocationProposal { mention: mention.into(), quote: format!("Fictional strike reported in {mention} today."), role: LocationRole::Event, canonical_name: None, country_code: None }
    }
    #[allow(clippy::too_many_arguments)]
    fn row(id: u32, name: &str, alternates: &str, class: &str, code: &str, country: &str, population: u64, lat: f64, lon: f64) -> String {
        format!("{id}\t{name}\t{name}\t{alternates}\t{lat}\t{lon}\t{class}\t{code}\t{country}\t\t01\t\t\t\t{population}\t\t10\tAsia/Aden\t2026-09-12\n")
    }
    #[test]
    fn a_capital_dominates_its_own_divisions_minor_features_and_small_namesakes() {
        let tmp = tempfile::tempdir().unwrap();
        let rows = [
            row(900101, "Fictopolis", "", "P", "PPLC", "YE", 2_000_000, 15.0, 44.0),
            row(900102, "Fictopolis", "", "A", "ADM1", "YE", 3_000_000, 15.1, 44.1),
            row(900103, "Fictopolis", "", "P", "PPL", "YE", 0, 13.0, 45.0),
            row(900104, "Fictopolis", "", "S", "SCH", "IQ", 0, 33.0, 44.0),
            row(900105, "Fictopolis", "", "P", "PPL", "SA", 30_000, 24.0, 46.0),
        ].concat();
        let mut resolver = resolver();
        resolver.indexes = vec![fictional_global(tmp.path(), &rows)];
        let p = fictional_event("Fictopolis");
        let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
        assert_eq!(r[0].status, "resolved", "{}", r[0].reason);
        assert_eq!(r[0].selected_candidate_id.as_deref(), Some("geonames:900101"));
        assert!(r[0].candidates.len() >= 5, "alternatives stay available");
    }
    #[test]
    fn a_lone_namesake_abroad_is_not_placed_against_the_message_context() {
        let tmp = tempfile::tempdir().unwrap();
        // The only "Fictamoud" listed is in Mauritania; the message is about Syria.
        let rows = [
            row(900301, "Fictamoud", "", "P", "PPL", "MR", 2_000, 18.0, -12.0),
            row(900302, "Fictaport", "", "P", "PPLA", "MR", 500_000, 18.1, -15.9),
        ].concat();
        let mut resolver = resolver();
        resolver.indexes = vec![fictional_global(tmp.path(), &rows)];
        let text = "Fictional attack in Fictamoud. Fictional strikes in Syria.";
        let proposal = |mention: &str, quote: &str| LocationProposal { mention: mention.into(), quote: quote.into(), role: LocationRole::Event, canonical_name: None, country_code: None };
        let place = proposal("Fictamoud", "attack in Fictamoud");
        let syria = proposal("Syria", "strikes in Syria");
        // Alone, the unique name stands: nothing in the message contradicts it.
        let alone = resolver.resolve(text, std::slice::from_ref(&place), true).unwrap();
        assert_eq!(alone[0].status, "resolved", "{}", alone[0].reason);
        // Beside Syria it is a namesake abroad and is not placed.
        let (both, country) = resolver.resolve_message(text, &[place.clone(), syria], true).unwrap();
        assert_eq!(both[0].status, FOREIGN_NAMESAKE, "{}", both[0].reason);
        assert!(both[0].geometry.is_none() && !both[0].occurrence_eligible && both[0].selected_candidate_id.is_none());
        assert_eq!(country.as_deref(), Some("SY"));
        // A claim naming only that place has no other event place of its own;
        // it follows the message's reading of the same mention.
        let mut lone = resolver.resolve_in(text, std::slice::from_ref(&place), true, country.as_deref()).unwrap();
        assert_eq!(lone[0].status, "resolved");
        follow_message(&both, &[0], &mut lone);
        assert_eq!(lone[0].status, FOREIGN_NAMESAKE);
        assert!(lone[0].geometry.is_none());
        // A regional capital abroad stays placed beside Syria, and a country
        // only mentioned is no evidence against a small place.
        let text = "Fictional talks in Fictaport. Fictional attack in Fictamoud. Fictional strikes in Syria.";
        let port = proposal("Fictaport", "talks in Fictaport");
        let place = proposal("Fictamoud", "attack in Fictamoud");
        let (r, _) = resolver.resolve_message(text, &[port, proposal("Syria", "strikes in Syria")], true).unwrap();
        assert_eq!(r[0].status, "resolved", "{}", r[0].reason);
        let mut mentioned = proposal("Syria", "strikes in Syria");
        mentioned.role = LocationRole::Mentioned;
        let (r, _) = resolver.resolve_message(text, &[place, mentioned], true).unwrap();
        assert_eq!(r[0].status, "resolved", "{}", r[0].reason);
    }
    #[test]
    fn comparable_namesakes_and_alias_only_collisions_are_never_chosen() {
        let tmp = tempfile::tempdir().unwrap();
        // A foreign namesake at 20% of the principal's size blocks the choice.
        let rows = [
            row(900201, "Twinford", "", "P", "PPLA", "YE", 1_000_000, 15.0, 44.0),
            row(900202, "Twinford", "", "P", "PPLA", "SA", 200_000, 24.0, 46.0),
        ].concat();
        let mut resolver = resolver();
        resolver.indexes = vec![fictional_global(tmp.path(), &rows)];
        let p = fictional_event("Twinford");
        let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
        assert_eq!(r[0].status, "ambiguous");
        assert!(r[0].geometry.is_none());
        // A large city that only lists the name as a flattened alternate can
        // block a small exact match but can never be selected itself.
        let tmp = tempfile::tempdir().unwrap();
        let rows = [
            row(900301, "Pale Hall", "", "P", "PPL", "US", 11_000, 35.0, -86.0),
            row(900302, "Grand Harbour", "Pale Hall", "P", "PPLA", "MA", 3_600_000, 33.6, -7.6),
        ].concat();
        resolver.indexes = vec![fictional_global(tmp.path(), &rows)];
        let p = fictional_event("Pale Hall");
        let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
        assert!(r[0].geometry.is_none(), "{} {:?}", r[0].status, r[0].selected_candidate_id);
        assert_ne!(r[0].selected_candidate_id.as_deref(), Some("geonames:900302"));
    }
    #[test]
    fn a_bare_neighbourhood_name_needs_country_context() {
        let tmp = tempfile::tempdir().unwrap();
        let rows = row(900401, "Middle Quarter", "", "P", "PPLX", "US", 1_800, 39.3, -76.6);
        let mut resolver = resolver();
        resolver.indexes = vec![fictional_global(tmp.path(), &rows)];
        let mut p = fictional_event("Middle Quarter");
        let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
        assert_eq!(r[0].status, "country_context_required");
        assert!(r[0].geometry.is_none());
        p.country_code = Some("US".into());
        let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
        assert_eq!(r[0].status, "resolved");
    }
    #[test]
    fn articles_directions_facilities_and_acronyms_are_read_literally() {
        assert_eq!(article_free("the Strait of Hormuz"), Some("Strait of Hormuz"));
        assert_eq!(article_free("The Hague"), Some("Hague"));
        assert_eq!(article_free("Thebes"), None);
        assert_eq!(directional("southern Lebanon"), Some(("southern", "Lebanon")));
        assert_eq!(directional("جنوب لبنان"), Some(("جنوب", "لبنان")));
        assert_eq!(directional("Westfield"), None);
        assert_eq!(article_free("Al-Mansouri"), Some("Mansouri"));
        assert_eq!(article_free("the Strait of Hormuz"), Some("Strait of Hormuz"));
        assert_eq!(article_free("Alexandria"), None);
        assert_eq!(generic_suffix("Chitgar area"), Some("Chitgar"));
        assert_eq!(generic_suffix("Area"), None);
        assert_eq!(place_qualifier("Mansouri, southern Lebanon"), Some(("Mansouri", "southern Lebanon")));
        assert_eq!(place_qualifier("Al-Mansouri in southern Lebanon"), Some(("Al-Mansouri", "southern Lebanon")));
        assert_eq!(place_qualifier("Mansouri"), None);
        assert_eq!(facility_city("Riyadh airport"), Some("Riyadh"));
        assert_eq!(facility_city("مطار الرياض الدولي"), Some("الرياض"));
        assert_eq!(facility_city("Kyiv railway station"), Some("Kyiv"));
        assert_eq!(facility_city("Riyadh"), None);
        assert!(acronym("UN") && acronym("U.S.") && acronym("UAE"));
        assert!(!acronym("Un") && !acronym("Gaza") && !acronym("UNGAX1"));
        assert!((distance_km((51.4, 35.7), (51.3, 35.7)) - 9.0).abs() < 1.0);
        let keys = crate::geonames::lookup_keys("جزیره لارَک");
        assert!(keys.contains(&"جزیره لارک".to_string()));
        assert!(crate::geonames::lookup_keys("Sana’a").contains(&"sana'a".to_string()));
        assert_eq!(crate::geonames::lookup_keys("Gaza"), vec!["gaza".to_string()]);
        // A direction before a real name is not split when the name exists.
        let resolver = resolver();
        let p = fictional_event("southern Lebanon");
        let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
        assert_eq!(r[0].mention, "southern Lebanon");
        assert!(r[0].selected_candidate_id.is_some(), "{}", r[0].reason);
        let p = fictional_event("UN");
        let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
        assert_eq!(r[0].status, "unresolved");
    }
    #[test]
    fn regional_homonyms_cannot_shadow_accepted_cross_country_names() {
        let tmp = tempfile::tempdir().unwrap();
        let places = tmp.path().join("places.txt");
        let aliases = tmp.path().join("aliases.txt");
        // Fictional gazetteer rows deliberately collide with the accepted US
        // alias. Their invented coordinates are test data, never a live release.
        fs::write(&places, "900031\tNew York\tNew York\t\t10\t20\tP\tPPL\tUA\t\t01\t\t\t\t1000\t\t10\tEurope/Kyiv\t2026-09-12\n900032\tNew York City\tNew York City\t\t30\t40\tP\tPPL\tUS\t\t01\t\t\t\t1000\t\t10\tAmerica/New_York\t2026-09-12\n").unwrap();
        fs::write(&aliases, []).unwrap();
        let path = tmp.path().join("edition");
        crate::geonames::build(
            &path,
            &places,
            &aliases,
            json!({"product":"cities500","fictional":true}),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        let mut resolver = resolver();
        resolver.indexes = vec![crate::geonames::Index::inspect(&path).unwrap()];
        let mut p = LocationProposal {
            mention: "New York".into(),
            quote: "A fictional meeting in New York.".into(),
            role: LocationRole::Event,
            canonical_name: None,
            country_code: None,
        };
        // The accepted world city dominates a village of 1,000 abroad.
        let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
        assert_eq!(r[0].status, "resolved");
        assert_eq!(r[0].candidates[0].country_code.as_deref(), Some("US"));
        for country in ["US", "UA"] {
            p.country_code = Some(country.into());
            let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
            assert_eq!(r[0].status, "resolved");
            assert_eq!(r[0].candidates[0].country_code.as_deref(), Some(country));
        }
        p.country_code = None;
        p.canonical_name = Some("New York City".into());
        let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
        assert_eq!(r[0].status, "resolved");
        assert_eq!(r[0].candidates[0].country_code.as_deref(), Some("US"));
        p.mention = "New York, United States".into();
        p.quote = format!("Fictional meeting in {}.", p.mention);
        p.canonical_name = None;
        let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
        assert_eq!(r[0].status, "resolved");
        assert_eq!(r[0].candidates[0].country_code.as_deref(), Some("US"));
        p.role = LocationRole::Issuer;
        let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
        assert!(r[0].geometry.is_none());
    }
    #[test]
    fn country_counts_ignore_names_nested_inside_longer_country_names() {
        let resolver = resolver();
        for (text, count) in [
            ("South Sudan", 1),
            ("Sudan and South Sudan", 2),
            ("Papua New Guinea", 1),
            ("Guinea and Papua New Guinea", 2),
            ("Lebanon and Syria", 2),
            ("Sudan, Sudan", 1),
        ] {
            assert_eq!(resolver.country_count(text), count, "{text}");
        }
    }
    #[test]
    fn evaluation_locations_preserve_roles_exact_support_and_abstention() {
        let resolver = resolver();
        let fixture: Value =
            serde_json::from_str(include_str!("../../examples/processing-evaluation.json"))
                .unwrap();
        assert_eq!(fixture["fictional"], true);
        let cases = fixture["cases"].as_array().unwrap();
        assert!(cases.len() >= 12);
        for case in cases {
            let proposals: Vec<LocationProposal> =
                serde_json::from_value(case["proposals"].clone()).unwrap();
            let allowed = case["expected"]["kind"] == "event"
                && case["expected"]["claim_status"] == "reported";
            let resolved = resolver
                .resolve(case["text"].as_str().unwrap(), &proposals, allowed)
                .unwrap();
            let statuses = resolved
                .iter()
                .map(|location| location.status.as_str())
                .collect::<Vec<_>>();
            assert_eq!(
                json!(statuses),
                case["expected"]["location_statuses"],
                "{}",
                case["id"]
            );
            let mapped = resolved
                .iter()
                .filter(|location| location.occurrence_eligible)
                .map(|location| location.candidates[0].name.clone())
                .collect::<Vec<_>>();
            assert_eq!(
                json!(mapped),
                case["expected"]["mapped_event_names"],
                "{}",
                case["id"]
            );
            for location in resolved {
                if location.geometry.is_some() {
                    assert!(allowed && location.role == LocationRole::Event);
                    assert_eq!(location.precision, "city_level_approximate");
                }
                for candidate in location.candidates {
                    assert!(!candidate.reference.source_sha256.is_empty());
                    if candidate.kind == "country" {
                        assert!(candidate.geometry.is_none());
                    }
                }
            }
        }
    }
    #[test]
    fn acceptance_retains_243_original_city_points_and_exposes_sparse_coverage() {
        let resolver = resolver();
        assert_eq!(resolver.reference_summary()["cities"], 243);
        assert_eq!(resolver.reference_summary()["countries"], 210);
        for place in ["Haifa", "Gaza", "Isfahan", "Aleppo", "Basra", "Mosul"] {
            let proposal = LocationProposal {
                mention: place.into(),
                quote: place.into(),
                role: LocationRole::Event,
                canonical_name: None,
                country_code: None,
            };
            let result = resolver.resolve(place, &[proposal], true).unwrap();
            assert_eq!(result[0].status, "unresolved");
            assert!(result[0].geometry.is_none());
        }
        let entry = resolver
            .entries
            .iter()
            .find(|entry| entry.candidate.name == "Tehran")
            .unwrap();
        assert_eq!(
            entry.candidate.geometry,
            Some(json!({"type":"Point","coordinates":[51.422398,35.673889]}))
        );
        assert_eq!(entry.candidate.reference.publisher_record_id, "1159151551");
    }
    #[test]
    fn model_coordinates_invalid_countries_and_oversized_requests_are_rejected() {
        assert!(serde_json::from_value::<LocationProposal>(json!({"mention":"Tehran","quote":"Tehran","role":"event","latitude":1,"longitude":2})).is_err());
        let resolver = resolver();
        let proposal = LocationProposal {
            mention: "Tehran".into(),
            quote: "Tehran".into(),
            role: LocationRole::Event,
            canonical_name: None,
            country_code: Some("IRN".into()),
        };
        assert!(resolver.resolve("Tehran", &[proposal], true).is_err());
        assert!(
            resolver
                .resolve(&"x".repeat(64 * 1024 + 1), &[], true)
                .is_err()
        );
    }

    fn copy_reference() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let relative = "reference/accepted/natural-earth";
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(relative);
        let destination = directory.path().join(relative);
        fs::create_dir_all(destination.join("raw")).unwrap();
        for file in [
            "processing-manifest.json",
            "processing-aliases.json",
            "raw/ne_110m_populated_places.geojson",
            "raw/ne_110m_admin_0_countries.geojson",
            "raw/ne_110m_admin_0_tiny_countries.geojson",
        ] {
            fs::copy(source.join(file), destination.join(file)).unwrap();
        }
        directory
    }

    #[test]
    fn changed_source_or_alias_bytes_cannot_silently_change_reference() {
        for file in [
            "raw/ne_110m_populated_places.geojson",
            "raw/ne_110m_admin_0_countries.geojson",
            "raw/ne_110m_admin_0_tiny_countries.geojson",
            "processing-aliases.json",
        ] {
            let directory = copy_reference();
            assert!(GeoResolver::load(directory.path()).is_ok());
            let path = directory
                .path()
                .join("reference/accepted/natural-earth")
                .join(file);
            let mut bytes = fs::read(&path).unwrap();
            bytes.push(b'\n');
            fs::write(path, bytes).unwrap();
            assert!(GeoResolver::load(directory.path()).is_err(), "{file}");
        }
    }

    #[test]
    fn alias_acceptance_checks_the_original_publisher_field() {
        let directory = copy_reference();
        let root = directory.path().join("reference/accepted/natural-earth");
        let mut aliases: Value =
            serde_json::from_slice(&fs::read(root.join("processing-aliases.json")).unwrap())
                .unwrap();
        aliases["rules"][0]["publisher_value"] = json!("A name the publisher never supplied");
        let bytes = serde_json::to_vec(&aliases).unwrap();
        fs::write(root.join("processing-aliases.json"), &bytes).unwrap();
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(root.join("processing-manifest.json")).unwrap())
                .unwrap();
        manifest["aliases"]["sha256"] = json!(hash(&bytes));
        fs::write(
            root.join("processing-manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(GeoResolver::load(directory.path()).is_err());
    }

    #[test]
    fn repeated_quotes_and_large_ambiguity_sets_never_select_a_city() {
        let mut resolver = resolver();
        let mut proposal = LocationProposal {
            mention: "Tehran".into(),
            quote: "Tehran".into(),
            role: LocationRole::Event,
            canonical_name: None,
            country_code: None,
        };
        let repeated = resolver
            .resolve("Tehran; Tehran", &[proposal.clone()], true)
            .unwrap();
        assert_eq!(repeated[0].status, "unsupported_quote");
        assert!(repeated[0].geometry.is_none());
        // An intentionally ambiguous in-memory index proves the output bound;
        // each candidate still has its actual retained publisher identity.
        resolver
            .index
            .insert("synthetic ambiguous name".into(), (0..MAX_CANDIDATES + 1).collect());
        proposal.mention = "synthetic ambiguous name".into();
        proposal.quote = proposal.mention.clone();
        let ambiguous = resolver
            .resolve(&proposal.mention, &[proposal.clone()], true)
            .unwrap();
        assert_eq!(ambiguous[0].status, "ambiguous");
        assert_eq!(ambiguous[0].candidate_count, MAX_CANDIDATES + 1);
        assert_eq!(ambiguous[0].candidates.len(), MAX_CANDIDATES);
        assert!(ambiguous[0].candidates_truncated);
        assert!(ambiguous[0].selected_candidate_id.is_none());
        assert!(ambiguous[0].geometry.is_none());
    }
    fn fictional_national(tmp: &Path, rows: &str, country: &str) -> crate::geonames::Index {
        let dir = tmp.join(country);
        fs::create_dir_all(&dir).unwrap();
        let places = dir.join("places.txt");
        let aliases = dir.join("aliases.txt");
        fs::write(&places, rows).unwrap();
        fs::write(&aliases, []).unwrap();
        let path = dir.join("edition");
        crate::geonames::build(&path, &places, &aliases, json!({"product":country,"fictional":true}), &std::sync::atomic::AtomicBool::new(false)).unwrap();
        crate::geonames::Index::inspect(&path).unwrap()
    }
    #[test]
    fn a_message_country_places_names_only_with_a_national_gazetteer() {
        let tmp = tempfile::tempdir().unwrap();
        let national = [
            row(910001, "Northvale", "", "P", "PPLA", "LB", 50_000, 33.3, 35.4),
            row(910002, "Quietmoor", "", "P", "PPL", "LB", 0, 33.2, 35.5),
        ].concat();
        // Northvale is also in the global list, so it is placed and anchors
        // the message; a name that itself needs country context would not.
        let global = [
            row(910001, "Northvale", "", "P", "PPLA", "LB", 50_000, 33.3, 35.4),
            row(910003, "Quietmoor", "", "P", "PPL", "SY", 0, 35.0, 38.0),
            row(910004, "Old Quarter", "", "P", "PPLX", "US", 0, 39.3, -76.6),
            row(910005, "Capitol Springs", "", "P", "PPLA", "US", 60_000, 39.0, -77.0),
        ].concat();
        let mut resolver = resolver();
        resolver.indexes = vec![fictional_national(tmp.path(), &national, "LB"), fictional_global(tmp.path(), &global)];
        let text = "Fictional strikes hit Northvale and Quietmoor overnight.";
        let place = |mention: &str| LocationProposal { mention: mention.into(), quote: text.into(), role: LocationRole::Event, canonical_name: None, country_code: None };
        // Alone, Quietmoor names places in two countries.
        let alone = resolver.resolve(text, &[place("Quietmoor")], true).unwrap();
        assert!(alone[0].geometry.is_none(), "{}", alone[0].status);
        let (together, country) = resolver.resolve_message(text, &[place("Northvale"), place("Quietmoor")], true).unwrap();
        assert_eq!(country.as_deref(), Some("LB"));
        assert_eq!(together[1].selected_candidate_id.as_deref(), Some("geonames:910002"), "{}", together[1].reason);
        assert_eq!(together[1].literal_qualifier, Some(json!({"message_country":"LB"})));
        // Without a national gazetteer, a country alone is too thin: a US
        // neighbourhood is never placed because the message mentions the US.
        let text = "Fictional officials in Capitol Springs discussed the Old Quarter.";
        let place = |mention: &str| LocationProposal { mention: mention.into(), quote: text.into(), role: LocationRole::Event, canonical_name: None, country_code: None };
        let (us, country) = resolver.resolve_message(text, &[place("Capitol Springs"), place("Old Quarter")], true).unwrap();
        assert_eq!(country.as_deref(), Some("US"));
        assert!(us[1].geometry.is_none(), "{} {}", us[1].status, us[1].reason);
        // Two countries in one message give no message country.
        let mixed = vec![
            LocationResolution { selected_candidate_id: Some("a".into()), candidates: vec![LocationCandidate { id: "a".into(), country_code: Some("LB".into()), ..together[0].candidates[0].clone() }], ..together[0].clone() },
            LocationResolution { selected_candidate_id: Some("b".into()), candidates: vec![LocationCandidate { id: "b".into(), country_code: Some("SY".into()), ..together[0].candidates[0].clone() }], ..together[0].clone() },
        ];
        assert_eq!(message_country(&mixed), None);
        // A namesake still waiting for country context is not a message place.
        let waiting = vec![
            together[0].clone(),
            LocationResolution { status: "country_context_required".into(), selected_candidate_id: Some("b".into()), candidates: vec![LocationCandidate { id: "b".into(), country_code: Some("ER".into()), ..together[0].candidates[0].clone() }], ..together[0].clone() },
        ];
        assert_eq!(message_country(&waiting).as_deref(), Some("LB"));
    }
    #[test]
    fn close_spellings_resolve_only_uniquely_within_a_national_gazetteer() {
        let tmp = tempfile::tempdir().unwrap();
        let national = [
            row(920001, "Baraachît", "", "P", "PPL", "LB", 0, 33.2, 35.4),
            row(920002, "Zaoutar ech Charqîyé", "", "P", "PPL", "LB", 0, 33.3, 35.6),
            row(920003, "Hulbta", "", "P", "PPL", "LB", 0, 34.5, 36.2),
            row(920004, "Kafrana", "", "P", "PPL", "LB", 0, 33.4, 35.5),
            row(920005, "Kafrani", "", "P", "PPL", "LB", 0, 33.5, 35.5),
        ].concat();
        // An exact namesake abroad does not stop the Lebanese spelling.
        let eritrea = row(920006, "Barachit", "", "P", "PPL", "ER", 0, 15.0, 39.0);
        let mut resolver = resolver();
        resolver.indexes = vec![fictional_national(tmp.path(), &national, "LB"), fictional_national(tmp.path(), &eritrea, "ER")];
        let in_lebanon = |mention: &str| LocationProposal { mention: mention.into(), quote: format!("Fictional strike reported in {mention} today."), role: LocationRole::Event, canonical_name: None, country_code: Some("LB".into()) };
        let resolve = |mention: &str| { let p = in_lebanon(mention); resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap().remove(0) };
        let r = resolve("Barachit");
        assert_eq!(r.selected_candidate_id.as_deref(), Some("geonames:920001"), "{}", r.reason);
        assert_eq!(r.candidates[0].match_basis, "spelling_variant");
        assert_eq!(r.precision, "city_level_approximate");
        assert_eq!(resolve("Zawtar al-Sharqiyyah").selected_candidate_id.as_deref(), Some("geonames:920002"));
        // A consonant added to a short name is another place.
        assert!(resolve("Hulta").geometry.is_none());
        // Two equally close spellings: no guess.
        assert!(resolve("Kafranu").geometry.is_none());
        // Arabic script and unknown countries are never guessed.
        let mut elsewhere = in_lebanon("Barachit");
        elsewhere.country_code = Some("SY".into());
        assert!(resolver.resolve(&elsewhere.quote, std::slice::from_ref(&elsewhere), true).unwrap()[0].geometry.is_none());
        assert_eq!(crate::geonames::latin_key("برعشيت"), None);
        assert_eq!(crate::geonames::latin_key("Al-Khiam"), crate::geonames::latin_key("Khiam"));
        assert_eq!(crate::geonames::spelling_distance("tulkaram", "tulkarm"), 1);
    }
    #[test]
    fn generic_words_name_a_settlement_or_a_division() {
        assert_eq!(generic_words("مدينة سحاب الأردنية"), Some(("سحاب", Some("JO"), Word::Settlement)));
        assert_eq!(generic_words("محافظة تعز اليمنية"), Some(("تعز", Some("YE"), Word::Division)));
        assert_eq!(generic_words("منطقةِ جيزانَ"), Some(("جيزانَ", None, Word::Area)));
        assert_eq!(generic_words("Borno State"), Some(("Borno", None, Word::Division)));
        assert_eq!(generic_words("Bekaa region"), Some(("Bekaa", None, Word::Area)));
        // Islands are not their towns; a bare name has nothing to strip.
        assert_eq!(generic_words("جزیره قشم"), None);
        assert_eq!(generic_words("Kharg Island"), None);
        assert_eq!(generic_words("السعودية"), None);
    }
    #[test]
    fn a_division_named_for_its_seat_is_not_its_seat_city() {
        let tmp = tempfile::tempdir().unwrap();
        let rows = [
            row(930001, "Seatford", "", "P", "PPLA", "YE", 200_000, 13.6, 44.0),
            row(930002, "Quietford", "", "P", "PPL", "YE", 0, 13.0, 44.5),
        ].concat();
        let mut resolver = resolver();
        resolver.indexes = vec![fictional_national(tmp.path(), &rows, "YE")];
        // Without loaded boundaries there is no province to show: the city
        // point is not substituted for "Seatford governorate".
        let p = LocationProposal { mention: "Seatford governorate".into(), quote: "Fictional strikes across Seatford governorate.".into(), role: LocationRole::Event, canonical_name: None, country_code: Some("YE".into()) };
        let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
        assert!(r[0].geometry.is_none(), "{} {}", r[0].status, r[0].reason);
        // "city of" keeps the city.
        let p = LocationProposal { mention: "city of Seatford".into(), quote: "Fictional strikes in the city of Seatford.".into(), role: LocationRole::Event, canonical_name: None, country_code: Some("YE".into()) };
        let r = resolver.resolve(&p.quote, std::slice::from_ref(&p), true).unwrap();
        assert_eq!(r[0].selected_candidate_id.as_deref(), Some("geonames:930001"), "{}", r[0].reason);
    }
    #[test]
    fn reviewed_corrections_name_quneitra_and_join_the_context_identity() {
        assert!(population_not_used("geonames:173334"));
        assert!(!population_not_used("geonames:173336"));
        let corrections: Value = serde_json::from_str(CORRECTIONS).unwrap();
        assert!(corrections["population_not_used"].as_array().unwrap().iter().all(|c| c["reason"].as_str().is_some_and(|r| !r.is_empty()) && c["reviewed"].is_string()));
        let with = crate::boundary_store::combined_edition(Some(&json!({"edition":"x"})), None, None, None).unwrap();
        assert_eq!(with.len(), 64);
        assert_eq!(corrections_hash().len(), 64);
    }
}
