use std::fs;
use std::io::Read;
use std::path::Path;

use lumin_model::{ConfigAbsenceParent, PhysicalFileIdentity, RepoPath, digest_hex};

use crate::capture;
use crate::physical_path::{
    is_physical_path_redirect, observe_config_input_identity, physical_redirect_entry_identity,
};
use crate::reserved_state::{
    ReservedStateIdentityLookup, validate_captured_semantic_input_topology,
    validate_semantic_input_identity, validate_semantic_input_path,
};
use crate::{InventoryError, native_relative, validate_root};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticInputValidationState {
    Regular,
    Missing,
    NonRegular,
    Unreadable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticInputExpectation {
    pub path: RepoPath,
    pub state: SemanticInputValidationState,
    pub payload_sha256: Option<String>,
    pub physical_identity: Option<PhysicalFileIdentity>,
    pub absence_parent: Option<ConfigAbsenceParent>,
}

pub fn validate_captured_semantic_input(
    root: &Path,
    expected: &SemanticInputExpectation,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<(), InventoryError> {
    validate_root(root)?;
    validate_semantic_input_path(root, &expected.path)?;
    match expected.state {
        SemanticInputValidationState::Regular => {
            validate_regular(root, expected, reserved_state_lookup)
        }
        SemanticInputValidationState::Missing => {
            validate_missing(root, expected, reserved_state_lookup)
        }
        SemanticInputValidationState::NonRegular => {
            validate_non_regular(root, expected, reserved_state_lookup)
        }
        SemanticInputValidationState::Unreadable => {
            validate_unreadable(root, expected, reserved_state_lookup)
        }
    }
}

pub(crate) fn observe_non_regular_semantic_input_identity(
    root: &Path,
    path: &RepoPath,
    expected_identity: Option<&PhysicalFileIdentity>,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<PhysicalFileIdentity, InventoryError> {
    let native = root.join(native_relative(path)?);
    let metadata = fs::symlink_metadata(&native).map_err(|error| {
        input_changed(path, format!("cannot inspect non-regular input: {error}"))
    })?;
    if metadata.is_file() && !is_physical_path_redirect(&native, &metadata.file_type()) {
        return Err(input_changed(
            path,
            "input became a regular file".to_owned(),
        ));
    }
    if is_physical_path_redirect(&native, &metadata.file_type()) {
        return validate_redirect_identities(
            root,
            path,
            &native,
            expected_identity,
            reserved_state_lookup,
        );
    }
    let observation = capture::physical_file_observation(&native)?;
    validate_semantic_input_identity(root, path, &observation, reserved_state_lookup)?;
    Ok(observation.identity)
}

fn validate_regular(
    root: &Path,
    expected: &SemanticInputExpectation,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<(), InventoryError> {
    let expected_identity = expected.physical_identity.as_ref().ok_or_else(|| {
        input_changed(
            &expected.path,
            "regular input omitted its identity".to_owned(),
        )
    })?;
    let expected_payload = expected.payload_sha256.as_deref().ok_or_else(|| {
        input_changed(
            &expected.path,
            "regular input omitted its payload digest".to_owned(),
        )
    })?;
    if expected.absence_parent.is_some() {
        return Err(input_changed(
            &expected.path,
            "regular input carried an absence parent".to_owned(),
        ));
    }

    let native = root.join(native_relative(&expected.path)?);
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?;
    let mut file =
        capture::OpenedSource::open(&canonical_root, &native, &expected.path.display_escaped())?;
    validate_observation(
        root,
        &expected.path,
        expected_identity,
        file.observation(),
        reserved_state_lookup,
    )?;

    let bytes = file.read_payload(&expected.path.display_escaped())?;
    let current = file.validate_path(&canonical_root, &native, &expected.path.display_escaped())?;
    validate_observation(
        root,
        &expected.path,
        expected_identity,
        &current,
        reserved_state_lookup,
    )?;
    if digest_hex(&bytes) != expected_payload {
        return Err(input_changed(
            &expected.path,
            "payload changed after capture".to_owned(),
        ));
    }
    Ok(())
}

fn validate_missing(
    root: &Path,
    expected: &SemanticInputExpectation,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<(), InventoryError> {
    if expected.payload_sha256.is_some() || expected.physical_identity.is_some() {
        return Err(input_changed(
            &expected.path,
            "missing input carried present-file evidence".to_owned(),
        ));
    }
    let current = observe_config_input_identity(root, &expected.path)?;
    if current.physical_identity.is_some() || current.absence_parent.is_none() {
        return Err(input_changed(
            &expected.path,
            "missing input state changed after capture".to_owned(),
        ));
    }
    if let Some(expected_parent) = expected.absence_parent.as_ref() {
        if current.absence_parent.as_ref() != Some(expected_parent) {
            return Err(input_changed(
                &expected.path,
                "missing input absence parent changed after capture".to_owned(),
            ));
        }
        validate_captured_semantic_input_topology(
            root,
            &expected_parent.path,
            &expected_parent.physical_identity,
            reserved_state_lookup,
        )?;
    }

    let final_identity = observe_config_input_identity(root, &expected.path)?;
    if final_identity != current {
        return Err(input_changed(
            &expected.path,
            "missing input changed during final validation".to_owned(),
        ));
    }
    if let Some(expected_parent) = expected.absence_parent.as_ref() {
        validate_captured_semantic_input_topology(
            root,
            &expected_parent.path,
            &expected_parent.physical_identity,
            reserved_state_lookup,
        )?;
    }
    Ok(())
}

fn validate_non_regular(
    root: &Path,
    expected: &SemanticInputExpectation,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<(), InventoryError> {
    if expected.payload_sha256.is_some() || expected.absence_parent.is_some() {
        return Err(input_changed(
            &expected.path,
            "non-regular input carried incompatible evidence".to_owned(),
        ));
    }
    let expected_identity = expected.physical_identity.as_ref().ok_or_else(|| {
        input_changed(
            &expected.path,
            "non-regular input omitted its identity".to_owned(),
        )
    })?;
    let identity = observe_non_regular_semantic_input_identity(
        root,
        &expected.path,
        Some(expected_identity),
        reserved_state_lookup,
    )?;
    if &identity != expected_identity {
        return Err(input_changed(
            &expected.path,
            "non-regular input changed physical identity after capture".to_owned(),
        ));
    }
    Ok(())
}

fn validate_unreadable(
    root: &Path,
    expected: &SemanticInputExpectation,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<(), InventoryError> {
    if expected.absence_parent.is_some() {
        return Err(input_changed(
            &expected.path,
            "unreadable input carried an absence parent".to_owned(),
        ));
    }
    let expected_detail = expected.payload_sha256.as_deref().ok_or_else(|| {
        input_changed(
            &expected.path,
            "unreadable input omitted its detail digest".to_owned(),
        )
    })?;
    let native = root.join(native_relative(&expected.path)?);
    let current_identity = observe_config_input_identity(root, &expected.path)?;
    if current_identity.physical_identity.as_ref() != expected.physical_identity.as_ref()
        || current_identity.absence_parent.is_some()
    {
        return Err(input_changed(
            &expected.path,
            "unreadable input identity changed after capture".to_owned(),
        ));
    }
    let metadata = match fs::symlink_metadata(&native) {
        Ok(metadata) => metadata,
        Err(error)
            if !matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return compare_unreadable_detail(&expected.path, expected_detail, &error.to_string());
        }
        Err(_) => {
            return Err(input_changed(
                &expected.path,
                "unreadable input became missing".to_owned(),
            ));
        }
    };
    if is_physical_path_redirect(&native, &metadata.file_type()) || !metadata.is_file() {
        return Err(input_changed(
            &expected.path,
            "unreadable input became non-regular".to_owned(),
        ));
    }

    let expected_identity = expected.physical_identity.as_ref().ok_or_else(|| {
        input_changed(
            &expected.path,
            "unreadable regular input omitted its identity".to_owned(),
        )
    })?;
    let mut file = match capture::open_source_file(&native) {
        Ok(file) => file,
        Err(error) => {
            let current = capture::physical_file_observation(&native)?;
            validate_observation(
                root,
                &expected.path,
                expected_identity,
                &current,
                reserved_state_lookup,
            )?;
            return compare_unreadable_detail(&expected.path, expected_detail, &error.to_string());
        }
    };
    let opened = capture::physical_file_observation_from_file(&file)?;
    validate_observation(
        root,
        &expected.path,
        expected_identity,
        &opened,
        reserved_state_lookup,
    )?;
    let mut bytes = Vec::new();
    match file.read_to_end(&mut bytes) {
        Ok(_) => Err(input_changed(
            &expected.path,
            "unreadable input became readable".to_owned(),
        )),
        Err(error) => {
            let current = capture::physical_file_observation(&native)?;
            validate_observation(
                root,
                &expected.path,
                expected_identity,
                &current,
                reserved_state_lookup,
            )?;
            compare_unreadable_detail(&expected.path, expected_detail, &error.to_string())
        }
    }
}

fn validate_redirect_identities(
    root: &Path,
    path: &RepoPath,
    native: &Path,
    expected_identity: Option<&PhysicalFileIdentity>,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<PhysicalFileIdentity, InventoryError> {
    let entry_identity = physical_redirect_entry_identity(native)?;
    reject_reserved_identity(path, &entry_identity, reserved_state_lookup)?;
    let current = observe_config_input_identity(root, path)?;
    if current.absence_parent.is_some() {
        return Err(input_changed(
            path,
            "redirect entry disappeared during validation".to_owned(),
        ));
    }
    let observed_target = capture::physical_file_observation(native).ok();
    if observed_target.as_ref().map(|target| &target.identity) != current.physical_identity.as_ref()
    {
        return Err(input_changed(
            path,
            "redirect target changed during validation".to_owned(),
        ));
    }
    if let Some(target_identity) = current.physical_identity.as_ref() {
        reject_reserved_identity(path, target_identity, reserved_state_lookup)?;
    }

    match (expected_identity, current.physical_identity) {
        (None, None) => Ok(entry_identity),
        (Some(expected), None) if expected == &entry_identity => Ok(entry_identity),
        (Some(expected), Some(target)) if expected == &target => Ok(target),
        _ => Err(input_changed(
            path,
            "redirect target availability or identity changed after capture".to_owned(),
        )),
    }
}

fn reject_reserved_identity(
    path: &RepoPath,
    identity: &PhysicalFileIdentity,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<(), InventoryError> {
    if reserved_state_lookup.contains_identity(identity)? {
        return Err(InventoryError::ReservedSemanticInputPath(
            path.display_escaped(),
        ));
    }
    Ok(())
}

fn validate_observation(
    root: &Path,
    path: &RepoPath,
    expected_identity: &PhysicalFileIdentity,
    observation: &capture::PhysicalFileObservation,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<(), InventoryError> {
    if &observation.identity != expected_identity {
        return Err(input_changed(
            path,
            "physical identity changed after capture".to_owned(),
        ));
    }
    validate_semantic_input_identity(root, path, observation, reserved_state_lookup)
}

fn compare_unreadable_detail(
    path: &RepoPath,
    expected_sha256: &str,
    detail: &str,
) -> Result<(), InventoryError> {
    if digest_hex(detail.as_bytes()) != expected_sha256 {
        return Err(input_changed(
            path,
            "unreadable input detail changed after capture".to_owned(),
        ));
    }
    Ok(())
}

fn input_changed(path: &RepoPath, detail: String) -> InventoryError {
    InventoryError::PhysicalIdentity(format!(
        "semantic input changed after capture ({}): {detail}",
        path.display_escaped()
    ))
}
