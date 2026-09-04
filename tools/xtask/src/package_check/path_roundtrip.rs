use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

use super::{expect_status, expect_success, parse_json, run_binary, run_binary_os_with_stdin};

mod root_oracle;

const BULK_FINDINGS: usize = 100;

pub(super) fn validate(binary: &Path, scratch: &Path) -> Result<(), String> {
    fs::create_dir(scratch)
        .map_err(|error| format!("cannot create path-roundtrip scratch directory: {error}"))?;
    validate_native_path_cursor(binary, scratch)?;
    validate_malformed_nul_inputs(binary, scratch)
}

fn validate_native_path_cursor(binary: &Path, scratch: &Path) -> Result<(), String> {
    let fixture = NativeFixture::create(scratch)?;
    let audit = expect_success(
        run_binary(
            binary,
            &fixture.root,
            &["audit", "--jobs", "1", "--format", "json"],
        ),
        "packaged native-path audit",
    )?;
    let audit = parse_json("packaged native-path audit", &audit.stdout)?;
    expect_u64(
        &audit,
        "/findingCount",
        (BULK_FINDINGS + fixture.sources.len()) as u64,
        "packaged native-path audit",
    )?;
    assert_native_root_dto(
        audit
            .get("repositoryRoot")
            .ok_or_else(|| "packaged native-path audit omitted repositoryRoot".to_owned())?,
        &fixture.root,
    )?;
    let run_id = required_string(&audit, "/runId", "packaged native-path audit")?;

    let first = expect_success(
        run_binary(
            binary,
            &fixture.root,
            &[
                "findings",
                "--run",
                &run_id,
                "--area",
                "dead-code",
                "--format",
                "json",
            ],
        ),
        "packaged first native-path findings page",
    )?;
    let first = parse_json("packaged first native-path findings page", &first.stdout)?;
    expect_u64(
        &first,
        "/returned",
        BULK_FINDINGS as u64,
        "packaged first native-path findings page",
    )?;
    expect_bool(
        &first,
        "/truncated",
        true,
        "packaged first native-path findings page",
    )?;
    let bulk_path = repo_path_base64(&[portable_component(b"bulk.ts")]);
    let first_items = items(&first, "packaged first native-path findings page")?;
    if first_items.len() != BULK_FINDINGS
        || first_items.iter().any(|item| {
            item.pointer("/path/canonicalBase64")
                .and_then(Value::as_str)
                != Some(bulk_path.as_str())
        })
    {
        return Err(
            "packaged first findings page did not end exactly at the portable-path boundary"
                .to_owned(),
        );
    }
    let cursor = required_string(
        &first,
        "/nextCursor",
        "packaged first native-path findings page",
    )?;

    let second = expect_success(
        run_binary(
            binary,
            &fixture.root,
            &[
                "findings",
                "--run",
                &run_id,
                "--area",
                "dead-code",
                "--cursor",
                &cursor,
                "--format",
                "json",
            ],
        ),
        "packaged second native-path findings page",
    )?;
    let second = parse_json("packaged second native-path findings page", &second.stdout)?;
    expect_u64(
        &second,
        "/returned",
        fixture.sources.len() as u64,
        "packaged second native-path findings page",
    )?;
    expect_bool(
        &second,
        "/truncated",
        false,
        "packaged second native-path findings page",
    )?;
    if second
        .get("nextCursor")
        .is_some_and(|value| !value.is_null())
    {
        return Err("packaged final native-path findings page returned another cursor".to_owned());
    }
    let second_items = items(&second, "packaged second native-path findings page")?;
    if second_items.len() != fixture.sources.len() {
        return Err(format!(
            "packaged second native-path findings page returned {} items; expected {}",
            second_items.len(),
            fixture.sources.len()
        ));
    }
    let mut ordered_sources = fixture.sources.iter().collect::<Vec<_>>();
    ordered_sources.sort_by(|left, right| left.canonical.cmp(&right.canonical));
    let mut source_ids = BTreeMap::new();
    let mut distinct_source_ids = BTreeSet::new();
    let mut finding_ids = BTreeSet::new();
    for (item, source) in second_items.iter().zip(&ordered_sources) {
        assert_native_path_dto(
            item.get("path")
                .ok_or_else(|| "packaged native finding omitted path".to_owned())?,
            source,
        )?;
        if item.get("exportedName").and_then(Value::as_str) != Some(source.exported_name.as_str()) {
            return Err("packaged native findings changed their canonical cursor order".to_owned());
        }
        let source_id = required_string(item, "/sourceId", "packaged native finding")?;
        let finding_id = required_string(item, "/findingId", "packaged native finding")?;
        if !distinct_source_ids.insert(source_id.clone())
            || source_ids
                .insert(source.canonical_base64.clone(), source_id)
                .is_some()
            || !finding_ids.insert(finding_id)
        {
            return Err(
                "packaged native names collapsed into one source or finding identity".to_owned(),
            );
        }
    }

    for source in &fixture.sources {
        let file_arguments = vec![
            OsString::from("files"),
            OsString::from("--run"),
            OsString::from(&run_id),
            OsString::from("--"),
            source.relative.clone(),
        ];
        let file = expect_success(
            run_binary_os_with_stdin(binary, &fixture.root, &file_arguments, &[]),
            "packaged native argv file query",
        )?;
        let file = parse_json("packaged native argv file query", &file.stdout)?;
        assert_native_path_dto(
            file.pointer("/sourceContext/path").ok_or_else(|| {
                "packaged native file query omitted sourceContext.path".to_owned()
            })?,
            source,
        )?;
        let file_items = items(&file, "packaged native argv file query")?;
        if file_items.len() != 1 {
            return Err(format!(
                "packaged native argv file query returned {} findings; expected 1",
                file_items.len()
            ));
        }
        assert_native_path_dto(
            file_items[0]
                .get("path")
                .ok_or_else(|| "packaged native file finding omitted path".to_owned())?,
            source,
        )?;
        if required_string(
            &file,
            "/sourceContext/sourceId",
            "packaged native argv file query",
        )? != *source_ids
            .get(&source.canonical_base64)
            .ok_or_else(|| "packaged native finding source identity disappeared".to_owned())?
        {
            return Err(
                "packaged native argv query changed the logical source identity".to_owned(),
            );
        }
        if source.component.tag != 1 {
            assert_display_is_not_query_identity(binary, &fixture.root, &run_id, source)?;
        }
    }

    let nul_arguments = os_arguments(&[
        "pre-write",
        "--operation-id",
        "package-native-path-open-0001",
        "--paths0-from",
        "-",
        "--jobs",
        "1",
        "--format",
        "json",
    ]);
    let mut nul_input = Vec::new();
    for source in &fixture.sources {
        nul_input.extend_from_slice(&source.nul_record);
    }
    let pre = expect_success(
        run_binary_os_with_stdin(binary, &fixture.root, &nul_arguments, &nul_input),
        "packaged native NUL pre-write",
    )?;
    let pre = parse_json("packaged native NUL pre-write", &pre.stdout)?;
    let decision = required_string(&pre, "/decision", "packaged native NUL pre-write")?;
    if decision != "allow-with-warnings" {
        return Err(format!(
            "packaged native NUL pre-write returned {decision}; expected allow-with-warnings"
        ));
    }
    let leased = required_array(&pre, "/leasedWriteSet", "packaged native NUL pre-write")?;
    if leased.len() != fixture.sources.len() {
        return Err(format!(
            "packaged native NUL pre-write returned {} leases; expected {}",
            leased.len(),
            fixture.sources.len()
        ));
    }
    for (lease, source) in leased.iter().zip(ordered_sources) {
        assert_native_path_dto(
            lease
                .get("path")
                .ok_or_else(|| "packaged native NUL lease omitted path".to_owned())?,
            source,
        )?;
    }
    Ok(())
}

