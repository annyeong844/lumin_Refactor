use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::RepoPath;

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
string_id!(EmbeddedSourceUnitId);
string_id!(FindingId);
string_id!(RunId);
string_id!(AttemptId);

impl LogicalSourceId {
    pub fn from_path(path: &RepoPath) -> Self {
        Self(format!("source_{}", digest_hex(path.canonical_bytes())))
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
        append_field(&mut bytes, parent.as_str().as_bytes());
        bytes.extend_from_slice(&start.to_be_bytes());
        bytes.extend_from_slice(&end.to_be_bytes());
        append_field(&mut bytes, payload_sha256.as_bytes());
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
        append_field(&mut bytes, rule_id.as_bytes());
        append_field(&mut bytes, source_id.as_str().as_bytes());
        bytes.push(namespace.tag());
        append_field(&mut bytes, export_name.as_bytes());
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

fn append_field(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value);
}

use crate::SymbolNamespace;
