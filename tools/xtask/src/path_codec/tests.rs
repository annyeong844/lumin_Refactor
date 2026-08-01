use super::*;

fn workspace_root() -> Result<PathBuf, std::io::Error> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
}

#[test]
fn frozen_artifact_validates_and_runtime_vectors_hold() -> Result<(), Box<dyn std::error::Error>> {
    let artifact = load_artifact(&workspace_root()?).map_err(std::io::Error::other)?;
    assert_eq!(validate_artifact(&artifact.value), Vec::<String>::new());
    assert_eq!(
        runtime::check(&artifact.value).map_err(std::io::Error::other)?,
        Vec::<String>::new()
    );
    Ok(())
}

#[test]
fn changed_component_tag_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let mut artifact = load_artifact(&workspace_root()?).map_err(std::io::Error::other)?;
    artifact.value["repoPath"]["component"]["kinds"][0]["tagHex"] = Value::String("09".to_owned());
    assert!(
        validate_artifact(&artifact.value)
            .iter()
            .any(|violation| violation.contains("component.kinds tags changed"))
    );
    Ok(())
}

#[test]
fn mismatched_golden_encodings_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let mut artifact = load_artifact(&workspace_root()?).map_err(std::io::Error::other)?;
    artifact.value["goldenVectors"][0]["base64"] = Value::String("AA==".to_owned());
    assert!(
        validate_artifact(&artifact.value)
            .iter()
            .any(|violation| violation.contains("hex and base64 disagree"))
    );
    Ok(())
}

#[test]
fn generation_is_idempotent_and_drift_is_visible() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let spec_target = temp.path().join(SPEC);
    std::fs::create_dir_all(
        spec_target
            .parent()
            .ok_or_else(|| std::io::Error::other("temporary spec path has no parent"))?,
    )?;
    std::fs::copy(workspace_root()?.join(SPEC), &spec_target)?;
    let contract_target = temp.path().join(ANALYSIS_CONTRACT_OWNER);
    std::fs::create_dir_all(
        contract_target
            .parent()
            .ok_or_else(|| std::io::Error::other("temporary contract path has no parent"))?,
    )?;
    std::fs::copy(
        workspace_root()?.join(ANALYSIS_CONTRACT_OWNER),
        &contract_target,
    )?;
    let output_target = temp.path().join(OUTPUT);
    std::fs::create_dir_all(
        output_target
            .parent()
            .ok_or_else(|| std::io::Error::other("temporary output path has no parent"))?,
    )?;

    write_generated_codec(temp.path()).map_err(std::io::Error::other)?;
    let first = std::fs::read(&output_target)?;
    write_generated_codec(temp.path()).map_err(std::io::Error::other)?;
    assert_eq!(first, std::fs::read(&output_target)?);

    std::fs::write(&output_target, b"drift\n")?;
    let result = check_path_codec(temp.path());
    assert!(result.tool_errors.is_empty());
    assert!(
        result
            .violations
            .iter()
            .any(|violation| violation.contains("PATH CODEC GENERATED DRIFT"))
    );
    Ok(())
}
