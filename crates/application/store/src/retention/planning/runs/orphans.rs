use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use lumin_evidence::{RetentionItemKind, RetentionPlanItem};
use lumin_model::digest_hex;

use crate::namespace::NamespaceGuard;
use crate::{RunCatalogRecord, StoreError, io_error};

pub(super) fn collect_run_orphans(
    guard: &NamespaceGuard,
    runs: &BTreeMap<String, (RunCatalogRecord, Vec<u8>)>,
    known: &BTreeSet<String>,
    before_unix_millis: u64,
    items: &mut Vec<RetentionPlanItem>,
) -> Result<(), StoreError> {
    let runs_path =
        guard.managed_parent_path(crate::namespace::records::ManagedStateParentKind::Runs);
    for child in directory_children(&runs_path, "runs")? {
        if runs.contains_key(&child) || known.contains(&child) {
            continue;
        }
        collect_orphan(
            &runs_path.join(&child),
            format!("runs/{child}"),
            before_unix_millis,
            items,
        )?;
    }
    Ok(())
}

pub(super) fn collect_orphan(
    path: &Path,
    record_id: String,
    before_unix_millis: u64,
    items: &mut Vec<RetentionPlanItem>,
) -> Result<(), StoreError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StoreError::Integrity(format!(
            "managed payload {} is not a real directory",
            path.display()
        )));
    }
    let modified = metadata
        .modified()
        .map_err(io_error)?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| StoreError::Io(error.to_string()))?
        .as_millis();
    if modified >= u128::from(before_unix_millis) {
        return Ok(());
    }
    let (identity_sha256, byte_count) = directory_payload_identity(path)?;
    items.push(RetentionPlanItem {
        kind: RetentionItemKind::OrphanPayload,
        owning_sequence: sequence_from_name(&record_id).unwrap_or(0),
        record_id,
        identity_sha256,
        byte_count,
    });
    Ok(())
}

pub(in crate::retention::planning) fn directory_payload_identity(
    path: &Path,
) -> Result<(String, u64), StoreError> {
    let mut framed = Vec::new();
    let mut byte_count = 0_u64;
    collect_directory_identity(path, path, &mut framed, &mut byte_count)?;
    Ok((digest_hex(&framed), byte_count))
}

fn collect_directory_identity(
    root: &Path,
    current: &Path,
    framed: &mut Vec<u8>,
    byte_count: &mut u64,
) -> Result<(), StoreError> {
    let mut entries = fs::read_dir(current)
        .map_err(io_error)?
        .map(|entry| entry.map_err(io_error))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|error| {
            StoreError::Integrity(format!("managed payload traversal failed: {error}"))
        })?;
        let relative = relative
            .to_str()
            .ok_or_else(|| StoreError::Integrity("managed payload name is not UTF-8".to_owned()))?;
        let metadata = fs::symlink_metadata(&path).map_err(io_error)?;
        lumin_model::append_length_prefixed(framed, relative.as_bytes());
        if metadata.file_type().is_symlink() {
            return Err(StoreError::Integrity(format!(
                "managed payload contains a symbolic link: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            framed.push(1);
            collect_directory_identity(root, &path, framed, byte_count)?;
        } else if metadata.is_file() {
            framed.push(2);
            let bytes = fs::read(&path).map_err(io_error)?;
            *byte_count = byte_count.checked_add(bytes.len() as u64).ok_or_else(|| {
                StoreError::Integrity("managed payload byte count overflow".to_owned())
            })?;
            lumin_model::append_length_prefixed(framed, digest_hex(&bytes).as_bytes());
            framed.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        } else {
            return Err(StoreError::Integrity(format!(
                "managed payload contains an unsupported entry: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

pub(super) fn directory_children(path: &Path, label: &str) -> Result<Vec<String>, StoreError> {
    let mut names = fs::read_dir(path)
        .map_err(io_error)?
        .map(|entry| {
            let entry = entry.map_err(io_error)?;
            entry.file_name().into_string().map_err(|_| {
                StoreError::Integrity(format!("{label} contains a non-UTF-8 child name"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.retain(|name| name != "namespace.anchor");
    names.sort();
    Ok(names)
}

fn sequence_from_name(value: &str) -> Option<u64> {
    let suffix = value.rsplit(['_', '/']).next()?;
    (suffix.len() == 16)
        .then(|| u64::from_str_radix(suffix, 16).ok())
        .flatten()
}
