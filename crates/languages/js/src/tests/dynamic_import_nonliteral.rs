use std::collections::BTreeSet;

use lumin_model::{
    DynamicImportTargetScope, Limitation, PhysicalFileIdentity, RepoPath, SourceKind, SourceRoles,
    SourceSnapshot,
};

use super::super::{extract, scope_dynamic_import_limitations};

#[test]
fn relative_static_prefixes_use_the_compact_source_inventory_domain()
-> Result<(), Box<dyn std::error::Error>> {
    let sources = vec![
        snapshot(
            "src/pages/main.ts",
            b"import(`../features/${name}.js`); import('../shared/prefix-' + name);",
            1,
        )?,
        snapshot("src/features/a.ts", b"export const a = 1;", 2)?,
        snapshot("src/features/nested/b.ts", b"export const b = 1;", 3)?,
        snapshot("src/features-old.ts", b"export const old = 1;", 4)?,
        snapshot("src/shared/prefix-one.ts", b"export const one = 1;", 5)?,
        snapshot("src/shared/other.ts", b"export const other = 1;", 6)?,
    ];
    let mut facts = vec![extract(&sources[0])?];
    scope_dynamic_import_limitations(&mut facts, &sources);

    let limitations = facts[0]
        .limitations
        .iter()
        .map(|limitation| match limitation {
            Limitation::DynamicImportNonLiteral {
                static_prefix,
                candidates,
                target_scope,
                ..
            } => Ok((
                static_prefix.clone().unwrap_or_default(),
                candidates.iter().cloned().collect::<BTreeSet<_>>(),
                *target_scope,
            )),
            other => Err(format!("unexpected limitation: {other:?}")),
        })
        .collect::<Result<Vec<_>, _>>()?;

    assert_eq!(limitations.len(), 2);
    assert_eq!(limitations[0].0, "../features/");
    assert_eq!(limitations[0].2, DynamicImportTargetScope::SourceInventory);
    assert!(limitations[0].1.is_empty());
    assert_eq!(limitations[1].0, "../shared/prefix-");
    assert_eq!(limitations[1].2, DynamicImportTargetScope::SourceInventory);
    assert!(limitations[1].1.is_empty());
    Ok(())
}

#[test]
fn no_substitution_template_import_is_literal_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let importer = snapshot("src/main.ts", b"void import(`./used.js`);", 1)?;
    let mut facts = vec![extract(&importer)?];
    scope_dynamic_import_limitations(&mut facts, std::slice::from_ref(&importer));

    assert!(facts[0].limitations.is_empty());
    assert_eq!(facts[0].uses.len(), 1);
    assert_eq!(facts[0].uses[0].specifier, "./used.js");
    assert_eq!(facts[0].uses[0].kind, lumin_model::ImportKind::DynamicBroad);
    Ok(())
}

#[test]
fn bounded_empty_and_unbounded_expressions_remain_distinct()
-> Result<(), Box<dyn std::error::Error>> {
    let importer = snapshot(
        "src/main.ts",
        concat!(
            "import(`./missing/${name}.js`);",
            "import(name);",
            "import(`/absolute/${name}.js`);",
            "import(`./encoded/%2e/${name}.js`);",
        )
        .as_bytes(),
        1,
    )?;
    let mut facts = vec![extract(&importer)?];
    scope_dynamic_import_limitations(&mut facts, std::slice::from_ref(&importer));

    let scopes = facts[0]
        .limitations
        .iter()
        .filter_map(|limitation| match limitation {
            Limitation::DynamicImportNonLiteral {
                static_prefix,
                candidates,
                target_scope,
                ..
            } => Some((static_prefix.as_deref(), candidates.len(), *target_scope)),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        scopes,
        vec![
            (
                Some("./missing/"),
                0,
                DynamicImportTargetScope::SourceInventory,
            ),
            (None, 0, DynamicImportTargetScope::Workspace),
            (Some("/absolute/"), 0, DynamicImportTargetScope::Workspace,),
            (
                Some("./encoded/%2e/"),
                0,
                DynamicImportTargetScope::Workspace,
            ),
        ]
    );
    Ok(())
}

fn snapshot(
    path: &str,
    source: &[u8],
    inode: u64,
) -> Result<SourceSnapshot, Box<dyn std::error::Error>> {
    Ok(SourceSnapshot::new(
        RepoPath::from_portable(path)?,
        SourceKind::TypeScript,
        SourceRoles::default(),
        PhysicalFileIdentity::Unix { device: 1, inode },
        source.to_vec(),
    ))
}
