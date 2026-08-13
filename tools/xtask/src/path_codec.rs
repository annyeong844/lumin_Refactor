//! Generated path/root codec identity and runtime-equivalence checks.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use lumin_model::{PATH_CODEC_ARTIFACT_SHA256, PATH_CODEC_TABLE_SHA256};
use serde_json::Value;
use sha2::{Digest, Sha256};
use syn::visit::Visit;

mod oracle;
mod runtime;
#[cfg(test)]
mod tests;

const SPEC: &str = "specs/repo-path-semantics.v1.json";
const OUTPUT: &str = "crates/foundation/model/src/generated_path_codec.rs";
const ANALYSIS_CONTRACT_OWNER: &str = "crates/application/engine/src/write_gate.rs";

#[derive(Debug, Default)]
pub(crate) struct PathCodecResult {
    pub violations: Vec<String>,
    pub tool_errors: Vec<String>,
}

struct Artifact {
    value: Value,
    source_sha256: String,
}

pub(crate) fn check_path_codec(workspace_root: &Path) -> PathCodecResult {
    let mut result = PathCodecResult::default();
    let artifact = match load_artifact(workspace_root) {
        Ok(artifact) => artifact,
        Err(error) => {
            result.tool_errors.push(error);
            return result;
        }
    };
    result.violations.extend(validate_artifact(&artifact.value));
    if !result.violations.is_empty() {
        return result;
    }

    let expected = match render_generated(&artifact) {
        Ok(expected) => expected,
        Err(error) => {
            result.tool_errors.push(error);
            return result;
        }
    };
    let output_path = workspace_root.join(OUTPUT);
    match std::fs::read_to_string(&output_path) {
        Ok(actual) if actual == expected => {}
        Ok(_) => result.violations.push(format!(
            "PATH CODEC GENERATED DRIFT: run `cargo run --locked -p lumin-xtask -- path-codec --write` for {OUTPUT}"
        )),
        Err(error) => result.tool_errors.push(format!(
            "cannot read generated path codec {}: {error}",
            output_path.display()
        )),
    }

    if PATH_CODEC_ARTIFACT_SHA256 != artifact.source_sha256 {
        result.violations.push(format!(
            "PATH CODEC ARTIFACT IDENTITY DRIFT: runtime {PATH_CODEC_ARTIFACT_SHA256}, artifact {}",
            artifact.source_sha256
        ));
    }
    match semantic_sha256(&artifact.value) {
        Ok(semantic_sha) if PATH_CODEC_TABLE_SHA256 == semantic_sha => {}
        Ok(semantic_sha) => result.violations.push(format!(
            "PATH CODEC TABLE IDENTITY DRIFT: runtime {PATH_CODEC_TABLE_SHA256}, artifact {semantic_sha}"
        )),
        Err(error) => result.tool_errors.push(error),
    }

    match oracle::check(&artifact.value) {
        Ok(violations) => result.violations.extend(violations),
        Err(error) => result.tool_errors.push(error),
    }
    match runtime::check(&artifact.value) {
        Ok(violations) => result.violations.extend(violations),
        Err(error) => result.tool_errors.push(error),
    }
    match check_analysis_contract_wiring(workspace_root) {
        Ok(violations) => result.violations.extend(violations),
        Err(error) => result.tool_errors.push(error),
    }
    result.violations.sort();
    result.violations.dedup();
    result.tool_errors.sort();
    result.tool_errors.dedup();
    result
}

pub(crate) fn write_generated_codec(workspace_root: &Path) -> Result<PathBuf, String> {
    let artifact = load_artifact(workspace_root)?;
    let violations = validate_artifact(&artifact.value);
    if !violations.is_empty() {
        return Err(format!(
            "path codec artifact violates its frozen contract:\n{}",
            violations.join("\n")
        ));
    }
    let content = render_generated(&artifact)?;
    let output = workspace_root.join(OUTPUT);
    std::fs::write(&output, content.as_bytes())
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    Ok(output)
}

fn load_artifact(workspace_root: &Path) -> Result<Artifact, String> {
    let path = workspace_root.join(SPEC);
    let bytes =
        std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let value =
        serde_json::from_slice(&bytes).map_err(|error| format!("cannot parse {SPEC}: {error}"))?;
    Ok(Artifact {
        value,
        source_sha256: sha256_hex(&bytes),
    })
}

