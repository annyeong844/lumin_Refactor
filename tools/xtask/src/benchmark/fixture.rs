use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const PACKET: &str = "reviews/probes/phase0-numeric-target-selection-2026-07-18";
const EXPECTED_SCHEMA: &str = "phase1-scale-findings.v1";
const EXPECTED_FILE_COUNT: u64 = 780;
const EXPECTED_TOTAL_BYTES: u64 = 7_461_511;

pub(super) struct Fixture {
    pub(super) root: PathBuf,
    pub(super) truth: Value,
    pub(super) identity: Value,
}

pub(super) fn prepare(workspace: &Path, scratch: &Path, python: &Path) -> Result<Fixture, String> {
    let packet = workspace.join(PACKET);
    run_python(
        python,
        &packet.join("source/verify-packet.py"),
        &[],
        workspace,
        "numeric-target packet verification",
    )?;

    let root = scratch.join("fixture");
    let generated_manifest = scratch.join("generated-manifest.json");
    let generated_truth = scratch.join("generated-truth.json");
    let arguments = vec![
        "--output".into(),
        root.as_os_str().to_owned(),
        "--manifest".into(),
        generated_manifest.as_os_str().to_owned(),
        "--truth".into(),
        generated_truth.as_os_str().to_owned(),
    ];
    run_python(
        python,
        &packet.join("source/generate-scale-corpus.py"),
        &arguments,
        workspace,
        "scale fixture generation",
    )?;

    let retained_manifest = packet.join("evidence/scale-corpus-manifest.json");
    let retained_truth = packet.join("evidence/scale-corpus-expected-truth.json");
    require_equal_files(
        &generated_manifest,
        &retained_manifest,
        "generated scale manifest",
    )?;
    require_equal_files(&generated_truth, &retained_truth, "generated scale truth")?;

    let manifest_bytes = fs::read(&generated_manifest)
        .map_err(|error| format!("cannot read generated scale manifest: {error}"))?;
    let truth_bytes = fs::read(&generated_truth)
        .map_err(|error| format!("cannot read generated scale truth: {error}"))?;
    let manifest = parse_json(&manifest_bytes, "generated scale manifest")?;
    let truth = parse_json(&truth_bytes, "generated scale truth")?;
    require_fixture_shape(&manifest, &truth)?;

    let identity = serde_json::json!({
        "schemaVersion": EXPECTED_SCHEMA,
        "fileCount": EXPECTED_FILE_COUNT,
        "totalBytes": EXPECTED_TOTAL_BYTES,
        "contentManifestSha256": required_string(&manifest, "/contentManifestSha256")?,
        "manifestSha256": super::sha256_hex(&manifest_bytes),
        "truthSha256": super::sha256_hex(&truth_bytes),
        "generator": "reviews/probes/phase0-numeric-target-selection-2026-07-18/source/generate-scale-corpus.py",
    });
    Ok(Fixture {
        root,
        truth,
        identity,
    })
}

