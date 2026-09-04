use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

use super::path_roundtrip::{
    expect_u64, portable_component, repo_path_base64, required_array, required_string,
};
use super::{expect_status, expect_success, parse_json, run_binary};

pub(super) fn validate(binary: &Path, root: &Path) -> Result<(), String> {
    fs::create_dir(root)
        .map_err(|error| format!("cannot create packaged alias fixture root: {error}"))?;
    fs::create_dir(root.join("src"))
        .map_err(|error| format!("cannot create packaged alias source directory: {error}"))?;
    let original = root.join("src/original.ts");
    let alias = root.join("src/alias.ts");
    let unrelated = root.join("src/unrelated.ts");
    fs::write(&original, b"export const beforeAliasValue = 1;\n")
        .map_err(|error| format!("cannot write packaged alias source: {error}"))?;
    fs::hard_link(&original, &alias)
        .map_err(|error| format!("cannot create packaged physical alias: {error}"))?;
    fs::write(
        &unrelated,
        b"const unrelatedValue = 3;\nconsole.log(unrelatedValue);\n",
    )
    .map_err(|error| format!("cannot write packaged unrelated source: {error}"))?;

    let audit = expect_success(
        run_binary(binary, root, &["audit", "--jobs", "1", "--format", "json"]),
        "packaged alias audit",
    )?;
    let audit = parse_json("packaged alias audit", &audit.stdout)?;
    let run_id = required_string(&audit, "/runId", "packaged alias audit")?;
    let overview = expect_success(
        run_binary(
            binary,
            root,
            &["overview", "--run", &run_id, "--format", "json"],
        ),
        "packaged alias overview",
    )?;
    let overview = parse_json("packaged alias overview", &overview.stdout)?;
    for (field, expected) in [
        ("logicalSourceCount", 3),
        ("physicalSourceCount", 2),
        ("payloadSnapshotCount", 2),
        ("jsParseProductCount", 2),
    ] {
        expect_u64(
            &overview,
            &format!("/analysisMetrics/{field}"),
            expected,
            "packaged alias overview",
        )?;
    }

    let original_file = query_file(binary, root, &run_id, "src/original.ts")?;
    let alias_file = query_file(binary, root, &run_id, "src/alias.ts")?;
    let unrelated_file = query_file(binary, root, &run_id, "src/unrelated.ts")?;
    let original_source = required_string(
        &original_file,
        "/sourceContext/sourceId",
        "packaged original alias query",
    )?;
    let alias_source = required_string(
        &alias_file,
        "/sourceContext/sourceId",
        "packaged alternate alias query",
    )?;
    let unrelated_source = required_string(
        &unrelated_file,
        "/sourceContext/sourceId",
        "packaged unrelated source query",
    )?;
    if original_source == alias_source
        || original_source == unrelated_source
        || alias_source == unrelated_source
    {
        return Err("packaged sources were merged into one logical identity".to_owned());
    }
    let original_observation = original_file
        .get("sourceObservation")
        .ok_or_else(|| "packaged original alias query omitted sourceObservation".to_owned())?;
    let alias_observation = alias_file
        .get("sourceObservation")
        .ok_or_else(|| "packaged alternate alias query omitted sourceObservation".to_owned())?;
    let original_physical = original_observation
        .get("physicalIdentity")
        .ok_or_else(|| "packaged original alias observation omitted physicalIdentity".to_owned())?;
    let alias_physical = alias_observation.get("physicalIdentity").ok_or_else(|| {
        "packaged alternate alias observation omitted physicalIdentity".to_owned()
    })?;
    let original_payload = original_observation
        .get("payloadSnapshotId")
        .ok_or_else(|| {
            "packaged original alias observation omitted payloadSnapshotId".to_owned()
        })?;
    let alias_payload = alias_observation.get("payloadSnapshotId").ok_or_else(|| {
        "packaged alternate alias observation omitted payloadSnapshotId".to_owned()
    })?;
    if original_physical != alias_physical || original_payload != alias_payload {
        return Err(
            "packaged physical aliases did not share physical and payload identities".to_owned(),
        );
    }
    let unrelated_observation = unrelated_file
        .get("sourceObservation")
        .ok_or_else(|| "packaged unrelated source query omitted sourceObservation".to_owned())?;
    let unrelated_physical = unrelated_observation
        .get("physicalIdentity")
        .ok_or_else(|| "packaged unrelated observation omitted physicalIdentity".to_owned())?;
    let unrelated_payload = unrelated_observation
        .get("payloadSnapshotId")
        .ok_or_else(|| "packaged unrelated observation omitted payloadSnapshotId".to_owned())?;
    if unrelated_physical == original_physical || unrelated_payload == original_payload {
        return Err(
            "packaged unrelated source was folded into the physical alias group".to_owned(),
        );
    }

    let pre = expect_success(
        run_binary(
            binary,
            root,
            &[
                "pre-write",
                "--operation-id",
                "package-alias-open-0001",
                "--path",
                "src/original.ts",
                "--jobs",
                "1",
                "--format",
                "json",
            ],
        ),
        "packaged alias pre-write",
    )?;
    let pre = parse_json("packaged alias pre-write", &pre.stdout)?;
    let gate_id = required_string(&pre, "/gateId", "packaged alias pre-write")?;
    assert_path_set(
        &pre,
        "/leasedWriteSet",
        "/path",
        &["src/alias.ts", "src/original.ts"],
        "packaged alias lease closure",
    )?;
    let baseline_findings = query_gate_findings(binary, root, &gate_id, "0")?;
    let baseline_ids = assert_alias_findings(
        &baseline_findings,
        "beforeAliasValue",
        &["src/alias.ts", "src/original.ts"],
        "packaged alias baseline findings",
    )?;

    fs::write(&alias, b"export const afterAliasValue = 2;\n")
        .map_err(|error| format!("cannot mutate packaged alias source: {error}"))?;
    let post = run_binary(
        binary,
        root,
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "package-alias-close-0002",
            "--format",
            "json",
        ],
    )?;
    expect_status(&post, Some(3), "packaged alias post-write")?;
    if !post.stderr.is_empty() {
        return Err("packaged alias denial wrote a stderr diagnostic".to_owned());
    }
    let post = parse_json("packaged alias post-write", &post.stdout)?;
    let decision = required_string(&post, "/decision", "packaged alias post-write")?;
    if decision != "deny" {
        return Err(format!(
            "packaged alias post-write returned {decision}; expected deny for a new dead export"
        ));
    }
    assert_path_set(
        &post,
        "/actualWriteSet/paths",
        "",
        &["src/alias.ts", "src/original.ts"],
        "packaged alias actual-write closure",
    )?;
    assert_alias_group(
        &post,
        "/actualWriteSet/baselineAliasClosures",
        &["src/alias.ts", "src/original.ts"],
    )?;
    assert_alias_group(
        &post,
        "/actualWriteSet/currentAliasClosures",
        &["src/alias.ts", "src/original.ts"],
    )?;
    let current_findings = query_gate_findings(binary, root, &gate_id, "1")?;
    let current_ids = assert_alias_findings(
        &current_findings,
        "afterAliasValue",
        &["src/alias.ts", "src/original.ts"],
        "packaged alias current findings",
    )?;
    if baseline_ids
        .iter()
        .any(|identity| current_ids.contains(identity))
    {
        return Err("packaged alias close reused stale baseline finding identities".to_owned());
    }

    let shown = expect_success(
        run_binary(
            binary,
            root,
            &["gate", "show", &gate_id, "--format", "json"],
        ),
        "packaged alias gate show",
    )?;
    let shown = parse_json("packaged alias gate show", &shown.stdout)?;
    expect_u64(
        &shown,
        "/revisions/1/aliasGroupCount",
        1,
        "packaged alias gate show",
    )
}

