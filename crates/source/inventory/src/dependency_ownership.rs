use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
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
) -> Result<Vec<SemanticPolicyInput>, InventoryError> {
    let mut candidates = BTreeMap::<RepoPath, ConfigSyntax>::new();
    for intent in intents {
        if context_is_hard_excluded(root, &intent.path)? {
            continue;
        }
        for directory in reservation_directories(&intent.path) {
            candidates.insert(join(&directory, "package.json")?, ConfigSyntax::StrictJson);
            candidates.insert(
                join(&directory, "pnpm-workspace.yaml")?,
                ConfigSyntax::RestrictedYaml,
            );
        }
    }
    let mut captured = BTreeMap::new();
    for (path, syntax) in candidates {
        if observations.contains_key(&path) {
            continue;
        }
        let capture = capture_config(root, &path, syntax)?;
        captured.insert(path, capture);
    }

    let mut nondirectory_contexts = BTreeSet::new();
    for intent in intents {
        // The direct child candidates were already reserved and captured. A missing
        // child therefore binds the context's physical identity before this kind
        // check; file contexts keep that observation only as a topology guard.
        if !context_is_hard_excluded(root, &intent.path)?
            && !context_is_directory(root, &intent.path, observations, &captured)?
        {
            nondirectory_contexts.insert(intent.path.clone());
        }
    }

    let mut context_guards = Vec::new();
    for (path, capture) in captured {
        let is_context_guard = path
            .parent()
            .is_some_and(|parent| nondirectory_contexts.contains(&parent));
        if is_context_guard {
            if capture.limitation.is_some() {
                return Err(InventoryError::MalformedConfiguration(format!(
                    "non-directory dependency context produced an observable config candidate: {}",
                    path.display_escaped()
                )));
            }
            context_guards.push(context_guard_input(capture.observation)?);
        } else {
            if let Some(limitation) = capture.limitation {
                limitations.push(limitation);
            }
            consulted_config_paths.push(path.clone());
            observations.insert(path, capture.observation);
        }
    }
    Ok(context_guards)
}

fn context_is_directory(
    root: &Path,
    context: &RepoPath,
    observations: &BTreeMap<RepoPath, ConfigObservation>,
    captured: &BTreeMap<RepoPath, crate::ConfigCapture>,
) -> Result<bool, InventoryError> {
    let package_path = join(context, "package.json")?;
    let observation = observations
        .get(&package_path)
        .or_else(|| {
            captured
                .get(&package_path)
                .map(|capture| &capture.observation)
        })
        .ok_or_else(|| {
            InventoryError::MalformedConfiguration(format!(
                "dependency context omitted its reserved package candidate: {}",
                package_path.display_escaped()
            ))
        })?;
    let ConfigObservation::Missing { parent, .. } = observation else {
        return Ok(true);
    };
    if parent.path != *context {
        return Ok(false);
    }

    let identity = observe_config_input_identity(root, context)?;
    if identity.absence_parent.is_some()
        || identity.physical_identity.as_ref() != Some(&parent.physical_identity)
    {
        return Err(InventoryError::PhysicalIdentity(format!(
            "dependency context changed while classifying its reserved package candidate: {}",
            context.display_escaped()
        )));
    }
    let native = root.join(native_relative(context)?);
    let metadata = fs::metadata(&native).map_err(|error| {
        InventoryError::PhysicalIdentity(format!(
            "cannot classify dependency context {}: {error}",
            context.display_escaped()
        ))
    })?;
    if metadata.is_dir() {
        Ok(true)
    } else if metadata.is_file() {
        Ok(false)
    } else {
        Err(InventoryError::PhysicalIdentity(format!(
            "dependency context is neither a file nor a directory: {}",
            context.display_escaped()
        )))
    }
}

fn context_guard_input(
    observation: ConfigObservation,
) -> Result<SemanticPolicyInput, InventoryError> {
    let ConfigObservation::Missing { path, parent } = observation else {
        return Err(InventoryError::MalformedConfiguration(
            "non-directory dependency context produced a non-missing config guard".to_owned(),
        ));
    };
    Ok(SemanticPolicyInput {
        path,
        state: SemanticPolicyState::Missing,
        payload_sha256: None,
        physical_identity: None,
        absence_parent: Some(parent),
        detail: None,
    })
}

