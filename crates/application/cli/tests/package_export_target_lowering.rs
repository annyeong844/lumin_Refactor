use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

mod support;

#[path = "support/gate.rs"]
mod gate;

use gate::{assert_incomplete_prewrite_retry, assert_probe_candidates_excluded};
use support::{assert_status, field, run};

#[test]
fn one_star_target_lowers_to_the_expected_package_source() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    write_workspace(root.path())?;
    write_json(
        root.path(),
        "packages/lib/package.json",
        &serde_json::json!({
            "name": "@acme/lib",
            "private": true,
            "exports": {"./feature/*": "./dist/*.js"},
        }),
    )?;
    write(
        root.path(),
        "packages/lib/dist/internal/x.ts",
        "export const lowered = 1;\n",
    )?;
    let main_source = concat!(
        "import { lowered } from '@acme/lib/feature/internal/x';\n",
        "console.log(lowered);\n",
    );
    write(root.path(), "src/main.ts", main_source)?;

    let run_id = audit(root.path(), "complete", 0)?;
    let source = file_response(root.path(), &run_id, "src/main.ts")?;
    let resolution = named_resolution(
        &source,
        "@acme/lib/feature/internal/x",
        expected_span(main_source, "lowered")?,
    )?;
    assert_eq!(required_str(resolution, "/outcome/kind")?, "internal");
    assert_eq!(
        required_str(resolution, "/outcome/target")?,
        source_id(root.path(), &run_id, "packages/lib/dist/internal/x.ts")?,
    );
    Ok(())
}

#[test]
fn invalid_target_strings_are_package_scoped_and_never_publish_candidates()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace(root.path())?;
    let invalid_targets = [
        "./dist%2Findex.js",
        "./dist%5Cindex.js",
        "./%2e/index.js",
        "./%6eode_modules/x.js",
        "./percent%25.js",
        "./bad%ZZ.js",
        "./index.js?mode=x",
        "./index.js#fragment",
        ".\\index.js",
        "./../escape.js",
    ];
    let mut main_source = String::new();
    for (index, target) in invalid_targets.iter().enumerate() {
        write_json(
            root.path(),
            &format!("packages/p{index}/package.json"),
            &serde_json::json!({
                "name": format!("@invalid/p{index}"),
                "private": true,
                "exports": target,
            }),
        )?;
        main_source.push_str(&format!(
            "import {{ value{index} }} from '@invalid/p{index}';\n"
        ));
    }
    write(root.path(), "src/main.ts", &main_source)?;

    let run_id = audit(root.path(), "incomplete", invalid_targets.len() as u64)?;
    let source = file_response(root.path(), &run_id, "src/main.ts")?;
    for index in 0..invalid_targets.len() {
        let specifier = format!("@invalid/p{index}");
        let binding = format!("value{index}");
        let resolution =
            named_resolution(&source, &specifier, expected_span(&main_source, &binding)?)?;
        assert_eq!(required_str(resolution, "/outcome/kind")?, "unsupported");
        assert!(resolution.pointer("/outcome/candidates").is_none());
        assert!(resolution.pointer("/outcome/target").is_none());
    }

    let overview = overview(root.path(), &run_id)?;
    let limitations = required_array(&overview, "/limitations")?;
    assert_eq!(limitations.len(), invalid_targets.len());
    let observed = limitations
        .iter()
        .map(|limitation| {
            assert_eq!(
                limitation.get("reason").and_then(Value::as_str),
                Some("public-surface-unsupported")
            );
            required_str(limitation, "/path")
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let expected = (0..invalid_targets.len())
        .map(|index| format!("packages/p{index}/package.json"))
        .collect::<BTreeSet<_>>();
    assert_eq!(observed, expected);
    Ok(())
}

#[test]
fn invalid_target_prewrite_excludes_the_candidate_and_retry_is_idempotent()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace(root.path())?;
    write_json(
        root.path(),
        "packages/lib/package.json",
        &serde_json::json!({
            "name": "@acme/lib",
            "private": true,
            "exports": "./target.js?mode=x",
        }),
    )?;
    write(
        root.path(),
        "packages/lib/target.ts",
        "export const bait = 1;\n",
    )?;
    write(
        root.path(),
        "src/main.ts",
        "import { bait } from '@acme/lib'; console.log(bait);\n",
    )?;

    let rejected = assert_incomplete_prewrite_retry(
        root.path(),
        "op-invalid-package-target",
        "src/main.ts",
        &[],
    )?;
    assert_probe_candidates_excluded(&rejected, 1)?;
    Ok(())
}

