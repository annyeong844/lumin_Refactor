use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

mod support;

use support::{assert_status, field, run, run_os_with_stdin};

const BULK_FINDINGS: usize = 100;

#[derive(Debug, Eq, PartialEq)]
struct FindingSnapshot {
    finding_id: String,
    source_id: String,
    canonical_path: String,
}

#[test]
fn native_repository_paths_round_trip_through_public_queries_and_cursors()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = NativeFixture::create()?;

    let first_audit = audit(&fixture.root, fixture.cases.len())?;
    let first_root = first_audit
        .get("repositoryRoot")
        .ok_or_else(|| std::io::Error::other("audit omitted repositoryRoot"))?;
    let first_root_identity = assert_nonportable_root_dto(
        first_root,
        &fixture.root_component.record(),
        &fixture.root_component.display,
    )?;
    let first_run = first_audit
        .get("runId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("audit omitted runId"))?;
    let first_findings = findings_snapshot(&fixture.root, first_run, &fixture.cases)?;

    for case in &fixture.cases {
        assert_native_file_query(&fixture.root, first_run, case)?;
    }
    let nonportable = fixture
        .cases
        .iter()
        .find(|case| case.component.tag != 1)
        .ok_or_else(|| std::io::Error::other("fixture omitted a nonportable source path"))?;
    assert_display_is_not_a_query_identity(&fixture.root, first_run, nonportable)?;

    let second_audit = audit(&fixture.root, fixture.cases.len())?;
    let second_root = second_audit
        .get("repositoryRoot")
        .ok_or_else(|| std::io::Error::other("second audit omitted repositoryRoot"))?;
    assert_eq!(
        assert_nonportable_root_dto(
            second_root,
            &fixture.root_component.record(),
            &fixture.root_component.display,
        )?,
        first_root_identity,
        "the same native repository root changed canonical identity",
    );
    let second_run = second_audit
        .get("runId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("second audit omitted runId"))?;
    assert_eq!(
        findings_snapshot(&fixture.root, second_run, &fixture.cases)?,
        first_findings,
        "native finding IDs or cursor ordering changed between runs",
    );

    assert_native_nul_input(&fixture.root, &fixture.cases)?;
    Ok(())
}

fn audit(root: &Path, native_source_count: usize) -> Result<Value, Box<dyn std::error::Error>> {
    let result = run(root, &["audit", "--jobs", "1"])?;
    assert_status(&result, 0);
    let response: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(
        response.get("findingCount").and_then(Value::as_u64),
        Some((BULK_FINDINGS + native_source_count) as u64),
        "native sources were merged or omitted",
    );
    Ok(response)
}

fn findings_snapshot(
    root: &Path,
    run_id: &str,
    cases: &[NativeSourceCase],
) -> Result<Vec<FindingSnapshot>, Box<dyn std::error::Error>> {
    let first = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&first, 0);
    let first: Value = serde_json::from_str(&first.stdout)?;
    assert_eq!(first.get("returned").and_then(Value::as_u64), Some(100));
    assert_eq!(first.get("truncated").and_then(Value::as_bool), Some(true));
    let cursor = first
        .get("nextCursor")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("first findings page omitted its cursor"))?;

    let second = run(
        root,
        &[
            "findings",
            "--run",
            run_id,
            "--area",
            "dead-code",
            "--cursor",
            cursor,
        ],
    )?;
    assert_status(&second, 0);
    let second: Value = serde_json::from_str(&second.stdout)?;
    assert_eq!(
        second.get("returned").and_then(Value::as_u64),
        Some(cases.len() as u64),
    );
    assert_eq!(
        second.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    assert!(second.get("nextCursor").is_none_or(Value::is_null));

    let first_items = items(&first)?;
    let bulk_path = STANDARD.encode(repo_path_bytes(&[portable_component(b"bulk.ts")]));
    assert!(first_items.iter().all(|item| {
        item.pointer("/path/canonicalBase64")
            .and_then(Value::as_str)
            == Some(bulk_path.as_str())
    }));

    let second_items = items(&second)?;
    let actual_native_paths = second_items
        .iter()
        .map(|item| required_string(item, "/path/canonicalBase64"))
        .collect::<Result<Vec<_>, _>>()?;
    let mut expected_native_paths = cases
        .iter()
        .map(|case| (case.canonical.clone(), case.canonical_base64.clone()))
        .collect::<Vec<_>>();
    expected_native_paths.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        actual_native_paths,
        expected_native_paths
            .into_iter()
            .map(|(_, encoded)| encoded)
            .collect::<Vec<_>>(),
        "the cursor did not resume in canonical native path order",
    );

    let mut all_items = first_items.to_vec();
    all_items.extend(second_items.iter().cloned());
    let finding_ids = all_items
        .iter()
        .map(|item| required_string(item, "/findingId"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    assert_eq!(finding_ids.len(), BULK_FINDINGS + cases.len());
    Ok(all_items
        .iter()
        .map(|item| {
            Ok(FindingSnapshot {
                finding_id: required_string(item, "/findingId")?,
                source_id: required_string(item, "/sourceId")?,
                canonical_path: required_string(item, "/path/canonicalBase64")?,
            })
        })
        .collect::<Result<Vec<_>, std::io::Error>>()?)
}

fn assert_native_file_query(
    root: &Path,
    run_id: &str,
    case: &NativeSourceCase,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments: Vec<OsString> = vec!["files".into(), "--run".into(), run_id.into()];
    if case.component.native_io.starts_with(b"--") {
        let mut unescaped = arguments.clone();
        unescaped.push(case.relative.clone());
        let rejected = run_os_with_stdin(root, &unescaped, &[])?;
        assert_status(&rejected, 2);
        assert!(
            rejected.stderr.contains("unknown command or argument"),
            "option-shaped native path did not fail as an unknown argument: {}",
            rejected.stderr,
        );
        arguments.push("--".into());
    }
    arguments.push(case.relative.clone());
    let result = run_os_with_stdin(root, &arguments, &[])?;
    assert_status(&result, 0);
    let response: Value = serde_json::from_str(&result.stdout)?;
    assert_path_dto(
        response
            .pointer("/sourceContext/path")
            .ok_or_else(|| std::io::Error::other("files query omitted sourceContext.path"))?,
        case,
    );
    let result_items = items(&response)?;
    assert_eq!(result_items.len(), 1);
    assert_path_dto(
        result_items[0]
            .get("path")
            .ok_or_else(|| std::io::Error::other("finding omitted path"))?,
        case,
    );
    Ok(())
}

fn assert_display_is_not_a_query_identity(
    root: &Path,
    run_id: &str,
    case: &NativeSourceCase,
) -> Result<(), Box<dyn std::error::Error>> {
    let display = format!("{}/mod.ts", case.component.display);
    let result = run(root, &["files", "--run", run_id, &display])?;
    assert_status(&result, 0);
    let response: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(0));
    assert!(items(&response)?.is_empty());
    Ok(())
}