pub(crate) fn reservation_paths(
    root: &Path,
    intents: &[DependencyIntent],
) -> Result<Vec<RepoPath>, InventoryError> {
    let mut candidates = BTreeMap::<RepoPath, ()>::new();
    for intent in intents {
        if context_is_hard_excluded(root, &intent.path)? {
            continue;
        }
        for directory in reservation_directories(&intent.path) {
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
        if context_is_hard_excluded(root, &intent.path)? {
            limitations.push(Limitation::DependencyOwnerAmbiguous {
                path: intent.path.display_escaped(),
                detail: "dependency context is inside a hard-excluded repository subtree"
                    .to_owned(),
            });
            continue;
        }
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
        for path in lockfile_search_paths(&package.root, &workspace_root)? {
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
    for directory in reservation_directories(&intent.path) {
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
    package_root: &RepoPath,
    workspace_root: &RepoPath,
) -> Result<Vec<RepoPath>, InventoryError> {
    validate_package_workspace(package_root, workspace_root)?;
    let mut paths = Vec::new();
    let mut directory = package_root.clone();
    loop {
        for name in LOCKFILE_NAMES {
            paths.push(join(&directory, name)?);
        }
        if directory == *workspace_root {
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

fn reservation_directories(path: &RepoPath) -> Vec<RepoPath> {
    ancestor_directories(path, true)
}

fn ancestor_directories(path: &RepoPath, starts_at_context: bool) -> Vec<RepoPath> {
    let mut directories = Vec::new();
    let mut current = if starts_at_context {
        path.clone()
    } else {
        path.parent().unwrap_or_else(RepoPath::empty)
    };
    loop {
        directories.push(current.clone());
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }
    directories
}

fn context_is_hard_excluded(root: &Path, path: &RepoPath) -> Result<bool, InventoryError> {
    let relative = native_relative(path)?;
    if relative.iter().any(hard_excluded_component) {
        return Ok(true);
    }

    let canonical_root = fs::canonicalize(root)
        .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?;
    let mut native_prefix = root.to_path_buf();
    for component in relative.iter() {
        native_prefix.push(component);
        match fs::symlink_metadata(&native_prefix) {
            Ok(_) => {
                let physical_prefix = fs::canonicalize(&native_prefix).map_err(|error| {
                    InventoryError::PhysicalIdentity(format!(
                        "cannot resolve dependency context prefix {}: {error}",
                        path.display_escaped()
                    ))
                })?;
                let physical_relative = physical_prefix
                    .strip_prefix(&canonical_root)
                    .map_err(|_| InventoryError::EntryEscapesRoot(path.display_escaped()))?;
                if physical_relative.iter().any(hard_excluded_component) {
                    return Ok(true);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                break;
            }
            Err(error) => {
                return Err(InventoryError::PhysicalIdentity(format!(
                    "cannot inspect dependency context prefix {}: {error}",
                    path.display_escaped()
                )));
            }
        }
    }
    Ok(false)
}

fn hard_excluded_component(component: &OsStr) -> bool {
    component.to_str().is_some_and(|component| {
        [".git", ".lumin", "node_modules"]
            .into_iter()
            .any(|excluded| component.eq_ignore_ascii_case(excluded))
    })
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
    fn directory_context_selects_its_own_manifest() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        write(
            root.path(),
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        )?;
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
                dependency_intents: vec![intent("packages/app", "zod")?],
                ..Default::default()
            },
        )?;
        assert_eq!(inventory.config.dependency_owners.len(), 1);
        assert_eq!(
            inventory.config.dependency_owners[0]
                .manifest_path
                .display_escaped(),
            "packages/app/package.json"
        );
        Ok(())
    }

    #[test]
    fn file_context_starts_at_parent_and_retains_an_identity_guard()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        write(
            root.path(),
            "package.json",
            r#"{"name":"root","private":true}"#,
        )?;
        write(root.path(), "src/main.ts", "console.log('app');\n")?;

        let inventory = scan(
            root.path(),
            &InventoryRequest {
                dependency_intents: vec![intent("src/main.ts", "zod")?],
                ..Default::default()
            },
        )?;
        let impossible = RepoPath::from_portable("src/main.ts/package.json")?;
        let context = RepoPath::from_portable("src/main.ts")?;
        assert_eq!(inventory.config.dependency_owners.len(), 1);
        assert_eq!(
            inventory.config.dependency_owners[0]
                .manifest_path
                .display_escaped(),
            "package.json"
        );
        assert!(!inventory.config.observations.contains_key(&impossible));
        assert!(inventory.policy_inputs.iter().any(|input| {
            input.path == impossible
                && input.state == SemanticPolicyState::Missing
                && input
                    .absence_parent
                    .as_ref()
                    .is_some_and(|parent| parent.path == context)
        }));
        Ok(())
    }

    #[test]
    fn hard_excluded_context_never_manufactures_an_owner() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        write(
            root.path(),
            "package.json",
            r#"{"name":"root","private":true}"#,
        )?;
        write(
            root.path(),
            "node_modules/pkg/package.json",
            r#"{"name":"pkg","private":true}"#,
        )?;

        let inventory = scan(
            root.path(),
            &InventoryRequest {
                dependency_intents: vec![intent("node_modules/pkg", "zod")?],
                ..Default::default()
            },
        )?;
        assert!(inventory.config.dependency_owners.is_empty());
        assert!(inventory.limitations.iter().any(|limitation| matches!(
            limitation,
            Limitation::DependencyOwnerAmbiguous { path, detail }
                if path == "node_modules/pkg" && detail.contains("hard-excluded")
        )));
        Ok(())
    }

    #[test]
    fn every_lockfile_candidate_is_demanded_before_observation()
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
        assert_eq!(demanded.len(), LOCKFILE_NAMES.len() * 3);
        assert!(
            demanded
                .iter()
                .any(|path| path == "packages/app/package-lock.json")
        );
        assert!(demanded.contains(&"packages/yarn.lock".to_owned()));
        assert!(demanded.contains(&"pnpm-lock.yaml".to_owned()));
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
