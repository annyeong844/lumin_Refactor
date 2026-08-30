//! First-slice unavailable-capability ownership checks.

use std::path::Path;

const ABSENT_CAPABILITY_CRATES: &[(&str, &str, &str)] = &[
    ("lumin-rust", "lumin_rust", "crates/languages/rust"),
    ("lumin-clones", "lumin_clones", "crates/analyses/clones"),
    (
        "lumin-structure",
        "lumin_structure",
        "crates/analyses/structure",
    ),
    (
        "lumin-discipline",
        "lumin_discipline",
        "crates/analyses/discipline",
    ),
];

#[derive(Debug, Default)]
pub(crate) struct CapabilityAvailabilityResult {
    pub violations: Vec<String>,
    pub tool_errors: Vec<String>,
}

pub(crate) fn check_capability_availability(
    production_members: &[crate::metadata::WorkspaceMember],
    workspace_root: &Path,
) -> CapabilityAvailabilityResult {
    let mut result = CapabilityAvailabilityResult::default();
    let workspace_manifest = match std::fs::read_to_string(workspace_root.join("Cargo.toml")) {
        Ok(source) => source,
        Err(error) => {
            result
                .tool_errors
                .push(format!("cannot read workspace Cargo.toml: {error}"));
            return result;
        }
    };
    let production_manifests = production_members
        .iter()
        .filter_map(
            |member| match std::fs::read_to_string(&member.manifest_path) {
                Ok(source) => Some((member.name.as_str(), source)),
                Err(error) => {
                    result.tool_errors.push(format!(
                        "cannot read {} while checking unavailable capabilities: {error}",
                        member.manifest_path.display()
                    ));
                    None
                }
            },
        )
        .collect::<Vec<_>>();

    for (package, rust_name, relative) in ABSENT_CAPABILITY_CRATES {
        if workspace_root.join(relative).exists() {
            result.violations.push(format!(
                "CAPABILITY FALLBACK: unavailable first-slice owner path exists: {relative}"
            ));
        }
        if workspace_manifest.contains(package) || workspace_manifest.contains(relative) {
            result.violations.push(format!(
                "CAPABILITY FALLBACK: workspace manifest registers unavailable owner {package}"
            ));
        }
        for (member_name, manifest) in &production_manifests {
            if manifest.contains(package) || manifest.contains(rust_name) {
                result.violations.push(format!(
                    "CAPABILITY FALLBACK: {} references unavailable owner {package}",
                    member_name
                ));
            }
        }
    }

    result.violations.sort();
    result.violations.dedup();
    result.tool_errors.sort();
    result.tool_errors.dedup();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_slice_has_no_unavailable_capability_crate() -> Result<(), Box<dyn std::error::Error>> {
        let workspace_root = crate::metadata::find_workspace_root()?;
        let workspace = crate::metadata::inspect_workspace(&workspace_root)?;
        let result = check_capability_availability(&workspace.production_members, &workspace_root);
        assert!(result.tool_errors.is_empty(), "{:?}", result.tool_errors);
        assert!(result.violations.is_empty(), "{:?}", result.violations);
        Ok(())
    }
}
