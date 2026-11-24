//! The ship finder's evaluation gate. Authored fictional messages
//! (`examples/vessel-mentions-evaluation.json`) go through the exact pinned
//! extraction request. Each request is admitted to a directory before it is
//! sent and each raw reply is retained there with a receipt. Scoring always
//! reads the retained replies, so a replay rescores them with no model call.
//! A record that meets the fixture's thresholds can be installed into a
//! stopped workspace; paid extraction needs one for the current
//! [`fingerprint`](super::fingerprint).
use super::{MODEL, Mention, Sender, decode, fingerprint, plain_name, request};
use crate::{
    error::{Error, Result},
    model::{hash, now, string},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub const KIND: &str = "relic.vessel-finder-evaluation.v1";
pub const FIXTURE: &str = include_str!("../../../examples/vessel-mentions-evaluation.json");
/// Requests in flight at once during a paid evaluation.
const CONCURRENCY: usize = 8;
/// No retained file read back is larger than this.
const MAX_FILE: u64 = 2 * 1024 * 1024;
const IDENTIFIERS: [&str; 4] = ["imo", "mmsi", "hull_number", "callsign"];
const THRESHOLDS: [&str; 7] = ["all_replies_decoded", "recall_min", "precision_min", "negative_vessels_max", "identifier_errors_max", "stated_attributes_min", "latin_names_min"];

/// The fixture, checked: fictional, every threshold defined, unique short
/// case ids, and positives that name vessels while negatives name none.
pub fn fixture() -> Result<Value> {
    let f: Value = serde_json::from_str(FIXTURE)?;
    let invalid = |why: String| Error::invalid(format!("The ship finder evaluation fixture is invalid: {why}"));
    if f["fictional"] != true {
        return Err(invalid("it must be marked fictional".into()));
    }
    if let Some(k) = THRESHOLDS.iter().find(|k| f["thresholds"].get(**k).is_none()) {
        return Err(invalid(format!("threshold {k} is not defined")));
    }
    let cases = f["cases"].as_array().filter(|c| !c.is_empty()).ok_or_else(|| invalid("it has no cases".into()))?;
    let mut ids = BTreeSet::new();
    for c in cases {
        let id = c["id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
            .ok_or_else(|| invalid("a case id is not a short lowercase name".into()))?;
        if !ids.insert(id) {
            return Err(invalid(format!("case id {id} repeats")));
        }
        let expected = c["expected"].as_array().ok_or_else(|| invalid(format!("{id} has no expected list")))?;
        if c["text"].as_str().is_none_or(|t| t.trim().is_empty()) {
            return Err(invalid(format!("{id} has no text")));
        }
        match c["kind"].as_str() {
            Some("positive") if !expected.is_empty() => {}
            Some("negative") if expected.is_empty() => {}
            _ => return Err(invalid(format!("{id}: a positive case names vessels and a negative one names none"))),
        }
    }
    Ok(f)
}
pub fn fixture_sha256() -> String {
    hash(FIXTURE.as_bytes())
}
/// A string or a list of strings.
fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
        _ => vec![],
    }
}

/// The authored expectation written as a reply of the pinned profile. It
/// checks that the fixture is consistent and serves tests; it is never
/// evidence of model quality.
pub fn gold_reply(case: &Value) -> Value {
    let vessels: Vec<Value> = case["expected"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            let name = e["name"].as_str();
            json!({"name":name,"quote":e["quote"],"name_latin":e["latin"][0].as_str().or(name),"imo":e["imo"].as_str(),"mmsi":e["mmsi"].as_str(),
                "callsign":e["callsign"].as_str(),"hull_number":e["hull_number"].as_str(),"flag":e["flag"].as_str(),"vessel_type":strings(&e["vessel_type"]).first()})
        })
        .collect();
    json!({"model":MODEL,"provider":"OpenAI","choices":[{"finish_reason":"stop","message":{"role":"assistant","content":string(&json!({"vessels":vessels}))}}],"usage":{"cost":0.0}})
}

