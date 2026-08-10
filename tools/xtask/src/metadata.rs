//! Workspace metadata parsing and dependency-edge policy enforcement.
//!
//! Runs `cargo metadata --format-version 1 --all-features --locked` (without
//! `--no-deps`) and validates workspace members, dependency edges, and
//! third-party owner isolation.

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A resolved workspace member.
#[derive(Debug, Clone)]
pub struct WorkspaceMember {
    pub name: String,
    #[allow(dead_code)]
    pub manifest_path: PathBuf,
    pub src_root: PathBuf,
}

/// Dependency edge kind on the wire: null means normal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
    /// Wire value not recognized — always a hard violation.
    Unknown,
}

/// A direct dependency edge from a workspace member to any package (workspace or third-party).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DirectEdge {
    pub from: String,
    pub declared_name: String,
    pub to: String,
    pub kind: DepKind,
    pub target: Option<String>,
    pub is_workspace_target: bool,
}

type WorkspaceEdgePolicy = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
);
type ThirdPartyEdgePolicy = (
    &'static str,
    &'static str,
    &'static str,
    DepKind,
    Option<&'static str>,
);

const WINDOWS_TARGET: &str = "cfg(windows)";

const fn unconditional_workspace_edge(
    from: &'static str,
    declared_name: &'static str,
    to: &'static str,
) -> WorkspaceEdgePolicy {
    (from, declared_name, to, None)
}

const fn unconditional_third_party_edge(
    from: &'static str,
    declared_name: &'static str,
    to: &'static str,
    kind: DepKind,
) -> ThirdPartyEdgePolicy {
    (from, declared_name, to, kind, None)
}

const fn target_specific_third_party_edge(
    from: &'static str,
    declared_name: &'static str,
    to: &'static str,
    kind: DepKind,
    target: &'static str,
) -> ThirdPartyEdgePolicy {
    (from, declared_name, to, kind, Some(target))
}

/// Result of workspace metadata analysis.
pub struct MetadataResult {
    pub production_members: Vec<WorkspaceMember>,
    pub all_members: Vec<WorkspaceMember>,
    pub violations: Vec<String>,
    pub workspace_root: PathBuf,
}

