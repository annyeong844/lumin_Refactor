use lumin_model::{
    CacheEvictionAuthorizationSetId, OperationId, PhysicalFileIdentity, RepositoryId,
};
use serde::{Deserialize, Serialize};

use crate::OperationLivenessLease;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheEvictionComponentKey {
    pub canonical: Vec<u8>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheEvictionPathKey {
    pub components: Vec<CacheEvictionComponentKey>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheEvictionEntryKind {
    Directory,
    RegularFile,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheEvictionManifestRow {
    pub relative_path: CacheEvictionPathKey,
    pub kind: CacheEvictionEntryKind,
    pub physical_identity: PhysicalFileIdentity,
    pub link_count: u64,
    pub byte_length: Option<u64>,
    pub payload_sha256: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheEvictionManifest {
    pub rows: Vec<CacheEvictionManifestRow>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheEvictionAuthorizationState {
    Authorized,
    Validated,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheEvictionAuthorization {
    pub schema_version: String,
    pub repository_id: RepositoryId,
    pub operation_id: OperationId,
    pub request_digest: String,
    pub invocation_id: String,
    pub ordinal: u64,
    pub source_component: CacheEvictionComponentKey,
    pub destination_component: CacheEvictionComponentKey,
    pub manifest_digest: String,
    pub expected_manifest: CacheEvictionManifest,
    pub state: CacheEvictionAuthorizationState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheCleanupOperationStatus {
    Pending,
    Interrupted,
    Committed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheCleanupDeliveryStatus {
    NotAttempted,
    Unknown,
    Succeeded,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheCleanupDeliveryOutcome {
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheCleanupDeliveryCompletion {
    pub sequence: u64,
    pub outcome: CacheCleanupDeliveryOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheCleanupResult {
    pub operation_id: OperationId,
    pub request_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheCleanupExecutionLease {
    pub execution_attempt_id: String,
    pub liveness: OperationLivenessLease,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheCleanupRecoveryReservation {
    pub interrupted_execution_attempt_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheCleanupOperationRecord {
    pub schema_version: String,
    pub repository_id: RepositoryId,
    pub operation_id: OperationId,
    pub request_digest: String,
    pub status: CacheCleanupOperationStatus,
    pub interruption_count: u64,
    pub invocation_id: String,
    pub initial_authorization_set_id: CacheEvictionAuthorizationSetId,
    pub initial_authorization_count: u64,
    pub plan_initialized: bool,
    pub authorization_keys: Vec<String>,
    pub validated_count: u64,
    pub execution_lease: Option<CacheCleanupExecutionLease>,
    pub recovery_reservation: Option<CacheCleanupRecoveryReservation>,
    pub result: Option<CacheCleanupResult>,
    pub greatest_allocated_delivery_sequence: u64,
    pub greatest_completed_delivery_sequence: Option<u64>,
    pub delivery_completions: Vec<CacheCleanupDeliveryCompletion>,
}

impl CacheCleanupOperationRecord {
    pub fn authorized_count(&self) -> u64 {
        self.authorization_keys.len() as u64
    }

    pub fn last_delivery_status(&self) -> CacheCleanupDeliveryStatus {
        if self.greatest_allocated_delivery_sequence == 0 {
            return CacheCleanupDeliveryStatus::NotAttempted;
        }
        self.delivery_completions
            .binary_search_by_key(&self.greatest_allocated_delivery_sequence, |completion| {
                completion.sequence
            })
            .ok()
            .map_or(CacheCleanupDeliveryStatus::Unknown, |index| {
                match self.delivery_completions[index].outcome {
                    CacheCleanupDeliveryOutcome::Succeeded => CacheCleanupDeliveryStatus::Succeeded,
                    CacheCleanupDeliveryOutcome::Failed => CacheCleanupDeliveryStatus::Failed,
                }
            })
    }
}
