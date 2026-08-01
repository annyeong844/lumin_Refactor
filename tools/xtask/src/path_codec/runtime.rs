use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
#[cfg(windows)]
use lumin_model::RepositoryRootPhysicalIdentity;
use lumin_model::{RepoPath, RepositoryRootIdentity};
use lumin_protocol::{RepoPathDto, RepositoryRootDto};
use serde_json::Value;

use super::{decode_hex, field};

pub(super) fn check(value: &Value) -> Result<Vec<String>, String> {
    let rows = value
        .pointer("/goldenVectors")
        .and_then(Value::as_array)
        .ok_or_else(|| "goldenVectors must be an array".to_owned())?;
    let mut vectors = BTreeMap::new();
    let mut violations = Vec::new();
    for row in rows {
        let id = field(row, "id")?;
        let bytes = decode_hex(field(row, "hex")?)?;
        vectors.insert(id.to_owned(), bytes.clone());
        if id.starts_with("repo-") {
            match RepoPath::from_canonical_bytes(&bytes) {
                Ok(path) => {
                    if path.canonical_bytes() != bytes {
                        violations.push(format!("PATH CODEC RUNTIME: {id} did not round-trip"));
                    }
                    let dto = RepoPathDto::from(&path);
                    if dto.canonical_base64 != field(row, "base64")? || dto.decode().is_err() {
                        violations.push(format!("PATH CODEC DTO: {id} did not round-trip"));
                    }
                }
                Err(error) => violations.push(format!(
                    "PATH CODEC RUNTIME: {id} rejected frozen vector: {error}"
                )),
            }
        } else if id.starts_with("root-") {
            match RepositoryRootIdentity::from_canonical_bytes(&bytes) {
                Ok(root) if root.canonical_bytes() == bytes => {}
                Ok(_) => violations.push(format!("ROOT CODEC RUNTIME: {id} did not round-trip")),
                Err(error) => violations.push(format!(
                    "ROOT CODEC RUNTIME: {id} rejected frozen vector: {error}"
                )),
            }
        }
    }
    check_root_dto_vectors(value, &vectors, &mut violations)?;
    check_native_io_vectors(value, &vectors, &mut violations)?;
    check_rejections(&vectors, &mut violations);
    Ok(violations)
}

fn check_root_dto_vectors(
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
        let root_vector = field(row, "rootVector")?;
        let dto = RepositoryRootDto {
            encoding: field(row, "encoding")?.to_owned(),
            canonical_base64: field(row, "canonicalBase64")?.to_owned(),
            display: field(row, "display")?.to_owned(),
            readable_address: row
                .get("readableAddress")
                .and_then(Value::as_str)
                .map(str::to_owned),
        };
        match dto.decode() {
            Ok(root)
                if vectors
                    .get(root_vector)
                    .is_some_and(|expected| root.canonical_bytes() == expected) =>
            {
                if RepositoryRootDto::from(&root) != dto {
                    violations.push(format!("ROOT DTO RUNTIME: {id} did not re-encode exactly"));
                }
            }
            Ok(_) => violations.push(format!("ROOT DTO RUNTIME: {id} references the wrong root")),
            Err(error) => violations.push(format!("ROOT DTO RUNTIME: {id} failed: {error}")),
        }
    }
    Ok(())
}

fn check_native_io_vectors(
    value: &Value,
    vectors: &BTreeMap<String, Vec<u8>>,
    violations: &mut Vec<String>,
) -> Result<(), String> {
    let rows = value
        .pointer("/ioGoldenVectors")
        .and_then(Value::as_array)
        .ok_or_else(|| "ioGoldenVectors must be an array".to_owned())?;
    for row in rows {
        let platform = field(row, "platform")?;
        let applies = platform == "Unix-or-Windows"
            || (cfg!(unix) && platform == "Unix")
            || (cfg!(windows) && platform == "Windows");
        if !applies {
            continue;
        }
        let id = field(row, "id")?;
        let repo_vector = field(row, "repoVector")?;
        let expected_match = decode_hex(field(row, "matchBytesHex")?)?;
        let expected_nul = decode_hex(field(row, "nulRecordHex")?)?;
        let Some(canonical) = vectors.get(repo_vector) else {
            return Err(format!("{id} references missing {repo_vector}"));
        };
        let path = RepoPath::from_canonical_bytes(canonical)
            .map_err(|error| format!("cannot decode {repo_vector}: {error}"))?;
        if path.native_match_bytes().as_deref() != Ok(expected_match.as_slice()) {
            violations.push(format!("PATH NATIVE IO: {id} match bytes disagree"));
        }
        if RepoPath::encode_native_nul_stream(std::slice::from_ref(&path)).as_deref()
            != Ok(expected_nul.as_slice())
        {
            violations.push(format!("PATH NATIVE IO: {id} NUL encoding disagrees"));
        }
        match RepoPath::decode_native_nul_stream(&expected_nul) {
            Ok(decoded) if decoded == [path] => {}
            _ => violations.push(format!("PATH NATIVE IO: {id} NUL decoding disagrees")),
        }
    }
    Ok(())
}

