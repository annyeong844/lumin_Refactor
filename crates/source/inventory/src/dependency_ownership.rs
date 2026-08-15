use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use lumin_model::{
    ConfigObservation, ConfigSyntax, DependencyIntent, DependencyOwnerFact, Limitation,
    PackageFact, PackageIdentityState, RepoPath, SemanticConfigSnapshot, WorkspaceFact, digest_hex,
};

use crate::{
    InventoryError, SemanticPolicyInput, SemanticPolicyState, capture_config, native_relative,
    observe_config_input_identity,
};

const LOCKFILE_NAMES: [&str; 6] = [
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
];

pub(crate) fn capture_owner_candidates(
    root: &Path,
    intents: &[DependencyIntent],
    observations: &mut BTreeMap<RepoPath, ConfigObservation>,
    consulted_config_paths: &mut Vec<RepoPath>,
    limitations: &mut Vec<Limitation>,
) -> Result<(), InventoryError> {
    let mut candidates = BTreeMap::<RepoPath, ConfigSyntax>::new();
    for intent in intents {
        for directory in ancestor_directories(&intent.path) {
            candidates.insert(join(&directory, "package.json")?, ConfigSyntax::StrictJson);
            candidates.insert(
                join(&directory, "pnpm-workspace.yaml")?,
                ConfigSyntax::RestrictedYaml,
            );
        }
    }
    for (path, syntax) in candidates {
        if observations.contains_key(&path) {
            continue;
        }
        let capture = capture_config(root, &path, syntax)?;
        if let Some(limitation) = capture.limitation {
            limitations.push(limitation);
        }
        consulted_config_paths.push(path.clone());
        observations.insert(path, capture.observation);
    }
    Ok(())
}

pub(crate) fn reservation_paths(
    intents: &[DependencyIntent],
) -> Result<Vec<RepoPath>, InventoryError> {
    let mut candidates = BTreeMap::<RepoPath, ()>::new();
    for intent in intents {
        for directory in ancestor_directories(&intent.path) {
            for name in ["package.json", "pnpm-workspace.yaml"] {
                candidates.insert(join(&directory, name)?, ());
            }
        }
    }
    Ok(candidates.into_keys().collect())
}

pub(crate) struct DependencyOwnershipPlan {
    owners: Vec<PendingOwner>,
    input_paths: Vec<RepoPath>,
}

impl DependencyOwnershipPlan {
    pub(crate) fn input_paths(&self) -> &[RepoPath] {
        &self.input_paths
    }
}

struct PendingOwner {
    intent: DependencyIntent,
    package_root: RepoPath,
    manifest_path: RepoPath,
    workspace_root: RepoPath,
}

pub(crate) fn plan(
    root: &Path,
    intents: &[DependencyIntent],
    config: &SemanticConfigSnapshot,
    limitations: &mut Vec<Limitation>,
) -> Result<DependencyOwnershipPlan, InventoryError> {
    let mut intents = intents.to_vec();
    intents.sort();
    intents.dedup();

    let mut owners = Vec::new();
    let mut input_paths = BTreeMap::<RepoPath, ()>::new();
    for intent in intents {
        let package = match nearest_package(config, &intent)? {
            PackageSelection::Complete(package) => package,
            PackageSelection::Ambiguous(detail) => {
                limitations.push(Limitation::DependencyOwnerAmbiguous {
                    path: intent.path.display_escaped(),
                    detail,
                });
                continue;
            }
        };
        if !package_supports_dependency_ownership(package)
            || workspace_ownership_is_unsupported(config, package)
        {
            continue;
        }
        let workspace_root = match selected_workspace(config, package)? {
            Some(workspace)
                if workspace.ownership_supported && workspace.dependency_ownership_supported =>
            {
                workspace.root.clone()
            }
            Some(_) => continue,
            None => package.root.clone(),
        };
        for path in lockfile_search_paths(root, &package.root, &workspace_root)? {
            input_paths.insert(path, ());
        }
        owners.push(PendingOwner {
            intent,
            package_root: package.root.clone(),
            manifest_path: package.manifest_path.clone(),
            workspace_root,
        });
    }
    Ok(DependencyOwnershipPlan {
        owners,
        input_paths: input_paths.into_keys().collect(),
    })
}

