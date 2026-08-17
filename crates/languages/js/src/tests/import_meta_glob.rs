use std::collections::BTreeSet;

use lumin_model::{
    ImportMetaGlobTargetScope, Limitation, ModuleRequestKind, PhysicalFileIdentity, RepoPath,
    SourceKind, SourceRoles, SourceSnapshot,
};

use super::super::{extract, scope_import_meta_globs};

#[test]
fn relative_literal_arrays_expand_with_negative_and_globstar_patterns()
-> Result<(), Box<dyn std::error::Error>> {
    let sources = vec![
        snapshot(
            "src/main.ts",
            concat!(
                "const modules = import.meta.glob([",
                "'./pages/*.ts', './pages/**/*.ts', '!./pages/private/**'",
                "]);",
            )
            .as_bytes(),
            1,
        )?,
        snapshot("src/pages/one.ts", b"export const one = 1;", 2)?,
        snapshot("src/pages/nested/two.ts", b"export const two = 2;", 3)?,
        snapshot(
            "src/pages/private/secret.ts",
            b"export const secret = 3;",
            4,
        )?,
        snapshot("src/other.ts", b"export const other = 4;", 5)?,
    ];
    let mut facts = vec![extract(&sources[0])?];
    let bound = scope_import_meta_globs(&mut facts, &sources, &[]);

    assert!(facts[0].limitations.is_empty());
    assert_eq!(
        bound
            .iter()
            .map(|bound| bound.source_use.specifier.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["./pages/nested/two.ts", "./pages/one.ts"]),
    );
    assert!(
        bound
            .iter()
            .all(|bound| bound.source_use.request_kind == ModuleRequestKind::ImportMetaGlob)
    );
    assert_eq!(
        bound
            .iter()
            .map(|bound| bound.target.clone())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([sources[1].id.clone(), sources[2].id.clone()]),
    );
    Ok(())
}

