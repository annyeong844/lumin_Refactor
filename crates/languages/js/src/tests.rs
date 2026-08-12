use lumin_model::{RepoPath, SourceRoles};

use super::*;

#[test]
fn lowers_named_imports_and_exports() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = SourceSnapshot::new(
        RepoPath::from_portable("src/main.ts")?,
        SourceKind::TypeScript,
        SourceRoles::default(),
        lumin_model::PhysicalFileIdentity::Unix {
            device: 1,
            inode: 1,
        },
        b"import { used } from './lib.js'; export const alive = used; export const dead = 1;"
            .to_vec(),
    );
    let facts = extract(&snapshot)?;
    assert!(facts.limitations.is_empty());
    assert_eq!(facts.uses.len(), 1);
    assert_eq!(facts.exports.len(), 2);
    assert_eq!(facts.uses[0].imported_name.as_deref(), Some("used"));
    assert_eq!(facts.uses[0].local_name.as_deref(), Some("used"));
    Ok(())
}

#[test]
fn one_payload_product_binds_distinct_logical_sources() -> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::TypeScript,
        b"import { value } from './dep.js'; export const local = value;",
    )?;
    let left_path = RepoPath::from_portable("packages/a/src/shared.ts")?;
    let right_path = RepoPath::from_portable("packages/b/src/shared.ts")?;
    let left_id = LogicalSourceId::from_path(&left_path);
    let right_id = LogicalSourceId::from_path(&right_path);

    let left = bind_payload(&payload, &left_id, SourceUnitId::Logical(left_id.clone()));
    let right = bind_payload(&payload, &right_id, SourceUnitId::Logical(right_id.clone()));

    assert_ne!(left.source_id, right.source_id);
    assert!(left.exports.iter().all(|fact| fact.source_id == left_id));
    assert!(right.exports.iter().all(|fact| fact.source_id == right_id));
    assert!(left.uses.iter().all(|fact| fact.importer == left_id));
    assert!(right.uses.iter().all(|fact| fact.importer == right_id));
    Ok(())
}

#[test]
fn parse_failure_is_visible_and_not_empty_success() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = SourceSnapshot::new(
        RepoPath::from_portable("broken.ts")?,
        SourceKind::TypeScript,
        SourceRoles::default(),
        lumin_model::PhysicalFileIdentity::Unix {
            device: 1,
            inode: 1,
        },
        b"export const = ;".to_vec(),
    );
    let facts = extract(&snapshot)?;
    assert!(facts.exports.is_empty());
    assert_eq!(facts.limitations.len(), 1);
    Ok(())
}

#[test]
fn commonjs_source_uses_remain_observable() -> Result<(), Box<dyn std::error::Error>> {
    for (kind, source, expected_uses) in [
        (
            SourceKind::Cts,
            concat!(
                "import { value } from '@acme/static';\n",
                "export * from '@acme/export-all';\n",
                "void import('@acme/dynamic');\n",
                "const loaded = require('@acme/require');\n",
                "console.log(value, loaded);\n",
            ),
            4,
        ),
        (
            SourceKind::CommonJs,
            concat!(
                "void import('@acme/dynamic');\n",
                "const loaded = require('@acme/require');\n",
                "console.log(loaded);\n",
            ),
            2,
        ),
    ] {
        let payload = parse_payload(kind, source.as_bytes())?;
        assert_eq!(payload.uses.len(), expected_uses);
        assert!(payload.limitation_details.contains(
            &"CommonJS export lowering is not implemented in the first audit increment".to_owned()
        ));
        assert!(payload.uses.iter().any(|source_use| {
            source_use.specifier == "@acme/dynamic"
                && source_use.request_kind == ModuleRequestKind::DynamicImport
        }));
        assert!(payload.uses.iter().any(|source_use| {
            source_use.specifier == "@acme/require"
                && source_use.request_kind == ModuleRequestKind::Require
        }));
        if kind == SourceKind::Cts {
            assert!(payload.uses.iter().any(|source_use| {
                source_use.specifier == "@acme/static"
                    && source_use.request_kind == ModuleRequestKind::StaticImport
            }));
            assert!(payload.uses.iter().any(|source_use| {
                source_use.specifier == "@acme/export-all"
                    && source_use.kind == ImportKind::ReExportAll
                    && source_use.request_kind == ModuleRequestKind::StaticImport
            }));
            assert!(payload.limitation_details.contains(&
                "export-all from @acme/export-all requires graph expansion not implemented in this increment"
                    .to_owned()
            ));
        }
    }
    Ok(())
}

