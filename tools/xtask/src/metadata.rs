//! Static workspace layout used by the structural architecture checker.
//!
//! Dependency admission belongs to the pre-Cargo Python guard. This module
//! deliberately does not invoke Cargo or reconstruct dependency evidence.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WorkspaceMember {
    pub name: String,
    #[allow(dead_code)]
    pub manifest_path: PathBuf,
    pub src_root: PathBuf,
}

pub struct WorkspaceLayout {
    pub production_members: Vec<WorkspaceMember>,
    pub all_members: Vec<WorkspaceMember>,
    pub violations: Vec<String>,
    pub workspace_root: PathBuf,
}

const PRODUCTION_MEMBERS: &[(&str, &str)] = &[
    ("lumin-cli", "crates/application/cli"),
    ("lumin-dead", "crates/analyses/dead-code"),
    ("lumin-engine", "crates/application/engine"),
    ("lumin-evidence", "crates/foundation/evidence"),
    ("lumin-graph", "crates/graph/symbols"),
    ("lumin-inventory", "crates/source/inventory"),
    ("lumin-js", "crates/languages/js"),
    ("lumin-model", "crates/foundation/model"),
    ("lumin-protocol", "crates/application/protocol"),
    ("lumin-resolve", "crates/graph/resolve"),
    ("lumin-sfc", "crates/languages/sfc"),
    ("lumin-store", "crates/application/store"),
];

const DEVELOPMENT_MEMBERS: &[(&str, &str)] = &[("lumin-xtask", "tools/xtask")];

pub fn find_workspace_root() -> Result<PathBuf, String> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "xtask manifest directory has no workspace parent".to_owned())?;
    let root = std::fs::canonicalize(root)
        .map_err(|error| format!("cannot resolve workspace root {}: {error}", root.display()))?;
    let manifest = root.join("Cargo.toml");
    if !manifest.is_file() {
        return Err(format!(
            "workspace manifest is missing: {}",
            manifest.display()
        ));
    }
    Ok(root)
}

pub fn inspect_workspace(workspace_root: &Path) -> Result<WorkspaceLayout, String> {
    let workspace_root = std::fs::canonicalize(workspace_root).map_err(|error| {
        format!(
            "cannot canonicalize workspace root {}: {error}",
            workspace_root.display()
        )
    })?;
    let mut violations = Vec::new();
    let mut production_members = collect_members(
        &workspace_root,
        PRODUCTION_MEMBERS,
        "production",
        &mut violations,
    );
    let development_members = collect_members(
        &workspace_root,
        DEVELOPMENT_MEMBERS,
        "development",
        &mut violations,
    );
    production_members.sort_by(|left, right| left.name.cmp(&right.name));
    let mut all_members = production_members.clone();
    all_members.extend(development_members);
    all_members.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(WorkspaceLayout {
        production_members,
        all_members,
        violations,
        workspace_root,
    })
}

fn collect_members(
    root: &Path,
    members: &[(&str, &str)],
    class: &str,
    violations: &mut Vec<String>,
) -> Vec<WorkspaceMember> {
    members
        .iter()
        .filter_map(|(name, relative)| {
            let directory = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
            let manifest = directory.join("Cargo.toml");
            let src = directory.join("src");
            match member_paths(root, name, class, &manifest, &src) {
                Ok(member) => Some(member),
                Err(violation) => {
                    violations.push(violation);
                    None
                }
            }
        })
        .collect()
}

fn member_paths(
    root: &Path,
    name: &str,
    class: &str,
    manifest: &Path,
    src: &Path,
) -> Result<WorkspaceMember, String> {
    let manifest_path = std::fs::canonicalize(manifest)
        .map_err(|error| format!("{class} member {name} manifest is unavailable: {error}"))?;
    let src_root = std::fs::canonicalize(src)
        .map_err(|error| format!("{class} member {name} source root is unavailable: {error}"))?;
    if !manifest_path.starts_with(root) || !src_root.starts_with(root) {
        return Err(format!("{class} member {name} escapes the workspace root"));
    }
    if !manifest_path.is_file() || !src_root.is_dir() {
        return Err(format!(
            "{class} member {name} has an invalid manifest or source root"
        ));
    }
    Ok(WorkspaceMember {
        name: (*name).to_owned(),
        manifest_path,
        src_root,
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
    fn workspace_root_is_the_static_xtask_owner() -> Result<(), Box<dyn std::error::Error>> {
        let expected = std::fs::canonicalize(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(Path::parent)
                .ok_or("xtask manifest directory has no workspace parent")?,
        )?;
        assert_eq!(find_workspace_root()?, expected);
        Ok(())
    }

    #[test]
    fn structural_layout_contains_twelve_products_and_one_tool()
    -> Result<(), Box<dyn std::error::Error>> {
        let layout = inspect_workspace(&find_workspace_root()?)?;
        assert!(layout.violations.is_empty(), "{:?}", layout.violations);
        assert_eq!(layout.production_members.len(), 12);
        assert_eq!(layout.all_members.len(), 13);
        assert_eq!(
            layout.all_members.last().map(|member| member.name.as_str()),
            Some("lumin-xtask")
        );
        Ok(())
    }

    #[test]
    fn structural_layout_never_spawns_cargo_or_the_bootstrap() {
        let source = include_str!("metadata.rs");
        for forbidden in [
            ["Command::", "new"].concat(),
            ["std::", "process"].concat(),
            ["source_", "provenance.py"].concat(),
        ] {
            assert!(!source.contains(&forbidden), "found {forbidden}");
        }
    }

    #[test]
    fn relative_display_uses_forward_slashes() {
        let base = PathBuf::from("project");
        let target = base.join("crates").join("model").join("src").join("lib.rs");
        assert_eq!(relative_display(&base, &target), "crates/model/src/lib.rs");
    }
}
