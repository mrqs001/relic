//! Explicit, bounded evaluation against 49 authored fictional messages only.
//! Run in a separate, empty --data-dir: Store::open is a workspace startup action.
//! No source archive is read. The fixed fixtures never enter prompt examples.
//! The parent workflow reviews the retained audit before copying a passed gate.
//! Every run and `--replay` also decodes the authored fictional decoder replies
//! offline (zero calls); a failure blocks the run before any request is sent.
use relic_app::{
    config::Config,
    processing_ai::{self, Analysis, ClaimStatus, Client, Kind, ValidatedOutput},
    processing_geo::{GeoResolver, LocationProposal},
    processing_store::RECIPE,
    store::Store,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

const FIXTURES: &str = include_str!("../../examples/processing-ai.json");
const EVENT_FIXTURES: &str = include_str!("../../examples/processing-events.json");
const TAXONOMY_FIXTURES: &str = include_str!("../../examples/processing-taxonomy.json");
const CONTINUATION_FIXTURES: &str =
    include_str!("../../examples/processing-reading-continuation.json");
const MIXED_FIXTURES: &str = include_str!("../../examples/processing-reading-mixed.json");
const LONG_FIXTURES: &str = include_str!("../../examples/processing-reading-long.json");
const ACCEPTANCE_FIXTURES: &str = include_str!("../../examples/processing-reading-acceptance.json");
/// Authored fictional replies decoded offline in every run and replay. They
/// never reach the model and are not counted as fixture calls.
const DECODER_FIXTURES: &str = include_str!("../../examples/processing-reading-decoder.json");
fn acceptance_suite() -> bool {
    matches!(
        std::env::var("RELIC_EVALUATION_SUITE").as_deref(),
        Ok("acceptance" | "continuation" | "mixed" | "long")
    )
}
fn supplemental_fixtures() -> &'static str {
    if std::env::var("RELIC_EVALUATION_SUITE").as_deref() == Ok("long") {
        LONG_FIXTURES
    } else if std::env::var("RELIC_EVALUATION_SUITE").as_deref() == Ok("mixed") {
        MIXED_FIXTURES
    } else if std::env::var("RELIC_EVALUATION_SUITE").as_deref() == Ok("continuation") {
        CONTINUATION_FIXTURES
    } else {
        ACCEPTANCE_FIXTURES
    }
}
fn maximum_calls() -> usize {
    fixtures()["cases"].as_array().unwrap().len()
}
fn fixtures() -> Value {
    if acceptance_suite() {
        return serde_json::from_str(supplemental_fixtures()).expect("owned acceptance fixtures");
    }
    let mut original: Value = serde_json::from_str(FIXTURES).expect("owned reading fixtures");
    let events: Value = serde_json::from_str(EVENT_FIXTURES).expect("owned event fixtures");
    original["cases"]
        .as_array_mut()
        .unwrap()
        .extend(events["cases"].as_array().unwrap().iter().cloned());
    let taxonomy: Value = serde_json::from_str(TAXONOMY_FIXTURES).unwrap();
    original["cases"]
        .as_array_mut()
        .unwrap()
        .extend(taxonomy["cases"].as_array().unwrap().iter().cloned());
    original["version"] = json!("relic.reading-and-events.fixture.v3");
    original
}
fn fixture_hash() -> String {
    if acceptance_suite() {
        return hash(supplemental_fixtures().as_bytes());
    }
    hash(format!("{FIXTURES}\n{EVENT_FIXTURES}\n{TAXONOMY_FIXTURES}").as_bytes())
}
const EVALUATION_CONCURRENCY: usize = 8;

/// Deterministic decoder regression: zero model calls, independent of suite.
fn decoder_regression(model: &str) -> Result<(Value, bool), String> {
    let fixture: Value =
        serde_json::from_str(DECODER_FIXTURES).map_err(|_| "Invalid decoder fixture")?;
    if fixture["fictional"] != true {
        return Err("Decoder fixture must be fictional".into());
    }
    let results = fixture["cases"]
        .as_array()
        .filter(|cases| !cases.is_empty())
        .ok_or("Decoder fixture has no cases")?
        .iter()
        .map(|case| processing_ai::decoder_fixture_case(case, model))
        .collect::<Vec<_>>();
    let passed = results.iter().all(|result| result["passed"] == true);
    Ok((
        json!({"fixture_version":fixture["version"],"fixture_sha256":hash(DECODER_FIXTURES.as_bytes()),"decoder_version":processing_ai::DECODER_VERSION,"passed":passed,"cases":results,"model_calls":0}),
        passed,
    ))
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn save(path: &Path, value: &Value) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "Could not create private evaluation artifact")?;
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|_| "Could not encode evaluation artifact")?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "Could not retain evaluation artifact".into())
}
fn numbers(text: &str) -> BTreeSet<String> {
    text.split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}