/// The exact 12 production crate names.
const PRODUCTION_NAMES: &[&str] = &[
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

/// Canonical normal-dep allowlist derived from ARCH-000 §5 and current Cargo.toml files.
const NORMAL_EDGES: &[WorkspaceEdgePolicy] = &[
    // lumin-cli
    unconditional_workspace_edge("lumin-cli", "lumin_engine", "lumin-engine"),
    unconditional_workspace_edge("lumin-cli", "lumin_model", "lumin-model"),
    unconditional_workspace_edge("lumin-cli", "lumin_protocol", "lumin-protocol"),
    // lumin-engine
    unconditional_workspace_edge("lumin-engine", "lumin_dead", "lumin-dead"),
    unconditional_workspace_edge("lumin-engine", "lumin_evidence", "lumin-evidence"),
    unconditional_workspace_edge("lumin-engine", "lumin_graph", "lumin-graph"),
    unconditional_workspace_edge("lumin-engine", "lumin_inventory", "lumin-inventory"),
    unconditional_workspace_edge("lumin-engine", "lumin_js", "lumin-js"),
    unconditional_workspace_edge("lumin-engine", "lumin_model", "lumin-model"),
    unconditional_workspace_edge("lumin-engine", "lumin_resolve", "lumin-resolve"),
    unconditional_workspace_edge("lumin-engine", "lumin_sfc", "lumin-sfc"),
    unconditional_workspace_edge("lumin-engine", "lumin_store", "lumin-store"),
    // lumin-protocol
    unconditional_workspace_edge("lumin-protocol", "lumin_evidence", "lumin-evidence"),
    unconditional_workspace_edge("lumin-protocol", "lumin_model", "lumin-model"),
    // lumin-store
    unconditional_workspace_edge("lumin-store", "lumin_evidence", "lumin-evidence"),
    unconditional_workspace_edge("lumin-store", "lumin_model", "lumin-model"),
    // lumin-dead
    unconditional_workspace_edge("lumin-dead", "lumin_evidence", "lumin-evidence"),
    unconditional_workspace_edge("lumin-dead", "lumin_graph", "lumin-graph"),
    unconditional_workspace_edge("lumin-dead", "lumin_model", "lumin-model"),
    // lumin-graph
    unconditional_workspace_edge("lumin-graph", "lumin_model", "lumin-model"),
    // lumin-resolve
    unconditional_workspace_edge("lumin-resolve", "lumin_model", "lumin-model"),
    // lumin-inventory
    unconditional_workspace_edge("lumin-inventory", "lumin_model", "lumin-model"),
    // lumin-evidence
    unconditional_workspace_edge("lumin-evidence", "lumin_model", "lumin-model"),
    // lumin-js
    unconditional_workspace_edge("lumin-js", "lumin_model", "lumin-model"),
    // lumin-sfc
    unconditional_workspace_edge("lumin-sfc", "lumin_model", "lumin-model"),
];

/// Canonical dev-dep allowlist: only lumin-store -> lumin-inventory.
const DEV_EDGES: &[WorkspaceEdgePolicy] = &[unconditional_workspace_edge(
    "lumin-store",
    "lumin_inventory",
    "lumin-inventory",
)];

/// Build-dep allowlist: empty.
const BUILD_EDGES: &[WorkspaceEdgePolicy] = &[];

/// Exact production-to-third-party direct dependency allowlist.
///
/// `cargo metadata --all-features` exposes optional edges before this check. A
/// new crate, dependency kind, or target predicate must remain blocked until
/// its Rule 7 cost and ownership review adds the exact edge here.
const THIRD_PARTY_EDGES: &[ThirdPartyEdgePolicy] = &[
    // lumin-cli
    unconditional_third_party_edge("lumin-cli", "base64", "base64", DepKind::Dev),
    unconditional_third_party_edge("lumin-cli", "serde_json", "serde_json", DepKind::Dev),
    unconditional_third_party_edge("lumin-cli", "tempfile", "tempfile", DepKind::Dev),
    unconditional_third_party_edge("lumin-cli", "thiserror", "thiserror", DepKind::Normal),
    // lumin-engine
    unconditional_third_party_edge("lumin-engine", "rayon", "rayon", DepKind::Normal),
    unconditional_third_party_edge("lumin-engine", "tempfile", "tempfile", DepKind::Dev),
    unconditional_third_party_edge("lumin-engine", "thiserror", "thiserror", DepKind::Normal),
    // lumin-evidence
    unconditional_third_party_edge("lumin-evidence", "serde", "serde", DepKind::Normal),
    // lumin-inventory
    unconditional_third_party_edge("lumin-inventory", "ignore", "ignore", DepKind::Normal),
    unconditional_third_party_edge("lumin-inventory", "same_file", "same-file", DepKind::Normal),
    unconditional_third_party_edge(
        "lumin-inventory",
        "saphyr_parser",
        "saphyr-parser",
        DepKind::Normal,
    ),
    unconditional_third_party_edge("lumin-inventory", "serde", "serde", DepKind::Normal),
    unconditional_third_party_edge(
        "lumin-inventory",
        "serde_json",
        "serde_json",
        DepKind::Normal,
    ),
    unconditional_third_party_edge("lumin-inventory", "tempfile", "tempfile", DepKind::Dev),
    unconditional_third_party_edge("lumin-inventory", "thiserror", "thiserror", DepKind::Normal),
    target_specific_third_party_edge(
        "lumin-inventory",
        "winapi_util",
        "winapi-util",
        DepKind::Normal,
        WINDOWS_TARGET,
    ),
    target_specific_third_party_edge(
        "lumin-inventory",
        "windows_sys",
        "windows-sys",
        DepKind::Normal,
        WINDOWS_TARGET,
    ),
    // lumin-js
    unconditional_third_party_edge(
        "lumin-js",
        "oxc_allocator",
        "oxc_allocator",
        DepKind::Normal,
    ),
    unconditional_third_party_edge("lumin-js", "oxc_ast", "oxc_ast", DepKind::Normal),
    unconditional_third_party_edge(
        "lumin-js",
        "oxc_ast_visit",
        "oxc_ast_visit",
        DepKind::Normal,
    ),
    unconditional_third_party_edge("lumin-js", "oxc_parser", "oxc_parser", DepKind::Normal),
    unconditional_third_party_edge("lumin-js", "oxc_span", "oxc_span", DepKind::Normal),
    // lumin-model
    unconditional_third_party_edge("lumin-model", "serde", "serde", DepKind::Normal),
    unconditional_third_party_edge("lumin-model", "sha2", "sha2", DepKind::Normal),
    unconditional_third_party_edge("lumin-model", "thiserror", "thiserror", DepKind::Normal),
    // lumin-protocol
    unconditional_third_party_edge("lumin-protocol", "base64", "base64", DepKind::Normal),
    unconditional_third_party_edge("lumin-protocol", "serde", "serde", DepKind::Normal),
    unconditional_third_party_edge(
        "lumin-protocol",
        "serde_json",
        "serde_json",
        DepKind::Normal,
    ),
    unconditional_third_party_edge("lumin-protocol", "sha2", "sha2", DepKind::Normal),
    unconditional_third_party_edge("lumin-protocol", "thiserror", "thiserror", DepKind::Normal),
    // lumin-resolve
    unconditional_third_party_edge("lumin-resolve", "thiserror", "thiserror", DepKind::Normal),
    // lumin-sfc
    unconditional_third_party_edge("lumin-sfc", "thiserror", "thiserror", DepKind::Normal),
    // lumin-store
    unconditional_third_party_edge("lumin-store", "fs2", "fs2", DepKind::Normal),
    unconditional_third_party_edge("lumin-store", "getrandom", "getrandom", DepKind::Normal),
    unconditional_third_party_edge("lumin-store", "redb", "redb", DepKind::Normal),
    unconditional_third_party_edge("lumin-store", "serde", "serde", DepKind::Normal),
    unconditional_third_party_edge("lumin-store", "serde_json", "serde_json", DepKind::Normal),
    unconditional_third_party_edge("lumin-store", "tempfile", "tempfile", DepKind::Normal),
    unconditional_third_party_edge("lumin-store", "thiserror", "thiserror", DepKind::Normal),
    target_specific_third_party_edge(
        "lumin-store",
        "winapi_util",
        "winapi-util",
        DepKind::Normal,
        WINDOWS_TARGET,
    ),
    target_specific_third_party_edge(
        "lumin-store",
        "windows_sys",
        "windows-sys",
        DepKind::Normal,
        WINDOWS_TARGET,
    ),
];

/// Owner isolation rules for third-party crates.
/// (third-party prefix, allowed owner crate, allowed kinds)
const OWNER_DEPS: &[(&str, &str, &[DepKind])] = &[
    ("redb", "lumin-store", &[DepKind::Normal, DepKind::Build]),
    (
        "oxc_allocator",
        "lumin-js",
        &[DepKind::Normal, DepKind::Build],
    ),
    (
        "oxc_ast_visit",
        "lumin-js",
        &[DepKind::Normal, DepKind::Build],
    ),
    ("oxc_ast", "lumin-js", &[DepKind::Normal, DepKind::Build]),
    ("oxc_parser", "lumin-js", &[DepKind::Normal, DepKind::Build]),
    ("oxc_span", "lumin-js", &[DepKind::Normal, DepKind::Build]),
];

/// Ask Cargo which parent workspace owns this development tool.
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

/// Run `cargo metadata` and validate workspace structure.
///
/// Returns `Err(String)` for tool/invocation failures (exit 2).
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
        .map_err(|e| format!("failed to run cargo metadata: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cargo metadata failed: {stderr}"));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("failed to parse cargo metadata JSON: {e}"))?;

    let ws_root = json["workspace_root"]
        .as_str()
        .ok_or("missing workspace_root in metadata")?;
    let ws_root_path = PathBuf::from(ws_root);

    let packages = json["packages"]
        .as_array()
        .ok_or("missing packages array")?;

    let workspace_members_raw = json["workspace_members"]
        .as_array()
        .ok_or("missing workspace_members array")?;

    let workspace_member_ids: BTreeSet<String> = workspace_members_raw
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_owned()))
        .collect();

    // Build id->package info map
    let mut id_to_name: HashMap<String, String> = HashMap::new();
    let mut name_to_manifest: HashMap<String, PathBuf> = HashMap::new();

    for pkg in packages {
        let id = pkg["id"].as_str().unwrap_or_default().to_owned();
        let name = pkg["name"].as_str().unwrap_or_default().to_owned();
        let manifest = pkg["manifest_path"].as_str().unwrap_or_default();
        id_to_name.insert(id, name.clone());
        name_to_manifest.insert(name, PathBuf::from(manifest));
    }

    // Determine which packages are workspace members
    let mut member_names: BTreeSet<String> = BTreeSet::new();
    for member_id in &workspace_member_ids {
        if let Some(name) = id_to_name.get(member_id) {
            member_names.insert(name.clone());
        }
    }

    let mut violations = Vec::new();

    // Validate expected 13 members
    let expected_all: BTreeSet<&str> = PRODUCTION_NAMES
        .iter()
        .copied()
        .chain(std::iter::once("lumin-xtask"))
        .collect();

    let actual_names_ref: BTreeSet<&str> = member_names.iter().map(|s| s.as_str()).collect();

    if actual_names_ref != expected_all {
        let missing: Vec<&&str> = expected_all.difference(&actual_names_ref).collect();
        let extra: Vec<&&str> = actual_names_ref.difference(&expected_all).collect();
        if !missing.is_empty() {
            violations.push(format!("missing workspace members: {missing:?}"));
        }
        if !extra.is_empty() {
            violations.push(format!("unexpected workspace members: {extra:?}"));
        }
    }

    // Build members list with src roots
    let mut all_members = Vec::new();
    let mut production_members = Vec::new();

    for name in &member_names {
        let manifest = match name_to_manifest.get(name) {
            Some(m) => m.clone(),
            None => continue,
        };
        let src_root = manifest.parent().map(|p| p.join("src")).unwrap_or_default();
        let member = WorkspaceMember {
            name: name.clone(),
            manifest_path: manifest,
            src_root,
        };
        if PRODUCTION_NAMES.contains(&name.as_str()) {
            production_members.push(member.clone());
        }
        all_members.push(member);
    }

    // Parse resolve graph for edge validation
    let resolve = json["resolve"]["nodes"]
        .as_array()
        .ok_or("missing resolve.nodes")?;

    let direct_edges = extract_direct_edges(resolve, &id_to_name, &member_names);
    validate_edges(&direct_edges, &mut violations);
    validate_third_party_allowlist_completeness(&direct_edges, THIRD_PARTY_EDGES, &mut violations);

    Ok(MetadataResult {
        production_members,
        all_members,
        violations,
        workspace_root: ws_root_path,
    })
}

