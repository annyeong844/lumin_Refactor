mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;
use support::{assert_status, field, run};

#[test]
fn dependency_intents_lease_each_nearest_manifest_and_lockfile()
-> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_fixture()?;

    let local_arguments =
        dependency_prewrite("nearest-local-open", "packages/local/src/main.ts", "zod");
    let local = run(root.path(), &local_arguments)?;
    assert_status(&local, 0);
    assert_eq!(
        leased_paths(&local.stdout)?,
        BTreeSet::from([
            "packages/local/package-lock.json".to_owned(),
            "packages/local/package.json".to_owned(),
            "packages/local/src/main.ts".to_owned(),
        ])
    );
    let local_gate = field(&local.stdout, "gateId")?;
    let local_input_id = analysis_input_id(root.path(), &local_gate)?;

    let retry = run(root.path(), &local_arguments)?;
    assert_status(&retry, 0);
    assert_eq!(retry.stdout, local.stdout, "same-operation retry drifted");

    let conflicting_reuse = run(
        root.path(),
        &dependency_prewrite("nearest-local-open", "packages/local/src/main.ts", "react"),
    )?;
    assert_status(&conflicting_reuse, 2);

    let candidate_conflict = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "nearest-local-candidate-conflict",
            "--path",
            "src/other.ts",
            "--dependency-at",
            "packages/local/src/main.ts",
            "react",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&candidate_conflict, 4);
    assert_signal(&candidate_conflict.stdout, "semantic-input-conflict")?;
    abandon(root.path(), &local_gate, "nearest-local-abandon")?;

    let local_control = run(
        root.path(),
        &dependency_prewrite(
            "nearest-local-input-control",
            "packages/local/src/main.ts",
            "react",
        ),
    )?;
    assert_status(&local_control, 0);
    let local_control_gate = field(&local_control.stdout, "gateId")?;
    assert_ne!(
        analysis_input_id(root.path(), &local_control_gate)?,
        local_input_id,
        "the dependency name was omitted from the sealed AnalysisInputId",
    );
    abandon(
        root.path(),
        &local_control_gate,
        "nearest-local-control-abandon",
    )?;

    let self_write = run(
        root.path(),
        &dependency_prewrite(
            "nearest-local-self-write-open",
            "packages/local/src/main.ts",
            "zod",
        ),
    )?;
    assert_status(&self_write, 0);
    let self_write_gate = field(&self_write.stdout, "gateId")?;
    write(
        root.path(),
        "packages/local/package.json",
        r#"{"name":"@acme/local","private":true,"dependencies":{"zod":"^4.0.0"}}"#,
    )?;
    write(
        root.path(),
        "packages/local/package-lock.json",
        r#"{"lockfileVersion":3}"#,
    )?;
    let closed = run(
        root.path(),
        &[
            "post-write",
            &self_write_gate,
            "--operation-id",
            "nearest-local-self-write-close",
        ],
    )?;
    assert_status(&closed, 4);
    assert_eq!(field(&closed.stdout, "decision")?, "incomplete");
    assert_signal(&closed.stdout, "lifecycle-delta-incomparable")?;
    assert_delta(
        &closed.stdout,
        "dependency-ownership",
        "changed-incomparable",
    )?;
    assert_eq!(
        actual_write_paths(&closed.stdout)?,
        BTreeSet::from([
            "packages/local/package-lock.json".to_owned(),
            "packages/local/package.json".to_owned(),
        ]),
        "the inferred manifest and lockfile changes must be attributed to this gate",
    );
    abandon(
        root.path(),
        &self_write_gate,
        "nearest-local-self-write-abandon",
    )?;

    let combined = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "nearest-combined-open",
            "--path",
            "packages/local/src/main.ts",
            "--path",
            "packages/inherited/src/main.ts",
            "--dependency-at",
            "packages/local/src/main.ts",
            "zod",
            "--dependency-at",
            "packages/inherited/src/main.ts",
            "serde",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&combined, 0);
    assert_eq!(
        leased_paths(&combined.stdout)?,
        BTreeSet::from([
            "packages/inherited/package.json".to_owned(),
            "packages/inherited/src/main.ts".to_owned(),
            "packages/local/package-lock.json".to_owned(),
            "packages/local/package.json".to_owned(),
            "packages/local/src/main.ts".to_owned(),
            "pnpm-lock.yaml".to_owned(),
        ]),
        "one request must resolve each dependency intent in its own package context",
    );
    let combined_gate = field(&combined.stdout, "gateId")?;
    abandon(root.path(), &combined_gate, "nearest-combined-abandon")?;

    let directory_context = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "nearest-directory-context-open",
            "--path",
            "packages/local",
            "--dependency-at",
            "packages/local",
            "zod",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&directory_context, 0);
    assert_eq!(
        leased_paths(&directory_context.stdout)?,
        BTreeSet::from([
            "packages/local".to_owned(),
            "packages/local/package-lock.json".to_owned(),
            "packages/local/package.json".to_owned(),
            "packages/local/src/main.ts".to_owned(),
        ]),
        "a directory package context must inspect its own manifest before its parent",
    );
    let directory_gate = field(&directory_context.stdout, "gateId")?;
    abandon(
        root.path(),
        &directory_gate,
        "nearest-directory-context-abandon",
    )?;

    let replaced_source = standalone_fixture()?;
    let opened = run(
        replaced_source.path(),
        &[
            "pre-write",
            "--operation-id",
            "nearest-source-replacement-open",
            "--path",
            "src/main.ts",
            "--dependency-at",
            "src/main.ts",
            "zod",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;
    fs::remove_file(replaced_source.path().join("src/main.ts"))?;
    write(
        replaced_source.path(),
        "src/main.ts",
        "console.log('atomically replaced');\n",
    )?;
    let closed = run(
        replaced_source.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "nearest-source-replacement-close",
        ],
    )?;
    assert_status(&closed, 0);
    assert_eq!(field(&closed.stdout, "decision")?, "allow");

    let inherited = run(
        root.path(),
        &dependency_prewrite(
            "nearest-inherited-open",
            "packages/inherited/src/main.ts",
            "serde",
        ),
    )?;
    assert_status(&inherited, 0);
    assert_eq!(
        leased_paths(&inherited.stdout)?,
        BTreeSet::from([
            "packages/inherited/package.json".to_owned(),
            "packages/inherited/src/main.ts".to_owned(),
            "pnpm-lock.yaml".to_owned(),
        ]),
        "the package must inherit only the nearest workspace lockfile",
    );
    let inherited_gate = field(&inherited.stdout, "gateId")?;

    write(
        root.path(),
        "package.json",
        r#"{"name":"root","private":true,"workspaces":["packages/*"],"description":"external drift"}"#,
    )?;
    let close = run(
        root.path(),
        &[
            "post-write",
            &inherited_gate,
            "--operation-id",
            "nearest-inherited-close",
        ],
    )?;
    assert_status(&close, 5);
    assert_eq!(field(&close.stdout, "decision")?, "stale");
    assert_signal(&close.stdout, "protected-input-changed")?;
    abandon(root.path(), &inherited_gate, "nearest-inherited-abandon")?;

    let owner_change = standalone_fixture()?;
    write(
        owner_change.path(),
        "package.json",
        r#"{"name":"standalone","private":true,"workspaces":["app"]}"#,
    )?;
    write(
        owner_change.path(),
        "app/src/main.ts",
        "console.log('nested app');\n",
    )?;
    let opened = run(
        owner_change.path(),
        &[
            "pre-write",
            "--operation-id",
            "nearest-owner-change-open",
            "--path",
            "app",
            "--dependency-at",
            "app/src/main.ts",
            "zod",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;
    write(
        owner_change.path(),
        "app/package.json",
        r#"{"name":"@acme/app","private":true}"#,
    )?;
    let closed = run(
        owner_change.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "nearest-owner-change-close",
        ],
    )?;
    assert_status(&closed, 4);
    assert_eq!(field(&closed.stdout, "decision")?, "incomplete");
    assert_signal(&closed.stdout, "lifecycle-delta-incomparable")?;
    assert_delta(
        &closed.stdout,
        "dependency-ownership",
        "changed-incomparable",
    )?;
    abandon(
        owner_change.path(),
        &gate_id,
        "nearest-owner-change-abandon",
    )?;
    Ok(())
}

#[test]
fn dependency_owner_gaps_are_scoped_to_the_requested_package()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    )?;
    write(root.path(), "pnpm-lock.yaml", "lockfileVersion: '9.0'\n")?;
    write(
        root.path(),
        "packages/a/package.json",
        r#"{"name":"@acme/a","private":true}"#,
    )?;
    write(root.path(), "packages/a/src/main.ts", "console.log('a');\n")?;
    write(
        root.path(),
        "packages/b/package.json",
        r#"{"name":"@acme/b","private":true,"dependencies":[]}"#,
    )?;
    write(root.path(), "packages/b/src/main.ts", "console.log('b');\n")?;

    let package_a = run(
        root.path(),
        &dependency_prewrite("nearest-scoped-gap-a", "packages/a/src/main.ts", "zod"),
    )?;
    assert_status(&package_a, 0);
    assert_eq!(field(&package_a.stdout, "decision")?, "allow");
    let gate_id = field(&package_a.stdout, "gateId")?;
    let package_a_closed = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "nearest-scoped-gap-a-close",
        ],
    )?;
    assert_status(&package_a_closed, 0);
    assert_eq!(field(&package_a_closed.stdout, "decision")?, "allow");

    let ordinary_a = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "nearest-scoped-ordinary-a",
            "--path",
            "packages/a/src/main.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&ordinary_a, 0);
    assert_eq!(field(&ordinary_a.stdout, "decision")?, "allow");
    let ordinary_a_gate = field(&ordinary_a.stdout, "gateId")?;
    abandon(
        root.path(),
        &ordinary_a_gate,
        "nearest-scoped-ordinary-a-abandon",
    )?;

    let ordinary_b = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "nearest-scoped-ordinary-b",
            "--path",
            "packages/b/src/main.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&ordinary_b, 4);
    assert_eq!(field(&ordinary_b.stdout, "decision")?, "incomplete");
    assert_signal(&ordinary_b.stdout, "required-evidence-incomplete")?;

    let package_b = run(
        root.path(),
        &dependency_prewrite("nearest-scoped-gap-b", "packages/b/src/main.ts", "zod"),
    )?;
    assert_status(&package_b, 4);
    assert_eq!(field(&package_b.stdout, "decision")?, "incomplete");
    assert_signal(&package_b.stdout, "required-evidence-incomplete")?;
    Ok(())
}

