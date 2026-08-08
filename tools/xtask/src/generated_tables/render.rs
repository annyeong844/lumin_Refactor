use super::*;

pub(super) fn expected_files(artifacts: &Artifacts) -> Result<Vec<ExpectedFile>, String> {
    Ok(vec![
        ExpectedFile {
            relative_path: RESOLVER_OUTPUT,
            content: render_resolver(artifacts)?,
        },
        ExpectedFile {
            relative_path: INVENTORY_OUTPUT,
            content: render_inventory(artifacts)?,
        },
    ])
}

fn render_resolver(artifacts: &Artifacts) -> Result<String, String> {
    let semantic_bytes = serde_json::to_vec(&artifacts.resolver_value)
        .map_err(|error| format!("cannot canonicalize {RESOLVER_SPEC}: {error}"))?;
    let mut output = generated_header(RESOLVER_SPEC);
    output.push_str(
        r#"#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldClassification {
    KnownResolutionNeutral,
    SupportedAndModeled,
    UnsupportedResolutionAffecting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackageFieldApplicability {
    BundlerValueWhenExportsAbsentOrNotConsulted,
    ExportsEnabledModeled,
    InternalImportsEnabledUnsupported,
    ValueFallbackWhenExportsAbsentOrNotConsulted,
    BundlerValueFallbackWhenExportsAbsent,
    SideEffectReachability,
    WorkspacePackageTsconfig,
    NodeImporterFormat,
    TypeFallbackWhenExportsAbsentOrNotConsulted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PackageFieldApplicabilityPolicy {
    pub path: &'static str,
    pub applicability: PackageFieldApplicability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldPolicy {
    pub path: &'static str,
    pub shape: &'static str,
    pub classification: FieldClassification,
    pub reason: Option<&'static str>,
    pub limitation: Option<&'static str>,
    pub rule: Option<&'static str>,
    pub shape_mismatch_limitation: Option<&'static str>,
    pub applies_when: Option<&'static str>,
}

"#,
    );
    render_identity_constants(
        &mut output,
        "RESOLVER_CONFIG",
        &artifacts.resolver_source_sha256,
        &sha256_hex(&semantic_bytes),
    );
    render_rows(
        &mut output,
        "RESOLVER_TSCONFIG_TOP_LEVEL",
        &artifacts.resolver_top_level,
        "FieldClassification",
    )?;
    render_rows(
        &mut output,
        "RESOLVER_COMPILER_OPTIONS",
        &artifacts.resolver_compiler_options,
        "FieldClassification",
    )?;
    render_rows(
        &mut output,
        "RESOLVER_PACKAGE_JSON_FIELDS",
        &artifacts.resolver_package_fields,
        "FieldClassification",
    )?;
    render_package_applicability(&mut output, &artifacts.resolver_package_fields)?;
    render_string_array(
        &mut output,
        "RESOLVER_INVENTORY_OWNED_FIELDS",
        &artifacts.resolver_inventory_owned,
    );
    output.push_str(
        r#"pub(crate) fn tsconfig_top_level_fields(
    path: &str,
) -> impl Iterator<Item = &'static FieldPolicy> + '_ {
    RESOLVER_TSCONFIG_TOP_LEVEL
        .iter()
        .filter(move |policy| policy.path == path)
}

pub(crate) fn compiler_option(path: &str) -> Option<&'static FieldPolicy> {
    RESOLVER_COMPILER_OPTIONS
        .iter()
        .find(|policy| policy.path == path)
}

pub(crate) fn package_json_field_for_rule(rule: &str) -> Option<&'static FieldPolicy> {
    RESOLVER_PACKAGE_JSON_FIELDS
        .iter()
        .find(|policy| policy.rule == Some(rule))
}
"#,
    );
    Ok(output)
}