pub(crate) fn capture(
    root: &Path,
    plan: DependencyOwnershipPlan,
    config: &mut SemanticConfigSnapshot,
    limitations: &mut Vec<Limitation>,
) -> Result<Vec<SemanticPolicyInput>, InventoryError> {
    let mut lockfile_inputs = BTreeMap::<RepoPath, SemanticPolicyInput>::new();
    for path in &plan.input_paths {
        lockfile_inputs.insert(path.clone(), capture_lockfile(root, path)?);
    }
    let mut facts = Vec::new();
    for owner in plan.owners {
        let lockfile_path = select_lockfile(&owner, &lockfile_inputs, limitations)?;
        if let LockfileSelection::Complete(lockfile_path) = lockfile_path {
            facts.push(owner_fact(owner, lockfile_path));
        }
    }
    facts.sort();
    facts.dedup();
    config.dependency_owners = facts;
    Ok(lockfile_inputs.into_values().collect())
}

fn owner_fact(owner: PendingOwner, lockfile_path: Option<RepoPath>) -> DependencyOwnerFact {
    DependencyOwnerFact {
        intent: owner.intent,
        package_root: owner.package_root,
        manifest_path: owner.manifest_path,
        lockfile_path,
    }
}

enum PackageSelection<'a> {
    Complete(&'a PackageFact),
    Ambiguous(String),
}

fn nearest_package<'a>(
    config: &'a SemanticConfigSnapshot,
    intent: &DependencyIntent,
) -> Result<PackageSelection<'a>, InventoryError> {
    for directory in ancestor_directories(&intent.path) {
        let manifest_path = join(&directory, "package.json")?;
        match config.observations.get(&manifest_path) {
            Some(ConfigObservation::Present { .. }) => {
                return config
                    .packages
                    .iter()
                    .find(|package| package.manifest_path == manifest_path)
                    .map(PackageSelection::Complete)
                    .ok_or_else(|| {
                        InventoryError::MalformedConfiguration(format!(
                            "package fact is missing for observed manifest {}",
                            manifest_path.display_escaped()
                        ))
                    });
            }
            Some(ConfigObservation::NonRegular { .. } | ConfigObservation::Unreadable { .. }) => {
                return Ok(PackageSelection::Ambiguous(format!(
                    "nearest ancestor package manifest is not observable: {}",
                    manifest_path.display_escaped()
                )));
            }
            Some(ConfigObservation::Missing { .. }) | None => {}
        }
    }
    Ok(PackageSelection::Ambiguous(
        "planned dependency path has no observable ancestor package manifest".to_owned(),
    ))
}

fn package_supports_dependency_ownership(package: &PackageFact) -> bool {
    package.dependency_ownership_supported
        && !matches!(package.identity, PackageIdentityState::Unsupported { .. })
}

fn workspace_ownership_is_unsupported(
    config: &SemanticConfigSnapshot,
    package: &PackageFact,
) -> bool {
    let selected_depth = package
        .workspace_root
        .as_ref()
        .map(RepoPath::components_len);
    config.workspaces.iter().any(|workspace| {
        !workspace.ownership_supported
            && package.root.is_within(&workspace.root)
            && selected_depth.is_none_or(|depth| workspace.root.components_len() >= depth)
    })
}

fn selected_workspace<'a>(
    config: &'a SemanticConfigSnapshot,
    package: &PackageFact,
) -> Result<Option<&'a WorkspaceFact>, InventoryError> {
    let Some(workspace_root) = &package.workspace_root else {
        return Ok(None);
    };
    config
        .workspaces
        .iter()
        .find(|workspace| &workspace.root == workspace_root)
        .map(Some)
        .ok_or_else(|| {
            InventoryError::MalformedConfiguration(format!(
                "workspace fact is missing for package {}",
                package.manifest_path.display_escaped()
            ))
        })
}