fn check_rejections(vectors: &BTreeMap<String, Vec<u8>>, violations: &mut Vec<String>) {
    for payload in [b".".as_slice(), b"..", b"a/b", b"a\\b", b"a\0b"] {
        reject_repo_component(1, payload, "noncanonical PortableUtf8", violations);
    }
    for payload in [b"a/b".as_slice(), b"a\0b", b".", b".."] {
        reject_repo_component(2, payload, "invalid UnixBytes", violations);
    }
    reject_repo_component(2, b"src", "portable UnixBytes", violations);
    reject_repo_component(3, b"\xd8", "odd WindowsWtf16", violations);
    reject_repo_component(3, b"\x00a", "scalar WindowsWtf16", violations);

    let mut trailing = RepoPath::empty().canonical_bytes().to_vec();
    trailing.push(0);
    if RepoPath::from_canonical_bytes(&trailing).is_ok() {
        violations.push("PATH CODEC REJECTION: trailing bytes were accepted".to_owned());
    }
    let bad_dto = RepoPathDto {
        encoding: "repo-path.v1".to_owned(),
        canonical_base64: "TFVNUlBBVEgAAQAAAAA".to_owned(),
        display: String::new(),
        utf8: Some(String::new()),
    };
    if bad_dto.decode().is_ok() {
        violations.push("PATH DTO REJECTION: unpadded Base64 was accepted".to_owned());
    }
    if RepoPath::decode_native_nul_stream(b"src/a.ts").is_ok() {
        violations.push("PATH NATIVE IO REJECTION: unterminated record was accepted".to_owned());
    }
    if RepoPath::decode_native_nul_stream(b"./src/a.ts\0").is_ok() {
        violations
            .push("PATH NATIVE IO REJECTION: noncanonical dot record was accepted".to_owned());
    }
    #[cfg(windows)]
    {
        if RepoPath::decode_native_nul_stream(b"\xed\xa0\xbd\xed\xb8\x80\0").is_ok() {
            violations.push("PATH NATIVE IO REJECTION: CESU-8 scalar pair was accepted".to_owned());
        }
        if RepoPath::decode_native_nul_stream(b"src\\a.ts\0").is_ok() {
            violations.push(
                "PATH NATIVE IO REJECTION: Windows backslash separator was accepted".to_owned(),
            );
        }
        let device = std::path::Path::new(r"\\.\PhysicalDrive0");
        let physical = RepositoryRootPhysicalIdentity::Windows {
            volume_serial: 1,
            file_id: [0; 16],
        };
        if RepositoryRootIdentity::from_native_absolute(device, physical).is_ok() {
            violations.push("ROOT CODEC REJECTION: Windows device root was accepted".to_owned());
        }
    }

    check_root_rejections(vectors, violations);
    check_root_dto_rejections(vectors, violations);
}

fn reject_repo_component(tag: u8, payload: &[u8], label: &str, violations: &mut Vec<String>) {
    let mut bytes = b"LUMRPATH\x00\x01\x00\x00\x00\x01".to_vec();
    bytes.push(tag);
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(payload);
    if RepoPath::from_canonical_bytes(&bytes).is_ok() {
        violations.push(format!("PATH CODEC REJECTION: {label} was accepted"));
    }
}