#[test]
fn dependency_owner_uncertainty_never_infers_a_lockfile() -> Result<(), Box<dyn std::error::Error>>
{
    let no_lock = standalone_fixture()?;
    let opened = run(
        no_lock.path(),
        &dependency_prewrite("nearest-no-lock-open", "src/main.ts", "zod"),
    )?;
    assert_status(&opened, 0);
    assert_eq!(
        leased_paths(&opened.stdout)?,
        BTreeSet::from(["package.json".to_owned(), "src/main.ts".to_owned()]),
        "an absent lockfile must not create an inferred write",
    );
    let gate_id = field(&opened.stdout, "gateId")?;
    write(
        no_lock.path(),
        "yarn.lock",
        "# external lockfile creation\n",
    )?;
    let close = run(
        no_lock.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "nearest-no-lock-close",
        ],
    )?;
    assert_status(&close, 5);
    assert_eq!(field(&close.stdout, "decision")?, "stale");
    assert_signal(&close.stdout, "protected-input-changed")?;
    abandon(no_lock.path(), &gate_id, "nearest-no-lock-abandon")?;

    let new_source = standalone_fixture()?;
    let opened = run(
        new_source.path(),
        &dependency_prewrite("nearest-new-source-open", "generated/deep/main.ts", "zod"),
    )?;
    assert_status(&opened, 0);
    assert_eq!(
        leased_paths(&opened.stdout)?,
        BTreeSet::from([
            "generated/deep/main.ts".to_owned(),
            "package.json".to_owned(),
        ]),
    );
    let gate_id = field(&opened.stdout, "gateId")?;
    write(
        new_source.path(),
        "generated/deep/main.ts",
        "console.log('new source');\n",
    )?;
    let closed = run(
        new_source.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "nearest-new-source-close",
        ],
    )?;
    assert_status(&closed, 0);
    assert_eq!(field(&closed.stdout, "decision")?, "allow");
    assert_eq!(
        actual_write_paths(&closed.stdout)?,
        BTreeSet::from(["generated/deep/main.ts".to_owned()]),
    );

    let directory_scope = standalone_fixture()?;
    let opened = run(
        directory_scope.path(),
        &[
            "pre-write",
            "--operation-id",
            "nearest-directory-parent-open",
            "--path",
            "src",
            "--dependency-at",
            "src/generated/main.ts",
            "zod",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;
    write(
        directory_scope.path(),
        "src/generated/main.ts",
        "console.log('directory-scoped source');\n",
    )?;
    let closed = run(
        directory_scope.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "nearest-directory-parent-close",
        ],
    )?;
    assert_status(&closed, 0);
    assert_eq!(field(&closed.stdout, "decision")?, "allow");
    assert_eq!(
        actual_write_paths(&closed.stdout)?,
        BTreeSet::from(["src/generated/main.ts".to_owned()]),
        "directory ownership must not attribute still-missing manifest candidates",
    );

    let unexplained_parent = standalone_fixture()?;
    let opened = run(
        unexplained_parent.path(),
        &dependency_prewrite(
            "nearest-unexplained-parent-open",
            "generated/deep/main.ts",
            "zod",
        ),
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;
    fs::create_dir(unexplained_parent.path().join("generated"))?;
    let closed = run(
        unexplained_parent.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "nearest-unexplained-parent-close",
        ],
    )?;
    assert_status(&closed, 5);
    assert_eq!(field(&closed.stdout, "decision")?, "stale");
    assert_signal(&closed.stdout, "protected-input-changed")?;
    abandon(
        unexplained_parent.path(),
        &gate_id,
        "nearest-unexplained-parent-abandon",
    )?;

    let ambiguous = standalone_fixture()?;
    write(ambiguous.path(), "package-lock.json", "{}\n")?;
    write(
        ambiguous.path(),
        "pnpm-lock.yaml",
        "lockfileVersion: '9.0'\n",
    )?;
    let rejected_arguments = dependency_prewrite("nearest-ambiguous-open", "src/main.ts", "zod");
    let rejected = run(ambiguous.path(), &rejected_arguments)?;
    assert_status(&rejected, 4);
    assert_eq!(field(&rejected.stdout, "decision")?, "incomplete");
    assert_signal(&rejected.stdout, "required-evidence-incomplete")?;
    assert_eq!(
        leased_paths(&rejected.stdout)?,
        BTreeSet::new(),
        "ambiguous ownership must reject without promoting a durable lease",
    );
    let retry = run(ambiguous.path(), &rejected_arguments)?;
    assert_status(&retry, 4);
    assert_eq!(retry.stdout, rejected.stdout);

    let unavailable_workspace = tempfile::tempdir()?;
    write(
        unavailable_workspace.path(),
        "package.json",
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    )?;
    fs::create_dir_all(unavailable_workspace.path().join("pnpm-workspace.yaml"))?;
    write(
        unavailable_workspace.path(),
        "pnpm-lock.yaml",
        "lockfileVersion: '9.0'\n",
    )?;
    write(
        unavailable_workspace.path(),
        "packages/app/package.json",
        r#"{"name":"@acme/app","private":true}"#,
    )?;
    write(
        unavailable_workspace.path(),
        "packages/app/src/main.ts",
        "console.log('app');\n",
    )?;
    let rejected = run(
        unavailable_workspace.path(),
        &dependency_prewrite(
            "nearest-unavailable-workspace",
            "packages/app/src/main.ts",
            "zod",
        ),
    )?;
    assert_status(&rejected, 4);
    assert_eq!(field(&rejected.stdout, "decision")?, "incomplete");
    assert_signal(&rejected.stdout, "required-evidence-incomplete")?;
    assert_eq!(
        leased_paths(&rejected.stdout)?,
        BTreeSet::new(),
        "an unobservable pnpm workspace must reject without promoting a durable lease",
    );

    let hard_excluded = standalone_fixture()?;
    write(
        hard_excluded.path(),
        "node_modules/pkg/package.json",
        r#"{"name":"pkg","private":true}"#,
    )?;
    let rejected = run(
        hard_excluded.path(),
        &[
            "pre-write",
            "--operation-id",
            "nearest-hard-excluded-context",
            "--path",
            "src/main.ts",
            "--dependency-at",
            "node_modules/pkg",
            "zod",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&rejected, 4);
    assert_eq!(field(&rejected.stdout, "decision")?, "incomplete");
    assert_signal(&rejected.stdout, "required-evidence-incomplete")?;
    assert_eq!(
        leased_paths(&rejected.stdout)?,
        BTreeSet::new(),
        "hard-excluded dependency contexts must reject without promoting a durable lease",
    );

    let aliased_hard_excluded = standalone_fixture()?;
    fs::create_dir(aliased_hard_excluded.path().join(".git"))?;
    create_directory_alias(
        &aliased_hard_excluded.path().join(".git"),
        &aliased_hard_excluded.path().join("context"),
    )?;
    let rejected = run(
        aliased_hard_excluded.path(),
        &[
            "pre-write",
            "--operation-id",
            "nearest-aliased-hard-excluded-context",
            "--path",
            "src/main.ts",
            "--dependency-at",
            "context",
            "zod",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&rejected, 4);
    assert_eq!(field(&rejected.stdout, "decision")?, "incomplete");
    assert_signal(&rejected.stdout, "required-evidence-incomplete")?;
    assert_eq!(
        leased_paths(&rejected.stdout)?,
        BTreeSet::new(),
        "a hard-excluded physical alias must reject without promoting a durable lease",
    );
    remove_directory_alias(&aliased_hard_excluded.path().join("context"))?;

    let aliased_reserved_state = standalone_fixture()?;
    fs::create_dir(aliased_reserved_state.path().join(".lumin"))?;
    create_directory_alias(
        &aliased_reserved_state.path().join(".lumin"),
        &aliased_reserved_state.path().join("state-context"),
    )?;
    let rejected = run(
        aliased_reserved_state.path(),
        &[
            "pre-write",
            "--operation-id",
            "nearest-aliased-reserved-state",
            "--path",
            "src/main.ts",
            "--dependency-at",
            "state-context",
            "zod",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&rejected, 2);
    assert!(
        fs::read_dir(aliased_reserved_state.path().join(".lumin"))?
            .next()
            .is_none(),
        "reserved-state alias validation mutated lifecycle storage",
    );
    remove_directory_alias(&aliased_reserved_state.path().join("state-context"))?;

    #[cfg(windows)]
    {
        let rejected = run(
            aliased_reserved_state.path(),
            &[
                "pre-write",
                "--operation-id",
                "nearest-case-aliased-reserved-state",
                "--path",
                "src/main.ts",
                "--dependency-at",
                ".LUMIN",
                "zod",
                "--jobs",
                "1",
            ],
        )?;
        assert_status(&rejected, 2);
        assert!(
            fs::read_dir(aliased_reserved_state.path().join(".lumin"))?
                .next()
                .is_none(),
            "case-alias validation mutated lifecycle storage",
        );
    }

    let missing_manifest = tempfile::tempdir()?;
    fs::create_dir_all(missing_manifest.path().join("src"))?;
    write(
        missing_manifest.path(),
        "src/main.ts",
        "console.log('missing owner');\n",
    )?;
    let rejected = run(
        missing_manifest.path(),
        &dependency_prewrite("nearest-missing-owner", "src/main.ts", "zod"),
    )?;
    assert_status(&rejected, 4);
    assert_eq!(field(&rejected.stdout, "decision")?, "incomplete");
    assert_signal(&rejected.stdout, "required-evidence-incomplete")?;
    assert_eq!(
        leased_paths(&rejected.stdout)?,
        BTreeSet::new(),
        "a missing owner must reject without promoting a durable lease",
    );

    let redirected_context = standalone_fixture()?;
    fs::create_dir(redirected_context.path().join("context"))?;
    let opened = run(
        redirected_context.path(),
        &[
            "pre-write",
            "--operation-id",
            "nearest-redirected-context-open",
            "--path",
            "src/main.ts",
            "--dependency-at",
            "context",
            "zod",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;
    fs::remove_dir(redirected_context.path().join("context"))?;
    let outside = tempfile::tempdir()?;
    create_directory_alias(outside.path(), &redirected_context.path().join("context"))?;
    let closed = run(
        redirected_context.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "nearest-redirected-context-close",
        ],
    )?;
    assert_status(&closed, 4);
    assert_eq!(field(&closed.stdout, "decision")?, "incomplete");
    assert_signal(&closed.stdout, "analysis-failed")?;
    remove_directory_alias(&redirected_context.path().join("context"))?;
    abandon(
        redirected_context.path(),
        &gate_id,
        "nearest-redirected-context-abandon",
    )?;
    Ok(())
}

fn workspace_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    )?;
    write(root.path(), "pnpm-lock.yaml", "lockfileVersion: '9.0'\n")?;
    write(root.path(), "src/other.ts", "console.log('other');\n")?;
    write(
        root.path(),
        "packages/local/package.json",
        r#"{"name":"@acme/local","private":true}"#,
    )?;
    write(root.path(), "packages/local/package-lock.json", "{}\n")?;
    write(
        root.path(),
        "packages/local/src/main.ts",
        "console.log('local');\n",
    )?;
    write(
        root.path(),
        "packages/inherited/package.json",
        r#"{"name":"@acme/inherited","private":true}"#,
    )?;
    write(
        root.path(),
        "packages/inherited/src/main.ts",
        "console.log('inherited');\n",
    )?;
    Ok(root)
}

fn standalone_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"standalone","private":true}"#,
    )?;
    write(root.path(), "src/main.ts", "console.log('standalone');\n")?;
    Ok(root)
}