#[test]
fn physical_escape_is_unsupported_before_candidate_publication()
-> Result<(), Box<dyn std::error::Error>> {
    let sandbox = tempfile::tempdir()?;
    let root = sandbox.path().join("repo");
    let outside = sandbox.path().join("outside");
    fs::create_dir_all(&root)?;
    fs::create_dir_all(&outside)?;
    write_workspace(&root)?;
    write_json(
        &root,
        "packages/lib/package.json",
        &serde_json::json!({
            "name": "@acme/lib",
            "private": true,
            "exports": "./escape/index.js",
        }),
    )?;
    write(&outside, "index.ts", "export const escaped = 1;\n")?;
    let main_source = concat!(
        "import { escaped } from '@acme/lib';\n",
        "console.log(escaped);\n",
    );
    write(&root, "src/main.ts", main_source)?;
    let _redirect =
        DirectoryRedirect::create(root.join("packages").join("lib").join("escape"), &outside)?;

    let run_id = audit(&root, "incomplete", 1)?;
    let source = file_response(&root, &run_id, "src/main.ts")?;
    let resolution =
        named_resolution(&source, "@acme/lib", expected_span(main_source, "escaped")?)?;
    assert_eq!(required_str(resolution, "/outcome/kind")?, "unsupported");
    assert_eq!(
        required_str(resolution, "/outcome/reason")?,
        "package target physically escapes the repository root"
    );
    assert!(resolution.pointer("/outcome/candidates").is_none());

    let overview = overview(&root, &run_id)?;
    let limitations = required_array(&overview, "/limitations")?;
    assert_eq!(limitations.len(), 1);
    assert_eq!(
        limitations[0].get("reason").and_then(Value::as_str),
        Some("public-surface-unsupported")
    );
    assert_eq!(
        limitations[0].get("path").and_then(Value::as_str),
        Some("packages/lib/package.json")
    );

    assert_incomplete_prewrite_retry(
        &root,
        "op-physical-package-target-escape",
        "src/main.ts",
        &[],
    )?;
    Ok(())
}

#[test]
fn hard_excluded_descendant_redirect_is_rejected_without_traversal()
-> Result<(), Box<dyn std::error::Error>> {
    let sandbox = tempfile::tempdir()?;
    let root = sandbox.path().join("repo");
    let outside = sandbox.path().join("outside");
    fs::create_dir_all(&root)?;
    fs::create_dir_all(&outside)?;
    write_workspace(&root)?;
    write_json(
        &root,
        "packages/lib/package.json",
        &serde_json::json!({
            "name": "@acme/lib",
            "private": true,
            "exports": "./.git/escape/index.js",
        }),
    )?;
    write(&outside, "index.ts", "export const hidden = 1;\n")?;
    let main_source = "import { hidden } from '@acme/lib'; console.log(hidden);\n";
    write(&root, "src/main.ts", main_source)?;
    let _redirect = DirectoryRedirect::create(
        root.join("packages")
            .join("lib")
            .join(".git")
            .join("escape"),
        &outside,
    )?;

    let run_id = audit(&root, "incomplete", 1)?;
    let source = file_response(&root, &run_id, "src/main.ts")?;
    let resolution = named_resolution(&source, "@acme/lib", expected_span(main_source, "hidden")?)?;
    assert_eq!(required_str(resolution, "/outcome/kind")?, "unsupported");
    assert_eq!(
        required_str(resolution, "/outcome/reason")?,
        "package target enters a hard-excluded source namespace"
    );
    assert!(resolution.pointer("/outcome/candidates").is_none());
    Ok(())
}

#[test]
fn static_target_prefix_redirect_is_probed_without_sources()
-> Result<(), Box<dyn std::error::Error>> {
    let sandbox = tempfile::tempdir()?;
    let root = sandbox.path().join("repo");
    let outside = sandbox.path().join("outside");
    fs::create_dir_all(&root)?;
    fs::create_dir_all(&outside)?;
    write_workspace(&root)?;
    write_json(
        &root,
        "packages/lib/package.json",
        &serde_json::json!({
            "name": "@acme/lib",
            "exports": {"./x/*": "./escape/generated/*.js"},
        }),
    )?;
    write(&root, "src/main.ts", "console.log('main');\n")?;
    let _redirect =
        DirectoryRedirect::create(root.join("packages").join("lib").join("escape"), &outside)?;

    let run_id = audit(&root, "incomplete", 1)?;
    let overview = overview(&root, &run_id)?;
    let limitations = required_array(&overview, "/limitations")?;
    assert_eq!(limitations.len(), 1);
    assert_eq!(
        limitations[0].get("detail").and_then(Value::as_str),
        Some("package target physically escapes the repository root")
    );
    Ok(())
}

