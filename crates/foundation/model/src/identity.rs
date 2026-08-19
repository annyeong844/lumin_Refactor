use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{PhysicalFileIdentity, RepoPath, RepositoryRootIdentity};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn from_string(value: String) -> Self {
                Self(value)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

string_id!(LogicalSourceId);
string_id!(PayloadSnapshotId);
string_id!(EmbeddedSourceUnitId);
string_id!(FindingId);
string_id!(EvidenceId);
string_id!(FindingRelationId);
string_id!(RunId);
string_id!(AttemptId);
string_id!(GateId);
string_id!(OperationId);
string_id!(RetentionPlanId);
string_id!(RetentionContentIdentity);
string_id!(RetentionTombstoneIdentity);
string_id!(CacheEvictionAuthorizationSetId);
string_id!(PinId);
string_id!(AnalysisInputId);
string_id!(RepositoryId);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttemptStatus {
    Running,
    Completed,
    Failed,
    Interrupted,
}

impl EvidenceId {
    pub fn for_source_span(
        kind: &str,
        source_id: &LogicalSourceId,
        start: u32,
        end: u32,
        payload_sha256: &str,
    ) -> Self {
        let mut bytes = Vec::new();
        append_length_prefixed(&mut bytes, b"lumin-evidence-id.v1");
        append_length_prefixed(&mut bytes, kind.as_bytes());
        append_length_prefixed(&mut bytes, source_id.as_str().as_bytes());
        bytes.extend_from_slice(&start.to_be_bytes());
        bytes.extend_from_slice(&end.to_be_bytes());
        append_length_prefixed(&mut bytes, payload_sha256.as_bytes());
        Self(format!("evidence_{}", digest_hex(&bytes)))
    }
}

impl RepositoryId {
    pub fn for_root(root: &RepositoryRootIdentity) -> Self {
        let mut bytes = Vec::new();
        append_length_prefixed(&mut bytes, b"lumin-repository-id.v1");
        append_length_prefixed(&mut bytes, root.canonical_bytes());
        Self(format!("repository_{}", digest_hex(&bytes)))
    }
}

impl CacheEvictionAuthorizationSetId {
    /// Bind one canonically ordered set of validated cache-eviction authorization rows.
    /// The store owns each row's framing; the model owns the stable set identity.
    pub fn for_canonical_rows(rows: &[Vec<u8>]) -> Self {
        let mut bytes = Vec::new();
        append_length_prefixed(&mut bytes, b"cache-eviction-authorization-set.v1");
        for row in rows {
            append_length_prefixed(&mut bytes, row);
        }
        Self(format!("cache_evictions_{}", digest_hex(&bytes)))
    }
}

impl LogicalSourceId {
    pub fn from_path(path: &RepoPath) -> Self {
        Self(format!("source_{}", digest_hex(path.canonical_bytes())))
    }
}

impl PayloadSnapshotId {
    pub fn for_capture(physical_identity: &PhysicalFileIdentity, payload_sha256: &str) -> Self {
        let mut bytes = Vec::new();
        append_length_prefixed(&mut bytes, b"lumin-payload-snapshot-id.v1");
        append_length_prefixed(&mut bytes, &physical_identity.canonical_bytes());
        append_length_prefixed(&mut bytes, payload_sha256.as_bytes());
        Self(format!("payload_{}", digest_hex(&bytes)))
    }
}

impl EmbeddedSourceUnitId {
    pub fn for_parent_span(
        parent: &LogicalSourceId,
        start: u32,
        end: u32,
        payload_sha256: &str,
    ) -> Self {
        let mut bytes = Vec::new();
        append_length_prefixed(&mut bytes, parent.as_str().as_bytes());
        bytes.extend_from_slice(&start.to_be_bytes());
        bytes.extend_from_slice(&end.to_be_bytes());
        append_length_prefixed(&mut bytes, payload_sha256.as_bytes());
        Self(format!("embedded_{}", digest_hex(&bytes)))
    }
}

impl FindingId {
    pub fn for_export(
        rule_id: &str,
        source_id: &LogicalSourceId,
        namespace: SymbolNamespace,
        export_name: &str,
    ) -> Self {
        let mut bytes = Vec::new();
        append_length_prefixed(&mut bytes, rule_id.as_bytes());
        append_length_prefixed(&mut bytes, source_id.as_str().as_bytes());
        bytes.push(namespace.tag());
        append_length_prefixed(&mut bytes, export_name.as_bytes());
        Self(format!("finding_{}", digest_hex(&bytes)))
    }
}

pub fn digest_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

pub fn append_length_prefixed(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_snapshot_identity_binds_physical_observation_and_exact_payload() {
        let left = PhysicalFileIdentity::Unix {
            device: 1,
            inode: 2,
        };
        let right = PhysicalFileIdentity::Unix {
            device: 1,
            inode: 3,
        };

        let baseline = PayloadSnapshotId::for_capture(&left, "payload-a");
        assert_eq!(baseline, PayloadSnapshotId::for_capture(&left, "payload-a"));
        assert_ne!(
            baseline,
            PayloadSnapshotId::for_capture(&right, "payload-a")
        );
        assert_ne!(baseline, PayloadSnapshotId::for_capture(&left, "payload-b"));
    }
}

use crate::SymbolNamespace;

/// Build-time identity for the compiled binary. Domain-separated and length-prefixed.
/// Same metadata + same registry rows intentionally share scope; no artifact-hash claim.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BuildIdentity(String);

impl BuildIdentity {
    /// Derive from package name, version, optional revision, and engine registry contract digest.
    pub fn derive(
        package_name: &str,
        package_version: &str,
        revision: Option<&str>,
        registry_contract_digest: &str,
    ) -> Self {
        let mut bytes = Vec::new();
        append_length_prefixed(&mut bytes, b"lumin-build-identity.v1");
        append_length_prefixed(&mut bytes, package_name.as_bytes());
        append_length_prefixed(&mut bytes, package_version.as_bytes());
        match revision {
            Some(rev) => {
                bytes.push(1);
                append_length_prefixed(&mut bytes, rev.as_bytes());
            }
            None => {
                bytes.push(0);
            }
        }
        append_length_prefixed(&mut bytes, registry_contract_digest.as_bytes());
        Self(format!("build_{}", digest_hex(&bytes)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_string(value: String) -> Self {
        Self(value)
    }
}