#[test]
fn commonjs_shadowed_require_is_opaque() -> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        b"function load(require) { return require('@acme/shadowed'); }",
    )?;
    assert!(payload.uses.is_empty());
    assert_eq!(
        payload.limitation_details,
        vec![
            "CommonJS export lowering is not implemented in the first audit increment".to_owned(),
            "local require binding or write makes CommonJS module-use attribution opaque"
                .to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn commonjs_mixed_require_scopes_preserve_grounded_edges() -> Result<(), Box<dyn std::error::Error>>
{
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "const real = require('@acme/real');\n",
            "function load(require) { return require('@acme/local'); }\n",
            "console.log(real, load);\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/real");
    assert_eq!(payload.uses[0].request_kind, ModuleRequestKind::Require);
    assert_eq!(
        payload.limitation_details,
        vec![
            "CommonJS export lowering is not implemented in the first audit increment".to_owned(),
            "local require binding or write makes CommonJS module-use attribution opaque"
                .to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn commonjs_reassigned_require_is_opaque() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "require = customLoader; require('@acme/reassigned');",
        "({ require } = loaders); require('@acme/destructured');",
    ] {
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert!(payload.uses.is_empty());
        assert_eq!(
            payload.limitation_details,
            vec![
                "CommonJS export lowering is not implemented in the first audit increment"
                    .to_owned(),
                "local require binding or write makes CommonJS module-use attribution opaque"
                    .to_owned(),
            ]
        );
    }
    Ok(())
}

#[test]
fn commonjs_dynamic_require_scope_cannot_hide_outer_writes()
-> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "with (loaders) { require = customLoader; } require('@acme/outer');",
        "function mutate() { eval('require = customLoader'); } require('@acme/outer');",
    ] {
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert!(payload.uses.is_empty());
        assert_eq!(
            payload.limitation_details,
            vec![
                "CommonJS export lowering is not implemented in the first audit increment"
                    .to_owned(),
                "local require binding or write makes CommonJS module-use attribution opaque"
                    .to_owned(),
            ]
        );
    }
    Ok(())
}

#[test]
fn commonjs_dynamic_lookup_only_hides_its_own_require_call()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "const real = require('@acme/real');\n",
            "with (loaders) { require('@acme/dynamic'); }\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/real");
    assert_eq!(
        payload.limitation_details,
        vec![
            "CommonJS export lowering is not implemented in the first audit increment".to_owned(),
            "local require binding or write makes CommonJS module-use attribution opaque"
                .to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn commonjs_block_function_require_is_opaque() -> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        b"if (enabled) { function require() {} } require('@acme/annex-b');",
    )?;
    assert!(payload.uses.is_empty());
    assert_eq!(
        payload.limitation_details,
        vec![
            "CommonJS export lowering is not implemented in the first audit increment".to_owned(),
            "local require binding or write makes CommonJS module-use attribution opaque"
                .to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn commonjs_shadowed_eval_preserves_grounded_require_edges()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "const real = require('@acme/real');\n",
            "function load(eval) {\n",
            "  eval('require = customLoader');\n",
            "  return require('@acme/nested');\n",
            "}\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 2);
    assert_eq!(payload.uses[0].specifier, "@acme/nested");
    assert_eq!(payload.uses[1].specifier, "@acme/real");
    assert_eq!(
        payload.limitation_details,
        vec!["CommonJS export lowering is not implemented in the first audit increment".to_owned(),]
    );
    Ok(())
}

#[test]
fn commonjs_optional_eval_preserves_grounded_require_edges()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        b"eval?.('noop'); require('@acme/real');",
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/real");
    assert_eq!(
        payload.limitation_details,
        vec!["CommonJS export lowering is not implemented in the first audit increment".to_owned(),]
    );
    Ok(())
}

#[test]
fn lowers_empty_reexport_and_external_import_equals_requests()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::Cts,
        concat!(
            "export {} from '@acme/empty';\n",
            "import lib = require('@acme/import-equals');\n",
            "console.log(lib);\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 2);
    assert_eq!(payload.uses[0].specifier, "@acme/empty");
    assert_eq!(payload.uses[0].kind, ImportKind::SideEffect);
    assert_eq!(
        payload.uses[0].request_kind,
        ModuleRequestKind::StaticImport
    );
    assert_eq!(payload.uses[1].specifier, "@acme/import-equals");
    assert_eq!(payload.uses[1].kind, ImportKind::Namespace);
    assert_eq!(payload.uses[1].request_kind, ModuleRequestKind::Require);
    assert_eq!(payload.uses[1].local_name.as_deref(), Some("lib"));
    assert_eq!(
        payload.limitation_details,
        vec!["CommonJS export lowering is not implemented in the first audit increment".to_owned(),]
    );
    Ok(())
}

#[test]
fn commonjs_body_var_does_not_shadow_default_parameter_require()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "function load(value = require('@acme/real')) {\n",
            "  var require;\n",
            "  return require('@acme/local');\n",
            "}\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/real");
    assert_eq!(
        payload.limitation_details,
        vec![
            "CommonJS export lowering is not implemented in the first audit increment".to_owned(),
            "local require binding or write makes CommonJS module-use attribution opaque"
                .to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn commonjs_strict_block_function_does_not_shadow_outer_require()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "'use strict';\n",
            "if (enabled) { function require() {} }\n",
            "require('@acme/real');\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/real");
    assert_eq!(
        payload.limitation_details,
        vec!["CommonJS export lowering is not implemented in the first audit increment".to_owned(),]
    );
    Ok(())
}

