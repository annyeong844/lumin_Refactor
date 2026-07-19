use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    ConfigDocument, ConfigObservation, ConfigValue, Limitation, LogicalSourceId, PackageFact,
    PackageIdentity, PackageIdentityState, PackagePrivacy, RepoPath, SemanticConfigSnapshot,
    SourceSnapshot, WorkspaceFact, WorkspaceSource,
};

pub(crate) fn build(
    observations: BTreeMap<RepoPath, ConfigObservation>,
    sources: &[SourceSnapshot],
    limitations: &mut Vec<Limitation>,
) -> Result<SemanticConfigSnapshot, String> {
    let manifests = package_manifests(&observations);
    let pnpm_workspaces = pnpm_workspace_documents(&observations);
    let mut packages = manifests
        .iter()
        .map(|manifest| package_fact(manifest, limitations))
        .collect::<Result<Vec<_>, _>>()?;
    packages.sort_by(|left, right| left.root.cmp(&right.root));

    let package_roots = packages
        .iter()
        .map(|package| package.root.clone())
        .collect::<Vec<_>>();
    let workspaces = build_workspaces(&manifests, &pnpm_workspaces, &package_roots, limitations);
    assign_workspace_roots(&mut packages, &workspaces);
    reject_duplicate_identities(&mut packages, limitations);
    let source_packages = map_source_packages(sources, &packages);

    Ok(SemanticConfigSnapshot {
        observations,
        packages,
        workspaces,
        source_packages,
    })
}

fn pnpm_workspace_documents(
    observations: &BTreeMap<RepoPath, ConfigObservation>,
) -> Vec<&ConfigDocument> {
    observations
        .values()
        .filter_map(|observation| match observation {
            ConfigObservation::Present(document)
                if document.path.file_name_portable() == Some("pnpm-workspace.yaml") =>
            {
                Some(document)
            }
            _ => None,
        })
        .collect()
}

fn package_manifests(observations: &BTreeMap<RepoPath, ConfigObservation>) -> Vec<&ConfigDocument> {
    observations
        .values()
        .filter_map(|observation| match observation {
            ConfigObservation::Present(document)
                if document.path.file_name_portable() == Some("package.json") =>
            {
                Some(document)
            }
            _ => None,
        })
        .collect()
}

fn build_workspaces(
    manifests: &[&ConfigDocument],
    pnpm_documents: &[&ConfigDocument],
    package_roots: &[RepoPath],
    limitations: &mut Vec<Limitation>,
) -> Vec<WorkspaceFact> {
    let mut workspaces = Vec::new();
    let pnpm_roots = pnpm_documents
        .iter()
        .map(|document| document.path.parent().unwrap_or_else(RepoPath::empty))
        .collect::<BTreeSet<_>>();
    for document in pnpm_documents {
        let root = document.path.parent().unwrap_or_else(RepoPath::empty);
        workspaces.push(WorkspaceFact {
            members: pnpm_workspace_members(document, &root, package_roots, limitations),
            root,
            source: WorkspaceSource::PnpmWorkspace,
        });
    }
    for &manifest in manifests {
        let root = manifest.path.parent().unwrap_or_else(RepoPath::empty);
        if pnpm_roots.contains(&root) {
            continue;
        }
        let Some(workspaces_value) = manifest.root.get("workspaces") else {
            continue;
        };
        let patterns = match workspace_patterns(workspaces_value) {
            Ok(patterns) => patterns,
            Err(detail) => {
                limitations.push(Limitation::WorkspaceOwnershipUnsupported {
                    path: manifest.path.display_escaped(),
                    detail,
                });
                workspaces.push(WorkspaceFact {
                    root: root.clone(),
                    source: WorkspaceSource::PackageJson,
                    members: vec![root],
                });
                continue;
            }
        };
        let mut members = vec![root.clone()];
        for package_root in package_roots {
            if package_root == &root || !package_root.is_within(&root) {
                continue;
            }
            let Some(relative) = package_root.portable_relative_to(&root) else {
                continue;
            };
            if patterns
                .iter()
                .any(|pattern| workspace_pattern_matches(pattern, &relative))
            {
                members.push(package_root.clone());
            }
        }
        members.sort();
        members.dedup();
        workspaces.push(WorkspaceFact {
            root,
            source: WorkspaceSource::PackageJson,
            members,
        });
    }
    workspaces.sort_by(|left, right| left.root.cmp(&right.root));
    workspaces
}