// ---------------------------------------------------------------- retained files

/// What a directory holds for one case.
#[derive(Clone, Debug, PartialEq)]
pub enum Retained {
    /// The exact reply: HTTP status and body bytes.
    Reply { status: u16, body: Vec<u8> },
    /// Admitted, and possibly sent, with no complete receipt: billing is
    /// unknown and the request is never sent again.
    Unknown(String),
    /// Never admitted.
    Missing,
}
/// A case's admission (the exact request), raw reply and receipt.
fn files(dir: &Path, id: &str) -> (PathBuf, PathBuf, PathBuf) {
    (dir.join(format!("{id}.request.json")), dir.join(format!("{id}.reply.json")), dir.join(format!("{id}.receipt.json")))
}
/// A case's id, its request and the request's exact bytes.
fn parts(case: &Value) -> Result<(&str, Value, String)> {
    let id = case["id"].as_str().ok_or_else(|| Error::invalid("An evaluation case has no id"))?;
    let body = request(case["text"].as_str().unwrap_or_default())?;
    let bytes = string(&body);
    Ok((id, body, bytes))
}
fn sync_dir(path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_dir(path)
}
/// Written whole or not at all: a staging file renamed into place.
fn commit(path: &Path, bytes: &[u8]) -> Result<()> {
    let staged = path.with_extension("staging");
    write_new(&staged, bytes)?;
    fs::rename(&staged, path)?;
    sync_dir(path)
}
fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    if fs::metadata(path)?.len() > MAX_FILE {
        return Err(Error::invalid(format!("{} exceeds the evaluation file size limit", path.display())));
    }
    Ok(fs::read(path)?)
}
fn changed(id: &str) -> Error {
    Error::conflict(format!(
        "The retained request for {id} differs from the current profile or case text. A replay cannot evaluate a changed prompt, schema, model or message; use a new directory."
    ))
}
/// Record one case's exact request before it is sent. None when the case
/// already has a complete receipt, so nothing is sent again. An admission
/// without a receipt may have been billed, so it is refused, never resent.
pub fn admit(dir: &Path, case: &Value) -> Result<Option<Value>> {
    let (id, body, bytes) = parts(case)?;
    let (admission, _, receipt) = files(dir, id);
    if receipt.exists() {
        return Ok(None);
    }
    if admission.exists() {
        return Err(Error::conflict(format!("{id} was sent before without a complete receipt; billing is unknown and it is not resent")));
    }
    write_new(&admission, bytes.as_bytes())?;
    Ok(Some(body))
}
/// Retain the exact reply, then its receipt; the receipt completes the case.
pub fn retain(dir: &Path, case: &Value, status: u16, body: &[u8], elapsed_ms: u128, generation_id: Option<&str>) -> Result<()> {
    let (id, _, bytes) = parts(case)?;
    let (_, reply, receipt) = files(dir, id);
    commit(&reply, body)?;
    let meta = json!({"case":id,"request_sha256":hash(bytes.as_bytes()),"status":status,"reply_sha256":hash(body),
        "elapsed_ms":u64::try_from(elapsed_ms).unwrap_or(u64::MAX),"generation_id":generation_id,"received_at":now()});
    commit(&receipt, string(&meta).as_bytes())
}
/// A request that never left Relic cost nothing: its admission is removed.
fn withdraw(dir: &Path, case: &Value) -> Result<()> {
    let (id, _, _) = parts(case)?;
    let (admission, _, _) = files(dir, id);
    fs::remove_file(&admission)?;
    sync_dir(&admission)
}
/// What `dir` holds for this case. A retained request that differs from the
/// current one, or a reply that differs from its receipt, is an error: a
/// replay never scores another profile's replies as this one's.
pub fn load(dir: &Path, case: &Value) -> Result<Retained> {
    let (id, _, bytes) = parts(case)?;
    let (admission, reply, receipt) = files(dir, id);
    let admitted = admission.exists();
    if admitted && read_bounded(&admission)? != bytes.as_bytes() {
        return Err(changed(id));
    }
    if !receipt.exists() {
        return Ok(if admitted { Retained::Unknown("Admitted without a complete receipt; billing is unknown and it is not resent".into()) } else { Retained::Missing });
    }
    let meta: Value = serde_json::from_slice(&read_bounded(&receipt)?).map_err(|_| Error::invalid(format!("The receipt for {id} is not valid JSON")))?;
    if meta["request_sha256"] != hash(bytes.as_bytes()) {
        return Err(changed(id));
    }
    let body = read_bounded(&reply)?;
    if meta["reply_sha256"] != hash(&body) {
        return Err(Error::invalid(format!("The retained reply for {id} does not match its receipt")));
    }
    let status = meta["status"].as_u64().and_then(|s| u16::try_from(s).ok()).ok_or_else(|| Error::invalid(format!("The receipt for {id} has no HTTP status")))?;
    Ok(Retained::Reply { status, body })
}

