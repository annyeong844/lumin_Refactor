use std::collections::BTreeSet;

use lumin_model::{
    ImportKind, Limitation, LogicalSourceId, ModuleRequestKind, ResolutionOutcome,
    ResolvedSourceUse, SourceKind, SourceSpan, SourceUseFact, SymbolNamespace,
};

use crate::{
    JsModuleFormat, parse_payload_with_module_format, scope_commonjs_computed_limitations,
};

#[test]
fn direct_dynamic_commonjs_members_and_destructuring_are_marked_computed()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload_with_module_format(
        SourceKind::TypeScript,
        concat!(
            "const key = process.argv[2];\n",
            "require('./member.js')[key];\n",
            "const { [key]: declared } = require('./declared.js');\n",
            "({ [key]: assigned } = require('./assigned.js'));\n",
            "const { ['known']: known } = require('./known-binding.js');\n",
            "require('./known-member.js')['known'];\n",
        )
        .as_bytes(),
        JsModuleFormat::CommonJs,
    )?;

    let computed = payload
        .uses
        .iter()
        .filter(|source_use| source_use.kind == ImportKind::CommonJsComputed)
        .map(|source_use| source_use.specifier.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        computed,
        vec!["./assigned.js", "./declared.js", "./member.js"]
    );
    assert!(payload.uses.iter().any(|source_use| {
        source_use.specifier == "./known-binding.js" && source_use.kind == ImportKind::DynamicBroad
    }));
    assert!(payload.uses.iter().any(|source_use| {
        source_use.specifier == "./known-member.js" && source_use.kind == ImportKind::DynamicBroad
    }));
    Ok(())
}

#[test]
fn computed_access_through_require_result_bindings_respects_value_scopes()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload_with_module_format(
        SourceKind::TypeScript,
        concat!(
            "const key = process.argv[2];\n",
            "const member = require('./bound-member.js'); member[key];\n",
            "const declared = require('./bound-declared.js'); const { [key]: value } = declared;\n",
            "const assigned = require('./bound-assigned.js'); ({ [key]: target } = assigned);\n",
            "const wrapped = require('./bound-wrapped.js'); (wrapped as Record<string, unknown>)[key];\n",
            "const captured = require('./captured.js'); function read() { return captured[key]; }\n",
            "const typed = require('./typed.js'); type typed = {}; typed[key];\n",
            "const staticOnly = require('./static.js'); staticOnly.known;\n",
            "const shadowed = require('./shadowed.js'); { const shadowed = other; shadowed[key]; }\n",
            "const parameter = require('./parameter.js'); function inspect(parameter: object) { return parameter[key]; }\n",
        )
        .as_bytes(),
        JsModuleFormat::CommonJs,
    )?;

    let computed = payload
        .uses
        .iter()
        .filter(|source_use| source_use.kind == ImportKind::CommonJsComputed)
        .map(|source_use| source_use.specifier.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        computed,
        BTreeSet::from([
            "./bound-assigned.js",
            "./bound-declared.js",
            "./bound-member.js",
            "./bound-wrapped.js",
            "./captured.js",
            "./typed.js",
        ])
    );
    for specifier in ["./parameter.js", "./shadowed.js", "./static.js"] {
        assert!(payload.uses.iter().any(|source_use| {
            source_use.specifier == specifier && source_use.kind == ImportKind::DynamicBroad
        }));
    }
    Ok(())
}

#[test]
fn only_resolved_internal_computed_uses_become_module_opacity() {
    let importer = LogicalSourceId::from_string("importer".to_owned());
    let target = LogicalSourceId::from_string("target".to_owned());
    let source_use = SourceUseFact {
        importer: importer.clone(),
        specifier: "./target.js".to_owned(),
        imported_name: None,
        local_name: None,
        namespace: SymbolNamespace::Value,
        kind: ImportKind::CommonJsComputed,
        request_kind: ModuleRequestKind::Require,
        span: SourceSpan { start: 10, end: 32 },
    };
    let limitations = scope_commonjs_computed_limitations(&[
        ResolvedSourceUse {
            source_use: source_use.clone(),
            outcome: ResolutionOutcome::Internal {
                target: target.clone(),
            },
        },
        ResolvedSourceUse {
            source_use,
            outcome: ResolutionOutcome::External {
                package: "external".to_owned(),
            },
        },
    ]);

    assert_eq!(limitations.len(), 1);
    assert!(matches!(
        &limitations[0],
        Limitation::CommonJsComputedMember {
            source_id,
            specifier,
            span: SourceSpan { start: 10, end: 32 },
            target: resolved_target,
        } if source_id == &importer
            && specifier == "./target.js"
            && resolved_target == &target
    ));
}