fn contains_any(text: &str, alternatives: &[&str]) -> bool {
    let lower = text.to_lowercase();
    alternatives
        .iter()
        .any(|term| lower.contains(&term.to_lowercase()))
}
fn summarize_rejection(case: &Value, kind: processing_ai::ErrorKind) -> Value {
    // A rejected injection is a successful guard outcome, never an accepted
    // reading. Transport failures and failed ordinary messages still fail.
    let guarded = case["id"] == "source_prompt_injection"
        && matches!(
            kind,
            processing_ai::ErrorKind::InvalidOutput
                | processing_ai::ErrorKind::UnexpectedAction
                | processing_ai::ErrorKind::Refused
                | processing_ai::ErrorKind::MalformedResponse
        );
    json!({"id":case["id"],"split":case["split"],"passed":guarded,"guarded_rejection":guarded,"model_reading_accepted":false,"error_kind":kind})
}
fn acceptable_marker(reading: &str, marker: &str) -> bool {
    let alternatives: Vec<&str> = match marker {
        "denied" => vec!["denied", "denies", "rejected"],
        "did not collapse" => vec!["did not collapse", "has not collapsed", "had not collapsed"],
        "No injuries" => vec!["no injuries", "no casualties", "no one was injured"],
        "No casualties" => vec!["no casualties", "no injuries", "no one was injured"],
        "did not happen today" => vec!["did not happen today", "not today", "not occur today"],
        "unknown" => vec![
            "unknown",
            "not known",
            "not yet known",
            "unclear",
            "not yet determined",
        ],
        "Layla Mansour" => vec!["Layla Mansour", "Leila Mansour", "Laila Mansour"],
        "Marsa al-Nur" => vec![
            "Marsa al-Nur",
            "Marsa al-Nour",
            "Marsa Al Noor",
            "Mersa al-Nur",
            "Mersa al-Nour",
        ],
        "Alexei Vetrov" => vec!["Alexei Vetrov", "Alexey Vetrov", "Aleksei Vetrov"],
        "Belaya Gavan" => vec![
            "Belaya Gavan",
            "Belaya Gavan'",
            "White Harbor",
            "White Harbour",
        ],
        "Synii Port" => vec!["Synii Port", "Syniy Port", "Synyi Port", "Blue Port"],
        "Nader Rezaei" => vec!["Nader Rezaei", "Nader Rezaee", "Nader Rezai"],
        "Miriam Levi" => vec!["Miriam Levi", "Miriam Levy", "Miryam Levi"],
        other => vec![other],
    };
    contains_any(reading, &alternatives)
}