fn assert_display_is_not_query_identity(
    binary: &Path,
    root: &Path,
    run_id: &str,
    source: &NativeSourceCase,
) -> Result<(), String> {
    let display = format!("{}/mod.ts", source.component.display);
    let query = expect_success(
        run_binary(
            binary,
            root,
            &["files", "--run", run_id, "--format", "json", "--", &display],
        ),
        "packaged escaped-display file query",
    )?;
    let query = parse_json("packaged escaped-display file query", &query.stdout)?;
    expect_u64(&query, "/total", 0, "packaged escaped-display file query")?;
    if !items(&query, "packaged escaped-display file query")?.is_empty()
        || query.get("sourceContext").is_some()
        || query.get("sourceObservation").is_some()
    {
        return Err("packaged CLI treated escaped display text as a path identity".to_owned());
    }
    Ok(())
}

fn validate_malformed_nul_inputs(binary: &Path, scratch: &Path) -> Result<(), String> {
    for (name, input) in [
        ("unterminated", b"src/a.ts".as_slice()),
        ("dot-prefix", b"./src/a.ts\0".as_slice()),
    ] {
        let root = scratch.join(format!("malformed-{name}"));
        fs::create_dir(&root)
            .map_err(|error| format!("cannot create packaged malformed NUL fixture: {error}"))?;
        let operation_id = format!("package-malformed-{name}-0001");
        let arguments = vec![
            OsString::from("pre-write"),
            OsString::from("--operation-id"),
            OsString::from(operation_id),
            OsString::from("--paths0-from"),
            OsString::from("-"),
            OsString::from("--format"),
            OsString::from("json"),
        ];
        let output = run_binary_os_with_stdin(binary, &root, &arguments, input)?;
        expect_status(
            &output,
            Some(2),
            &format!("packaged malformed NUL input ({name})"),
        )?;
        if !output.stdout.is_empty() || output.stderr.is_empty() || root.join(".lumin").exists() {
            return Err(format!(
                "packaged malformed NUL input ({name}) emitted success bytes or initialized state"
            ));
        }
    }
    Ok(())
}

