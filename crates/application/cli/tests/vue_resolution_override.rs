use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

const INLINE_APP: &str = concat!(
    "<script setup lang=\"ts\">\n",
    "import InlineWidget from './InlineWidget';\n",
    "import InlineControl from './InlineControl.js';\n",
    "</script>\n",
    "<template><InlineWidget /><InlineControl /></template>\n",
);
const EXTERNAL_SCRIPT: &str = concat!(
    "import ExternalWidget from './ExternalWidget';\n",
    "import ExternalControl from './ExternalControl.js';\n",
);

#[derive(Clone, Copy)]
struct ScriptRequest<'a> {
    path: &'a str,
    specifier: &'a str,
    local_name: &'a str,
    source: &'a str,
    target: &'a str,
    requires_extension: bool,
}

const SCRIPT_REQUESTS: [ScriptRequest<'static>; 4] = [
    ScriptRequest {
        path: "src/InlineApp.vue",
        specifier: "./InlineWidget",
        local_name: "InlineWidget",
        source: INLINE_APP,
        target: "src/InlineWidget.ts",
        requires_extension: true,
    },
    ScriptRequest {
        path: "src/InlineApp.vue",
        specifier: "./InlineControl.js",
        local_name: "InlineControl",
        source: INLINE_APP,
        target: "src/InlineControl.ts",
        requires_extension: false,
    },
    ScriptRequest {
        path: "src/external.ts",
        specifier: "./ExternalWidget",
        local_name: "ExternalWidget",
        source: EXTERNAL_SCRIPT,
        target: "src/ExternalWidget.ts",
        requires_extension: true,
    },
    ScriptRequest {
        path: "src/external.ts",
        specifier: "./ExternalControl.js",
        local_name: "ExternalControl",
        source: EXTERNAL_SCRIPT,
        target: "src/ExternalControl.ts",
        requires_extension: false,
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
        let reason =
            format!("{argument} import-mode resolution requires an explicit relative extension");
        for request in SCRIPT_REQUESTS {
            let expected = if request.requires_extension {
                ExpectedOutcome::Unsupported { reason: &reason }
            } else {
                ExpectedOutcome::Internal
            };
            assert_script_request(root.path(), &run_id, request, serialized, expected)?;
        }
        assert_external_sfc_has_no_resolver_lane(root.path(), &run_id, serialized)?;
        assert_empty_findings(root.path(), &run_id)?;
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

#[test]
fn external_vue_template_binding_uses_attached_script_facts()
-> Result<(), Box<dyn std::error::Error>> {
    let root = external_binding_fixture()?;
    let audit = run(
        root.path(),
        &["audit", "--jobs", "1", "--resolution-profile", "bundler"],
    )?;
    assert_status(&audit, 0);
    let summary: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        summary.get("status").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        summary.get("limitationCount").and_then(Value::as_u64),
        Some(1)
    );
    let run_id = field(&audit.stdout, "runId")?;

    for request in SCRIPT_REQUESTS
        .iter()
        .copied()
        .filter(|request| request.path == "src/external.ts")
    {
        assert_script_request(
            root.path(),
            &run_id,
            request,
            "bundler",
            ExpectedOutcome::Internal,
        )?;
    }
    assert_external_sfc_has_no_resolver_lane(root.path(), &run_id, "bundler")?;
    let overview = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    let limitations = required_array(&overview, "/limitations")?;
    assert_eq!(limitations.len(), 1);
    assert_eq!(
        required_str(&limitations[0], "/reason")?,
        "vue-template-opaque"
    );
    assert_eq!(
        required_str(&limitations[0], "/source_id")?,
        source_id(root.path(), &run_id, "src/ExternalApp.vue")?
    );
    assert_eq!(
        required_str(&limitations[0], "/detail")?,
        "template component `MissingExternal` has no local script binding"
    );
    assert_empty_findings(root.path(), &run_id)?;
    Ok(())
}

#[test]
fn vue_resolution_profile_changes_sealed_analysis_input_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let root = identity_fixture()?;

    let (node_gate, node_input) = open_profile_gate(root.path(), "node16", "vue-profile-node16")?;
    abandon_gate(root.path(), &node_gate, "vue-profile-node16-abandon")?;

    let (node_control_gate, node_control_input) =
        open_profile_gate(root.path(), "node16", "vue-profile-node16-control")?;
    assert_eq!(
        node_control_input, node_input,
        "operation identity or prior gate history changed the sealed AnalysisInputId",
    );
    abandon_gate(
        root.path(),
        &node_control_gate,
        "vue-profile-node16-control-abandon",
    )?;

    let (bundler_gate, bundler_input) =
        open_profile_gate(root.path(), "bundler", "vue-profile-bundler")?;
    assert_ne!(
        bundler_input, node_input,
        "changing the Vue resolution override reused the sealed AnalysisInputId",
    );
    abandon_gate(root.path(), &bundler_gate, "vue-profile-bundler-abandon")?;
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
        2,
        "template bindings must not add resolver lanes beyond the two script requests for {}: {source:#?}",
        request.path,
    );
    let matching = resolutions
        .iter()
        .filter(|resolution| {
            resolution
                .pointer("/sourceUse/specifier")
                .and_then(Value::as_str)
                == Some(request.specifier)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        matching.len(),
        1,
        "expected one resolution for {} in {}: {source:#?}",
        request.specifier,
        request.path,
    );
    let resolution = matching[0];
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
            "<template><ExternalWidget /><ExternalControl /></template>\n",
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
    write(
        root.path(),
        "src/InlineControl.ts",
        "export default { name: 'InlineControl' };\n",
    )?;
    write(
        root.path(),
        "src/ExternalControl.ts",
        "export default { name: 'ExternalControl' };\n",
    )?;
    Ok(root)
}