fn check_root_rejections(vectors: &BTreeMap<String, Vec<u8>>, violations: &mut Vec<String>) {
    let Some(unix) = vectors.get("root-unix-repo") else {
        violations.push("ROOT CODEC REJECTION: root-unix-repo vector is missing".to_owned());
        return;
    };
    let Some(drive) = vectors.get("root-windows-drive") else {
        violations.push("ROOT CODEC REJECTION: root-windows-drive vector is missing".to_owned());
        return;
    };
    let mutations = [
        (mutated(unix, 10, 2), "platform/address mismatch"),
        (
            mutated(unix, unix.len() - 17, 2),
            "platform/identity mismatch",
        ),
        (mutated(drive, 12, b'c'), "lowercase drive"),
    ];
    for (bytes, label) in mutations {
        if RepositoryRootIdentity::from_canonical_bytes(&bytes).is_ok() {
            violations.push(format!("ROOT CODEC REJECTION: {label} was accepted"));
        }
    }
    let mut trailing = unix.clone();
    trailing.push(0);
    if RepositoryRootIdentity::from_canonical_bytes(&trailing).is_ok() {
        violations.push("ROOT CODEC REJECTION: trailing bytes were accepted".to_owned());
    }
    let mut noncanonical_component = unix.clone();
    noncanonical_component[16] = 2;
    if RepositoryRootIdentity::from_canonical_bytes(&noncanonical_component).is_ok() {
        violations.push(
            "ROOT CODEC REJECTION: portable component encoded as UnixBytes was accepted".to_owned(),
        );
    }
}

fn check_root_dto_rejections(vectors: &BTreeMap<String, Vec<u8>>, violations: &mut Vec<String>) {
    let Some(unix) = vectors.get("root-unix-repo") else {
        return;
    };
    let Some(drive) = vectors.get("root-windows-drive") else {
        return;
    };
    let valid = RepositoryRootDto {
        encoding: "repository-root.v1".to_owned(),
        canonical_base64: STANDARD.encode(unix),
        display: "/repo".to_owned(),
        readable_address: Some("/repo".to_owned()),
    };
    let mut cases = Vec::new();
    let mut wrong_encoding = valid.clone();
    wrong_encoding.encoding = "root.v0".to_owned();
    cases.push((wrong_encoding, "wrong encoding"));
    let mut unpadded = valid.clone();
    unpadded.canonical_base64.pop();
    cases.push((unpadded, "unpadded Base64"));
    let mut repo_path_payload = valid.clone();
    repo_path_payload.canonical_base64 = STANDARD.encode(RepoPath::empty().canonical_bytes());
    cases.push((repo_path_payload, "repo-path payload"));
    let mut missing_physical = valid.clone();
    missing_physical.canonical_base64 = STANDARD.encode(&unix[..unix.len() - 17]);
    cases.push((missing_physical, "missing physical identity"));
    let mut readable_mismatch = valid.clone();
    readable_mismatch.readable_address = Some("/other".to_owned());
    cases.push((readable_mismatch, "readableAddress mismatch"));
    let mut display_only = valid.clone();
    display_only.canonical_base64 = "not-base64".to_owned();
    cases.push((display_only, "display identity recovery"));
    cases.push((
        RepositoryRootDto {
            encoding: "repository-root.v1".to_owned(),
            canonical_base64: STANDARD.encode(drive),
            display: "C:/repo".to_owned(),
            readable_address: Some("c:/repo".to_owned()),
        },
        "lowercase readable drive",
    ));

    if let Some(volume) = vectors.get("root-windows-volume-guid") {
        cases.push((
            RepositoryRootDto {
                encoding: "repository-root.v1".to_owned(),
                canonical_base64: STANDARD.encode(volume),
                display: "//?/Volume{00112233-4455-6677-8899-aabbccddeeff}/repo".to_owned(),
                readable_address: Some(
                    "//?/Volume{00112233-4455-6677-8899-AABBCCDDEEFF}/repo".to_owned(),
                ),
            },
            "noncanonical readable volume GUID",
        ));
    }

    let mut nonportable_drive = drive.clone();
    nonportable_drive[17] = 3;
    nonportable_drive[22..26].copy_from_slice(b"\xd8\x00\x00a");
    cases.push((
        RepositoryRootDto {
            encoding: "repository-root.v1".to_owned(),
            canonical_base64: STANDARD.encode(&nonportable_drive),
            display: "C:/wtf16[d800,0061]".to_owned(),
            readable_address: Some("C:/wtf16[d800,0061]".to_owned()),
        },
        "nonportable readable component",
    ));

    for (dto, label) in cases {
        if dto.decode().is_ok() {
            violations.push(format!("ROOT DTO REJECTION: {label} was accepted"));
        }
    }
    let parallel = serde_json::json!({
        "encoding": "repository-root.v1",
        "canonicalBase64": STANDARD.encode(unix),
        "display": "/repo",
        "platform": "unix"
    });
    if serde_json::from_value::<RepositoryRootDto>(parallel).is_ok() {
        violations.push("ROOT DTO REJECTION: parallel identity field was accepted".to_owned());
    }
}

fn mutated(bytes: &[u8], offset: usize, value: u8) -> Vec<u8> {
    let mut mutated = bytes.to_vec();
    mutated[offset] = value;
    mutated
}