// ---------------------------------------------------------------- scoring

fn field<'a>(m: &'a Mention, key: &str) -> Option<&'a str> {
    match key {
        "imo" => m.imo.as_deref(),
        "mmsi" => m.mmsi.as_deref(),
        "hull_number" => m.hull_number.as_deref(),
        "callsign" => m.callsign.as_deref(),
        "flag" => m.flag.as_deref(),
        "vessel_type" => m.vessel_type.as_deref(),
        _ => None,
    }
}
/// Written forms the case accepts for a vessel's name, without ship
/// prefixes, punctuation or case.
fn accepted_names(e: &Value) -> Vec<String> {
    strings(&e["name"]).iter().chain(&strings(&e["also"])).map(|n| plain_name(n)).filter(|n| !n.is_empty()).collect()
}
fn label(name: Option<&str>, identifier: impl Fn(&str) -> Option<String>) -> String {
    name.map(str::to_owned).or_else(|| IDENTIFIERS.iter().find_map(|k| identifier(k).map(|v| format!("{k} {v}")))).unwrap_or_else(|| "vessel".into())
}
fn expected_label(e: &Value) -> String {
    label(e["name"].as_str(), |k| e[k].as_str().map(str::to_owned))
}
/// A named vessel matches on its name; one identified only by a number
/// matches on that number, written in any field or as the name.
fn same_vessel(m: &Mention, e: &Value) -> bool {
    let names = accepted_names(e);
    if !names.is_empty() {
        return m.name.as_deref().is_some_and(|n| names.contains(&plain_name(n)));
    }
    IDENTIFIERS.iter().any(|k| e[*k].as_str().is_some_and(|v| field(m, k) == Some(v) || m.name.as_deref().is_some_and(|n| plain_name(n) == plain_name(v))))
}
/// Latin letters only (accents allowed), and at least one.
fn latin_script(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_alphabetic()) && s.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_ascii_alphabetic() || ('\u{C0}'..='\u{24F}').contains(&c))
}
/// One case against its retained reply.
fn score_case(case: &Value, retained: &Retained) -> Value {
    let text = case["text"].as_str().unwrap_or_default();
    let expected: Vec<Value> = case["expected"].as_array().cloned().unwrap_or_default();
    let identifiers_expected = expected.iter().map(|e| IDENTIFIERS.iter().filter(|k| e[**k].is_string()).count()).sum::<usize>();
    let mut r = json!({"id":case["id"],"kind":case["kind"],"language":case["language"],"expected":expected.len(),"identifiers_expected":identifiers_expected,
        "attempted":true,"status":null,"decoded":false,"cost_usd":null,"passed":false});
    let (status, body) = match retained {
        Retained::Missing => {
            r["attempted"] = json!(false);
            r["error"] = json!("No retained reply for this case");
            return r;
        }
        Retained::Unknown(why) => {
            r["error"] = json!(why);
            return r;
        }
        Retained::Reply { status, body } => (*status, body),
    };
    r["status"] = json!(status);
    let response = serde_json::from_slice::<Value>(body).ok();
    if let Some(cost) = response.as_ref().and_then(|v| v["usage"]["cost"].as_f64()).filter(|c| c.is_finite() && *c >= 0.0) {
        r["cost_usd"] = json!(cost);
    }
    let decoded = match (status, &response) {
        (200, Some(v)) => decode(text, v).map_err(|e| e.message),
        (200, None) => Err("The reply is not JSON".to_owned()),
        (s, _) => Err(format!("Model provider HTTP {s}")),
    };
    let mentions = match decoded {
        Ok(m) => m,
        Err(e) => {
            r["error"] = json!(e);
            return r;
        }
    };
    r["decoded"] = json!(true);
    let mut matched = vec![false; expected.len()];
    let (mut extra, mut duplicates, mut identifier_errors, mut wrong) = (vec![], vec![], vec![], vec![]);
    let (mut stated, mut correct, mut other_script, mut latin, mut listed) = (0, 0, 0, 0, 0);
    for m in &mentions {
        let shown = label(m.name.as_deref(), |k| field(m, k).map(str::to_owned));
        let Some(i) = (0..expected.len()).find(|&i| !matched[i] && same_vessel(m, &expected[i])) else {
            if expected.iter().any(|e| same_vessel(m, e)) { duplicates.push(shown) } else { extra.push(shown) }
            continue;
        };
        matched[i] = true;
        let e = &expected[i];
        let vessel = expected_label(e);
        for k in IDENTIFIERS {
            if e[k].as_str() != field(m, k) {
                identifier_errors.push(json!({"vessel":vessel,"field":k,"expected":e[k].as_str(),"returned":field(m, k)}));
            }
        }
        for k in ["flag", "vessel_type"] {
            let accepted = strings(&e[k]);
            if accepted.is_empty() {
                continue;
            }
            stated += 1;
            if field(m, k).is_some_and(|v| accepted.iter().any(|a| a == v)) {
                correct += 1;
            } else {
                wrong.push(json!({"vessel":vessel,"field":k,"expected":accepted,"returned":field(m, k)}));
            }
        }
        if e["name"].as_str().is_some_and(|n| !latin_script(n)) {
            other_script += 1;
            let given = m.name_latin.as_deref().filter(|l| latin_script(l));
            latin += usize::from(given.is_some());
            listed += usize::from(given.is_some_and(|l| strings(&e["latin"]).iter().any(|a| plain_name(a) == plain_name(l))));
        }
    }
    let missing: Vec<String> = expected.iter().zip(&matched).filter(|(_, m)| !**m).map(|(e, _)| expected_label(e)).collect();
    r["passed"] = json!(missing.is_empty() && extra.is_empty() && duplicates.is_empty() && identifier_errors.is_empty());
    r["returned"] = json!(mentions.len());
    r["matched"] = json!(matched.iter().filter(|m| **m).count());
    r["missing"] = json!(missing);
    r["extra"] = json!(extra);
    r["duplicates"] = json!(duplicates);
    r["identifier_errors"] = json!(identifier_errors);
    r["stated_attributes"] = json!({"checked":stated,"correct":correct,"wrong":wrong});
    r["latin"] = json!({"other_script_names":other_script,"latin_names":latin,"listed_spellings":listed});
    r["mentions"] = json!(mentions);
    r
}
/// Totals over every case. Ratios are exact (unrounded) so a threshold is
/// never met by rounding.
fn summarize(results: &[Value]) -> Value {
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let len = |v: &Value| v.as_array().map_or(0, Vec::len) as u64;
    let (mut positives, mut expected, mut matched, mut returned, mut unmatched) = (0, 0, 0, 0, 0);
    let (mut negatives, mut negatives_with, mut negative_vessels) = (0, 0, 0);
    let (mut identifiers, mut identifier_errors, mut stated, mut correct, mut other_script, mut latin, mut listed) = (0, 0, 0, 0, 0, 0, 0);
    let (mut replies, mut decoded, mut passed, mut known, mut unknown) = (0, 0, 0, 0.0, 0);
    for r in results {
        if r["attempted"] == true {
            match r["cost_usd"].as_f64() {
                Some(c) => known += c,
                None => unknown += 1,
            }
        }
        replies += u64::from(r["status"].is_number());
        decoded += u64::from(r["decoded"] == true);
        passed += u64::from(r["passed"] == true);
        if r["kind"] == "positive" {
            positives += 1;
            expected += n(&r["expected"]);
            matched += n(&r["matched"]);
            returned += n(&r["returned"]);
            unmatched += len(&r["extra"]) + len(&r["duplicates"]);
        } else {
            negatives += 1;
            negative_vessels += n(&r["returned"]);
            negatives_with += u64::from(n(&r["returned"]) > 0);
        }
        identifiers += n(&r["identifiers_expected"]);
        identifier_errors += len(&r["identifier_errors"]);
        stated += n(&r["stated_attributes"]["checked"]);
        correct += n(&r["stated_attributes"]["correct"]);
        other_script += n(&r["latin"]["other_script_names"]);
        latin += n(&r["latin"]["latin_names"]);
        listed += n(&r["latin"]["listed_spellings"]);
    }
    let ratio = |a: u64, b: u64, none: f64| if b == 0 { none } else { a as f64 / b as f64 };
    json!({"cases":results.len(),"positive_cases":positives,"negative_cases":negatives,"replies":replies,"decoded":decoded,"cases_passed":passed,
        "expected_vessels":expected,"matched_vessels":matched,"recall":ratio(matched, expected, 0.0),
        "returned_on_positives":returned,"unmatched_on_positives":unmatched,"precision":ratio(matched, returned, 0.0),
        "negative_cases_with_vessels":negatives_with,"negative_vessels":negative_vessels,
        "identifiers_expected":identifiers,"identifier_errors":identifier_errors,
        "stated_attributes":stated,"stated_attributes_correct":correct,"stated_attribute_accuracy":ratio(correct, stated, 1.0),
        "other_script_names":other_script,"latin_names":latin,"latin_name_rate":ratio(latin, other_script, 1.0),"latin_spellings_listed":listed,
        "known_cost_usd":known,"unknown_cost_requests":unknown})
}
/// The thresholds a summary misses; empty when it passes. A threshold the
/// fixture does not define fails closed.
pub fn failures(s: &Value, t: &Value) -> Vec<String> {
    let mut out = vec![];
    let cases = s["cases"].as_u64().unwrap_or(0);
    let decoded = s["decoded"].as_u64().unwrap_or(0);
    if t["all_replies_decoded"] != json!(false) && (cases == 0 || decoded != cases) {
        out.push(format!("{} of {cases} cases have no reply that decodes with exact quotes", cases.saturating_sub(decoded)));
    }
    let shown = |v: f64| if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v:.4}") };
    let check = |value: &str, limit: &str, what: &str, at_least: bool| -> Option<String> {
        match (s[value].as_f64(), t[limit].as_f64()) {
            (Some(v), Some(l)) if (at_least && v >= l) || (!at_least && v <= l) => None,
            (Some(v), Some(l)) => Some(format!("{what} is {}, {} {l}", shown(v), if at_least { "below the minimum" } else { "above the maximum" })),
            _ => Some(format!("{what}: threshold {limit} is not defined")),
        }
    };
    out.extend(
        [
            check("recall", "recall_min", "Recall of expected vessels", true),
            check("precision", "precision_min", "Precision on messages that name vessels", true),
            check("negative_vessels", "negative_vessels_max", "Vessels returned for messages that name none", false),
            check("identifier_errors", "identifier_errors_max", "Identifier errors", false),
            check("stated_attribute_accuracy", "stated_attributes_min", "Accuracy of stated flags and types", true),
            check("latin_name_rate", "latin_names_min", "Latin search names for names in other scripts", true),
        ]
        .into_iter()
        .flatten(),
    );
    out
}
/// The evaluation record: one retained reply (or its absence) per fixture
/// case, in fixture order.
pub fn record(f: &Value, retained: &[Retained], mode: &str, new_calls: usize) -> Value {
    let cases = f["cases"].as_array().cloned().unwrap_or_default();
    let results: Vec<Value> = cases.iter().enumerate().map(|(i, c)| score_case(c, retained.get(i).unwrap_or(&Retained::Missing))).collect();
    let summary = summarize(&results);
    let failures = failures(&summary, &f["thresholds"]);
    let fp = fingerprint();
    json!({"kind":KIND,"fictional":true,"mode":mode,"checked_at":now(),"fingerprint":fp,"fingerprint_sha256":hash(string(&fp).as_bytes()),
        "fixture_version":f["version"],"fixture_sha256":fixture_sha256(),"thresholds":f["thresholds"],"cases":results.len(),"new_calls":new_calls,
        "passed":failures.is_empty(),"failures":failures,"summary":summary,"results":results,
        "limitations":"Authored fictional messages scored by fixed rules. A pass shows that this profile met these thresholds on this set, not its accuracy on real messages."})
}
fn score_dir(dir: &Path, mode: &str, new_calls: usize) -> Result<Value> {
    let f = fixture()?;
    let mut retained = vec![];
    for case in f["cases"].as_array().into_iter().flatten() {
        retained.push(load(dir, case)?);
    }
    Ok(record(&f, &retained, mode, new_calls))
}
/// Score the replies retained in `dir`. No model request is made.
pub fn replay(dir: &Path) -> Result<Value> {
    score_dir(dir, "replay", 0)
}