fn query_file(binary: &Path, root: &Path, run_id: &str, path: &str) -> Result<Value, String> {
    let output = expect_success(
        run_binary(binary, root, &["files", "--run", run_id, path]),
        "packaged physical-alias file query",
    )?;
    parse_json("packaged physical-alias file query", &output.stdout)
}

fn query_gate_findings(
    binary: &Path,
    root: &Path,
    gate_id: &str,
    revision: &str,
) -> Result<Value, String> {
    let output = expect_success(
        run_binary(
            binary,
            root,
            &[
                "gate",
                "findings",
                gate_id,
                "--revision",
                revision,
                "--format",
                "json",
            ],
        ),
        "packaged physical-alias gate findings",
    )?;
    parse_json("packaged physical-alias gate findings", &output.stdout)
}

fn assert_alias_findings(
    response: &Value,
    exported_name: &str,
    expected_paths: &[&str],
    label: &str,
) -> Result<BTreeSet<String>, String> {
    let findings = required_array(response, "/items", label)?;
    if findings.len() != expected_paths.len() {
        return Err(format!(
            "{label} returned {} findings; expected {}",
            findings.len(),
            expected_paths.len()
        ));
    }
    let mut source_ids = BTreeSet::new();
    let mut finding_ids = BTreeSet::new();
    for (finding, expected_path) in findings.iter().zip(expected_paths) {
        if finding.get("exportedName").and_then(Value::as_str) != Some(exported_name) {
            return Err(format!("{label} omitted the {exported_name} owner result"));
        }
        assert_portable_projection_dto(
            finding
                .get("path")
                .ok_or_else(|| format!("{label} omitted its finding path"))?,
            expected_path,
            label,
        )?;
        source_ids.insert(required_string(finding, "/sourceId", label)?);
        finding_ids.insert(required_string(finding, "/findingId", label)?);
    }
    if source_ids.len() != expected_paths.len() || finding_ids.len() != expected_paths.len() {
        return Err(format!(
            "{label} collapsed physical aliases into one logical analysis result"
        ));
    }
    Ok(finding_ids)
}

