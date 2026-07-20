use std::io::Write;
use std::path::Path;

use lumin_model::PhysicalFileIdentity;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tempfile::NamedTempFile;

use crate::{StoreError, io_error, serialization_error};

use super::NamespaceState;
use super::platform::{EntryAccess, EntryKind, HeldEntry};

pub(super) const MANAGED_KINDS: [ManagedStateParentKind; 4] = [
    ManagedStateParentKind::Attempts,
    ManagedStateParentKind::Runs,
    ManagedStateParentKind::Trash,
    ManagedStateParentKind::Cache,
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ManagedStateParentKind {
    Attempts,
    Runs,
    Trash,
    Cache,
}

impl ManagedStateParentKind {
    pub(super) fn directory_name(self) -> &'static str {
        match self {
            Self::Attempts => "attempts",
            Self::Runs => "runs",
            Self::Trash => "trash",
            Self::Cache => "cache",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GlobalNamespaceBinding {
    pub(super) state_directory_identity: PhysicalFileIdentity,
    pub(super) lifecycle_lock_identity: PhysicalFileIdentity,
    pub(super) namespace_nonce: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ManagedStateParentBinding {
    pub(super) kind: ManagedStateParentKind,
    pub(super) directory_physical_identity: PhysicalFileIdentity,
    pub(super) anchor_physical_identity: PhysicalFileIdentity,
    pub(super) parent_nonce: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NamespaceBinding {
    pub(super) global: GlobalNamespaceBinding,
    pub(super) managed_parents: [ManagedStateParentBinding; 4],
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RepositoryMarker {
    pub(super) schema_version: String,
    pub(super) binding: NamespaceBinding,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LifecycleLockHeader {
    pub(super) schema_version: String,
    pub(super) global: GlobalNamespaceBinding,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ManagedParentAnchorHeader {
    pub(super) schema_version: String,
    pub(super) global: GlobalNamespaceBinding,
    pub(super) binding: ManagedStateParentBinding,
}

pub(super) fn read_canonical_path<T: DeserializeOwned + Serialize>(
    path: &Path,
    label: &str,
) -> Result<T, StoreError> {
    let entry = HeldEntry::open(
        path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        false,
        label,
    )?;
    let bytes = entry.read_all()?;
    let value = serde_json::from_slice::<T>(&bytes)
        .map_err(|error| StoreError::Integrity(format!("{label} header is malformed: {error}")))?;
    if canonical_json(&value)? != bytes {
        return Err(StoreError::Integrity(format!(
            "{label} bytes are not canonical"
        )));
    }
    Ok(value)
}

pub(super) fn write_canonical_entry(
    entry: &HeldEntry,
    value: &impl Serialize,
) -> Result<(), StoreError> {
    entry.replace_contents(&canonical_json(value)?)
}

pub(super) fn verify_canonical_entry(
    entry: &HeldEntry,
    expected: &impl Serialize,
    label: &str,
) -> Result<(), StoreError> {
    if entry.read_all()? != canonical_json(expected)? {
        return Err(StoreError::Integrity(format!(
            "{label} immutable header changed"
        )));
    }
    Ok(())
}

pub(super) fn write_new_canonical(path: &Path, value: &impl Serialize) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Io("state marker has no parent".to_owned()))?;
    let mut temp = NamedTempFile::new_in(parent).map_err(io_error)?;
    temp.write_all(&canonical_json(value)?).map_err(io_error)?;
    temp.as_file().sync_all().map_err(io_error)?;
    temp.persist_noclobber(path)
        .map(|_| ())
        .map_err(|error| io_error(error.error))
}

pub(super) fn verify_marker(state: &NamespaceState) -> Result<(), StoreError> {
    let expected = RepositoryMarker {
        schema_version: "lumin-repository.v2".to_owned(),
        binding: state.binding.clone(),
    };
    let entry = HeldEntry::open(
        &state.state_dir.join("repository.json"),
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        false,
        "repository marker",
    )?;
    verify_canonical_entry(&entry, &expected, "repository marker")
}

pub(super) fn verify_lock_header(
    lock: &HeldEntry,
    global: &GlobalNamespaceBinding,
) -> Result<(), StoreError> {
    verify_canonical_entry(
        lock,
        &LifecycleLockHeader {
            schema_version: "lumin-lifecycle-lock.v1".to_owned(),
            global: global.clone(),
        },
        "lifecycle.lock",
    )
}

pub(super) fn validate_marker(marker: &RepositoryMarker) -> Result<(), StoreError> {
    if marker.schema_version != "lumin-repository.v2" {
        return Err(StoreError::Integrity(
            "repository marker schema is unsupported".to_owned(),
        ));
    }
    for (expected, binding) in MANAGED_KINDS
        .iter()
        .zip(marker.binding.managed_parents.iter())
    {
        if expected != &binding.kind {
            return Err(StoreError::Integrity(
                "repository marker managed-parent set is not exact kind order".to_owned(),
            ));
        }
    }
    Ok(())
}

fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, StoreError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(serialization_error)?;
    bytes.push(b'\n');
    Ok(bytes)
}