fn pnpm_workspace_members(
    document: &ConfigDocument,
    root: &RepoPath,
    package_roots: &[RepoPath],
    limitations: &mut Vec<Limitation>,
) -> Vec<RepoPath> {
    let Some(entries) = document.root.as_object() else {
        return vec![root.clone()];
    };
    let mut patterns = None;
    for entry in entries {
        match entry.key.as_str() {
            "packages" => match pnpm_workspace_patterns(&entry.value) {
                Ok(value) => patterns = Some(value),
                Err(detail) => limitations.push(Limitation::WorkspaceOwnershipUnsupported {
                    path: document.path.display_escaped(),
                    detail,
                }),
            },
            "catalog" | "catalogs" => {
                let detail = if entry.value.as_object().is_some() {
                    format!("pnpm {} semantics are unsupported", entry.key)
                } else {
                    format!("pnpm {} field must be an object", entry.key)
                };
                limitations.push(Limitation::PnpmDependencySemanticsUnsupported {
                    path: document.path.display_escaped(),
                    detail,
                });
            }
            "packageConfigs" => {
                limitations.push(Limitation::PnpmDependencySemanticsUnsupported {
                    path: document.path.display_escaped(),
                    detail: "pnpm packageConfigs semantics are unsupported".to_owned(),
                });
            }
            _ => limitations.push(Limitation::WorkspaceOwnershipUnsupported {
                path: document.path.display_escaped(),
                detail: format!("unknown pnpm workspace field {}", entry.key),
            }),
        }
    }

    let mut members = vec![root.clone()];
    let Some(patterns) = patterns else {
        return members;
    };
    for package_root in package_roots {
        if package_root == root || !package_root.is_within(root) {
            continue;
        }
        let Some(relative) = package_root.portable_relative_to(root) else {
            continue;
        };
        let included = patterns
            .iter()
            .filter(|pattern| !pattern.exclusion)
            .any(|pattern| workspace_pattern_matches(&pattern.value, &relative));
        let excluded = patterns
            .iter()
            .filter(|pattern| pattern.exclusion)
            .any(|pattern| workspace_pattern_matches(&pattern.value, &relative));
        if included && !excluded {
            members.push(package_root.clone());
        }
    }
    members.sort();
    members.dedup();
    members
}

struct PnpmWorkspacePattern {
    value: String,
    exclusion: bool,
}

fn pnpm_workspace_patterns(value: &ConfigValue) -> Result<Vec<PnpmWorkspacePattern>, String> {
    let values = value
        .as_array()
        .ok_or_else(|| "pnpm packages field must be array<string>".to_owned())?;
    let mut patterns = Vec::new();
    for value in values {
        let raw = value
            .as_str()
            .ok_or_else(|| "pnpm workspace patterns must be strings".to_owned())?;
        let (exclusion, pattern) = match raw.strip_prefix('!') {
            Some(pattern) if !pattern.starts_with('!') => (true, pattern),
            Some(_) => return Err(format!("unsupported workspace pattern {raw}")),
            None => (false, raw),
        };
        validate_workspace_pattern(pattern)?;
        patterns.push(PnpmWorkspacePattern {
            value: pattern.to_owned(),
            exclusion,
        });
    }
    Ok(patterns)
}