fn assert_native_nul_input(
    root: &Path,
    cases: &[NativeSourceCase],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::new();
    for case in cases {
        input.extend_from_slice(&case.nul_record);
    }
    let arguments = vec![
        "pre-write".into(),
        "--operation-id".into(),
        "op-repo-path-lossless".into(),
        "--paths0-from".into(),
        "-".into(),
        "--jobs".into(),
        "1".into(),
    ];
    let result = run_os_with_stdin(root, &arguments, &input)?;
    assert_status(&result, 0);
    assert_eq!(field(&result.stdout, "decision")?, "allow-with-warnings");
    let response: Value = serde_json::from_str(&result.stdout)?;
    let leased = response
        .get("leasedWriteSet")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("pre-write omitted leasedWriteSet"))?;
    let actual = leased
        .iter()
        .map(|lease| required_string(lease, "/path/canonicalBase64"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let expected = cases
        .iter()
        .map(|case| case.canonical_base64.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "native NUL input changed the write set");
    Ok(())
}

fn assert_path_dto(dto: &Value, case: &NativeSourceCase) {
    let expected_display = format!("{}/mod.ts", case.component.display);
    assert_eq!(
        dto.get("encoding").and_then(Value::as_str),
        Some("repo-path.v1")
    );
    assert_eq!(
        dto.get("canonicalBase64").and_then(Value::as_str),
        Some(case.canonical_base64.as_str())
    );
    assert_eq!(
        dto.get("display").and_then(Value::as_str),
        Some(expected_display.as_str())
    );
    assert!(dto.get("utf8").is_none());
}

fn assert_nonportable_root_dto(
    dto: &Value,
    final_component: &[u8],
    display_component: &str,
) -> Result<String, Box<dyn std::error::Error>> {
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
    assert_eq!(bytes.get(..10), Some(b"LUMRROOT\0\x01".as_slice()));
    let physical_length = if cfg!(windows) { 25 } else { 17 };
    let address_end = bytes
        .len()
        .checked_sub(physical_length)
        .ok_or_else(|| std::io::Error::other("root DTO omitted physical identity"))?;
    assert!(bytes[..address_end].ends_with(final_component));
    assert_eq!(bytes[address_end], if cfg!(windows) { 2 } else { 1 });
    assert!(
        dto.get("display")
            .and_then(Value::as_str)
            .is_some_and(|display| display.ends_with(display_component))
    );
    assert!(dto.get("readableAddress").is_none());
    Ok(encoded.to_owned())
}

fn items(response: &Value) -> Result<&[Value], std::io::Error> {
    response
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| std::io::Error::other("query omitted items"))
}

