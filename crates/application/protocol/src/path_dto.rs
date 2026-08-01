use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use lumin_evidence::RepoPathProjection;
use lumin_model::{RepoPath, RepositoryRootIdentity};
use serde::{Deserialize, Serialize};

use crate::ProtocolError;

const REPO_PATH_ENCODING: &str = "repo-path.v1";
const REPOSITORY_ROOT_ENCODING: &str = "repository-root.v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RepoPathDto {
    pub encoding: String,
    pub canonical_base64: String,
    pub display: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utf8: Option<String>,
}

impl RepoPathDto {
    pub fn from_path(path: &RepoPath) -> Self {
        Self {
            encoding: REPO_PATH_ENCODING.to_owned(),
            canonical_base64: STANDARD.encode(path.canonical_bytes()),
            display: path.display_escaped(),
            utf8: path.portable(),
        }
    }

    pub fn decode(&self) -> Result<RepoPath, ProtocolError> {
        if self.encoding != REPO_PATH_ENCODING {
            return Err(ProtocolError::InvalidRepoPathDto(
                "encoding must be repo-path.v1".to_owned(),
            ));
        }
        let canonical = decode_canonical_base64(&self.canonical_base64)
            .map_err(ProtocolError::InvalidRepoPathDto)?;
        let path = RepoPath::from_canonical_bytes(&canonical)
            .map_err(|error| ProtocolError::InvalidRepoPathDto(error.to_string()))?;
        if self.display != path.display_escaped() {
            return Err(ProtocolError::InvalidRepoPathDto(
                "display disagrees with canonicalBase64".to_owned(),
            ));
        }
        if let Some(utf8) = &self.utf8 {
            let projected = RepoPath::from_portable(utf8)
                .map_err(|error| ProtocolError::InvalidRepoPathDto(error.to_string()))?;
            if projected.canonical_bytes() != path.canonical_bytes() {
                return Err(ProtocolError::InvalidRepoPathDto(
                    "utf8 disagrees with canonicalBase64".to_owned(),
                ));
            }
        }
        Ok(path)
    }
}

impl From<&RepoPath> for RepoPathDto {
    fn from(path: &RepoPath) -> Self {
        Self::from_path(path)
    }
}

impl From<&RepoPathProjection> for RepoPathDto {
    fn from(path: &RepoPathProjection) -> Self {
        Self {
            encoding: REPO_PATH_ENCODING.to_owned(),
            canonical_base64: STANDARD.encode(&path.canonical),
            display: path.display.clone(),
            // Evidence projections preserve opaque canonical identity. An optional
            // Unicode projection is emitted only when the caller owns a RepoPath.
            utf8: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RepositoryRootDto {
    pub encoding: String,
    pub canonical_base64: String,
    pub display: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readable_address: Option<String>,
}

impl RepositoryRootDto {
    pub fn decode(&self) -> Result<RepositoryRootIdentity, ProtocolError> {
        if self.encoding != REPOSITORY_ROOT_ENCODING {
            return Err(ProtocolError::InvalidRepositoryRootDto(
                "encoding must be repository-root.v1".to_owned(),
            ));
        }
        let canonical = decode_canonical_base64(&self.canonical_base64)
            .map_err(ProtocolError::InvalidRepositoryRootDto)?;
        let root = RepositoryRootIdentity::from_canonical_bytes(&canonical)
            .map_err(|error| ProtocolError::InvalidRepositoryRootDto(error.to_string()))?;
        if self.display != root.display_escaped() {
            return Err(ProtocolError::InvalidRepositoryRootDto(
                "display disagrees with canonicalBase64".to_owned(),
            ));
        }
        if let Some(readable_address) = &self.readable_address
            && root.readable_address().as_ref() != Some(readable_address)
        {
            return Err(ProtocolError::InvalidRepositoryRootDto(
                "readableAddress disagrees with canonicalBase64".to_owned(),
            ));
        }
        Ok(root)
    }
}

impl From<&RepositoryRootIdentity> for RepositoryRootDto {
    fn from(root: &RepositoryRootIdentity) -> Self {
        Self {
            encoding: REPOSITORY_ROOT_ENCODING.to_owned(),
            canonical_base64: STANDARD.encode(root.canonical_bytes()),
            display: root.display_escaped(),
            readable_address: root.readable_address(),
        }
    }
}

fn decode_canonical_base64(value: &str) -> Result<Vec<u8>, String> {
    let decoded = STANDARD
        .decode(value)
        .map_err(|_| "canonicalBase64 is not padded RFC 4648 Base64".to_owned())?;
    if STANDARD.encode(&decoded) != value {
        return Err("canonicalBase64 is not canonical padded RFC 4648 Base64".to_owned());
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_path_dto_round_trips_and_rejects_projection_disagreement()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = RepoPath::from_portable("src/a.ts")?;
        let dto = RepoPathDto::from(&path);
        assert_eq!(dto.decode()?, path);
        assert_eq!(dto.utf8.as_deref(), Some("src/a.ts"));
        assert_eq!(
            serde_json::to_value(&dto)?,
            serde_json::json!({
                "encoding": "repo-path.v1",
                "canonicalBase64": "TFVNUlBBVEgAAQAAAAIBAAAAA3NyYwEAAAAEYS50cw==",
                "display": "src/a.ts",
                "utf8": "src/a.ts"
            })
        );

        let mut mismatch = dto;
        mismatch.utf8 = Some("src/b.ts".to_owned());
        assert!(matches!(
            mismatch.decode(),
            Err(ProtocolError::InvalidRepoPathDto(_))
        ));
        Ok(())
    }

    #[test]
    fn root_dto_round_trips_and_rejects_noncanonical_base64()
    -> Result<(), Box<dyn std::error::Error>> {
        let dto: RepositoryRootDto = serde_json::from_value(serde_json::json!({
            "encoding": "repository-root.v1",
            "canonicalBase64": "TFVNUlJPT1QAAQEBAAAAAQEAAAAEcmVwbwEAAAAAAAAAAQAAAAAAAAAC",
            "display": "/repo",
            "readableAddress": "/repo"
        }))?;
        let root = dto.decode()?;
        assert_eq!(RepositoryRootDto::from(&root), dto);

        let mut unpadded = dto;
        unpadded.canonical_base64.pop();
        assert!(matches!(
            unpadded.decode(),
            Err(ProtocolError::InvalidRepositoryRootDto(_))
        ));
        Ok(())
    }

    #[test]
    fn root_dto_rejects_parallel_identity_fields() {
        let value = serde_json::json!({
            "encoding": "repository-root.v1",
            "canonicalBase64": "TFVNUlJPT1QAAQEBAAAAAQEAAAAEcmVwbwEAAAAAAAAAAQAAAAAAAAAC",
            "display": "/repo",
            "platform": "unix"
        });
        assert!(serde_json::from_value::<RepositoryRootDto>(value).is_err());
    }
}