fn assign_workspace_roots(packages: &mut [PackageFact], workspaces: &[WorkspaceFact]) {
    for package in packages {
        package.workspace_root = workspaces
            .iter()
            .filter(|workspace| workspace.members.contains(&package.root))
            .max_by_key(|workspace| workspace.root.components_len())
            .map(|workspace| workspace.root.clone());
    }
}

fn map_source_packages(
    sources: &[SourceSnapshot],
    packages: &[PackageFact],
) -> BTreeMap<LogicalSourceId, RepoPath> {
    sources
        .iter()
        .filter_map(|source| {
            packages
                .iter()
                .filter(|package| source.path.is_within(&package.root))
                .max_by_key(|package| package.root.components_len())
                .map(|package| (source.id.clone(), package.root.clone()))
        })
        .collect()
}

fn package_fact(
    manifest: &ConfigDocument,
    limitations: &mut Vec<Limitation>,
) -> Result<PackageFact, String> {
    if manifest.root.as_object().is_none() {
        return Err(format!(
            "package manifest root must be an object: {}",
            manifest.path.display_escaped()
        ));
    }
    let root = manifest.path.parent().unwrap_or_else(RepoPath::empty);
    let identity = match manifest.root.get("name") {
        None => PackageIdentityState::Missing,
        Some(ConfigValue::String(name)) if valid_package_name(name) => {
            PackageIdentityState::Valid(PackageIdentity::new(name.clone()))
        }
        Some(_) => {
            limitations.push(Limitation::PackageIdentityUnsupported {
                path: manifest.path.display_escaped(),
                detail: "package name does not match package-name.v1".to_owned(),
            });
            PackageIdentityState::Unsupported
        }
    };
    let privacy = match manifest.root.get("private") {
        None => PackagePrivacy::Unspecified,
        Some(ConfigValue::Boolean(true)) => PackagePrivacy::Private,
        Some(ConfigValue::Boolean(false)) => PackagePrivacy::Public,
        Some(_) => {
            limitations.push(Limitation::PackagePrivacyUnsupported {
                path: manifest.path.display_escaped(),
                detail: "package private field must be boolean".to_owned(),
            });
            PackagePrivacy::Unsupported
        }
    };
    for field in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        let Some(value) = manifest.root.get(field) else {
            continue;
        };
        let valid = value.as_object().is_some_and(|entries| {
            entries
                .iter()
                .all(|entry| matches!(entry.value, ConfigValue::String(_)))
        });
        if !valid {
            limitations.push(Limitation::DependencyOwnerAmbiguous {
                path: manifest.path.display_escaped(),
                detail: format!("package {field} field must be object<string,string>"),
            });
        }
    }
    Ok(PackageFact {
        root,
        manifest_path: manifest.path.clone(),
        identity,
        privacy,
        workspace_root: None,
    })
}

fn reject_duplicate_identities(packages: &mut [PackageFact], limitations: &mut Vec<Limitation>) {
    let mut groups = BTreeMap::<(RepoPath, String), Vec<usize>>::new();
    for (index, package) in packages.iter().enumerate() {
        let (Some(workspace_root), PackageIdentityState::Valid(identity)) =
            (&package.workspace_root, &package.identity)
        else {
            continue;
        };
        groups
            .entry((workspace_root.clone(), identity.as_str().to_owned()))
            .or_default()
            .push(index);
    }
    for ((_, identity), indexes) in groups {
        if indexes.len() < 2 {
            continue;
        }
        for index in indexes {
            limitations.push(Limitation::PackageIdentityUnsupported {
                path: packages[index].manifest_path.display_escaped(),
                detail: format!("duplicate workspace package identity {identity}"),
            });
            packages[index].identity = PackageIdentityState::Unsupported;
        }
    }
}