#[test]
fn cts_ambient_require_declaration_keeps_runtime_loader_edge()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::Cts,
        b"declare const require: NodeRequire; require('@acme/real');",
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/real");
    assert_eq!(
        payload.limitation_details,
        vec!["CommonJS export lowering is not implemented in the first audit increment".to_owned(),]
    );
    Ok(())
}

#[test]
fn cts_ambient_global_require_declaration_keeps_runtime_loader_edge()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::Cts,
        concat!(
            "export {};\n",
            "declare global { var require: NodeRequire; }\n",
            "require('@acme/real');\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/real");
    assert_eq!(
        payload.limitation_details,
        vec!["CommonJS export lowering is not implemented in the first audit increment".to_owned(),]
    );
    Ok(())
}

#[test]
fn embedded_script_keeps_its_unit_identity() -> Result<(), Box<dyn std::error::Error>> {
    let parent = SourceSnapshot::new(
        RepoPath::from_portable("src/App.vue")?,
        SourceKind::Vue,
        SourceRoles::default(),
        lumin_model::PhysicalFileIdentity::Unix {
            device: 1,
            inode: 1,
        },
        Vec::new(),
    );
    let bytes = b"import Card from './Card.vue';".to_vec();
    let payload_sha256 = lumin_model::digest_hex(&bytes);
    let unit_id =
        lumin_model::EmbeddedSourceUnitId::for_parent_span(&parent.id, 20, 50, &payload_sha256);
    let unit = EmbeddedSourceUnit {
        id: unit_id.clone(),
        parent_source_id: parent.id.clone(),
        parent_span: SourceSpan { start: 20, end: 50 },
        kind: SourceKind::TypeScript,
        payload_sha256,
        bytes,
    };
    let facts = extract_embedded(&unit)?;
    assert_eq!(facts.source_id, parent.id);
    assert_eq!(facts.source_unit, SourceUnitId::Embedded(unit_id));
    assert_eq!(facts.uses[0].local_name.as_deref(), Some("Card"));
    Ok(())
}

#[test]
fn raw_sfc_source_is_a_routing_error() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = SourceSnapshot::new(
        RepoPath::from_portable("src/App.vue")?,
        SourceKind::Vue,
        SourceRoles::default(),
        lumin_model::PhysicalFileIdentity::Unix {
            device: 1,
            inode: 1,
        },
        b"<script>export default {}</script>".to_vec(),
    );
    assert!(extract(&snapshot).is_err());
    Ok(())
}