fn dependency_prewrite<'a>(
    operation_id: &'a str,
    path: &'a str,
    dependency: &'a str,
) -> Vec<&'a str> {
    vec![
        "pre-write",
        "--operation-id",
        operation_id,
        "--path",
        path,
        "--dependency-at",
        path,
        dependency,
        "--jobs",
        "1",
    ]
}

fn leased_paths(json: &str) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let response: Value = serde_json::from_str(json)?;
    response
        .get("leasedWriteSet")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("leasedWriteSet is missing"))?
        .iter()
        .map(|lease| {
            lease
                .pointer("/path/display")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other("lease path display is missing").into())
        })
        .collect()
}

fn actual_write_paths(json: &str) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let response: Value = serde_json::from_str(json)?;
    response
        .pointer("/actualWriteSet/paths")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("actualWriteSet.paths is missing"))?
        .iter()
        .map(|path| {
            path.get("display")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other("actual write path display is missing").into())
        })
        .collect()
}

fn assert_signal(json: &str, expected: &str) -> Result<(), Box<dyn std::error::Error>> {
    let response: Value = serde_json::from_str(json)?;
    assert!(
        response
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals
                .iter()
                .any(|signal| { signal.get("kind").and_then(Value::as_str) == Some(expected) })),
        "missing {expected} signal: {response:#?}",
    );
    Ok(())
}