#[test]
fn public_pattern_probe_avoids_exact_and_more_specific_key_collisions()
-> Result<(), Box<dyn std::error::Error>> {
    for target in ["./escape/generated/*.js", "./escape/generated/index.js"] {
        let sandbox = tempfile::tempdir()?;
        let root = sandbox.path().join("repo");
        let outside = sandbox.path().join("outside");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(&outside)?;
        write_workspace(&root)?;
        write_json(
            &root,
            "packages/lib/package.json",
            &serde_json::json!({
                "name": "@acme/lib",
                "exports": {
                    "./features/lumin-pattern": null,
                    "./features/lumin-*": null,
                    "./features/*": target,
                },
            }),
        )?;
        write(&root, "src/main.ts", "console.log('main');\n")?;
        let _redirect =
            DirectoryRedirect::create(root.join("packages").join("lib").join("escape"), &outside)?;

        let run_id = audit(&root, "incomplete", 1)?;
        let limitations = required_array(&overview(&root, &run_id)?, "/limitations")?.to_owned();
        assert_eq!(
            limitations[0].get("detail").and_then(Value::as_str),
            Some("package target physically escapes the repository root"),
            "target: {target}"
        );
    }
    Ok(())
}

#[test]
fn hard_excluded_target_is_rejected_before_topology_pruning()
-> Result<(), Box<dyn std::error::Error>> {
    let sandbox = tempfile::tempdir()?;
    let root = sandbox.path().join("repo");
    let outside = sandbox.path().join("outside");
    fs::create_dir_all(&root)?;
    fs::create_dir_all(&outside)?;
    write_workspace(&root)?;
    write_json(
        &root,
        "packages/lib/package.json",
        &serde_json::json!({
            "name": "@acme/lib",
            "private": true,
            "exports": "./.git/index.js",
        }),
    )?;
    write(&outside, "index.ts", "export const hidden = 1;\n")?;
    let main_source = "import { hidden } from '@acme/lib'; console.log(hidden);\n";
    write(&root, "src/main.ts", main_source)?;
    let _redirect =
        DirectoryRedirect::create(root.join("packages").join("lib").join(".git"), &outside)?;

    let run_id = audit(&root, "incomplete", 1)?;
    let source = file_response(&root, &run_id, "src/main.ts")?;
    let resolution = named_resolution(&source, "@acme/lib", expected_span(main_source, "hidden")?)?;
    assert_eq!(required_str(resolution, "/outcome/kind")?, "unsupported");
    assert_eq!(
        required_str(resolution, "/outcome/reason")?,
        "package target enters a hard-excluded source namespace"
    );
    assert!(resolution.pointer("/outcome/candidates").is_none());
    Ok(())
}

#[test]
fn redirect_into_hard_excluded_namespace_is_rejected_after_lowering()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace(root.path())?;
    write_json(
        root.path(),
        "packages/lib/package.json",
        &serde_json::json!({
            "name": "@acme/lib",
            "private": true,
            "exports": "./alias/index.js",
        }),
    )?;
    write(
        root.path(),
        "packages/lib/.git/real/index.ts",
        "export const hidden = 1;\n",
    )?;
    let main_source = "import { hidden } from '@acme/lib'; console.log(hidden);\n";
    write(root.path(), "src/main.ts", main_source)?;
    let hidden_target = root
        .path()
        .join("packages")
        .join("lib")
        .join(".git")
        .join("real");
    let _redirect = DirectoryRedirect::create(
        root.path().join("packages").join("lib").join("alias"),
        &hidden_target,
    )?;

    let run_id = audit(root.path(), "incomplete", 1)?;
    let source = file_response(root.path(), &run_id, "src/main.ts")?;
    let resolution = named_resolution(&source, "@acme/lib", expected_span(main_source, "hidden")?)?;
    assert_eq!(required_str(resolution, "/outcome/kind")?, "unsupported");
    assert_eq!(
        required_str(resolution, "/outcome/reason")?,
        "package target enters a hard-excluded source namespace"
    );
    assert!(resolution.pointer("/outcome/candidates").is_none());
    Ok(())
}

