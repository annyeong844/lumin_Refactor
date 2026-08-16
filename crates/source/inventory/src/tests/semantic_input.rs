#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;

use super::super::*;

#[cfg(unix)]
#[test]
fn dangling_config_symlink_remains_non_regular_evidence() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    std::os::unix::fs::symlink("missing-target.json", root.path().join("package.json"))?;

    let capture = capture_config(
        root.path(),
        &RepoPath::from_portable("package.json")?,
        ConfigSyntax::StrictJson,
    )?;

    assert!(matches!(
        capture.observation,
        ConfigObservation::NonRegular {
            physical_identity: Some(_),
            ..
        }
    ));
    assert!(matches!(
        capture.limitation,
        Some(Limitation::PackageMetadataUnobservable { .. })
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn dangling_config_target_appearance_invalidates_final_validation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let path = RepoPath::from_portable("package.json")?;
    std::os::unix::fs::symlink("missing-target.json", root.path().join("package.json"))?;
    let lookup = ReservedStateIdentityLookup::empty();
    let capture = capture_config_with_reserved_state_lookup(
        root.path(),
        &path,
        ConfigSyntax::StrictJson,
        &lookup,
    )?;
    let ConfigObservation::NonRegular {
        physical_identity: Some(physical_identity),
        ..
    } = capture.observation
    else {
        return Err("dangling config was not captured as non-regular".into());
    };
    let expected = SemanticInputExpectation {
        path,
        state: SemanticInputValidationState::NonRegular,
        payload_sha256: None,
        physical_identity: Some(physical_identity),
        absence_parent: None,
    };

    fs::write(root.path().join("missing-target.json"), "{}\n")?;
    let final_lookup = lookup.for_final_validation(&BTreeSet::new());
    let error = match validate_captured_semantic_input(root.path(), &expected, &final_lookup) {
        Err(error) => error,
        Ok(()) => return Err("a newly available redirect target passed final validation".into()),
    };

    assert!(matches!(
        error,
        InventoryError::PhysicalIdentity(detail)
            if detail.contains("redirect target availability or identity changed")
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn unrelated_child_directory_does_not_change_missing_input_topology()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("context"))?;
    let path = RepoPath::from_portable("context")?;
    let identity = physical_file_identity(&root.path().join("context"))?;
    let lookup = ReservedStateIdentityLookup::empty();
    validate_captured_semantic_input_topology(root.path(), &path, &identity, &lookup)?;

    fs::create_dir(root.path().join("context/unrelated"))?;
    let final_lookup = lookup.for_final_validation(&BTreeSet::new());
    validate_captured_semantic_input_topology(root.path(), &path, &identity, &final_lookup)?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn missing_gitignore_rejects_a_reserved_parent_identity() -> Result<(), Box<dyn std::error::Error>>
{
    if super::linux_mount::enter_private_namespace(
        "tests::semantic_input::missing_gitignore_rejects_a_reserved_parent_identity",
    )? {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let state_cache = root.path().join(".lumin/cache");
    let alias = root.path().join("state-alias");
    fs::create_dir_all(&state_cache)?;
    fs::create_dir(&alias)?;
    let reserved = BTreeSet::from([physical_file_identity(&state_cache)?]);
    let lookup = ReservedStateIdentityLookup::from_identities(reserved);
    let mut mount = super::linux_mount::DirectoryBindMount::install(&state_cache, &alias)?;

    let error = match ApplicableIgnore::build_with_reserved_state_lookup(root.path(), &lookup) {
        Err(error) => error,
        Ok(_) => return Err("a missing .gitignore beneath reserved state was accepted".into()),
    };

    assert!(matches!(
        error,
        InventoryError::ReservedSemanticInputPath(path) if path == "state-alias"
    ));
    mount.remove()?;
    Ok(())
}
