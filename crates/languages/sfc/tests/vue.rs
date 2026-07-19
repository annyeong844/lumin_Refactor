use lumin_model::{
    CapabilityState, FileFacts, ImportKind, Limitation, ModuleRequestKind, RepoPath,
    SfcTemplateUseKind, SourceKind, SourceRoles, SourceSnapshot, SourceSpan, SourceUnitId,
    SourceUseFact, SymbolNamespace,
};

fn source(
    path: &str,
    kind: SourceKind,
    text: &str,
) -> Result<SourceSnapshot, Box<dyn std::error::Error>> {
    Ok(SourceSnapshot::new(
        RepoPath::from_portable(path)?,
        kind,
        SourceRoles::default(),
        text.as_bytes().to_vec(),
    ))
}

#[test]
fn vue_inline_units_bind_template_components_and_resources()
-> Result<(), Box<dyn std::error::Error>> {
    let app = source(
        "src/App.vue",
        SourceKind::Vue,
        r#"<template>
  <UserCard />
  <user-list />
  <!-- <GhostCard /> -->
</template>
<script setup lang="ts">
import UserCard from './UserCard.vue';
import UserList from './UserList.vue';
</script>
<style>
  .hero { background: url('./hero.svg'); }
  @import "./theme.css";
  /* url('./ignored.svg') */
</style>
"#,
    )?;
    let decomposition =
        lumin_sfc::decompose(&app, &lumin_sfc::source_index(std::slice::from_ref(&app)))?;
    assert_eq!(decomposition.state, CapabilityState::Complete);
    assert_eq!(decomposition.inline_scripts.len(), 1);
    assert_eq!(decomposition.template_uses.len(), 2);
    assert!(
        decomposition
            .template_uses
            .iter()
            .all(|usage| usage.kind == SfcTemplateUseKind::Static)
    );
    assert_eq!(decomposition.resource_uses.len(), 2);

    let unit = &decomposition.inline_scripts[0];
    assert_eq!(unit.kind, SourceKind::TypeScript);
    assert_eq!(
        &app.bytes[unit.parent_span.start as usize..unit.parent_span.end as usize],
        unit.bytes.as_slice()
    );
    let mut embedded = FileFacts::embedded(app.id.clone(), unit.id.clone());
    embedded.uses = vec![
        imported(&app, "./UserCard.vue", "UserCard"),
        imported(&app, "./UserList.vue", "UserList"),
    ];
    let analysis = lumin_sfc::finalize(decomposition, vec![embedded], &[])?;
    assert_eq!(analysis.state, CapabilityState::Complete);
    assert_eq!(analysis.component_uses.len(), 2);
    assert!(
        analysis
            .component_uses
            .iter()
            .any(|usage| usage.tag_name == "user-list" && usage.binding_name == "UserList")
    );
    assert!(analysis.file_facts.iter().any(|facts| {
        matches!(facts.source_unit, SourceUnitId::Logical(_))
            && facts
                .exports
                .iter()
                .any(|export| export.exported_name == "default")
            && facts
                .uses
                .iter()
                .any(|usage| usage.specifier == "./hero.svg")
            && facts
                .uses
                .iter()
                .any(|usage| usage.specifier == "./theme.css")
    }));
    Ok(())
}

#[test]
fn external_script_attaches_existing_logical_source_without_copying_facts()
-> Result<(), Box<dyn std::error::Error>> {
    let app = source(
        "src/App.vue",
        SourceKind::Vue,
        "<template><ExternalCard /></template><script src=\"./app.ts\"></script>",
    )?;
    let external = source(
        "src/app.ts",
        SourceKind::TypeScript,
        "import ExternalCard from './ExternalCard.vue';",
    )?;
    let sources = vec![app.clone(), external.clone()];
    let decomposition = lumin_sfc::decompose(&app, &lumin_sfc::source_index(&sources))?;
    assert!(decomposition.inline_scripts.is_empty());
    assert_eq!(decomposition.external_scripts.len(), 1);
    assert_eq!(
        decomposition.external_scripts[0].target_source_id,
        external.id
    );

    let mut external_facts = FileFacts::physical(external.id.clone());
    external_facts
        .uses
        .push(imported(&external, "./ExternalCard.vue", "ExternalCard"));
    let analysis = lumin_sfc::finalize(decomposition, Vec::new(), &[external_facts])?;
    assert_eq!(analysis.script_attachments.len(), 1);
    assert_eq!(analysis.component_uses.len(), 1);
    assert_eq!(analysis.file_facts.len(), 1);
    Ok(())
}

#[test]
fn external_script_mode_conflict_is_typed_and_not_attached()
-> Result<(), Box<dyn std::error::Error>> {
    let app = source(
        "src/App.vue",
        SourceKind::Vue,
        "<script lang=\"tsx\" src=\"./app.ts\"></script>",
    )?;
    let external = source("src/app.ts", SourceKind::TypeScript, "export default {};")?;
    let sources = vec![app.clone(), external];
    let decomposition = lumin_sfc::decompose(&app, &lumin_sfc::source_index(&sources))?;
    assert_eq!(decomposition.state, CapabilityState::Incomplete);
    assert!(decomposition.external_scripts.is_empty());
    assert!(matches!(
        decomposition.limitations.as_slice(),
        [Limitation::VueExternalScriptModeConflict { .. }]
    ));
    Ok(())
}

#[test]
fn unsupported_dialects_and_malformed_vue_never_become_empty_success()
-> Result<(), Box<dyn std::error::Error>> {
    let svelte = source(
        "src/Page.svelte",
        SourceKind::Svelte,
        "<script>let x = 1</script>",
    )?;
    let unavailable = lumin_sfc::decompose(
        &svelte,
        &lumin_sfc::source_index(std::slice::from_ref(&svelte)),
    )?;
    assert_eq!(unavailable.state, CapabilityState::Unavailable);
    assert!(matches!(
        unavailable.limitations.as_slice(),
        [Limitation::SfcDialectUnavailable { .. }]
    ));

    let malformed = source(
        "src/App.vue",
        SourceKind::Vue,
        "<script>export default {}</script><script>export default {}</script>",
    )?;
    let decomposition = lumin_sfc::decompose(
        &malformed,
        &lumin_sfc::source_index(std::slice::from_ref(&malformed)),
    )?;
    assert_eq!(decomposition.state, CapabilityState::Incomplete);
    assert!(!decomposition.module_export_known);
    assert!(matches!(
        decomposition.limitations.as_slice(),
        [Limitation::SfcDecompositionUnknown { .. }]
    ));
    Ok(())
}

fn imported(snapshot: &SourceSnapshot, specifier: &str, local_name: &str) -> SourceUseFact {
    SourceUseFact {
        importer: snapshot.id.clone(),
        specifier: specifier.to_owned(),
        imported_name: Some("default".to_owned()),
        local_name: Some(local_name.to_owned()),
        namespace: SymbolNamespace::Value,
        kind: ImportKind::Default,
        request_kind: ModuleRequestKind::StaticImport,
        span: SourceSpan { start: 0, end: 1 },
    }
}
