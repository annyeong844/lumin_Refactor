//! Checked-in configuration-table generation and drift verification.
//!
//! The reviewed JSON artifacts are authoring inputs. Production crates consume
//! only the typed Rust tables rendered here; they never parse repository source
//! files or the artifact JSON at runtime.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

mod render;

#[cfg(test)]
mod tests;

const RESOLVER_SPEC: &str = "specs/resolver-config-semantics.v1.json";
const INVENTORY_SPEC: &str = "specs/inventory-config-semantics.v1.json";
const RESOLVER_OUTPUT: &str = "crates/graph/resolve/src/generated_config_policy.rs";
const INVENTORY_OUTPUT: &str = "crates/source/inventory/src/generated_config_policy.rs";

const RESOLVER_SCHEMA: &str = "resolver-config-semantics.v1";
const INVENTORY_SCHEMA: &str = "inventory-config-semantics.v1";
const RESOLVER_TOP_LEVEL_COUNT: usize = 21;
const RESOLVER_COMPILER_OPTION_COUNT: usize = 122;
const RESOLVER_PACKAGE_FIELD_COUNT: usize = 12;
const INVENTORY_PACKAGE_FIELD_COUNT: usize = 7;
const INVENTORY_PNPM_FIELD_COUNT: usize = 4;

#[derive(Default)]
pub(crate) struct GeneratedTableResult {
    pub violations: Vec<String>,
    pub tool_errors: Vec<String>,
}

struct ExpectedFile {
    relative_path: &'static str,
    content: String,
}

#[derive(Clone, Debug)]
struct PolicyRow {
    path: String,
    shape: String,
    classification: String,
    reason: Option<String>,
    limitation: Option<String>,
    rule: Option<String>,
    shape_mismatch_limitation: Option<String>,
    applies_when: Option<String>,
}

struct Artifacts {
    resolver_value: Value,
    inventory_value: Value,
    resolver_source_sha256: String,
    inventory_source_sha256: String,
    resolver_top_level: Vec<PolicyRow>,
    resolver_compiler_options: Vec<PolicyRow>,
    resolver_package_fields: Vec<PolicyRow>,
    resolver_inventory_owned: Vec<String>,
    inventory_package_fields: Vec<PolicyRow>,
    inventory_pnpm_fields: Vec<PolicyRow>,
    inventory_resolver_owned: Vec<String>,
}

pub(crate) fn check_generated_tables(workspace_root: &Path) -> GeneratedTableResult {
    let mut result = GeneratedTableResult::default();
    let artifacts = match load_artifacts(workspace_root) {
        Ok(artifacts) => artifacts,
        Err(error) => {
            result.tool_errors.push(error);
            return result;
        }
    };
    result.violations.extend(validate_artifacts(&artifacts));
    if !result.violations.is_empty() {
        return result;
    }
    let expected = match render::expected_files(&artifacts) {
        Ok(expected) => expected,
        Err(error) => {
            result.tool_errors.push(error);
            return result;
        }
    };
    for file in expected {
        compare_expected_file(workspace_root, &file, &mut result);
    }
    result
}

