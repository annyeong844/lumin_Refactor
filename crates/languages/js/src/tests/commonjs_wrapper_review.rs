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
fn ordinary_template_coercion_mutates_before_later_substitutions()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "require('@acme/before');\n",
            "arguments.toString = function () { this[1] = customLoader; return ''; };\n",
            "`${arguments}${require('@acme/after-coercion')}`;\n",
            "require('@acme/after-template');\n",
        )
        .as_bytes(),
    )?;
    let specifiers = payload
        .uses
        .iter()
        .map(|source_use| source_use.specifier.as_str())
        .collect::<Vec<_>>();
    assert_eq!(specifiers, ["@acme/before"]);
    assert!(
        payload
            .limitation_details
            .iter()
            .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
    );

    Ok(())
}

#[test]
fn coercive_unary_arguments_mutate_require_without_poisoning_noncoercive_unary()
-> Result<(), Box<dyn std::error::Error>> {
    for operator in ["+", "-", "~"] {
        let source = format!(
            concat!(
                "require('@acme/before');\n",
                "arguments.valueOf = function () {{ this[1] = customLoader; return 0; }};\n",
                "{}arguments;\n",
                "require('@acme/after');\n",
            ),
            operator,
        );
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        let specifiers = payload
            .uses
            .iter()
            .map(|source_use| source_use.specifier.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            specifiers,
            ["@acme/before"],
            "wrong attribution for {operator}"
        );
        assert!(
            payload
                .limitation_details
                .iter()
                .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE),
            "missing unary coercion opacity for {operator}",
        );
    }

    for source in [
        "const inspected = !arguments; require('@acme/grounded');",
        "consume(typeof arguments); require('@acme/grounded');",
        "const ignored = void arguments; require('@acme/grounded');",
        "const retained = delete arguments; require('@acme/grounded');",
    ] {
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert_eq!(payload.uses.len(), 1, "wrong attribution for {source}");
        assert_eq!(payload.uses[0].specifier, "@acme/grounded");
        assert!(
            !payload
                .limitation_details
                .iter()
                .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE),
            "noncoercive unary expression became opaque: {source}",
        );
    }

    Ok(())
}

#[test]
fn transparent_typescript_eval_wrapper_still_poisons_later_require()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload_with_module_format(
        SourceKind::TypeScript,
        b"eval!(\"require = customLoader\"); require('@acme/after-eval');",
        JsModuleFormat::CommonJs,
    )?;
    assert!(payload.uses.is_empty());
    assert!(
        payload
            .limitation_details
            .iter()
            .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
    );
    Ok(())
}

#[test]
fn coercive_binary_arguments_mutate_after_both_operands_are_evaluated()
-> Result<(), Box<dyn std::error::Error>> {
    for expression in [
        "arguments + require('@acme/during')",
        "require('@acme/during') + arguments",
        "arguments < require('@acme/during')",
        "arguments == require('@acme/during')",
        "arguments in require('@acme/during')",
        "arguments instanceof require('@acme/during')",
        "require('@acme/during') instanceof arguments",
        "(condition ? arguments : 0) + require('@acme/during')",
        "require('@acme/during') + (condition ? arguments : 0)",
        "(false || arguments) < require('@acme/during')",
        "(0, arguments) == require('@acme/during')",
        "(condition ? arguments : require('@acme/during')) == (condition ? arguments : 1)",
    ] {
        let source = format!(
            concat!(
                "require('@acme/before');\n",
                "arguments.valueOf = function () {{ this[1] = customLoader; return 0; }};\n",
                "{};\n",
                "require('@acme/after');\n",
            ),
            expression,
        );
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        let specifiers = payload
            .uses
            .iter()
            .map(|source_use| source_use.specifier.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            specifiers,
            ["@acme/before", "@acme/during"],
            "wrong binary evaluation order for {expression}",
        );
        assert!(
            payload
                .limitation_details
                .iter()
                .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE),
            "missing binary coercion opacity for {expression}",
        );
    }

    for expression in [
        "+(condition ? arguments : 0)",
        "+(false || arguments)",
        "+(0, arguments)",
        "+(arguments ||= 0)",
        "+(arguments ??= 0)",
        "+(false ? 0 : arguments)",
        "arguments == ({} && +arguments)",
        "arguments == (false ? {} : +arguments)",
    ] {
        let source = format!(
            concat!(
                "arguments.valueOf = function () {{ this[1] = customLoader; return 0; }};\n",
                "try {{ {}; }} catch {{}}\n",
                "require('@acme/after');\n",
            ),
            expression,
        );
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert!(
            payload.uses.is_empty(),
            "wrapped unary coercion left a false grounded edge for {expression}",
        );
        assert!(
            payload
                .limitation_details
                .iter()
                .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE),
            "missing wrapped unary coercion opacity for {expression}",
        );
    }

    for expression in ["arguments++", "++arguments"] {
        let source = format!(
            concat!(
                "arguments.valueOf = function () {{ this[1] = customLoader; return 0; }};\n",
                "try {{ arguments instanceof {}; }} catch {{}}\n",
                "require('@acme/after');\n",
            ),
            expression,
        );
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert!(
            payload.uses.is_empty(),
            "update coercion left a false grounded edge for {expression}",
        );
        assert!(
            payload
                .limitation_details
                .iter()
                .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE),
            "missing update coercion opacity for {expression}",
        );
    }
    let source = concat!(
        "arguments.valueOf = function () { this[1] = customLoader; return 0; };\n",
        "arguments += require('@acme/during');\n",
        "require('@acme/after');\n",
    );
    let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
    assert_eq!(
        payload
            .uses
            .iter()
            .map(|source_use| source_use.specifier.as_str())
            .collect::<Vec<_>>(),
        ["@acme/during"],
    );
    assert!(
        payload
            .limitation_details
            .iter()
            .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE),
        "missing compound-assignment coercion opacity",
    );
    Ok(())
}

