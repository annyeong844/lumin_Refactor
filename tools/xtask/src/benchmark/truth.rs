use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use lumin_model::RepoPath;
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SemanticDump {
    tuple_to_finding_id: BTreeMap<String, String>,
}

#[derive(Clone, Copy)]
pub(super) enum Scope<'a> {
    Run { run_id: &'a str },
    Gate { gate_id: &'a str, revision: u64 },
}

impl SemanticDump {
    pub(super) fn sha256(&self) -> Result<String, String> {
        let bytes = serde_json::to_vec(&self.tuple_to_finding_id)
            .map_err(|error| format!("cannot encode semantic dump: {error}"))?;
        Ok(super::sha256_hex(&bytes))
    }

    pub(super) fn report_value(&self) -> Result<Value, String> {
        let mappings = self
            .tuple_to_finding_id
            .iter()
            .map(|(tuple, finding_id)| {
                let tuple = serde_json::from_str::<Value>(tuple)
                    .map_err(|error| format!("cannot decode authored tuple: {error}"))?;
                Ok(serde_json::json!({
                    "tuple": tuple,
                    "findingId": finding_id,
                }))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(serde_json::json!({
            "schemaVersion": "phase1-scale-finding-id-map.v1",
            "mappingCount": mappings.len(),
            "mappings": mappings,
            "sha256": self.sha256()?,
        }))
    }
}

pub(super) fn audit_scope(response: &Value) -> Result<(String, u64), String> {
    require_equal_string(response, "/schemaVersion", "lumin.audit.v2")?;
    require_equal_string(response, "/status", "complete")?;
    require_equal_u64(response, "/findingCount", 256)?;
    require_equal_u64(response, "/limitationCount", 0)?;
    Ok((
        required_string(response, "/runId")?.to_owned(),
        required_u64(response, "/sequence")?,
    ))
}

pub(super) fn gate_scope(
    response: &Value,
    expected_observation: &str,
) -> Result<(String, u64), String> {
    require_equal_string(response, "/schemaVersion", "lumin.gate-mutation.v2")?;
    require_equal_string(
        response,
        "/observationBinding/observation/kind",
        expected_observation,
    )?;
    let signals = response
        .pointer("/signals")
        .and_then(Value::as_array)
        .ok_or_else(|| "benchmark gate response omitted signals".to_owned())?;
    let deltas = response
        .pointer("/deltas")
        .and_then(Value::as_array)
        .ok_or_else(|| "benchmark gate response omitted deltas".to_owned())?;
    let (lifecycle, signal_kind) = match expected_observation {
        "baseline" => {
            if !deltas.is_empty() {
                return Err("benchmark pre-write response unexpectedly contains deltas".to_owned());
            }
            ("active", "finding-warnings")
        }
        "close" => {
            if deltas.len() != 256
                || deltas.iter().any(|delta| {
                    delta.pointer("/key/family").and_then(Value::as_str) != Some("dead-export")
                        || delta
                            .pointer("/classification/kind")
                            .and_then(Value::as_str)
                            != Some("unchanged")
                })
            {
                return Err(
                    "benchmark post-write response omitted exact unchanged finding deltas"
                        .to_owned(),
                );
            }
            ("closed", "pre-existing-adverse-facts")
        }
        other => return Err(format!("unsupported benchmark gate observation {other}")),
    };
    require_equal_string(response, "/decision", "allow-with-warnings")?;
    require_equal_string(response, "/lifecycle", lifecycle)?;
    if signals.len() != 1 {
        return Err("benchmark gate response must contain exactly one warning signal".to_owned());
    }
    require_equal_string(&signals[0], "/kind", signal_kind)?;
    require_equal_u64(&signals[0], "/count", 256)?;
    Ok((
        required_string(response, "/gateId")?.to_owned(),
        required_u64(response, "/revision")?,
    ))
}

pub(super) fn validate_semantic_dump(
    binary: &Path,
    root: &Path,
    truth: &Value,
    scope: Scope<'_>,
    capture: &Path,
) -> Result<SemanticDump, String> {
    let expected = expected_findings(truth)?;
    let mut observed_items = Vec::new();
    let mut cursor = None::<String>;
    let mut cursors = BTreeSet::new();
    loop {
        let mut arguments = match scope {
            Scope::Run { run_id } => vec![
                "findings".into(),
                "--run".into(),
                run_id.into(),
                "--area".into(),
                "dead-code".into(),
                "--format".into(),
                "json".into(),
            ],
            Scope::Gate { gate_id, revision } => vec![
                "gate".into(),
                "findings".into(),
                gate_id.into(),
                "--revision".into(),
                revision.to_string().into(),
                "--format".into(),
                "json".into(),
            ],
        };
        if let Some(value) = &cursor {
            arguments.push("--cursor".into());
            arguments.push(value.into());
        }
        let page = super::measurement::run_query(
            binary,
            root,
            &arguments,
            &capture.join(format!("findings-page-{}", cursors.len())),
        )?;
        validate_page_header(&page, &scope, expected.len())?;
        let items = page
            .pointer("/items")
            .and_then(Value::as_array)
            .ok_or_else(|| "benchmark findings page omitted items".to_owned())?;
        require_equal_u64(&page, "/returned", items.len() as u64)?;
        observed_items.extend(items.iter().cloned());
        let next = page.pointer("/nextCursor");
        match next {
            Some(Value::String(value)) => {
                if !cursors.insert(value.clone()) {
                    return Err("benchmark findings pagination repeated a cursor".to_owned());
                }
                if page.pointer("/truncated").and_then(Value::as_bool) != Some(true) {
                    return Err("benchmark findings cursor was not marked truncated".to_owned());
                }
                cursor = Some(value.clone());
            }
            Some(Value::Null) => {
                if page.pointer("/truncated").and_then(Value::as_bool) != Some(false) {
                    return Err("final benchmark findings page was marked truncated".to_owned());
                }
                break;
            }
            _ => return Err("benchmark findings page omitted nextCursor".to_owned()),
        }
    }
    if observed_items.len() != expected.len() {
        return Err(format!(
            "benchmark findings returned {} rows; expected {}",
            observed_items.len(),
            expected.len()
        ));
    }

    let expected_by_identity = expected
        .iter()
        .map(|item| Ok((finding_identity(item)?, *item)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    if expected_by_identity.len() != expected.len() {
        return Err("authored scale truth contains duplicate finding identities".to_owned());
    }
    let mut tuple_to_finding_id = BTreeMap::new();
    let mut observed_identities = BTreeSet::new();
    let mut previous_order_key = None;
    for item in &observed_items {
        let identity = (
            required_string(item, "/path/display")?.to_owned(),
            required_string(item, "/exportedName")?.to_owned(),
        );
        let expected_item = expected_by_identity.get(&identity).ok_or_else(|| {
            format!(
                "public finding is absent from authored truth: {}/{}",
                identity.0, identity.1
            )
        })?;
        let tuple = validate_finding(item, expected_item)?;
        let finding_id = required_string(item, "/findingId")?.to_owned();
        if tuple_to_finding_id.insert(tuple, finding_id).is_some() {
            return Err("two public findings mapped to one authored semantic tuple".to_owned());
        }
        if !observed_identities.insert(identity) {
            return Err("public findings repeated an authored finding identity".to_owned());
        }
        let order_key = finding_order_key(item)?;
        if previous_order_key
            .as_ref()
            .is_some_and(|previous| previous >= &order_key)
        {
            return Err("public findings violate findings.v1 canonical order".to_owned());
        }
        previous_order_key = Some(order_key);
    }
    if observed_identities != expected_by_identity.keys().cloned().collect() {
        return Err("public findings do not cover the complete authored truth".to_owned());
    }
    let finding_ids = tuple_to_finding_id.values().collect::<BTreeSet<_>>();
    if finding_ids.len() != tuple_to_finding_id.len() {
        return Err("one finding ID was assigned to multiple authored tuples".to_owned());
    }

    if let Scope::Run { run_id } = scope {
        let overview = super::measurement::run_query(
            binary,
            root,
            &[
                "overview".into(),
                "--run".into(),
                run_id.into(),
                "--format".into(),
                "json".into(),
            ],
            &capture.join("overview"),
        )?;
        require_equal_string(&overview, "/schemaVersion", "lumin.overview.v2")?;
        require_equal_u64(&overview, "/findingCount", 256)?;
        require_equal_u64(&overview, "/limitationCount", 0)?;
        if overview
            .pointer("/limitations")
            .and_then(Value::as_array)
            .map(Vec::len)
            != Some(0)
        {
            return Err("benchmark run overview contains limitations".to_owned());
        }
    }

    Ok(SemanticDump {
        tuple_to_finding_id,
    })
}

fn finding_identity(value: &Value) -> Result<(String, String), String> {
    Ok((
        required_string(value, "/path")?.to_owned(),
        required_string(value, "/exportName")?.to_owned(),
    ))
}

fn finding_order_key(value: &Value) -> Result<(String, Vec<u8>, u64, u64, String), String> {
    let canonical = STANDARD
        .decode(required_string(value, "/path/canonicalBase64")?)
        .map_err(|error| format!("finding path has invalid canonicalBase64: {error}"))?;
    Ok((
        required_string(value, "/ruleId")?.to_owned(),
        canonical,
        required_u64(value, "/span/start")?,
        required_u64(value, "/span/end")?,
        required_string(value, "/findingId")?.to_owned(),
    ))
}

fn validate_page_header(page: &Value, scope: &Scope<'_>, expected: usize) -> Result<(), String> {
    require_equal_string(page, "/schemaVersion", "lumin.collection.v1")?;
    require_equal_string(page, "/ordering", "findings.v1")?;
    require_equal_u64(page, "/scopeTotal", expected as u64)?;
    require_equal_u64(page, "/total", expected as u64)?;
    if page
        .pointer("/filters")
        .and_then(Value::as_object)
        .map(serde_json::Map::len)
        != Some(0)
    {
        return Err("benchmark findings query applied a filter".to_owned());
    }
    match scope {
        Scope::Run { run_id } => {
            require_equal_string(page, "/scope/kind", "run")?;
            require_equal_string(page, "/scope/id", run_id)?;
        }
        Scope::Gate { gate_id, revision } => {
            require_equal_string(page, "/scope/kind", "gate-attempt")?;
            require_equal_string(page, "/scope/gateId", gate_id)?;
            require_equal_u64(page, "/scope/revision", *revision)?;
        }
    }
    Ok(())
}

fn expected_findings(truth: &Value) -> Result<Vec<&Value>, String> {
    let expected = truth
        .pointer("/expectedFindings")
        .and_then(Value::as_array)
        .ok_or_else(|| "scale truth omitted expectedFindings".to_owned())?;
    if expected.len() != 256 {
        return Err("scale truth must contain exactly 256 findings".to_owned());
    }
    Ok(expected.iter().collect())
}

fn validate_finding(item: &Value, expected: &Value) -> Result<String, String> {
    for (pointer, value) in [
        ("/ruleId", "dead-code/zero-exact-fan-in.v1"),
        ("/ownerCapability", "dead-code.v1"),
        ("/severity", "warning"),
        ("/confidence", "grounded"),
        ("/namespace", "value"),
        ("/path/encoding", "repo-path.v1"),
    ] {
        require_equal_string(item, pointer, value)?;
    }
    let path = required_string(expected, "/path")?;
    let export_name = required_string(expected, "/exportName")?;
    require_equal_string(item, "/path/display", path)?;
    require_equal_string(item, "/exportedName", export_name)?;
    let canonical_path = RepoPath::from_portable(path)
        .map_err(|error| format!("authored scale path is invalid: {path}: {error}"))?;
    let expected_base64 = STANDARD.encode(canonical_path.canonical_bytes());
    require_equal_string(item, "/path/canonicalBase64", &expected_base64)?;
    let expected_claim = format!("export `{export_name}` has zero grounded exact fan-in");
    require_equal_string(item, "/claim", &expected_claim)?;

    let disposition = required_string(expected, "/disposition")?;
    let reason = expected.pointer("/dispositionReason");
    match (disposition, reason) {
        ("ReviewCandidate", Some(Value::Null)) => {
            require_equal_string(item, "/disposition/kind", "review-candidate")?;
            if item.pointer("/disposition/reason").is_some() {
                return Err("review-candidate finding unexpectedly has a reason".to_owned());
            }
        }
        ("ReviewOnly", Some(reason)) => {
            require_equal_string(item, "/disposition/kind", "review-only")?;
            let (role, public_reason, classification_reason) =
                match required_string(reason, "/role")? {
                    "Generated" => (
                        "Generated",
                        "generated-source",
                        "leading-comment-@generated-within-first-2KiB",
                    ),
                    "Vendored" => ("Vendored", "vendored-source", "explicit-vendor-role"),
                    other => {
                        return Err(format!("scale truth has unsupported source role {other}"));
                    }
                };
            require_equal_string(item, "/disposition/reason", public_reason)?;
            require_equal_string(reason, "/role", role)?;
            require_equal_string(reason, "/classificationReason", classification_reason)?;
            require_equal_string(reason, "/classificationVersion", "source-classification.v1")?;
        }
        _ => return Err("scale truth has an invalid disposition shape".to_owned()),
    }
    require_equal_string(expected, "/exportKind", "named-value")?;
    require_equal_string(expected, "/findingClass", "grounded-zero-fan-in-export")?;
    let package = required_string(expected, "/packageName")?;
    let package_segment = path
        .split('/')
        .nth(1)
        .ok_or_else(|| format!("scale finding path has no package segment: {path}"))?;
    if package != format!("@lumin-scale/{package_segment}") {
        return Err(format!(
            "scale finding package identity disagrees with path {path}"
        ));
    }
    semantic_tuple(expected)
}

fn semantic_tuple(value: &Value) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| format!("cannot encode finding tuple: {error}"))
}

fn required_string<'a>(value: &'a Value, pointer: &str) -> Result<&'a str, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("JSON response omitted string {pointer}"))
}

fn required_u64(value: &Value, pointer: &str) -> Result<u64, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("JSON response omitted integer {pointer}"))
}

fn require_equal_string(value: &Value, pointer: &str, expected: &str) -> Result<(), String> {
    let observed = required_string(value, pointer)?;
    if observed != expected {
        return Err(format!(
            "JSON field {pointer} was {observed:?}; expected {expected:?}"
        ));
    }
    Ok(())
}

fn require_equal_u64(value: &Value, pointer: &str, expected: u64) -> Result<(), String> {
    let observed = required_u64(value, pointer)?;
    if observed != expected {
        return Err(format!(
            "JSON field {pointer} was {observed}; expected {expected}"
        ));
    }
    Ok(())
}