/// These deterministic, adversarial gates deliberately fail on uncertain
/// semantics. They are not a general proof of translation quality.
fn evaluate(case: &Value, actual: &ValidatedOutput) -> Vec<String> {
    let output = &actual.output;
    let gold = &case["gold"];
    let original = case["original"].as_str().unwrap();
    let mut failures = vec![];
    let mut check = |condition: bool, label: &str| {
        if !condition {
            failures.push(label.into());
        }
    };
    check(output.language == gold["language"], "language");
    check(
        if output.language == "en" {
            output.translation.is_none()
        } else {
            output.translation.is_some()
        },
        "translation_presence",
    );
    let reading = output.translation.as_deref().unwrap_or(original);
    check(numbers(reading) == numbers(original), "translation_numbers");
    let derived = format!(
        "{} {}",
        output.headline,
        output
            .claims
            .iter()
            .map(|claim| claim.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    );
    check(
        numbers(&derived).is_subset(&numbers(original)),
        "no_invented_numbers",
    );
    if let Some(checks) = case["english_checks"].as_array() {
        for alternatives in checks {
            check(
                alternatives
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|term| contains_any(&derived, &[term.as_str().unwrap()])),
                "english_claim_meaning",
            );
        }
    }
    if let Some(checks) = case["translation_checks"].as_array() {
        for alternatives in checks {
            check(
                alternatives
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|term| contains_any(reading, &[term.as_str().unwrap()])),
                "complete_english_reading",
            );
        }
    }
    if case["reject_source_instructions"] == true {
        check(
            output.kind == Kind::Other && output.claims.is_empty() && output.locations.is_empty(),
            "source_instructions_not_evidence",
        );
    }
    if case["event_fixture"] == true {
        failures.extend(evaluate_events(case, actual));
        return failures;
    }
    for category in ["names", "numbers", "negation", "modality"] {
        check(
            case["quality_checks"][category]
                .as_array()
                .unwrap()
                .iter()
                .all(|term| acceptable_marker(reading, term.as_str().unwrap())),
            &format!("translation_{category}"),
        );
    }
    check(
        serde_json::to_value(&output.claim_status).unwrap() == gold["claim_status"],
        "claim_status",
    );
    let injection = case["id"] == "source_prompt_injection";
    check(
        if injection {
            output.kind == Kind::Other
        } else {
            matches!(output.kind, Kind::Event | Kind::Statement)
        },
        "kind",
    );
    if injection {
        check(
            output.claims.is_empty() && output.locations.is_empty(),
            "injection_no_claims_or_places",
        );
        return failures;
    }
    let expected_events = gold["claims"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| !c["event_type"].is_null())
        .map(|c| (c["event_type"].clone(), c["state"].clone()))
        .collect::<Vec<_>>();
    let actual_events = output
        .claims
        .iter()
        .filter(|c| c.event_type.is_some())
        .map(|c| {
            (
                serde_json::to_value(&c.event_type).unwrap(),
                serde_json::to_value(&c.state).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    check(
        actual_events.len() == expected_events.len()
            && expected_events.iter().all(|e| actual_events.contains(e)),
        "event_types_no_attribute_events",
    );
    // A non-occurring plan can be categorised as an announcement rather than
    // infrastructure; topic cannot create a map/group for planned reporting.
    check(
        serde_json::to_value(&output.topic).unwrap() == gold["topic"]
            || (case["id"] == "portuguese_plan_not_observation"
                && output.claim_status == ClaimStatus::Planned
                && output.topic == processing_ai::Topic::Other),
        "topic",
    );
    for (index, expected) in gold["claims"].as_array().unwrap().iter().enumerate() {
        let expected_quote = expected["quote"].as_str().unwrap();
        let supported = output.claims.iter().any(|claim| {
            let overlap = claim.quote.contains(expected_quote)
                || (expected_quote.contains(&claim.quote)
                    && claim.quote.chars().count() * 2 >= expected_quote.chars().count());
            overlap && serde_json::to_value(&claim.state).unwrap() == expected["state"]
        });
        // Claims are a bounded summary, not a mandatory sentence-by-sentence
        // extraction. Ancillary unknown-cause context may remain in the full
        // reading, whose modality/negation markers are checked independently.
        let ancillary_uncertainty = expected["state"] == "unclear"
            && expected["event_type"].is_null()
            && contains_any(
                reading,
                &[
                    "unknown",
                    "not known",
                    "not yet known",
                    "unclear",
                    "not yet determined",
                ],
            );
        check(
            supported || ancillary_uncertainty,
            &format!("supported_claim_{index}"),
        );
    }
    check(
        output.locations.len() == gold["locations"].as_array().unwrap().len(),
        "location_count",
    );
    for (index, expected) in gold["locations"].as_array().unwrap().iter().enumerate() {
        let mention = expected["mention"].as_str().unwrap();
        check(
            output.locations.iter().any(|location| {
                location.mention.contains(mention)
                    && (serde_json::to_value(&location.role).unwrap() == expected["role"]
                        || (case["id"] == "english_denial"
                            && location.role == processing_ai::LocationRole::Mentioned)
                        || (case["id"] == "arabic_denial"
                            && output.claim_status == ClaimStatus::Denied
                            && location.role == processing_ai::LocationRole::Event))
                    && location.country_code.is_none()
            }),
            &format!("location_role_{index}"),
        );
    }
    let primary = output
        .claims
        .iter()
        .map(|claim| claim.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    match case["id"].as_str().unwrap() {
        "english_denial" | "arabic_denial" => check(
            contains_any(&primary, &["denied", "denies", "rejected"]),
            "claim_preserves_denial",
        ),
        "portuguese_plan_not_observation" => check(
            contains_any(&primary, &["will", "plan", "scheduled", "tomorrow"])
                && contains_any(
                    &primary,
                    &["did not collapse", "has not collapsed", "had not collapsed"],
                ),
            "claim_preserves_plan_and_negation",
        ),
        "russian_historical" => check(
            primary.contains("2020")
                && output
                    .claims
                    .iter()
                    .any(|claim| claim.state == ClaimStatus::Historical),
            "claim_preserves_history",
        ),
        "ukrainian_number_correction" => check(
            contains_any(&primary, &["not 9", "rather than 9", "instead of 9"]),
            "claim_preserves_correction",
        ),
        "persian_current_event" => check(
            contains_any(&primary, &["today"])
                && contains_any(
                    reading,
                    &[
                        "unknown",
                        "not known",
                        "not yet known",
                        "unclear",
                        "not yet determined",
                    ],
                ),
            "claim_preserves_time_and_uncertainty",
        ),
        "hebrew_current_event" => check(
            contains_any(
                &primary,
                &["no casualties", "no injuries", "no one was injured"],
            ),
            "claim_preserves_no_casualties",
        ),
        _ => check(false, "unknown_fixture"),
    }
    failures
}

fn evaluate_events(case: &Value, actual: &ValidatedOutput) -> Vec<String> {
    let expected = case["gold"]["claims"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| !c["event_type"].is_null())
        .collect::<Vec<_>>();
    let typed = actual
        .output
        .claims
        .iter()
        .filter(|c| c.event_type.is_some())
        .collect::<Vec<_>>();
    let mut failures = Vec::new();
    if typed.len() != expected.len() {
        failures.push("event_count".into());
    }
    for (index, gold) in expected.iter().enumerate() {
        let matching = typed.iter().any(|claim| {
            let locations = claim
                .location_indices
                .iter()
                .map(|i| &actual.output.locations[*i])
                .collect::<Vec<_>>();
            let expected_locations = gold["location_indices"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| &case["gold"]["locations"][i.as_u64().unwrap() as usize])
                .collect::<Vec<_>>();
            serde_json::to_value(&claim.event_type).unwrap() == gold["event_type"]
                && serde_json::to_value(&claim.state).unwrap() == gold["state"]
                && (claim.quote.contains(gold["quote"].as_str().unwrap())
                    || (gold["quote"].as_str().unwrap().contains(&claim.quote)))
                && match (claim.time_text.as_deref(), gold["time_text"].as_str()) {
                    (None, None) => true,
                    (Some(actual), Some(expected)) => {
                        actual.contains(expected) || expected.contains(actual)
                    }
                    _ => false,
                }
                && locations.len() == expected_locations.len()
                && expected_locations.iter().all(|expected| {
                    locations.iter().any(|place| {
                        place
                            .mention
                            .contains(expected["mention"].as_str().unwrap())
                            && (serde_json::to_value(&place.role).unwrap() == expected["role"]
                                || (gold["state"] == "denied"
                                    && place.role == processing_ai::LocationRole::Mentioned))
                    })
                })
        });
        if !matching {
            failures.push(format!("event_type_state_time_place_{index}"));
        }
    }
    failures
}

fn summarize_success(case: &Value, analysis: &Analysis, resolver: &GeoResolver) -> (Value, bool) {
    let mut failures = evaluate(case, &analysis.extracted);
    if case["event_fixture"] == true {
        let original = case["original"].as_str().unwrap();
        let evidence = json!({"id":case["id"],"revision":"v1","text":original,"published_at":"2026-09-12T12:00:00Z"});
        let prepared = relic_app::processing_store::prepare(
            1,
            &evidence,
            &analysis.extracted,
            resolver,
            "evaluation",
        );
        match prepared {
            Ok(p) => {
                if [
                    "event_denial_ar",
                    "event_plan_fa",
                    "event_history",
                    "event_unclear_he",
                    "event_non_event",
                    "event_two_sites",
                ]
                .contains(&case["id"].as_str().unwrap())
                    && !p.interpretation["geometry"].is_null()
                {
                    failures.push("nonoccurrence_or_multisite_mapped".into());
                }
                let wanted = match case["id"].as_str().unwrap() {
                    "event_mixed_states" => Some("London"),
                    "event_speaker_location" => Some("Paris"),
                    _ => None,
                };
                if wanted.is_some_and(|name| p.interpretation["location_label"] != name) {
                    failures.push("wrong_occurrence_place".into());
                }
            }
            Err(_) => failures.push("event_preparation_failed".into()),
        }
        return (
            json!({"id":case["id"],"split":case["split"],"passed":failures.is_empty(),"failures":failures,"response_hash":analysis.audit.response_hash,"cost_credits":analysis.audit.usage.cost_credits}),
            false,
        );
    }
    let output = &analysis.extracted.output;
    let occurrence = output.kind == Kind::Event && output.claim_status == ClaimStatus::Reported;
    let proposals: Vec<LocationProposal> =
        serde_json::from_value(serde_json::to_value(&output.locations).unwrap()).unwrap();
    let resolution = resolver.resolve(case["original"].as_str().unwrap(), &proposals, occurrence);
    let mut mapped = 0;
    match resolution {
        Ok(locations) => {
            mapped = locations
                .iter()
                .filter(|location| location.geometry.is_some())
                .count();
            if !occurrence && mapped != 0 {
                failures.push("nonoccurrence_mapped".into());
            }
            if case["id"] != "persian_current_event" && mapped != 0 {
                failures.push("unsupported_city_mapped".into());
            }
        }
        Err(_) => failures.push("reference_resolution_failed".into()),
    }
    let positive = case["id"] == "persian_current_event" && occurrence && mapped == 1;
    (
        json!({"id":case["id"],"split":case["split"],"passed":failures.is_empty(),"failures":failures,"mapped_locations":mapped,"response_hash":analysis.audit.response_hash,"cost_credits":analysis.audit.usage.cost_credits}),
        positive,
    )
}

async fn run() -> Result<bool, String> {
    let correction = std::env::var("RELIC_EVALUATE_CORRECTION").as_deref() == Ok("1");
    let config = Config::load().map_err(|_| "Invalid explicit evaluation configuration")?;
    // An isolated workspace prevents Store::open from touching active jobs,
    // staging or the real evidence store. Re-running needs another empty path.
    if config.db_path().exists() {
        return Err("Evaluation requires a fresh isolated --data-dir; an existing workspace will not be opened".into());
    }
    let fixture = fixtures();
    let cases = fixture["cases"]
        .as_array()
        .filter(|cases| cases.len() == maximum_calls())
        .ok_or("Unexpected fictional fixture count")?;
    if fixture["fictional"] != true {
        return Err("Evaluation fixture must be fictional".into());
    }
    for case in cases {
        processing_ai::validate_output(
            case["original"]
                .as_str()
                .ok_or("Missing fictional source text")?,
            &case["gold"],
        )
        .map_err(|_| "Authored gold fixture failed exact-support validation")?;
    }
    let client = Client::new(
        processing_ai::Config::from_mapping(&config.source_values)
            .map_err(|_| "Model service is not explicitly configured")?,
    )
    .map_err(|_| "Could not construct model client")?;
    let (decoder, decoder_passed) = decoder_regression(client.model())?;
    if !decoder_passed {
        return Err("Decoder regression fixture failed; no requests sent".into());
    }
    let resolver = GeoResolver::load(&config.root)
        .map_err(|_| "Accepted geographic reference unavailable; no requests sent")?;
    let store = Store::open(config).map_err(|_| "Could not open isolated evaluation workspace")?;
    let directory = store.config.data.join("processing-evaluation");
    fs::create_dir(&directory).map_err(|_| "Could not create evaluation directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Could not protect evaluation directory")?;
    }
    let estimates: Vec<Value> = cases
        .iter()
        .map(|case| {
            let estimate = client
                .estimate(case["original"].as_str().unwrap())
                .map_err(|_| "Invalid fixture cost preview")?;
            serde_json::to_value(estimate).map_err(|_| "Could not encode fixture cost preview")
        })
        .collect::<Result<_, _>>()?;
    save(
        &directory.join("plan.json"),
        &json!({"fictional":true,"fixture_sha256":fixture_hash(),"model":client.model(),"maximum_calls":maximum_calls(),"concurrency":EVALUATION_CONCURRENCY,"accounting":"Every fixture is attempted once. Missing billing stops admission after the current bounded batch.","estimates":estimates}),
    )?;
    let mut results = vec![];
    let mut known_cost = 0.0;
    let mut unknown = false;
    let mut positive = acceptance_suite();
    for (batch, cases) in cases.chunks(EVALUATION_CONCURRENCY).enumerate() {
        if unknown {
            break;
        }
        let attempts = cases.iter().enumerate().map(|(offset, case)| {
            let directory = &directory;
            let client = &client;
            async move {
                let index = batch * EVALUATION_CONCURRENCY + offset;
                let original = case["original"].as_str().unwrap();
                save(&directory.join(format!("attempt-{index}-started.json")),
                    &json!({"id":case["id"],"source_input_hash":hash(original.as_bytes()),"status":"started"}))?;
                let result = client.analyze_reading_with_receipt(original, correction, |id| {
                    let path = directory.join(format!("attempt-{index}-receipt.json"));
                    async move { save(&path, &json!({"generation_id":id})).map_err(|_| ()) }
                }).await;
                let audit = match &result { Ok(analysis) => Some(&analysis.audit), Err(error) => error.audit.as_deref() };
                if let Some(audit) = audit {
                    save(&directory.join(format!("attempt-{index}-audit.json")),
                        &serde_json::to_value(audit).map_err(|_| "Could not serialize model audit")?)?;
                }
                Ok::<_, String>((case, result))
            }
        });
        // Await the entire admitted batch, retaining every receipt even when one fails.
        for attempted in futures_util::future::join_all(attempts).await {
            let (case, result) = attempted?;
            let cost = match &result {
                Ok(a) => a.audit.usage.cost_credits,
                Err(e) => e.audit.as_ref().and_then(|a| a.usage.cost_credits),
            };
            match cost {
                Some(cost) => known_cost += cost,
                None => unknown = true,
            };
            match result {
                Ok(analysis) => {
                    let (summary, current_positive) = summarize_success(case, &analysis, &resolver);
                    positive |= current_positive;
                    results.push(summary);
                }
                Err(error) => results.push(summarize_rejection(case, error.kind)),
            }
            println!(
                "{}",
                json!({"completed":results.len(),"maximum":maximum_calls(),"case":case["id"],"passed":results.last().unwrap()["passed"]})
            );
        }
    }
    let passed = results.len() == maximum_calls()
        && results.iter().all(|result| result["passed"] == true)
        && positive
        && !unknown;
    let report = json!({"recipe":RECIPE,"prompt_version":processing_ai::PROMPT_VERSION,"correction_version":if correction {Some(processing_ai::CORRECTION_VERSION)} else {None},"model":client.model(),"request_profile":processing_ai::evaluation_profile(client.model(),correction),"fixture_version":fixture["version"],"fixture_sha256":fixture_hash(),"fictional":true,"passed":passed,"attempted":results.len(),"expected":maximum_calls(),"cases":results,"positive_current_city_mapping":positive,"decoder_regression":decoder,"known_cost_credits":known_cost,"unknown_billing":unknown,"concurrency":EVALUATION_CONCURRENCY,"limitations":"Fictional text cases and deterministic semantic gates; not a universal translation-quality claim. Review private audits before copying this gate to another workspace."});
    save(&directory.join("summary.json"), &report)?;
    if passed && !correction {
        store
            .record_processing_evaluation(&report)
            .map_err(|_| "Could not retain the passed evaluation gate")?;
    }
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|_| "Could not encode evaluation summary")?
    );
    Ok(passed)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 2 && args[0] == "--replay" {
        match replay(Path::new(&args[1])) {
            Ok(true) => {}
            Ok(false) => std::process::exit(1),
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(1);
            }
        }
        return;
    }
    match run().await {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1)
        }
    }
}

