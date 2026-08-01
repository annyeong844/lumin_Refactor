use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

#[test]
fn vue_entry_resolves_and_graph_completes() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/main.ts",
        "import App from './App.vue'; console.log(App);\n",
    )?;
    write(
        root.path(),
        "src/App.vue",
        "<template><article>Hello</article></template>\n",
    )?;

    let evidence = audit_evidence(root.path())?;
    assert_complete_vue(&evidence)?;
    assert_empty_findings(root.path(), &evidence.run_id)?;
    Ok(())
}

#[test]
fn vue_inline_script_setup_binds_template_components() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/main.ts",
        "import App from './App.vue'; console.log(App);\n",
    )?;
    write(
        root.path(),
        "src/App.vue",
        concat!(
            "<template><UserCard /></template>\n",
            "<script setup lang=\"ts\">\n",
            "import UserCard from './UserCard.vue';\n",
            "</script>\n",
        ),
    )?;
    write(
        root.path(),
        "src/UserCard.vue",
        "<template><article>User</article></template>\n",
    )?;

    let evidence = audit_evidence(root.path())?;
    assert_complete_vue(&evidence)?;
    assert_empty_findings(root.path(), &evidence.run_id)?;
    Ok(())
}

#[test]
fn vue_external_script_attach_and_mode_conflict() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/main.ts",
        "import App from './App.vue'; console.log(App);\n",
    )?;
    write(
        root.path(),
        "src/App.vue",
        "<template><UserCard /></template><script lang=\"ts\" src=\"./app.ts\"></script>\n",
    )?;
    write(
        root.path(),
        "src/app.ts",
        "import UserCard from './UserCard.vue';\n",
    )?;
    write(
        root.path(),
        "src/UserCard.vue",
        "<template><article>User</article></template>\n",
    )?;
    let attached = audit_evidence(root.path())?;
    assert_complete_vue(&attached)?;
    assert_empty_findings(root.path(), &attached.run_id)?;

    let conflict = tempfile::tempdir()?;
    write(
        conflict.path(),
        "src/main.ts",
        "import App from './App.vue'; console.log(App);\n",
    )?;
    write(
        conflict.path(),
        "src/App.vue",
        "<script lang=\"tsx\" src=\"./app.ts\"></script>\n",
    )?;
    write(conflict.path(), "src/app.ts", "console.log('external');\n")?;
    let conflicted = audit_evidence(conflict.path())?;
    assert_eq!(conflicted.audit_status, "incomplete");
    assert_eq!(
        capability_state(&conflicted.overview, "sfc/vue.v1"),
        Some("incomplete")
    );
    assert_eq!(
        capability_state(&conflicted.overview, "dead-code.v1"),
        Some("incomplete")
    );
    let limitations = limitations(&conflicted.overview)?;
    assert_eq!(limitations.len(), 1);
    assert_eq!(
        limitations[0].get("reason").and_then(Value::as_str),
        Some("vue-external-script-mode-conflict")
    );
    assert_empty_findings(conflict.path(), &conflicted.run_id)?;
    Ok(())
}

#[test]
fn vue_missing_target_is_scoped_without_aborting_graph() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/main.ts",
        "import Missing from './Missing.vue'; console.log(Missing);\n",
    )?;
    write(root.path(), "src/lib.ts", "export const dead = 1;\n")?;

    let evidence = audit_evidence(root.path())?;
    assert_eq!(evidence.audit_status, "incomplete");
    let unresolved = limitations(&evidence.overview)?
        .iter()
        .find(|limitation| {
            limitation.get("reason").and_then(Value::as_str)
                == Some("internal-specifier-unresolved")
        })
        .ok_or_else(|| std::io::Error::other("missing unresolved limitation"))?;
    assert_eq!(
        unresolved.get("specifier").and_then(Value::as_str),
        Some("./Missing.vue")
    );
    assert!(
        unresolved
            .get("candidates")
            .and_then(Value::as_array)
            .is_some_and(|candidates| !candidates.is_empty())
    );
    let findings = findings(root.path(), &evidence.run_id)?;
    assert_eq!(findings.len(), 1);
    assert_eq!(
        findings[0].pointer("/path/display").and_then(Value::as_str),
        Some("src/lib.ts")
    );
    assert_eq!(
        findings[0].get("exportedName").and_then(Value::as_str),
        Some("dead")
    );
    Ok(())
}

