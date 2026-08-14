use std::collections::BTreeSet;

use lumin_model::{
    DynamicImportTargetScope, Limitation, PhysicalFileIdentity, RepoPath, SourceKind, SourceRoles,
    SourceSnapshot,
};

use super::super::{extract, scope_dynamic_import_limitations};

#[test]
fn relative_static_prefixes_bind_only_matching_inventory_sources()
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
    assert_eq!(limitations[0].2, DynamicImportTargetScope::ExplicitTargets);
    assert_eq!(
        limitations[0].1,
        BTreeSet::from([sources[1].id.clone(), sources[2].id.clone()])
    );
    assert_eq!(limitations[1].0, "../shared/prefix-");
    assert_eq!(limitations[1].2, DynamicImportTargetScope::ExplicitTargets);
    assert_eq!(limitations[1].1, BTreeSet::from([sources[4].id.clone()]));
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
                DynamicImportTargetScope::ExplicitTargets,
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
