use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Component, Path};

use lumin_model::PhysicalFileIdentity;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition, TableError};
use serde::{Deserialize, Serialize};

use crate::{StoreError, StoreGeneration, backend_error, serialization_error};

use super::super::platform::{EntryAccess, EntryKind, HeldEntry, UnpublishedFile};
use super::super::store_header::{PRIOR_STORE_HEADER_SCHEMA, STORE_HEADER_SCHEMA};
use super::super::{NamespaceGuard, entry_exists, require_state_volume};
use super::MigrationCrashPoint;

pub(super) const MIGRATION_ROOT_NAME: &str = "lifecycle-migration.json";
const MIGRATION_REVISION_PREFIX: &str = "lifecycle-migration.revision-";
const MIGRATION_REVISION_SUFFIX: &str = ".json";
const MIGRATION_ARTIFACT_PREFIX: &str = "lifecycle.store.migration-";
pub(super) const MIGRATION_ROOT_AUTHORIZATIONS_TABLE_NAME: &str = "migration-root-authorizations";
const MIGRATION_ROOT_AUTHORIZATIONS: TableDefinition<&str, &[u8]> =
    TableDefinition::new(MIGRATION_ROOT_AUTHORIZATIONS_TABLE_NAME);
const MIGRATION_JOURNAL_SCHEMA: &str = "lumin-lifecycle-migration-journal.v2";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MigrationRootAuthorization {
    pub(super) authorization_sequence: u64,
    pub(super) source_generation: StoreGeneration,
    pub(super) target_generation: StoreGeneration,
    pub(super) source_schema: String,
    pub(super) target_schema: String,
    pub(super) root_name: String,
    pub(super) root_physical_identity: PhysicalFileIdentity,
    pub(super) root_core_sha256: String,
    pub(super) source_user_logical_sha256: String,
    pub(super) target_user_logical_sha256: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum MigrationArtifactRole {
    Source,
    Target,
}

struct MigrationArtifactNames {
    pre_exchange: String,
    post_exchange: String,
}

struct MigrationArtifactDigests {
    bytes: String,
    logical: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MigrationArtifactBinding {
    pub(super) binding_id: String,
    pub(super) role: MigrationArtifactRole,
    pub(super) publication_attempt: u64,
    pub(super) pre_exchange_name: String,
    pub(super) post_exchange_name: String,
    pub(super) generation: StoreGeneration,
    pub(super) schema: String,
    pub(super) byte_sha256: String,
    pub(super) logical_sha256: String,
    pub(super) physical_identity: PhysicalFileIdentity,
    pub(super) link_count_at_publication: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MigrationPredecessor {
    name: String,
    physical_identity: PhysicalFileIdentity,
    payload_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub(super) enum MigrationBindingEvent {
    ObservedCanonicalSource { binding: MigrationArtifactBinding },
    PendingPublication { binding: MigrationArtifactBinding },
    Published { binding_id: String },
    SupersededUnpublished { binding_id: String },
    Exchanged { binding_id: String },
    RetainedImmutable { binding_id: String },
    CanonicalMutable { binding_id: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum MigrationPhase {
    ObservedSource,
    TargetPending,
    TargetPublished,
    Exchanged,
    Terminal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationIntent {
    schema_version: String,
    revision: u64,
    phase: MigrationPhase,
    authorization_sequence: u64,
    root_core_sha256: String,
    source_user_logical_sha256: String,
    target_user_logical_sha256: String,
    source_generation: StoreGeneration,
    target_generation: StoreGeneration,
    source_schema: String,
    target_schema: String,
    revision_physical_identity: PhysicalFileIdentity,
    predecessor: Option<MigrationPredecessor>,
    events: Vec<MigrationBindingEvent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MigrationBindingState {
    Observed,
    Pending,
    Published,
    Superseded,
    Exchanged,
    Retained,
    Canonical,
}

#[derive(Clone, Debug)]
pub(super) struct FoldedBinding {
    pub(super) binding: MigrationArtifactBinding,
    pub(super) state: MigrationBindingState,
}

#[derive(Debug)]
struct JournalEntry {
    name: String,
    entry: HeldEntry,
    payload_sha256: String,
    intent: MigrationIntent,
}

#[derive(Debug)]
pub(super) struct MigrationJournal {
    pub(super) root_authorization: MigrationRootAuthorization,
    pub(super) phase: MigrationPhase,
    pub(super) source: FoldedBinding,
    pub(super) targets: BTreeMap<String, FoldedBinding>,
    entries: Vec<JournalEntry>,
}

impl MigrationJournal {
    pub(super) fn target(&self) -> Result<&FoldedBinding, StoreError> {
        let mut current = self
            .targets
            .values()
            .filter(|target| target.state != MigrationBindingState::Superseded);
        let target = current.next().ok_or_else(|| {
            StoreError::Integrity("migration journal has no current target binding".to_owned())
        })?;
        if current.next().is_some() {
            return Err(StoreError::Integrity(
                "migration journal has multiple current target bindings".to_owned(),
            ));
        }
        Ok(target)
    }

    pub(super) fn next_target_publication_attempt(&self) -> Result<u64, StoreError> {
        target_publication_attempt_for_count(self.targets.len())
    }

    fn head(&self) -> Result<&JournalEntry, StoreError> {
        self.entries.last().ok_or_else(|| {
            StoreError::Integrity("migration journal omitted its authorized root".to_owned())
        })
    }
}

pub(super) fn root_core_sha256(
    root_physical_identity: &PhysicalFileIdentity,
    source_generation: StoreGeneration,
    target_generation: StoreGeneration,
    source_physical_identity: &PhysicalFileIdentity,
    source_user_logical_sha256: &str,
) -> Result<String, StoreError> {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": "lumin-lifecycle-migration-root-core.v1",
        "rootName": MIGRATION_ROOT_NAME,
        "rootPhysicalIdentity": root_physical_identity,
        "sourceGeneration": source_generation,
        "targetGeneration": target_generation,
        "sourceSchema": PRIOR_STORE_HEADER_SCHEMA,
        "targetSchema": STORE_HEADER_SCHEMA,
        "sourceName": "lifecycle.store",
        "sourcePhysicalIdentity": source_physical_identity,
        "sourceUserLogicalSha256": source_user_logical_sha256,
    }))
    .map_err(serialization_error)?;
    Ok(crate::digest_hex(&bytes))
}

pub(super) fn next_authorization_sequence(database: &Database) -> Result<u64, StoreError> {
    read_root_authorizations(database)?
        .last_key_value()
        .map_or(Ok(1), |(sequence, _)| {
            sequence.checked_add(1).ok_or_else(|| {
                StoreError::Integrity("migration root authorization sequence overflow".to_owned())
            })
        })
}

pub(super) fn append_root_authorization(
    database: &Database,
    authorization: &MigrationRootAuthorization,
) -> Result<(), StoreError> {
    validate_root_authorization(authorization)?;
    let observed = read_root_authorizations(database)?;
    let expected = observed.last_key_value().map_or(1, |(sequence, _)| {
        sequence.checked_add(1).unwrap_or(u64::MAX)
    });
    if authorization.authorization_sequence != expected {
        return Err(StoreError::Integrity(
            "migration root authorization sequence is not append-only".to_owned(),
        ));
    }
    let key = authorization_key(authorization.authorization_sequence);
    let bytes = serde_json::to_vec(authorization).map_err(serialization_error)?;
    let write = database.begin_write().map_err(backend_error)?;
    {
        let mut table = write
            .open_table(MIGRATION_ROOT_AUTHORIZATIONS)
            .map_err(backend_error)?;
        if table.get(key.as_str()).map_err(backend_error)?.is_some() {
            return Err(StoreError::Integrity(
                "migration root authorization key already exists".to_owned(),
            ));
        }
        table
            .insert(key.as_str(), bytes.as_slice())
            .map_err(backend_error)?;
    }
    write.commit().map_err(backend_error)
}

#[cfg(feature = "lifecycle-migration-test-fault")]
pub(super) fn remove_root_authorization_for_test(
    database: &Database,
    sequence: u64,
) -> Result<(), StoreError> {
    let key = authorization_key(sequence);
    let write = database.begin_write().map_err(backend_error)?;
    {
        let mut table = write
            .open_table(MIGRATION_ROOT_AUTHORIZATIONS)
            .map_err(backend_error)?;
        if table.remove(key.as_str()).map_err(backend_error)?.is_none() {
            return Err(StoreError::Integrity(
                "test root authorization row is missing".to_owned(),
            ));
        }
    }
    write.commit().map_err(backend_error)
}

pub(super) fn read_root_authorizations(
    database: &Database,
) -> Result<BTreeMap<u64, MigrationRootAuthorization>, StoreError> {
    let read = database.begin_read().map_err(backend_error)?;
    let table = match read.open_table(MIGRATION_ROOT_AUTHORIZATIONS) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(BTreeMap::new()),
        Err(error) => return Err(backend_error(error)),
    };
    let mut rows = BTreeMap::new();
    for row in table.iter().map_err(backend_error)? {
        let (key, value) = row.map_err(backend_error)?;
        let sequence = parse_authorization_key(key.value())?;
        let authorization = serde_json::from_slice::<MigrationRootAuthorization>(value.value())
            .map_err(|error| {
                StoreError::Integrity(format!(
                    "migration root authorization {sequence} is malformed: {error}"
                ))
            })?;
        validate_root_authorization(&authorization)?;
        let canonical = serde_json::to_vec(&authorization).map_err(serialization_error)?;
        if authorization.authorization_sequence != sequence || value.value() != canonical {
            return Err(StoreError::Integrity(
                "migration root authorization row is not canonical".to_owned(),
            ));
        }
        rows.insert(sequence, authorization);
    }
    for (index, sequence) in rows.keys().enumerate() {
        if *sequence != index as u64 + 1 {
            return Err(StoreError::Integrity(
                "migration root authorization history has a gap".to_owned(),
            ));
        }
    }
    Ok(rows)
}

pub(super) fn source_binding(
    physical_identity: PhysicalFileIdentity,
    post_exchange_name: String,
    generation: StoreGeneration,
    byte_sha256: String,
    logical_sha256: String,
) -> Result<MigrationArtifactBinding, StoreError> {
    build_binding(
        MigrationArtifactRole::Source,
        0,
        MigrationArtifactNames {
            pre_exchange: "lifecycle.store".to_owned(),
            post_exchange: post_exchange_name,
        },
        generation,
        MigrationArtifactDigests {
            bytes: byte_sha256,
            logical: logical_sha256,
        },
        physical_identity,
    )
}

pub(super) fn target_binding(
    physical_identity: PhysicalFileIdentity,
    publication_attempt: u64,
    pre_exchange_name: String,
    generation: StoreGeneration,
    byte_sha256: String,
    logical_sha256: String,
) -> Result<MigrationArtifactBinding, StoreError> {
    build_binding(
        MigrationArtifactRole::Target,
        publication_attempt,
        MigrationArtifactNames {
            pre_exchange: pre_exchange_name,
            post_exchange: "lifecycle.store".to_owned(),
        },
        generation,
        MigrationArtifactDigests {
            bytes: byte_sha256,
            logical: logical_sha256,
        },
        physical_identity,
    )
}

pub(super) fn publish_root(
    guard: &NamespaceGuard,
    unpublished: UnpublishedFile,
    authorization: &MigrationRootAuthorization,
    source: MigrationArtifactBinding,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<MigrationJournal, StoreError> {
    if entry_exists(&guard.state.state_dir.join(MIGRATION_ROOT_NAME))? {
        return Err(StoreError::Integrity(
            "migration root already exists before no-replace publication".to_owned(),
        ));
    }
    if unpublished.entry().identity() != &authorization.root_physical_identity {
        return Err(StoreError::Integrity(
            "migration root candidate identity disagrees with its authorization".to_owned(),
        ));
    }
    let intent = root_intent(authorization, unpublished.entry().identity(), source);
    write_candidate(
        unpublished.entry(),
        &intent_bytes(&intent)?,
        MigrationCrashPoint::RootCandidateWriteStarted,
        MigrationCrashPoint::RootCandidatePartiallyWritten,
        MigrationCrashPoint::RootCandidateWritten,
        hook,
    )?;
    let published = unpublished.publish_noreplace(
        &guard.state_directory,
        &guard.state.state_dir,
        OsStr::new(MIGRATION_ROOT_NAME),
        "lifecycle migration root",
        || hook(MigrationCrashPoint::RootNamePublished),
    )?;
    hook(MigrationCrashPoint::RootReopened)?;
    published.sync()?;
    hook(MigrationCrashPoint::RootFileFlushed)?;
    guard.state_directory.sync_directory()?;
    hook(MigrationCrashPoint::RootParentFlushed)?;
    read_journal(guard)?
        .ok_or_else(|| StoreError::Integrity("published migration root disappeared".to_owned()))
}

pub(super) fn append_revision(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
    phase: MigrationPhase,
    events: Vec<MigrationBindingEvent>,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<MigrationJournal, StoreError> {
    let revision = u64::try_from(journal.entries.len())
        .map_err(|_| StoreError::Integrity("migration revision overflow".to_owned()))?;
    let name = revision_name(revision);
    let unpublished = UnpublishedFile::create(&guard.state.state_dir, &guard.state_directory)?;
    hook(MigrationCrashPoint::RevisionCandidateCreated)?;
    let head = journal.head()?;
    let intent = MigrationIntent {
        schema_version: MIGRATION_JOURNAL_SCHEMA.to_owned(),
        revision,
        phase,
        authorization_sequence: journal.root_authorization.authorization_sequence,
        root_core_sha256: journal.root_authorization.root_core_sha256.clone(),
        source_user_logical_sha256: journal
            .root_authorization
            .source_user_logical_sha256
            .clone(),
        target_user_logical_sha256: journal
            .root_authorization
            .target_user_logical_sha256
            .clone(),
        source_generation: journal.root_authorization.source_generation,
        target_generation: journal.root_authorization.target_generation,
        source_schema: journal.root_authorization.source_schema.clone(),
        target_schema: journal.root_authorization.target_schema.clone(),
        revision_physical_identity: unpublished.entry().identity().clone(),
        predecessor: Some(MigrationPredecessor {
            name: head.name.clone(),
            physical_identity: head.entry.identity().clone(),
            payload_sha256: head.payload_sha256.clone(),
        }),
        events,
    };
    write_candidate(
        unpublished.entry(),
        &intent_bytes(&intent)?,
        MigrationCrashPoint::RevisionCandidateWriteStarted,
        MigrationCrashPoint::RevisionCandidatePartiallyWritten,
        MigrationCrashPoint::RevisionCandidateWritten,
        hook,
    )?;
    validate_journal_entry_path(guard, head)?;
    let published = unpublished.publish_noreplace(
        &guard.state_directory,
        &guard.state.state_dir,
        OsStr::new(&name),
        "lifecycle migration journal revision",
        || hook(MigrationCrashPoint::RevisionNamePublished),
    )?;
    hook(MigrationCrashPoint::RevisionReopened)?;
    published.sync()?;
    hook(MigrationCrashPoint::RevisionFileFlushed)?;
    validate_journal_entry_path(guard, head)?;
    guard.state_directory.sync_directory()?;
    hook(MigrationCrashPoint::RevisionParentFlushed)?;
    read_journal(guard)?.ok_or_else(|| {
        StoreError::Integrity("migration journal disappeared after append".to_owned())
    })
}

fn write_candidate(
    entry: &HeldEntry,
    bytes: &[u8],
    started: MigrationCrashPoint,
    partial: MigrationCrashPoint,
    complete: MigrationCrashPoint,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    entry.file().set_len(0).map_err(crate::io_error)?;
    let mut writer = entry.file();
    writer.seek(SeekFrom::Start(0)).map_err(crate::io_error)?;
    hook(started)?;
    let midpoint = bytes.len() / 2;
    writer
        .write_all(&bytes[..midpoint])
        .map_err(crate::io_error)?;
    hook(partial)?;
    writer
        .write_all(&bytes[midpoint..])
        .map_err(crate::io_error)?;
    writer.flush().map_err(crate::io_error)?;
    entry.sync()?;
    hook(complete)
}

pub(super) fn read_journal(guard: &NamespaceGuard) -> Result<Option<MigrationJournal>, StoreError> {
    let mut revisions = BTreeMap::new();
    let mut root_present = false;
    for item in fs::read_dir(&guard.state.state_dir).map_err(crate::io_error)? {
        let item = item.map_err(crate::io_error)?;
        let name = item.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == MIGRATION_ROOT_NAME {
            root_present = true;
        } else if name.starts_with(MIGRATION_REVISION_PREFIX) {
            let sequence = parse_revision_name(name)?;
            if revisions.insert(sequence, name.to_owned()).is_some() {
                return Err(StoreError::Integrity(
                    "duplicate migration journal revision".to_owned(),
                ));
            }
        } else if name.starts_with("lifecycle-migration") {
            return Err(StoreError::Integrity(format!(
                "noncanonical migration journal artifact is present: {name}"
            )));
        }
    }
    if !root_present {
        if revisions.is_empty() {
            return Ok(None);
        }
        return Err(StoreError::Integrity(
            "migration revisions exist without an authorized root".to_owned(),
        ));
    }
    for (index, sequence) in revisions.keys().enumerate() {
        if *sequence != index as u64 + 1 {
            return Err(StoreError::Integrity(
                "migration journal revision chain has a gap".to_owned(),
            ));
        }
    }
    let mut names = vec![MIGRATION_ROOT_NAME.to_owned()];
    names.extend(revisions.into_values());
    let entries = names
        .iter()
        .map(|name| read_journal_entry(guard, name))
        .collect::<Result<Vec<_>, _>>()?;
    validate_predecessors(&entries)?;
    fold_journal(guard, entries).map(Some)
}

pub(super) fn validate_root_authority(
    database: &Database,
    journal: &MigrationJournal,
) -> Result<(), StoreError> {
    let rows = read_root_authorizations(database)?;
    let Some((_, greatest)) = rows.last_key_value() else {
        return Err(StoreError::Integrity(
            "authorized migration root has no source authorization row".to_owned(),
        ));
    };
    if greatest != &journal.root_authorization {
        return Err(StoreError::Integrity(
            "migration root disagrees with the greatest source authorization".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn validate_binding_at(
    guard: &NamespaceGuard,
    binding: &MigrationArtifactBinding,
    name: &str,
    require_payload: bool,
    label: &str,
) -> Result<HeldEntry, StoreError> {
    validate_direct_name(name, label)?;
    let path = guard.state.state_dir.join(name);
    let entry = HeldEntry::open(
        &path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        label,
    )?;
    require_state_volume(&entry, &guard.state_directory, label)?;
    if entry.identity() != &binding.physical_identity || entry.links() != 1 {
        return Err(StoreError::Integrity(format!(
            "{label} physical binding changed"
        )));
    }
    if require_payload && file_sha256(&entry)? != binding.byte_sha256 {
        return Err(StoreError::Integrity(format!("{label} payload changed")));
    }
    Ok(entry)
}

pub(super) fn file_sha256(entry: &HeldEntry) -> Result<String, StoreError> {
    Ok(crate::digest_hex(&entry.read_all()?))
}

pub(super) fn target_name(binding_id: &str) -> String {
    format!("{MIGRATION_ARTIFACT_PREFIX}target-{binding_id}")
}

pub(super) fn source_retirement_name(_target_name: &str, source_binding_id: &str) -> String {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        let _ = source_binding_id;
        _target_name.to_owned()
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        format!("{MIGRATION_ARTIFACT_PREFIX}source-{source_binding_id}")
    }
}

fn root_intent(
    authorization: &MigrationRootAuthorization,
    root_identity: &PhysicalFileIdentity,
    source: MigrationArtifactBinding,
) -> MigrationIntent {
    MigrationIntent {
        schema_version: MIGRATION_JOURNAL_SCHEMA.to_owned(),
        revision: 0,
        phase: MigrationPhase::ObservedSource,
        authorization_sequence: authorization.authorization_sequence,
        root_core_sha256: authorization.root_core_sha256.clone(),
        source_user_logical_sha256: authorization.source_user_logical_sha256.clone(),
        target_user_logical_sha256: authorization.target_user_logical_sha256.clone(),
        source_generation: authorization.source_generation,
        target_generation: authorization.target_generation,
        source_schema: authorization.source_schema.clone(),
        target_schema: authorization.target_schema.clone(),
        revision_physical_identity: root_identity.clone(),
        predecessor: None,
        events: vec![MigrationBindingEvent::ObservedCanonicalSource { binding: source }],
    }
}

fn build_binding(
    role: MigrationArtifactRole,
    publication_attempt: u64,
    names: MigrationArtifactNames,
    generation: StoreGeneration,
    digests: MigrationArtifactDigests,
    physical_identity: PhysicalFileIdentity,
) -> Result<MigrationArtifactBinding, StoreError> {
    let schema = match role {
        MigrationArtifactRole::Source => PRIOR_STORE_HEADER_SCHEMA,
        MigrationArtifactRole::Target => STORE_HEADER_SCHEMA,
    };
    let mut binding = MigrationArtifactBinding {
        binding_id: String::new(),
        role,
        publication_attempt,
        pre_exchange_name: names.pre_exchange,
        post_exchange_name: names.post_exchange,
        generation,
        schema: schema.to_owned(),
        byte_sha256: digests.bytes,
        logical_sha256: digests.logical,
        physical_identity,
        link_count_at_publication: 1,
    };
    binding.binding_id = derive_binding_id(&binding)?;
    validate_binding(&binding)?;
    Ok(binding)
}

fn derive_binding_id(binding: &MigrationArtifactBinding) -> Result<String, StoreError> {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": "lumin-lifecycle-migration-artifact-binding.v2",
        "role": binding.role,
        "publicationAttempt": binding.publication_attempt,
        "preExchangeName": binding.pre_exchange_name,
        "postExchangeName": binding.post_exchange_name,
        "generation": binding.generation,
        "schema": binding.schema,
        "byteSha256": binding.byte_sha256,
        "logicalSha256": binding.logical_sha256,
        "physicalIdentity": binding.physical_identity,
        "linkCountAtPublication": binding.link_count_at_publication,
    }))
    .map_err(serialization_error)?;
    Ok(crate::digest_hex(&bytes))
}

fn validate_binding(binding: &MigrationArtifactBinding) -> Result<(), StoreError> {
    validate_direct_name(
        &binding.pre_exchange_name,
        "migration artifact pre-exchange name",
    )?;
    validate_direct_name(
        &binding.post_exchange_name,
        "migration artifact post-exchange name",
    )?;
    let expected_schema = match binding.role {
        MigrationArtifactRole::Source => PRIOR_STORE_HEADER_SCHEMA,
        MigrationArtifactRole::Target => STORE_HEADER_SCHEMA,
    };
    let valid_publication_attempt = match binding.role {
        MigrationArtifactRole::Source => binding.publication_attempt == 0,
        MigrationArtifactRole::Target => binding.publication_attempt > 0,
    };
    if binding.schema != expected_schema
        || !valid_publication_attempt
        || binding.link_count_at_publication != 1
        || !is_sha256(&binding.byte_sha256)
        || !is_sha256(&binding.logical_sha256)
        || derive_binding_id(binding)? != binding.binding_id
    {
        return Err(StoreError::Integrity(
            "migration artifact binding is malformed".to_owned(),
        ));
    }
    Ok(())
}

fn fold_journal(
    guard: &NamespaceGuard,
    entries: Vec<JournalEntry>,
) -> Result<MigrationJournal, StoreError> {
    let root = &entries[0].intent;
    if root.schema_version != MIGRATION_JOURNAL_SCHEMA
        || root.revision != 0
        || root.phase != MigrationPhase::ObservedSource
        || root.predecessor.is_some()
        || root.events.len() != 1
    {
        return Err(StoreError::Integrity(
            "migration root has an invalid initial revision".to_owned(),
        ));
    }
    let authorization = root_authorization_from_root(root, entries[0].entry.identity())?;
    let MigrationBindingEvent::ObservedCanonicalSource { binding: source } = &root.events[0] else {
        return Err(StoreError::Integrity(
            "migration root omitted its canonical source binding".to_owned(),
        ));
    };
    validate_binding(source)?;
    if source.role != MigrationArtifactRole::Source
        || source.generation != authorization.source_generation
        || source.logical_sha256 != authorization.source_user_logical_sha256
    {
        return Err(StoreError::Integrity(
            "migration source binding disagrees with root authorization".to_owned(),
        ));
    }
    let mut source = FoldedBinding {
        binding: source.clone(),
        state: MigrationBindingState::Observed,
    };
    let mut targets = BTreeMap::<String, FoldedBinding>::new();
    let mut ids = BTreeSet::from([source.binding.binding_id.clone()]);
    let mut last_phase = MigrationPhase::ObservedSource;
    for entry in entries.iter().skip(1) {
        validate_revision_common(&entry.intent, &authorization)?;
        if phase_rank(entry.intent.phase) < phase_rank(last_phase) {
            return Err(StoreError::Integrity(
                "migration journal phase regressed".to_owned(),
            ));
        }
        for event in &entry.intent.events {
            apply_event(event, &mut source, &mut targets, &mut ids)?;
        }
        last_phase = entry.intent.phase;
    }
    validate_phase(last_phase, &source, &targets)?;
    let current_target_name = targets
        .values()
        .find(|target| target.state != MigrationBindingState::Superseded)
        .map(|target| target.binding.pre_exchange_name.as_str());
    for target in targets.values() {
        if target.state == MigrationBindingState::Superseded
            && current_target_name != Some(target.binding.pre_exchange_name.as_str())
            && entry_exists(
                &guard
                    .state
                    .state_dir
                    .join(&target.binding.pre_exchange_name),
            )?
        {
            return Err(StoreError::Integrity(
                "superseded unpublished migration target became visible".to_owned(),
            ));
        }
    }
    Ok(MigrationJournal {
        root_authorization: authorization,
        phase: last_phase,
        source,
        targets,
        entries,
    })
}

fn apply_event(
    event: &MigrationBindingEvent,
    source: &mut FoldedBinding,
    targets: &mut BTreeMap<String, FoldedBinding>,
    ids: &mut BTreeSet<String>,
) -> Result<(), StoreError> {
    match event {
        MigrationBindingEvent::ObservedCanonicalSource { .. } => Err(StoreError::Integrity(
            "migration source binding was initialized more than once".to_owned(),
        )),
        MigrationBindingEvent::PendingPublication { binding } => {
            validate_binding(binding)?;
            if binding.role != MigrationArtifactRole::Target {
                return Err(StoreError::Integrity(
                    "migration target binding has the wrong role".to_owned(),
                ));
            }
            let expected_attempt = target_publication_attempt_for_count(targets.len())?;
            if binding.publication_attempt != expected_attempt {
                return Err(StoreError::Integrity(
                    "migration target publication attempts are not contiguous".to_owned(),
                ));
            }
            if !ids.insert(binding.binding_id.clone()) {
                return Err(StoreError::Integrity(
                    "migration target binding is duplicate".to_owned(),
                ));
            }
            if targets
                .values()
                .any(|target| target.state != MigrationBindingState::Superseded)
            {
                return Err(StoreError::Integrity(
                    "migration journal initialized a second current target".to_owned(),
                ));
            }
            targets.insert(
                binding.binding_id.clone(),
                FoldedBinding {
                    binding: binding.clone(),
                    state: MigrationBindingState::Pending,
                },
            );
            Ok(())
        }
        MigrationBindingEvent::Published { binding_id } => transition_target(
            targets,
            binding_id,
            MigrationBindingState::Pending,
            MigrationBindingState::Published,
        ),
        MigrationBindingEvent::SupersededUnpublished { binding_id } => transition_target(
            targets,
            binding_id,
            MigrationBindingState::Pending,
            MigrationBindingState::Superseded,
        ),
        MigrationBindingEvent::Exchanged { binding_id }
            if binding_id == &source.binding.binding_id =>
        {
            transition(
                &mut source.state,
                MigrationBindingState::Observed,
                MigrationBindingState::Exchanged,
                "migration source",
            )
        }
        MigrationBindingEvent::Exchanged { binding_id } => transition_target(
            targets,
            binding_id,
            MigrationBindingState::Published,
            MigrationBindingState::Exchanged,
        ),
        MigrationBindingEvent::RetainedImmutable { binding_id }
            if binding_id == &source.binding.binding_id =>
        {
            transition(
                &mut source.state,
                MigrationBindingState::Exchanged,
                MigrationBindingState::Retained,
                "migration source",
            )
        }
        MigrationBindingEvent::RetainedImmutable { .. } => Err(StoreError::Integrity(
            "migration target cannot become retained source provenance".to_owned(),
        )),
        MigrationBindingEvent::CanonicalMutable { binding_id } => transition_target(
            targets,
            binding_id,
            MigrationBindingState::Exchanged,
            MigrationBindingState::Canonical,
        ),
    }
}

fn transition_target(
    targets: &mut BTreeMap<String, FoldedBinding>,
    binding_id: &str,
    from: MigrationBindingState,
    to: MigrationBindingState,
) -> Result<(), StoreError> {
    let target = targets.get_mut(binding_id).ok_or_else(|| {
        StoreError::Integrity("migration event names an unknown target binding".to_owned())
    })?;
    transition(&mut target.state, from, to, "migration target")
}

fn transition(
    state: &mut MigrationBindingState,
    from: MigrationBindingState,
    to: MigrationBindingState,
    label: &str,
) -> Result<(), StoreError> {
    if *state != from {
        return Err(StoreError::Integrity(format!(
            "{label} binding transition is out of order"
        )));
    }
    *state = to;
    Ok(())
}

fn validate_phase(
    phase: MigrationPhase,
    source: &FoldedBinding,
    targets: &BTreeMap<String, FoldedBinding>,
) -> Result<(), StoreError> {
    let current = targets
        .values()
        .find(|target| target.state != MigrationBindingState::Superseded);
    let valid = match phase {
        MigrationPhase::ObservedSource => {
            source.state == MigrationBindingState::Observed && current.is_none()
        }
        MigrationPhase::TargetPending => {
            source.state == MigrationBindingState::Observed
                && current.is_some_and(|target| target.state == MigrationBindingState::Pending)
        }
        MigrationPhase::TargetPublished => {
            source.state == MigrationBindingState::Observed
                && current.is_some_and(|target| target.state == MigrationBindingState::Published)
        }
        MigrationPhase::Exchanged => {
            source.state == MigrationBindingState::Exchanged
                && current.is_some_and(|target| target.state == MigrationBindingState::Exchanged)
        }
        MigrationPhase::Terminal => {
            source.state == MigrationBindingState::Retained
                && current.is_some_and(|target| target.state == MigrationBindingState::Canonical)
        }
    };
    if !valid {
        return Err(StoreError::Integrity(
            "migration journal phase disagrees with folded artifact states".to_owned(),
        ));
    }
    Ok(())
}

fn root_authorization_from_root(
    root: &MigrationIntent,
    observed_identity: &PhysicalFileIdentity,
) -> Result<MigrationRootAuthorization, StoreError> {
    if &root.revision_physical_identity != observed_identity {
        return Err(StoreError::Integrity(
            "migration root self-binding changed".to_owned(),
        ));
    }
    let authorization = MigrationRootAuthorization {
        authorization_sequence: root.authorization_sequence,
        source_generation: root.source_generation,
        target_generation: root.target_generation,
        source_schema: root.source_schema.clone(),
        target_schema: root.target_schema.clone(),
        root_name: MIGRATION_ROOT_NAME.to_owned(),
        root_physical_identity: root.revision_physical_identity.clone(),
        root_core_sha256: root.root_core_sha256.clone(),
        source_user_logical_sha256: root.source_user_logical_sha256.clone(),
        target_user_logical_sha256: root.target_user_logical_sha256.clone(),
    };
    validate_root_authorization(&authorization)?;
    Ok(authorization)
}

fn validate_revision_common(
    revision: &MigrationIntent,
    authorization: &MigrationRootAuthorization,
) -> Result<(), StoreError> {
    if revision.schema_version != MIGRATION_JOURNAL_SCHEMA
        || revision.authorization_sequence != authorization.authorization_sequence
        || revision.root_core_sha256 != authorization.root_core_sha256
        || revision.source_user_logical_sha256 != authorization.source_user_logical_sha256
        || revision.target_user_logical_sha256 != authorization.target_user_logical_sha256
        || revision.source_generation != authorization.source_generation
        || revision.target_generation != authorization.target_generation
        || revision.source_schema != authorization.source_schema
        || revision.target_schema != authorization.target_schema
        || revision.events.is_empty()
    {
        return Err(StoreError::Integrity(
            "migration journal revision disagrees with its authorized root".to_owned(),
        ));
    }
    Ok(())
}

fn validate_predecessors(entries: &[JournalEntry]) -> Result<(), StoreError> {
    for (index, entry) in entries.iter().enumerate() {
        if entry.intent.revision != index as u64
            || entry.intent.revision_physical_identity != *entry.entry.identity()
        {
            return Err(StoreError::Integrity(
                "migration journal revision self-binding changed".to_owned(),
            ));
        }
        if index == 0 {
            continue;
        }
        let predecessor = entry.intent.predecessor.as_ref().ok_or_else(|| {
            StoreError::Integrity("migration journal successor omitted its predecessor".to_owned())
        })?;
        let expected = &entries[index - 1];
        if predecessor.name != expected.name
            || predecessor.physical_identity != *expected.entry.identity()
            || predecessor.payload_sha256 != expected.payload_sha256
        {
            return Err(StoreError::Integrity(
                "migration journal predecessor binding changed".to_owned(),
            ));
        }
    }
    Ok(())
}

fn read_journal_entry(guard: &NamespaceGuard, name: &str) -> Result<JournalEntry, StoreError> {
    validate_direct_name(name, "migration journal name")?;
    let path = guard.state.state_dir.join(name);
    let entry = HeldEntry::open(
        &path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "lifecycle migration journal revision",
    )?;
    require_state_volume(
        &entry,
        &guard.state_directory,
        "lifecycle migration journal revision",
    )?;
    let bytes = entry.read_all()?;
    let intent = serde_json::from_slice::<MigrationIntent>(&bytes).map_err(|error| {
        StoreError::Integrity(format!(
            "lifecycle migration journal revision is malformed: {error}"
        ))
    })?;
    if bytes != intent_bytes(&intent)? {
        return Err(StoreError::Integrity(
            "lifecycle migration journal revision bytes are not canonical".to_owned(),
        ));
    }
    Ok(JournalEntry {
        name: name.to_owned(),
        entry,
        payload_sha256: crate::digest_hex(&bytes),
        intent,
    })
}

fn validate_journal_entry_path(
    guard: &NamespaceGuard,
    entry: &JournalEntry,
) -> Result<(), StoreError> {
    entry.entry.validate_path(
        &guard.state.state_dir.join(&entry.name),
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "lifecycle migration journal predecessor",
    )
}

fn intent_bytes(intent: &MigrationIntent) -> Result<Vec<u8>, StoreError> {
    let mut bytes = serde_json::to_vec(intent).map_err(serialization_error)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn validate_root_authorization(
    authorization: &MigrationRootAuthorization,
) -> Result<(), StoreError> {
    if authorization.authorization_sequence == 0
        || authorization.source_generation.checked_next() != Some(authorization.target_generation)
        || authorization.source_schema != PRIOR_STORE_HEADER_SCHEMA
        || authorization.target_schema != STORE_HEADER_SCHEMA
        || authorization.root_name != MIGRATION_ROOT_NAME
        || !is_sha256(&authorization.root_core_sha256)
        || !is_sha256(&authorization.source_user_logical_sha256)
        || !is_sha256(&authorization.target_user_logical_sha256)
    {
        return Err(StoreError::Integrity(
            "migration root authorization is malformed".to_owned(),
        ));
    }
    Ok(())
}

fn authorization_key(sequence: u64) -> String {
    format!("{sequence:016}")
}

fn parse_authorization_key(key: &str) -> Result<u64, StoreError> {
    if key.len() != 16 || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(StoreError::Integrity(
            "migration root authorization key is noncanonical".to_owned(),
        ));
    }
    key.parse::<u64>().map_err(|_| {
        StoreError::Integrity("migration root authorization key overflowed".to_owned())
    })
}

fn revision_name(revision: u64) -> String {
    format!("{MIGRATION_REVISION_PREFIX}{revision:016}{MIGRATION_REVISION_SUFFIX}")
}

fn parse_revision_name(name: &str) -> Result<u64, StoreError> {
    let digits = name
        .strip_prefix(MIGRATION_REVISION_PREFIX)
        .and_then(|rest| rest.strip_suffix(MIGRATION_REVISION_SUFFIX))
        .ok_or_else(|| StoreError::Integrity("migration revision name is invalid".to_owned()))?;
    if digits.len() != 16 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(StoreError::Integrity(
            "migration revision name is noncanonical".to_owned(),
        ));
    }
    let revision = digits
        .parse::<u64>()
        .map_err(|_| StoreError::Integrity("migration revision name overflowed".to_owned()))?;
    if revision == 0 || revision_name(revision) != name {
        return Err(StoreError::Integrity(
            "migration revision name is noncanonical".to_owned(),
        ));
    }
    Ok(revision)
}

fn validate_direct_name(name: &str, label: &str) -> Result<(), StoreError> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(StoreError::Integrity(format!(
            "{label} must be one normal path component"
        )));
    }
    Ok(())
}

fn phase_rank(phase: MigrationPhase) -> u8 {
    match phase {
        MigrationPhase::ObservedSource => 0,
        MigrationPhase::TargetPending => 1,
        MigrationPhase::TargetPublished => 2,
        MigrationPhase::Exchanged => 3,
        MigrationPhase::Terminal => 4,
    }
}

fn target_publication_attempt_for_count(target_count: usize) -> Result<u64, StoreError> {
    u64::try_from(target_count)
        .ok()
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| {
            StoreError::Integrity("migration target publication attempts exhausted".to_owned())
        })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superseded_target_attempt_remains_unique_when_physical_identity_is_reused()
    -> Result<(), StoreError> {
        let digest = "0".repeat(64);
        let source_binding = source_binding(
            PhysicalFileIdentity::Unix {
                device: 7,
                inode: 10,
            },
            "lifecycle.store.migration-source".to_owned(),
            StoreGeneration::INITIAL,
            digest.clone(),
            digest.clone(),
        )?;
        let target_generation = StoreGeneration::INITIAL
            .checked_next()
            .ok_or_else(|| StoreError::Integrity("test target generation overflowed".to_owned()))?;
        let reused_identity = PhysicalFileIdentity::Unix {
            device: 7,
            inode: 11,
        };
        let first = target_binding(
            reused_identity.clone(),
            1,
            "lifecycle.store.migration-target".to_owned(),
            target_generation,
            digest.clone(),
            digest.clone(),
        )?;
        let second = target_binding(
            reused_identity,
            2,
            first.pre_exchange_name.clone(),
            target_generation,
            digest.clone(),
            digest,
        )?;
        assert_ne!(first.binding_id, second.binding_id);

        let mut source = FoldedBinding {
            binding: source_binding.clone(),
            state: MigrationBindingState::Observed,
        };
        let mut targets = BTreeMap::new();
        let mut ids = BTreeSet::from([source_binding.binding_id]);
        apply_event(
            &MigrationBindingEvent::PendingPublication {
                binding: first.clone(),
            },
            &mut source,
            &mut targets,
            &mut ids,
        )?;
        apply_event(
            &MigrationBindingEvent::SupersededUnpublished {
                binding_id: first.binding_id,
            },
            &mut source,
            &mut targets,
            &mut ids,
        )?;
        apply_event(
            &MigrationBindingEvent::PendingPublication {
                binding: second.clone(),
            },
            &mut source,
            &mut targets,
            &mut ids,
        )?;

        assert_eq!(targets.len(), 2);
        assert_eq!(
            targets.get(&second.binding_id).map(|target| target.state),
            Some(MigrationBindingState::Pending)
        );
        Ok(())
    }
}
