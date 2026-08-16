use std::collections::BTreeSet;
use std::path::Path;

use lumin_evidence::{AnalysisSnapshot, RepoPathProjection};
use lumin_inventory::ReservedStateIdentityLookup;
use lumin_model::{PhysicalFileIdentity, RepoPath};
use lumin_store::{AttemptSession, PublishedRun, RepositoryStore, StoreError};

pub(super) fn publish(
    store: &RepositoryStore,
    attempt: &mut AttemptSession<'_>,
    root: &Path,
    reserved_state_lookup: &ReservedStateIdentityLookup,
    snapshot: &AnalysisSnapshot,
) -> Result<PublishedRun, StoreError> {
    store.publish_run(attempt, &snapshot.evidence, |reserved_identities| {
        validate_snapshot(
            root,
            &snapshot.inputs,
            reserved_state_lookup,
            reserved_identities,
        )
    })
}

fn validate_snapshot(
    root: &Path,
    inputs: &[lumin_evidence::SemanticInputRecord],
    reserved_state_lookup: &ReservedStateIdentityLookup,
    reserved_identities: &BTreeSet<PhysicalFileIdentity>,
) -> Result<(), StoreError> {
    let final_lookup = reserved_state_lookup.for_final_validation(reserved_identities);
    for input in inputs {
        if let Some(sha256) = &input.physical_redirect_sha256 {
            validate_redirect_path(root, &input.path, sha256, reserved_identities)?;
        } else if let Some(identity) = &input.physical_identity {
            validate_input_path(root, &input.path, identity, &final_lookup)?;
        }
        if let Some(parent) = &input.absence_parent {
            validate_input_path(root, &parent.path, &parent.physical_identity, &final_lookup)?;
        }
    }
    Ok(())
}

fn validate_redirect_path(
    root: &Path,
    projection: &RepoPathProjection,
    expected_sha256: &str,
    reserved_state_identities: &BTreeSet<PhysicalFileIdentity>,
) -> Result<(), StoreError> {
    let path = decode_input_path(projection)?;
    lumin_inventory::validate_captured_physical_path_redirect(
        root,
        &path,
        expected_sha256,
        reserved_state_identities,
    )
    .map_err(|error| {
        StoreError::Integrity(format!(
            "captured audit redirect changed before publication ({}): {error}",
            projection.display
        ))
    })
}

fn validate_input_path(
    root: &Path,
    projection: &RepoPathProjection,
    expected_identity: &PhysicalFileIdentity,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<(), StoreError> {
    let path = decode_input_path(projection)?;
    lumin_inventory::validate_captured_semantic_input_topology(
        root,
        &path,
        expected_identity,
        reserved_state_lookup,
    )
    .map_err(|error| {
        StoreError::Integrity(format!(
            "captured audit input changed before publication ({}): {error}",
            projection.display
        ))
    })
}

fn decode_input_path(projection: &RepoPathProjection) -> Result<RepoPath, StoreError> {
    let path = RepoPath::from_canonical_bytes(&projection.canonical).map_err(|error| {
        StoreError::Integrity(format!(
            "captured audit input path is corrupt before publication ({}): {error}",
            projection.display
        ))
    })?;
    if &RepoPathProjection::from(&path) != projection {
        return Err(StoreError::Integrity(format!(
            "captured audit input projection changed before publication: {}",
            projection.display
        )));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use lumin_inventory::{InventoryRequest, repository_admission};

    use super::*;
    use crate::{capture_admitted_repository, reserved_state_identity_lookup};

    #[test]
    fn late_reserved_alias_stops_audit_before_run_publication()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        fs::create_dir(fixture.path().join("src"))?;
        let source = fixture.path().join("src/lib.ts");
        fs::write(&source, "export const value = 1;\n")?;
        let admission = repository_admission(fixture.path())?;
        let store = RepositoryStore::open(&admission.canonical_root, &admission.binding)?;
        let reserved_state_lookup = reserved_state_identity_lookup(&store);
        let mut attempt = store.begin_attempt()?;
        let capture = capture_admitted_repository(
            &admission.canonical_root,
            admission.binding.root().clone(),
            &InventoryRequest::default(),
            1,
            None,
            &reserved_state_lookup,
        )?;

        fs::hard_link(
            &source,
            admission.canonical_root.join(".lumin/cache/late-alias.ts"),
        )?;
        let error = match publish(
            &store,
            &mut attempt,
            &admission.canonical_root,
            &reserved_state_lookup,
            &capture.snapshot,
        ) {
            Err(error) => error,
            Ok(_) => return Err("a late reserved-state alias published audit evidence".into()),
        };

        assert!(matches!(
            error,
            StoreError::Integrity(detail)
                if detail.contains("captured audit input changed before publication")
                    && detail.contains("src/lib.ts")
        ));
        assert!(
            fs::read_dir(admission.canonical_root.join(".lumin/runs"))?
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().starts_with("run_")),
            "failed final validation left a publishable run directory",
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unchanged_redirect_can_publish_audit() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        fs::create_dir_all(fixture.path().join("packages/lib"))?;
        fs::write(
            outside.path().join("target.ts"),
            "export const value = 1;\n",
        )?;
        symlink(
            outside.path().join("target.ts"),
            fixture.path().join("packages/lib/escape.ts"),
        )?;
        let admission = repository_admission(fixture.path())?;
        let store = RepositoryStore::open(&admission.canonical_root, &admission.binding)?;
        let reserved_state_lookup = reserved_state_identity_lookup(&store);
        let mut attempt = store.begin_attempt()?;
        let capture = capture_admitted_repository(
            &admission.canonical_root,
            admission.binding.root().clone(),
            &InventoryRequest::default(),
            1,
            None,
            &reserved_state_lookup,
        )?;

        publish(
            &store,
            &mut attempt,
            &admission.canonical_root,
            &reserved_state_lookup,
            &capture.snapshot,
        )?;
        Ok(())
    }
}
