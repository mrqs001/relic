//! GeoNames reference index: immutable editions, exact names and bounded lookup.
//! This is reference geography, never evidence of an incident or site observation.
use crate::{
    error::{Error, Result},
    media_store::file_hash,
    model::{hash, required},
    processing_geo::{
        LocationCandidate, LocationProposal, LocationResolution, LocationRole, ReferenceIdentity,
    },
};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use unicode_normalization::UnicodeNormalization;

/// Malformed-data guard: a GeoNames record line is far shorter than this.
/// There is no file-size or row-count cap on a selected country or the world.
const MAX_LINE: u64 = 64 * 1024;
/// Candidates kept per lookup. Large enough that a capital's many small
/// namesakes (27 places named Baghdad) stay visible to the dominance rule.
const MAX_RESULTS: usize = 100;
pub(crate) fn key(s: &str) -> String {
    s.nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
/// Keys to try for a source mention: the exact key first, then one folded form
/// without Arabic/Hebrew vowel marks or tatweel, both apostrophe forms, and the
/// Persian/Arabic letter forms of yeh and kaf. Stored keys are unchanged, so an
/// exact match is always still found; folding only adds spelling variants.
pub(crate) fn lookup_keys(s: &str) -> Vec<String> {
    let exact = key(s);
    let folded: String = exact
        .chars()
        .filter(|c| {
            !matches!(*c, '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{0640}' | '\u{0591}'..='\u{05BD}' | '\u{05BF}' | '\u{05C1}' | '\u{05C2}' | '\u{05C4}' | '\u{05C5}' | '\u{05C7}')
        })
        .map(|c| match c {
            '\u{2019}' | '\u{2018}' | '\u{02BC}' | '\u{02BB}' | '`' | '\u{00B4}' | '\u{2032}' => '\'',
            c => c,
        })
        .collect();
    let curly = folded.replace('\'', "\u{2019}");
    let persian: String = folded.chars().map(|c| match c { '\u{064A}' | '\u{0649}' => '\u{06CC}', '\u{0643}' => '\u{06A9}', c => c }).collect();
    let arabic: String = folded.chars().map(|c| match c { '\u{06CC}' => '\u{064A}', '\u{06A9}' => '\u{0643}', c => c }).collect();
    let mut keys = vec![exact];
    for k in [folded, curly, persian, arabic] {
        if !k.is_empty() && !keys.contains(&k) {
            keys.push(k);
        }
    }
    keys
}
fn invalid() -> Error {
    Error::invalid(
        "GeoNames release has invalid or unsupported records; prior accepted reference remains usable",
    )
}
fn numeric_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
}
fn rows(
    path: &Path,
    cancel: &AtomicBool,
    mut visit: impl FnMut(&str) -> Result<()>,
) -> Result<usize> {
    let mut input = BufReader::new(fs::File::open(path)?);
    let mut line = String::new();
    let mut count = 0;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        line.clear();
        let n = input.by_ref().take(MAX_LINE + 1).read_line(&mut line)?;
        if n == 0 {
            break;
        }
        count += 1;
        if n as u64 > MAX_LINE {
            return Err(invalid());
        }
        visit(line.trim_end_matches(['\r', '\n']))?;
    }
    Ok(count)
}

/// Builds a NEW candidate directory. No active pointer or existing edition is changed.
/// Inputs are exact extracted publisher members, retained alongside the index.
pub fn build(
    destination: &Path,
    places: &Path,
    alternate_names_v2: &Path,
    acquisition: Value,
    cancel: &AtomicBool,
) -> Result<Value> {
    if destination.exists() {
        return Err(Error::conflict("Reference edition already exists"));
    }
    let parent = destination.parent().ok_or_else(invalid)?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix("geonames-")
        .tempdir_in(parent)?;
    for (from, name) in [
        (places, "places.txt"),
        (alternate_names_v2, "alternateNamesV2.txt"),
    ] {
        fs::copy(from, staging.path().join(name))?;
    }
    let c = Connection::open(staging.path().join("index.sqlite3"))?;
    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA cache_size=-8192; PRAGMA temp_store=FILE;
        CREATE TABLE places(id TEXT PRIMARY KEY,name TEXT NOT NULL,class TEXT NOT NULL,code TEXT NOT NULL,country TEXT NOT NULL,lat REAL NOT NULL,lon REAL NOT NULL,raw TEXT NOT NULL,hash TEXT NOT NULL);
        CREATE TABLE names(id TEXT PRIMARY KEY,place TEXT NOT NULL REFERENCES places(id),key TEXT NOT NULL,name TEXT NOT NULL,language TEXT NOT NULL,preferred TEXT NOT NULL,short TEXT NOT NULL,colloquial TEXT NOT NULL,historic TEXT NOT NULL,valid_from TEXT NOT NULL,valid_to TEXT NOT NULL,raw TEXT NOT NULL,active INTEGER NOT NULL);
        CREATE INDEX names_lookup ON names(key,active,place);
        CREATE TABLE possible_names(key TEXT NOT NULL,place TEXT NOT NULL REFERENCES places(id),PRIMARY KEY(key,place)) WITHOUT ROWID;")?;
    let tx = c.unchecked_transaction()?;
    let mut countries = std::collections::BTreeSet::new();
    let mut unclassified_places = 0usize;
    let mut possible_name_count = 0usize;
    let global = acquisition["product"] == "cities500";
    let place_count = rows(&staging.path().join("places.txt"), cancel, |line| {
        let f: Vec<_> = line.split('\t').collect();
        if f.len() != 19
            || !numeric_id(f[0])
            || f[1].is_empty()
            || f[8].len() != 2
            || !f[8].bytes().all(|b| b.is_ascii_uppercase())
        {
            return Err(invalid());
        }
        let lat: f64 = f[4].parse().map_err(|_| invalid())?;
        let lon: f64 = f[5].parse().map_err(|_| invalid())?;
        if !lat.is_finite()
            || !lon.is_finite()
            || !(-90.0..=90.0).contains(&lat)
            || !(-180.0..=180.0).contains(&lon)
            || !["A", "H", "L", "P", "R", "S", "T", "U", "V"].contains(&f[6])
            || chrono::NaiveDate::parse_from_str(f[18], "%Y-%m-%d").is_err()
        {
            return Err(invalid());
        }
        // A missing publisher feature code does not invalidate its identity or
        // names. Keep the exact row as context; only explicit locality codes
        // can ever supply occurrence coordinates in lookup().
        if f[7].is_empty() {
            unclassified_places += 1;
        }
        countries.insert(f[8].to_owned());
        tx.execute(
            "INSERT INTO places VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                f[0],
                f[1],
                f[6],
                f[7],
                f[8],
                lat,
                lon,
                line,
                hash(line.as_bytes())
            ],
        )?;
        // Flattened global names have no language/history flags. They are
        // negative ambiguity evidence ONLY, never affirmative name matches.
        if global {
            for name in [f[1], f[2]].into_iter().chain(f[3].split(',')) {
                let name = key(name);
                if name.is_empty() {
                    continue;
                }
                possible_name_count += tx
                    .prepare_cached("INSERT OR IGNORE INTO possible_names VALUES(?1,?2)")?
                    .execute(params![name, f[0]])?;
            }
        }
        for (suffix, name) in [("name", f[1]), ("ascii", f[2])] {
            if !name.is_empty() {
                tx.execute(
                    "INSERT INTO names VALUES(?1,?2,?3,?4,'','','','','','','','',1)",
                    params![format!("{}:{suffix}", f[0]), f[0], key(name), name],
                )?;
            }
        }
        Ok(())
    })?;
    if place_count == 0 {
        return Err(invalid());
    }
    let mut indexed_aliases = 0;
    let mut unjoined_aliases = 0;
    let mut empty_aliases = 0;
    let aliases = rows(
        &staging.path().join("alternateNamesV2.txt"),
        cancel,
        |line| {
            let f: Vec<_> = line.split('\t').collect();
            if f.len() != 10
                || !numeric_id(f[0])
                || !numeric_id(f[1])
                || f[4..8].iter().any(|flag| !["", "0", "1"].contains(flag))
            {
                return Err(invalid());
            }
            // Publisher occasionally supplies an empty translated name. Retain
            // the exact row in the raw member, count it, and never index it.
            if f[3].is_empty() {
                empty_aliases += 1;
                return Ok(());
            }
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM places WHERE id=?1)",
                [f[1]],
                |r| r.get(0),
            )?;
            if !exists {
                unjoined_aliases += 1;
                return Ok(());
            }
            // Postal/link/airport IDs, old/colloquial and temporally bounded names
            // remain inspectable, but cannot silently resolve a current occurrence.
            let language = f[2];
            let linguistic = language.is_empty()
                || language.split('-').all(|s| {
                    (2..=3).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphabetic())
                });
            let active =
                linguistic && f[6] != "1" && f[7] != "1" && f[8].is_empty() && f[9].is_empty();
            tx.execute(
                "INSERT INTO names VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![
                    format!("alternate:{}", f[0]),
                    f[1],
                    key(f[3]),
                    f[3],
                    language,
                    f[4],
                    f[5],
                    f[6],
                    f[7],
                    f[8],
                    f[9],
                    line,
                    active
                ],
            )?;
            indexed_aliases += 1;
            Ok(())
        },
    )?;
    tx.commit()?;
    c.execute_batch("ANALYZE; PRAGMA optimize;")?;
    drop(c);
    if cancel.load(Ordering::Acquire) {
        return Err(Error::cancelled());
    }
    let mut assets = serde_json::Map::new();
    for name in ["places.txt", "alternateNamesV2.txt", "index.sqlite3"] {
        let (sha, size) =
            crate::media_store::file_hash_cancellable(&staging.path().join(name), cancel)?;
        fs::File::open(staging.path().join(name))?.sync_all()?;
        assets.insert(name.into(), json!({"sha256":sha,"bytes":size}));
    }
    let manifest = json!({"schema_version":1,"status":"candidate","publisher":"GeoNames","license":"CC-BY-4.0","attribution":"GeoNames (geonames.org), CC BY 4.0","format":"geoname-19/alternateNamesV2-10","normalization":"NFC, whitespace, lowercase; no fuzzy match","place_count":place_count,"unclassified_places":unclassified_places,"possible_name_count":possible_name_count,"possible_names_use":"negative global collision evidence only; no positive alias resolution","alternate_name_rows":aliases,"indexed_aliases":indexed_aliases,"unjoined_aliases":unjoined_aliases,"empty_aliases_excluded":empty_aliases,"countries":countries,"assets":assets,"acquisition":acquisition,"limits":{"line_bytes":MAX_LINE,"candidates":MAX_RESULTS}});
    fs::write(
        staging.path().join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::File::open(staging.path().join("manifest.json"))?.sync_all()?;
    fs::File::open(staging.path())?.sync_all()?;
    fs::rename(staging.path(), destination)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(manifest)
}