#[test]
fn literal_target_escape_is_checked_before_extension_probe()
-> Result<(), Box<dyn std::error::Error>> {
    let sandbox = tempfile::tempdir()?;
    let root = sandbox.path().join("repo");
    let outside = sandbox.path().join("outside");
    fs::create_dir_all(&root)?;
    fs::create_dir_all(&outside)?;
    write_workspace(&root)?;
    write_json(
        &root,
        "packages/lib/package.json",
        &serde_json::json!({
            "name": "@acme/lib",
            "private": true,
            "exports": "./dist/index.js",
        }),
    )?;
    write(
        &root,
        "packages/lib/dist/index.ts",
        "export const selectedTooEarly = 1;\n",
    )?;
    let main_source = concat!(
        "import { selectedTooEarly } from '@acme/lib';\n",
        "console.log(selectedTooEarly);\n",
    );
    write(&root, "src/main.ts", main_source)?;
    let _redirect = DirectoryRedirect::create(
        root.join("packages")
            .join("lib")
            .join("dist")
            .join("index.js"),
        &outside,
    )?;

    let run_id = audit(&root, "incomplete", 1)?;
    let source = file_response(&root, &run_id, "src/main.ts")?;
    let resolution = named_resolution(
        &source,
        "@acme/lib",
        expected_span(main_source, "selectedTooEarly")?,
    )?;
    assert_eq!(required_str(resolution, "/outcome/kind")?, "unsupported");
    assert_eq!(
        required_str(resolution, "/outcome/reason")?,
        "package target physically escapes the repository root"
    );
    assert!(resolution.pointer("/outcome/target").is_none());
    Ok(())
}

#[cfg(unix)]
#[test]
fn file_redirect_is_not_treated_as_a_wildcard_directory() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    write_workspace(root.path())?;
    write_json(
        root.path(),
        "packages/lib/package.json",
        &serde_json::json!({
            "name": "@acme/lib",
            "exports": {"./features/*": "./dist/*.js"},
        }),
    )?;
    write(root.path(), "shared/readme.txt", "not a source\n")?;
    write(root.path(), "src/main.ts", "console.log('main');\n")?;
    let _redirect = FileRedirect::create(
        root.path().join("packages/lib/dist/readme.txt"),
        &root.path().join("shared/readme.txt"),
    )?;

    let run_id = audit(root.path(), "complete", 0)?;
    assert!(required_array(&overview(root.path(), &run_id)?, "/limitations")?.is_empty());
    Ok(())
}

#[test]
fn more_specific_null_pattern_prevents_a_general_escape_probe()
-> Result<(), Box<dyn std::error::Error>> {
    let sandbox = tempfile::tempdir()?;
    let root = sandbox.path().join("repo");
    let outside = sandbox.path().join("outside");
    fs::create_dir_all(&root)?;
    fs::create_dir_all(&outside)?;
    write_workspace(&root)?;
    write_json(
        &root,
        "packages/lib/package.json",
        &serde_json::json!({
            "name": "@acme/lib",
            "exports": {
                "./features/private/*": null,
                "./features/*": "./escape/*.js",
            },
        }),
    )?;
    write(&root, "src/main.ts", "console.log('main');\n")?;
    let _redirect = DirectoryRedirect::create(
        root.join("packages")
            .join("lib")
            .join("escape")
            .join("private"),
        &outside,
    )?;

    let run_id = audit(&root, "complete", 0)?;
    assert!(required_array(&overview(&root, &run_id)?, "/limitations")?.is_empty());
    Ok(())
}

#[test]
fn empty_wildcard_target_still_reports_its_physical_escape()
-> Result<(), Box<dyn std::error::Error>> {
    let sandbox = tempfile::tempdir()?;
    let root = sandbox.path().join("repo");
    let outside = sandbox.path().join("outside");
    fs::create_dir_all(&root)?;
    fs::create_dir_all(&outside)?;
    write_workspace(&root)?;
    write_json(
        &root,
        "packages/lib/package.json",
        &serde_json::json!({
            "name": "@acme/lib",
            "exports": {"./features/*": "./escape/*.js"},
        }),
    )?;
    write(&root, "src/main.ts", "console.log('main');\n")?;
    let _redirect =
        DirectoryRedirect::create(root.join("packages").join("lib").join("escape"), &outside)?;

    let run_id = audit(&root, "incomplete", 1)?;
    let overview = overview(&root, &run_id)?;
    let limitations = required_array(&overview, "/limitations")?;
    assert_eq!(limitations.len(), 1);
    assert_eq!(
        limitations[0].get("reason").and_then(Value::as_str),
        Some("public-surface-unsupported")
    );
    assert_eq!(
        limitations[0].get("path").and_then(Value::as_str),
        Some("packages/lib/package.json")
    );
    assert_eq!(
        limitations[0].get("detail").and_then(Value::as_str),
        Some("package target physically escapes the repository root")
    );
    Ok(())
}

