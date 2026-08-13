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
