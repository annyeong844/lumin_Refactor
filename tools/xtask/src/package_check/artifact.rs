use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    BINARY_ENVIRONMENT, PACKAGE_ROOT_ENVIRONMENT, expect_status, parse_json, run_binary,
    scratch_directory_for,
};

const MANIFEST_NAME: &str = "lumin-package.json";
const MANIFEST_SCHEMA: &str = "lumin.package.v1";
const CODEX_SKILL_PATH: &str = "skills/codex/SKILL.md";
const CLAUDE_SKILL_PATH: &str = "skills/claude-code/SKILL.md";

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    path: String,
    byte_count: u64,
    sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SkillIdentity {
    adapter: String,
    file: FileIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PackageManifest {
    target: String,
    build_id: String,
    binary: FileIdentity,
    skills: Vec<SkillIdentity>,
}

pub(super) struct PackageArtifact {
    pub(super) root: PathBuf,
    pub(super) binary: PathBuf,
    pub(super) build_id: String,
}

pub(super) fn stage(target: &str) -> Result<(), String> {
    require_host_target(target)?;
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    let package_root = configured_package_root()?;
    require_external_absolute_root(&workspace, &package_root, false)?;
    let source_binary = configured_source_binary()?;
    require_regular_file(&source_binary, "release binary")?;

    super::skills::stage_skill_sources(&workspace, &package_root)?;
    let binary_relative = binary_relative_path(target)?;
    let binary = package_root.join(binary_relative);
    let binary_parent = binary
        .parent()
        .ok_or_else(|| "packaged binary path has no parent".to_owned())?;
    fs::create_dir_all(binary_parent).map_err(|error| {
        format!(
            "cannot create packaged binary directory {}: {error}",
            binary_parent.display()
        )
    })?;
    fs::copy(&source_binary, &binary).map_err(|error| {
        format!(
            "cannot stage release binary {}: {error}",
            source_binary.display()
        )
    })?;
    require_executable(&binary)?;

    let build_id = read_binary_build_id(&binary)?;
    let manifest = PackageManifest {
        target: target.to_owned(),
        build_id,
        binary: file_identity(&package_root, binary_relative)?,
        skills: vec![
            SkillIdentity {
                adapter: "codex".to_owned(),
                file: file_identity(&package_root, CODEX_SKILL_PATH)?,
            },
            SkillIdentity {
                adapter: "claude-code".to_owned(),
                file: file_identity(&package_root, CLAUDE_SKILL_PATH)?,
            },
        ],
    };
    let manifest_bytes = render_manifest(&manifest);
    let manifest_path = package_root.join(MANIFEST_NAME);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(&manifest_path).map_err(|error| {
        format!(
            "cannot create package manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    std::io::Write::write_all(&mut file, &manifest_bytes)
        .map_err(|error| format!("cannot write package manifest: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("cannot flush package manifest: {error}"))?;

    let staged = read_package_root(&package_root, target)?;
    let observed_build_id = read_binary_build_id(&staged.binary)?;
    if observed_build_id != staged.build_id {
        return Err("staged binary build ID differs from package manifest".to_owned());
    }
    Ok(())
}

pub(super) fn load(target: &str) -> Result<PackageArtifact, String> {
    require_host_target(target)?;
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    let configured = configured_package_root()?;
    require_external_absolute_root(&workspace, &configured, true)?;
    let package_root = configured.canonicalize().map_err(|error| {
        format!(
            "cannot open staged package root {}: {error}",
            configured.display()
        )
    })?;
    let package = read_package_root(&package_root, target)?;
    let observed_build_id = read_binary_build_id(&package.binary)?;
    if observed_build_id != package.build_id {
        return Err(format!(
            "packaged binary build ID {observed_build_id} differs from manifest {}",
            package.build_id
        ));
    }
    Ok(package)
}

pub(super) fn load_for_host() -> Result<PackageArtifact, String> {
    load(host_target()?)
}

fn configured_package_root() -> Result<PathBuf, String> {
    std::env::var_os(PACKAGE_ROOT_ENVIRONMENT)
        .map(PathBuf::from)
        .ok_or_else(|| format!("a staged package root is required; set {PACKAGE_ROOT_ENVIRONMENT}"))
}

fn configured_source_binary() -> Result<PathBuf, String> {
    let configured = std::env::var_os(BINARY_ENVIRONMENT)
        .map(PathBuf::from)
        .ok_or_else(|| {
            format!("a release binary is required for staging; set {BINARY_ENVIRONMENT}")
        })?;
    configured.canonicalize().map_err(|error| {
        format!(
            "cannot open release binary {}: {error}",
            configured.display()
        )
    })
}

fn require_external_absolute_root(
    workspace: &Path,
    package_root: &Path,
    may_exist: bool,
) -> Result<(), String> {
    if !package_root.is_absolute() {
        return Err("staged package root must be absolute".to_owned());
    }
    if !may_exist && package_root.exists() {
        return Err(format!(
            "staged package root already exists: {}",
            package_root.display()
        ));
    }
    let canonical_workspace = workspace
        .canonicalize()
        .map_err(|error| format!("cannot resolve workspace root: {error}"))?;
    let comparison_root = if may_exist {
        package_root.canonicalize().map_err(|error| {
            format!(
                "cannot resolve staged package root {}: {error}",
                package_root.display()
            )
        })?
    } else {
        let parent = package_root.parent().ok_or_else(|| {
            format!(
                "staged package root has no parent: {}",
                package_root.display()
            )
        })?;
        let canonical_parent = parent.canonicalize().map_err(|error| {
            format!(
                "cannot resolve staged package parent {}: {error}",
                parent.display()
            )
        })?;
        let name = package_root.file_name().ok_or_else(|| {
            format!(
                "staged package root has no final name: {}",
                package_root.display()
            )
        })?;
        canonical_parent.join(name)
    };
    if comparison_root.starts_with(&canonical_workspace) {
        return Err("staged package root must be outside the checkout workspace".to_owned());
    }
    Ok(())
}

fn read_package_root(
    package_root: &Path,
    expected_target: &str,
) -> Result<PackageArtifact, String> {
    require_directory_inventory(
        package_root,
        &["bin", MANIFEST_NAME, "skills"],
        "package root",
    )?;
    require_plain_directory(&package_root.join("bin"), "package bin directory")?;
    require_plain_directory(&package_root.join("skills"), "package skills directory")?;
    require_directory_inventory(
        &package_root.join("skills"),
        &["claude-code", "codex"],
        "package skills directory",
    )?;
    for adapter in ["claude-code", "codex"] {
        let directory = package_root.join("skills").join(adapter);
        require_plain_directory(&directory, &format!("package {adapter} skill directory"))?;
        require_directory_inventory(
            &directory,
            &["SKILL.md"],
            &format!("package {adapter} skill directory"),
        )?;
    }

    let manifest_path = package_root.join(MANIFEST_NAME);
    require_regular_file(&manifest_path, "package manifest")?;
    let bytes = fs::read(&manifest_path)
        .map_err(|error| format!("cannot read package manifest: {error}"))?;
    let manifest = parse_manifest(&bytes)?;
    if manifest.target != expected_target {
        return Err(format!(
            "package target {} differs from requested target {expected_target}",
            manifest.target
        ));
    }

    let binary_relative = binary_relative_path(expected_target)?;
    if manifest.binary.path != binary_relative {
        return Err(format!(
            "package manifest binary path {} differs from {binary_relative}",
            manifest.binary.path
        ));
    }
    let binary_name = Path::new(binary_relative)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "packaged binary path has no UTF-8 file name".to_owned())?;
    require_directory_inventory(
        &package_root.join("bin"),
        &[binary_name],
        "package bin directory",
    )?;
    validate_file_identity(package_root, &manifest.binary, "packaged binary")?;
    require_executable(&package_root.join(binary_relative))?;

    let expected_skills = [
        ("codex", CODEX_SKILL_PATH),
        ("claude-code", CLAUDE_SKILL_PATH),
    ];
    if manifest.skills.len() != expected_skills.len() {
        return Err("package manifest must bind exactly two skill adapters".to_owned());
    }
    for (observed, (adapter, path)) in manifest.skills.iter().zip(expected_skills) {
        if observed.adapter != adapter || observed.file.path != path {
            return Err(format!(
                "package manifest skill binding {:?} differs from {adapter}:{path}",
                observed.adapter
            ));
        }
        validate_file_identity(package_root, &observed.file, "packaged skill")?;
    }

    Ok(PackageArtifact {
        root: package_root.to_path_buf(),
        binary: package_root.join(binary_relative),
        build_id: manifest.build_id,
    })
}

fn parse_manifest(bytes: &[u8]) -> Result<PackageManifest, String> {
    let value = parse_json("package manifest", bytes)?;
    let object = value
        .as_object()
        .ok_or_else(|| "package manifest root must be an object".to_owned())?;
    require_exact_keys(
        object,
        &["schemaVersion", "target", "buildId", "binary", "skills"],
        "package manifest",
    )?;
    require_string(&value, "/schemaVersion", MANIFEST_SCHEMA)?;
    let target = string_at(&value, "/target", "package target")?;
    binary_relative_path(&target)?;
    let build_id = string_at(&value, "/buildId", "package build ID")?;
    require_build_id(&build_id)?;
    let binary = parse_file_identity(
        value
            .get("binary")
            .ok_or_else(|| "package manifest omitted binary".to_owned())?,
        "package binary",
    )?;
    let skill_values = value
        .get("skills")
        .and_then(Value::as_array)
        .ok_or_else(|| "package manifest skills must be an array".to_owned())?;
    let mut skills = Vec::with_capacity(skill_values.len());
    for skill in skill_values {
        let object = skill
            .as_object()
            .ok_or_else(|| "package skill binding must be an object".to_owned())?;
        require_exact_keys(
            object,
            &["adapter", "path", "byteCount", "sha256"],
            "package skill binding",
        )?;
        skills.push(SkillIdentity {
            adapter: string_at(skill, "/adapter", "skill adapter")?,
            file: parse_file_identity(skill, "package skill")?,
        });
    }
    let manifest = PackageManifest {
        target,
        build_id,
        binary,
        skills,
    };
    if bytes != render_manifest(&manifest) {
        return Err("package manifest is not in canonical byte form".to_owned());
    }
    Ok(manifest)
}

fn parse_file_identity(value: &Value, label: &str) -> Result<FileIdentity, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{label} identity must be an object"))?;
    let allowed = if object.contains_key("adapter") {
        &["adapter", "path", "byteCount", "sha256"][..]
    } else {
        &["path", "byteCount", "sha256"][..]
    };
    require_exact_keys(object, allowed, label)?;
    let path = string_at(value, "/path", &format!("{label} path"))?;
    let byte_count = value
        .get("byteCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{label} byteCount must be an unsigned integer"))?;
    let sha256 = string_at(value, "/sha256", &format!("{label} SHA-256"))?;
    require_sha256(&sha256, label)?;
    Ok(FileIdentity {
        path,
        byte_count,
        sha256,
    })
}

fn render_manifest(manifest: &PackageManifest) -> Vec<u8> {
    let skills = manifest
        .skills
        .iter()
        .map(|skill| {
            format!(
                "{{\"adapter\":\"{}\",\"path\":\"{}\",\"byteCount\":{},\"sha256\":\"{}\"}}",
                skill.adapter, skill.file.path, skill.file.byte_count, skill.file.sha256
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"schemaVersion\":\"{MANIFEST_SCHEMA}\",\"target\":\"{}\",\"buildId\":\"{}\",\"binary\":{{\"path\":\"{}\",\"byteCount\":{},\"sha256\":\"{}\"}},\"skills\":[{skills}]}}\n",
        manifest.target,
        manifest.build_id,
        manifest.binary.path,
        manifest.binary.byte_count,
        manifest.binary.sha256
    )
    .into_bytes()
}

fn file_identity(root: &Path, relative: &str) -> Result<FileIdentity, String> {
    let path = root.join(relative);
    require_regular_file(&path, relative)?;
    let bytes = fs::read(&path).map_err(|error| format!("cannot read {relative}: {error}"))?;
    let byte_count =
        u64::try_from(bytes.len()).map_err(|_| format!("{relative} byte count exceeds u64"))?;
    Ok(FileIdentity {
        path: relative.to_owned(),
        byte_count,
        sha256: sha256_hex(&bytes),
    })
}

fn validate_file_identity(root: &Path, identity: &FileIdentity, label: &str) -> Result<(), String> {
    let observed = file_identity(root, &identity.path)?;
    if observed != *identity {
        return Err(format!(
            "{label} identity differs from package manifest: expected {identity:?}, observed {observed:?}"
        ));
    }
    Ok(())
}

fn read_binary_build_id(binary: &Path) -> Result<String, String> {
    let scratch = scratch_directory_for("package-build-id")?;
    fs::create_dir(&scratch)
        .map_err(|error| format!("cannot create package build-ID scratch directory: {error}"))?;
    let result = (|| {
        let output = run_binary(binary, &scratch, &["capabilities", "--format", "json"])?;
        expect_status(&output, Some(0), "packaged capabilities build ID")?;
        if !output.stderr.is_empty() {
            return Err(format!(
                "packaged capabilities build ID wrote stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let value = parse_json("packaged capabilities build ID", &output.stdout)?;
        let build_id = string_at(&value, "/scope/buildId", "packaged build ID")?;
        require_build_id(&build_id)?;
        if scratch.join(".lumin").exists() {
            return Err("packaged capabilities build-ID probe created repository state".to_owned());
        }
        Ok(build_id)
    })();
    let cleanup = fs::remove_dir_all(&scratch)
        .map_err(|error| format!("cannot remove package build-ID scratch directory: {error}"));
    match result {
        Ok(build_id) => {
            cleanup?;
            Ok(build_id)
        }
        Err(error) => Err(error),
    }
}

fn require_exact_keys(
    object: &serde_json::Map<String, Value>,
    expected: &[&str],
    label: &str,
) -> Result<(), String> {
    let mut observed = object.keys().map(String::as_str).collect::<Vec<_>>();
    let mut expected = expected.to_vec();
    observed.sort_unstable();
    expected.sort_unstable();
    if observed != expected {
        return Err(format!(
            "{label} fields differ: expected {expected:?}, observed {observed:?}"
        ));
    }
    Ok(())
}

fn require_string(value: &Value, pointer: &str, expected: &str) -> Result<(), String> {
    let observed = string_at(value, pointer, pointer)?;
    if observed != expected {
        return Err(format!(
            "package manifest field {pointer} was {observed:?}; expected {expected:?}"
        ));
    }
    Ok(())
}

fn string_at(value: &Value, pointer: &str, label: &str) -> Result<String, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{label} must be a string"))
}

fn require_build_id(value: &str) -> Result<(), String> {
    let Some(digest) = value.strip_prefix("build_") else {
        return Err(format!("package build ID is malformed: {value:?}"));
    };
    require_lower_hex(digest, 64, "package build ID")
}

fn require_sha256(value: &str, label: &str) -> Result<(), String> {
    require_lower_hex(value, 64, &format!("{label} SHA-256"))
}

fn require_lower_hex(value: &str, length: usize, label: &str) -> Result<(), String> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(format!(
            "{label} is not {length} lowercase hexadecimal bytes"
        ));
    }
    Ok(())
}

fn require_directory_inventory(
    directory: &Path,
    expected: &[&str],
    label: &str,
) -> Result<(), String> {
    let mut observed = fs::read_dir(directory)
        .map_err(|error| format!("cannot inspect {label} {}: {error}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot inspect {label} {}: {error}", directory.display()))?;
    observed.sort();
    let mut expected = expected
        .iter()
        .map(|name| OsString::from(*name))
        .collect::<Vec<_>>();
    expected.sort();
    if observed != expected {
        return Err(format!(
            "{label} inventory differs: expected {expected:?}, observed {observed:?}"
        ));
    }
    Ok(())
}

fn require_regular_file(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {label} {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{label} is not a regular file: {}", path.display()));
    }
    Ok(())
}

fn require_plain_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {label} {}: {error}", path.display()))?;
    if !metadata.file_type().is_dir() {
        return Err(format!("{label} is not a directory: {}", path.display()));
    }
    Ok(())
}

#[cfg(unix)]
fn require_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .map_err(|error| format!("cannot inspect packaged binary permissions: {error}"))?
        .permissions()
        .mode();
    if mode & 0o111 == 0 {
        return Err("packaged Linux binary is not executable".to_owned());
    }
    Ok(())
}

#[cfg(windows)]
fn require_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn binary_relative_path(target: &str) -> Result<&'static str, String> {
    match target {
        "windows-x64" => Ok("bin/lumin.exe"),
        "linux-x64" => Ok("bin/lumin"),
        _ => Err(format!("unsupported package target: {target}")),
    }
}

fn host_target() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("windows-x64"),
        ("linux", "x86_64") => Ok("linux-x64"),
        (os, architecture) => Err(format!(
            "package checks require Windows/Linux x86_64; current host is {os}-{architecture}"
        )),
    }
}