#[test]
fn same_category_redirect_retargeting_invalidates_the_active_gate()
-> Result<(), Box<dyn std::error::Error>> {
    let sandbox = tempfile::tempdir()?;
    let root = sandbox.path().join("repo");
    let outside_before = sandbox.path().join("outside-before");
    let outside_after = sandbox.path().join("outside-after");
    fs::create_dir_all(&root)?;
    fs::create_dir_all(&outside_before)?;
    fs::create_dir_all(&outside_after)?;
    write_workspace(&root)?;
    write_json(
        &root,
        "packages/lib/package.json",
        &serde_json::json!({
            "name": "@acme/lib",
            "private": true,
            "exports": "./dist/index.js",
        }),
    )?;
    write(
        &root,
        "packages/lib/dist/index.ts",
        "export const value = 1;\n",
    )?;
    write(
        &root,
        "src/main.ts",
        "import { value } from '@acme/lib'; console.log(value);\n",
    )?;
    let mut redirect = DirectoryRedirect::create(
        root.join("packages").join("lib").join("escape"),
        &outside_before,
    )?;

    let open_arguments = [
        "pre-write",
        "--operation-id",
        "op-redirect-retarget-open",
        "--path",
        "src/main.ts",
        "--jobs",
        "1",
    ];
    let opened = run(&root, &open_arguments)?;
    assert_status(&opened, 0);
    assert_eq!(field(&opened.stdout, "decision")?, "allow");
    let gate_id = field(&opened.stdout, "gateId")?;

    redirect.retarget(&outside_after)?;

    let close_arguments = [
        "post-write",
        gate_id.as_str(),
        "--operation-id",
        "op-redirect-retarget-close",
    ];
    let closed = run(&root, &close_arguments)?;
    assert_status(&closed, 5);
    assert_eq!(field(&closed.stdout, "decision")?, "stale");
    assert_eq!(field(&closed.stdout, "lifecycle")?, "active");
    let closed_json: Value = serde_json::from_str(&closed.stdout)?;
    assert!(closed_json.get("actualWriteSet").is_none());

    let operation_before = run(&root, &["operation", "show", "op-redirect-retarget-close"])?;
    assert_status(&operation_before, 0);
    let gate_before = run(&root, &["gate", "show", &gate_id])?;
    assert_status(&gate_before, 0);

    let retry = run(&root, &close_arguments)?;
    assert_status(&retry, 5);
    assert_eq!(retry.stdout, closed.stdout);
    let operation_after = run(&root, &["operation", "show", "op-redirect-retarget-close"])?;
    assert_status(&operation_after, 0);
    assert_eq!(operation_after.stdout, operation_before.stdout);
    let gate_after = run(&root, &["gate", "show", &gate_id])?;
    assert_status(&gate_after, 0);
    assert_eq!(gate_after.stdout, gate_before.stdout);
    Ok(())
}

