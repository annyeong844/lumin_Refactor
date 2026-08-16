use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use lumin_model::{
    ConfigAbsenceParent, PhysicalAliasWriteClosure, PhysicalFileIdentity, PhysicalPathRedirect,
    PhysicalPathRedirectKind, PhysicalPathRedirectTarget, RepoPath, append_length_prefixed,
    digest_hex,
};
use thiserror::Error;

use super::{InventoryError, native_relative, validate_root};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteTargetKind {
    ExistingFile,
    ExistingDirectory,
    NewFile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteTargetObservation {
    pub path: RepoPath,
    pub kind: WriteTargetKind,
    pub physical_identity: Option<PhysicalFileIdentity>,
    pub nearest_existing_parent: Option<RepoPath>,
    pub prefix_identities: Vec<(RepoPath, PhysicalFileIdentity)>,
}

#[derive(Debug, Error)]
pub enum WriteTargetError {
    #[error("repository root cannot be leased as one directory scope")]
    UnboundedDirectory,
    #[error("planned path has no observable real parent: {0}")]
    MissingParent(String),
    #[error("planned path resolves outside the repository root: {0}")]
    OutsideRoot(String),
    #[error("planned path is not a regular file or real directory: {0}")]
    NonRegular(String),
    #[error("planned directory is reached through a symlink or junction: {0}")]
    LinkedDirectory(String),
    #[error("failed to inspect planned path {path}: {detail}")]
    Io { path: String, detail: String },
    #[error(transparent)]
    PhysicalIdentity(#[from] InventoryError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigInputIdentity {
    pub physical_identity: Option<PhysicalFileIdentity>,
    pub absence_parent: Option<ConfigAbsenceParent>,
}

pub fn physical_alias_write_closure(
    root: &Path,
    target: &RepoPath,
    source_paths: &[RepoPath],
) -> Result<PhysicalAliasWriteClosure, InventoryError> {
    let target_native = root.join(native_relative(target)?);
    let physical_identity = physical_file_identity(&target_native)?;
    let target_handle = same_file::Handle::from_path(&target_native)
        .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
    let mut aliases = Vec::new();
    for source_path in source_paths {
        let handle = same_file::Handle::from_path(root.join(native_relative(source_path)?))
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        if handle == target_handle {
            aliases.push(source_path.clone());
        }
    }
    aliases.sort();
    aliases.dedup();
    Ok(PhysicalAliasWriteClosure {
        physical_identity,
        members: aliases,
    })
}

pub fn inspect_write_target(
    root: &Path,
    path: &RepoPath,
) -> Result<WriteTargetObservation, WriteTargetError> {
    if path.components_len() == 0 {
        return Err(WriteTargetError::UnboundedDirectory);
    }
    let canonical_root = fs::canonicalize(root).map_err(|error| WriteTargetError::Io {
        path: root.display().to_string(),
        detail: error.to_string(),
    })?;
    let native = root.join(native_relative(path)?);
    let metadata = match fs::symlink_metadata(&native) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let nearest_parent = nearest_existing_parent(root, path)?;
            let prefix_identities =
                observe_directory_prefixes(root, Some(&nearest_parent), &canonical_root)?;
            return Ok(WriteTargetObservation {
                path: path.clone(),
                kind: WriteTargetKind::NewFile,
                physical_identity: None,
                nearest_existing_parent: Some(nearest_parent),
                prefix_identities,
            });
        }
        Err(error) => {
            return Err(WriteTargetError::Io {
                path: path.display_escaped(),
                detail: error.to_string(),
            });
        }
    };

    let target_metadata = if metadata.file_type().is_symlink() {
        let followed = fs::metadata(&native).map_err(|error| WriteTargetError::Io {
            path: path.display_escaped(),
            detail: error.to_string(),
        })?;
        if followed.is_dir() {
            return Err(WriteTargetError::LinkedDirectory(path.display_escaped()));
        }
        followed
    } else {
        metadata
    };
    let prefix_identities =
        observe_directory_prefixes(root, path.parent().as_ref(), &canonical_root)?;
    ensure_contained(&canonical_root, &native, path)?;
    let kind = if target_metadata.is_file() {
        WriteTargetKind::ExistingFile
    } else if target_metadata.is_dir() {
        WriteTargetKind::ExistingDirectory
    } else {
        return Err(WriteTargetError::NonRegular(path.display_escaped()));
    };
    Ok(WriteTargetObservation {
        path: path.clone(),
        kind,
        physical_identity: Some(physical_file_identity(&native)?),
        nearest_existing_parent: None,
        prefix_identities,
    })
}