enum LockfileSelection {
    Complete(Option<RepoPath>),
    Ambiguous,
}

fn lockfile_search_paths(
    root: &Path,
    package_root: &RepoPath,
    workspace_root: &RepoPath,
) -> Result<Vec<RepoPath>, InventoryError> {
    validate_package_workspace(package_root, workspace_root)?;
    let mut paths = Vec::new();
    let mut directory = package_root.clone();
    loop {
        let mut observed_at_directory = false;
        for name in LOCKFILE_NAMES {
            let path = join(&directory, name)?;
            let identity = observe_config_input_identity(root, &path)?;
            observed_at_directory |= identity.absence_parent.is_none();
            paths.push(path);
        }
        if observed_at_directory || directory == *workspace_root {
            return Ok(paths);
        }
        directory = next_lockfile_directory(package_root, workspace_root, directory)?;
    }
}

fn select_lockfile(
    owner: &PendingOwner,
    inputs: &BTreeMap<RepoPath, SemanticPolicyInput>,
    limitations: &mut Vec<Limitation>,
) -> Result<LockfileSelection, InventoryError> {
    validate_package_workspace(&owner.package_root, &owner.workspace_root)?;
    let mut directory = owner.package_root.clone();
    loop {
        let mut present = Vec::new();
        let mut unavailable = Vec::new();
        for name in LOCKFILE_NAMES {
            let path = join(&directory, name)?;
            let observation = inputs.get(&path).ok_or_else(|| {
                InventoryError::MalformedConfiguration(format!(
                    "lockfile plan omitted captured candidate {}",
                    path.display_escaped()
                ))
            })?;
            match observation.state {
                SemanticPolicyState::Present => present.push(path),
                SemanticPolicyState::NonRegular | SemanticPolicyState::Unreadable => {
                    unavailable.push(observation.clone())
                }
                SemanticPolicyState::Missing => {}
            }
        }
        if !unavailable.is_empty() {
            let details = unavailable
                .into_iter()
                .map(|input| {
                    input.detail.unwrap_or_else(|| {
                        format!(
                            "{} is not an observable regular file",
                            input.path.display_escaped()
                        )
                    })
                })
                .collect::<Vec<_>>()
                .join("; ");
            limitations.push(Limitation::DependencyOwnerAmbiguous {
                path: owner.manifest_path.display_escaped(),
                detail: format!("lockfile ownership is unobservable: {details}"),
            });
            return Ok(LockfileSelection::Ambiguous);
        }
        if present.len() > 1 {
            let names = present
                .iter()
                .map(RepoPath::display_escaped)
                .collect::<Vec<_>>()
                .join(", ");
            limitations.push(Limitation::DependencyOwnerAmbiguous {
                path: owner.manifest_path.display_escaped(),
                detail: format!(
                    "multiple supported lockfiles coexist at the nearest directory: {names}"
                ),
            });
            return Ok(LockfileSelection::Ambiguous);
        }
        if let Some(path) = present.pop() {
            return Ok(LockfileSelection::Complete(Some(path)));
        }
        if directory == owner.workspace_root {
            return Ok(LockfileSelection::Complete(None));
        }
        directory = next_lockfile_directory(&owner.package_root, &owner.workspace_root, directory)?;
    }
}

fn validate_package_workspace(
    package_root: &RepoPath,
    workspace_root: &RepoPath,
) -> Result<(), InventoryError> {
    if package_root.is_within(workspace_root) {
        Ok(())
    } else {
        Err(InventoryError::MalformedConfiguration(format!(
            "package {} is outside its workspace {}",
            package_root.display_escaped(),
            workspace_root.display_escaped()
        )))
    }
}

