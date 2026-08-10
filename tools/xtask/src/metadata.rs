//! Workspace metadata acquisition and dependency-policy orchestration.

mod dependency_policy;

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct WorkspaceMember {
    pub name: String,
    #[allow(dead_code)]
    pub manifest_path: PathBuf,
    pub src_root: PathBuf,
}

pub struct MetadataResult {
    pub production_members: Vec<WorkspaceMember>,
    pub all_members: Vec<WorkspaceMember>,
    pub violations: Vec<String>,
    pub workspace_root: PathBuf,
}

pub(super) const PRODUCTION_NAMES: &[&str] = &[
    "lumin-cli",
    "lumin-dead",
    "lumin-engine",
    "lumin-evidence",
    "lumin-graph",
    "lumin-inventory",
    "lumin-js",
    "lumin-model",
    "lumin-protocol",
    "lumin-resolve",
    "lumin-sfc",
    "lumin-store",
];

pub fn find_workspace_root() -> Result<PathBuf, String> {
    locate_workspace_root(Path::new(env!("CARGO_MANIFEST_DIR")))
}

fn locate_workspace_root(manifest_dir: &Path) -> Result<PathBuf, String> {
    let member_manifest = manifest_dir.join("Cargo.toml");
    let member_manifest = std::fs::canonicalize(&member_manifest)
        .map_err(|error| format!("cannot resolve {}: {error}", member_manifest.display()))?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .args([
            "locate-project",
            "--workspace",
            "--message-format",
            "plain",
            "--manifest-path",
        ])
        .arg(&member_manifest)
        .output()
        .map_err(|error| format!("failed to run cargo locate-project: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cargo locate-project failed: {}", stderr.trim()));
    }

    let workspace_manifest = std::str::from_utf8(&output.stdout)
        .map_err(|error| format!("cargo locate-project returned non-UTF-8 output: {error}"))?
        .trim();
    if workspace_manifest.is_empty() {
        return Err("cargo locate-project returned an empty workspace manifest".to_owned());
    }
    let workspace_manifest = PathBuf::from(workspace_manifest);
    let workspace_manifest = std::fs::canonicalize(&workspace_manifest).map_err(|error| {
        format!(
            "cannot resolve Cargo workspace manifest {}: {error}",
            workspace_manifest.display()
        )
    })?;
    if workspace_manifest == member_manifest {
        return Err(format!(
            "{} is not attached to a parent Cargo workspace",
            member_manifest.display()
        ));
    }
    let workspace_root = workspace_manifest.parent().ok_or_else(|| {
        format!(
            "Cargo workspace manifest has no parent: {}",
            workspace_manifest.display()
        )
    })?;
    if !member_manifest.starts_with(workspace_root) {
        return Err(format!(
            "Cargo workspace {} does not contain {}",
            workspace_root.display(),
            member_manifest.display()
        ));
    }
    Ok(workspace_root.to_path_buf())
}

pub fn analyze_workspace(workspace_root: &Path) -> Result<MetadataResult, String> {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--all-features",
            "--locked",
        ])
        .current_dir(workspace_root)
        .output()
        .map_err(|error| format!("failed to run cargo metadata: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cargo metadata failed: {stderr}"));
    }

    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("failed to parse cargo metadata JSON: {error}"))?;
    analyze_metadata(&metadata, workspace_root)
}