fn identity_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
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
            "import InlineIdentityApp from './InlineIdentityApp.vue';\n",
            "import ExternalIdentityApp from './ExternalIdentityApp.vue';\n",
            "console.log(InlineIdentityApp, ExternalIdentityApp);\n",
        ),
    )?;
    write(
        root.path(),
        "src/InlineIdentityApp.vue",
        concat!(
            "<script setup lang=\"ts\">\n",
            "import InlineIdentity from './InlineIdentity.js';\n",
            "</script>\n",
            "<template><InlineIdentity /></template>\n",
        ),
    )?;
    write(
        root.path(),
        "src/ExternalIdentityApp.vue",
        concat!(
            "<script lang=\"ts\" src=\"./identity-external.ts\"></script>\n",
            "<template><ExternalIdentity /></template>\n",
        ),
    )?;
    write(
        root.path(),
        "src/identity-external.ts",
        "import ExternalIdentity from './ExternalIdentity.js';\n",
    )?;
    write(
        root.path(),
        "src/InlineIdentity.ts",
        "export default { name: 'InlineIdentity' };\n",
    )?;
    write(
        root.path(),
        "src/ExternalIdentity.ts",
        "export default { name: 'ExternalIdentity' };\n",
    )?;
    write(
        root.path(),
        "src/planned-change.ts",
        "export const plannedChange = true;\n",
    )?;
    Ok(root)
}

fn external_binding_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true,"type":"module"}"#,
    )?;
    write(
        root.path(),
        "src/main.ts",
        "import ExternalApp from './ExternalApp.vue';\nconsole.log(ExternalApp);\n",
    )?;
    write(
        root.path(),
        "src/ExternalApp.vue",
        concat!(
            "<script lang=\"ts\" src=\"./external.ts\"></script>\n",
            "<template><ExternalWidget /><ExternalControl /><MissingExternal /></template>\n",
        ),
    )?;
    write(root.path(), "src/external.ts", EXTERNAL_SCRIPT)?;
    write(
        root.path(),
        "src/ExternalWidget.ts",
        "export default { name: 'ExternalWidget' };\n",
    )?;
    write(
        root.path(),
        "src/ExternalControl.ts",
        "export default { name: 'ExternalControl' };\n",
    )?;
    Ok(root)
}

fn open_profile_gate(
    root: &Path,
    profile: &str,
    operation_id: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let opened = run(
        root,
        &[
            "pre-write",
            "--operation-id",
            operation_id,
            "--path",
            "src/planned-change.ts",
            "--resolution-profile",
            profile,
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;
    let shown = run(root, &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(required_str(&shown, "/lifecycle")?, "active");
    assert_eq!(required_u64(&shown, "/baseline/limitationCount")?, 0);
    let analysis_input_id = required_str(&shown, "/baseline/analysisInputId")?;
    assert!(!analysis_input_id.is_empty());
    Ok((gate_id, analysis_input_id))
}

fn abandon_gate(
    root: &Path,
    gate_id: &str,
    operation_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let abandoned = run(
        root,
        &[
            "gate",
            "abandon",
            gate_id,
            "--operation-id",
            operation_id,
            "--reason",
            "profile identity comparison complete",
        ],
    )?;
    assert_status(&abandoned, 3);
    Ok(())
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