pub(crate) fn write_generated_tables(workspace_root: &Path) -> Result<Vec<PathBuf>, String> {
    let artifacts = load_artifacts(workspace_root)?;
    let violations = validate_artifacts(&artifacts);
    if !violations.is_empty() {
        return Err(format!(
            "configuration artifacts violate their generated-table contract:\n{}",
            violations.join("\n")
        ));
    }
    let expected = render::expected_files(&artifacts)?;
    let mut written = Vec::new();
    for file in expected {
        let path = workspace_path(workspace_root, file.relative_path);
        std::fs::write(&path, file.content.as_bytes())
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

fn load_artifacts(workspace_root: &Path) -> Result<Artifacts, String> {
    let resolver_bytes = read_artifact(workspace_root, RESOLVER_SPEC)?;
    let inventory_bytes = read_artifact(workspace_root, INVENTORY_SPEC)?;
    let resolver_value = serde_json::from_slice::<Value>(&resolver_bytes)
        .map_err(|error| format!("cannot parse {RESOLVER_SPEC}: {error}"))?;
    let inventory_value = serde_json::from_slice::<Value>(&inventory_bytes)
        .map_err(|error| format!("cannot parse {INVENTORY_SPEC}: {error}"))?;
    extract_artifacts(
        resolver_value,
        inventory_value,
        sha256_hex(&resolver_bytes),
        sha256_hex(&inventory_bytes),
    )
}

fn read_artifact(workspace_root: &Path, relative_path: &str) -> Result<Vec<u8>, String> {
    let path = workspace_path(workspace_root, relative_path);
    std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn extract_artifacts(
    resolver_value: Value,
    inventory_value: Value,
    resolver_source_sha256: String,
    inventory_source_sha256: String,
) -> Result<Artifacts, String> {
    let resolver_top_level = rows_from_array(
        value_at(&resolver_value, "/tsconfigTopLevel", RESOLVER_SPEC)?,
        "resolver tsconfigTopLevel",
    )?;
    let resolver_compiler_options = rows_from_object(
        value_at(&resolver_value, "/compilerOptions", RESOLVER_SPEC)?,
        "resolver compilerOptions",
    )?;
    let resolver_package_fields = rows_from_array(
        value_at(&resolver_value, "/packageJson/fields", RESOLVER_SPEC)?,
        "resolver packageJson.fields",
    )?;
    let resolver_inventory_owned = strings_from_array(
        value_at(
            &resolver_value,
            "/packageJson/inventoryOwnedFields",
            RESOLVER_SPEC,
        )?,
        "resolver packageJson.inventoryOwnedFields",
    )?;
    let inventory_package_fields = rows_from_array(
        value_at(&inventory_value, "/packageJson/fields", INVENTORY_SPEC)?,
        "inventory packageJson.fields",
    )?;
    let inventory_pnpm_fields = rows_from_array(
        value_at(
            &inventory_value,
            "/pnpmWorkspaceYaml/fields",
            INVENTORY_SPEC,
        )?,
        "inventory pnpmWorkspaceYaml.fields",
    )?;
    let inventory_resolver_owned = strings_from_array(
        value_at(
            &inventory_value,
            "/packageJson/resolverOwnedFields",
            INVENTORY_SPEC,
        )?,
        "inventory packageJson.resolverOwnedFields",
    )?;
    Ok(Artifacts {
        resolver_value,
        inventory_value,
        resolver_source_sha256,
        inventory_source_sha256,
        resolver_top_level,
        resolver_compiler_options,
        resolver_package_fields,
        resolver_inventory_owned,
        inventory_package_fields,
        inventory_pnpm_fields,
        inventory_resolver_owned,
    })
}

fn value_at<'a>(root: &'a Value, pointer: &str, artifact: &str) -> Result<&'a Value, String> {
    root.pointer(pointer)
        .ok_or_else(|| format!("{artifact} is missing {pointer}"))
}

fn rows_from_array(value: &Value, context: &str) -> Result<Vec<PolicyRow>, String> {
    let rows = value
        .as_array()
        .ok_or_else(|| format!("{context} must be an array"))?;
    rows.iter()
        .enumerate()
        .map(|(index, row)| policy_row(row, None, &format!("{context}[{index}]")))
        .collect()
}

fn rows_from_object(value: &Value, context: &str) -> Result<Vec<PolicyRow>, String> {
    let rows = value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object"))?;
    rows.iter()
        .map(|(path, row)| policy_row(row, Some(path), &format!("{context}.{path}")))
        .collect()
}

fn policy_row(value: &Value, path: Option<&str>, context: &str) -> Result<PolicyRow, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object"))?;
    let path = match path {
        Some(path) => path.to_owned(),
        None => required_string(object.get("path"), "path", context)?,
    };
    Ok(PolicyRow {
        path,
        shape: required_string(object.get("shape"), "shape", context)?,
        classification: required_string(object.get("classification"), "classification", context)?,
        reason: optional_string(object.get("reason"), "reason", context)?,
        limitation: optional_string(object.get("limitation"), "limitation", context)?,
        rule: optional_string(object.get("rule"), "rule", context)?,
        shape_mismatch_limitation: optional_string(
            object.get("shapeMismatchLimitation"),
            "shapeMismatchLimitation",
            context,
        )?,
        applies_when: optional_string(object.get("appliesWhen"), "appliesWhen", context)?,
    })
}

