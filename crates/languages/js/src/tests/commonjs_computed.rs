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