fn assert_native_path_dto(dto: &Value, source: &NativeSourceCase) -> Result<(), String> {
    let expected_display = format!("{}/mod.ts", source.component.display);
    if dto.as_object().map(serde_json::Map::len) != Some(3)
        || dto.get("encoding").and_then(Value::as_str) != Some("repo-path.v1")
        || dto.get("canonicalBase64").and_then(Value::as_str)
            != Some(source.canonical_base64.as_str())
        || dto.get("display").and_then(Value::as_str) != Some(expected_display.as_str())
        || dto.get("utf8").is_some()
    {
        return Err(format!(
            "packaged native path DTO did not preserve its exact bytes: {dto}"
        ));
    }
    Ok(())
}

fn assert_native_root_dto(dto: &Value, root: &Path) -> Result<(), String> {
    let expected = root_oracle::native_root_expectation(root)?;
    if dto.as_object().map(serde_json::Map::len) != Some(3)
        || dto.get("encoding").and_then(Value::as_str) != Some("repository-root.v1")
        || dto.get("canonicalBase64").and_then(Value::as_str)
            != Some(expected.canonical_base64.as_str())
        || dto.get("display").and_then(Value::as_str) != Some(expected.display.as_str())
        || dto.get("readableAddress").is_some()
    {
        return Err(format!(
            "packaged native root DTO changed its complete address or physical identity: {dto}"
        ));
    }
    Ok(())
}

fn items<'a>(value: &'a Value, label: &str) -> Result<&'a [Value], String> {
    required_array(value, "/items", label)
}

pub(super) fn required_array<'a>(
    value: &'a Value,
    pointer: &str,
    label: &str,
) -> Result<&'a [Value], String> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{label} omitted array {pointer}"))
}

pub(super) fn required_string(value: &Value, pointer: &str, label: &str) -> Result<String, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{label} omitted string {pointer}"))
}

pub(super) fn expect_u64(
    value: &Value,
    pointer: &str,
    expected: u64,
    label: &str,
) -> Result<(), String> {
    let observed = value.pointer(pointer).and_then(Value::as_u64);
    if observed != Some(expected) {
        return Err(format!(
            "{label} field {pointer} was {observed:?}; expected {expected}"
        ));
    }
    Ok(())
}

fn expect_bool(value: &Value, pointer: &str, expected: bool, label: &str) -> Result<(), String> {
    let observed = value.pointer(pointer).and_then(Value::as_bool);
    if observed != Some(expected) {
        return Err(format!(
            "{label} field {pointer} was {observed:?}; expected {expected}"
        ));
    }
    Ok(())
}

fn os_arguments(arguments: &[&str]) -> Vec<OsString> {
    arguments.iter().map(OsString::from).collect()
}

struct NativeFixture {
    root: PathBuf,
    sources: Vec<NativeSourceCase>,
}

impl NativeFixture {
    fn create(scratch: &Path) -> Result<Self, String> {
        let (root_component, source_components) = native_components();
        let root = scratch.join(&root_component.native);
        fs::create_dir(&root)
            .map_err(|error| format!("cannot create packaged native root: {error}"))?;
        let mut bulk = String::new();
        for index in 0..BULK_FINDINGS {
            writeln!(bulk, "export const bulk{index} = {index};")
                .map_err(|error| format!("cannot render packaged cursor fixture: {error}"))?;
        }
        fs::write(root.join("bulk.ts"), bulk)
            .map_err(|error| format!("cannot write packaged cursor fixture: {error}"))?;
        let mut sources = Vec::new();
        for (index, source_component) in source_components.into_iter().enumerate() {
            let exported_name = format!("nativeValue{index}");
            let source_directory = root.join(&source_component.native);
            fs::create_dir(&source_directory).map_err(|error| {
                format!("cannot create packaged native source directory: {error}")
            })?;
            fs::write(
                source_directory.join("mod.ts"),
                format!("export const {exported_name} = {index};\n"),
            )
            .map_err(|error| format!("cannot write packaged native source: {error}"))?;
            sources.push(NativeSourceCase::new(source_component, exported_name));
        }
        Ok(Self { root, sources })
    }
}

