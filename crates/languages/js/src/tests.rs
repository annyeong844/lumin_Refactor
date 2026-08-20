use lumin_model::{RepoPath, SourceRoles};

use super::*;

mod commonjs_computed;
mod commonjs_wrapper_review;
mod dynamic_import_member;
mod dynamic_import_nonliteral;
mod import_meta_glob;

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
fn recoverable_parse_failure_preserves_module_uses_but_withholds_local_definitions()
-> Result<(), Box<dyn std::error::Error>> {
    let snapshot = SourceSnapshot::new(
        RepoPath::from_portable("recoverable.ts")?,
        SourceKind::TypeScript,
        SourceRoles::default(),
        lumin_model::PhysicalFileIdentity::Unix {
            device: 1,
            inode: 1,
        },
        concat!(
            "import { consumed } from './target.js';\n",
            "console.log(consumed);\n",
            "export const broken;\n",
        )
        .as_bytes()
        .to_vec(),
    );
    let facts = extract(&snapshot)?;
    assert!(facts.exports.is_empty());
    assert_eq!(facts.uses.len(), 1);
    assert_eq!(facts.uses[0].specifier, "./target.js");
    assert!(matches!(
        facts.limitations.as_slice(),
        [Limitation::JsRecoverableParseLocal { .. }]
    ));
    Ok(())
}

#[test]
fn unrecoverable_parse_failure_is_visible_and_not_empty_success()
-> Result<(), Box<dyn std::error::Error>> {
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
    assert!(facts.uses.is_empty());
    assert!(matches!(
        facts.limitations.as_slice(),
        [Limitation::JsModuleUseUnknown { .. }]
    ));
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
            REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
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
            REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
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
                REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
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
                REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
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
            REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
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
            REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
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
            "import {} from '@acme/empty-import';\n",
            "export {} from '@acme/empty';\n",
            "import lib = require('@acme/import-equals');\n",
            "console.log(lib);\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 3);
    assert_eq!(payload.uses[0].specifier, "@acme/empty");
    assert_eq!(payload.uses[0].kind, ImportKind::SideEffect);
    assert_eq!(
        payload.uses[0].request_kind,
        ModuleRequestKind::StaticImport
    );
    assert_eq!(payload.uses[1].specifier, "@acme/empty-import");
    assert_eq!(payload.uses[1].kind, ImportKind::SideEffect);
    assert_eq!(
        payload.uses[1].request_kind,
        ModuleRequestKind::StaticImport
    );
    assert_eq!(payload.uses[2].specifier, "@acme/import-equals");
    assert_eq!(payload.uses[2].kind, ImportKind::Namespace);
    assert_eq!(payload.uses[2].request_kind, ModuleRequestKind::Require);
    assert_eq!(payload.uses[2].local_name.as_deref(), Some("lib"));
    assert_eq!(
        payload.limitation_details,
        vec!["CommonJS export lowering is not implemented in the first audit increment".to_owned(),]
    );
    Ok(())
}

#[test]
fn exported_import_equals_retains_request_and_export_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::TypeScript,
        b"export import lib = require('@acme/import-equals');",
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/import-equals");
    assert_eq!(payload.uses[0].kind, ImportKind::Namespace);
    assert_eq!(payload.uses[0].request_kind, ModuleRequestKind::Require);
    assert_eq!(payload.exports.len(), 1);
    assert_eq!(payload.exports[0].exported_name, "lib");
    assert_eq!(payload.exports[0].local_name.as_deref(), Some("lib"));
    assert_eq!(payload.exports[0].namespace, SymbolNamespace::Value);
    assert!(payload.limitation_details.is_empty());
    Ok(())
}

#[test]
fn escaped_require_is_visible_without_discarding_grounded_edges()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "const known = require('@acme/known');\n",
            "const load = require;\n",
            "load('@acme/hidden');\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/known");
    assert_eq!(
        payload.limitation_details,
        vec![
            "CommonJS export lowering is not implemented in the first audit increment".to_owned(),
            REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn escaped_local_require_binding_does_not_poison_the_wrapper_loader()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "function local(require) { const load = require; return load('local'); }\n",
            "require('@acme/known');\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/known");
    assert_eq!(
        payload.limitation_details,
        vec!["CommonJS export lowering is not implemented in the first audit increment".to_owned(),]
    );
    Ok(())
}

#[test]
fn module_require_is_visible_without_discarding_direct_require_edges()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "const known = require('@acme/known');\n",
            "module.require('@acme/member');\n",
            "module['require']('@acme/computed-member');\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/known");
    assert_eq!(
        payload.limitation_details,
        vec![
            "CommonJS export lowering is not implemented in the first audit increment".to_owned(),
            MODULE_REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn shadowed_commonjs_wrapper_names_do_not_create_limitations()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::TypeScript,
        concat!(
            "function local(module, exports, Object, Reflect) {\n",
            "  module.require('@acme/local');\n",
            "  module.exports.hidden = 1;\n",
            "  exports.alsoHidden = 2;\n",
            "  Object.defineProperty(exports, 'objectHidden', { value: 3 });\n",
            "  Reflect.defineProperty(module.exports, 'reflectHidden', { value: 4 });\n",
            "  Object.defineProperty(module, 'exports', { value: 5 });\n",
            "}\n",
            "export const visible = 1;\n",
        )
        .as_bytes(),
    )?;
    assert!(payload.uses.is_empty());
    assert!(payload.limitation_details.is_empty());
    Ok(())
}