fn required_string(value: Option<&Value>, field: &str, context: &str) -> Result<String, String> {
    value
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{context}.{field} must be a string"))
}

fn optional_string(
    value: Option<&Value>,
    field: &str,
    context: &str,
) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| format!("{context}.{field} must be a string when present")),
    }
}

fn strings_from_array(value: &Value, context: &str) -> Result<Vec<String>, String> {
    value
        .as_array()
        .ok_or_else(|| format!("{context} must be an array"))?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{context}[{index}] must be a string"))
        })
        .collect()
}

fn validate_artifacts(artifacts: &Artifacts) -> Vec<String> {
    let mut violations = Vec::new();
    expect_string(
        &artifacts.resolver_value,
        "/schemaVersion",
        RESOLVER_SCHEMA,
        RESOLVER_SPEC,
        &mut violations,
    );
    expect_string(
        &artifacts.inventory_value,
        "/schemaVersion",
        INVENTORY_SCHEMA,
        INVENTORY_SPEC,
        &mut violations,
    );
    expect_count(
        "resolver tsconfigTopLevel",
        artifacts.resolver_top_level.len(),
        RESOLVER_TOP_LEVEL_COUNT,
        &mut violations,
    );
    expect_count(
        "resolver compilerOptions",
        artifacts.resolver_compiler_options.len(),
        RESOLVER_COMPILER_OPTION_COUNT,
        &mut violations,
    );
    expect_count(
        "resolver packageJson.fields",
        artifacts.resolver_package_fields.len(),
        RESOLVER_PACKAGE_FIELD_COUNT,
        &mut violations,
    );
    expect_count(
        "inventory packageJson.fields",
        artifacts.inventory_package_fields.len(),
        INVENTORY_PACKAGE_FIELD_COUNT,
        &mut violations,
    );
    expect_count(
        "inventory pnpmWorkspaceYaml.fields",
        artifacts.inventory_pnpm_fields.len(),
        INVENTORY_PNPM_FIELD_COUNT,
        &mut violations,
    );
    validate_rows(
        "resolver tsconfigTopLevel",
        &artifacts.resolver_top_level,
        &[
            "KnownResolutionNeutral",
            "SupportedAndModeled",
            "UnsupportedResolutionAffecting",
        ],
        &mut violations,
    );
    validate_rows(
        "resolver compilerOptions",
        &artifacts.resolver_compiler_options,
        &[
            "KnownResolutionNeutral",
            "SupportedAndModeled",
            "UnsupportedResolutionAffecting",
        ],
        &mut violations,
    );
    validate_rows(
        "resolver packageJson.fields",
        &artifacts.resolver_package_fields,
        &["SupportedAndModeled", "UnsupportedResolutionAffecting"],
        &mut violations,
    );
    validate_package_applicability(&artifacts.resolver_package_fields, &mut violations);
    validate_rows(
        "inventory packageJson.fields",
        &artifacts.inventory_package_fields,
        &["SupportedAndModeled"],
        &mut violations,
    );
    validate_rows(
        "inventory pnpmWorkspaceYaml.fields",
        &artifacts.inventory_pnpm_fields,
        &["SupportedAndModeled", "UnsupportedInventoryAffecting"],
        &mut violations,
    );
    validate_baseline_count(artifacts, &mut violations);
    validate_owner_partition(artifacts, &mut violations);
    violations
}

