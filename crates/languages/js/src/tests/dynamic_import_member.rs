use super::*;

type UseView = (String, Option<String>, Option<String>, &'static str);

#[test]
fn literal_dynamic_members_are_exact_while_escaped_bindings_remain_broad()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::TypeScript,
        concat!(
            "async function run() {\n",
            "  const loaded = await import('./awaited.js');\n",
            "  console.log(loaded.selectedAwait);\n",
            "  import('./then.js').then((callback) => callback.selectedThen());\n",
            "  console.log((await import('./direct.js')).selectedDirect);\n",
            "  const escaped = await import('./broad.js');\n",
            "  consume(escaped);\n",
            "}\n",
        )
        .as_bytes(),
    )?;

    assert!(payload.limitation_details.is_empty());
    assert_eq!(
        use_views(&payload),
        vec![
            (
                "./awaited.js".to_owned(),
                Some("selectedAwait".to_owned()),
                Some("loaded".to_owned()),
                "named",
            ),
            (
                "./broad.js".to_owned(),
                None,
                Some("escaped".to_owned()),
                "dynamic-broad",
            ),
            (
                "./direct.js".to_owned(),
                Some("selectedDirect".to_owned()),
                None,
                "named",
            ),
            (
                "./then.js".to_owned(),
                Some("selectedThen".to_owned()),
                Some("callback".to_owned()),
                "named",
            ),
        ]
    );
    assert!(
        payload
            .uses
            .iter()
            .all(|source_use| source_use.request_kind == ModuleRequestKind::DynamicImport)
    );
    Ok(())
}

#[test]
fn shadowed_dynamic_bindings_resolve_to_their_own_literal_imports()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = parse_payload(
        SourceKind::TypeScript,
        concat!(
            "async function run(flag: boolean) {\n",
            "  const scoped = await import('./outer.js');\n",
            "  if (flag) {\n",
            "    const scoped = await import('./inner.js');\n",
            "    console.log(scoped.selectedInner);\n",
            "  }\n",
            "  console.log(scoped.selectedOuter);\n",
            "}\n",
        )
        .as_bytes(),
    )?;

    assert_eq!(
        use_views(&payload),
        vec![
            (
                "./inner.js".to_owned(),
                Some("selectedInner".to_owned()),
                Some("scoped".to_owned()),
                "named",
            ),
            (
                "./outer.js".to_owned(),
                Some("selectedOuter".to_owned()),
                Some("scoped".to_owned()),
                "named",
            ),
        ]
    );
    Ok(())
}

#[test]
fn computed_or_aliased_dynamic_bindings_degrade_the_whole_import()
-> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "async function run(key: string) { const loaded = await import('./mod.js'); loaded.safe; loaded[key]; }",
        "async function run() { const loaded = await import('./mod.js'); const alias = loaded; alias.safe; }",
    ] {
        let payload = parse_payload(SourceKind::TypeScript, source.as_bytes())?;
        assert_eq!(payload.uses.len(), 1, "unexpected uses for {source}");
        assert_eq!(payload.uses[0].specifier, "./mod.js");
        assert_eq!(payload.uses[0].kind, ImportKind::DynamicBroad);
        assert_eq!(payload.uses[0].imported_name, None);
    }
    Ok(())
}

#[test]
fn eval_and_import_options_cannot_publish_exact_dynamic_members()
-> Result<(), Box<dyn std::error::Error>> {
    let eval_payload = parse_payload(
        SourceKind::TypeScript,
        b"async function run() { const loaded = await import('./mod.js'); loaded.safe; eval('loaded.hidden'); }",
    )?;
    assert_eq!(eval_payload.uses.len(), 1);
    assert_eq!(eval_payload.uses[0].kind, ImportKind::DynamicBroad);
    assert_eq!(eval_payload.uses[0].imported_name, None);

    let options_payload = parse_payload(
        SourceKind::TypeScript,
        b"async function run() { const loaded = await import('./mod.js', { with: { type: 'json' } }); loaded.safe; }",
    )?;
    assert_eq!(options_payload.uses.len(), 1);
    assert_eq!(options_payload.uses[0].kind, ImportKind::DynamicBroad);
    assert_eq!(options_payload.uses[0].imported_name, None);
    assert!(
        options_payload
            .limitation_details
            .iter()
            .any(|detail| detail.contains("dynamic import options or phases"))
    );
    Ok(())
}

fn use_views(payload: &JsPayloadFacts) -> Vec<UseView> {
    let mut views = payload
        .uses
        .iter()
        .map(|source_use| {
            (
                source_use.specifier.clone(),
                source_use.imported_name.clone(),
                source_use.local_name.clone(),
                match source_use.kind {
                    ImportKind::Named => "named",
                    ImportKind::DynamicBroad => "dynamic-broad",
                    _ => "other",
                },
            )
        })
        .collect::<Vec<_>>();
    views.sort();
    views
}