fn assert_delta(
    json: &str,
    expected_family: &str,
    expected_classification: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let response: Value = serde_json::from_str(json)?;
    let deltas = response
        .get("deltas")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("deltas are missing"))?;
    assert_eq!(deltas.len(), 1, "unexpected delta set: {deltas:#?}");
    assert_eq!(
        deltas[0].pointer("/key/family").and_then(Value::as_str),
        Some(expected_family),
    );
    assert_eq!(
        deltas[0]
            .pointer("/classification/kind")
            .and_then(Value::as_str),
        Some(expected_classification),
    );
    Ok(())
}

fn analysis_input_id(root: &Path, gate_id: &str) -> Result<String, Box<dyn std::error::Error>> {
    let shown = run(root, &["gate", "show", gate_id])?;
    assert_status(&shown, 0);
    let response: Value = serde_json::from_str(&shown.stdout)?;
    response
        .pointer("/baseline/analysisInputId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("baseline AnalysisInputId is missing").into())
}

fn abandon(
    root: &Path,
    gate_id: &str,
    operation_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = run(
        root,
        &[
            "gate",
            "abandon",
            gate_id,
            "--operation-id",
            operation_id,
            "--reason",
            "nearest-manifest corpus scenario complete",
        ],
    )?;
    assert_status(&result, 3);
    Ok(())
}

fn write(root: &Path, relative: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}

#[cfg(unix)]
fn create_directory_alias(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(unix)]
fn remove_directory_alias(link: &Path) -> std::io::Result<()> {
    fs::remove_file(link)
}

#[cfg(windows)]
fn create_directory_alias(target: &Path, link: &Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "mklink /J failed with {status}"
        )))
    }
}

#[cfg(windows)]
fn remove_directory_alias(link: &Path) -> std::io::Result<()> {
    fs::remove_dir(link)
}
