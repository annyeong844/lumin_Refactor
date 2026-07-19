use std::fs;

use lumin_engine::analyze_repository;
use lumin_inventory::InventoryRequest;
use lumin_model::{CapabilityState, Limitation};

#[test]
fn vue_entry_and_inline_script_setup_complete_the_graph() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/main.ts",
        "import App from './App.vue'; console.log(App);",
    )?;
    write(
        root.path(),
        "src/App.vue",
        r#"<template><UserCard /></template>
<script setup lang="ts">
import UserCard from './UserCard.vue';
</script>
<style>.hero { background: url('./hero.svg'); }</style>
"#,
    )?;
    write(
        root.path(),
        "src/UserCard.vue",
        "<template><article>User</article></template>",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 3, None)?;
    assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
    assert!(evidence.findings.is_empty(), "{:#?}", evidence.findings);
    assert!(
        evidence.limitations.is_empty(),
        "{:#?}",
        evidence.limitations
    );
    assert_eq!(
        capability(&evidence, lumin_sfc::VUE_CAPABILITY_ID),
        Some(CapabilityState::Complete)
    );
    assert_eq!(
        capability(&evidence, lumin_sfc::SVELTE_CAPABILITY_ID),
        Some(CapabilityState::Unavailable)
    );
    Ok(())
}

#[test]
fn vue_external_script_uses_existing_source_facts() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/main.ts",
        "import App from './App.vue'; console.log(App);",
    )?;
    write(
        root.path(),
        "src/App.vue",
        "<template><UserCard /></template><script lang=\"ts\" src=\"./app.ts\"></script>",
    )?;
    write(
        root.path(),
        "src/app.ts",
        "import UserCard from './UserCard.vue';",
    )?;
    write(
        root.path(),
        "src/UserCard.vue",
        "<template><article>User</article></template>",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 2, None)?;
    assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
    assert!(
        evidence.limitations.is_empty(),
        "{:#?}",
        evidence.limitations
    );
    assert!(evidence.findings.is_empty(), "{:#?}", evidence.findings);
    Ok(())
}

#[test]
fn missing_vue_target_is_scoped_without_aborting_unrelated_files()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/main.ts",
        "import Missing from './Missing.vue'; console.log(Missing);",
    )?;
    write(root.path(), "src/lib.ts", "export const dead = 1;")?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 2, None)?;
    assert_eq!(evidence.dead_code_state(), CapabilityState::Incomplete);
    assert!(evidence.limitations.iter().any(|limitation| matches!(
        limitation,
        Limitation::InternalSpecifierUnresolved { specifier, .. } if specifier == "./Missing.vue"
    )));
    assert!(
        evidence
            .findings
            .iter()
            .any(|finding| finding.path.display == "src/lib.ts" && finding.exported_name == "dead"),
        "{:#?}",
        evidence.findings
    );
    Ok(())
}

#[test]
fn unsupported_sfc_dialect_is_visible_and_deterministic() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/Page.svelte",
        "<script>import Hidden from './Hidden.ts';</script>",
    )?;
    write(root.path(), "src/Hidden.ts", "export default 1;")?;

    let one = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;
    let many = analyze_repository(root.path(), &InventoryRequest::default(), 4, None)?;
    assert_eq!(one, many);
    assert_eq!(one.dead_code_state(), CapabilityState::Incomplete);
    assert_eq!(
        capability(&one, lumin_sfc::SVELTE_CAPABILITY_ID),
        Some(CapabilityState::Unavailable)
    );
    assert!(matches!(
        one.limitations.as_slice(),
        [Limitation::SfcDialectUnavailable { dialect, .. }] if dialect == "svelte"
    ));
    assert!(one.findings.is_empty());
    Ok(())
}

fn capability(
    evidence: &lumin_evidence::RunEvidence,
    capability_id: &str,
) -> Option<CapabilityState> {
    evidence
        .capabilities
        .iter()
        .find(|record| record.capability_id == capability_id)
        .map(|record| record.state)
}

fn write(root: &std::path::Path, path: &str, source: &str) -> std::io::Result<()> {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, source)
}