#[test]
fn noncoercive_binary_arguments_preserve_later_require() -> Result<(), Box<dyn std::error::Error>> {
    for expression in [
        "arguments === candidate",
        "arguments !== candidate",
        "arguments == null",
        "arguments != void 0",
        "arguments == arguments",
        "arguments == (condition ? arguments : null)",
        "arguments == (condition ? arguments : {})",
        "'1' in arguments",
        "(arguments, 0) + 1",
        "(arguments && 0) + 1",
    ] {
        let source = format!("consume({expression}); require('@acme/grounded');");
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert_eq!(payload.uses.len(), 1, "wrong attribution for {expression}");
        assert_eq!(payload.uses[0].specifier, "@acme/grounded");
        assert!(
            !payload
                .limitation_details
                .iter()
                .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE),
            "noncoercive binary expression became opaque: {expression}",
        );
    }
    for expression in [
        "arguments || +arguments",
        "arguments ?? +arguments",
        "arguments || (arguments[1] = customLoader)",
        "true || +arguments",
        "1 || +arguments",
        "1n || +arguments",
        "`value` || +arguments",
        "false && +arguments",
        "0 && +arguments",
        "0n && +arguments",
        "`` && +arguments",
        "false && (arguments[1] = customLoader)",
        "arguments ||= +arguments",
        "arguments ??= +arguments",
        "+(true || +arguments)",
        "+(1 || +arguments)",
        "+(false && +arguments)",
        "+(0 && +arguments)",
        "true ? 0 : +arguments",
        "+(true ? 0 : +arguments)",
        "arguments == ({} || +arguments)",
        "arguments == (true ? {} : +arguments)",
    ] {
        let source = format!("{expression}; require('@acme/grounded');");
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert_eq!(payload.uses.len(), 1, "wrong attribution for {expression}");
        assert_eq!(payload.uses[0].specifier, "@acme/grounded");
        assert!(
            !payload
                .limitation_details
                .iter()
                .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE),
            "unreachable logical RHS became opaque: {expression}",
        );
    }
    for rhs in [
        "null",
        "false",
        "0",
        "1n",
        "'value'",
        "`value`",
        "void 0",
        "(1 + 2)",
        "(condition ? 1 : 2)",
        "(1, 2)",
        "(true && 1)",
        "(target = 1)",
        "target++",
    ] {
        let source =
            format!("try {{ arguments instanceof {rhs}; }} catch {{}} require('@acme/grounded');");
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert_eq!(payload.uses.len(), 1, "wrong attribution for RHS {rhs}");
        assert_eq!(payload.uses[0].specifier, "@acme/grounded");
        assert!(
            !payload
                .limitation_details
                .iter()
                .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE),
            "primitive instanceof RHS became opaque: {rhs}",
        );
    }
    Ok(())
}

#[test]
fn computed_property_keys_coerce_mapped_arguments_after_key_inputs()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::CommonJs,
        concat!(
            "require('@acme/before');\n",
            "arguments.toString = function () { this[1] = customLoader; return 'key'; };\n",
            "({ [(require('@acme/during'), arguments)]: 1 });\n",
            "require('@acme/after');\n",
        )
        .as_bytes(),
    )?;
    assert_eq!(
        payload
            .uses
            .iter()
            .map(|source_use| source_use.specifier.as_str())
            .collect::<Vec<_>>(),
        ["@acme/before", "@acme/during"],
    );
    assert!(
        payload
            .limitation_details
            .iter()
            .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
    );

    for source in [
        concat!(
            "arguments.toString = function () { this[1] = customLoader; return 'key'; };\n",
            "const { [arguments]: selected } = require('@acme/binding-rhs');\n",
            "require('@acme/binding-after');\n",
        ),
        concat!(
            "arguments.toString = function () { this[1] = customLoader; return 'key'; };\n",
            "({ [arguments]: selected } = require('@acme/assignment-rhs'));\n",
            "require('@acme/assignment-after');\n",
        ),
    ] {
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert_eq!(
            payload.uses.len(),
            1,
            "computed destructuring key ran before its RHS: {source}",
        );
        assert!(payload.uses[0].specifier.ends_with("-rhs"));
        assert!(
            payload
                .limitation_details
                .iter()
                .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
        );
    }
    Ok(())
}