fn render_inventory(artifacts: &Artifacts) -> Result<String, String> {
    let semantic_bytes = serde_json::to_vec(&artifacts.inventory_value)
        .map_err(|error| format!("cannot canonicalize {INVENTORY_SPEC}: {error}"))?;
    let mut output = generated_header(INVENTORY_SPEC);
    output.push_str(
        r#"#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldClassification {
    SupportedAndModeled,
    UnsupportedInventoryAffecting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldPolicy {
    pub path: &'static str,
    pub shape: &'static str,
    pub classification: FieldClassification,
    pub reason: Option<&'static str>,
    pub limitation: Option<&'static str>,
    pub rule: Option<&'static str>,
    pub shape_mismatch_limitation: Option<&'static str>,
    pub applies_when: Option<&'static str>,
}

"#,
    );
    render_identity_constants(
        &mut output,
        "INVENTORY_CONFIG",
        &artifacts.inventory_source_sha256,
        &sha256_hex(&semantic_bytes),
    );
    render_rows(
        &mut output,
        "INVENTORY_PACKAGE_JSON_FIELDS",
        &artifacts.inventory_package_fields,
        "FieldClassification",
    )?;
    render_rows(
        &mut output,
        "INVENTORY_PNPM_WORKSPACE_FIELDS",
        &artifacts.inventory_pnpm_fields,
        "FieldClassification",
    )?;
    render_string_array(
        &mut output,
        "INVENTORY_RESOLVER_OWNED_FIELDS",
        &artifacts.inventory_resolver_owned,
    );
    output.push_str(
        r#"pub(crate) fn package_json_field_for_rule(rule: &str) -> Option<&'static FieldPolicy> {
    INVENTORY_PACKAGE_JSON_FIELDS
        .iter()
        .find(|policy| policy.rule == Some(rule))
}

pub(crate) fn pnpm_workspace_field(path: &str) -> Option<&'static FieldPolicy> {
    INVENTORY_PNPM_WORKSPACE_FIELDS
        .iter()
        .find(|policy| policy.path == path)
}
"#,
    );
    Ok(output)
}

fn generated_header(source: &str) -> String {
    format!(
        "// @generated by `cargo run --locked -p lumin-xtask -- generated-tables --write`.\n\
         // Source: {source}\n\
         // Do not edit this file by hand.\n\n"
    )
}

fn render_identity_constants(output: &mut String, prefix: &str, source_sha: &str, table_sha: &str) {
    push_line(
        output,
        &format!("pub const {prefix}_ARTIFACT_SHA256: &str =\n    {source_sha:?};"),
    );
    push_line(
        output,
        &format!("pub const {prefix}_TABLE_SHA256: &str =\n    {table_sha:?};\n"),
    );
}

fn render_rows(
    output: &mut String,
    constant: &str,
    rows: &[PolicyRow],
    classification_type: &str,
) -> Result<(), String> {
    push_line(
        output,
        &format!("pub static {constant}: &[FieldPolicy] = &["),
    );
    for row in rows {
        push_line(output, "    FieldPolicy {");
        push_line(output, &format!("        path: {:?},", row.path));
        push_line(output, &format!("        shape: {:?},", row.shape));
        push_line(
            output,
            &format!(
                "        classification: {classification_type}::{},",
                classification_variant(&row.classification)?
            ),
        );
        render_optional(output, "reason", row.reason.as_deref());
        render_optional(output, "limitation", row.limitation.as_deref());
        render_optional(output, "rule", row.rule.as_deref());
        render_optional(
            output,
            "shape_mismatch_limitation",
            row.shape_mismatch_limitation.as_deref(),
        );
        render_optional(output, "applies_when", row.applies_when.as_deref());
        push_line(output, "    },");
    }
    push_line(output, "];\n");
    Ok(())
}

fn render_package_applicability(output: &mut String, rows: &[PolicyRow]) -> Result<(), String> {
    push_line(
        output,
        "pub(crate) static RESOLVER_PACKAGE_FIELD_APPLICABILITY: &[PackageFieldApplicabilityPolicy] = &[",
    );
    for row in rows {
        let variant =
            package_applicability_variant(row.applies_when.as_deref().unwrap_or_default())?;
        push_line(output, "    PackageFieldApplicabilityPolicy {");
        push_line(output, &format!("        path: {:?},", row.path));
        push_line(
            output,
            &format!("        applicability: PackageFieldApplicability::{variant},"),
        );
        push_line(output, "    },");
    }
    push_line(output, "];\n");
    Ok(())
}

fn classification_variant(classification: &str) -> Result<&'static str, String> {
    match classification {
        "KnownResolutionNeutral" => Ok("KnownResolutionNeutral"),
        "SupportedAndModeled" => Ok("SupportedAndModeled"),
        "UnsupportedResolutionAffecting" => Ok("UnsupportedResolutionAffecting"),
        "UnsupportedInventoryAffecting" => Ok("UnsupportedInventoryAffecting"),
        other => Err(format!("cannot render unknown classification {other}")),
    }
}

fn render_optional(output: &mut String, field: &str, value: Option<&str>) {
    match value {
        Some(value) => {
            let inline = format!("        {field}: Some({value:?}),");
            if inline.len() <= 100 {
                push_line(output, &inline);
            } else {
                push_line(
                    output,
                    &format!("        {field}: Some(\n            {value:?},\n        ),"),
                );
            }
        }
        None => push_line(output, &format!("        {field}: None,")),
    }
}

fn render_string_array(output: &mut String, constant: &str, values: &[String]) {
    push_line(output, &format!("pub static {constant}: &[&str] = &["));
    for value in values {
        push_line(output, &format!("    {value:?},"));
    }
    push_line(output, "];\n");
}

fn push_line(output: &mut String, line: &str) {
    output.push_str(line);
    output.push('\n');
}