#[test]
fn call_based_commonjs_exports_are_visible_in_ordinary_typescript()
-> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "Object.defineProperty(exports, 'foo', { value: 1 });",
        "Object.defineProperties(module.exports, { foo: { value: 1 } });",
        "Object.assign(exports, { foo: 1 });",
        "Reflect.defineProperty(module['exports'], 'foo', { value: 1 });",
        "Object.defineProperty(module, 'exports', { value: {} });",
        "defineExport(exports);",
        "const exportObject = module.exports; consume(exportObject);",
    ] {
        let payload = parse_payload(SourceKind::TypeScript, source.as_bytes())?;
        assert!(payload.uses.is_empty());
        assert_eq!(
            payload.limitation_details,
            vec![COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned()],
            "call-based CommonJS export was not visible: {source}",
        );
    }
    Ok(())
}

#[test]
fn commonjs_wrapper_export_syntax_is_visible_in_ordinary_typescript()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::TypeScript,
        b"module.exports.foo = 1; exports.bar = 2;",
    )?;
    assert!(payload.uses.is_empty());
    assert_eq!(
        payload.limitation_details,
        vec![COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned()]
    );
    Ok(())
}

#[test]
fn require_assignment_rhs_uses_the_original_loader_before_the_write()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "require = require('@acme/rhs');\n",
            "require('@acme/after');\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/rhs");
    assert_eq!(
        payload.limitation_details,
        vec![
            COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned(),
            REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn initialized_var_require_applies_its_write_after_the_initializer()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "var require = require('@acme/rhs');\n",
            "require('@acme/after');\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/rhs");
    assert_eq!(
        payload.limitation_details,
        vec![
            COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned(),
            REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn destructuring_require_writes_precede_later_pattern_defaults()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "let x;\n",
            "[require, x = require('@acme/not-node')] = [customLoader, undefined];\n",
        )
        .as_bytes(),
    )?;
    assert!(payload.uses.is_empty());
    assert_eq!(
        payload.limitation_details,
        vec![
            COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned(),
            REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn destructuring_rhs_and_same_target_default_precede_their_writes()
-> Result<(), Box<dyn std::error::Error>> {
    for (source, expected) in [
        ("[require] = [require('@acme/rhs')];", "@acme/rhs"),
        (
            "[require = require('@acme/default')] = [undefined];",
            "@acme/default",
        ),
    ] {
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert_eq!(payload.uses.len(), 1, "lost known request in {source}");
        assert_eq!(payload.uses[0].specifier, expected);
        assert_eq!(
            payload.limitation_details,
            vec![
                COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned(),
                REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
            ]
        );
    }
    Ok(())
}

#[test]
fn var_destructuring_uses_binding_initialization_order() -> Result<(), Box<dyn std::error::Error>> {
    let poisoned = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "let x;\n",
            "var [require, x = require('@acme/not-node')] = [customLoader, undefined];\n",
        )
        .as_bytes(),
    )?;
    assert!(poisoned.uses.is_empty());

    for (source, expected) in [
        ("var [require] = [require('@acme/rhs')];", "@acme/rhs"),
        (
            "var [require = require('@acme/default')] = [undefined];",
            "@acme/default",
        ),
    ] {
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert_eq!(payload.uses.len(), 1, "lost known request in {source}");
        assert_eq!(payload.uses[0].specifier, expected);
    }
    Ok(())
}

#[test]
fn eval_poisoning_follows_argument_evaluation() -> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "eval(require('@acme/code'));\n",
            "require('@acme/after');\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/code");
    assert_eq!(
        payload.limitation_details,
        vec![
            COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned(),
            REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
        ]
    );

    let rhs = parse_payload(
        SourceKind::CommonJs,
        b"[code] = [eval('require = customLoader'), require('@acme/after-eval')];",
    )?;
    assert!(rhs.uses.is_empty());
    assert!(
        rhs.limitation_details
            .iter()
            .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
    );
    Ok(())
}

