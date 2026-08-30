use std::ffi::OsStr;

use lumin_model::digest_hex;
use redb::{ReadableTable, WriteTransaction};

use crate::gate::records::{load_record, read_record, write_record};
use crate::namespace::records::ManagedStateParentKind;
use crate::namespace::{EntryAccess, EntryKind, HeldEntry, UnpublishedFile, same_volume_and_mount};
use crate::{RepositoryStore, StoreError, backend_error, nonce_hex};

use super::{
    ANALYSIS_CACHE_AUTHORIZATION_SCHEMA, ANALYSIS_CACHE_AUTHORIZATIONS, ANALYSIS_CACHE_PREFIX,
    ANALYSIS_CACHE_SUFFIX, AnalysisCacheAuthorization, analysis_cache_candidate_name,
    is_canonical_sha256, validate_analysis_cache_authorization,
};

impl RepositoryStore {
    /// Read every physically valid candidate for one exact supplied-input key.
    /// Invalid or concurrently replaced cache payloads are misses; a changed
    /// canonical namespace binding remains an integrity failure.
    pub fn read_analysis_cache_candidates(
        &self,
        supplied_input_key: &str,
    ) -> Result<Vec<Vec<u8>>, StoreError> {
        validate_supplied_input_key(supplied_input_key)?;
        self.with_shared_lock(|guard| {
            let database = guard.open_database()?;
            let parent = guard.managed_parent_entry(ManagedStateParentKind::Cache)?;
            let parent_path = guard.managed_parent_path(ManagedStateParentKind::Cache);
            let mut candidates = Vec::new();

            for native_name in parent.directory_names("active cache")? {
                let Some(name) = native_name.to_str() else {
                    continue;
                };
                let Some(expected_digest) = candidate_digest(name, supplied_input_key) else {
                    continue;
                };
                let Some(authorization) = load_record::<AnalysisCacheAuthorization>(
                    &database,
                    ANALYSIS_CACHE_AUTHORIZATIONS,
                    name,
                )?
                else {
                    continue;
                };
                validate_analysis_cache_authorization(
                    name,
                    &authorization,
                    Some(guard.repository_id()),
                )?;
                if authorization.supplied_input_key != supplied_input_key
                    || authorization.payload_sha256 != expected_digest
                {
                    return Err(StoreError::Integrity(format!(
                        "analysis cache authorization disagrees with its entry name: {name}"
                    )));
                }
                let path = parent_path.join(&native_name);
                let entry = match HeldEntry::open(
                    &path,
                    EntryKind::RegularFile,
                    EntryAccess::ReadOnly,
                    true,
                    "analysis cache candidate",
                ) {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                if !same_volume_and_mount(&entry, parent) {
                    continue;
                }
                let first = match entry.read_all() {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                };
                if entry
                    .validate_path(
                        &path,
                        EntryKind::RegularFile,
                        EntryAccess::ReadOnly,
                        true,
                        "analysis cache candidate",
                    )
                    .is_err()
                {
                    continue;
                }
                let second = match entry.read_all() {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                };
                let byte_length = u64::try_from(second.len()).map_err(|_| {
                    StoreError::Integrity(
                        "analysis cache candidate byte length overflow".to_owned(),
                    )
                })?;
                if first == second
                    && byte_length == authorization.byte_length
                    && digest_hex(&second) == expected_digest
                {
                    candidates.push(second);
                }
            }
            guard.validate_bound_entries()?;
            Ok(candidates)
        })
    }

    /// Publish one immutable content-addressed analysis-cache candidate. A
    /// live cleanup reservation or an already published identical candidate
    /// makes this a no-op; neither condition may change canonical evidence.
    pub fn write_analysis_cache_candidate(
        &self,
        supplied_input_key: &str,
        bytes: &[u8],
    ) -> Result<bool, StoreError> {
        validate_supplied_input_key(supplied_input_key)?;
        let payload_digest = digest_hex(bytes);
        let name = analysis_cache_candidate_name(supplied_input_key, &payload_digest);
        let byte_length = u64::try_from(bytes.len()).map_err(|_| {
            StoreError::Integrity("analysis cache candidate byte length overflow".to_owned())
        })?;
        self.with_exclusive_lock(|guard| {
            let database = guard.open_database()?;
            let write = database.begin_write()?;
            match super::reject_active_cache_mutation_reservation(&write, None) {
                Ok(()) => {}
                Err(StoreError::OperationBusy(_)) => return Ok(false),
                Err(error) => return Err(error),
            }

            let authorization = AnalysisCacheAuthorization {
                schema_version: ANALYSIS_CACHE_AUTHORIZATION_SCHEMA.to_owned(),
                repository_id: guard.repository_id().clone(),
                supplied_input_key: supplied_input_key.to_owned(),
                entry_name: name.clone(),
                payload_sha256: payload_digest.clone(),
                byte_length,
            };
            let existing_authorization = read_record::<AnalysisCacheAuthorization>(
                &write,
                ANALYSIS_CACHE_AUTHORIZATIONS,
                &name,
            )?;
            if let Some(existing) = &existing_authorization {
                validate_analysis_cache_authorization(
                    &name,
                    existing,
                    Some(guard.repository_id()),
                )?;
                if existing != &authorization {
                    return Err(StoreError::Integrity(format!(
                        "analysis cache authorization changed for {name}"
                    )));
                }
            }

            let parent = guard.managed_parent_entry(ManagedStateParentKind::Cache)?;
            let parent_path = guard.managed_parent_path(ManagedStateParentKind::Cache);
            if parent
                .directory_names("active cache")?
                .iter()
                .any(|candidate| candidate == OsStr::new(&name))
            {
                let exact = candidate_has_exact_bytes(
                    &parent_path.join(&name),
                    parent,
                    bytes,
                    &payload_digest,
                )?;
                guard.validate_bound_entries()?;
                if !exact {
                    return Ok(false);
                }
                if existing_authorization.is_some() {
                    return Ok(false);
                }
                write_record(&write, ANALYSIS_CACHE_AUTHORIZATIONS, &name, &authorization)?;
                guard.commit(write)?;
                return Ok(true);
            }
            let fallback_name = format!(".analysis-cache-candidate-{}", nonce_hex()?);
            let unpublished = UnpublishedFile::create_with_named_fallback(
                &parent_path,
                parent,
                OsStr::new(&fallback_name),
            )?;
            unpublished.entry().replace_contents(bytes)?;
            if unpublished.entry().read_all()? != bytes {
                return Err(StoreError::Integrity(
                    "analysis cache candidate changed before publication".to_owned(),
                ));
            }
            guard.validate_bound_entries()?;
            match super::reject_active_cache_mutation_reservation(&write, None) {
                Ok(()) => {}
                Err(StoreError::OperationBusy(_)) => return Ok(false),
                Err(error) => return Err(error),
            }

            let published = match unpublished.publish_noreplace(
                parent,
                &parent_path,
                OsStr::new(&name),
                "analysis cache candidate",
                || guard.validate_bound_entries(),
            ) {
                Ok(published) => published,
                Err(error) => {
                    let target_exists = parent
                        .directory_names("active cache")?
                        .iter()
                        .any(|candidate| candidate == OsStr::new(&name));
                    guard.validate_bound_entries()?;
                    if target_exists {
                        return Ok(false);
                    }
                    return Err(error);
                }
            };
            if published.read_all()? != bytes {
                return Err(StoreError::Integrity(
                    "published analysis cache candidate changed contents".to_owned(),
                ));
            }
            parent.sync_directory()?;
            write_record(&write, ANALYSIS_CACHE_AUTHORIZATIONS, &name, &authorization)?;
            guard.validate_bound_entries()?;
            guard.commit(write)?;
            Ok(true)
        })
    }
}

pub(super) fn clear_analysis_cache_authorizations(
    write: &WriteTransaction,
    repository_id: &lumin_model::RepositoryId,
) -> Result<(), StoreError> {
    let table = write
        .open_table(ANALYSIS_CACHE_AUTHORIZATIONS)
        .map_err(backend_error)?;
    let rows = table
        .iter()
        .map_err(backend_error)?
        .map(|item| {
            item.map_err(backend_error)
                .map(|(key, value)| (key.value().to_owned(), value.value().to_vec()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (key, bytes) in &rows {
        let authorization = crate::decode_closed_json::<AnalysisCacheAuthorization>(bytes)
            .map_err(|error| {
                StoreError::Integrity(format!(
                    "analysis cache authorization {key} is malformed: {error}"
                ))
            })?;
        validate_analysis_cache_authorization(key, &authorization, Some(repository_id))?;
    }
    if !write.delete_table(table).map_err(backend_error)? {
        return Err(StoreError::Integrity(
            "analysis cache authorization table disappeared during cleanup".to_owned(),
        ));
    }
    Ok(())
}

fn candidate_has_exact_bytes(
    path: &std::path::Path,
    parent: &HeldEntry,
    expected_bytes: &[u8],
    expected_digest: &str,
) -> Result<bool, StoreError> {
    let entry = match HeldEntry::open(
        path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "existing analysis cache candidate",
    ) {
        Ok(entry) => entry,
        Err(_) => return Ok(false),
    };
    if !same_volume_and_mount(&entry, parent) {
        return Ok(false);
    }
    let first = match entry.read_all() {
        Ok(bytes) => bytes,
        Err(_) => return Ok(false),
    };
    if entry
        .validate_path(
            path,
            EntryKind::RegularFile,
            EntryAccess::ReadOnly,
            true,
            "existing analysis cache candidate",
        )
        .is_err()
    {
        return Ok(false);
    }
    let second = match entry.read_all() {
        Ok(bytes) => bytes,
        Err(_) => return Ok(false),
    };
    Ok(first == second && second == expected_bytes && digest_hex(&second) == expected_digest)
}

fn validate_supplied_input_key(key: &str) -> Result<(), StoreError> {
    if is_canonical_sha256(key) {
        Ok(())
    } else {
        Err(StoreError::Integrity(
            "analysis cache supplied-input key is not canonical SHA-256".to_owned(),
        ))
    }
}

fn candidate_digest<'a>(name: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{ANALYSIS_CACHE_PREFIX}{key}-");
    let digest = name
        .strip_prefix(&prefix)?
        .strip_suffix(ANALYSIS_CACHE_SUFFIX)?;
    is_canonical_sha256(digest).then_some(digest)
}
