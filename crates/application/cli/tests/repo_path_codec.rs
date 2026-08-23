use std::ffi::OsString;
use std::fs;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

mod support;

use support::{assert_status, field, run, run_os_with_stdin};

const PORTABLE_BASE64: &str = "TFVNUlBBVEgAAQAAAAIBAAAAA3NyYwEAAAAEYS50cw==";

#[test]
fn repo_path_codec_golden_vectors_round_trip_through_public_binary()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(root.path().join("src/a.ts"), "export const value = 1;\n")?;

    let (native_name, native_record, native_base64, native_display) = native_vector();
    fs::write(root.path().join(&native_name), b"native path payload\n")?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let _run_id = field(&audit.stdout, "runId")?;
    let audit: Value = serde_json::from_str(&audit.stdout)?;
    assert_repository_root_dto(
        audit
            .get("repositoryRoot")
            .ok_or_else(|| std::io::Error::other("audit omitted repositoryRoot"))?,
    )?;

    let mut paths0 = b"src/a.ts\0".to_vec();
    paths0.extend_from_slice(&native_record);
    let arguments = vec![
        "pre-write".into(),
        "--operation-id".into(),
        "op-repo-path-codec".into(),
        "--path".into(),
        native_name,
        "--paths0-from".into(),
        "-".into(),
        "--jobs".into(),
        "1".into(),
    ];
    let pre = run_os_with_stdin(root.path(), &arguments, &paths0)?;
    assert_status(&pre, 4);
    let pre: Value = serde_json::from_str(&pre.stdout)?;

    let leased = pre
        .get("leasedWriteSet")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("pre-write omitted leasedWriteSet"))?;
    assert!(
        leased.is_empty(),
        "an unsealed rejected gate must not retain active-like leases"
    );
    let attempted = pre
        .pointer("/observationBinding/attemptedDomain")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("pre-write omitted its attempted domain"))?;
    let portable = attempted
        .iter()
        .find(|path| path.get("display").and_then(Value::as_str) == Some("src/a.ts"))
        .ok_or_else(|| std::io::Error::other("portable attempted path is missing"))?;
    assert_repo_path_dto(portable, PORTABLE_BASE64, "src/a.ts", None);

    let native_paths = pre
        .get("signals")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|signal| signal.get("paths").and_then(Value::as_array))
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(
        native_paths.len(),
        1,
        "native --path and paths0 input must lower to one canonical path"
    );
    assert_repo_path_dto(native_paths[0], native_base64, native_display, None);

    assert_malformed_paths0_fails_before_state(b"src/a.ts", "unterminated")?;
    assert_malformed_paths0_fails_before_state(b"./src/a.ts\0", "dot-prefix")?;
    Ok(())
}

fn assert_repo_path_dto(dto: &Value, canonical_base64: &str, display: &str, utf8: Option<&str>) {
    assert_eq!(
        dto.get("encoding").and_then(Value::as_str),
        Some("repo-path.v1")
    );
    assert_eq!(
        dto.get("canonicalBase64").and_then(Value::as_str),
        Some(canonical_base64)
    );
    assert_eq!(dto.get("display").and_then(Value::as_str), Some(display));
    assert_eq!(dto.get("utf8").and_then(Value::as_str), utf8);
}

fn assert_repository_root_dto(dto: &Value) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        dto.get("encoding").and_then(Value::as_str),
        Some("repository-root.v1")
    );
    let encoded = dto
        .get("canonicalBase64")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("root DTO omitted canonicalBase64"))?;
    let bytes = STANDARD.decode(encoded)?;
    assert_eq!(STANDARD.encode(&bytes), encoded);
    let projection = decode_root_projection(&bytes)?;
    assert_eq!(
        dto.get("display").and_then(Value::as_str),
        Some(projection.as_str())
    );
    assert_eq!(
        dto.get("readableAddress").and_then(Value::as_str),
        Some(projection.as_str())
    );
    Ok(())
}

fn decode_root_projection(bytes: &[u8]) -> Result<String, std::io::Error> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(8)? != b"LUMRROOT" || cursor.u16()? != 1 {
        return Err(std::io::Error::other("root DTO has wrong tag or version"));
    }
    let platform = cursor.u8()?;
    let kind = cursor.u8()?;
    let prefix = match (platform, kind) {
        (1, 1) => "/".to_owned(),
        (2, 2) => format!("{}:/", cursor.u8()? as char),
        (2, 3) => {
            let server = cursor.portable_component()?;
            let share = cursor.portable_component()?;
            format!("//{server}/{share}/")
        }
        (2, 4) => {
            let guid = cursor.take(16)?;
            format!("//?/Volume{{{}}}/", format_guid(guid))
        }
        _ => {
            return Err(std::io::Error::other(
                "root DTO has mismatched platform/address",
            ));
        }
    };
    let count = cursor.u32()? as usize;
    let mut components = Vec::with_capacity(count);
    for _ in 0..count {
        components.push(cursor.portable_component()?);
    }
    let physical_tag = cursor.u8()?;
    let physical_len = match (platform, physical_tag) {
        (1, 1) => 16,
        (2, 2) => 24,
        _ => {
            return Err(std::io::Error::other(
                "root DTO has mismatched physical identity",
            ));
        }
    };
    cursor.take(physical_len)?;
    if !cursor.finished() {
        return Err(std::io::Error::other("root DTO has trailing bytes"));
    }
    Ok(format!("{prefix}{}", components.join("/")))
}

fn format_guid(bytes: &[u8]) -> String {
    let mut output = String::new();
    for (index, byte) in bytes.iter().enumerate() {
        if [4, 6, 8, 10].contains(&index) {
            output.push('-');
        }
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn assert_malformed_paths0_fails_before_state(
    input: &[u8],
    suffix: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let arguments = vec![
        "pre-write".into(),
        "--operation-id".into(),
        format!("op-paths0-reject-{suffix}").into(),
        "--paths0-from".into(),
        "-".into(),
    ];
    let result = run_os_with_stdin(root.path(), &arguments, input)?;
    assert_status(&result, 2);
    assert!(result.stdout.is_empty());
    assert!(!root.path().join(".lumin").exists());
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], std::io::Error> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| std::io::Error::other("root DTO length overflow"))?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| std::io::Error::other("root DTO is truncated"))?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, std::io::Error> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, std::io::Error> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, std::io::Error> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn portable_component(&mut self) -> Result<String, std::io::Error> {
        if self.u8()? != 1 {
            return Err(std::io::Error::other("root test fixture is not portable"));
        }
        let length = self.u32()? as usize;
        String::from_utf8(self.take(length)?.to_vec())
            .map_err(|_| std::io::Error::other("root component is not UTF-8"))
    }

    fn finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

#[cfg(unix)]
fn native_vector() -> (OsString, Vec<u8>, &'static str, &'static str) {
    use std::os::unix::ffi::OsStringExt;

    (
        OsString::from_vec(vec![b'f', 0x80, b'o']),
        vec![b'f', 0x80, b'o', 0],
        "TFVNUlBBVEgAAQAAAAECAAAAA2aAbw==",
        "$'\\x66\\x80\\x6f'",
    )
}

#[cfg(windows)]
fn native_vector() -> (OsString, Vec<u8>, &'static str, &'static str) {
    use std::os::windows::ffi::OsStringExt;

    (
        OsString::from_wide(&[0xd800, 0x0061]),
        vec![0xed, 0xa0, 0x80, b'a', 0],
        "TFVNUlBBVEgAAQAAAAEDAAAABNgAAGE=",
        "wtf16[d800,0061]",
    )
}