#[test]
fn with_object_is_evaluated_before_dynamic_body_lookup() -> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        b"with (require('@acme/context')) { require('@acme/inside'); }",
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/context");
    assert_eq!(
        payload.limitation_details,
        vec![
            COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned(),
            REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn bare_var_require_redeclaration_preserves_the_wrapper_loader()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        b"var require; const loaded = require('@acme/real');",
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/real");
    assert_eq!(
        payload.limitation_details,
        vec![COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned()]
    );
    Ok(())
}

#[test]
fn package_commonjs_context_preserves_bare_var_require_redeclaration()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload_with_module_format(
        SourceKind::TypeScript,
        b"var require; const loaded = require('@acme/package-wrapper');",
        JsModuleFormat::CommonJs,
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/package-wrapper");
    assert!(payload.limitation_details.is_empty());
    Ok(())
}

#[test]
fn for_in_and_for_of_var_require_write_after_rhs_before_body()
-> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "for (var require of [require('@acme/rhs')]) { require('@acme/body'); }",
        "for (var require in { [require('@acme/rhs')]: true }) { require('@acme/body'); }",
    ] {
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert_eq!(payload.uses.len(), 1, "wrong loop attribution for {source}");
        assert_eq!(payload.uses[0].specifier, "@acme/rhs");
        assert_eq!(
            payload.limitation_details,
            vec![
                COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned(),
                REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
            ]
        );
    }
    Ok(())
}

#[test]
fn declaration_import_equals_is_projected_entirely_to_type_space()
-> Result<(), Box<dyn std::error::Error>> {
    for kind in [
        SourceKind::DeclarationTs,
        SourceKind::DeclarationMts,
        SourceKind::DeclarationCts,
    ] {
        let payload = parse_payload(
            kind,
            b"export import declarationLib = require('@acme/declaration-lib');",
        )?;
        assert_eq!(payload.exports.len(), 1);
        assert_eq!(payload.exports[0].namespace, SymbolNamespace::Type);
        assert_eq!(payload.uses.len(), 1);
        assert_eq!(payload.uses[0].namespace, SymbolNamespace::Type);
        assert_eq!(payload.uses[0].request_kind, ModuleRequestKind::Require);
        assert!(payload.limitation_details.is_empty());
    }
    Ok(())
}

#[test]
fn typeof_require_does_not_escape_the_wrapper_loader() -> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        b"if (typeof (require) === 'function') require('@acme/real');",
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/real");
    assert_eq!(
        payload.limitation_details,
        vec![COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned()]
    );
    Ok(())
}

#[test]
fn require_calls_before_a_proven_top_level_write_remain_grounded()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "require('@acme/before');\n",
            "require = customLoader;\n",
            "require('@acme/after');\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/before");
    assert_eq!(
        payload.limitation_details,
        vec![
            COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned(),
            REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn require_calls_before_later_root_control_flow_writes_remain_grounded()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "require('@acme/before');\n",
            "if (enabled) { require = customLoader; }\n",
            "require('@acme/after');\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/before");
    assert!(
        payload
            .limitation_details
            .iter()
            .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
    );
    Ok(())
}

#[test]
fn instance_field_initializers_are_deferred_without_deferring_class_evaluation()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "class C {\n",
            "  static immediate = require('@acme/static');\n",
            "  [require('@acme/computed')] = 1;\n",
            "  deferred = require('@acme/instance');\n",
            "}\n",
            "require = customLoader;\n",
            "new C();\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(payload.uses.len(), 2);
    for expected in ["@acme/static", "@acme/computed"] {
        assert!(
            payload
                .uses
                .iter()
                .any(|use_fact| use_fact.specifier == expected),
            "missing immediate class-evaluation request {expected}",
        );
    }
    assert!(
        payload
            .limitation_details
            .iter()
            .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
    );
    Ok(())
}

#[test]
fn explicit_esm_module_exports_does_not_imply_a_commonjs_wrapper()
-> Result<(), Box<dyn std::error::Error>> {
    for kind in [SourceKind::Mjs, SourceKind::Mts] {
        let payload = parse_payload(kind, b"module.exports.value = 1;")?;
        assert!(payload.uses.is_empty());
        assert!(payload.limitation_details.is_empty());
    }
    Ok(())
}

#[test]
fn empty_type_import_keeps_resolution_without_value_side_effect_semantics()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(SourceKind::Mts, b"import type {} from '@acme/type-only';")?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/type-only");
    assert_eq!(payload.uses[0].namespace, SymbolNamespace::Type);
    assert!(payload.limitation_details.is_empty());
    Ok(())
}

#[test]
fn intrinsic_eval_without_a_direct_require_is_visible() -> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(SourceKind::CommonJs, b"eval(\"require('@acme/hidden')\");")?;
    assert!(payload.uses.is_empty());
    assert_eq!(
        payload.limitation_details,
        vec![
            COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned(),
            REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn namespace_reexport_retains_its_identity_without_export_all_opacity()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::TypeScript,
        b"export * as namespaceExport from '@acme/lib';",
    )?;
    assert_eq!(payload.uses.len(), 1);
    assert_eq!(payload.uses[0].specifier, "@acme/lib");
    assert_eq!(payload.uses[0].kind, ImportKind::Namespace);
    assert_eq!(
        payload.uses[0].request_kind,
        ModuleRequestKind::StaticImport
    );
    assert_eq!(payload.exports.len(), 1);
    assert_eq!(payload.exports[0].exported_name, "namespaceExport");
    assert_eq!(payload.exports[0].namespace, SymbolNamespace::Value);
    assert!(payload.limitation_details.is_empty());
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
            REQUIRE_ATTRIBUTION_OPAQUE.to_owned(),
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