fn valid_package_name(value: &str) -> bool {
    if value.is_empty() || value.len() > 214 || !value.is_ascii() {
        return false;
    }
    if let Some(scoped) = value.strip_prefix('@') {
        let mut parts = scoped.split('/');
        let Some(scope) = parts.next() else {
            return false;
        };
        let Some(name) = parts.next() else {
            return false;
        };
        parts.next().is_none() && valid_package_segment(scope) && valid_package_segment(name)
    } else {
        !value.contains('/') && valid_package_segment(value)
    }
}

fn valid_package_segment(value: &str) -> bool {
    value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn workspace_patterns(value: &ConfigValue) -> Result<Vec<String>, String> {
    let values = match value {
        ConfigValue::Array(values) => values.as_slice(),
        ConfigValue::Object(_) => value
            .get("packages")
            .and_then(ConfigValue::as_array)
            .ok_or_else(|| "workspaces object must contain packages: array<string>".to_owned())?,
        _ => {
            return Err(
                "workspaces must be array<string> or object{packages:array<string>}".to_owned(),
            );
        }
    };
    let mut patterns = Vec::new();
    for value in values {
        let pattern = value
            .as_str()
            .ok_or_else(|| "workspace patterns must be strings".to_owned())?;
        if pattern.starts_with('!') {
            return Err(format!("unsupported workspace pattern {pattern}"));
        }
        validate_workspace_pattern(pattern)?;
        patterns.push(pattern.to_owned());
    }
    Ok(patterns)
}

fn validate_workspace_pattern(pattern: &str) -> Result<(), String> {
    if pattern.is_empty()
        || pattern.starts_with('/')
        || pattern.ends_with('/')
        || pattern.contains(['\\', '?', '[', ']', '{', '}', '(', ')'])
    {
        return Err(format!("unsupported workspace pattern {pattern}"));
    }
    for component in pattern.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || (component.contains("**") && component != "**")
        {
            return Err(format!("unsupported workspace pattern {pattern}"));
        }
    }
    Ok(())
}

fn workspace_pattern_matches(pattern: &str, path: &str) -> bool {
    let patterns = pattern.split('/').collect::<Vec<_>>();
    let components = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect::<Vec<_>>()
    };
    match_components(&patterns, &components)
}

fn match_components(patterns: &[&str], components: &[&str]) -> bool {
    let Some((first, rest)) = patterns.split_first() else {
        return components.is_empty();
    };
    if *first == "**" {
        return match_components(rest, components)
            || (!components.is_empty() && match_components(patterns, &components[1..]));
    }
    let Some((component, remaining)) = components.split_first() else {
        return false;
    };
    segment_matches(first, component) && match_components(rest, remaining)
}

fn segment_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut table = vec![vec![false; value.len() + 1]; pattern.len() + 1];
    table[0][0] = true;
    for pattern_index in 0..pattern.len() {
        for value_index in 0..=value.len() {
            if !table[pattern_index][value_index] {
                continue;
            }
            if pattern[pattern_index] == b'*' {
                table[pattern_index + 1][value_index] = true;
                if value_index < value.len() {
                    table[pattern_index][value_index + 1] = true;
                }
            } else if value_index < value.len() && pattern[pattern_index] == value[value_index] {
                table[pattern_index + 1][value_index + 1] = true;
            }
        }
    }
    table[pattern.len()][value.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_name_v1_accepts_only_the_frozen_ascii_subset() {
        assert!(valid_package_name("lumin-core"));
        assert!(valid_package_name("@acme/tsconfig"));
        for value in [
            "",
            "Uppercase",
            "@scope/",
            ".hidden",
            "name with space",
            "name%2fother",
            "name:tag",
            "name\\other",
        ] {
            assert!(!valid_package_name(value), "{value}");
        }
    }

    #[test]
    fn workspace_glob_supports_star_and_complete_double_star() {
        assert!(workspace_pattern_matches("packages/*", "packages/a"));
        assert!(workspace_pattern_matches("packages/**", "packages/group/a"));
        assert!(!workspace_pattern_matches("packages/*", "packages/group/a"));
    }
}