// ---------------------------------------------------------------- the gate

/// A passed record of the current profile: what paid extraction needs.
pub fn current(record: &Value) -> bool {
    record["kind"] == KIND && record["fictional"] == true && record["passed"] == true && record["fingerprint"] == fingerprint()
}
/// What the interface shows of an installed record.
pub fn public(record: &Value) -> Value {
    json!({"passed":record["passed"] == true,"current":current(record),"checked_at":record["checked_at"],"mode":record["mode"],"cases":record["cases"],
        "fingerprint_sha256":record["fingerprint_sha256"],"summary":record["summary"]})
}
/// A record may be installed only when it passed, is for the current
/// profile, used this version's cases and thresholds, and its summary meets
/// them.
pub fn installable(record: &Value) -> Result<()> {
    if record["kind"] != KIND || record["fictional"] != true {
        return Err(Error::invalid("This is not a ship finder evaluation record"));
    }
    if record["passed"] != true {
        return Err(Error::invalid("The evaluation did not pass; a failed record is not installed"));
    }
    if record["fingerprint"] != fingerprint() {
        return Err(Error::invalid("The evaluation is for another model profile (model, provider routing, prompt, schema or decoder); evaluate the current profile"));
    }
    let f = fixture()?;
    let cases = f["cases"].as_array().map_or(0, Vec::len);
    if record["fixture_sha256"] != fixture_sha256() || record["thresholds"] != f["thresholds"] || record["cases"] != cases || record["results"].as_array().map_or(0, Vec::len) != cases {
        return Err(Error::invalid("The evaluation used other cases or thresholds than this version's fixture"));
    }
    if !failures(&record["summary"], &f["thresholds"]).is_empty() {
        return Err(Error::invalid("The record's summary does not meet the fixture's thresholds"));
    }
    Ok(())
}

