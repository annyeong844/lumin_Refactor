use lumin_evidence::{SourceContextRecord, SourceObservationRecord};
use lumin_model::{PayloadSnapshotId, PhysicalFileIdentity, SourceKind};
use serde::Serialize;

use crate::RepoPathDto;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceContextDto {
    pub source_id: String,
    pub path: RepoPathDto,
    pub kind: SourceKind,
    pub package_root: Option<RepoPathDto>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceObservationDto {
    pub source_id: String,
    pub physical_identity: PhysicalFileIdentity,
    pub payload_snapshot_id: PayloadSnapshotId,
}

impl From<&SourceContextRecord> for SourceContextDto {
    fn from(record: &SourceContextRecord) -> Self {
        Self {
            source_id: record.source_id.as_str().to_owned(),
            path: RepoPathDto::from(&record.path),
            kind: record.kind,
            package_root: record.package_root.as_ref().map(RepoPathDto::from),
        }
    }
}

impl From<&SourceObservationRecord> for SourceObservationDto {
    fn from(record: &SourceObservationRecord) -> Self {
        Self {
            source_id: record.source_id.as_str().to_owned(),
            physical_identity: record.physical_identity.clone(),
            payload_snapshot_id: record.payload_snapshot_id.clone(),
        }
    }
}
