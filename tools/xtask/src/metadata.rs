//! Workspace metadata parsing and dependency-edge policy enforcement.
//!
//! Runs `cargo metadata --format-version 1 --locked` (without `--no-deps`) and
//! validates workspace members, dependency edges, and third-party owner isolation.

use std::collections::{BTreeSet, HashMap};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    pub to: String,
    pub kind: DepKind,
    pub is_workspace_target: bool,
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
const NORMAL_EDGES: &[(&str, &str)] = &[
    // lumin-cli
    ("lumin-cli", "lumin-engine"),
    ("lumin-cli", "lumin-model"),
    ("lumin-cli", "lumin-protocol"),
    // lumin-engine
    ("lumin-engine", "lumin-dead"),
    ("lumin-engine", "lumin-evidence"),
    ("lumin-engine", "lumin-graph"),
    ("lumin-engine", "lumin-inventory"),
    ("lumin-engine", "lumin-js"),
    ("lumin-engine", "lumin-model"),
    ("lumin-engine", "lumin-resolve"),
    ("lumin-engine", "lumin-sfc"),
    ("lumin-engine", "lumin-store"),
    // lumin-protocol
    ("lumin-protocol", "lumin-evidence"),
    ("lumin-protocol", "lumin-model"),
    // lumin-store
    ("lumin-store", "lumin-evidence"),
    ("lumin-store", "lumin-model"),
    // lumin-dead
    ("lumin-dead", "lumin-evidence"),
    ("lumin-dead", "lumin-graph"),
    ("lumin-dead", "lumin-model"),
    // lumin-graph
    ("lumin-graph", "lumin-model"),
    // lumin-resolve
    ("lumin-resolve", "lumin-model"),
    // lumin-inventory
    ("lumin-inventory", "lumin-model"),
    // lumin-evidence
    ("lumin-evidence", "lumin-model"),
    // lumin-js
    ("lumin-js", "lumin-model"),
    // lumin-sfc
    ("lumin-sfc", "lumin-model"),
];

/// Canonical dev-dep allowlist: only lumin-store -> lumin-inventory.
const DEV_EDGES: &[(&str, &str)] = &[("lumin-store", "lumin-inventory")];

/// Build-dep allowlist: empty.
const BUILD_EDGES: &[(&str, &str)] = &[];

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

/// Run `cargo metadata` and validate workspace structure.
///
/// Returns `Err(String)` for tool/invocation failures (exit 2).
pub fn analyze_workspace(workspace_root: &Path) -> Result<MetadataResult, String> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--locked"])
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
                    edges.push(DirectEdge {
                        from: from_name.clone(),
                        to: to_name.clone(),
                        kind,
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
    let normal_set: BTreeSet<(&str, &str)> = NORMAL_EDGES.iter().map(|(f, t)| (*f, *t)).collect();
    let dev_set: BTreeSet<(&str, &str)> = DEV_EDGES.iter().map(|(f, t)| (*f, *t)).collect();
    let build_set: BTreeSet<(&str, &str)> = BUILD_EDGES.iter().map(|(f, t)| (*f, *t)).collect();

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
            let pair = (edge.from.as_str(), edge.to.as_str());
            let allowed = match edge.kind {
                DepKind::Normal => normal_set.contains(&pair),
                DepKind::Dev => dev_set.contains(&pair),
                DepKind::Build => build_set.contains(&pair),
                DepKind::Unknown => false, // handled above
            };

            if !allowed {
                violations.push(format!(
                    "FORBIDDEN edge: {} -> {} ({:?}) not in canonical allowlist",
                    edge.from, edge.to, edge.kind
                ));
            }
        } else {
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
            to: "lumin-engine".to_owned(),
            kind: DepKind::Unknown,
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
            to: "lumin-xtask".to_owned(),
            kind: DepKind::Normal,
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
            to: "lumin-engine".to_owned(),
            kind: DepKind::Normal,
            is_workspace_target: true,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert!(violations.is_empty());
    }

    #[test]
    fn validate_disallowed_normal_edge_fails() {
        let edges = vec![DirectEdge {
            from: "lumin-model".to_owned(),
            to: "lumin-store".to_owned(),
            kind: DepKind::Normal,
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
            to: "lumin-inventory".to_owned(),
            kind: DepKind::Dev,
            is_workspace_target: true,
        }];
        let mut violations = Vec::new();
        validate_edges(&edges, &mut violations);
        assert!(violations.is_empty());

        // Disallowed
        let edges = vec![DirectEdge {
            from: "lumin-cli".to_owned(),
            to: "lumin-inventory".to_owned(),
            kind: DepKind::Dev,
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
            to: "lumin-model".to_owned(),
            kind: DepKind::Build,
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
            to: "redb".to_owned(),
            kind: DepKind::Normal,
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
            to: "redb".to_owned(),
            kind: DepKind::Normal,
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
            to: "oxc_parser".to_owned(),
            kind: DepKind::Normal,
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
            to: "oxc_parser".to_owned(),
            kind: DepKind::Normal,
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
    fn third_party_edge_no_owner_rule_passes() {
        // serde_json is not in OWNER_DEPS, so any crate can use it
        let edges = vec![DirectEdge {
            from: "lumin-cli".to_owned(),
            to: "serde_json".to_owned(),
            kind: DepKind::Normal,
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
                "pkg": "redb-id",
                "dep_kinds": [{"kind": null}]
            }]
        })];
        let edges = extract_direct_edges(&nodes, &id_to_name, &member_names);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, "lumin-store");
        assert_eq!(edges[0].to, "redb");
        assert_eq!(edges[0].kind, DepKind::Normal);
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
                "pkg": "engine-id",
                "dep_kinds": [{"kind": null}]
            }]
        })];
        let edges = extract_direct_edges(&nodes, &id_to_name, &member_names);
        assert_eq!(edges.len(), 1);
        assert!(edges[0].is_workspace_target);
    }

    #[test]
    fn relative_display_uses_forward_slash() {
        let base = Path::new("C:\\Users\\test\\project");
        let target = Path::new("C:\\Users\\test\\project\\crates\\model\\src\\lib.rs");
        let result = relative_display(base, target);
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