#[test]
fn unsupported_options_keep_relative_candidates_while_aliases_remain_package_scoped()
-> Result<(), Box<dyn std::error::Error>> {
    let sources = vec![
        snapshot(
            "src/main.ts",
            concat!(
                "import.meta.glob([",
                "'./features/z*.ts', './features/*.ts', './features/*.ts'",
                "], { eager: true });",
                "import.meta.glob('@alias/*.ts');",
                "import.meta.glob(name);",
                "import.meta['glob']('./features/*.ts');",
            )
            .as_bytes(),
            1,
        )?,
        snapshot("src/features/one.ts", b"export const one = 1;", 2)?,
        snapshot("src/unrelated.ts", b"export const unrelated = 1;", 3)?,
    ];
    let mut facts = vec![extract(&sources[0])?];
    let bound = scope_import_meta_globs(&mut facts, &sources, &[]);

    assert!(facts[0].uses.is_empty());
    assert!(bound.is_empty());
    assert_eq!(facts[0].limitations.len(), 4);
    let feature_id = sources[1].id.clone();
    let explicit = facts[0]
        .limitations
        .iter()
        .filter_map(|limitation| match limitation {
            Limitation::ImportMetaGlobUnsupported {
                candidates,
                target_scope: ImportMetaGlobTargetScope::ExplicitTargets,
                ..
            } => Some(candidates),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(explicit.len(), 2);
    assert!(
        explicit
            .iter()
            .all(|candidates| candidates == &&vec![feature_id.clone()])
    );
    assert!(facts[0].limitations.iter().any(|limitation| matches!(
        limitation,
        Limitation::ImportMetaGlobUnsupported { patterns, .. }
            if patterns.iter().map(String::as_str).eq([
                "./features/*.ts",
                "./features/z*.ts",
            ])
    )));
    assert_eq!(
        facts[0]
            .limitations
            .iter()
            .filter(|limitation| matches!(
                limitation,
                Limitation::ImportMetaGlobUnsupported {
                    target_scope: ImportMetaGlobTargetScope::Package,
                    ..
                }
            ))
            .count(),
        2,
    );
    Ok(())
}

#[test]
fn no_substitution_templates_are_supported_literals() -> Result<(), Box<dyn std::error::Error>> {
    let sources = vec![
        snapshot(
            "src/main.ts",
            b"(import.meta.glob)(`./used.ts`); import.meta.glob!('./other.ts');",
            1,
        )?,
        snapshot("src/used.ts", b"export const used = 1;", 2)?,
        snapshot("src/other.ts", b"export const other = 1;", 3)?,
    ];
    let mut facts = vec![extract(&sources[0])?];
    let bound = scope_import_meta_globs(&mut facts, &sources, &[]);
    assert!(facts[0].limitations.is_empty());
    assert_eq!(
        bound
            .iter()
            .map(|bound| bound.source_use.specifier.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["./other.ts", "./used.ts"]),
    );
    Ok(())
}

#[test]
fn repository_escape_never_becomes_a_clean_empty_expansion()
-> Result<(), Box<dyn std::error::Error>> {
    let sources = vec![snapshot(
        "src/main.ts",
        b"import.meta.glob('../../outside/*.ts');",
        1,
    )?];
    let mut facts = vec![extract(&sources[0])?];
    let bound = scope_import_meta_globs(&mut facts, &sources, &[]);

    assert!(facts[0].uses.is_empty());
    assert!(bound.is_empty());
    assert!(matches!(
        facts[0].limitations.as_slice(),
        [Limitation::ImportMetaGlobUnsupported {
            target_scope: ImportMetaGlobTargetScope::Package,
            candidates,
            ..
        }] if candidates.is_empty()
    ));
    Ok(())
}

#[test]
fn hard_excluded_literal_contexts_remain_package_scoped() -> Result<(), Box<dyn std::error::Error>>
{
    let sources = vec![snapshot(
        "src/main.ts",
        b"import.meta.glob('./node_modules/**/*.ts');",
        1,
    )?];
    let mut facts = vec![extract(&sources[0])?];
    let bound = scope_import_meta_globs(&mut facts, &sources, &["node_modules"]);

    assert!(facts[0].uses.is_empty());
    assert!(bound.is_empty());
    assert!(matches!(
        facts[0].limitations.as_slice(),
        [Limitation::ImportMetaGlobUnsupported {
            target_scope: ImportMetaGlobTargetScope::Package,
            candidates,
            ..
        }] if candidates.is_empty()
    ));
    Ok(())
}

#[test]
fn wildcard_hard_excluded_contexts_remain_package_scoped() -> Result<(), Box<dyn std::error::Error>>
{
    let sources = vec![snapshot(
        "src/main.ts",
        b"import.meta.glob('./node*/**/*.ts');",
        1,
    )?];
    let mut facts = vec![extract(&sources[0])?];
    let bound = scope_import_meta_globs(&mut facts, &sources, &["node_modules"]);

    assert!(bound.is_empty());
    assert!(matches!(
        facts[0].limitations.as_slice(),
        [Limitation::ImportMetaGlobUnsupported {
            target_scope: ImportMetaGlobTargetScope::Package,
            candidates,
            ..
        }] if candidates.is_empty()
    ));
    Ok(())
}

#[test]
fn unsupported_relative_grammar_keeps_its_cross_package_static_domain()
-> Result<(), Box<dyn std::error::Error>> {
    let sources = vec![
        snapshot(
            "packages/a/src/main.ts",
            b"import.meta.glob('../../b/src/*.{ts,tsx}');",
            1,
        )?,
        snapshot("packages/b/src/one.ts", b"export const one = 1;", 2)?,
        snapshot("packages/a/src/unrelated.ts", b"export const other = 1;", 3)?,
    ];
    let mut facts = vec![extract(&sources[0])?];
    let bound = scope_import_meta_globs(&mut facts, &sources, &[]);

    assert!(bound.is_empty());
    assert!(matches!(
        facts[0].limitations.as_slice(),
        [Limitation::ImportMetaGlobUnsupported {
            target_scope: ImportMetaGlobTargetScope::ExplicitTargets,
            candidates,
            ..
        }] if candidates == &vec![sources[1].id.clone()]
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn native_only_components_match_wildcards_without_display_lowering()
-> Result<(), Box<dyn std::error::Error>> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    let importer = snapshot("src/main.ts", b"import.meta.glob('./pages/*.ts');", 1)?;
    let native_path =
        RepoPath::from_native_relative(Path::new(OsStr::from_bytes(b"src/pages/\x80.ts")))?;
    let target = SourceSnapshot::new(
        native_path,
        SourceKind::TypeScript,
        SourceRoles::default(),
        PhysicalFileIdentity::Unix {
            device: 1,
            inode: 2,
        },
        b"export const native = 1;".to_vec(),
    );
    let sources = vec![importer, target.clone()];
    let mut facts = vec![extract(&sources[0])?];
    let bound = scope_import_meta_globs(&mut facts, &sources, &[]);

    assert!(facts[0].limitations.is_empty());
    assert_eq!(bound.len(), 1);
    assert_eq!(bound[0].target, target.id);
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
