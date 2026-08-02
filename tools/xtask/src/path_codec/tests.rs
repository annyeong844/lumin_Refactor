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
    assert_eq!(
        oracle::check(&artifact.value).map_err(std::io::Error::other)?,
        Vec::<String>::new()
    );
    Ok(())
}

#[test]
fn independent_oracle_rejects_changed_endian_and_dto_projection()
-> Result<(), Box<dyn std::error::Error>> {
    let mut artifact = load_artifact(&workspace_root()?).map_err(std::io::Error::other)?;
    let mut bytes = decode_hex(
        artifact.value["goldenVectors"][1]["hex"]
            .as_str()
            .ok_or("golden vector hex is missing")?,
    )
    .map_err(std::io::Error::other)?;
    bytes[15..19].copy_from_slice(&3_u32.to_le_bytes());
    artifact.value["goldenVectors"][1]["hex"] =
        Value::String(bytes.iter().map(|byte| format!("{byte:02x}")).collect());
    artifact.value["goldenVectors"][1]["base64"] = Value::String(STANDARD.encode(&bytes));
    let violations = oracle::check(&artifact.value).map_err(std::io::Error::other)?;
    assert!(violations.iter().any(|violation| {
        violation.contains("repo-portable-src-a-ts")
            && violation.contains("independent decoder rejected")
    }));

    let mut artifact = load_artifact(&workspace_root()?).map_err(std::io::Error::other)?;
    artifact.value["rootDtoGoldenVectors"][0]["display"] = Value::String("/other".to_owned());
    assert!(
        oracle::check(&artifact.value)
            .map_err(std::io::Error::other)?
            .iter()
            .any(|violation| violation.contains("root DTO projection disagrees"))
    );
    Ok(())
}

#[test]
fn independent_oracle_rejects_native_io_vector_drift() -> Result<(), Box<dyn std::error::Error>> {
    let mut artifact = load_artifact(&workspace_root()?).map_err(std::io::Error::other)?;
    artifact.value["ioGoldenVectors"][3]["nulRecordHex"] = Value::String("f09f988001".to_owned());
    assert!(
        oracle::check(&artifact.value)
            .map_err(std::io::Error::other)?
            .iter()
            .any(|violation| violation.contains("paths0-windows-scalar-emoji NUL record"))
    );
    Ok(())
}

#[test]
fn independent_windows_native_decoder_rejects_cesu8_pairs() {
    assert!(
        oracle::decode_native_nul_stream(b"\xed\xa0\xbd\xed\xb8\x80\0", oracle::Platform::Windows,)
            .is_err()
    );
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
