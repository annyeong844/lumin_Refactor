use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

use super::{Platform, decode_native_nul_stream, decode_repo, decode_root, root_projection};
use crate::path_codec::{decode_hex, field};

pub(in crate::path_codec) fn check(value: &Value) -> Result<Vec<String>, String> {
    let rows = value
        .pointer("/goldenVectors")
        .and_then(Value::as_array)
        .ok_or_else(|| "goldenVectors must be an array".to_owned())?;
    let mut decoded = BTreeMap::new();
    let mut violations = Vec::new();
    for row in rows {
        let id = field(row, "id")?;
        let bytes = decode_hex(field(row, "hex")?)?;
        let expected_base64 = field(row, "base64")?;
        if STANDARD.encode(&bytes) != expected_base64 {
            violations.push(format!("PATH CODEC ORACLE: {id} Base64 disagrees"));
        }
        let reencoded = if id.starts_with("repo-") {
            decode_repo(&bytes).map(|path| path.encode())
        } else if id.starts_with("root-") {
            decode_root(&bytes).map(|root| root.encode())
        } else {
            Err("unknown golden-vector family".to_owned())
        };
        match reencoded {
            Ok(reencoded) if reencoded == bytes => {
                decoded.insert(id.to_owned(), bytes);
            }
            Ok(_) => violations.push(format!(
                "PATH CODEC ORACLE: {id} independent re-encoding disagrees"
            )),
            Err(error) => violations.push(format!(
                "PATH CODEC ORACLE: {id} independent decoder rejected vector: {error}"
            )),
        }
    }
    check_root_dtos(value, &decoded, &mut violations)?;
    check_native_io(value, &decoded, &mut violations)?;
    check_rejections(&mut violations);
    Ok(violations)
}

fn check_root_dtos(
    value: &Value,
    vectors: &BTreeMap<String, Vec<u8>>,
    violations: &mut Vec<String>,
) -> Result<(), String> {
    let rows = value
        .pointer("/rootDtoGoldenVectors")
        .and_then(Value::as_array)
        .ok_or_else(|| "rootDtoGoldenVectors must be an array".to_owned())?;
    for row in rows {
        let id = field(row, "id")?;
        let root_id = field(row, "rootVector")?;
        let Some(bytes) = vectors.get(root_id) else {
            violations.push(format!(
                "PATH CODEC ORACLE: {id} references unavailable {root_id}"
            ));
            continue;
        };
        let projection = root_projection(bytes)?;
        let expected_readable = row.get("readableAddress").and_then(Value::as_str);
        if field(row, "encoding")? != "repository-root.v1"
            || field(row, "canonicalBase64")? != projection.canonical_base64
            || field(row, "display")? != projection.display
            || expected_readable != projection.readable_address.as_deref()
        {
            violations.push(format!(
                "PATH CODEC ORACLE: {id} root DTO projection disagrees"
            ));
        }
    }
    Ok(())
}

fn check_native_io(
    value: &Value,
    vectors: &BTreeMap<String, Vec<u8>>,
    violations: &mut Vec<String>,
) -> Result<(), String> {
    let rows = value
        .pointer("/ioGoldenVectors")
        .and_then(Value::as_array)
        .ok_or_else(|| "ioGoldenVectors must be an array".to_owned())?;
    for row in rows {
        let id = field(row, "id")?;
        let path_id = field(row, "repoVector")?;
        let Some(bytes) = vectors.get(path_id) else {
            violations.push(format!(
                "PATH CODEC ORACLE: {id} references unavailable {path_id}"
            ));
            continue;
        };
        let path = decode_repo(bytes)?;
        let expected_match = decode_hex(field(row, "matchBytesHex")?)?;
        let expected_nul = decode_hex(field(row, "nulRecordHex")?)?;
        let platforms: &[Platform] = match field(row, "platform")? {
            "Unix" => &[Platform::Unix],
            "Windows" => &[Platform::Windows],
            "Unix-or-Windows" => &[Platform::Unix, Platform::Windows],
            platform => return Err(format!("{id} has unknown platform {platform}")),
        };
        for platform in platforms {
            match path.native_bytes(*platform) {
                Ok(actual) if actual == expected_match => {
                    let mut nul = actual;
                    nul.push(0);
                    if nul != expected_nul {
                        violations.push(format!(
                            "PATH CODEC ORACLE: {id} NUL record disagrees for {platform:?}"
                        ));
                    }
                    match decode_native_nul_stream(&expected_nul, *platform) {
                        Ok(decoded) if decoded == [path.clone()] => {}
                        Ok(_) => violations.push(format!(
                            "PATH CODEC ORACLE: {id} NUL decode disagrees for {platform:?}"
                        )),
                        Err(error) => violations.push(format!(
                            "PATH CODEC ORACLE: {id} cannot decode {platform:?}: {error}"
                        )),
                    }
                }
                Ok(_) => violations.push(format!(
                    "PATH CODEC ORACLE: {id} match bytes disagree for {platform:?}"
                )),
                Err(error) => violations.push(format!(
                    "PATH CODEC ORACLE: {id} cannot encode {platform:?}: {error}"
                )),
            }
        }
    }
    Ok(())
}

fn check_rejections(violations: &mut Vec<String>) {
    for (bytes, label) in repo_rejections() {
        if decode_repo(&bytes).is_ok() {
            violations.push(format!("PATH CODEC ORACLE REJECTION: accepted {label}"));
        }
    }
    let Ok(valid_root) = decode_hex(
        "4c554d52524f4f54000101010000000101000000047265706f0100000000000000010000000000000002",
    ) else {
        violations.push("PATH CODEC ORACLE: root rejection fixture is invalid".to_owned());
        return;
    };
    let mut wrong_platform = valid_root.clone();
    wrong_platform[10] = 2;
    let mut trailing = valid_root;
    trailing.push(0);
    for (bytes, label) in [
        (wrong_platform, "root platform/address mismatch"),
        (trailing, "root trailing byte"),
    ] {
        if decode_root(&bytes).is_ok() {
            violations.push(format!("PATH CODEC ORACLE REJECTION: accepted {label}"));
        }
    }
}

fn repo_rejections() -> Vec<(Vec<u8>, &'static str)> {
    let component = |tag: u8, payload: &[u8]| {
        let mut bytes = b"LUMRPATH\0\x01\0\0\0\x01".to_vec();
        bytes.push(tag);
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    };
    let mut trailing = b"LUMRPATH\0\x01\0\0\0\0".to_vec();
    trailing.push(0);
    vec![
        (component(1, b"."), "PortableUtf8 dot"),
        (component(1, b"a/b"), "PortableUtf8 slash"),
        (component(1, b"a\\b"), "PortableUtf8 backslash"),
        (component(2, b"a/b"), "UnixBytes slash"),
        (component(2, b"src"), "portable UnixBytes"),
        (component(3, b"\xd8"), "odd WindowsWtf16"),
        (component(3, b"\0a"), "scalar WindowsWtf16"),
        (trailing, "trailing bytes"),
    ]
}