pub fn rehash_existing_write_target(
    root: &Path,
    observation: &WriteTargetObservation,
) -> Result<String, WriteTargetError> {
    if observation.kind != WriteTargetKind::ExistingFile {
        return Err(WriteTargetError::NonRegular(
            observation.path.display_escaped(),
        ));
    }
    let expected_identity = observation.physical_identity.as_ref().ok_or_else(|| {
        WriteTargetError::PhysicalIdentity(InventoryError::PhysicalIdentity(format!(
            "existing write target omitted its physical identity: {}",
            observation.path.display_escaped()
        )))
    })?;
    let native = root.join(native_relative(&observation.path)?);
    let mut file = fs::File::open(&native).map_err(|error| WriteTargetError::Io {
        path: observation.path.display_escaped(),
        detail: error.to_string(),
    })?;
    let opened_identity = super::capture::physical_identity_from_file(&file)?;
    if &opened_identity != expected_identity {
        return Err(WriteTargetError::PhysicalIdentity(
            InventoryError::PhysicalIdentity(format!(
                "write target changed before payload rehash: {}",
                observation.path.display_escaped()
            )),
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| WriteTargetError::Io {
            path: observation.path.display_escaped(),
            detail: error.to_string(),
        })?;
    let current = inspect_write_target(root, &observation.path)?;
    if current != *observation {
        return Err(WriteTargetError::PhysicalIdentity(
            InventoryError::PhysicalIdentity(format!(
                "write target changed during payload rehash: {}",
                observation.path.display_escaped()
            )),
        ));
    }
    Ok(digest_hex(&bytes))
}

fn nearest_existing_parent(root: &Path, path: &RepoPath) -> Result<RepoPath, WriteTargetError> {
    let mut candidate = path.parent();
    while let Some(parent) = candidate {
        let native = root.join(native_relative(&parent)?);
        match fs::symlink_metadata(&native) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(WriteTargetError::LinkedDirectory(parent.display_escaped()));
            }
            Ok(metadata) if metadata.is_dir() => return Ok(parent),
            Ok(_) => return Err(WriteTargetError::MissingParent(parent.display_escaped())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                candidate = parent.parent();
            }
            Err(error) => {
                return Err(WriteTargetError::Io {
                    path: parent.display_escaped(),
                    detail: error.to_string(),
                });
            }
        }
    }
    Err(WriteTargetError::MissingParent(path.display_escaped()))
}

fn observe_directory_prefixes(
    root: &Path,
    parent: Option<&RepoPath>,
    canonical_root: &Path,
) -> Result<Vec<(RepoPath, PhysicalFileIdentity)>, WriteTargetError> {
    let Some(parent) = parent else {
        return Ok(Vec::new());
    };
    let mut prefixes = Vec::new();
    let mut cursor = Some(parent.clone());
    while let Some(path) = cursor {
        let is_root = path.components_len() == 0;
        prefixes.push(path.clone());
        if is_root {
            break;
        }
        cursor = path.parent();
    }
    prefixes.reverse();

    let mut observed = Vec::with_capacity(prefixes.len());
    for prefix in prefixes {
        let native = root.join(native_relative(&prefix)?);
        let metadata = fs::symlink_metadata(&native).map_err(|error| WriteTargetError::Io {
            path: prefix.display_escaped(),
            detail: error.to_string(),
        })?;
        if metadata.file_type().is_symlink() {
            return Err(WriteTargetError::LinkedDirectory(prefix.display_escaped()));
        }
        if !metadata.is_dir() {
            return Err(WriteTargetError::MissingParent(prefix.display_escaped()));
        }
        ensure_contained(canonical_root, &native, &prefix)?;
        observed.push((prefix, physical_file_identity(&native)?));
    }
    Ok(observed)
}

pub fn physical_file_identity(path: &Path) -> Result<PhysicalFileIdentity, InventoryError> {
    crate::capture::physical_file_observation(path).map(|observation| observation.identity)
}

#[cfg(windows)]
fn windows_physical_identity(
    information: &winapi_util::file::Information,
) -> Result<PhysicalFileIdentity, InventoryError> {
    let volume_serial = u32::try_from(information.volume_serial_number()).map_err(|_| {
        InventoryError::PhysicalIdentity("volume serial number exceeds u32".to_owned())
    })?;
    Ok(PhysicalFileIdentity::Windows {
        volume_serial,
        file_index: information.file_index(),
    })
}

pub(super) fn is_physical_path_redirect(path: &Path, file_type: &fs::FileType) -> bool {
    if file_type.is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        fs::symlink_metadata(path).map_or(true, |metadata| {
            metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        })
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

fn physical_redirect_entry_identity(path: &Path) -> Result<PhysicalFileIdentity, InventoryError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let metadata = fs::symlink_metadata(path)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        Ok(PhysicalFileIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };

        let file = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        let handle = winapi_util::Handle::from_file(file);
        let information = winapi_util::file::information(&handle)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        windows_physical_identity(&information)
    }
}

