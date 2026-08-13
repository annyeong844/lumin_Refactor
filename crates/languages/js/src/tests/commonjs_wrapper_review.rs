use super::*;

#[test]
fn escaped_mapped_arguments_mutate_only_after_the_escape_executes()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "require('@acme/before');\n",
            "function replace(value) { value[1] = customLoader; }\n",
            "replace(arguments, require('@acme/during-arguments'));\n",
            "require('@acme/after');\n",
        )
        .as_bytes(),
    )?;
    let specifiers = payload
        .uses
        .iter()
        .map(|source_use| source_use.specifier.as_str())
        .collect::<Vec<_>>();
    assert_eq!(specifiers, ["@acme/before", "@acme/during-arguments"]);
    assert!(
        payload
            .limitation_details
            .iter()
            .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
    );

    Ok(())
}

#[test]
fn mapped_arguments_method_calls_escape_after_call_arguments_are_evaluated()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "require('@acme/before');\n",
            "arguments.poison = function () { this[1] = customLoader; };\n",
            "arguments.poison(require('@acme/during-call'));\n",
            "require('@acme/after');\n",
        )
        .as_bytes(),
    )?;
    let specifiers = payload
        .uses
        .iter()
        .map(|source_use| source_use.specifier.as_str())
        .collect::<Vec<_>>();
    assert_eq!(specifiers, ["@acme/before", "@acme/during-call"]);
    assert!(
        payload
            .limitation_details
            .iter()
            .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
    );

    let property_lookup = parse_payload(
        SourceKind::CommonJs,
        b"const poison = arguments.poison; require('@acme/after-property-lookup');",
    )?;
    assert_eq!(property_lookup.uses.len(), 1);
    assert_eq!(
        property_lookup.uses[0].specifier,
        "@acme/after-property-lookup"
    );
    assert!(
        !property_lookup
            .limitation_details
            .iter()
            .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
    );

    Ok(())
}

#[test]
fn tagged_template_arguments_escape_after_substitutions_are_evaluated()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "require('@acme/before');\n",
            "function replaceTag(_parts, value) { value[1] = customLoader; }\n",
            "replaceTag`${arguments}${require('@acme/during-template')}`;\n",
            "require('@acme/after');\n",
        )
        .as_bytes(),
    )?;
    let specifiers = payload
        .uses
        .iter()
        .map(|source_use| source_use.specifier.as_str())
        .collect::<Vec<_>>();
    assert_eq!(specifiers, ["@acme/before", "@acme/during-template"]);
    assert!(
        payload
            .limitation_details
            .iter()
            .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
    );

    Ok(())
}

#[test]
fn jsx_arguments_escape_after_attributes_and_children_are_evaluated()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload_with_module_format(
        SourceKind::Tsx,
        concat!(
            "require('@acme/before');\n",
            "function X(_props) { return null; }\n",
            "<X wrapper={arguments} during={require('@acme/during-jsx')} />;\n",
            "require('@acme/after');\n",
        )
        .as_bytes(),
        JsModuleFormat::CommonJs,
    )?;
    let specifiers = payload
        .uses
        .iter()
        .map(|source_use| source_use.specifier.as_str())
        .collect::<Vec<_>>();
    assert_eq!(specifiers, ["@acme/before", "@acme/during-jsx"]);
    assert!(
        payload
            .limitation_details
            .iter()
            .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
    );

    let property_lookup = parse_payload_with_module_format(
        SourceKind::Tsx,
        b"consume(<arguments.Component />); require('@acme/after-property-lookup');",
        JsModuleFormat::CommonJs,
    )?;
    assert_eq!(property_lookup.uses.len(), 1);
    assert_eq!(
        property_lookup.uses[0].specifier,
        "@acme/after-property-lookup"
    );
    assert!(
        !property_lookup
            .limitation_details
            .iter()
            .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
    );
    Ok(())
}

#[test]
fn escaped_module_require_reference_is_visible_as_opaque() -> Result<(), Box<dyn std::error::Error>>
{
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "const load = module.require.bind({ filename: module.filename, paths: module.paths });\n",
            "load('@acme/hidden');\n",
        )
        .as_bytes(),
    )?;
    assert!(payload.uses.is_empty());
    assert_eq!(
        payload.limitation_details,
        [
            COMMONJS_EXPORT_LOWERING_UNSUPPORTED,
            MODULE_REQUIRE_ATTRIBUTION_OPAQUE,
        ]
    );

    let inspected = parse_payload(
        SourceKind::CommonJs,
        b"if (typeof module.require === 'function') require('@acme/known');",
    )?;
    assert_eq!(inspected.uses.len(), 1);
    assert_eq!(inspected.uses[0].specifier, "@acme/known");
    assert_eq!(
        inspected.limitation_details,
        [COMMONJS_EXPORT_LOWERING_UNSUPPORTED]
    );
    Ok(())
}

#[test]
fn strict_or_shadowed_arguments_cannot_mutate_the_wrapper_require()
-> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "'use strict'; replace(arguments); require('@acme/strict');",
        "const arguments = []; replace(arguments); require('@acme/shadowed');",
        "function inspect() { replace(arguments); } require('@acme/nested');",
    ] {
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert_eq!(payload.uses.len(), 1, "wrong attribution for {source}");
        assert!(
            !payload
                .limitation_details
                .iter()
                .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
        );
    }
    Ok(())
}

#[test]
fn wrapper_this_is_opaque_only_in_commonjs_lexical_this_scopes()
-> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "this.value = 1;",
        "Object.defineProperty(this, 'value', { value: 1 });",
        "const publish = () => { this.value = 1; };",
    ] {
        let payload = parse_payload_with_module_format(
            SourceKind::TypeScript,
            source.as_bytes(),
            JsModuleFormat::CommonJs,
        )?;
        assert_eq!(
            payload.limitation_details,
            [COMMONJS_EXPORT_LOWERING_UNSUPPORTED],
            "wrapper this was not visible for {source}",
        );
    }

    let ordinary_function = parse_payload_with_module_format(
        SourceKind::TypeScript,
        b"function local() { this.value = 1; }",
        JsModuleFormat::CommonJs,
    )?;
    assert!(ordinary_function.limitation_details.is_empty());

    let esm = parse_payload_with_module_format(
        SourceKind::TypeScript,
        b"this.value = 1;",
        JsModuleFormat::EsModule,
    )?;
    assert!(esm.limitation_details.is_empty());

    let unknown = parse_payload_with_module_format(
        SourceKind::TypeScript,
        b"this.value = 1;",
        JsModuleFormat::Unknown,
    )?;
    assert_eq!(
        unknown.limitation_details,
        [COMMONJS_EXPORT_LOWERING_UNSUPPORTED]
    );
    Ok(())
}

#[test]
fn one_parse_surface_derives_deduplicated_module_format_products()
-> Result<(), Box<dyn std::error::Error>> {
    let neutral = parse_payload_with_module_formats(
        SourceKind::TypeScript,
        b"import { value } from './dep.js'; export { value };",
        &[
            JsModuleFormat::Unknown,
            JsModuleFormat::CommonJs,
            JsModuleFormat::Unknown,
        ],
    )?;
    assert_eq!(neutral.len(), 2);
    assert_eq!(neutral[0].1, neutral[1].1);

    let contextual = parse_payload_with_module_formats(
        SourceKind::TypeScript,
        b"this.value = 1;",
        &[JsModuleFormat::CommonJs, JsModuleFormat::EsModule],
    )?;
    assert_eq!(contextual.len(), 2);
    assert_ne!(contextual[0].1, contextual[1].1);
    Ok(())
}