#[test]
fn same_target_redirect_replacement_invalidates_the_active_gate()
-> Result<(), Box<dyn std::error::Error>> {
    let sandbox = tempfile::tempdir()?;
    let root = sandbox.path().join("repo");
    let target = root.join("packages").join("lib").join("target");
    fs::create_dir_all(&target)?;
    write_workspace(&root)?;
    write_json(
        &root,
        "packages/lib/package.json",
        &serde_json::json!({"name": "@acme/lib", "private": true}),
    )?;
    write(&root, "src/main.ts", "console.log('main');\n")?;
    let redirect_path = root.join("packages").join("lib").join("link");
    let redirect = DirectoryRedirect::create(redirect_path.clone(), &target)?;

    let opened = run(
        &root,
        &[
            "pre-write",
            "--operation-id",
            "op-redirect-replace-open",
            "--path",
            "src/main.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    assert_eq!(field(&opened.stdout, "decision")?, "allow");
    let gate_id = field(&opened.stdout, "gateId")?;

    let retired_path = sandbox.path().join("retired-link");
    fs::rename(&redirect.path, &retired_path)?;
    create_directory_redirect(&redirect.path, &target)?;
    let _retired_redirect = DirectoryRedirect { path: retired_path };

    let close_arguments = [
        "post-write",
        gate_id.as_str(),
        "--operation-id",
        "op-redirect-replace-close",
    ];
    let closed = run(&root, &close_arguments)?;
    assert_status(&closed, 5);
    assert_eq!(field(&closed.stdout, "decision")?, "stale");
    assert_eq!(field(&closed.stdout, "lifecycle")?, "active");
    let operation_before = run(&root, &["operation", "show", "op-redirect-replace-close"])?;
    assert_status(&operation_before, 0);
    let gate_before = run(&root, &["gate", "show", &gate_id])?;
    assert_status(&gate_before, 0);

    let retry = run(&root, &close_arguments)?;
    assert_status(&retry, 5);
    assert_eq!(retry.stdout, closed.stdout);
    let operation_after = run(&root, &["operation", "show", "op-redirect-replace-close"])?;
    assert_status(&operation_after, 0);
    assert_eq!(operation_after.stdout, operation_before.stdout);
    let gate_after = run(&root, &["gate", "show", &gate_id])?;
    assert_status(&gate_after, 0);
    assert_eq!(gate_after.stdout, gate_before.stdout);
    Ok(())
}

#[test]
fn redirect_target_identity_blocks_a_physical_directory_writer()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace(root.path())?;
    write_json(
        root.path(),
        "packages/lib/package.json",
        &serde_json::json!({"name": "@acme/lib", "private": true}),
    )?;
    fs::create_dir_all(root.path().join("packages/lib/inside"))?;
    write(root.path(), "src/main.ts", "console.log('main');\n")?;
    let inside = root.path().join("packages").join("lib").join("inside");
    let _redirect = DirectoryRedirect::create(
        root.path().join("packages").join("lib").join("link"),
        &inside,
    )?;

    let reader = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-redirect-reader",
            "--path",
            "src/main.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&reader, 0);
    assert_eq!(field(&reader.stdout, "decision")?, "allow");
    let reader_gate = field(&reader.stdout, "gateId")?;

    let writer_arguments = [
        "pre-write",
        "--operation-id",
        "op-redirect-target-writer",
        "--path",
        "packages/lib/inside",
        "--jobs",
        "1",
    ];
    let writer = run(root.path(), &writer_arguments)?;
    assert_status(&writer, 4);
    assert_eq!(field(&writer.stdout, "decision")?, "incomplete");
    let response: Value = serde_json::from_str(&writer.stdout)?;
    let conflict = required_array(&response, "/signals")?
        .iter()
        .find(|signal| signal.get("kind").and_then(Value::as_str) == Some("write-conflict"))
        .ok_or_else(|| std::io::Error::other("physical redirect write conflict is missing"))?;
    assert_eq!(
        required_str(conflict, "/paths/0/display")?,
        "packages/lib/inside"
    );
    assert_eq!(required_str(conflict, "/gateIds/0")?, reader_gate);
    let writer_gate = field(&writer.stdout, "gateId")?;
    let operation_before = run(
        root.path(),
        &["operation", "show", "op-redirect-target-writer"],
    )?;
    assert_status(&operation_before, 0);
    let gate_before = run(root.path(), &["gate", "show", &writer_gate])?;
    assert_status(&gate_before, 0);

    let retry = run(root.path(), &writer_arguments)?;
    assert_status(&retry, 4);
    assert_eq!(retry.stdout, writer.stdout);
    let operation_after = run(
        root.path(),
        &["operation", "show", "op-redirect-target-writer"],
    )?;
    assert_status(&operation_after, 0);
    assert_eq!(operation_after.stdout, operation_before.stdout);
    let gate_after = run(root.path(), &["gate", "show", &writer_gate])?;
    assert_status(&gate_after, 0);
    assert_eq!(gate_after.stdout, gate_before.stdout);
    Ok(())
}

struct DirectoryRedirect {
    path: PathBuf,
}

#[cfg(unix)]
struct FileRedirect {
    path: PathBuf,
}

#[cfg(unix)]
impl FileRedirect {
    fn create(path: PathBuf, target: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        create_file_redirect(&path, target)?;
        Ok(Self { path })
    }
}

#[cfg(unix)]
impl Drop for FileRedirect {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl DirectoryRedirect {
    fn create(path: PathBuf, target: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        create_directory_redirect(&path, target)?;
        Ok(Self { path })
    }

    fn retarget(&mut self, target: &Path) -> Result<(), Box<dyn std::error::Error>> {
        remove_directory_redirect(&self.path)?;
        create_directory_redirect(&self.path, target)?;
        Ok(())
    }
}

impl Drop for DirectoryRedirect {
    fn drop(&mut self) {
        let _ = remove_directory_redirect(&self.path);
    }
}

#[cfg(unix)]
fn create_directory_redirect(path: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}

#[cfg(unix)]
fn create_file_redirect(path: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}

#[cfg(windows)]
fn create_directory_redirect(path: &Path, target: &Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(path)
        .arg(target)
        .status()?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| std::io::Error::other(format!("mklink /J exited with {status}")))
}

#[cfg(unix)]
fn remove_directory_redirect(path: &Path) -> std::io::Result<()> {
    fs::remove_file(path)
}

#[cfg(windows)]
fn remove_directory_redirect(path: &Path) -> std::io::Result<()> {
    fs::remove_dir(path)
}

fn write_workspace(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    write_json(
        root,
        "package.json",
        &serde_json::json!({
            "name": "app",
            "private": true,
            "workspaces": ["packages/*"],
        }),
    )
}

fn audit(
    root: &Path,
    expected_status: &str,
    expected_limitations: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    let output = run(root, &["audit", "--jobs", "1"])?;
    assert_status(&output, 0);
    assert_eq!(field(&output.stdout, "status")?, expected_status);
    let response: Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(
        response.get("limitationCount").and_then(Value::as_u64),
        Some(expected_limitations)
    );
    field(&output.stdout, "runId")
}

fn file_response(
    root: &Path,
    run_id: &str,
    path: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, &["files", "--run", run_id, path])?;
    assert_status(&output, 0);
    serde_json::from_str(&output.stdout).map_err(Into::into)
}

