use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingIdentity = (String, String, String);

#[test]
fn limitation_scope_exhaustiveness_is_public() -> Result<(), Box<dyn std::error::Error>> {
    file_and_workspace_required_evidence_remain_distinct()?;
    package_required_evidence_does_not_escape_its_owner()?;
    public_surface_required_evidence_follows_its_consumer()?;
    resolved_module_opacity_remains_advisory()?;
    mixed_opacity_scope_selects_fact_or_required_gap()?;
    known_empty_target_scope_remains_normalized()?;
    unavailable_sfc_owner_has_a_distinct_gate_signal()?;
    Ok(())
}

fn file_and_workspace_required_evidence_remain_distinct() -> Result<(), Box<dyn std::error::Error>>
{
    let local = tempfile::tempdir()?;
    write(
        local.path(),
        "package.json",
        r#"{"name":"local-gap","private":true,"type":"module"}"#,
    )?;
    write(
        local.path(),
        "src/broken.ts",
        concat!(
            "import { used } from './target.js';\n",
            "console.log(used);\n",
            "export const visible = 1;\n",
            "export const hiddenLocal;\n",
        ),
    )?;
    write(
        local.path(),
        "src/consumer.ts",
        "import { visible } from './broken.js'; console.log(visible);\n",
    )?;
    write(
        local.path(),
        "src/target.ts",
        "export const used = 1; export const deadSibling = 2;\n",
    )?;
    write(
        local.path(),
        "src/unrelated.ts",
        "export const unrelatedDead = 1;\n",
    )?;
    write(local.path(), "src/safe.ts", "console.log('safe');\n")?;

    let local_audit = run(local.path(), &["audit", "--jobs", "1"])?;
    assert_status(&local_audit, 0);
    let local_run = field(&local_audit.stdout, "runId")?;
    let local_overview = json_command(local.path(), &["overview", "--run", &local_run], 0)?;
    assert_eq!(
        limitation_reasons(&local_overview)?,
        BTreeSet::from(["js-recoverable-parse-local".to_owned()]),
    );
    assert_eq!(
        finding_paths(local.path(), &local_run)?,
        BTreeSet::from(["src/target.ts".to_owned(), "src/unrelated.ts".to_owned(),]),
        "a file-local definition gap escaped its source or erased recovered module uses",
    );
    let safe = json_command(
        local.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-limitation-local-safe",
            "--path",
            "src/safe.ts",
            "--jobs",
            "1",
        ],
        0,
    )?;
    assert!(!has_signal(&safe, "required-evidence-incomplete"));
    let consumer = json_command(
        local.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-limitation-local-consumer",
            "--path",
            "src/consumer.ts",
            "--jobs",
            "1",
        ],
        4,
    )?;
    assert!(has_signal(&consumer, "required-evidence-incomplete"));

    let workspace = tempfile::tempdir()?;
    write(
        workspace.path(),
        "package.json",
        r#"{"name":"workspace-gap","private":true,"type":"module"}"#,
    )?;
    write(workspace.path(), "src/broken.ts", "export const = ;\n")?;
    write(
        workspace.path(),
        "src/candidate.ts",
        "export const mustNotBecomeADeletionCandidate = 1;\n",
    )?;
    write(workspace.path(), "src/safe.ts", "console.log('safe');\n")?;
    let workspace_audit = run(workspace.path(), &["audit", "--jobs", "1"])?;
    assert_status(&workspace_audit, 0);
    let workspace_run = field(&workspace_audit.stdout, "runId")?;
    let workspace_overview =
        json_command(workspace.path(), &["overview", "--run", &workspace_run], 0)?;
    assert_eq!(
        limitation_reasons(&workspace_overview)?,
        BTreeSet::from(["js-module-use-unknown".to_owned()]),
    );
    assert!(finding_paths(workspace.path(), &workspace_run)?.is_empty());
    let blocked = json_command(
        workspace.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-limitation-workspace",
            "--path",
            "src/safe.ts",
            "--jobs",
            "1",
        ],
        4,
    )?;
    assert!(has_signal(&blocked, "required-evidence-incomplete"));
    Ok(())
}

