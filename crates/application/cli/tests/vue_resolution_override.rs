use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

const INLINE_APP: &str = concat!(
    "<script setup lang=\"ts\">\n",
    "import InlineWidget from './InlineWidget';\n",
    "</script>\n",
    "<template><InlineWidget /></template>\n",
);
const EXTERNAL_SCRIPT: &str = "import ExternalWidget from './ExternalWidget';\n";

#[derive(Clone, Copy)]
struct ScriptRequest<'a> {
    path: &'a str,
    specifier: &'a str,
    local_name: &'a str,
    source: &'a str,
    target: &'a str,
}

const SCRIPT_REQUESTS: [ScriptRequest<'static>; 2] = [
    ScriptRequest {
        path: "src/InlineApp.vue",
        specifier: "./InlineWidget",
        local_name: "InlineWidget",
        source: INLINE_APP,
        target: "src/InlineWidget.ts",
    },
    ScriptRequest {
        path: "src/external.ts",
        specifier: "./ExternalWidget",
        local_name: "ExternalWidget",
        source: EXTERNAL_SCRIPT,
        target: "src/ExternalWidget.ts",
    },
];

#[test]
fn vue_embedded_scripts_follow_invocation_extension_rules_without_a_template_lane()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;

    for (argument, serialized) in [("node16", "node16"), ("nodenext", "node-next")] {
        let audit = run(
            root.path(),
            &["audit", "--jobs", "1", "--resolution-profile", argument],
        )?;
        assert_status(&audit, 0);
        let summary: Value = serde_json::from_str(&audit.stdout)?;
        assert_eq!(
            summary.get("status").and_then(Value::as_str),
            Some("incomplete")
        );
        assert_eq!(
            summary.get("limitationCount").and_then(Value::as_u64),
            Some(2)
        );
        let run_id = field(&audit.stdout, "runId")?;

        assert_only_extension_limitations(root.path(), &run_id)?;
        for request in SCRIPT_REQUESTS {
            assert_script_request(
                root.path(),
                &run_id,
                request,
                serialized,
                ExpectedOutcome::Unsupported {
                    reason: &format!(
                        "{argument} import-mode resolution requires an explicit relative extension"
                    ),
                },
            )?;
        }
        assert_external_sfc_has_no_resolver_lane(root.path(), &run_id, serialized)?;
    }

    let audit = run(
        root.path(),
        &["audit", "--jobs", "1", "--resolution-profile", "bundler"],
    )?;
    assert_status(&audit, 0);
    let summary: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        summary.get("status").and_then(Value::as_str),
        Some("complete")
    );
    assert_eq!(
        summary.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    let run_id = field(&audit.stdout, "runId")?;
    for request in SCRIPT_REQUESTS {
        assert_script_request(
            root.path(),
            &run_id,
            request,
            "bundler",
            ExpectedOutcome::Internal,
        )?;
    }
    assert_external_sfc_has_no_resolver_lane(root.path(), &run_id, "bundler")?;
    assert_empty_findings(root.path(), &run_id)?;
    Ok(())
}

enum ExpectedOutcome<'a> {
    Internal,
    Unsupported { reason: &'a str },
}

fn assert_script_request(
    root: &Path,
    run_id: &str,
    request: ScriptRequest<'_>,
    profile: &str,
    expected: ExpectedOutcome<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = file_response(root, run_id, request.path)?;
    assert_invocation_profile(&source, profile, request.path)?;
    let resolutions = required_array(&source, "/resolutions")?;
    assert_eq!(
        resolutions.len(),
        1,
        "template binding must not add a second resolver lane for {}: {source:#?}",
        request.path,
    );
    let resolution = &resolutions[0];
    assert_eq!(
        required_str(resolution, "/sourceUse/importer")?,
        required_str(&source, "/sourceContext/sourceId")?,
    );
    assert_eq!(
        required_str(resolution, "/sourceUse/specifier")?,
        request.specifier
    );
    assert_eq!(
        required_str(resolution, "/sourceUse/importedName")?,
        "default"
    );
    assert_eq!(
        required_str(resolution, "/sourceUse/localName")?,
        request.local_name
    );
    assert_eq!(required_str(resolution, "/sourceUse/namespace")?, "value");
    assert_eq!(required_str(resolution, "/sourceUse/kind")?, "default");
    assert_eq!(
        required_str(resolution, "/sourceUse/requestKind")?,
        "static-import"
    );
    let expected_span = import_binding_span(request.source, request.local_name)?;
    assert_eq!(
        required_u64(resolution, "/sourceUse/span/start")?,
        expected_span.0
    );
    assert_eq!(
        required_u64(resolution, "/sourceUse/span/end")?,
        expected_span.1
    );

    match expected {
        ExpectedOutcome::Internal => {
            assert_eq!(required_str(resolution, "/outcome/kind")?, "internal");
            assert_eq!(
                required_str(resolution, "/outcome/target")?,
                source_id(root, run_id, request.target)?,
            );
        }
        ExpectedOutcome::Unsupported { reason } => {
            assert_eq!(required_str(resolution, "/outcome/kind")?, "unsupported");
            assert_eq!(required_str(resolution, "/outcome/reason")?, reason);
            assert!(resolution.pointer("/outcome/target").is_none());
        }
    }
    Ok(())
}