fn validate_package_applicability(rows: &[PolicyRow], violations: &mut Vec<String>) {
    let mut paths = BTreeSet::new();
    for row in rows {
        if !paths.insert(row.path.as_str()) {
            violations.push(format!(
                "GENERATED TABLE DUPLICATE PACKAGE FIELD: resolver packageJson.fields repeats {}",
                row.path
            ));
        }
        match row.applies_when.as_deref() {
            Some(value) if package_applicability_variant(value).is_ok() => {}
            Some(value) => violations.push(format!(
                "GENERATED TABLE UNKNOWN APPLICABILITY: resolver packageJson.fields.{} has {value}",
                row.path
            )),
            None => violations.push(format!(
                "GENERATED TABLE MISSING APPLICABILITY: resolver packageJson.fields.{}",
                row.path
            )),
        }
    }
}

fn package_applicability_variant(value: &str) -> Result<&'static str, String> {
    match value {
        "bundler value lane when exports is absent or NotConsultedForProfile" => {
            Ok("BundlerValueWhenExportsAbsentOrNotConsulted")
        }
        "selected profile exports state is enabled-modeled" => Ok("ExportsEnabledModeled"),
        "an internal # specifier is observed and selected profile imports state is enabled-unsupported" => {
            Ok("InternalImportsEnabledUnsupported")
        }
        "value lane fallback when exports is absent or NotConsultedForProfile" => {
            Ok("ValueFallbackWhenExportsAbsentOrNotConsulted")
        }
        "bundler value lane fallback when exports is absent" => {
            Ok("BundlerValueFallbackWhenExportsAbsent")
        }
        "side-effect reachability is requested in any profile" => Ok("SideEffectReachability"),
        "a supported workspace-package config extends selects this package" => {
            Ok("WorkspacePackageTsconfig")
        }
        "node16 or nodenext importer-format selection" => Ok("NodeImporterFormat"),
        "type lane fallback when exports is absent or NotConsultedForProfile" => {
            Ok("TypeFallbackWhenExportsAbsentOrNotConsulted")
        }
        other => Err(format!("unknown package applicability {other}")),
    }
}

fn expect_string(
    value: &Value,
    pointer: &str,
    expected: &str,
    artifact: &str,
    violations: &mut Vec<String>,
) {
    let actual = value.pointer(pointer).and_then(Value::as_str);
    if actual != Some(expected) {
        violations.push(format!(
            "GENERATED TABLE SCHEMA DRIFT: {artifact}{pointer} expected {expected:?}, got {actual:?}"
        ));
    }
}

fn expect_count(context: &str, actual: usize, expected: usize, violations: &mut Vec<String>) {
    if actual != expected {
        violations.push(format!(
            "GENERATED TABLE COUNT DRIFT: {context} expected {expected}, got {actual}"
        ));
    }
}

fn validate_rows(
    context: &str,
    rows: &[PolicyRow],
    allowed_classifications: &[&str],
    violations: &mut Vec<String>,
) {
    let mut identities = BTreeSet::new();
    for row in rows {
        if row.path.is_empty() || row.shape.is_empty() {
            violations.push(format!(
                "GENERATED TABLE INVALID ROW: {context} has an empty path or shape"
            ));
        }
        if !identities.insert((row.path.as_str(), row.shape.as_str())) {
            violations.push(format!(
                "GENERATED TABLE DUPLICATE ROW: {context} repeats {} / {}",
                row.path, row.shape
            ));
        }
        if !allowed_classifications.contains(&row.classification.as_str()) {
            violations.push(format!(
                "GENERATED TABLE UNKNOWN CLASSIFICATION: {context}.{} has {}",
                row.path, row.classification
            ));
            continue;
        }
        let metadata_valid = match row.classification.as_str() {
            "KnownResolutionNeutral" => {
                row.reason.is_some() && row.limitation.is_none() && row.rule.is_none()
            }
            "SupportedAndModeled" => {
                row.rule.is_some() && row.reason.is_none() && row.limitation.is_none()
            }
            "UnsupportedResolutionAffecting" | "UnsupportedInventoryAffecting" => {
                row.limitation.is_some() && row.reason.is_none() && row.rule.is_none()
            }
            _ => false,
        };
        if !metadata_valid {
            violations.push(format!(
                "GENERATED TABLE METADATA DRIFT: {context}.{} does not match {} metadata ownership",
                row.path, row.classification
            ));
        }
    }
}