#[test]
fn class_computed_keys_precede_static_initializers() -> Result<(), Box<dyn std::error::Error>> {
    for (source, expected_grounded) in [
        (
            concat!(
                "require('@acme/before');\n",
                "class C {\n",
                "  static dep = require('@acme/static');\n",
                "  [require = customLoader]() {}\n",
                "}\n",
                "require('@acme/after');\n",
            ),
            &["@acme/before"][..],
        ),
        (
            concat!(
                "class C {\n",
                "  [(require('@acme/key-before'), require = customLoader)]() {}\n",
                "  static dep = require('@acme/static');\n",
                "}\n",
            ),
            &["@acme/key-before"][..],
        ),
        (
            concat!(
                "class C {\n",
                "  static dep = require('@acme/static');\n",
                "  [require = customLoader]() {}\n",
                "  static { require('@acme/block'); }\n",
                "}\n",
            ),
            &[][..],
        ),
    ] {
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        let grounded = payload
            .uses
            .iter()
            .map(|source_use| source_use.specifier.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            grounded, expected_grounded,
            "static phase retained a false grounded edge: {source}\n{grounded:?}",
        );
        assert!(
            payload
                .limitation_details
                .iter()
                .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
        );
    }

    let later_write = parse_payload(
        SourceKind::CommonJs,
        b"class C { static dep = require('@acme/static'); } require = customLoader;",
    )?;
    assert_eq!(later_write.uses.len(), 1);
    assert_eq!(later_write.uses[0].specifier, "@acme/static");

    for source in [
        concat!(
            "class C {\n",
            "  static value = (require = customLoader);\n",
            "  [require('@acme/key')]() {}\n",
            "}\n",
            "require('@acme/after');\n",
        ),
        concat!(
            "class C {\n",
            "  static { require = customLoader; }\n",
            "  [require('@acme/key')]() {}\n",
            "}\n",
            "require('@acme/after');\n",
        ),
    ] {
        let payload = parse_payload(SourceKind::CommonJs, source.as_bytes())?;
        assert_eq!(payload.uses.len(), 1, "computed key was poisoned: {source}");
        assert_eq!(payload.uses[0].specifier, "@acme/key");
        assert!(
            payload
                .limitation_details
                .iter()
                .any(|detail| detail == REQUIRE_ATTRIBUTION_OPAQUE)
        );
    }
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

    for source in [
        "class C extends (this.heritage = 1, Base) {}",
        "class C { [this.computed = 1]() {} }",
    ] {
        let class_definition = parse_payload_with_module_format(
            SourceKind::TypeScript,
            source.as_bytes(),
            JsModuleFormat::CommonJs,
        )?;
        assert_eq!(
            class_definition.limitation_details,
            [COMMONJS_EXPORT_LOWERING_UNSUPPORTED],
            "class definition expression lost wrapper this for {source}",
        );
    }

    let class_elements = parse_payload_with_module_format(
        SourceKind::TypeScript,
        concat!(
            "class C {\n",
            "  field = (this.instance = 1);\n",
            "  static field = (this.staticValue = 1);\n",
            "  method() { this.methodValue = 1; }\n",
            "}\n",
        )
        .as_bytes(),
        JsModuleFormat::CommonJs,
    )?;
    assert!(class_elements.limitation_details.is_empty());
    Ok(())
}

#[test]
fn mapped_wrapper_export_slots_are_visible_without_poisoning_shadowed_arguments()
-> Result<(), Box<dyn std::error::Error>> {
    for source in ["arguments[0].foo = 1;", "arguments[2].exports = api;"] {
        let payload = parse_payload_with_module_format(
            SourceKind::TypeScript,
            source.as_bytes(),
            JsModuleFormat::CommonJs,
        )?;
        assert_eq!(
            payload.limitation_details,
            [COMMONJS_EXPORT_LOWERING_UNSUPPORTED],
            "mapped wrapper export was not observed for {source}",
        );
    }

    let shadowed = parse_payload_with_module_format(
        SourceKind::TypeScript,
        b"function local(arguments) { arguments[0].foo = 1; arguments[2].exports = api; }",
        JsModuleFormat::CommonJs,
    )?;
    assert!(shadowed.limitation_details.is_empty());
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