fn require_host_target(target: &str) -> Result<(), String> {
    let host = host_target()?;
    if target != host {
        return Err(format!(
            "package target {target} cannot be staged or executed on host {host}"
        ));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
pub(super) fn write_test_package(root: &Path, target: &str, binary: &[u8]) -> Result<(), String> {
    fs::create_dir_all(root.join("bin"))
        .map_err(|error| format!("cannot create test bin directory: {error}"))?;
    fs::create_dir_all(root.join("skills/codex"))
        .map_err(|error| format!("cannot create test Codex skill directory: {error}"))?;
    fs::create_dir_all(root.join("skills/claude-code"))
        .map_err(|error| format!("cannot create test Claude skill directory: {error}"))?;
    let binary_relative = binary_relative_path(target)?;
    fs::write(root.join(binary_relative), binary)
        .map_err(|error| format!("cannot write test binary: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            root.join(binary_relative),
            fs::Permissions::from_mode(0o755),
        )
        .map_err(|error| format!("cannot mark test binary executable: {error}"))?;
    }
    fs::write(root.join(CODEX_SKILL_PATH), b"codex")
        .map_err(|error| format!("cannot write test Codex skill: {error}"))?;
    fs::write(root.join(CLAUDE_SKILL_PATH), b"claude")
        .map_err(|error| format!("cannot write test Claude skill: {error}"))?;
    let manifest = PackageManifest {
        target: target.to_owned(),
        build_id: format!("build_{}", "0".repeat(64)),
        binary: file_identity(root, binary_relative)?,
        skills: vec![
            SkillIdentity {
                adapter: "codex".to_owned(),
                file: file_identity(root, CODEX_SKILL_PATH)?,
            },
            SkillIdentity {
                adapter: "claude-code".to_owned(),
                file: file_identity(root, CLAUDE_SKILL_PATH)?,
            },
        ],
    };
    fs::write(root.join(MANIFEST_NAME), render_manifest(&manifest))
        .map_err(|error| format!("cannot write test package manifest: {error}"))
}

#[cfg(test)]
pub(super) fn validate_test_package(root: &Path, target: &str) -> Result<(), String> {
    read_package_root(root, target).map(|_| ())
}