// ---------------------------------------------------------------- the paid run

/// What one case's attempt did.
struct Attempted {
    /// A request left (or may have left) Relic.
    sent: bool,
    error: Option<String>,
}
async fn attempt(dir: &Path, case: &Value, sender: &Sender, stop: &AtomicBool, halt: Arc<AtomicBool>) -> Attempted {
    let id = case["id"].as_str().unwrap_or_default().to_owned();
    let failed = |sent: bool, why: String| Attempted { sent, error: Some(format!("{id}: {why}")) };
    if files(dir, &id).2.exists() {
        return Attempted { sent: false, error: None };
    }
    if stop.load(Ordering::Acquire) {
        return failed(false, "stopped before sending".into());
    }
    let body = match admit(dir, case) {
        Ok(Some(body)) => body,
        Ok(None) => return Attempted { sent: false, error: None },
        Err(e) => return failed(false, e.message),
    };
    match sender.post(&body, halt).await {
        Ok(r) => match retain(dir, case, r.status, &r.body, r.elapsed_ms, r.generation_id.as_deref()) {
            Ok(()) => Attempted { sent: true, error: None },
            Err(e) => failed(true, format!("the reply could not be retained ({}); billing is unknown and it is not resent", e.message)),
        },
        Err(e) if e.code == "model_not_sent" => match withdraw(dir, case) {
            Ok(()) => failed(false, e.message),
            Err(w) => failed(false, format!("{}; its admission could not be removed ({}), so it is not sent again", e.message, w.message)),
        },
        Err(e) => failed(true, format!("{}; billing is unknown and it is not resent", e.message)),
    }
}
/// The paid evaluation through OpenRouter, configured by `values`
/// (OPENROUTER_*). `stop` ends admission: requests already sent finish and
/// are retained. `halt` cuts requests in flight; their billing is unknown.
pub async fn run(dir: &Path, values: &BTreeMap<String, String>, stop: Arc<AtomicBool>, halt: Arc<AtomicBool>) -> Result<Value> {
    let sender = Sender::OpenRouter(crate::model_transport::Client::chat(values)?);
    run_with(dir, &sender, stop, halt).await
}
/// Admit, send and retain every case without a complete receipt, then score
/// the directory. A case with a receipt is reused and an admission without
/// one is never resent, so rerunning into the same directory bills nothing
/// twice.
pub(crate) async fn run_with(dir: &Path, sender: &Sender, stop: Arc<AtomicBool>, halt: Arc<AtomicBool>) -> Result<Value> {
    let f = fixture()?;
    let cases = f["cases"].as_array().cloned().unwrap_or_default();
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    // One directory holds one profile's requests: checked before anything is sent.
    for case in &cases {
        load(dir, case)?;
    }
    let attempts = futures_util::stream::iter(cases.iter().map(|case| attempt(dir, case, sender, &stop, halt.clone()))).buffer_unordered(CONCURRENCY).collect::<Vec<_>>().await;
    let mut record = score_dir(dir, "paid", attempts.iter().filter(|a| a.sent).count())?;
    record["acquisition_errors"] = json!(attempts.into_iter().filter_map(|a| a.error).collect::<Vec<_>>());
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vessel_finder::fixture as stand_in;

    fn cases() -> Vec<Value> {
        fixture().unwrap()["cases"].as_array().unwrap().clone()
    }
    fn gold() -> Vec<Retained> {
        cases().iter().map(|c| Retained::Reply { status: 200, body: string(&gold_reply(c)).into_bytes() }).collect()
    }
    #[test]
    fn the_fixture_is_consistent_and_its_gold_replies_meet_every_threshold() {
        let f = fixture().unwrap();
        let record = record(&f, &gold(), "replay", 0);
        let failed: Vec<&Value> = record["results"].as_array().unwrap().iter().filter(|r| r["passed"] != true).collect();
        assert!(failed.is_empty(), "{failed:?}");
        let s = &record["summary"];
        assert_eq!((s["cases"].clone(), s["positive_cases"].clone(), s["negative_cases"].clone()), (json!(40), json!(28), json!(12)));
        assert_eq!((s["expected_vessels"].clone(), s["recall"].clone(), s["precision"].clone()), (json!(34), json!(1.0), json!(1.0)));
        assert_eq!((s["negative_vessels"].clone(), s["identifier_errors"].clone(), s["latin_name_rate"].clone()), (json!(0), json!(0), json!(1.0)));
        // Every Latin spelling the gold reply gives is one the case lists.
        assert_eq!(s["latin_spellings_listed"], s["other_script_names"]);
        assert_eq!(record["passed"], true, "{}", record["failures"]);
        installable(&record).unwrap();
    }
    #[test]
    fn missing_replies_and_unknown_billing_fail_the_gate() {
        let f = fixture().unwrap();
        let mut retained = gold();
        retained[0] = Retained::Unknown("Admitted without a complete receipt".into());
        retained.truncate(39);
        let record = record(&f, &retained, "replay", 0);
        assert_eq!(record["passed"], false);
        assert_eq!(record["failures"][0], "2 of 40 cases have no reply that decodes with exact quotes");
        assert_eq!((record["summary"]["unknown_cost_requests"].clone(), record["results"][39]["attempted"].clone()), (json!(1), json!(false)));
        assert!(installable(&record).unwrap_err().message.contains("did not pass"));
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_paid_run_admits_before_sending_retains_every_reply_and_never_resends() {
        let dir = tempfile::tempdir().unwrap();
        // A stand-in that names no vessel: negatives pass, positives miss.
        let model = stand_in::model(200, stand_in::answer(0.001)).await;
        let sender = Sender::Local(model.url());
        let (stop, halt) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let first = run_with(dir.path(), &sender, stop.clone(), halt.clone()).await.unwrap();
        assert_eq!((model.calls(), first["new_calls"].clone(), first["mode"].clone()), (40, json!(40), json!("paid")));
        assert_eq!((first["passed"].clone(), first["summary"]["negative_vessels"].clone(), first["summary"]["recall"].clone()), (json!(false), json!(0), json!(0.0)));
        assert!((first["summary"]["known_cost_usd"].as_f64().unwrap() - 0.04).abs() < 1e-9);
        // The exact profile request was admitted for every case.
        for case in cases() {
            let sent = fs::read(dir.path().join(format!("{}.request.json", case["id"].as_str().unwrap()))).unwrap();
            assert_eq!(sent, string(&request(case["text"].as_str().unwrap()).unwrap()).into_bytes());
        }
        // A rerun reuses every retained reply and scores them the same way.
        let again = run_with(dir.path(), &sender, stop.clone(), halt.clone()).await.unwrap();
        assert_eq!((model.calls(), again["new_calls"].clone()), (40, json!(0)));
        assert_eq!(again["results"], first["results"]);
        // An admission whose receipt was never written may have been billed:
        // it is unknown and never resent.
        fs::remove_file(dir.path().join("en_hms.receipt.json")).unwrap();
        let third = run_with(dir.path(), &sender, stop.clone(), halt.clone()).await.unwrap();
        assert_eq!(model.calls(), 40);
        assert_eq!(third["summary"]["unknown_cost_requests"], 1);
        assert!(third["acquisition_errors"][0].as_str().unwrap().starts_with("en_hms: en_hms was sent before"), "{}", third["acquisition_errors"]);
        // Stop before sending admits nothing.
        let fresh = tempfile::tempdir().unwrap();
        stop.store(true, Ordering::Release);
        let stopped = run_with(fresh.path(), &sender, stop, halt).await.unwrap();
        assert_eq!((model.calls(), stopped["new_calls"].clone()), (40, json!(0)));
        assert_eq!(fs::read_dir(fresh.path()).unwrap().count(), 0);
    }
}