fn overview(root: &Path, run_id: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, &["overview", "--run", run_id])?;
    assert_status(&output, 0);
    serde_json::from_str(&output.stdout).map_err(Into::into)
}

fn source_id(root: &Path, run_id: &str, path: &str) -> Result<String, Box<dyn std::error::Error>> {
    required_str(
        &file_response(root, run_id, path)?,
        "/sourceContext/sourceId",
    )
    .map_err(Into::into)
}

fn named_resolution<'a>(
    source: &'a Value,
    specifier: &str,
    expected_span: (u64, u64),
) -> Result<&'a Value, std::io::Error> {
    required_array(source, "/resolutions")?
        .iter()
        .find(|resolution| {
            resolution
                .pointer("/sourceUse/specifier")
                .and_then(Value::as_str)
                == Some(specifier)
                && resolution
                    .pointer("/sourceUse/kind")
                    .and_then(Value::as_str)
                    == Some("named")
                && resolution
                    .pointer("/sourceUse/requestKind")
                    .and_then(Value::as_str)
                    == Some("static-import")
                && resolution
                    .pointer("/sourceUse/span/start")
                    .and_then(Value::as_u64)
                    == Some(expected_span.0)
                && resolution
                    .pointer("/sourceUse/span/end")
                    .and_then(Value::as_u64)
                    == Some(expected_span.1)
        })
        .ok_or_else(|| {
            std::io::Error::other(format!(
                "named static resolution for {specifier} at {expected_span:?} is missing"
            ))
        })
}

fn expected_span(source: &str, syntax: &str) -> Result<(u64, u64), std::io::Error> {
    let start = source
        .find(syntax)
        .ok_or_else(|| std::io::Error::other(format!("{syntax:?} is missing from fixture")))?;
    Ok((start as u64, (start + syntax.len()) as u64))
}

fn required_array<'a>(value: &'a Value, pointer: &str) -> Result<&'a Vec<Value>, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other(format!("missing array {pointer}")))
}

fn required_str(value: &Value, pointer: &str) -> Result<String, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("missing string {pointer}")))
}

fn write_json(
    root: &Path,
    relative: &str,
    value: &Value,
) -> Result<(), Box<dyn std::error::Error>> {
    write(root, relative, &serde_json::to_string(value)?).map_err(Into::into)
}

fn write(root: &Path, relative: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