fn next_lockfile_directory(
    package_root: &RepoPath,
    workspace_root: &RepoPath,
    directory: RepoPath,
) -> Result<RepoPath, InventoryError> {
    directory.parent().ok_or_else(|| {
        InventoryError::MalformedConfiguration(format!(
            "package {} cannot reach workspace {} while selecting a lockfile",
            package_root.display_escaped(),
            workspace_root.display_escaped()
        ))
    })
}

fn capture_lockfile(root: &Path, path: &RepoPath) -> Result<SemanticPolicyInput, InventoryError> {
    let identity = observe_config_input_identity(root, path)?;
    if let Some(parent) = identity.absence_parent {
        return Ok(SemanticPolicyInput {
            path: path.clone(),
            state: SemanticPolicyState::Missing,
            payload_sha256: None,
            physical_identity: None,
            absence_parent: Some(parent),
            detail: None,
        });
    }
    let native = root.join(native_relative(path)?);
    let metadata = match fs::symlink_metadata(&native) {
        Ok(metadata) => metadata,
        Err(error) => {
            return Ok(unreadable_input(
                path,
                identity.physical_identity,
                error.to_string(),
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(SemanticPolicyInput {
            path: path.clone(),
            state: SemanticPolicyState::NonRegular,
            payload_sha256: None,
            physical_identity: identity.physical_identity,
            absence_parent: None,
            detail: Some(format!(
                "{} is a symlink or non-regular file",
                path.display_escaped()
            )),
        });
    }
    let mut file = match fs::File::open(&native) {
        Ok(file) => file,
        Err(error) => {
            return Ok(unreadable_input(
                path,
                identity.physical_identity,
                error.to_string(),
            ));
        }
    };
    let physical_identity = crate::capture::physical_identity_from_file(&file)?;
    let mut bytes = Vec::new();
    if let Err(error) = file.read_to_end(&mut bytes) {
        return Ok(unreadable_input(
            path,
            Some(physical_identity),
            error.to_string(),
        ));
    }
    let current = observe_config_input_identity(root, path)?;
    if current.physical_identity.as_ref() != Some(&physical_identity)
        || current.absence_parent.is_some()
    {
        return Err(InventoryError::PhysicalIdentity(format!(
            "lockfile path changed physical identity during capture: {}",
            path.display_escaped()
        )));
    }
    Ok(SemanticPolicyInput {
        path: path.clone(),
        state: SemanticPolicyState::Present,
        payload_sha256: Some(digest_hex(&bytes)),
        physical_identity: Some(physical_identity),
        absence_parent: None,
        detail: None,
    })
}

fn unreadable_input(
    path: &RepoPath,
    physical_identity: Option<lumin_model::PhysicalFileIdentity>,
    detail: String,
) -> SemanticPolicyInput {
    SemanticPolicyInput {
        path: path.clone(),
        state: SemanticPolicyState::Unreadable,
        payload_sha256: None,
        physical_identity,
        absence_parent: None,
        detail: Some(detail),
    }
}

fn ancestor_directories(path: &RepoPath) -> Vec<RepoPath> {
    let mut directories = Vec::new();
    let mut current = path.parent().unwrap_or_else(RepoPath::empty);
    loop {
        directories.push(current.clone());
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }
    directories
}

fn join(directory: &RepoPath, name: &str) -> Result<RepoPath, InventoryError> {
    directory
        .join_portable(name)
        .map_err(|source| InventoryError::InvalidRepoPath {
            path: format!("{}/{}", directory.display_escaped(), name),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InventoryRequest, begin_scan, scan};

    #[test]
    fn selects_each_nearest_manifest_and_nearest_lockfile() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        write(
            root.path(),
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        )?;
        write(root.path(), "pnpm-lock.yaml", "lockfileVersion: '9.0'\n")?;
        write(
            root.path(),
            "packages/local/package.json",
            r#"{"name":"@acme/local","private":true}"#,
        )?;
        write(root.path(), "packages/local/package-lock.json", "{}\n")?;
        write(
            root.path(),
            "packages/local/src/main.ts",
            "console.log('local');\n",
        )?;
        write(
            root.path(),
            "packages/inherited/package.json",
            r#"{"name":"@acme/inherited","private":true}"#,
        )?;
        write(
            root.path(),
            "packages/inherited/src/main.ts",
            "console.log('inherited');\n",
        )?;

        let inventory = scan(
            root.path(),
            &InventoryRequest {
                dependency_intents: vec![
                    intent("packages/local/src/main.ts", "zod")?,
                    intent("packages/inherited/src/main.ts", "serde")?,
                ],
                ..Default::default()
            },
        )?;
        let owners = inventory
            .config
            .dependency_owners
            .iter()
            .map(|owner| {
                (
                    owner.intent.path.display_escaped(),
                    owner.manifest_path.display_escaped(),
                    owner.lockfile_path.as_ref().map(RepoPath::display_escaped),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            owners,
            vec![
                (
                    "packages/local/src/main.ts".to_owned(),
                    "packages/local/package.json".to_owned(),
                    Some("packages/local/package-lock.json".to_owned()),
                ),
                (
                    "packages/inherited/src/main.ts".to_owned(),
                    "packages/inherited/package.json".to_owned(),
                    Some("pnpm-lock.yaml".to_owned()),
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn an_unobservable_pnpm_workspace_never_falls_back_to_package_workspaces()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        write(
            root.path(),
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        )?;
        std::fs::create_dir(root.path().join("pnpm-workspace.yaml"))?;
        write(
            root.path(),
            "packages/app/package.json",
            r#"{"name":"@acme/app","private":true}"#,
        )?;
        write(
            root.path(),
            "packages/app/src/main.ts",
            "console.log('app');\n",
        )?;

        let inventory = scan(
            root.path(),
            &InventoryRequest {
                dependency_intents: vec![intent("packages/app/src/main.ts", "zod")?],
                ..Default::default()
            },
        )?;
        assert!(inventory.config.dependency_owners.is_empty());
        assert!(inventory.limitations.iter().any(|limitation| matches!(
            limitation,
            Limitation::WorkspaceOwnershipUnsupported { path, .. }
                if path == "pnpm-workspace.yaml"
        )));
        Ok(())
    }

    #[test]
    fn a_nearer_lockfile_stops_the_pre_capture_demand_walk()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        write(
            root.path(),
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        )?;
        write(root.path(), "pnpm-lock.yaml", "lockfileVersion: '9.0'\n")?;
        write(
            root.path(),
            "packages/app/package.json",
            r#"{"name":"@acme/app","private":true}"#,
        )?;
        write(root.path(), "packages/app/package-lock.json", "{}\n")?;
        write(
            root.path(),
            "packages/app/src/main.ts",
            "console.log('app');\n",
        )?;

        let pending = begin_scan(
            root.path(),
            &InventoryRequest {
                dependency_intents: vec![intent("packages/app/src/main.ts", "zod")?],
                ..Default::default()
            },
        )?;
        let demanded = pending
            .dependency_input_paths()
            .iter()
            .map(RepoPath::display_escaped)
            .collect::<Vec<_>>();
        assert_eq!(demanded.len(), LOCKFILE_NAMES.len());
        assert!(
            demanded
                .iter()
                .all(|path| path.starts_with("packages/app/"))
        );
        assert!(!demanded.contains(&"pnpm-lock.yaml".to_owned()));
        Ok(())
    }

    fn intent(path: &str, dependency: &str) -> Result<DependencyIntent, InventoryError> {
        Ok(DependencyIntent {
            path: RepoPath::from_portable(path).map_err(|source| {
                InventoryError::InvalidRepoPath {
                    path: path.to_owned(),
                    source,
                }
            })?,
            dependency: dependency.to_owned(),
        })
    }

    fn write(root: &Path, relative: &str, contents: &str) -> std::io::Result<()> {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)
    }
}