fn extract_direct_edges(
    nodes: &[serde_json::Value],
    id_to_name: &HashMap<String, String>,
    member_names: &BTreeSet<String>,
) -> Vec<DirectEdge> {
    let mut edges = Vec::new();
    for node in nodes {
        let node_id = node["id"].as_str().unwrap_or_default();
        let from_name = match id_to_name.get(node_id) {
            Some(n) if member_names.contains(n) => n.clone(),
            _ => continue,
        };

        let deps = match node["deps"].as_array() {
            Some(d) => d,
            None => continue,
        };

        for dep in deps {
            let declared_name = dep["name"].as_str().unwrap_or_default().to_owned();
            let dep_pkg_id = dep["pkg"].as_str().unwrap_or_default();
            let to_name = match id_to_name.get(dep_pkg_id) {
                Some(n) => n.clone(),
                None => continue,
            };
            let is_workspace_target = member_names.contains(&to_name);

            let dep_kinds = dep["dep_kinds"].as_array();
            if let Some(kinds) = dep_kinds {
                for kind_entry in kinds {
                    let kind = parse_dep_kind(kind_entry["kind"].as_str());
                    let target = kind_entry["target"].as_str().map(str::to_owned);
                    edges.push(DirectEdge {
                        from: from_name.clone(),
                        declared_name: declared_name.clone(),
                        to: to_name.clone(),
                        kind,
                        target,
                        is_workspace_target,
                    });
                }
            }
        }
    }
    edges
}