fn package_required_evidence_does_not_escape_its_owner() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    )?;
    write(
        root.path(),
        "packages/opaque/package.json",
        r#"{"name":"@scope/opaque","private":true,"imports":7}"#,
    )?;
    write(
        root.path(),
        "packages/opaque/main.ts",
        "import value from '#internal'; export const opaqueDead = value;\n",
    )?;
    write(
        root.path(),
        "packages/clear/package.json",
        r#"{"name":"@scope/clear","private":true}"#,
    )?;
    write(
        root.path(),
        "packages/clear/candidate.ts",
        "export const clearDead = 1;\n",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    let overview = json_command(root.path(), &["overview", "--run", &run_id], 0)?;
    assert_eq!(
        limitation_reasons(&overview)?,
        BTreeSet::from(["package-imports-unsupported".to_owned()]),
    );
    assert_eq!(
        finding_paths(root.path(), &run_id)?,
        BTreeSet::from(["packages/clear/candidate.ts".to_owned()]),
        "package-scoped uncertainty either escaped its owner or failed to suppress its owner",
    );

    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-limitation-clear-package",
            "--path",
            "packages/clear/candidate.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    assert_eq!(field(&opened.stdout, "decision")?, "allow-with-warnings");
    let opened: Value = serde_json::from_str(&opened.stdout)?;
    assert!(
        !has_signal(&opened, "required-evidence-incomplete"),
        "package-scoped required evidence blocked a disjoint package: {opened:#?}",
    );
    Ok(())
}

fn public_surface_required_evidence_follows_its_consumer() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"surface-root","private":true,"workspaces":["packages/*"]}"#,
    )?;
    write(
        root.path(),
        "packages/app/package.json",
        r#"{"name":"@scope/app","private":true}"#,
    )?;
    write(
        root.path(),
        "packages/app/main.ts",
        "import { selected } from '@scope/lib'; console.log(selected);\n",
    )?;
    write(
        root.path(),
        "packages/lib/package.json",
        r#"{"name":"@scope/lib","private":true,"exports":{"custom":"./custom.js","default":"./default.js"}}"#,
    )?;
    write(
        root.path(),
        "packages/lib/custom.ts",
        "export const selected = 1; export const customDead = 2;\n",
    )?;
    write(
        root.path(),
        "packages/lib/default.ts",
        "export const selected = 1; export const defaultDead = 2;\n",
    )?;
    write(
        root.path(),
        "packages/clear/package.json",
        r#"{"name":"@scope/clear","private":true}"#,
    )?;
    write(
        root.path(),
        "packages/clear/candidate.ts",
        "export const clearDead = 1;\n",
    )?;

    let audit = run(
        root.path(),
        &["audit", "--jobs", "1", "--resolution-profile", "bundler"],
    )?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    let overview = json_command(root.path(), &["overview", "--run", &run_id], 0)?;
    assert_eq!(
        limitation_reasons(&overview)?,
        BTreeSet::from(["public-surface-unsupported".to_owned()]),
    );
    assert_eq!(
        finding_paths(root.path(), &run_id)?,
        BTreeSet::from(["packages/clear/candidate.ts".to_owned()]),
    );
    let consumer = json_command(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-limitation-surface-consumer",
            "--path",
            "packages/app/main.ts",
            "--jobs",
            "1",
            "--resolution-profile",
            "bundler",
        ],
        4,
    )?;
    assert!(has_signal(&consumer, "required-evidence-incomplete"));
    let clear = json_command(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-limitation-surface-clear",
            "--path",
            "packages/clear/candidate.ts",
            "--jobs",
            "1",
            "--resolution-profile",
            "bundler",
        ],
        0,
    )?;
    assert!(!has_signal(&clear, "required-evidence-incomplete"));
    Ok(())
}