fn assert_external_sfc_has_no_resolver_lane(
    root: &Path,
    run_id: &str,
    profile: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = file_response(root, run_id, "src/ExternalApp.vue")?;
    assert_invocation_profile(&source, profile, "src/ExternalApp.vue")?;
    assert!(
        source
            .get("resolutions")
            .is_none_or(|resolutions| resolutions.as_array().is_some_and(Vec::is_empty)),
        "<script src> must attach the physical script binding without resolving it again: {source:#?}",
    );
    Ok(())
}

fn assert_invocation_profile(
    source: &Value,
    profile: &str,
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        required_str(source, "/resolutionProfile/profile")?,
        profile,
        "wrong profile for {path}",
    );
    assert_eq!(
        required_str(source, "/resolutionProfile/source/kind")?,
        "invocation",
        "wrong profile source for {path}",
    );
    Ok(())
}

fn assert_only_extension_limitations(
    root: &Path,
    run_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let overview = run(root, &["overview", "--run", run_id])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    let limitations = required_array(&overview, "/limitations")?;
    assert_eq!(limitations.len(), 2);
    assert!(limitations.iter().all(|limitation| {
        limitation.get("reason").and_then(Value::as_str) == Some("alias-shape-unsupported")
    }));
    assert!(limitations.iter().all(|limitation| {
        limitation.get("reason").and_then(Value::as_str) != Some("vue-template-opaque")
    }));
    Ok(())
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true,"type":"module"}"#,
    )?;
    write(
        root.path(),
        "tsconfig.json",
        r#"{"compilerOptions":{"moduleResolution":"node16"}}"#,
    )?;
    write(
        root.path(),
        "src/main.ts",
        concat!(
            "import InlineApp from './InlineApp.vue';\n",
            "import ExternalApp from './ExternalApp.vue';\n",
            "console.log(InlineApp, ExternalApp);\n",
        ),
    )?;
    write(root.path(), "src/InlineApp.vue", INLINE_APP)?;
    write(
        root.path(),
        "src/ExternalApp.vue",
        concat!(
            "<script lang=\"ts\" src=\"./external.ts\"></script>\n",
            "<template><ExternalWidget /></template>\n",
        ),
    )?;
    write(root.path(), "src/external.ts", EXTERNAL_SCRIPT)?;
    write(
        root.path(),
        "src/InlineWidget.ts",
        "export default { name: 'InlineWidget' };\n",
    )?;
    write(
        root.path(),
        "src/ExternalWidget.ts",
        "export default { name: 'ExternalWidget' };\n",
    )?;
    Ok(root)
}

fn import_binding_span(source: &str, binding: &str) -> Result<(u64, u64), std::io::Error> {
    let marker = format!("import {binding} from");
    let statement = source
        .find(&marker)
        .ok_or_else(|| std::io::Error::other(format!("missing import marker {marker}")))?;
    let start = statement + "import ".len();
    let end = start + binding.len();
    Ok((
        u64::try_from(start).map_err(|_| std::io::Error::other("import span start overflow"))?,
        u64::try_from(end).map_err(|_| std::io::Error::other("import span end overflow"))?,
    ))
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

fn source_id(root: &Path, run_id: &str, path: &str) -> Result<String, Box<dyn std::error::Error>> {
    required_str(
        &file_response(root, run_id, path)?,
        "/sourceContext/sourceId",
    )
    .map_err(Into::into)
}

fn assert_empty_findings(root: &Path, run_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let output = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(0));
    Ok(())
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

fn required_u64(value: &Value, pointer: &str) -> Result<u64, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other(format!("missing integer {pointer}")))
}

fn write(root: &Path, relative: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
