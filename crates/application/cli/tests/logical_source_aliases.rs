use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

const ALIASES: &[&str] = &[
    "packages/a/src/Case.ts",
    "packages/a/src/case.ts",
    "packages/a/src/same-context-alias.ts",
    "packages/b/src/cross-hardlink.ts",
    "packages/b/src/cross-package-alias.ts",
];

#[test]
fn logical_source_physical_aliases_keep_context_and_reuse_payload()
-> Result<(), Box<dyn std::error::Error>> {
    let root = alias_fixture()?;
    let audit = run(
        root.path(),
        &[
            "audit",
            "--entry",
            "packages/a/src/Case.ts",
            "--entry",
            "packages/a/src/case.ts",
        ],
    )?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;

    let overview = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    assert_eq!(
        overview.get("analysisMetrics"),
        Some(&serde_json::json!({
            "logicalSourceCount": 7,
            "physicalSourceCount": 3,
            "payloadSnapshotCount": 3,
            "jsParseProductCount": 3,
        }))
    );

    let package_a_dep = source_id(root.path(), &run_id, "packages/a/src/dep.ts")?;
    let package_b_dep = source_id(root.path(), &run_id, "packages/b/src/dep.ts")?;
    let mut logical_ids = BTreeSet::new();
    let mut observations = Vec::new();
    for path in ALIASES {
        let response = file_response(root.path(), &run_id, path)?;
        let source_id = required_str(&response, "/sourceContext/sourceId")?;
        assert!(logical_ids.insert(source_id));
        assert_eq!(
            required_str(&response, "/sourceContext/path/display")?,
            *path
        );
        assert_eq!(
            response
                .pointer("/sourceContext/kind")
                .and_then(Value::as_str),
            Some("type-script")
        );
        assert_eq!(response.get("returned").and_then(Value::as_u64), Some(2));

        let in_package_a = path.starts_with("packages/a/");
        let expected_package = if in_package_a {
            "packages/a"
        } else {
            "packages/b"
        };
        let expected_role = if in_package_a {
            "authored"
        } else {
            "generated"
        };
        let expected_profile = if in_package_a { "bundler" } else { "node" };
        let expected_config = if in_package_a {
            "packages/a/tsconfig.json"
        } else {
            "packages/b/tsconfig.json"
        };
        let expected_target = if in_package_a {
            package_a_dep.as_str()
        } else {
            package_b_dep.as_str()
        };
        assert_eq!(
            required_str(&response, "/sourceContext/packageRoot/display")?,
            expected_package
        );
        assert_eq!(
            required_str(&response, "/sourceClassification/classifications/0/role")?,
            expected_role
        );
        assert_eq!(
            required_str(&response, "/resolutionProfile/profile")?,
            expected_profile
        );
        assert_eq!(
            required_str(&response, "/resolutionProfile/source/kind")?,
            "config"
        );
        assert_eq!(
            required_str(&response, "/resolutionProfile/source/path_display")?,
            expected_config
        );
        assert_eq!(
            required_str(&response, "/resolutions/0/sourceUse/specifier")?,
            "./dep.js"
        );
        assert_eq!(
            required_str(&response, "/resolutions/0/outcome/kind")?,
            "internal"
        );
        assert_eq!(
            required_str(&response, "/resolutions/0/outcome/target")?,
            expected_target
        );
        observations.push(
            response
                .get("sourceObservation")
                .cloned()
                .ok_or_else(|| std::io::Error::other("sourceObservation is missing"))?,
        );
    }
    assert_eq!(logical_ids.len(), ALIASES.len());
    for observation in observations.iter().skip(1) {
        assert_eq!(
            observation.get("physicalIdentity"),
            observations[0].get("physicalIdentity")
        );
        assert_eq!(
            observation.get("payloadSnapshotId"),
            observations[0].get("payloadSnapshotId")
        );
    }

    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-logical-source-aliases",
            "--path",
            "packages/a/src/Case.ts",
            "--entry",
            "packages/a/src/Case.ts",
            "--entry",
            "packages/a/src/case.ts",
        ],
    )?;
    assert_status(&pre, 0);
    let pre: Value = serde_json::from_str(&pre.stdout)?;
    let leased = pre
        .get("leasedWriteSet")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("leasedWriteSet is missing"))?
        .iter()
        .map(|lease| required_str(lease, "/path/display"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    assert_eq!(
        leased,
        ALIASES.iter().map(|path| (*path).to_owned()).collect()
    );
    Ok(())
}

fn alias_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    for package in ["a", "b"] {
        fs::create_dir_all(root.path().join(format!("packages/{package}/src")))?;
        fs::write(
            root.path().join(format!("packages/{package}/package.json")),
            format!(r#"{{"name":"@fixture/{package}"}}"#),
        )?;
    }
    fs::write(
        root.path().join("lumin.json"),
        concat!(
            r#"{"schemaVersion":"lumin-config.v1","scan":{"roles":["#,
            r#"{"pattern":"packages/a/src/**","role":"authored"},"#,
            r#"{"pattern":"packages/b/src/**","role":"generated"}"#,
            "]}}",
        ),
    )?;
    fs::write(
        root.path().join("packages/a/tsconfig.json"),
        r#"{"compilerOptions":{"moduleResolution":"bundler"}}"#,
    )?;
    fs::write(
        root.path().join("packages/b/tsconfig.json"),
        r#"{"compilerOptions":{"moduleResolution":"node"}}"#,
    )?;
    fs::write(
        root.path().join("packages/a/src/dep.ts"),
        "export const dependency = 'a';\n",
    )?;
    fs::write(
        root.path().join("packages/b/src/dep.ts"),
        "export const dependency = 'b';\n",
    )?;
    let original = root.path().join("packages/a/src/Case.ts");
    fs::write(
        &original,
        concat!(
            "import { dependency } from './dep.js';\n",
            "export const shared = dependency;\n",
            "export const deadAlias = 1;\n",
        ),
    )?;
    create_file_alias(
        &original,
        &root.path().join("packages/a/src/same-context-alias.ts"),
    )?;
    create_file_alias(
        &original,
        &root.path().join("packages/b/src/cross-package-alias.ts"),
    )?;
    fs::hard_link(
        &original,
        root.path().join("packages/b/src/cross-hardlink.ts"),
    )?;
    #[cfg(unix)]
    fs::hard_link(&original, root.path().join("packages/a/src/case.ts"))?;
    Ok(root)
}

fn file_response(
    root: &Path,
    run_id: &str,
    path: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, &["files", "--run", run_id, path])?;
    assert_status(&output, 0);
    Ok(serde_json::from_str(&output.stdout)?)
}

fn source_id(root: &Path, run_id: &str, path: &str) -> Result<String, Box<dyn std::error::Error>> {
    required_str(
        &file_response(root, run_id, path)?,
        "/sourceContext/sourceId",
    )
    .map_err(Into::into)
}

fn required_str(value: &Value, pointer: &str) -> Result<String, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("missing string {pointer}")))
}

#[cfg(unix)]
fn create_file_alias(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_file_alias(target: &Path, link: &Path) -> std::io::Result<()> {
    // The Linux lane exercises file symlinks. Windows runners may not hold
    // SeCreateSymbolicLinkPrivilege, so this lane proves hard-link closure plus
    // the case-insensitive alternate lexical spelling without a conditional skip.
    fs::hard_link(target, link)
}
