use super::*;

#[test]
fn root_dirs_blocks_relative_probes_and_disables_only_affected_absence()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace_root(root.path())?;
    write_package(root.path(), "affected", "@acme/affected")?;
    write_package(root.path(), "clean", "@acme/clean")?;

    write(
        root.path(),
        "packages/affected/tsconfig.json",
        r#"{"compilerOptions":{"rootDirs":["src","generated"]}}"#,
    )?;
    write(
        root.path(),
        "packages/affected/src/views/main.ts",
        concat!(
            "import { templateUsed } from './template';\n",
            "console.log(templateUsed);\n",
        ),
    )?;
    module(
        root.path(),
        "packages/affected/src/views/template.ts",
        "templateUsed",
        "ordinaryDead",
    )?;
    module(
        root.path(),
        "packages/affected/generated/views/template.ts",
        "templateUsed",
        "virtualRootDead",
    )?;
    module(
        root.path(),
        "packages/affected/unrelated.ts",
        "unrelatedOne",
        "unrelatedTwo",
    )?;

    write(
        root.path(),
        "packages/clean/main.ts",
        concat!(
            "import { controlUsed } from './value';\n",
            "console.log(controlUsed);\n",
        ),
    )?;
    module(
        root.path(),
        "packages/clean/value.ts",
        "controlUsed",
        "controlDead",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    assert_eq!(field(&audit.stdout, "status")?, "incomplete");
    let run_id = field(&audit.stdout, "runId")?;
    let overview = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    let limitations = overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("limitations are missing"))?;
    assert_eq!(limitations.len(), 1);
    assert_eq!(
        limitations[0].get("reason").and_then(Value::as_str),
        Some("tsconfig-semantics-unsupported")
    );
    assert_eq!(
        limitations[0].get("path").and_then(Value::as_str),
        Some("packages/affected/tsconfig.json")
    );
    assert_eq!(
        limitations[0].get("detail").and_then(Value::as_str),
        Some("unsupported resolution-affecting compiler option rootDirs")
    );
    assert_eq!(
        finding_set(root.path(), &run_id)?,
        BTreeSet::from([finding("packages/clean/value.ts", "controlDead")])
    );
    Ok(())
}

#[test]
fn root_dirs_prewrite_excludes_candidate_reads_and_retry_is_idempotent()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace_root(root.path())?;
    write_package(root.path(), "affected", "@acme/affected")?;
    module(
        root.path(),
        "packages/affected/src/views/template.ts",
        "templateUsed",
        "ordinaryDead",
    )?;
    module(
        root.path(),
        "packages/affected/generated/views/template.ts",
        "templateUsed",
        "virtualRootDead",
    )?;

    let writer = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-root-dirs-candidate-writer",
            "--path",
            "packages/affected/src/views/template.ts",
            "--path",
            "packages/affected/generated/views/template.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&writer, 0);
    let writer_gate = field(&writer.stdout, "gateId")?;

    write(
        root.path(),
        "packages/affected/tsconfig.json",
        r#"{"compilerOptions":{"rootDirs":["src","generated"]}}"#,
    )?;
    write(
        root.path(),
        "packages/affected/src/views/main.ts",
        concat!(
            "import { templateUsed } from './template';\n",
            "console.log(templateUsed);\n",
        ),
    )?;

    let rejected_gate = assert_incomplete_prewrite_retry(
        root.path(),
        "op-root-dirs-reader",
        "packages/affected/src/views/main.ts",
        &[],
    )?;
    assert_probe_candidates_excluded(&rejected_gate, 2)?;

    write_package(root.path(), "control", "@acme/control")?;
    write(
        root.path(),
        "packages/control/main.ts",
        concat!(
            "import { templateUsed } from '../affected/src/views/template';\n",
            "console.log(templateUsed);\n",
        ),
    )?;
    let control = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-root-dirs-probe-control",
            "--path",
            "packages/control/main.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&control, 4);
    assert_write_conflict(
        &control.stdout,
        "packages/affected/src/views/template.ts",
        &writer_gate,
    )?;
    Ok(())
}
