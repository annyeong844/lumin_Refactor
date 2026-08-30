use std::ffi::OsStr;

use lumin_model::digest_hex;

use crate::namespace::records::ManagedStateParentKind;
use crate::namespace::{EntryAccess, EntryKind, HeldEntry, UnpublishedFile, same_volume_and_mount};
use crate::{RepositoryStore, StoreError, nonce_hex};

const ANALYSIS_CACHE_PREFIX: &str = "analysis-step-";
const ANALYSIS_CACHE_SUFFIX: &str = ".json";

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
                if first == second && digest_hex(&second) == expected_digest {
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
        let name = format!(
            "{ANALYSIS_CACHE_PREFIX}{supplied_input_key}-{payload_digest}{ANALYSIS_CACHE_SUFFIX}"
        );
        self.with_exclusive_lock(|guard| {
            let database = guard.open_database()?;
            let write = database.begin_write()?;
            match super::reject_active_cache_mutation_reservation(&write, None) {
                Ok(()) => {}
                Err(StoreError::OperationBusy(_)) => return Ok(false),
                Err(error) => return Err(error),
            }

            let parent = guard.managed_parent_entry(ManagedStateParentKind::Cache)?;
            if parent
                .directory_names("active cache")?
                .iter()
                .any(|candidate| candidate == OsStr::new(&name))
            {
                return Ok(false);
            }
            let parent_path = guard.managed_parent_path(ManagedStateParentKind::Cache);
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
            guard.validate_bound_entries()?;
            guard.commit(write)?;
            Ok(true)
        })
    }
}

fn validate_supplied_input_key(key: &str) -> Result<(), StoreError> {
    if is_lower_hex(key, 64) {
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
    is_lower_hex(digest, 64).then_some(digest)
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