fn validate_baseline_count(artifacts: &Artifacts, violations: &mut Vec<String>) {
    let baseline_count = artifacts
        .resolver_value
        .pointer("/typeScriptBaseline/compilerOptionCount")
        .and_then(Value::as_u64);
    if baseline_count != Some(RESOLVER_COMPILER_OPTION_COUNT as u64) {
        violations.push(format!(
            "GENERATED TABLE BASELINE DRIFT: compilerOptionCount expected {}, got {baseline_count:?}",
            RESOLVER_COMPILER_OPTION_COUNT
        ));
    }
}

fn validate_owner_partition(artifacts: &Artifacts, violations: &mut Vec<String>) {
    validate_unique_strings(
        "resolver packageJson.inventoryOwnedFields",
        &artifacts.resolver_inventory_owned,
        violations,
    );
    validate_unique_strings(
        "inventory packageJson.resolverOwnedFields",
        &artifacts.inventory_resolver_owned,
        violations,
    );
    let resolver_fields = field_names(&artifacts.resolver_package_fields);
    let inventory_fields = field_names(&artifacts.inventory_package_fields);
    let resolver_delegation = string_set(&artifacts.resolver_inventory_owned);
    let inventory_delegation = string_set(&artifacts.inventory_resolver_owned);

    if resolver_fields != inventory_delegation {
        violations.push(format!(
            "GENERATED TABLE OWNER DRIFT: resolver fields {} do not equal inventory resolverOwnedFields {}",
            display_set(&resolver_fields),
            display_set(&inventory_delegation)
        ));
    }
    if inventory_fields != resolver_delegation {
        violations.push(format!(
            "GENERATED TABLE OWNER DRIFT: inventory fields {} do not equal resolver inventoryOwnedFields {}",
            display_set(&inventory_fields),
            display_set(&resolver_delegation)
        ));
    }
    let overlap = resolver_fields
        .intersection(&inventory_fields)
        .copied()
        .collect::<Vec<_>>();
    if !overlap.is_empty() {
        violations.push(format!(
            "GENERATED TABLE OWNER OVERLAP: package fields are owned by both artifacts: {}",
            overlap.join(", ")
        ));
    }
}

fn validate_unique_strings(context: &str, values: &[String], violations: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            violations.push(format!(
                "GENERATED TABLE OWNER DUPLICATE: {context} repeats {value}"
            ));
        }
    }
}

fn field_names(rows: &[PolicyRow]) -> BTreeSet<&str> {
    rows.iter().map(|row| row.path.as_str()).collect()
}

fn string_set(values: &[String]) -> BTreeSet<&str> {
    values.iter().map(String::as_str).collect()
}

fn display_set(values: &BTreeSet<&str>) -> String {
    values.iter().copied().collect::<Vec<_>>().join(", ")
}

fn compare_expected_file(
    workspace_root: &Path,
    expected: &ExpectedFile,
    result: &mut GeneratedTableResult,
) {
    let path = workspace_path(workspace_root, expected.relative_path);
    let actual = match std::fs::read_to_string(&path) {
        Ok(actual) => actual,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            result.violations.push(format!(
                "GENERATED TABLE MISSING: {} (run `cargo run --locked -p lumin-xtask -- generated-tables --write`)",
                expected.relative_path
            ));
            return;
        }
        Err(error) => {
            result
                .tool_errors
                .push(format!("cannot read {}: {error}", path.display()));
            return;
        }
    };
    if actual != expected.content {
        result.violations.push(format!(
            "GENERATED TABLE DRIFT: {} differs from its reviewed artifact (run `cargo run --locked -p lumin-xtask -- generated-tables --write`)",
            expected.relative_path
        ));
    }
}

fn workspace_path(workspace_root: &Path, relative_path: &str) -> PathBuf {
    workspace_root.join(relative_path.replace('/', std::path::MAIN_SEPARATOR_STR))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