fn analyze_metadata(
    metadata: &serde_json::Value,
    requested_root: &Path,
) -> Result<MetadataResult, String> {
    let metadata_root = metadata
        .get("workspace_root")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "missing workspace_root in metadata".to_owned())?;
    let metadata_root = PathBuf::from(metadata_root);
    let canonical_requested = std::fs::canonicalize(requested_root).map_err(|error| {
        format!(
            "cannot canonicalize requested workspace {}: {error}",
            requested_root.display()
        )
    })?;
    let canonical_metadata = std::fs::canonicalize(&metadata_root).map_err(|error| {
        format!(
            "cannot canonicalize metadata workspace {}: {error}",
            metadata_root.display()
        )
    })?;
    if canonical_requested != canonical_metadata {
        return Err(format!(
            "cargo metadata workspace mismatch: requested {} resolved {}",
            canonical_requested.display(),
            canonical_metadata.display()
        ));
    }

    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "missing packages array".to_owned())?;
    let member_values = metadata
        .get("workspace_members")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "missing workspace_members array".to_owned())?;

    let mut package_by_id = HashMap::new();
    for package in packages {
        let id = package
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "metadata package has no string id".to_owned())?;
        if package_by_id.insert(id.to_owned(), package).is_some() {
            return Err(format!("duplicate package id in metadata: {id}"));
        }
    }

    let mut violations = Vec::new();
    let mut member_ids = BTreeSet::new();
    let mut member_names = BTreeSet::new();
    for value in member_values {
        let id = value
            .as_str()
            .ok_or_else(|| "workspace member id is not a string".to_owned())?;
        if !member_ids.insert(id.to_owned()) {
            violations.push(format!("duplicate workspace member id: {id}"));
        }
        let package = package_by_id
            .get(id)
            .copied()
            .ok_or_else(|| format!("workspace member package missing: {id}"))?;
        let name = package
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("workspace member has no string name: {id}"))?;
        if !member_names.insert(name.to_owned()) {
            violations.push(format!("duplicate workspace member name: {name}"));
        }
    }

    let expected_names = PRODUCTION_NAMES
        .iter()
        .copied()
        .chain(std::iter::once("lumin-xtask"))
        .collect::<BTreeSet<_>>();
    let actual_names = member_names
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    for missing in expected_names.difference(&actual_names) {
        violations.push(format!("missing workspace member: {missing}"));
    }
    for unexpected in actual_names.difference(&expected_names) {
        violations.push(format!("unexpected workspace member: {unexpected}"));
    }

    let mut all_members = Vec::new();
    for id in &member_ids {
        let package = package_by_id
            .get(id)
            .copied()
            .ok_or_else(|| format!("workspace member package missing: {id}"))?;
        let name = package
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("workspace member has no string name: {id}"))?;
        let manifest = package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("workspace member has no manifest path: {name}"))?;
        let manifest_path = PathBuf::from(manifest);
        let src_root = manifest_path
            .parent()
            .map(|parent| parent.join("src"))
            .ok_or_else(|| format!("workspace manifest has no parent: {manifest}"))?;
        all_members.push(WorkspaceMember {
            name: name.to_owned(),
            manifest_path,
            src_root,
        });
    }
    all_members.sort_by(|left, right| left.name.cmp(&right.name));
    let production_members = all_members
        .iter()
        .filter(|member| PRODUCTION_NAMES.contains(&member.name.as_str()))
        .cloned()
        .collect::<Vec<_>>();

    dependency_policy::validate_dependency_surface(metadata, &canonical_metadata, &mut violations)?;

    Ok(MetadataResult {
        production_members,
        all_members,
        violations,
        workspace_root: canonical_metadata,
    })
}

pub fn relative_display(base: &Path, target: &Path) -> String {
    match target.strip_prefix(base) {
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => target.to_string_lossy().replace('\\', "/"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_comes_from_cargo_ownership() -> Result<(), Box<dyn std::error::Error>> {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let expected = std::fs::canonicalize(
            manifest_dir
                .parent()
                .and_then(Path::parent)
                .ok_or("xtask manifest directory has no workspace parent")?,
        )?;
        assert_eq!(find_workspace_root()?, expected);
        Ok(())
    }

    #[test]
    fn workspace_text_in_package_metadata_is_not_a_workspace() -> std::io::Result<()> {
        let decoy = tempfile::tempdir()?;
        std::fs::create_dir_all(decoy.path().join("src"))?;
        std::fs::write(
            decoy.path().join("Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"workspace-text-decoy\"\n",
                "version = \"0.0.0\"\n",
                "edition = \"2024\"\n",
                "description = \"[workspace]\"\n",
            ),
        )?;
        std::fs::write(decoy.path().join("src/lib.rs"), "")?;
        let member = decoy.path().join("tools/xtask");
        std::fs::create_dir_all(member.join("src"))?;
        std::fs::write(
            member.join("Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"workspace-text-decoy-xtask\"\n",
                "version = \"0.0.0\"\n",
                "edition = \"2024\"\n",
            ),
        )?;
        std::fs::write(member.join("src/main.rs"), "fn main() {}\n")?;

        let error = match locate_workspace_root(&member) {
            Err(error) => error,
            Ok(root) => {
                return Err(std::io::Error::other(format!(
                    "decoy workspace was accepted as {}",
                    root.display()
                )));
            }
        };
        assert!(
            error.contains("is not attached to a parent Cargo workspace"),
            "unexpected decoy rejection: {error}"
        );
        Ok(())
    }

    #[test]
    fn relative_display_uses_forward_slash() {
        let base = PathBuf::from("project");
        let target = base.join("crates").join("model").join("src").join("lib.rs");
        let result = relative_display(&base, &target);
        assert_eq!(result, "crates/model/src/lib.rs");
        assert!(!result.contains('\\'));
    }

    #[test]
    fn expected_production_count_is_twelve() {
        assert_eq!(PRODUCTION_NAMES.len(), 12);
    }
}