pub(super) fn copy_repository(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err(format!(
            "benchmark repository destination already exists: {}",
            destination.display()
        ));
    }
    fs::create_dir(destination).map_err(|error| {
        format!(
            "cannot create benchmark repository {}: {error}",
            destination.display()
        )
    })?;
    copy_directory(source, destination)
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    let mut entries = fs::read_dir(source)
        .map_err(|error| format!("cannot enumerate fixture {}: {error}", source.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot enumerate fixture {}: {error}", source.display()))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path).map_err(|error| {
            format!(
                "cannot inspect fixture member {}: {error}",
                source_path.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "scale fixture contains a symbolic link: {}",
                source_path.display()
            ));
        }
        if metadata.is_dir() {
            fs::create_dir(&destination_path).map_err(|error| {
                format!(
                    "cannot create fixture directory {}: {error}",
                    destination_path.display()
                )
            })?;
            copy_directory(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &destination_path).map_err(|error| {
                format!(
                    "cannot copy fixture member {}: {error}",
                    source_path.display()
                )
            })?;
        } else {
            return Err(format!(
                "scale fixture contains an unsupported member: {}",
                source_path.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn mutate_one(root: &Path) -> Result<(), String> {
    mutate_numeric_value(root, "packages/pkg-00/src/live/live-000.ts")
}

pub(super) fn mutate_wave(root: &Path) -> Result<(), String> {
    for package in 0..8 {
        for source in 0..4 {
            mutate_numeric_value(
                root,
                &format!("packages/pkg-{package:02}/src/live/live-{source:03}.ts"),
            )?;
        }
    }
    Ok(())
}

fn mutate_numeric_value(root: &Path, relative: &str) -> Result<(), String> {
    let path = root.join(relative);
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read mutation target {}: {error}", path.display()))?;
    let marker = source
        .rfind(" + ")
        .ok_or_else(|| format!("mutation target has no numeric suffix: {relative}"))?;
    let suffix = &source[marker + 3..];
    let number_end = suffix
        .find(';')
        .ok_or_else(|| format!("mutation target has no statement terminator: {relative}"))?;
    let number = suffix[..number_end]
        .parse::<u64>()
        .map_err(|error| format!("mutation target has a nonnumeric suffix: {relative}: {error}"))?;
    let replacement = format!(
        "{}{}{}",
        &source[..marker + 3],
        number + 1,
        &suffix[number_end..]
    );
    fs::write(&path, replacement.as_bytes())
        .map_err(|error| format!("cannot mutate {}: {error}", path.display()))
}

fn run_python(
    python: &Path,
    script: &Path,
    arguments: &[std::ffi::OsString],
    current_dir: &Path,
    label: &str,
) -> Result<(), String> {
    let output = Command::new(python)
        .arg("-I")
        .arg("-S")
        .arg(script)
        .args(arguments)
        .current_dir(current_dir)
        .output()
        .map_err(|error| format!("cannot run {label}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{label} failed with {:?}; stdout={} stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if !output.stderr.is_empty() {
        return Err(format!(
            "{label} wrote stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn require_equal_files(actual: &Path, expected: &Path, label: &str) -> Result<(), String> {
    let actual_bytes = fs::read(actual)
        .map_err(|error| format!("cannot read {label} {}: {error}", actual.display()))?;
    let expected_bytes = fs::read(expected).map_err(|error| {
        format!(
            "cannot read retained {label} {}: {error}",
            expected.display()
        )
    })?;
    if actual_bytes != expected_bytes {
        return Err(format!(
            "{label} differs from retained independent evidence"
        ));
    }
    Ok(())
}

fn parse_json(bytes: &[u8], label: &str) -> Result<Value, String> {
    serde_json::from_slice(bytes).map_err(|error| format!("{label} is invalid JSON: {error}"))
}

fn require_fixture_shape(manifest: &Value, truth: &Value) -> Result<(), String> {
    for (document, label) in [(manifest, "manifest"), (truth, "truth")] {
        if required_string(document, "/schemaVersion")? != EXPECTED_SCHEMA {
            return Err(format!("scale {label} has the wrong schema"));
        }
    }
    if manifest.pointer("/fileCount").and_then(Value::as_u64) != Some(EXPECTED_FILE_COUNT)
        || manifest.pointer("/totalBytes").and_then(Value::as_u64) != Some(EXPECTED_TOTAL_BYTES)
    {
        return Err("scale fixture size differs from the frozen contract".to_owned());
    }
    if truth
        .pointer("/expectedFindings")
        .and_then(Value::as_array)
        .map(Vec::len)
        != Some(256)
        || truth
            .pointer("/filters")
            .and_then(Value::as_object)
            .map(serde_json::Map::len)
            != Some(0)
        || truth
            .pointer("/limitations")
            .and_then(Value::as_array)
            .map(Vec::len)
            != Some(0)
    {
        return Err("scale truth differs from the frozen semantic contract".to_owned());
    }
    Ok(())
}

fn required_string<'a>(value: &'a Value, pointer: &str) -> Result<&'a str, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("scale fixture document omitted {pointer}"))
}