fn resolved_module_opacity_remains_advisory() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"module-opacity","private":true,"type":"commonjs"}"#,
    )?;
    write(
        root.path(),
        "src/main.ts",
        concat!(
            "const key = process.argv[2] ?? 'first';\n",
            "console.log(require('./target.js')[key]);\n",
        ),
    )?;
    write(
        root.path(),
        "src/target.ts",
        concat!(
            "export const first = 1; export const second = 2; ",
            "export type TargetType = string;\n",
        ),
    )?;
    write(
        root.path(),
        "src/unrelated.ts",
        "export const unrelated = 1; export type UnrelatedType = string;\n",
    )?;

    let audit = run(
        root.path(),
        &["audit", "--jobs", "1", "--resolution-profile", "node16"],
    )?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    let overview = json_command(root.path(), &["overview", "--run", &run_id], 0)?;
    assert_eq!(
        limitation_reasons(&overview)?,
        BTreeSet::from(["common-js-computed-member".to_owned()]),
    );
    assert_eq!(
        finding_identities(root.path(), &run_id)?,
        BTreeSet::from([
            (
                "src/target.ts".to_owned(),
                "TargetType".to_owned(),
                "type".to_owned(),
            ),
            (
                "src/unrelated.ts".to_owned(),
                "unrelated".to_owned(),
                "value".to_owned(),
            ),
            (
                "src/unrelated.ts".to_owned(),
                "UnrelatedType".to_owned(),
                "type".to_owned(),
            ),
        ]),
        "resolved-module opacity did not consume only the target's value exports",
    );
    let opened = json_command(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-limitation-module-opacity",
            "--path",
            "src/main.ts",
            "--jobs",
            "1",
            "--resolution-profile",
            "node16",
        ],
        0,
    )?;
    assert_eq!(
        opened.get("decision").and_then(Value::as_str),
        Some("allow-with-warnings")
    );
    assert!(!has_signal(&opened, "required-evidence-incomplete"));
    assert!(!has_signal(&opened, "required-owner-unavailable"));
    Ok(())
}

fn mixed_opacity_scope_selects_fact_or_required_gap() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"glob-root","private":true,"workspaces":["packages/*"]}"#,
    )?;
    for package in ["explicit", "opaque"] {
        write(
            root.path(),
            &format!("packages/{package}/package.json"),
            &format!(r#"{{"name":"@scope/{package}","private":true,"type":"module"}}"#),
        )?;
    }
    write(
        root.path(),
        "packages/explicit/main.ts",
        concat!(
            "const modules = import.meta.glob('./targets/*.ts', { eager: true });\n",
            "console.log(modules);\n",
        ),
    )?;
    write(
        root.path(),
        "packages/explicit/targets/one.ts",
        "export const oneValue = 1; export type OneType = string;\n",
    )?;
    write(
        root.path(),
        "packages/explicit/unrelated.ts",
        "export const explicitDead = 1;\n",
    )?;
    write(
        root.path(),
        "packages/opaque/main.ts",
        "const modules = import.meta.glob('@opaque/*.ts'); console.log(modules);\n",
    )?;
    write(
        root.path(),
        "packages/opaque/blocked.ts",
        "export const opaqueDead = 1;\n",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    let overview = json_command(root.path(), &["overview", "--run", &run_id], 0)?;
    assert_eq!(
        limitation_reasons(&overview)?,
        BTreeSet::from(["import-meta-glob-unsupported".to_owned()]),
    );
    let scopes = overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("glob limitations are missing"))?
        .iter()
        .filter_map(|limitation| {
            limitation
                .pointer("/targetScope/kind")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        scopes,
        BTreeSet::from(["explicit-targets".to_owned(), "package".to_owned()])
    );
    assert_eq!(
        finding_identities(root.path(), &run_id)?,
        BTreeSet::from([
            (
                "packages/explicit/targets/one.ts".to_owned(),
                "OneType".to_owned(),
                "type".to_owned(),
            ),
            (
                "packages/explicit/unrelated.ts".to_owned(),
                "explicitDead".to_owned(),
                "value".to_owned(),
            ),
        ]),
    );
    let opaque = json_command(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-limitation-opacity-package",
            "--path",
            "packages/opaque/main.ts",
            "--jobs",
            "1",
        ],
        4,
    )?;
    assert!(has_signal(&opaque, "required-evidence-incomplete"));
    let explicit = json_command(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-limitation-opacity-explicit",
            "--path",
            "packages/explicit/main.ts",
            "--jobs",
            "1",
        ],
        0,
    )?;
    assert_eq!(
        explicit.get("decision").and_then(Value::as_str),
        Some("allow-with-warnings")
    );
    assert!(!has_signal(&explicit, "required-evidence-incomplete"));
    Ok(())
}