fn parse_dep_kind(wire: Option<&str>) -> DepKind {
    match wire {
        None | Some("") => DepKind::Normal,
        Some("dev") => DepKind::Dev,
        Some("build") => DepKind::Build,
        Some(_) => DepKind::Unknown,
    }
}

fn validate_edges(edges: &[DirectEdge], violations: &mut Vec<String>) {
    let normal_set = NORMAL_EDGES.iter().copied().collect::<BTreeSet<_>>();
    let dev_set = DEV_EDGES.iter().copied().collect::<BTreeSet<_>>();
    let build_set = BUILD_EDGES.iter().copied().collect::<BTreeSet<_>>();

    for edge in edges {
        // Unknown dep kind is always a hard violation.
        if edge.kind == DepKind::Unknown {
            violations.push(format!(
                "UNKNOWN DEP KIND: {} -> {} — unrecognized dependency kind on wire",
                edge.from, edge.to
            ));
            continue;
        }

        // Rule: no production crate may depend on lumin-xtask in any kind
        if edge.to == "lumin-xtask" && PRODUCTION_NAMES.contains(&edge.from.as_str()) {
            violations.push(format!(
                "FORBIDDEN: production crate {} depends on lumin-xtask ({:?})",
                edge.from, edge.kind
            ));
            continue;
        }

        // Skip edges from/to lumin-xtask for allowlist checking — xtask is dev-only
        if edge.from == "lumin-xtask" || edge.to == "lumin-xtask" {
            continue;
        }

        // Workspace-to-workspace edge: check canonical allowlist
        if edge.is_workspace_target {
            let identity = (
                edge.from.as_str(),
                edge.declared_name.as_str(),
                edge.to.as_str(),
                edge.target.as_deref(),
            );
            let allowed = match edge.kind {
                DepKind::Normal => normal_set.contains(&identity),
                DepKind::Dev => dev_set.contains(&identity),
                DepKind::Build => build_set.contains(&identity),
                DepKind::Unknown => false, // handled above
            };

            if !allowed {
                violations.push(format!(
                    "FORBIDDEN edge: {} -[{}]-> {} ({:?}, target={:?}) not in canonical allowlist",
                    edge.from, edge.declared_name, edge.to, edge.kind, edge.target
                ));
            }
        } else {
            if !THIRD_PARTY_EDGES
                .iter()
                .any(|(from, declared_name, to, kind, target)| {
                    edge.from == *from
                        && edge.declared_name == *declared_name
                        && edge.to == *to
                        && edge.kind == *kind
                        && edge.target.as_deref() == *target
                })
            {
                violations.push(format!(
                    "FORBIDDEN third-party edge: {} -[{}]-> {} ({:?}, target={:?}) not in canonical allowlist",
                    edge.from, edge.declared_name, edge.to, edge.kind, edge.target
                ));
            }

            // Third-party edge: check owner isolation rules
            for (dep_prefix, allowed_owner, allowed_kinds) in OWNER_DEPS {
                if edge.to.starts_with(dep_prefix)
                    && edge.from != *allowed_owner
                    && allowed_kinds.contains(&edge.kind)
                {
                    violations.push(format!(
                        "OWNER VIOLATION: {} uses third-party {} ({:?}) but only {} may own it",
                        edge.from, edge.to, edge.kind, allowed_owner
                    ));
                }
            }
        }
    }
}

