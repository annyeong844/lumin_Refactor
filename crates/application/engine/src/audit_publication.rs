use std::collections::BTreeSet;
use std::path::Path;

use lumin_evidence::{AnalysisSnapshot, RepoPathProjection, SemanticInputState};
use lumin_inventory::{
    ReservedStateIdentityLookup, SemanticInputExpectation, SemanticInputValidationState,
};
use lumin_model::{ConfigAbsenceParent, PhysicalFileIdentity, RepoPath};
use lumin_store::{AttemptSession, PublishedRun, RepositoryStore, StoreError};
use rayon::prelude::*;

pub(super) fn publish(
    store: &RepositoryStore,
    attempt: &mut AttemptSession<'_>,
    root: &Path,
    reserved_state_lookup: &ReservedStateIdentityLookup,
    snapshot: &AnalysisSnapshot,
    #[cfg(feature = "audit-execution-test-profile")] mut profile: Option<
        &mut super::audit_profile::AuditProfiler,
    >,
) -> Result<PublishedRun, StoreError> {
    audit_phase_begin!(profile, EvidencePrepare);
    let evidence = lumin_store::prepare_run_evidence(&snapshot.evidence)?;
    audit_phase_end!(profile, EvidencePrepare);
    audit_phase_begin!(profile, StorePublish);
    let result = store.publish_run_with_preflight(attempt, |reserved_identities| {
        audit_phase_begin!(profile, FinalInputs);
        validate_snapshot(
            root,
            &snapshot.inputs,
            reserved_state_lookup,
            reserved_identities,
        )?;
        audit_phase_end!(profile, FinalInputs);
        Ok(evidence)
    });
    audit_phase_end!(profile, StorePublish);
    result
}

fn validate_snapshot(
    root: &Path,
    inputs: &[lumin_evidence::SemanticInputRecord],
    reserved_state_lookup: &ReservedStateIdentityLookup,
    reserved_identities: &BTreeSet<PhysicalFileIdentity>,
) -> Result<(), StoreError> {
    let final_lookup = reserved_state_lookup.for_final_validation(reserved_identities);
    let validations = inputs
        .par_iter()
        .map(|input| {
            if let Some(sha256) = &input.physical_redirect_sha256 {
                validate_redirect_path(root, &input.path, sha256, reserved_identities)?;
            }
            if input.state != SemanticInputState::PathRedirect {
                validate_input(root, input, &final_lookup)?;
            }
            Ok(())
        })
        .collect::<Vec<Result<(), StoreError>>>();
    for validation in validations {
        validation?;
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

fn validate_input(
    root: &Path,
    input: &lumin_evidence::SemanticInputRecord,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<(), StoreError> {
    let expectation = SemanticInputExpectation {
        path: decode_input_path(&input.path)?,
        state: match input.state {
            SemanticInputState::Source
            | SemanticInputState::ConfigPresent
            | SemanticInputState::CapabilityTarget => SemanticInputValidationState::Regular,
            SemanticInputState::Missing => SemanticInputValidationState::Missing,
            SemanticInputState::NonRegular => SemanticInputValidationState::NonRegular,
            SemanticInputState::Unreadable => SemanticInputValidationState::Unreadable,
            SemanticInputState::PathRedirect => {
                return Err(StoreError::Integrity(format!(
                    "standalone redirect entered ordinary audit-input validation: {}",
                    input.path.display
                )));
            }
        },
        payload_sha256: input.payload_sha256.clone(),
        physical_identity: input.physical_identity.clone(),
        absence_parent: input
            .absence_parent
            .as_ref()
            .map(|parent| {
                Ok(ConfigAbsenceParent {
                    path: decode_input_path(&parent.path)?,
                    physical_identity: parent.physical_identity.clone(),
                })
            })
            .transpose()?,
    };
    lumin_inventory::validate_captured_semantic_input(root, &expectation, reserved_state_lookup)
        .map_err(|error| {
            StoreError::Integrity(format!(
                "captured audit input changed before publication ({}): {error}",
                input.path.display
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
            #[cfg(feature = "audit-execution-test-profile")]
            None,
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

    #[test]
    fn late_in_place_config_rewrite_stops_audit_before_run_publication()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        fs::create_dir(fixture.path().join("src"))?;
        fs::write(
            fixture.path().join("src/lib.ts"),
            "export const value = 1;\n",
        )?;
        let manifest = fixture.path().join("package.json");
        fs::write(&manifest, "{\"name\":\"before\"}\n")?;
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
        let captured_identity = capture
            .snapshot
            .inputs
            .iter()
            .find(|input| input.path.display == "package.json")
            .and_then(|input| input.physical_identity.clone())
            .ok_or("package.json was not captured as a semantic input")?;

        fs::write(&manifest, "{\"name\":\"after\"}\n")?;
        assert_eq!(
            lumin_inventory::observe_physical_file_identity(
                &admission.canonical_root,
                &RepoPath::from_portable("package.json")?,
            )?,
            captured_identity,
            "fixture replaced the manifest instead of rewriting it in place",
        );
        let error = match publish(
            &store,
            &mut attempt,
            &admission.canonical_root,
            &reserved_state_lookup,
            &capture.snapshot,
            #[cfg(feature = "audit-execution-test-profile")]
            None,
        ) {
            Err(error) => error,
            Ok(_) => return Err("an in-place config rewrite published stale audit evidence".into()),
        };

        assert!(matches!(
            error,
            StoreError::Integrity(detail)
                if detail.contains("captured audit input changed before publication")
                    && detail.contains("package.json")
                    && detail.contains("payload changed after capture")
        ));
        assert_no_published_run(&admission.canonical_root)?;
        Ok(())
    }

    #[test]
    fn late_missing_config_creation_stops_audit_before_run_publication()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        fs::create_dir(fixture.path().join("src"))?;
        fs::write(
            fixture.path().join("src/lib.ts"),
            "export const value = 1;\n",
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
        assert!(capture.snapshot.inputs.iter().any(|input| {
            input.path.display == "lumin.json" && input.state == SemanticInputState::Missing
        }));

        fs::write(
            admission.canonical_root.join("lumin.json"),
            "{\"schemaVersion\":\"lumin-config.v1\"}\n",
        )?;
        let error = match publish(
            &store,
            &mut attempt,
            &admission.canonical_root,
            &reserved_state_lookup,
            &capture.snapshot,
            #[cfg(feature = "audit-execution-test-profile")]
            None,
        ) {
            Err(error) => error,
            Ok(_) => return Err("a newly created config published stale audit evidence".into()),
        };

        assert!(matches!(
            error,
            StoreError::Integrity(detail)
                if detail.contains("captured audit input changed before publication")
                    && detail.contains("lumin.json")
                    && detail.contains("missing input state changed after capture")
        ));
        assert_no_published_run(&admission.canonical_root)?;
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
            #[cfg(feature = "audit-execution-test-profile")]
            None,
        )?;
        Ok(())
    }

    fn assert_no_published_run(root: &Path) -> Result<(), std::io::Error> {
        if fs::read_dir(root.join(".lumin/runs"))?
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().starts_with("run_"))
        {
            return Err(std::io::Error::other(
                "failed final validation left a publishable run directory",
            ));
        }
        Ok(())
    }
}