pub fn observe_config_input_identity(
    root: &Path,
    path: &RepoPath,
) -> Result<ConfigInputIdentity, InventoryError> {
    validate_root(root)?;
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?;
    let native = root.join(native_relative(path)?);
    match fs::symlink_metadata(&native) {
        Ok(metadata) => {
            match fs::canonicalize(&native) {
                Ok(canonical) if !canonical.starts_with(&canonical_root) => {
                    return Err(InventoryError::MalformedConfiguration(format!(
                        "config path resolves outside the repository root: {}",
                        path.display_escaped()
                    )));
                }
                Ok(_) => {}
                Err(_) if metadata.file_type().is_symlink() => {
                    return Ok(ConfigInputIdentity {
                        physical_identity: None,
                        absence_parent: None,
                    });
                }
                Err(error) => {
                    return Err(InventoryError::PhysicalIdentity(format!(
                        "cannot resolve config path {}: {error}",
                        path.display_escaped()
                    )));
                }
            }
            Ok(ConfigInputIdentity {
                physical_identity: Some(physical_file_identity(&native)?),
                absence_parent: None,
            })
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(ConfigInputIdentity {
                physical_identity: None,
                absence_parent: Some(config_absence_parent(
                    root,
                    &canonical_root,
                    path.parent().unwrap_or_else(RepoPath::empty),
                )?),
            })
        }
        Err(error) => Err(InventoryError::PhysicalIdentity(error.to_string())),
    }
}

fn config_absence_parent(
    root: &Path,
    canonical_root: &Path,
    mut path: RepoPath,
) -> Result<ConfigAbsenceParent, InventoryError> {
    loop {
        let native = root.join(native_relative(&path)?);
        match fs::symlink_metadata(&native) {
            Ok(_) => {
                let canonical = fs::canonicalize(&native).map_err(|error| {
                    InventoryError::PhysicalIdentity(format!(
                        "cannot resolve missing-config parent {}: {error}",
                        path.display_escaped()
                    ))
                })?;
                if !canonical.starts_with(canonical_root) {
                    return Err(InventoryError::MalformedConfiguration(format!(
                        "missing-config parent resolves outside the repository root: {}",
                        path.display_escaped()
                    )));
                }
                return Ok(ConfigAbsenceParent {
                    physical_identity: physical_file_identity(&native)?,
                    path,
                });
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                path = path.parent().ok_or_else(|| {
                    InventoryError::PhysicalIdentity(
                        "repository root disappeared while binding a missing config".to_owned(),
                    )
                })?;
            }
            Err(error) => {
                return Err(InventoryError::PhysicalIdentity(format!(
                    "cannot inspect missing-config parent {}: {error}",
                    path.display_escaped()
                )));
            }
        }
    }
}

pub fn observe_physical_file_identity(
    root: &Path,
    path: &RepoPath,
) -> Result<PhysicalFileIdentity, InventoryError> {
    validate_root(root)?;
    physical_file_identity(&root.join(native_relative(path)?))
}

pub fn validate_captured_physical_path_redirect(
    root: &Path,
    path: &RepoPath,
    expected_sha256: &str,
    reserved_state_identities: &BTreeSet<PhysicalFileIdentity>,
) -> Result<(), InventoryError> {
    validate_root(root)?;
    super::reserved_state::validate_semantic_input_path(root, path)?;
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?;
    let native = root.join(native_relative(path)?);
    let file_type = fs::symlink_metadata(&native)
        .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?
        .file_type();
    if !is_physical_path_redirect(&native, &file_type) {
        return Err(InventoryError::PhysicalIdentity(format!(
            "physical redirect disappeared after capture: {}",
            path.display_escaped()
        )));
    }
    let (observed, _) = observe_physical_path_redirect(&canonical_root, &native, path.clone());
    if observed.semantic_sha256() != expected_sha256 {
        return Err(InventoryError::PhysicalIdentity(format!(
            "physical redirect changed after capture: {}",
            path.display_escaped()
        )));
    }
    if observed
        .entry_physical_identity
        .iter()
        .chain(observed.target_physical_identity.iter())
        .any(|identity| reserved_state_identities.contains(identity))
    {
        return Err(InventoryError::ReservedSemanticInputPath(
            path.display_escaped(),
        ));
    }
    Ok(())
}