fn validate_artifact(value: &Value) -> Vec<String> {
    let mut violations = Vec::new();
    expect_string(
        value,
        "/schemaVersion",
        "repo-path-semantics.v1",
        &mut violations,
    );
    expect_string(
        value,
        "/repoPath/magicHex",
        "4c554d5250415448",
        &mut violations,
    );
    expect_u64(value, "/repoPath/versionU16", 1, &mut violations);
    expect_string(
        value,
        "/repositoryRoot/magicHex",
        "4c554d52524f4f54",
        &mut violations,
    );
    expect_u64(value, "/repositoryRoot/versionU16", 1, &mut violations);

    let component_tags = [
        ("PortableUtf8", "01"),
        ("UnixBytes", "02"),
        ("WindowsWtf16", "03"),
    ];
    validate_named_tags(
        value.pointer("/repoPath/component/kinds"),
        "repoPath.component.kinds",
        &component_tags,
        &mut violations,
    );
    let address_tags = [
        ("UnixAbsolute", "01"),
        ("WindowsDrive", "02"),
        ("WindowsUNC", "03"),
        ("WindowsVolumeGuid", "04"),
    ];
    validate_named_tags(
        value.pointer("/repositoryRoot/addressKinds"),
        "repositoryRoot.addressKinds",
        &address_tags,
        &mut violations,
    );
    expect_string(
        value,
        "/repositoryRoot/platformTags/Unix",
        "01",
        &mut violations,
    );
    expect_string(
        value,
        "/repositoryRoot/platformTags/Windows",
        "02",
        &mut violations,
    );
    expect_array_len(value, "/goldenVectors", 10, &mut violations);
    expect_array_len(value, "/ioGoldenVectors", 4, &mut violations);
    expect_array_len(value, "/rootDtoGoldenVectors", 4, &mut violations);
    expect_array_len(value, "/rejectionVectors", 9, &mut violations);
    expect_array_len(value, "/rootDtoRejectionVectors", 7, &mut violations);
    expect_string(
        value,
        "/compiledContract/typeOwner",
        "lumin-model",
        &mut violations,
    );
    expect_string(
        value,
        "/compiledContract/valueAuthority",
        "lumin-inventory",
        &mut violations,
    );
    expect_string(
        value,
        "/compiledContract/wireOwner",
        "lumin-protocol",
        &mut violations,
    );

    validate_golden_rows(value, &mut violations);
    violations
}

fn validate_golden_rows(value: &Value, violations: &mut Vec<String>) {
    let Some(rows) = value.pointer("/goldenVectors").and_then(Value::as_array) else {
        return;
    };
    let mut ids = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        let context = format!("goldenVectors[{index}]");
        let Some(id) = row.get("id").and_then(Value::as_str) else {
            violations.push(format!(
                "PATH CODEC ARTIFACT: {context}.id must be a string"
            ));
            continue;
        };
        if ids.insert(id, index).is_some() {
            violations.push(format!(
                "PATH CODEC ARTIFACT: duplicate golden vector id {id}"
            ));
        }
        let Some(hex) = row.get("hex").and_then(Value::as_str) else {
            violations.push(format!(
                "PATH CODEC ARTIFACT: {context}.hex must be a string"
            ));
            continue;
        };
        let Some(base64) = row.get("base64").and_then(Value::as_str) else {
            violations.push(format!(
                "PATH CODEC ARTIFACT: {context}.base64 must be a string"
            ));
            continue;
        };
        match (decode_hex(hex), STANDARD.decode(base64)) {
            (Ok(hex_bytes), Ok(base64_bytes)) if hex_bytes == base64_bytes => {
                if STANDARD.encode(&hex_bytes) != base64 {
                    violations.push(format!(
                        "PATH CODEC ARTIFACT: {context}.base64 is not canonical padded Base64"
                    ));
                }
            }
            (Ok(_), Ok(_)) => violations.push(format!(
                "PATH CODEC ARTIFACT: {context}.hex and base64 disagree"
            )),
            (Err(error), _) => violations.push(format!(
                "PATH CODEC ARTIFACT: {context}.hex cannot decode: {error}"
            )),
            (_, Err(error)) => violations.push(format!(
                "PATH CODEC ARTIFACT: {context}.base64 cannot decode: {error}"
            )),
        }
    }
}

fn render_generated(artifact: &Artifact) -> Result<String, String> {
    let table_sha = semantic_sha256(&artifact.value)?;
    Ok(format!(
        "// @generated by `cargo run --locked -p lumin-xtask -- path-codec --write`.\n\
         // Source: {SPEC}\n\
         // Do not edit this file by hand.\n\n\
         pub const PATH_CODEC_ARTIFACT_SHA256: &str =\n    {:?};\n\
         pub const PATH_CODEC_TABLE_SHA256: &str =\n    {:?};\n\n\
         pub const REPO_PATH_MAGIC: &[u8; 8] = b\"LUMRPATH\";\n\
         pub const REPO_PATH_VERSION: u16 = 1;\n\
         pub const PORTABLE_UTF8_TAG: u8 = 1;\n\
         pub const UNIX_BYTES_TAG: u8 = 2;\n\
         pub const WINDOWS_WTF16_TAG: u8 = 3;\n\n\
         pub const REPOSITORY_ROOT_MAGIC: &[u8; 8] = b\"LUMRROOT\";\n\
         pub const REPOSITORY_ROOT_VERSION: u16 = 1;\n\
         pub const UNIX_PLATFORM_TAG: u8 = 1;\n\
         pub const WINDOWS_PLATFORM_TAG: u8 = 2;\n\
         pub const UNIX_ABSOLUTE_TAG: u8 = 1;\n\
         pub const WINDOWS_DRIVE_TAG: u8 = 2;\n\
         pub const WINDOWS_UNC_TAG: u8 = 3;\n\
         pub const WINDOWS_VOLUME_GUID_TAG: u8 = 4;\n\
         pub const UNIX_PHYSICAL_IDENTITY_TAG: u8 = 1;\n\
         pub const WINDOWS_PHYSICAL_IDENTITY_TAG: u8 = 2;\n",
        artifact.source_sha256, table_sha
    ))
}