fn assert_path_set(
    value: &Value,
    pointer: &str,
    nested_path: &str,
    expected: &[&str],
    label: &str,
) -> Result<(), String> {
    let paths = required_array(value, pointer, label)?;
    if paths.len() != expected.len() {
        return Err(format!(
            "{label} returned {} paths; expected {}",
            paths.len(),
            expected.len()
        ));
    }
    for (path, expected_path) in paths.iter().zip(expected) {
        let dto = if nested_path.is_empty() {
            path
        } else {
            path.pointer(nested_path)
                .ok_or_else(|| format!("{label} omitted nested path {nested_path}"))?
        };
        assert_portable_projection_dto(dto, expected_path, label)?;
    }
    Ok(())
}

fn assert_alias_group(value: &Value, pointer: &str, expected: &[&str]) -> Result<(), String> {
    let groups = required_array(value, pointer, "packaged alias group")?;
    if groups.len() != 1 {
        return Err(format!(
            "packaged alias closure at {pointer} returned {} groups; expected 1",
            groups.len()
        ));
    }
    let Some(members) = groups[0].get("members").and_then(Value::as_array) else {
        return Err(format!(
            "packaged alias closure at {pointer} omitted group members"
        ));
    };
    if members.len() != expected.len() {
        return Err(format!(
            "packaged alias closure at {pointer} omitted the exact physical group"
        ));
    }
    for (member, expected_path) in members.iter().zip(expected) {
        assert_portable_projection_dto(member, expected_path, "packaged alias group")?;
    }
    Ok(())
}

fn assert_portable_projection_dto(
    dto: &Value,
    expected_path: &str,
    label: &str,
) -> Result<(), String> {
    let canonical = repo_path_base64(
        &expected_path
            .split('/')
            .map(|component| portable_component(component.as_bytes()))
            .collect::<Vec<_>>(),
    );
    if dto.as_object().map(serde_json::Map::len) != Some(3)
        || dto.get("encoding").and_then(Value::as_str) != Some("repo-path.v1")
        || dto.get("canonicalBase64").and_then(Value::as_str) != Some(canonical.as_str())
        || dto.get("display").and_then(Value::as_str) != Some(expected_path)
        || dto.get("utf8").is_some()
    {
        return Err(format!(
            "{label} changed the exact DTO for {expected_path}: {dto}"
        ));
    }
    Ok(())
}