pub fn directory_physical_identity(
    root: &Path,
    path: &RepoPath,
) -> Result<PhysicalFileIdentity, WriteTargetError> {
    if path.components_len() == 0 {
        let metadata = fs::symlink_metadata(root).map_err(|error| WriteTargetError::Io {
            path: path.display_escaped(),
            detail: error.to_string(),
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(WriteTargetError::LinkedDirectory(path.display_escaped()));
        }
        return physical_file_identity(root).map_err(WriteTargetError::from);
    }
    let observation = inspect_write_target(root, path)?;
    if observation.kind != WriteTargetKind::ExistingDirectory {
        return Err(WriteTargetError::NonRegular(path.display_escaped()));
    }
    observation
        .physical_identity
        .ok_or_else(|| WriteTargetError::NonRegular(path.display_escaped()))
}

fn ensure_contained(
    canonical_root: &Path,
    native: &Path,
    logical: &RepoPath,
) -> Result<(), WriteTargetError> {
    let canonical = fs::canonicalize(native).map_err(|error| WriteTargetError::Io {
        path: logical.display_escaped(),
        detail: error.to_string(),
    })?;
    if !canonical.starts_with(canonical_root) {
        return Err(WriteTargetError::OutsideRoot(logical.display_escaped()));
    }
    Ok(())
}

pub(super) fn observe_physical_path_redirect(
    canonical_root: &Path,
    native_path: &Path,
    path: RepoPath,
) -> (PhysicalPathRedirect, Option<String>) {
    let raw_target = fs::read_link(native_path);
    let canonical_target = fs::canonicalize(native_path);
    let kind = match fs::metadata(native_path) {
        Ok(metadata) if metadata.is_file() => PhysicalPathRedirectKind::File,
        Ok(metadata) if metadata.is_dir() => PhysicalPathRedirectKind::Directory,
        Ok(_) => PhysicalPathRedirectKind::Other,
        Err(_) => PhysicalPathRedirectKind::Unavailable,
    };
    let entry_physical_identity = physical_redirect_entry_identity(native_path);
    let target_physical_identity = physical_file_identity(native_path);
    let target_identity_sha256 = physical_redirect_target_sha256(
        raw_target.as_ref(),
        canonical_target.as_ref(),
        target_physical_identity.as_ref().ok(),
    );
    let target = match &canonical_target {
        _ if raw_target.is_err() => PhysicalPathRedirectTarget::Unavailable,
        Ok(target) if target.starts_with(canonical_root) => target
            .strip_prefix(canonical_root)
            .ok()
            .and_then(|relative| RepoPath::from_native_relative(relative).ok())
            .map_or(
                PhysicalPathRedirectTarget::Unavailable,
                PhysicalPathRedirectTarget::Repository,
            ),
        Ok(_) => PhysicalPathRedirectTarget::OutsideRepository,
        Err(_) => PhysicalPathRedirectTarget::Unavailable,
    };
    let mut errors = Vec::new();
    if let Err(error) = &raw_target {
        errors.push(format!("cannot observe physical redirect target: {error}"));
    }
    if let Err(error) = &canonical_target {
        errors.push(format!("cannot resolve physical redirect target: {error}"));
    }
    if let Err(error) = &entry_physical_identity {
        errors.push(format!(
            "cannot observe physical redirect entry identity: {error}"
        ));
    }
    if let Err(error) = &target_physical_identity {
        errors.push(format!(
            "cannot observe physical redirect target identity: {error}"
        ));
    }
    let target_error = (!errors.is_empty()).then(|| errors.join("; "));
    (
        PhysicalPathRedirect {
            path,
            target,
            kind,
            entry_physical_identity: entry_physical_identity.ok(),
            target_physical_identity: target_physical_identity.ok(),
            target_identity_sha256,
        },
        target_error,
    )
}

fn physical_redirect_target_sha256(
    raw_target: Result<&PathBuf, &std::io::Error>,
    canonical_target: Result<&PathBuf, &std::io::Error>,
    target_physical_identity: Option<&PhysicalFileIdentity>,
) -> String {
    let mut framed = Vec::new();
    match raw_target {
        Ok(target) => {
            framed.push(1);
            append_native_path(&mut framed, target);
        }
        Err(error) => {
            framed.push(0);
            append_length_prefixed(&mut framed, format!("{:?}", error.kind()).as_bytes());
        }
    }
    match canonical_target {
        Ok(target) => {
            framed.push(1);
            append_native_path(&mut framed, target);
        }
        Err(error) => {
            framed.push(0);
            append_length_prefixed(&mut framed, format!("{:?}", error.kind()).as_bytes());
        }
    }
    match target_physical_identity {
        Some(identity) => {
            framed.push(1);
            append_length_prefixed(&mut framed, &identity.canonical_bytes());
        }
        None => framed.push(0),
    }
    digest_hex(&framed)
}

fn append_native_path(output: &mut Vec<u8>, path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        output.push(1);
        append_length_prefixed(output, path.as_os_str().as_bytes());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        output.push(2);
        let mut encoded = Vec::new();
        for value in path.as_os_str().encode_wide() {
            encoded.extend_from_slice(&value.to_be_bytes());
        }
        append_length_prefixed(output, &encoded);
    }
}