#[test]
fn vue_non_source_asset_does_not_probe_declarations() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/main.ts",
        "import App from './App.vue'; console.log(App);\n",
    )?;
    write(
        root.path(),
        "src/App.vue",
        concat!(
            "<template><div>App</div></template>\n",
            "<style>.hero { background: url('./hero.svg'); } @import \"./theme.css\";</style>\n",
        ),
    )?;

    let evidence = audit_evidence(root.path())?;
    assert_complete_vue(&evidence)?;
    assert_empty_findings(root.path(), &evidence.run_id)?;
    Ok(())
}

#[test]
fn sfc_dialect_boundary_vue_complete_svelte_astro_unavailable()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/main.ts",
        "import App from './App.vue'; console.log(App);\n",
    )?;
    write(
        root.path(),
        "src/App.vue",
        "<template><div>Vue</div></template>\n",
    )?;
    write(
        root.path(),
        "src/Page.svelte",
        "<script>import Hidden from './Hidden.ts';</script>\n",
    )?;
    write(
        root.path(),
        "src/Layout.astro",
        "---\nimport X from './X.ts';\n---\n<html></html>\n",
    )?;
    write(root.path(), "src/Hidden.ts", "export default 1;\n")?;
    write(root.path(), "src/X.ts", "export default 2;\n")?;

    let evidence = audit_evidence(root.path())?;
    assert_eq!(evidence.audit_status, "incomplete");
    assert_eq!(
        capability_state(&evidence.overview, "sfc/vue.v1"),
        Some("complete")
    );
    assert_eq!(
        capability_state(&evidence.overview, "sfc/svelte.v1"),
        Some("unavailable")
    );
    assert_eq!(
        capability_state(&evidence.overview, "sfc/astro.v1"),
        Some("unavailable")
    );
    assert_eq!(
        capability_state(&evidence.overview, "dead-code.v1"),
        Some("incomplete")
    );
    let dialects = limitations(&evidence.overview)?
        .iter()
        .map(|limitation| {
            assert_eq!(
                limitation.get("reason").and_then(Value::as_str),
                Some("sfc-dialect-unavailable")
            );
            limitation
                .get("dialect")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other("dialect is missing"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    assert_eq!(
        dialects,
        BTreeSet::from(["astro".to_owned(), "svelte".to_owned()])
    );
    assert_empty_findings(root.path(), &evidence.run_id)?;
    Ok(())
}

struct AuditEvidence {
    run_id: String,
    audit_status: String,
    overview: Value,
}

fn audit_evidence(root: &Path) -> Result<AuditEvidence, Box<dyn std::error::Error>> {
    let audit = run(root, &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    let audit_status = field(&audit.stdout, "status")?;
    let overview = run(root, &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    assert_eq!(
        overview.get("limitationCount").and_then(Value::as_u64),
        overview
            .get("limitations")
            .and_then(Value::as_array)
            .map(|rows| rows.len() as u64)
    );
    Ok(AuditEvidence {
        run_id,
        audit_status,
        overview,
    })
}

fn assert_complete_vue(evidence: &AuditEvidence) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(evidence.audit_status, "complete");
    assert_eq!(
        capability_state(&evidence.overview, "sfc/vue.v1"),
        Some("complete")
    );
    assert!(limitations(&evidence.overview)?.is_empty());
    Ok(())
}

fn capability_state<'a>(overview: &'a Value, capability_id: &str) -> Option<&'a str> {
    overview
        .get("capabilityStates")?
        .as_array()?
        .iter()
        .find(|row| row.get("capabilityId").and_then(Value::as_str) == Some(capability_id))?
        .get("state")?
        .as_str()
}

fn limitations(overview: &Value) -> Result<&Vec<Value>, Box<dyn std::error::Error>> {
    overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("overview limitations are missing").into())
}

fn findings(root: &Path, run_id: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let output = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(
        response.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    response
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| std::io::Error::other("findings items are missing").into())
}

fn assert_empty_findings(root: &Path, run_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    assert!(findings(root, run_id)?.is_empty());
    Ok(())
}

fn write(root: &Path, relative: &str, content: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)
}