struct NativeSourceCase {
    component: EncodedComponent,
    relative: OsString,
    canonical: Vec<u8>,
    canonical_base64: String,
    nul_record: Vec<u8>,
    exported_name: String,
}

impl NativeSourceCase {
    fn new(component: EncodedComponent, exported_name: String) -> Self {
        let relative = PathBuf::from(&component.native)
            .join("mod.ts")
            .into_os_string();
        let records = [component.record(), portable_component(b"mod.ts")];
        let canonical = repo_path_bytes(&records);
        let canonical_base64 = STANDARD.encode(&canonical);
        let mut nul_record = component.native_io.clone();
        nul_record.extend_from_slice(b"/mod.ts\0");
        Self {
            component,
            relative,
            canonical,
            canonical_base64,
            nul_record,
            exported_name,
        }
    }
}

struct EncodedComponent {
    native: OsString,
    tag: u8,
    payload: Vec<u8>,
    native_io: Vec<u8>,
    display: String,
}

impl EncodedComponent {
    fn record(&self) -> Vec<u8> {
        component_record(self.tag, &self.payload)
    }
}

pub(super) fn repo_path_base64(components: &[Vec<u8>]) -> String {
    STANDARD.encode(repo_path_bytes(components))
}

fn repo_path_bytes(components: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"LUMRPATH\0\x01".to_vec();
    bytes.extend_from_slice(&(components.len() as u32).to_be_bytes());
    for component in components {
        bytes.extend_from_slice(component);
    }
    bytes
}

pub(super) fn portable_component(payload: &[u8]) -> Vec<u8> {
    component_record(1, payload)
}

fn component_record(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut record = vec![tag];
    record.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    record.extend_from_slice(payload);
    record
}

#[cfg(unix)]
fn native_components() -> (EncodedComponent, Vec<EncodedComponent>) {
    use std::os::unix::ffi::OsStringExt;

    fn component(bytes: Vec<u8>) -> EncodedComponent {
        EncodedComponent {
            native: OsString::from_vec(bytes.clone()),
            tag: 2,
            payload: bytes.clone(),
            native_io: bytes.clone(),
            display: format!(
                "$'{}'",
                bytes
                    .iter()
                    .map(|byte| format!("\\x{byte:02x}"))
                    .collect::<String>()
            ),
        }
    }

    (
        component(vec![b'r', 0x82, b'o', b'o', b't']),
        vec![
            component(vec![b'n', b'a', b't', 0x80]),
            component(vec![b'n', b'a', b't', 0x81]),
        ],
    )
}

#[cfg(windows)]
fn native_components() -> (EncodedComponent, Vec<EncodedComponent>) {
    use std::os::windows::ffi::OsStringExt;

    fn non_scalar(units: &[u16], native_io: &[u8]) -> EncodedComponent {
        let mut payload = Vec::with_capacity(units.len() * 2);
        for unit in units {
            payload.extend_from_slice(&unit.to_be_bytes());
        }
        EncodedComponent {
            native: OsString::from_wide(units),
            tag: 3,
            payload,
            native_io: native_io.to_vec(),
            display: format!(
                "wtf16[{}]",
                units
                    .iter()
                    .map(|unit| format!("{unit:04x}"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }

    fn portable(value: &str) -> EncodedComponent {
        EncodedComponent {
            native: OsString::from_wide(&value.encode_utf16().collect::<Vec<_>>()),
            tag: 1,
            payload: value.as_bytes().to_vec(),
            native_io: value.as_bytes().to_vec(),
            display: value.to_owned(),
        }
    }

    (
        non_scalar(&[0xd802, b'r' as u16], &[0xed, 0xa0, 0x82, b'r']),
        vec![
            portable("\u{e9}"),
            portable("e\u{301}"),
            non_scalar(
                &[b'n' as u16, b'a' as u16, b't' as u16, 0xd800],
                &[b'n', b'a', b't', 0xed, 0xa0, 0x80],
            ),
            non_scalar(
                &[b'n' as u16, b'a' as u16, b't' as u16, 0xd801],
                &[b'n', b'a', b't', 0xed, 0xa0, 0x81],
            ),
        ],
    )
}