/// Re-index retained publisher bytes without network access or activation.
/// A complete new candidate is renamed into place only after all hashes match.
pub fn rebuild(source: &Path, destination: &Path, cancel: &AtomicBool) -> Result<Value> {
    if destination.exists() {
        return Err(Error::conflict("Reference edition already exists"));
    }
    if fs::metadata(source.join("manifest.json"))?.len() > 64 * 1024 {
        return Err(invalid());
    }
    let bytes = fs::read(source.join("manifest.json"))?;
    let old: Value = serde_json::from_slice(&bytes)?;
    let parent = destination.parent().ok_or_else(invalid)?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    fs::create_dir_all(parent)?;
    let staging = tempfile::tempdir_in(parent)?;
    let candidate = staging.path().join("candidate");
    let mut report = build(
        &candidate,
        &source.join("places.txt"),
        &source.join("alternateNamesV2.txt"),
        old["acquisition"].clone(),
        cancel,
    )?;
    for name in ["places.txt", "alternateNamesV2.txt"] {
        if report["assets"][name] != old["assets"][name] {
            return Err(invalid());
        }
    }
    let mut extras = vec!["places.zip", "readme.txt"];
    if old["acquisition"]["product"] != "cities500" {
        extras.push("aliases.zip");
    }
    for name in extras {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::cancelled());
        }
        fs::copy(source.join(name), candidate.join(name))?;
        let (sha256, bytes) =
            crate::media_store::file_hash_cancellable(&candidate.join(name), cancel)?;
        let asset = json!({"sha256":sha256,"bytes":bytes});
        if asset != old["assets"][name] {
            return Err(invalid());
        }
        report["assets"][name] = asset;
        fs::File::open(candidate.join(name))?.sync_all()?;
    }
    fs::write(
        candidate.join("manifest.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    fs::File::open(candidate.join("manifest.json"))?.sync_all()?;
    fs::File::open(&candidate)?.sync_all()?;
    if cancel.load(Ordering::Acquire) {
        return Err(Error::cancelled());
    }
    fs::rename(candidate, destination)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(report)
}

#[derive(Clone)]
pub struct Index {
    index_path: PathBuf,
    manifest: Value,
    manifest_hash: String,
}
impl Index {
    /// The GeoNames product: a country code, or "cities500" for the global list.
    pub(crate) fn product(&self) -> &str {
        self.manifest["acquisition"]["product"].as_str().unwrap_or("")
    }
    /// Explicit candidate inspection is separate from application activation.
    pub fn inspect(root: &Path) -> Result<Self> {
        if fs::metadata(root.join("manifest.json"))?.len() > 64 * 1024 {
            return Err(invalid());
        }
        let bytes = fs::read(root.join("manifest.json"))?;
        let manifest: Value = serde_json::from_slice(&bytes)?;
        if manifest["schema_version"] != 1 || manifest["format"] != "geoname-19/alternateNamesV2-10"
        {
            return Err(invalid());
        }
        for name in ["places.txt", "alternateNamesV2.txt", "index.sqlite3"] {
            let (h, n) = file_hash(&root.join(name))?;
            if manifest["assets"][name]["sha256"] != h || manifest["assets"][name]["bytes"] != n {
                return Err(invalid());
            }
        }
        Ok(Self {
            index_path: root.join("index.sqlite3"),
            manifest,
            manifest_hash: hash(&bytes),
        })
    }
    pub fn summary(&self) -> Value {
        self.manifest.clone()
    }
    pub(crate) fn stored(
        manifest: Value,
        manifest_hash: String,
        index_path: PathBuf,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        let (h, bytes) = crate::media_store::file_hash_cancellable(&index_path, cancel)?;
        if manifest["assets"]["index.sqlite3"]["sha256"] != h
            || manifest["assets"]["index.sqlite3"]["bytes"] != bytes
        {
            return Err(invalid());
        }
        Ok(Self {
            index_path,
            manifest,
            manifest_hash,
        })
    }
    pub(crate) fn publisher_alias_keys(
        &self,
        id: &str,
        country: &str,
    ) -> Result<std::collections::BTreeSet<String>> {
        let Some(id) = id.strip_prefix("geonames:") else {
            return Ok(Default::default());
        };
        let c = Connection::open_with_flags(&self.index_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut q = c.prepare("SELECT n.key FROM names n JOIN places p ON p.id=n.place WHERE n.place=?1 AND n.active=1 AND p.country=?2")?;
        Ok(q.query_map(params![id, country], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?)
    }
    fn contains_locality(&self, id: &str, country: Option<&str>, name: &str) -> Result<bool> {
        // Old editions remain readable for explicit-country lookup, but cannot
        // prove uniqueness without the negative collision index.
        if !self.has_possible_names() {
            return Ok(false);
        }
        let Some(id) = id.strip_prefix("geonames:") else {
            return Ok(false);
        };
        let c = Connection::open_with_flags(&self.index_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let name_match = "(EXISTS(SELECT 1 FROM possible_names n WHERE n.place=places.id AND n.key IN (SELECT value FROM json_each(?3))) OR EXISTS(SELECT 1 FROM names n WHERE n.place=places.id AND n.key IN (SELECT value FROM json_each(?3)) AND n.active=1))";
        Ok(c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM places WHERE id=?1 AND country=?2 AND class='P' AND code IN ('PPL','PPLA','PPLA2','PPLA3','PPLA4','PPLA5','PPLC','PPLL','PPLX') AND {name_match})"),params![id,country,serde_json::to_string(&lookup_keys(name))?],|r| r.get(0))?)
    }
    fn has_possible_names(&self) -> bool {
        self.manifest["acquisition"]["product"] == "cities500"
            && self.manifest["possible_name_count"].as_u64().is_some()
    }
    fn possible_collisions(
        &self,
        name: &str,
        countries: &[String],
        selected_id: &str,
        feature: Option<&str>,
    ) -> Result<(Vec<LocationCandidate>, usize)> {
        if !self.has_possible_names() {
            return Ok((vec![], 0));
        }
        self.candidates_matching(
            name,
            None,
            countries,
            feature,
            Some(selected_id.strip_prefix("geonames:").ok_or_else(invalid)?),
        )
    }
    fn candidates(
        &self,
        name: &str,
        country: Option<&str>,
    ) -> Result<(Vec<LocationCandidate>, usize)> {
        self.candidates_filtered(name, country, &[], None)
    }
    fn candidates_filtered(
        &self,
        name: &str,
        country: Option<&str>,
        excluded_countries: &[String],
        class: Option<&str>,
    ) -> Result<(Vec<LocationCandidate>, usize)> {
        self.candidates_matching(name, country, excluded_countries, class, None)
    }
    fn candidates_matching(
        &self,
        name: &str,
        country: Option<&str>,
        excluded_countries: &[String],
        class: Option<&str>,
        collision_exclusion: Option<&str>,
    ) -> Result<(Vec<LocationCandidate>, usize)> {
        let c = Connection::open_with_flags(&self.index_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        c.pragma_update(None, "cache_size", -2048)?;
        let names = if collision_exclusion.is_some() {
            "SELECT place FROM possible_names WHERE key IN (SELECT value FROM json_each(?1))"
        } else {
            "SELECT place FROM names WHERE key IN (SELECT value FROM json_each(?1)) AND active=1"
        };
        let filter = format!(
            "FROM places p WHERE p.id IN ({names}) AND (?2 IS NULL OR p.country=?2) AND p.country NOT IN (SELECT value FROM json_each(?3)) AND (?4 IS NULL OR p.class=?4 OR p.code=?4) AND (?5 IS NULL OR p.id!=?5)"
        );
        let count: usize = c.query_row(
            &format!("SELECT count(*) {filter}"),
            params![
                serde_json::to_string(&lookup_keys(name))?,
                country,
                serde_json::to_string(excluded_countries)?,
                class,
                collision_exclusion
            ],
            |r| r.get::<_, i64>(0),
        )? as usize;
        let mut stmt = c.prepare(&format!(
            "SELECT id,name,class,code,country,lat,lon,hash,raw {filter} ORDER BY id LIMIT {MAX_RESULTS}"
        ))?;
        let mut candidates = vec![];
        for row in stmt.query_map(
            params![
                serde_json::to_string(&lookup_keys(name))?,
                country,
                serde_json::to_string(excluded_countries)?,
                class,
                collision_exclusion
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, f64>(5)?,
                    r.get::<_, f64>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                ))
            },
        )? {
            let (id, label, class, code, country, lat, lon, row_hash, raw) = row?;
            let city = class == "P"
                && [
                    "PPL", "PPLA", "PPLA2", "PPLA3", "PPLA4", "PPLA5", "PPLC", "PPLL", "PPLX",
                ]
                .contains(&code.as_str());
            let geometry = (city && collision_exclusion.is_none())
                .then(|| json!({"type":"Point","coordinates":[lon,lat]}));
            let aliases:Vec<Value>=c.prepare("SELECT id,language,raw FROM names WHERE place=?1 AND key IN (SELECT value FROM json_each(?2)) AND active=1 ORDER BY id LIMIT 8")?.query_map(params![id,serde_json::to_string(&lookup_keys(name)).map_err(|_| invalid())?],|r|Ok(json!({"id":r.get::<_,String>(0)?,"language":r.get::<_,String>(1)?,"record":r.get::<_,String>(2)?})))?.collect::<std::result::Result<_,_>>()?;
            candidates.push(LocationCandidate{id:format!("geonames:{id}"),name:label,kind:if city{"city"}else{"area_or_feature"}.into(),country_code:Some(country.clone()),country_name:None,geometry,precision:if collision_exclusion.is_some(){"unverified_alias_context"}else if city{"city_level_approximate"}else{"feature_context_only"}.into(),reference:ReferenceIdentity{dataset_id:format!("geonames:{}",required(&self.manifest["assets"]["places.txt"],"sha256")?),publisher:"GeoNames".into(),edition:self.manifest_hash.clone(),product_version:"geoname-19/alternateNamesV2-10".into(),source_path:"places.txt".into(),source_sha256:required(&self.manifest["assets"]["places.txt"],"sha256")?.into(),publisher_record_id:id,record_sha256:row_hash,acceptance_id:format!("geonames-candidate:{}",self.manifest_hash),acceptance_sha256:self.manifest_hash.clone(),represented_time:format!("Publisher record modified {}; not an observation date",raw.split('\t').nth(18).unwrap_or("unknown"))},match_basis:if collision_exclusion.is_some(){"unverified_global_alias_collision"}else{"exact_geonames_name"}.into(),matched_alias:Some(name.into()),alias_reference:Some(json!({"aliases":aliases,"possible_alias_only":collision_exclusion.is_some(),"source_sha256":self.manifest["assets"][if collision_exclusion.is_some(){"places.txt"}else{"alternateNamesV2.txt"}]["sha256"],"feature_class":class,"feature_code":code,"admin_codes_namespace":"GeoNames publisher codes; FIPS/ISO exceptions not crosswalked","publisher_record":raw})),country_constraint_match:None});
        }
        Ok((candidates, count))
    }
    pub fn resolve(
        &self,
        text: &str,
        p: &LocationProposal,
        occurrence: bool,
    ) -> Result<Option<LocationResolution>> {
        if text.len() > 64 * 1024
            || p.mention.len() > 240
            || p.quote.len() > 4000
            || p.mention.is_empty()
            || !p.quote.contains(&p.mention)
            || text.match_indices(&p.quote).count() != 1
        {
            return Err(invalid());
        }
        let (all, all_count) = self.candidates(&p.mention, None)?;
        if all_count == 0 {
            return Ok(None);
        }
        let (candidates, count) = if let Some(country) = p.country_code.as_deref() {
            self.candidates(&p.mention, Some(country))?
        } else {
            (all.clone(), all_count)
        };
        let mut r = LocationResolution {
            literal_qualifier: None,
            mention: p.mention.clone(),
            quote: p.quote.clone(),
            role: p.role.clone(),
            status: "ambiguous".into(),
            candidates,
            candidate_count: count,
            candidates_truncated: count > MAX_RESULTS,
            selected_candidate_id: None,
            geometry: None,
            precision: "unresolved".into(),
            area: None,
            occurrence_eligible: false,
            reason: "Multiple publisher identities match; no automatic choice.".into(),
        };
        if count == 0 {
            r.status = "country_conflict".into();
            r.candidates = all;
            r.candidate_count = all_count;
            r.candidates_truncated = all_count > MAX_RESULTS;
            r.reason = "No exact place matches the supplied country.".into();
            return Ok(Some(r));
        }
        for candidate in &mut r.candidates {
            candidate.country_constraint_match = p
                .country_code
                .as_ref()
                .map(|c| candidate.country_code.as_ref() == Some(c));
        }
        if count != 1 {
            return Ok(Some(r));
        }
        let candidate = &r.candidates[0];
        if let Some(canonical) = p.canonical_name.as_deref() {
            let (matches, n) = self.candidates(canonical, p.country_code.as_deref())?;
            if n > 0 && !matches.iter().any(|c| c.id == candidate.id) {
                r.status = "canonical_conflict".into();
                return Ok(Some(r));
            }
        }
        r.selected_candidate_id = Some(candidate.id.clone());
        r.precision = candidate.precision.clone();
        if candidate.geometry.is_none() {
            r.status = "broad".into();
            r.reason =
                "Area or feature identity retained; its label point is not an occurrence site."
                    .into();
        } else if p.country_code.is_none() {
            r.status = "country_context_required".into();
            r.reason = "This regional reference cannot exclude same-named places outside its coverage; retain country context before assigning a point.".into();
        } else if p.role != LocationRole::Event || !occurrence {
            r.status = "context_only".into();
            r.reason = "Place retained as context; no reported occurrence asserted.".into();
        } else {
            r.status = "resolved".into();
            r.geometry = candidate.geometry.clone();
            r.occurrence_eligible = true;
            r.reason="Exact GeoNames locality; approximate settlement point, not an observed incident coordinate.".into();
        }
        Ok(Some(r))
    }
}

/// A Latin transliteration folded for spelling comparison: marks removed,
/// articles dropped, words joined, common French/English digraphs unified,
/// doubled letters collapsed. None for any other script.
pub(crate) fn latin_key(name: &str) -> Option<String> {
    use unicode_normalization::char::is_combining_mark;
    const ARTICLES: [&str; 22] = ["al", "el", "ul", "il", "ash", "esh", "ech", "as", "es", "ad", "ed", "ar", "er", "az", "ez", "at", "et", "an", "en", "ath", "adh", "the"];
    let decomposed: String = name.nfkd().filter(|c| !is_combining_mark(*c)).collect::<String>().to_lowercase();
    let mut spaced = String::with_capacity(decomposed.len());
    for c in decomposed.chars() {
        if c.is_ascii_lowercase() {
            spaced.push(c);
        } else if c.is_alphabetic() || c.is_numeric() {
            return None;
        } else {
            spaced.push(' ');
        }
    }
    let joined: String = spaced.split_whitespace().filter(|w| !ARTICLES.contains(w)).collect();
    let unified = joined.replace("dj", "j").replace("ch", "sh").replace("ou", "u").replace("oo", "u").replace("ee", "i").replace(['q', 'c'], "k");
    let mut s = String::with_capacity(unified.len());
    for c in unified.chars() {
        if !s.ends_with(c) {
            s.push(c);
        }
    }
    (!s.is_empty()).then_some(s)
}
/// Edit distance in half-steps: a vowel (or h, or w, which transliterates
/// the same letter as "ou") inserted, dropped or swapped for another costs one
/// half-step; anything else, including an adjacent transposition, costs two.
pub(crate) fn spelling_distance(a: &str, b: &str) -> usize {
    let soft = |c: u8| b"aeiouyhw".contains(&c);
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 1..=a.len() {
        d[i][0] = d[i - 1][0] + if soft(a[i - 1]) { 1 } else { 2 };
    }
    for j in 1..=b.len() {
        d[0][j] = d[0][j - 1] + if soft(b[j - 1]) { 1 } else { 2 };
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let substitution = if a[i - 1] == b[j - 1] {
                0
            } else if soft(a[i - 1]) && soft(b[j - 1]) {
                1
            } else {
                2
            };
            let mut best = (d[i - 1][j] + if soft(a[i - 1]) { 1 } else { 2 })
                .min(d[i][j - 1] + if soft(b[j - 1]) { 1 } else { 2 })
                .min(d[i - 1][j - 1] + substitution);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(d[i - 2][j - 2] + 2);
            }
            d[i][j] = best;
        }
    }
    d[a.len()][b.len()]
}
/// Folded Latin names of one index's settlements in one country, read on
/// first use and kept for the resolver's lifetime.
#[derive(Default)]
pub struct SpellingCache(std::sync::Mutex<std::collections::HashMap<(PathBuf, String), FoldedNames>>);
/// Folded spelling and GeoNames id of each settlement name.
type FoldedNames = std::sync::Arc<Vec<(Box<str>, u32)>>;
impl SpellingCache {
    fn rows(&self, index: &Index, country: &str) -> Result<FoldedNames> {
        let key = (index.index_path.clone(), country.to_owned());
        if let Some(rows) = self.0.lock().map_err(|_| Error::internal())?.get(&key) {
            return Ok(rows.clone());
        }
        let c = Connection::open_with_flags(&index.index_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut q = c.prepare("SELECT n.name,p.id FROM names n JOIN places p ON p.id=n.place WHERE p.country=?1 AND p.class='P' AND p.code IN ('PPL','PPLA','PPLA2','PPLA3','PPLA4','PPLA5','PPLC','PPLL') AND n.active=1")?;
        let mut seen = std::collections::HashSet::new();
        let mut rows = vec![];
        for row in q.query_map([country], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (name, id) = row?;
            let (Some(folded), Ok(id)) = (latin_key(&name), id.parse::<u32>()) else { continue };
            if seen.insert((folded.clone(), id)) {
                rows.push((folded.into_boxed_str(), id));
            }
        }
        let rows = std::sync::Arc::new(rows);
        self.0.lock().map_err(|_| Error::internal())?.insert(key, rows.clone());
        Ok(rows)
    }
}
/// "Barachit" for the gazetteer's "Braachit": when no exact name matches, the
/// one settlement in the proposal's country whose Latin spelling is close. The
/// closest must be clearly closer than any other; ties and short names give
/// nothing. A gazetteer inference at settlement precision, never an observation.
pub(crate) fn spelling_variant(
    indexes: &[Index],
    cache: &SpellingCache,
    text: &str,
    p: &LocationProposal,
    occurrence: bool,
) -> Result<Option<LocationResolution>> {
    let Some(country) = p.country_code.as_deref() else {
        return Ok(None);
    };
    if !p.quote.contains(&p.mention) || text.match_indices(&p.quote).count() != 1 {
        return Ok(None);
    }
    let Some(target) = latin_key(&p.mention) else {
        return Ok(None);
    };
    if target.len() < 4 {
        return Ok(None);
    }
    // Half-steps: one vowel slip in a short name ("Hulta" is not "Hulbta"),
    // two in a medium one, three in a long one.
    let allowed = match target.len() {
        0..=6 => 1,
        7..=10 => 2,
        _ => 3,
    };
    let mut best = std::collections::BTreeMap::<u32, (usize, usize, Box<str>)>::new();
    for (position, index) in indexes.iter().enumerate() {
        let product = index.manifest["acquisition"]["product"].as_str().unwrap_or("");
        if product != country && product != "cities500" {
            continue;
        }
        for (key, id) in cache.rows(index, country)?.iter() {
            if key.as_bytes()[0] != target.as_bytes()[0] || key.len().abs_diff(target.len()) > 4 {
                continue;
            }
            let distance = spelling_distance(&target, key);
            if distance <= allowed + 2 && best.get(id).is_none_or(|b| distance < b.0) {
                best.insert(*id, (distance, position, key.clone()));
            }
        }
    }
    let mut ranked = best.into_iter().collect::<Vec<_>>();
    ranked.sort_by_key(|(_, (distance, _, _))| *distance);
    let Some((id, (distance, position, key))) = ranked.first().cloned() else {
        return Ok(None);
    };
    // "Barachit" and "Baraachît" fold to the same spelling. A rival within
    // one full edit of the closest makes the choice a guess.
    if distance > allowed || ranked.get(1).is_some_and(|(_, (next, _, _))| *next < distance + 2) {
        return Ok(None);
    }
    let c = Connection::open_with_flags(&indexes[position].index_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let names = c
        .prepare("SELECT name FROM names WHERE place=?1 AND active=1 ORDER BY id")?
        .query_map([id.to_string()], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let Some(name) = names.into_iter().find(|n| latin_key(n).as_deref() == Some(&*key)) else {
        return Ok(None);
    };
    let (found, _) = indexes[position].candidates_matching(&name, Some(country), &[], None, None)?;
    let Some(mut candidate) = found.into_iter().find(|c| c.id == format!("geonames:{id}")) else {
        return Ok(None);
    };
    if candidate.geometry.is_none() {
        return Ok(None);
    }
    candidate.match_basis = "spelling_variant".into();
    candidate.matched_alias = Some(name.clone());
    candidate.country_constraint_match = Some(true);
    let asserted = p.role == LocationRole::Event && occurrence;
    Ok(Some(LocationResolution {
        literal_qualifier: None,
        mention: p.mention.clone(),
        quote: p.quote.clone(),
        role: p.role.clone(),
        status: if asserted { "resolved" } else { "context_only" }.into(),
        selected_candidate_id: Some(candidate.id.clone()),
        geometry: if asserted { candidate.geometry.clone() } else { None },
        precision: "city_level_approximate".into(),
        area: None,
        occurrence_eligible: asserted,
        reason: format!("No exact name matches. \"{name}\" is the only close spelling of a settlement in {country} (weighted difference {}); approximate settlement point, not an observed incident coordinate.", distance as f64 / 2.0),
        candidates: vec![candidate],
        candidate_count: 1,
        candidates_truncated: false,
    }))
}
/// Resolve against the complete activated set, deduplicating publisher identity.
/// Global city coverage allows unique locality matches without a model-supplied
/// country. This remains a gazetteer inference, never an observed coordinate.
/// Global same-name alternatives of a selected place: exact and alias-only
/// (possible-name) matches outside the regional editions, excluding the place.
pub(crate) fn global_alternatives(
    indexes: &[Index],
    name: &str,
    selected_id: &str,
) -> Result<Vec<LocationCandidate>> {
    let regional = indexes
        .iter()
        .filter_map(|i| i.manifest["acquisition"]["product"].as_str())
        .filter(|p| p.len() == 2)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let Some(global) = indexes
        .iter()
        .find(|i| i.manifest["acquisition"]["product"] == "cities500")
    else {
        return Ok(vec![]);
    };
    Ok(global.possible_collisions(name, &regional, selected_id, None)?.0)
}
pub(crate) fn resolve_indexes(
    indexes: &[Index],
    text: &str,
    p: &LocationProposal,
    occurrence: bool,
) -> Result<Option<LocationResolution>> {
    if text.len() > 64 * 1024
        || p.mention.is_empty()
        || !p.quote.contains(&p.mention)
        || text.match_indices(&p.quote).count() != 1
    {
        return Err(invalid());
    }
    let literal_name = p.mention.strip_prefix('#').unwrap_or(&p.mention);
    let mut name = literal_name;
    let mut class = None;
    // Only immediate, literal feature qualifiers. Never use a model's canonical
    // name to replace an unmatched original or infer a site from a whole post.
    for (prefix, suffix, kind) in [
        ("city of ", " city", "P"),
        ("province of ", " province", "A"),
        ("governorate of ", " governorate", "A"),
        ("district of ", " district", "A"),
        ("مدينة ", "", "P"),
        ("محافظة ", "", "A"),
        ("شهر ", "", "P"),
        ("استان ", "", "A"),
    ] {
        let normalized = key(name);
        if normalized.starts_with(prefix) {
            name = name.get(prefix.len()..).ok_or_else(invalid)?.trim();
            class = Some(kind);
            break;
        }
        if !suffix.is_empty() && normalized.ends_with(suffix) {
            name = name
                .get(..name.len() - suffix.len())
                .ok_or_else(invalid)?
                .trim();
            class = Some(kind);
            break;
        }
    }
    let mut context_class = false;
    if class.is_none()
        && let Some((before, after)) = p.quote.split_once(&p.mention)
        && (key(before).ends_with("city of")
            || key(after).starts_with("city ")
            || key(before).ends_with("مدينة")
            || key(before).ends_with("شهر"))
    {
        class = Some("P");
        context_class = true;
    }
    let capital = p.quote.split_once(&p.mention).is_some_and(|(before, _)| {
        let before = key(before.trim_end_matches(|c: char| c.is_whitespace() || c == ','));
        let Some(prefix) = before
            .strip_suffix(" capital")
            .or_else(|| before.strip_suffix(" capital of"))
        else {
            return false;
        };
        // A local modifier qualifies "capital", not an unrelated negation in
        // the rest of the sentence. A preceding list item is not apposition.
        let modifiers = prefix.split_whitespace().rev().take(3).collect::<Vec<_>>();
        matches!(modifiers.first().copied(), Some("the" | "national"))
            && (p.country_code.is_some() || modifiers.first() == Some(&"national"))
            && !modifiers.iter().any(|word| {
                word.ends_with(',')
                    || [
                        "not",
                        "former",
                        "previous",
                        "old",
                        "once",
                        "longer",
                        "provincial",
                        "regional",
                        "state",
                        "district",
                        "cultural",
                        "financial",
                    ]
                    .contains(word)
            })
    });
    let regional: std::collections::BTreeSet<String> = indexes
        .iter()
        .filter_map(|i| i.manifest["acquisition"]["product"].as_str())
        .filter(|p| p.len() == 2)
        .map(str::to_owned)
        .collect();
    let global = indexes
        .iter()
        .find(|i| i.manifest["acquisition"]["product"] == "cities500");
    let lookup =
        |lookup_name: &str, country: Option<&str>| -> Result<(Vec<LocationCandidate>, usize)> {
            let mut found = std::collections::BTreeMap::new();
            let mut omitted = 0;
            for index in indexes {
                let excluded = if index.manifest["acquisition"]["product"] == "cities500" {
                    regional.iter().cloned().collect::<Vec<_>>()
                } else {
                    vec![]
                };
                // Apply coverage and feature filters before counting and LIMIT:
                // hidden global rows must not crowd out eligible candidates.
                let (rows, count) =
                    index.candidates_filtered(lookup_name, country, &excluded, class)?;
                omitted += count.saturating_sub(rows.len());
                for mut c in rows {
                    c.reference.acceptance_id = format!("geonames-active:{}", index.manifest_hash);
                    c.match_basis =
                        if class.is_some() && (lookup_name != literal_name || context_class) {
                            "exact_name_with_literal_feature_qualifier"
                        } else {
                            "exact_geonames_name"
                        }
                        .into();
                    found.insert(c.id.clone(), c);
                }
            }
            let count = found.len() + omitted;
            Ok((found.into_values().take(MAX_RESULTS).collect(), count))
        };
    // "City" can be part of the publisher's actual name (Gaza City, New York
    // City). Do not erase that stronger literal evidence and introduce homonyms
    // of the shorter name. Fall back to the feature-qualified short name only
    // when no current publisher name matches the full literal phrase.
    let full = if class == Some("P") && name != literal_name {
        Some(lookup(literal_name, None)?)
    } else {
        None
    };
    let (all, total) = if let Some(full) = full.filter(|(_, n)| *n > 0) {
        name = literal_name;
        full
    } else {
        lookup(name, None)?
    };
    if total == 0 {
        return Ok(None);
    }
    let (mut candidates, mut count) = if p.country_code.is_some() {
        lookup(name, p.country_code.as_deref())?
    } else {
        (all.clone(), total)
    };
    // A canonical spelling can narrow identities only when BOTH names are
    // retained publisher aliases of those identities. It cannot create a match.
    let mut capital_applied = false;
    if count <= MAX_RESULTS {
        if let Some(canonical) = p.canonical_name.as_deref().filter(|c| key(c) != key(name)) {
            let (matches, n) = lookup(canonical, p.country_code.as_deref())?;
            if n > 0 && n <= MAX_RESULTS {
                let overlap = candidates
                    .iter()
                    .filter(|c| matches.iter().any(|m| m.id == c.id))
                    .cloned()
                    .collect::<Vec<_>>();
                if !overlap.is_empty() {
                    candidates = overlap;
                    count = candidates.len();
                    for c in &mut candidates {
                        c.match_basis = "literal_and_canonical_publisher_alias_intersection".into();
                    }
                }
            }
        }
        if capital {
            let capitals = candidates
                .iter()
                .filter(|c| {
                    c.alias_reference
                        .as_ref()
                        .is_some_and(|a| a["feature_code"] == "PPLC")
                })
                .cloned()
                .collect::<Vec<_>>();
            if !capitals.is_empty() {
                capital_applied = true;
                candidates = capitals;
                count = candidates.len();
                for c in &mut candidates {
                    c.match_basis = "literal_capital_qualifier".into();
                }
            }
        }
    }
    for c in &mut candidates {
        c.country_constraint_match = p
            .country_code
            .as_ref()
            .map(|v| c.country_code.as_ref() == Some(v));
    }
    let mut r = LocationResolution {
        literal_qualifier: None,
        mention: p.mention.clone(),
        quote: p.quote.clone(),
        role: p.role.clone(),
        status: "ambiguous".into(),
        candidates,
        candidate_count: count,
        candidates_truncated: count > MAX_RESULTS,
        selected_candidate_id: None,
        geometry: None,
        precision: "unresolved".into(),
        area: None,
        occurrence_eligible: false,
        reason: "Multiple publisher identities match; no automatic choice.".into(),
    };
    if count == 0 {
        r.status = "country_conflict".into();
        r.candidates = all;
        r.candidate_count = total;
        r.candidates_truncated = total > MAX_RESULTS;
        r.reason = "No exact place matches the supplied country.".into();
        return Ok(Some(r));
    }
    if count != 1 {
        return Ok(Some(r));
    }
    if p.country_code.is_none()
        && r.candidates[0].geometry.is_some()
        && let Some(global) = global
    {
        let (mut alternatives, count) = global.possible_collisions(
            name,
            &regional.iter().cloned().collect::<Vec<_>>(),
            &r.candidates[0].id,
            if capital_applied { Some("PPLC") } else { class },
        )?;
        for alternative in &mut alternatives {
            alternative.reference.acceptance_id =
                format!("geonames-active:{}", global.manifest_hash);
        }
        if count > 0 {
            r.candidate_count += count;
            r.candidates.extend(alternatives);
            r.candidates.truncate(MAX_RESULTS);
            r.candidates_truncated = r.candidate_count > MAX_RESULTS;
            r.reason = "The global reference contains possible same-name alternatives. Unclassified global aliases block a point but cannot establish one.".into();
            return Ok(Some(r));
        }
    }
    let candidate = &r.candidates[0];
    if let Some(canonical) = p.canonical_name.as_deref() {
        let (matches, n) = lookup(canonical, p.country_code.as_deref())?;
        if n > 0 && !matches.iter().any(|c| c.id == candidate.id) {
            r.status = "canonical_conflict".into();
            return Ok(Some(r));
        }
    }
    r.selected_candidate_id = Some(candidate.id.clone());
    r.precision = candidate.precision.clone();
    if candidate.geometry.is_none() {
        r.status = "broad".into();
        r.reason =
            "Area or feature identity retained; its label point is not an occurrence site.".into();
    } else if p.country_code.is_none()
        && candidate
            .alias_reference
            .as_ref()
            .is_some_and(|a| a["feature_code"] == "PPLX")
    {
        // A neighbourhood name alone is too weak: "Middle East" is a Baltimore
        // section, and a Tehran section once stood in for Iranshahr city.
        r.status = "country_context_required".into();
        r.reason = "A neighbourhood or city-section name needs country or city context before it can be placed.".into();
    } else if p.country_code.is_none()
        && !global
            .map(|index| {
                index.contains_locality(&candidate.id, candidate.country_code.as_deref(), name)
            })
            .transpose()?
            .unwrap_or(false)
    {
        r.status = "country_context_required".into();
        r.reason = "A locality or matched name absent from the global city baseline requires country context; regional uniqueness alone is insufficient.".into();
    } else if p.role != LocationRole::Event || !occurrence {
        r.status = "context_only".into();
        r.reason = "Place retained as context; no reported occurrence asserted.".into();
    } else {
        r.status = "resolved".into();
        r.geometry = candidate.geometry.clone();
        r.occurrence_eligible = true;
        r.reason="Unique literal locality in activated reference coverage; approximate settlement point, not an observed incident coordinate.".into();
    }
    Ok(Some(r))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seed(temp: &Path) -> PathBuf {
        let places = temp.join("input-places.txt");
        let aliases = temp.join("input-aliases.txt");
        fs::write(&places,concat!(
            "900001\tFictional Port\tFictional Port\tDO NOT USE THIS FLATTENED ALIAS\t14\t43\tP\tPPL\tYE\t\t01\t02\t\t\t1000\t\t10\tAsia/Aden\t2026-09-12\n",
            "900002\tFictional Province\tFictional Province\t\t15\t44\tA\tADM1\tYE\t\t01\t\t\t\t0\t\t10\tAsia/Aden\t2026-09-12\n",
            "900003\tFictional Port\tFictional Port\t\t12\t42\tP\tPPL\tSA\t\t01\t02\t\t\t1000\t\t10\tAsia/Riyadh\t2026-09-12\n")).unwrap();
        fs::write(
            &aliases,
            concat!(
                "1\t900001\tar\tميناء خيالي\t1\t\t\t\t\t\n",
                "2\t900001\ten\tOld Fictional Port\t\t\t\t1\t1900\t1950\n",
                "3\t900001\tlink\thttps://fictional.invalid\t\t\t\t\t\t\n",
                "4\t900099\ten\tAbsent foreign place\t\t\t\t\t\t\n",
                "5\t900001\tsi\t\t\t\t\t\t\t\n"
            ),
        )
        .unwrap();
        let dest = temp.join("edition");
        let m = build(
            &dest,
            &places,
            &aliases,
            json!({"fictional":true}),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(m["unjoined_aliases"], 1);
        assert_eq!(m["empty_aliases_excluded"], 1);
        assert_eq!(m["indexed_aliases"], 3);
        assert_eq!(m["place_count"], 3);
        dest
    }
    fn proposal(name: &str, country: Option<&str>) -> LocationProposal {
        LocationProposal {
            mention: name.into(),
            quote: format!("Fictional fire in {name}."),
            role: LocationRole::Event,
            canonical_name: None,
            country_code: country.map(str::to_owned),
        }
    }
    #[test]
    fn missing_feature_code_is_retained_as_context_without_coordinates() {
        let temp = tempfile::tempdir().unwrap();
        let places = temp.path().join("places.txt");
        let aliases = temp.path().join("aliases.txt");
        let original = "900010\tFictional Unknown\tFictional Unknown\t\t14\t43\tP\t\tYE\t\t01\t\t\t\t0\t\t10\tAsia/Aden\t2026-09-12\n";
        fs::write(&places, original).unwrap();
        fs::write(&aliases, []).unwrap();
        let dest = temp.path().join("edition");
        let manifest = build(
            &dest,
            &places,
            &aliases,
            json!({"fictional":true}),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(manifest["place_count"], 1);
        assert_eq!(manifest["unclassified_places"], 1);
        assert_eq!(
            fs::read_to_string(dest.join("places.txt")).unwrap(),
            original
        );
        let p = proposal("Fictional Unknown", Some("YE"));
        let r = Index::inspect(&dest)
            .unwrap()
            .resolve(&p.quote, &p, true)
            .unwrap()
            .unwrap();
        assert_eq!(r.status, "broad");
        assert!(!r.occurrence_eligible);
        assert!(r.geometry.is_none());
        assert!(r.candidates[0].geometry.is_none());
        assert_eq!(
            r.candidates[0].alias_reference.as_ref().unwrap()["feature_code"],
            ""
        );
        fs::write(&places, original.replace("\t14\t43\t", "\t94\t43\t")).unwrap();
        assert!(
            build(
                &temp.path().join("invalid"),
                &places,
                &aliases,
                json!({}),
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
    #[test]
    fn excluded_coverage_and_classes_do_not_consume_candidate_limit() {
        let temp = tempfile::tempdir().unwrap();
        let places = temp.path().join("places.txt");
        let aliases = temp.path().join("aliases.txt");
        let mut rows = String::new();
        for n in 0..25 {
            rows.push_str(&format!("{}\tSame Name\tSame Name\t\t14\t43\tP\tPPL\tYE\t\t01\t\t\t\t1000\t\t10\tAsia/Aden\t2026-09-12\n", 900000+n));
        }
        rows.push_str("999999\tSame Name\tSame Name\t\t14\t43\tA\tADM1\tSA\t\t01\t\t\t\t0\t\t10\tAsia/Riyadh\t2026-09-12\n");
        fs::write(&places, rows).unwrap();
        fs::write(&aliases, []).unwrap();
        let dest = temp.path().join("edition");
        build(&dest, &places, &aliases, json!({}), &AtomicBool::new(false)).unwrap();
        let index = Index::inspect(&dest).unwrap();
        assert_eq!(index.candidates("Same Name", None).unwrap().1, 26);
        for (excluded, class) in [(vec!["YE".to_owned()], None), (vec![], Some("A"))] {
            let (rows, count) = index
                .candidates_filtered("Same Name", None, &excluded, class)
                .unwrap();
            assert_eq!(count, 1);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].id, "geonames:999999");
        }
    }
    #[test]
    fn full_literal_city_name_is_not_reduced_to_its_ambiguous_short_name() {
        let temp = tempfile::tempdir().unwrap();
        let places = temp.path().join("places.txt");
        let aliases = temp.path().join("aliases.txt");
        fs::write(&places, concat!(
            "900021\tFictional City\tFictional City\t\t14\t43\tP\tPPLC\tYE\t\t01\t\t\t\t9000\t\t10\tAsia/Aden\t2026-09-12\n",
            "900022\tFictional\tFictional\t\t24\t53\tP\tPPL\tSA\t\t01\t\t\t\t9000\t\t10\tAsia/Riyadh\t2026-09-12\n",
            "900023\tAnother City\tAnother City\tFictional\t25\t54\tP\tPPL\tSA\t\t01\t\t\t\t9000\t\t10\tAsia/Riyadh\t2026-09-12\n"
        )).unwrap();
        fs::write(&aliases, "1\t900021\ten\tFictional\t1\t\t\t\t\t\n").unwrap();
        let dest = temp.path().join("global");
        build(
            &dest,
            &places,
            &aliases,
            json!({"product":"cities500"}),
            &AtomicBool::new(false),
        )
        .unwrap();
        let indexes = [Index::inspect(&dest).unwrap()];
        let p = proposal("Fictional City", None);
        let r = resolve_indexes(&indexes, &p.quote, &p, true)
            .unwrap()
            .unwrap();
        assert_eq!(r.status, "resolved");
        assert_eq!(r.selected_candidate_id.as_deref(), Some("geonames:900021"));
        assert_eq!(
            r.candidates[0].matched_alias.as_deref(),
            Some("Fictional City")
        );
        let short = proposal("Fictional", None);
        assert_eq!(
            resolve_indexes(&indexes, &short.quote, &short, true)
                .unwrap()
                .unwrap()
                .status,
            "ambiguous"
        );
        // An explicit contradictory country must not fall back to the shorter
        // locality, and context roles never become occurrence coordinates.
        let other_country = proposal("Fictional City", Some("SA"));
        assert_eq!(
            resolve_indexes(&indexes, &other_country.quote, &other_country, true)
                .unwrap()
                .unwrap()
                .status,
            "country_conflict"
        );
        let context = LocationProposal {
            role: LocationRole::Issuer,
            ..p
        };
        assert!(
            resolve_indexes(&indexes, &context.quote, &context, true)
                .unwrap()
                .unwrap()
                .geometry
                .is_none()
        );
        for (prefix, status) in [
            ("the capital, ", "ambiguous"),
            ("the former capital, ", "ambiguous"),
            ("not the capital, ", "ambiguous"),
            ("the regional capital, ", "ambiguous"),
            ("the governorate capital, ", "ambiguous"),
            ("the region's capital, ", "ambiguous"),
            ("Lebanon's northern capital, ", "ambiguous"),
            ("the national capital, ", "resolved"),
            ("no longer the capital, ", "ambiguous"),
            ("once the capital, ", "ambiguous"),
            ("Port, the capital, ", "ambiguous"),
            ("strikes did not stop in the national capital, ", "resolved"),
            ("British Columbia, protest in the capital, ", "ambiguous"),
        ] {
            let p = LocationProposal {
                quote: format!("Fire in {prefix}Fictional."),
                ..short.clone()
            };
            assert_eq!(
                resolve_indexes(&indexes, &p.quote, &p, true)
                    .unwrap()
                    .unwrap()
                    .status,
                status
            );
        }
        let qualified = LocationProposal {
            quote: "Fire in city of Fictional.".into(),
            country_code: Some("YE".into()),
            ..short.clone()
        };
        let resolved = resolve_indexes(&indexes, &qualified.quote, &qualified, true)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.status, "resolved");
        assert_eq!(
            resolved.candidates[0].match_basis,
            "exact_name_with_literal_feature_qualifier"
        );
        // A second current alias of the *full* name still blocks placement.
        fs::write(
            &aliases,
            "1\t900021\ten\tFictional\t1\t\t\t\t\t\n2\t900022\ten\tFictional City\t1\t\t\t\t\t\n",
        )
        .unwrap();
        let duplicate = temp.path().join("duplicate");
        build(
            &duplicate,
            &places,
            &aliases,
            json!({"product":"cities500"}),
            &AtomicBool::new(false),
        )
        .unwrap();
        let p = proposal("Fictional City", None);
        assert_eq!(
            resolve_indexes(&[Index::inspect(&duplicate).unwrap()], &p.quote, &p, true)
                .unwrap()
                .unwrap()
                .status,
            "ambiguous"
        );
        // A capital phrase cannot remove collision evidence when the matched
        // candidate is an ordinary town rather than a national capital.
        let raw = fs::read_to_string(&places)
            .unwrap()
            .replace("\tPPLC\t", "\tPPL\t")
            .replace(
                "Another City\tAnother City\tFictional\t",
                "Another City\tAnother City\tFictional,Fictional City\t",
            );
        fs::write(&places, raw).unwrap();
        fs::write(&aliases, []).unwrap();
        let ordinary = temp.path().join("ordinary");
        build(
            &ordinary,
            &places,
            &aliases,
            json!({"product":"cities500"}),
            &AtomicBool::new(false),
        )
        .unwrap();
        let p = LocationProposal {
            quote: "Fire in the capital, Fictional City.".into(),
            ..proposal("Fictional City", None)
        };
        assert_eq!(
            resolve_indexes(&[Index::inspect(&ordinary).unwrap()], &p.quote, &p, true)
                .unwrap()
                .unwrap()
                .status,
            "ambiguous"
        );
    }
    #[test]
    fn flattened_global_names_veto_collisions_but_never_create_a_positive_match() {
        let temp = tempfile::tempdir().unwrap();
        let places = temp.path().join("places.txt");
        let aliases = temp.path().join("aliases.txt");
        let first = "900021\tFictional Hub\tFictional Hub\tРегиональное имя\t14\t43\tP\tPPL\tYE\t\t01\t\t\t\t9000\t\t10\tAsia/Aden\t2026-09-12\n";
        let other = "900099\tOther Hub\tOther Hub\tРегиональное имя\t24\t53\tP\tPPL\tSA\t\t01\t\t\t\t9000\t\t10\tAsia/Riyadh\t2026-09-12\n";
        fs::write(&places, first).unwrap();
        fs::write(&aliases, "1\t900021\tru\tРегиональное имя\t1\t\t\t\t\t\n2\t900021\ten\tOld Native Name\t\t\t\t1\t1900\t1950\n").unwrap();
        let regional = temp.path().join("regional");
        build(
            &regional,
            &places,
            &aliases,
            json!({"product":"YE"}),
            &AtomicBool::new(false),
        )
        .unwrap();
        for (label, rows, expected) in [
            ("unique", first.to_owned(), "resolved"),
            ("collision", format!("{first}{other}"), "ambiguous"),
        ] {
            fs::write(&places, rows).unwrap();
            fs::write(&aliases, []).unwrap();
            let global = temp.path().join(label);
            build(
                &global,
                &places,
                &aliases,
                json!({"product":"cities500"}),
                &AtomicBool::new(false),
            )
            .unwrap();
            let indexes = [
                Index::inspect(&regional).unwrap(),
                Index::inspect(&global).unwrap(),
            ];
            let p = proposal("Региональное имя", None);
            let r = resolve_indexes(&indexes, &p.quote, &p, true)
                .unwrap()
                .unwrap();
            assert_eq!(r.status, expected);
            assert_eq!(r.geometry.is_some(), expected == "resolved");
            if expected == "ambiguous" {
                assert_eq!(r.candidate_count, 2);
                assert!(r.candidates[1].geometry.is_none());
                assert_eq!(
                    r.candidates[1].match_basis,
                    "unverified_global_alias_collision"
                );
                assert!(r.selected_candidate_id.is_none());
            }
            // No tagged positive alias: flattened names cannot return a point.
            assert!(
                resolve_indexes(&indexes[1..], &p.quote, &p, true)
                    .unwrap()
                    .is_none()
            );
            let old = proposal("Old Native Name", None);
            assert!(
                resolve_indexes(&indexes, &old.quote, &old, true)
                    .unwrap()
                    .is_none()
            );
            let scoped = proposal("Региональное имя", Some("YE"));
            assert_eq!(
                resolve_indexes(&indexes, &scoped.quote, &scoped, true)
                    .unwrap()
                    .unwrap()
                    .status,
                "resolved"
            );
            if label == "unique" {
                let path = global.join("manifest.json");
                let mut legacy: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                legacy
                    .as_object_mut()
                    .unwrap()
                    .remove("possible_name_count");
                fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
                let legacy = [Index::inspect(&global).unwrap()];
                let primary = proposal("Fictional Hub", None);
                let r = resolve_indexes(&legacy, &primary.quote, &primary, true)
                    .unwrap()
                    .unwrap();
                assert_eq!(r.status, "country_context_required");
                assert!(r.geometry.is_none());
                let scoped = proposal("Fictional Hub", Some("YE"));
                assert_eq!(
                    resolve_indexes(&legacy, &scoped.quote, &scoped, true)
                        .unwrap()
                        .unwrap()
                        .status,
                    "resolved"
                );
            }
        }
    }

    #[test]
    fn regional_unique_names_need_country_unless_identity_is_in_global_baseline() {
        let temp = tempfile::tempdir().unwrap();
        let places = temp.path().join("input.txt");
        let aliases = temp.path().join("aliases.txt");
        fs::write(&aliases, []).unwrap();
        let global_row = "900021\tFictional Hub\tFictional Hub\t\t14\t43\tP\tPPL\tYE\t\t01\t\t\t\t9000\t\t10\tAsia/Aden\t2026-09-12\n";
        let regional_row = "900022\tSmall Hamlet\tSmall Hamlet\t\t15\t44\tP\tPPL\tYE\t\t01\t\t\t\t3\t\t10\tAsia/Aden\t2026-09-12\n";
        fs::write(&places, global_row).unwrap();
        let global = temp.path().join("global");
        build(
            &global,
            &places,
            &aliases,
            json!({"product":"cities500"}),
            &AtomicBool::new(false),
        )
        .unwrap();
        fs::write(&places, format!("{global_row}{regional_row}")).unwrap();
        fs::write(&aliases, "1\t900021\tru\tРегиональное имя\t1\t\t\t\t\t\n").unwrap();
        let regional = temp.path().join("regional");
        build(
            &regional,
            &places,
            &aliases,
            json!({"product":"YE"}),
            &AtomicBool::new(false),
        )
        .unwrap();
        let indexes = [
            Index::inspect(&regional).unwrap(),
            Index::inspect(&global).unwrap(),
        ];
        for (name, country, expected) in [
            ("Fictional Hub", None, "resolved"),
            ("Региональное имя", None, "country_context_required"),
            ("Региональное имя", Some("YE"), "resolved"),
            ("Small Hamlet", None, "country_context_required"),
            ("Small Hamlet", Some("YE"), "resolved"),
        ] {
            let p = proposal(name, country);
            let r = resolve_indexes(&indexes, &p.quote, &p, true)
                .unwrap()
                .unwrap();
            assert_eq!(r.status, expected);
            assert_eq!(r.geometry.is_some(), expected == "resolved");
            assert_eq!(r.candidate_count, 1);
        }
    }
    #[test]
    fn multilingual_index_preserves_ambiguity_history_precision_and_roles() {
        let temp = tempfile::tempdir().unwrap();
        let path = seed(temp.path());
        let index = Index::inspect(&path).unwrap();
        for (name, country, status) in [
            ("Fictional Port", None, "ambiguous"),
            ("Fictional Port", Some("YE"), "resolved"),
            ("ميناء خيالي", Some("YE"), "resolved"),
            ("Fictional Province", None, "broad"),
            ("Fictional Port", Some("FR"), "country_conflict"),
        ] {
            let p = proposal(name, country);
            let r = index.resolve(&p.quote, &p, true).unwrap().unwrap();
            assert_eq!(r.status, status);
            assert_eq!(r.geometry.is_some(), status == "resolved");
            if status == "resolved" {
                assert!(r.candidates[0].reference.publisher_record_id == "900001");
            }
        }
        for name in [
            "Old Fictional Port",
            "DO NOT USE THIS FLATTENED ALIAS",
            "https://fictional.invalid",
        ] {
            let p = proposal(name, None);
            assert!(index.resolve(&p.quote, &p, true).unwrap().is_none());
        }
        let mut p = proposal("ميناء خيالي", None);
        p.role = LocationRole::Issuer;
        assert!(
            index
                .resolve(&p.quote, &p, true)
                .unwrap()
                .unwrap()
                .geometry
                .is_none()
        );
        p.role = LocationRole::Event;
        assert!(
            index
                .resolve(&p.quote, &p, false)
                .unwrap()
                .unwrap()
                .geometry
                .is_none()
        );
        assert!(index.resolve("unrelated original", &p, true).is_err());
        fs::write(path.join("places.txt"), "corrupted").unwrap();
        assert!(Index::inspect(&path).is_err());
    }
    #[test]
    fn failed_or_cancelled_build_does_not_replace_previous_edition() {
        let temp = tempfile::tempdir().unwrap();
        let path = seed(temp.path());
        let before = fs::read(path.join("manifest.json")).unwrap();
        let places = temp.path().join("input-places.txt");
        let aliases = temp.path().join("input-aliases.txt");
        assert!(build(&path, &places, &aliases, json!({}), &AtomicBool::new(false)).is_err());
        assert!(
            build(
                &temp.path().join("cancelled"),
                &places,
                &aliases,
                json!({}),
                &AtomicBool::new(true)
            )
            .is_err()
        );
        assert!(!temp.path().join("cancelled").exists());
        fs::write(&places, "malformed\tfile\n").unwrap();
        assert!(
            build(
                &temp.path().join("failed"),
                &places,
                &aliases,
                json!({}),
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(!temp.path().join("failed").exists());
        assert_eq!(fs::read(path.join("manifest.json")).unwrap(), before);
    }
}