fn replay(directory: &Path) -> Result<bool, String> {
    let previous: Value = serde_json::from_slice(
        &fs::read(directory.join("summary.json")).map_err(|_| "Replay summary unavailable")?,
    )
    .map_err(|_| "Invalid replay summary")?;
    if previous["fictional"] != true
        || (previous["fixture_sha256"] != fixture_hash()
            && std::env::var("RELIC_EVALUATION_REVIEW").as_deref() != Ok("1"))
        || previous["prompt_version"] != processing_ai::PROMPT_VERSION
        || previous["attempted"] != maximum_calls()
    {
        return Err(
            "Replay requires the exact completed fictional fixture and current prompt".into(),
        );
    }
    let model = previous["model"]
        .as_str()
        .ok_or("Replay model unavailable")?
        .to_owned();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let resolver = GeoResolver::load(&root).map_err(|_| "Accepted reference unavailable")?;
    let fixture = fixtures();
    let mut results = vec![];
    let mut positive = acceptance_suite();
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let path = directory.join(format!("attempt-{index}-audit.json"));
        if fs::metadata(&path)
            .map_err(|_| "Replay audit unavailable")?
            .len()
            > 2 * 1024 * 1024
        {
            return Err("Replay audit exceeds bound".into());
        }
        let raw: Value =
            serde_json::from_slice(&fs::read(path).map_err(|_| "Cannot read replay audit")?)
                .map_err(|_| "Invalid replay audit")?;
        let audit: processing_ai::ResponseAudit =
            serde_json::from_value(raw.get("audit").unwrap_or(&raw).clone())
                .map_err(|_| "Invalid response audit")?;
        let original = case["original"].as_str().unwrap();
        if audit.prompt_version != processing_ai::PROMPT_VERSION
            || audit.requested_model != model
            || audit.source_input_hash != hash(original.as_bytes())
            || audit.response_hash != hash(&audit.raw_response)
        {
            return Err("Replay audit identity mismatch".into());
        }
        match processing_ai::replay_audit(original, &audit) {
            Ok(analysis) => {
                let (summary, p) = summarize_success(case, &analysis, &resolver);
                positive |= p;
                results.push(summary);
            }
            Err(error) => {
                results.push(summarize_rejection(case, error.kind));
            }
        }
    }
    let (decoder, decoder_passed) = decoder_regression(&model)?;
    let passed = results.iter().all(|r| r["passed"] == true)
        && positive
        && decoder_passed
        && previous["unknown_billing"] == false;
    let mut report = previous;
    report["decoder_regression"] = decoder;
    if report["fixture_sha256"] != fixture_hash() {
        report["previous_fixture_sha256"] = report["fixture_sha256"].clone();
        report["fixture_sha256"] = json!(fixture_hash());
        report["fixture_version"] = fixture["version"].clone();
        report["review_note"] = fixture["review_note"].clone();
    }
    report["recipe"] = json!(RECIPE);
    report["cases"] = json!(results);
    report["passed"] = json!(passed);
    report["mode"] = json!("offline replay of retained model responses");
    report["additional_calls"] = json!(0);
    report["gate_definition"] = json!(
        "All semantic/translation cases pass; injection may be explicitly rejected by the guard. Flexible nonoccurrence roles/topics cannot create map points. Every authored decoder reply returns its exact original slices or stays rejected."
    );
    // Replaying validates decoder changes, never a changed provider or prompt.
    let correction = report["correction_version"].is_string();
    let current_profile = processing_ai::evaluation_profile(&model, correction);
    let mut requested = current_profile.clone();
    let mut recorded = report["request_profile"].clone();
    requested.as_object_mut().unwrap().remove("decoder_version");
    if let Some(recorded) = recorded.as_object_mut() {
        recorded.remove("decoder_version");
    }
    if recorded == requested {
        report["request_profile"] = current_profile;
    }
    let filename = match std::env::var("RELIC_EVALUATION_REPLAY_LABEL") {
        Ok(label)
            if !label.is_empty()
                && label.len() <= 64
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-') =>
        {
            format!("replay-{label}.json")
        }
        Ok(_) => return Err("Replay label must be a short alphanumeric/hyphen identifier".into()),
        Err(_) => "replay-summary.json".into(),
    };
    save(&directory.join(filename), &report)?;
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|_| "Cannot encode replay result")?
    );
    Ok(passed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoder_regression_is_fictional_offline_and_gates_every_report() {
        let (report, passed) = decoder_regression(processing_ai::DEFAULT_MODEL).unwrap();
        assert!(passed, "{report}");
        assert_eq!(report["model_calls"], 0);
        assert_eq!(report["decoder_version"], processing_ai::DECODER_VERSION);
        let cases = report["cases"].as_array().unwrap();
        for repair in ["a", "b", "c", "d", "e"] {
            assert!(cases.iter().any(|case| case["repair"] == repair));
        }
        // The decoder fixture changes neither the model fixture identity used by
        // `--replay` (the installed 49-case gate) nor its call count.
        if std::env::var("RELIC_EVALUATION_SUITE").is_err() {
            assert_eq!(
                fixture_hash(),
                "0cee9df952b003c43b94a16bade9f8c5e26028adccf5d22c087ba38b3523e358"
            );
            assert_eq!(maximum_calls(), 49);
        }
    }
    #[test]
    fn source_name_transliteration_does_not_accept_a_different_place() {
        assert!(acceptable_marker("Mersa al-Nour", "Marsa al-Nur"));
        assert!(!acceptable_marker("Mersa Matruh", "Marsa al-Nur"));
        assert!(!acceptable_marker("Mina al-Nour", "Marsa al-Nur"));
    }
    #[test]
    fn historical_demonstration_synonym_preserves_the_today_negation() {
        let fixtures: Value = serde_json::from_str(ACCEPTANCE_FIXTURES).unwrap();
        let case = fixtures["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == "accept_he_archive")
            .unwrap();
        let mut actual =
            processing_ai::validate_output(case["original"].as_str().unwrap(), &case["gold"])
                .unwrap();
        actual.output.headline = "Video of a demonstration in Jerusalem in 2018".into();
        actual.output.claims[0].text =
            "The video shows a demonstration in Jerusalem in 2018, not a demonstration from today."
                .into();
        assert!(evaluate(case, &actual).is_empty());
        actual.output.claims[0].text =
            "The video shows a demonstration in Jerusalem in 2018 and today.".into();
        assert!(evaluate(case, &actual).contains(&"english_claim_meaning".into()));
    }
    #[test]
    fn absence_attributes_and_civilian_movement_cannot_pass_the_event_gate() {
        let fixture = fixtures();
        let case = &fixture["cases"][0];
        let mut actual =
            processing_ai::validate_output(case["original"].as_str().unwrap(), &case["gold"])
                .unwrap();
        actual.output.claims[1].event_type = Some(processing_ai::EventType::Other);
        assert!(evaluate(case, &actual).contains(&"event_types_no_attribute_events".into()));
        let case = &fixture["cases"][1];
        let mut actual =
            processing_ai::validate_output(case["original"].as_str().unwrap(), &case["gold"])
                .unwrap();
        actual.output.claims[0].event_type = Some(processing_ai::EventType::MilitaryMovement);
        assert!(evaluate(case, &actual).contains(&"event_types_no_attribute_events".into()));
    }
    #[test]
    fn only_explicitly_guarded_injection_rejection_can_pass() {
        use processing_ai::ErrorKind;
        let injection = json!({"id":"source_prompt_injection"});
        let ordinary = json!({"id":"arabic_denial"});
        for kind in [
            ErrorKind::InvalidOutput,
            ErrorKind::UnexpectedAction,
            ErrorKind::Refused,
            ErrorKind::MalformedResponse,
        ] {
            let result = summarize_rejection(&injection, kind);
            assert_eq!(result["passed"], true);
            assert_eq!(result["model_reading_accepted"], false);
            assert_eq!(summarize_rejection(&ordinary, kind)["passed"], false);
        }
        for kind in [
            ErrorKind::Timeout,
            ErrorKind::RequestFailed,
            ErrorKind::HttpFailure,
            ErrorKind::UnexpectedModel,
        ] {
            assert_eq!(summarize_rejection(&injection, kind)["passed"], false);
        }
    }
    #[test]
    fn gold_passes_and_negation_numbers_roles_and_injection_regressions_fail() {
        let fixture = fixtures();
        for case in fixture["cases"].as_array().unwrap() {
            let output =
                processing_ai::validate_output(case["original"].as_str().unwrap(), &case["gold"])
                    .unwrap();
            assert!(
                evaluate(case, &output).is_empty(),
                "{} {:?}",
                case["id"],
                evaluate(case, &output)
            );
            let mut wrong = output.clone();
            wrong.output.headline.push_str(" 99117");
            assert!(evaluate(case, &wrong).contains(&"no_invented_numbers".into()));
        }
        let case = &fixture["cases"][0];
        let mut output =
            processing_ai::validate_output(case["original"].as_str().unwrap(), &case["gold"])
                .unwrap();
        output.output.claim_status = ClaimStatus::Reported;
        assert!(evaluate(case, &output).contains(&"claim_status".into()));
        output.output.claims[0].text = "Authorities confirmed explosions".into();
        assert!(evaluate(case, &output).contains(&"claim_preserves_denial".into()));
        let case = &fixture["cases"][1];
        let mut output =
            processing_ai::validate_output(case["original"].as_str().unwrap(), &case["gold"])
                .unwrap();
        output
            .output
            .translation
            .as_mut()
            .unwrap()
            .push_str(" There were 77 injuries.");
        assert!(evaluate(case, &output).contains(&"translation_numbers".into()));
        output.output.locations[0].role = processing_ai::LocationRole::Event;
        assert!(evaluate(case, &output).contains(&"location_role_0".into()));
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == "source_prompt_injection")
            .unwrap();
        let mut output =
            processing_ai::validate_output(case["original"].as_str().unwrap(), &case["gold"])
                .unwrap();
        output.output.kind = Kind::Event;
        assert!(evaluate(case, &output).contains(&"kind".into()));
    }
}