fn validate_third_party_allowlist_completeness(
    edges: &[DirectEdge],
    allowlist: &[ThirdPartyEdgePolicy],
    violations: &mut Vec<String>,
) {
    let unique_allowlist = allowlist.iter().copied().collect::<BTreeSet<_>>();
    if unique_allowlist.len() != allowlist.len() {
        violations.push("DUPLICATE third-party dependency edge in canonical allowlist".to_owned());
    }

    for (from, declared_name, to, kind, target) in &unique_allowlist {
        if !edges.iter().any(|edge| {
            !edge.is_workspace_target
                && edge.from == *from
                && edge.declared_name == *declared_name
                && edge.to == *to
                && edge.kind == *kind
                && edge.target.as_deref() == *target
        }) {
            violations.push(format!(
                "STALE third-party edge: {from} -[{declared_name}]-> {to} ({kind:?}, target={target:?}) is allowlisted but absent"
            ));
        }
    }
}

/// Produce a relative path for diagnostics using `/` separators.
pub fn relative_display(base: &Path, target: &Path) -> String {
    match target.strip_prefix(base) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
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
    fn parse_dep_kind_null_is_normal() {
        assert_eq!(parse_dep_kind(None), DepKind::Normal);
        assert_eq!(parse_dep_kind(Some("")), DepKind::Normal);
    }

    #[test]
    fn parse_dep_kind_dev_and_build() {
        assert_eq!(parse_dep_kind(Some("dev")), DepKind::Dev);
        assert_eq!(parse_dep_kind(Some("build")), DepKind::Build);
    }

    #[test]
    fn parse_dep_kind_unknown_is_explicit_unknown() {
        assert_eq!(parse_dep_kind(Some("proc-macro")), DepKind::Unknown);
        assert_eq!(parse_dep_kind(Some("foobar")), DepKind::Unknown);
    }

    #[test]
    fn unknown_dep_kind_is_hard_violation() {
        let edges = vec![DirectEdge {
            from: "lumin-cli".to_owned(),
            declared_name: "lumin_engine".to_owned(),
            to: "lumin-engine".to_owned(),
            kind: DepKind::Unknown,
            target: None,
            is_workspace_target: true,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("UNKNOWN DEP KIND"));
    }

    #[test]
    fn validate_forbidden_production_to_xtask_edge() {
        let edges = vec![DirectEdge {
            from: "lumin-cli".to_owned(),
            declared_name: "lumin_xtask".to_owned(),
            to: "lumin-xtask".to_owned(),
            kind: DepKind::Normal,
            target: None,
            is_workspace_target: true,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("production crate lumin-cli depends on lumin-xtask"));
    }

    #[test]
    fn validate_allowed_normal_edge_passes() {
        let edges = vec![DirectEdge {
            from: "lumin-cli".to_owned(),
            declared_name: "lumin_engine".to_owned(),
            to: "lumin-engine".to_owned(),
            kind: DepKind::Normal,
            target: None,
            is_workspace_target: true,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert!(violations.is_empty());
    }

    #[test]
    fn workspace_target_predicate_is_part_of_edge_identity() {
        let edge = DirectEdge {
            from: "lumin-cli".to_owned(),
            declared_name: "lumin_engine".to_owned(),
            to: "lumin-engine".to_owned(),
            kind: DepKind::Normal,
            target: Some(WINDOWS_TARGET.to_owned()),
            is_workspace_target: true,
        };
        let mut violations = Vec::new();
        validate_edges(&[edge], &mut violations);
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("FORBIDDEN edge")),
            "a target-specific workspace edge reused an unconditional approval: {violations:?}"
        );
    }

    #[test]
    fn workspace_declared_dependency_name_is_part_of_edge_identity() {
        let edge = DirectEdge {
            from: "lumin-cli".to_owned(),
            declared_name: "engine_alias".to_owned(),
            to: "lumin-engine".to_owned(),
            kind: DepKind::Normal,
            target: None,
            is_workspace_target: true,
        };
        let mut violations = Vec::new();
        validate_edges(&[edge], &mut violations);
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("FORBIDDEN edge")),
            "a renamed workspace dependency reused the package approval: {violations:?}"
        );
    }

    #[test]
    fn validate_disallowed_normal_edge_fails() {
        let edges = vec![DirectEdge {
            from: "lumin-model".to_owned(),
            declared_name: "lumin_store".to_owned(),
            to: "lumin-store".to_owned(),
            kind: DepKind::Normal,
            target: None,
            is_workspace_target: true,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("FORBIDDEN edge"));
    }

    #[test]
    fn validate_dev_edge_only_store_to_inventory() {
        // Allowed
        let edges = vec![DirectEdge {
            from: "lumin-store".to_owned(),
            declared_name: "lumin_inventory".to_owned(),
            to: "lumin-inventory".to_owned(),
            kind: DepKind::Dev,
            target: None,
            is_workspace_target: true,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert!(violations.is_empty());

        // Disallowed
        let edges = vec![DirectEdge {
            from: "lumin-cli".to_owned(),
            declared_name: "lumin_inventory".to_owned(),
            to: "lumin-inventory".to_owned(),
            kind: DepKind::Dev,
            target: None,
            is_workspace_target: true,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert_eq!(violations.len(), 1);
    }

    #[test]
    fn validate_build_edges_always_forbidden() {
        let edges = vec![DirectEdge {
            from: "lumin-engine".to_owned(),
            declared_name: "lumin_model".to_owned(),
            to: "lumin-model".to_owned(),
            kind: DepKind::Build,
            target: None,
            is_workspace_target: true,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert_eq!(violations.len(), 1);
    }

    #[test]
    fn owner_violation_redb_outside_store() {
        let edges = vec![DirectEdge {
            from: "lumin-engine".to_owned(),
            declared_name: "redb".to_owned(),
            to: "redb".to_owned(),
            kind: DepKind::Normal,
            target: None,
            is_workspace_target: false,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert!(
            violations.iter().any(|v| v.contains("OWNER VIOLATION")),
            "expected OWNER VIOLATION in: {violations:?}"
        );
    }

    #[test]
    fn owner_allowed_redb_in_store() {
        let edges = vec![DirectEdge {
            from: "lumin-store".to_owned(),
            declared_name: "redb".to_owned(),
            to: "redb".to_owned(),
            kind: DepKind::Normal,
            target: None,
            is_workspace_target: false,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert!(
            violations.is_empty(),
            "unexpected violations: {violations:?}"
        );
    }

    #[test]
    fn owner_violation_oxc_outside_js() {
        let edges = vec![DirectEdge {
            from: "lumin-engine".to_owned(),
            declared_name: "oxc_parser".to_owned(),
            to: "oxc_parser".to_owned(),
            kind: DepKind::Normal,
            target: None,
            is_workspace_target: false,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert!(
            violations.iter().any(|v| v.contains("OWNER VIOLATION")),
            "expected OWNER VIOLATION in: {violations:?}"
        );
    }

    #[test]
    fn owner_allowed_oxc_in_js() {
        let edges = vec![DirectEdge {
            from: "lumin-js".to_owned(),
            declared_name: "oxc_parser".to_owned(),
            to: "oxc_parser".to_owned(),
            kind: DepKind::Normal,
            target: None,
            is_workspace_target: false,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert!(
            violations.is_empty(),
            "unexpected violations: {violations:?}"
        );
    }

    #[test]
    fn approved_third_party_edge_passes() {
        let edges = vec![DirectEdge {
            from: "lumin-protocol".to_owned(),
            declared_name: "serde_json".to_owned(),
            to: "serde_json".to_owned(),
            kind: DepKind::Normal,
            target: None,
            is_workspace_target: false,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert!(
            violations.is_empty(),
            "unexpected violations: {violations:?}"
        );
    }

    #[test]
    fn targeted_third_party_approval_rejects_other_target_scopes() {
        let approved = DirectEdge {
            from: "lumin-inventory".to_owned(),
            declared_name: "windows_sys".to_owned(),
            to: "windows-sys".to_owned(),
            kind: DepKind::Normal,
            target: Some(WINDOWS_TARGET.to_owned()),
            is_workspace_target: false,
        };
        let mut approved_violations = Vec::new();
        validate_edges(std::slice::from_ref(&approved), &mut approved_violations);
        assert!(approved_violations.is_empty(), "{approved_violations:?}");

        for target in [None, Some("cfg(unix)".to_owned())] {
            let mut changed = approved.clone();
            changed.target = target;
            let mut violations = Vec::new();
            validate_edges(&[changed], &mut violations);
            assert!(
                violations
                    .iter()
                    .any(|violation| violation.contains("FORBIDDEN third-party edge")),
                "a different target scope reused the Windows-only approval: {violations:?}"
            );
        }

        let mut renamed = approved;
        renamed.declared_name = "win".to_owned();
        let mut renamed_violations = Vec::new();
        validate_edges(&[renamed], &mut renamed_violations);
        assert!(
            renamed_violations
                .iter()
                .any(|violation| violation.contains("FORBIDDEN third-party edge")),
            "a renamed dependency reused the package approval: {renamed_violations:?}"
        );
    }

    #[test]
    fn unapproved_third_party_crate_or_kind_fails_closed() {
        let cases = [
            DirectEdge {
                from: "lumin-cli".to_owned(),
                declared_name: "duct".to_owned(),
                to: "duct".to_owned(),
                kind: DepKind::Normal,
                target: None,
                is_workspace_target: false,
            },
            DirectEdge {
                from: "lumin-cli".to_owned(),
                declared_name: "serde_json".to_owned(),
                to: "serde_json".to_owned(),
                kind: DepKind::Normal,
                target: None,
                is_workspace_target: false,
            },
            DirectEdge {
                from: "lumin-cli".to_owned(),
                declared_name: "thiserror".to_owned(),
                to: "thiserror".to_owned(),
                kind: DepKind::Build,
                target: None,
                is_workspace_target: false,
            },
        ];

        for edge in cases {
            let mut violations = Vec::new();
            validate_edges(&[edge], &mut violations);
            assert!(
                violations
                    .iter()
                    .any(|violation| violation.contains("FORBIDDEN third-party edge")),
                "expected an exact third-party edge violation in {violations:?}"
            );
        }
    }

    #[test]
    fn stale_or_duplicate_third_party_approval_fails_closed() {
        let edge = DirectEdge {
            from: "lumin-cli".to_owned(),
            declared_name: "thiserror".to_owned(),
            to: "thiserror".to_owned(),
            kind: DepKind::Normal,
            target: None,
            is_workspace_target: false,
        };
        let expected = [("lumin-cli", "thiserror", "thiserror", DepKind::Normal, None)];

        let mut violations = Vec::new();
        validate_third_party_allowlist_completeness(
            std::slice::from_ref(&edge),
            &expected,
            &mut violations,
        );
        assert!(violations.is_empty(), "{violations:?}");

        validate_third_party_allowlist_completeness(&[], &expected, &mut violations);
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("STALE third-party edge")),
            "expected a stale-edge violation in {violations:?}"
        );

        let mut duplicate_violations = Vec::new();
        validate_third_party_allowlist_completeness(
            &[edge],
            &[
                ("lumin-cli", "thiserror", "thiserror", DepKind::Normal, None),
                ("lumin-cli", "thiserror", "thiserror", DepKind::Normal, None),
            ],
            &mut duplicate_violations,
        );
        assert!(
            duplicate_violations
                .iter()
                .any(|violation| violation.contains("DUPLICATE third-party dependency edge")),
            "expected a duplicate-edge violation in {duplicate_violations:?}"
        );

        let targeted_edge = DirectEdge {
            from: "lumin-store".to_owned(),
            declared_name: "winapi_util".to_owned(),
            to: "winapi-util".to_owned(),
            kind: DepKind::Normal,
            target: Some(WINDOWS_TARGET.to_owned()),
            is_workspace_target: false,
        };
        let targeted_expected = [(
            "lumin-store",
            "winapi_util",
            "winapi-util",
            DepKind::Normal,
            Some(WINDOWS_TARGET),
        )];
        let mut targeted_violations = Vec::new();
        validate_third_party_allowlist_completeness(
            std::slice::from_ref(&targeted_edge),
            &targeted_expected,
            &mut targeted_violations,
        );
        assert!(targeted_violations.is_empty(), "{targeted_violations:?}");

        let mut unconditional_edge = targeted_edge;
        unconditional_edge.target = None;
        validate_third_party_allowlist_completeness(
            &[unconditional_edge],
            &targeted_expected,
            &mut targeted_violations,
        );
        assert!(
            targeted_violations
                .iter()
                .any(|violation| violation.contains("STALE third-party edge")),
            "a mismatched target scope satisfied the allowlist: {targeted_violations:?}"
        );
    }

    #[test]
    fn extract_direct_edges_includes_third_party() {
        // Simulated resolve node for lumin-store depending on redb
        let id_to_name: HashMap<String, String> = [
            ("store-id".to_owned(), "lumin-store".to_owned()),
            ("redb-id".to_owned(), "redb".to_owned()),
        ]
        .into_iter()
        .collect();
        let member_names: BTreeSet<String> = ["lumin-store".to_owned()].into_iter().collect();
        let nodes: Vec<serde_json::Value> = vec![serde_json::json!({
            "id": "store-id",
            "deps": [{
                "name": "redb",
                "pkg": "redb-id",
                "dep_kinds": [{"kind": null}]
            }]
        })];
        let edges = extract_direct_edges(&nodes, &id_to_name, &member_names);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, "lumin-store");
        assert_eq!(edges[0].declared_name, "redb");
        assert_eq!(edges[0].to, "redb");
        assert_eq!(edges[0].kind, DepKind::Normal);
        assert_eq!(edges[0].target, None);
        assert!(!edges[0].is_workspace_target);
    }

    #[test]
    fn extract_direct_edges_preserves_target_predicate() {
        let id_to_name: HashMap<String, String> = [
            ("inventory-id".to_owned(), "lumin-inventory".to_owned()),
            ("windows-id".to_owned(), "windows-sys".to_owned()),
        ]
        .into_iter()
        .collect();
        let member_names: BTreeSet<String> = ["lumin-inventory".to_owned()].into_iter().collect();
        let nodes: Vec<serde_json::Value> = vec![serde_json::json!({
            "id": "inventory-id",
            "deps": [{
                "name": "windows_sys",
                "pkg": "windows-id",
                "dep_kinds": [{"kind": null, "target": WINDOWS_TARGET}]
            }]
        })];
        let edges = extract_direct_edges(&nodes, &id_to_name, &member_names);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].declared_name, "windows_sys");
        assert_eq!(edges[0].target.as_deref(), Some(WINDOWS_TARGET));
        assert!(!edges[0].is_workspace_target);
    }

    #[test]
    fn extract_direct_edges_marks_workspace_target() {
        let id_to_name: HashMap<String, String> = [
            ("cli-id".to_owned(), "lumin-cli".to_owned()),
            ("engine-id".to_owned(), "lumin-engine".to_owned()),
        ]
        .into_iter()
        .collect();
        let member_names: BTreeSet<String> = ["lumin-cli".to_owned(), "lumin-engine".to_owned()]
            .into_iter()
            .collect();
        let nodes: Vec<serde_json::Value> = vec![serde_json::json!({
            "id": "cli-id",
            "deps": [{
                "name": "lumin_engine",
                "pkg": "engine-id",
                "dep_kinds": [{"kind": null}]
            }]
        })];
        let edges = extract_direct_edges(&nodes, &id_to_name, &member_names);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].declared_name, "lumin_engine");
        assert_eq!(edges[0].target, None);
        assert!(edges[0].is_workspace_target);
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
    fn relative_display_outside_base_uses_forward_slash() {
        let base = Path::new("/home/user/project");
        let target = Path::new("/other/path/file.rs");
        let result = relative_display(base, target);
        assert!(!result.contains('\\'));
    }

    #[test]
    fn expected_production_count_is_twelve() {
        assert_eq!(PRODUCTION_NAMES.len(), 12);
    }
}