fn known_empty_target_scope_remains_normalized() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"known-empty","private":true,"type":"commonjs","workspaces":["packages/*"]}"#,
    )?;
    write(
        root.path(),
        "tsconfig.json",
        r#"{"compilerOptions":{"moduleResolution":"node16","module":"node16"}}"#,
    )?;
    write(
        root.path(),
        "packages/ui/package.json",
        r#"{"name":"@app/ui","private":true,"exports":{"./blocked":null}}"#,
    )?;
    write(
        root.path(),
        "src/main.ts",
        concat!(
            "import { blocked } from '@app/ui/blocked';\n",
            "console.log(blocked);\n",
        ),
    )?;
    write(
        root.path(),
        "src/candidate.ts",
        "export const stillDead = 1;\n",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    let overview = json_command(root.path(), &["overview", "--run", &run_id], 0)?;
    assert_eq!(
        limitation_reasons(&overview)?,
        BTreeSet::from(["internal-specifier-unresolved".to_owned()]),
    );
    let limitation = overview
        .get("limitations")
        .and_then(Value::as_array)
        .and_then(|limitations| limitations.first())
        .ok_or_else(|| std::io::Error::other("known-target limitation is missing"))?;
    assert_eq!(
        limitation
            .pointer("/targetScope/kind")
            .and_then(Value::as_str),
        Some("known-no-target")
    );
    assert_eq!(
        finding_paths(root.path(), &run_id)?,
        BTreeSet::from(["src/candidate.ts".to_owned()]),
    );
    let opened = json_command(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-limitation-known-empty",
            "--path",
            "src/main.ts",
            "--jobs",
            "1",
        ],
        0,
    )?;
    assert_eq!(
        opened.get("decision").and_then(Value::as_str),
        Some("allow-with-warnings")
    );
    assert!(!has_signal(&opened, "required-evidence-incomplete"));
    Ok(())
}

fn unavailable_sfc_owner_has_a_distinct_gate_signal() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"sfc-owner-gap","private":true}"#,
    )?;
    write(
        root.path(),
        "src/App.svelte",
        "<script>export let value;</script><p>{value}</p>\n",
    )?;
    write(root.path(), "src/edit.ts", "export const edit = 1;\n")?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    let overview = json_command(root.path(), &["overview", "--run", &run_id], 0)?;
    assert_eq!(
        limitation_reasons(&overview)?,
        BTreeSet::from(["sfc-dialect-unavailable".to_owned()]),
    );

    let opened = json_command(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-limitation-required-owner",
            "--path",
            "src/edit.ts",
            "--jobs",
            "1",
        ],
        4,
    )?;
    assert_eq!(
        opened.get("decision").and_then(Value::as_str),
        Some("incomplete")
    );
    assert!(has_signal(&opened, "required-owner-unavailable"));
    assert!(!has_signal(&opened, "required-evidence-incomplete"));
    Ok(())
}

fn finding_paths(
    root: &Path,
    run_id: &str,
) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    Ok(finding_identities(root, run_id)?
        .into_iter()
        .map(|(path, _, _)| path)
        .collect())
}

fn finding_identities(
    root: &Path,
    run_id: &str,
) -> Result<BTreeSet<FindingIdentity>, Box<dyn std::error::Error>> {
    let response = json_command(
        root,
        &["findings", "--run", run_id, "--area", "dead-code"],
        0,
    )?;
    response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("finding items are missing"))?
        .iter()
        .map(|item| {
            let path = item
                .pointer("/path/display")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("finding path is missing"))?;
            let name = item
                .get("exportedName")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("finding export is missing"))?;
            let namespace = item
                .get("namespace")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("finding namespace is missing"))?;
            Ok((path.to_owned(), name.to_owned(), namespace.to_owned()))
        })
        .collect::<Result<BTreeSet<_>, std::io::Error>>()
        .map_err(Into::into)
}

fn limitation_reasons(overview: &Value) -> Result<BTreeSet<String>, std::io::Error> {
    overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("overview limitations are missing"))?
        .iter()
        .map(|limitation| {
            limitation
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other("limitation reason is missing"))
        })
        .collect()
}

fn has_signal(response: &Value, kind: &str) -> bool {
    response
        .get("signals")
        .and_then(Value::as_array)
        .is_some_and(|signals| {
            signals
                .iter()
                .any(|signal| signal.get("kind").and_then(Value::as_str) == Some(kind))
        })
}

fn json_command(
    root: &Path,
    arguments: &[&str],
    expected_status: i32,
) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, arguments)?;
    assert_status(&output, expected_status);
    Ok(serde_json::from_str(&output.stdout)?)
}

fn write(root: &Path, relative: &str, contents: &str) -> Result<(), std::io::Error> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