fn check_analysis_contract_wiring(workspace_root: &Path) -> Result<Vec<String>, String> {
    let path = workspace_root.join(ANALYSIS_CONTRACT_OWNER);
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let syntax = syn::parse_file(&source)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
    let function = syntax.items.iter().find_map(|item| match item {
        syn::Item::Fn(function) if function.sig.ident == "analysis_contract_id" => Some(function),
        _ => None,
    });
    let Some(function) = function else {
        return Ok(vec![
            "ANALYSIS CONTRACT: lumin-engine is missing analysis_contract_id".to_owned(),
        ]);
    };
    let mut visitor = ContractInputVisitor::default();
    visitor.visit_item_fn(function);
    let required = [
        "PATH_CODEC_ARTIFACT_SHA256",
        "PATH_CODEC_TABLE_SHA256",
        "SOURCE_CLASSIFICATION_RULE_VERSION",
        "INVENTORY_CONFIG_ARTIFACT_SHA256",
        "INVENTORY_CONFIG_TABLE_SHA256",
        "EXTRACTOR_SEMANTICS_VERSION",
        "SYMBOL_GRAPH_SEMANTICS_VERSION",
        "RESOLVER_VERSION",
        "RESOLVER_CONFIG_ARTIFACT_SHA256",
        "RESOLVER_CONFIG_TABLE_SHA256",
    ];
    let mut violations = required
        .into_iter()
        .filter(|required| !visitor.path_tails.contains(*required))
        .map(|missing| {
            format!("ANALYSIS CONTRACT: analysis_contract_id omits compiled input {missing}")
        })
        .collect::<Vec<_>>();
    let call_count = source.matches("analysis_contract_id()").count();
    if call_count != 3 {
        violations.push(format!(
            "ANALYSIS CONTRACT: expected definition plus open/close use of analysis_contract_id; found {call_count} occurrences"
        ));
    }
    Ok(violations)
}

#[derive(Default)]
struct ContractInputVisitor {
    path_tails: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for ContractInputVisitor {
    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        if let Some(segment) = node.path.segments.last() {
            self.path_tails.insert(segment.ident.to_string());
        }
        syn::visit::visit_expr_path(self, node);
    }
}

fn expect_string(value: &Value, pointer: &str, expected: &str, violations: &mut Vec<String>) {
    if value.pointer(pointer).and_then(Value::as_str) != Some(expected) {
        violations.push(format!(
            "PATH CODEC ARTIFACT: {pointer} must equal {expected:?}"
        ));
    }
}

fn expect_u64(value: &Value, pointer: &str, expected: u64, violations: &mut Vec<String>) {
    if value.pointer(pointer).and_then(Value::as_u64) != Some(expected) {
        violations.push(format!(
            "PATH CODEC ARTIFACT: {pointer} must equal {expected}"
        ));
    }
}

fn expect_array_len(value: &Value, pointer: &str, expected: usize, violations: &mut Vec<String>) {
    if value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(Vec::len)
        != Some(expected)
    {
        violations.push(format!(
            "PATH CODEC ARTIFACT: {pointer} must contain exactly {expected} rows"
        ));
    }
}

fn validate_named_tags(
    value: Option<&Value>,
    context: &str,
    expected: &[(&str, &str)],
    violations: &mut Vec<String>,
) {
    let Some(rows) = value.and_then(Value::as_array) else {
        violations.push(format!("PATH CODEC ARTIFACT: {context} must be an array"));
        return;
    };
    let actual = rows
        .iter()
        .filter_map(|row| Some((row.get("name")?.as_str()?, row.get("tagHex")?.as_str()?)))
        .collect::<Vec<_>>();
    if actual != expected {
        violations.push(format!(
            "PATH CODEC ARTIFACT: {context} tags changed: {actual:?}"
        ));
    }
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("path codec vector field {name} must be a string"))
}

fn semantic_sha256(value: &Value) -> Result<String, String> {
    serde_json::to_vec(value)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| format!("cannot canonicalize {SPEC}: {error}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("hex has odd length".to_owned());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16).map_err(|error| error.to_string())
        })
        .collect()
}