fn required_string(value: &Value, pointer: &str) -> Result<String, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("missing string at {pointer}")))
}

struct NativeFixture {
    _parent: tempfile::TempDir,
    root: PathBuf,
    root_component: EncodedComponent,
    cases: Vec<NativeSourceCase>,
}

impl NativeFixture {
    fn create() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = tempfile::tempdir()?;
        let (root_component, source_components) = native_components();
        let root = parent.path().join(&root_component.native);
        fs::create_dir(&root)?;

        let mut bulk = String::new();
        for index in 0..BULK_FINDINGS {
            use std::fmt::Write as _;
            writeln!(bulk, "export const bulk{index} = {index};")?;
        }
        fs::write(root.join("bulk.ts"), bulk)?;

        let mut cases = Vec::new();
        for (index, component) in source_components.into_iter().enumerate() {
            let directory = root.join(&component.native);
            fs::create_dir(&directory)?;
            fs::write(
                directory.join("mod.ts"),
                format!("export const native{index} = {index};\n"),
            )?;
            cases.push(NativeSourceCase::new(component));
        }
        Ok(Self {
            _parent: parent,
            root,
            root_component,
            cases,
        })
    }
}

struct NativeSourceCase {
    component: EncodedComponent,
    relative: OsString,
    canonical: Vec<u8>,
    canonical_base64: String,
    nul_record: Vec<u8>,
}

impl NativeSourceCase {
    fn new(component: EncodedComponent) -> Self {
        let relative = PathBuf::from(&component.native)
            .join("mod.ts")
            .into_os_string();
        let canonical = repo_path_bytes(&[component.record(), portable_component(b"mod.ts")]);
        let canonical_base64 = STANDARD.encode(&canonical);
        let mut nul_record = component.native_io.clone();
        nul_record.extend_from_slice(b"/mod.ts\0");
        Self {
            component,
            relative,
            canonical,
            canonical_base64,
            nul_record,
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

fn repo_path_bytes(components: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"LUMRPATH\0\x01".to_vec();
    bytes.extend_from_slice(&(components.len() as u32).to_be_bytes());
    for component in components {
        bytes.extend_from_slice(component);
    }
    bytes
}

fn portable_component(payload: &[u8]) -> Vec<u8> {
    component_record(1, payload)
}

fn component_record(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut record = vec![tag];
    record.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    record.extend_from_slice(payload);
    record
}

fn portable_encoded_component(value: &str) -> EncodedComponent {
    EncodedComponent {
        native: OsString::from(value),
        tag: 1,
        payload: value.as_bytes().to_vec(),
        native_io: value.as_bytes().to_vec(),
        display: value.to_owned(),
    }
}

#[cfg(unix)]
fn native_components() -> (EncodedComponent, Vec<EncodedComponent>) {
    use std::os::unix::ffi::OsStringExt;

    fn component(bytes: Vec<u8>) -> EncodedComponent {
        let display = format!(
            "$'{}'",
            bytes
                .iter()
                .map(|byte| format!("\\x{byte:02x}"))
                .collect::<String>()
        );
        EncodedComponent {
            native: OsString::from_vec(bytes.clone()),
            tag: 2,
            payload: bytes.clone(),
            native_io: bytes,
            display,
        }
    }

    (
        component(vec![b'r', 0x82, b'o', b'o', b't']),
        vec![
            portable_encoded_component("--option"),
            component(vec![b'-', b'-', b'n', b'a', b't', 0x80]),
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

    (
        non_scalar(&[0xd802, b'r' as u16], &[0xed, 0xa0, 0x82, b'r']),
        vec![
            portable_encoded_component("--option"),
            portable_encoded_component("\u{e9}"),
            portable_encoded_component("e\u{301}"),
            non_scalar(
                &[b'-' as u16, b'-' as u16, 0xd800, b'a' as u16],
                &[b'-', b'-', 0xed, 0xa0, 0x80, b'a'],
            ),
        ],
    )
}
