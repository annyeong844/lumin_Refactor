use std::fs;

use lumin_engine::analyze_repository;
use lumin_inventory::InventoryRequest;
use lumin_model::{CapabilityState, FindingDisposition, Limitation, ResolutionProfile};

#[test]
fn duplicate_workspace_identity_has_no_winner_or_false_dead_finding()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("packages/a"))?;
    fs::create_dir_all(root.path().join("packages/b"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","workspaces":["packages/*"]}"#,
    )?;
    for package in ["a", "b"] {
        fs::write(
            root.path().join(format!("packages/{package}/package.json")),
            r#"{"name":"@acme/lib"}"#,
        )?;
        fs::write(
            root.path().join(format!("packages/{package}/index.ts")),
            format!("export const {package}Dead = 1;"),
        )?;
    }
    fs::write(
        root.path().join("src/main.ts"),
        "import { value } from '@acme/lib'; console.log(value);",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

    assert_eq!(evidence.dead_code_state(), CapabilityState::Incomplete);
    assert!(evidence.findings.is_empty());
    assert_eq!(
        evidence
            .limitations
            .iter()
            .filter(|limitation| matches!(
                limitation,
                Limitation::PackageIdentityUnsupported { detail, .. }
                    if detail.contains("duplicate workspace package identity")
            ))
            .count(),
        2
    );
    Ok(())
}

#[test]
fn package_local_public_surface_gap_keeps_unrelated_review_only_finding()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","exports":7}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/index.ts"),
        "export const publicDead = 1;",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { value } from '@acme/lib'; console.log(value);",
    )?;
    fs::write(
        root.path().join("src/generated.ts"),
        "// @generated\nexport const unrelatedDead = 1;",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

    assert_eq!(evidence.dead_code_state(), CapabilityState::Incomplete);
    assert_eq!(evidence.findings.len(), 1);
    assert_eq!(evidence.findings[0].exported_name, "unrelatedDead");
    assert!(matches!(
        evidence.findings[0].disposition,
        FindingDisposition::ReviewOnly { .. }
    ));
    assert!(
        evidence
            .limitations
            .iter()
            .any(|limitation| matches!(limitation, Limitation::PublicSurfaceUnsupported { .. }))
    );
    Ok(())
}

#[test]
fn pnpm_workspace_membership_drives_package_resolution() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","workspaces":["legacy/*"]}"#,
    )?;
    fs::write(
        root.path().join("pnpm-workspace.yaml"),
        "packages:\n  - packages/*\n  - '!packages/ignored'\n",
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","private":true}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/index.ts"),
        "export const used = 1; export const dead = 2;",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from '@acme/lib'; console.log(used);",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

    assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
    assert_eq!(evidence.findings.len(), 1);
    assert_eq!(evidence.findings[0].exported_name, "dead");
    Ok(())
}

#[test]
fn bundler_excludes_node_condition_and_selects_default() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","private":true,"exports":{"node":"./node.js","default":"./default.js"}}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/node.ts"),
        "export const nodeOnly = 1;",
    )?;
    fs::write(
        root.path().join("packages/lib/default.ts"),
        "export const used = 1; export const defaultDead = 2;",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from '@acme/lib'; console.log(used);",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;
    let mut names = evidence
        .findings
        .iter()
        .map(|finding| finding.exported_name.as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();

    assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
    assert_eq!(names, ["defaultDead", "nodeOnly"]);
    Ok(())
}

#[test]
fn legacy_node_ignores_malformed_exports_and_uses_main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("tsconfig.json"),
        r#"{"compilerOptions":{"moduleResolution":"node","module":"commonjs"}}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","private":true,"exports":7,"imports":7,"main":"./main.js"}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/main.ts"),
        "export const used = 1; export const dead = 2;",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from '@acme/lib'; console.log(used);",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

    assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
    assert_eq!(evidence.findings.len(), 1);
    assert_eq!(evidence.findings[0].exported_name, "dead");
    assert!(!evidence.limitations.iter().any(|limitation| matches!(
        limitation,
        Limitation::PublicSurfaceUnsupported { .. } | Limitation::PackageImportsUnsupported { .. }
    )));
    Ok(())
}

#[test]
fn exact_export_key_wins_before_pattern() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","private":true,"exports":{"./*":"./pattern/*.js","./exact":"./exact.js"}}"#,
    )?;
    fs::create_dir_all(root.path().join("packages/lib/pattern"))?;
    fs::write(
        root.path().join("packages/lib/exact.ts"),
        "export const used = 1; export const exactDead = 2;",
    )?;
    fs::write(
        root.path().join("packages/lib/pattern/exact.ts"),
        "export const patternDead = 1;",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from '@acme/lib/exact'; console.log(used);",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;
    let names = evidence
        .findings
        .iter()
        .map(|finding| finding.exported_name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
    assert_eq!(names, ["exactDead", "patternDead"]);
    Ok(())
}

#[test]
fn invalid_exports_target_blocks_fallback_before_probe() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","private":true,"exports":"./dist%2Findex.js","main":"./main.js"}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/main.ts"),
        "export const used = 1;",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from '@acme/lib'; console.log(used);",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

    assert_eq!(evidence.dead_code_state(), CapabilityState::Incomplete);
    assert!(evidence.findings.is_empty());
    assert!(evidence.limitations.iter().any(|limitation| matches!(
        limitation,
        Limitation::PublicSurfaceUnsupported { detail, .. }
            if detail.contains("forbidden path character")
    )));
    Ok(())
}

#[test]
fn typings_precedes_types_for_type_imports() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","private":true,"typings":"./legacy.ts","types":"./modern.ts"}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/legacy.ts"),
        "export type Shape = string;",
    )?;
    fs::write(
        root.path().join("packages/lib/modern.ts"),
        "export type Shape = number;",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import type { Shape } from '@acme/lib'; const value: Shape = 'ok'; console.log(value);",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

    assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
    assert_eq!(evidence.findings.len(), 1);
    assert_eq!(evidence.findings[0].exported_name, "Shape");
    assert_eq!(evidence.findings[0].path.display, "packages/lib/modern.ts");
    Ok(())
}

#[test]
fn package_imports_and_malformed_importer_type_are_typed_limitations()
-> Result<(), Box<dyn std::error::Error>> {
    let imports_root = tempfile::tempdir()?;
    fs::write(
        imports_root.path().join("package.json"),
        r#"{"name":"app"}"#,
    )?;
    fs::write(
        imports_root.path().join("main.ts"),
        "import { value } from '#internal'; console.log(value);",
    )?;
    let imports = analyze_repository(imports_root.path(), &InventoryRequest::default(), 1, None)?;
    assert!(
        imports
            .limitations
            .iter()
            .any(|limitation| matches!(limitation, Limitation::PackageImportsUnsupported { .. }))
    );

    let format_root = tempfile::tempdir()?;
    fs::write(
        format_root.path().join("package.json"),
        r#"{"name":"app","type":"future"}"#,
    )?;
    fs::write(
        format_root.path().join("tsconfig.json"),
        r#"{"compilerOptions":{"moduleResolution":"node16","module":"node16"}}"#,
    )?;
    fs::write(
        format_root.path().join("main.ts"),
        "export const value = 1;",
    )?;
    let format = analyze_repository(format_root.path(), &InventoryRequest::default(), 1, None)?;
    assert!(
        format
            .limitations
            .iter()
            .any(|limitation| matches!(limitation, Limitation::ImporterFormatUnsupported { .. }))
    );
    Ok(())
}

#[test]
fn legacy_node_does_not_consult_package_imports() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("package.json"),
        r##"{"name":"app","imports":{"#internal":"./internal.js"}}"##,
    )?;
    fs::write(
        root.path().join("main.ts"),
        "import { value } from '#internal'; export const dead = 1; console.log(value);",
    )?;

    let evidence = analyze_repository(
        root.path(),
        &InventoryRequest::default(),
        1,
        Some(ResolutionProfile::Node),
    )?;

    assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
    assert_eq!(evidence.findings.len(), 1);
    assert_eq!(evidence.findings[0].exported_name, "dead");
    assert!(
        !evidence
            .limitations
            .iter()
            .any(|limitation| matches!(limitation, Limitation::PackageImportsUnsupported { .. }))
    );
    Ok(())
}

#[test]
fn public_barrel_protects_only_exported_identity() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","private":true,"workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","exports":"./index.js"}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/index.ts"),
        "export { publicValue } from './internal.js';",
    )?;
    fs::write(
        root.path().join("packages/lib/internal.ts"),
        "export const publicValue = 1; export const siblingDead = 2;",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

    assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
    assert_eq!(evidence.findings.len(), 1);
    assert_eq!(evidence.findings[0].exported_name, "siblingDead");
    Ok(())
}

#[test]
fn bare_external_stays_external_without_package_limitation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("main.ts"),
        "import react from 'react'; export const dead = react;",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

    assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
    assert_eq!(evidence.findings.len(), 1);
    assert!(evidence.limitations.is_empty());
    Ok(())
}

#[test]
fn node16_uses_import_for_dynamic_and_require_for_cjs_static_edges()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","type":"commonjs","workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("tsconfig.json"),
        r#"{"compilerOptions":{"moduleResolution":"node16","module":"node16"}}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","private":true,"exports":{"import":"./import.js","require":"./require.js"}}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/import.ts"),
        "export const imported = 1;",
    )?;
    fs::write(
        root.path().join("packages/lib/require.ts"),
        "export const required = 1; export const requireDead = 2;",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { required } from '@acme/lib'; void import('@acme/lib'); console.log(required);",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

    assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
    assert_eq!(evidence.findings.len(), 1);
    assert_eq!(evidence.findings[0].exported_name, "requireDead");
    Ok(())
}

#[test]
fn types_versions_blocks_type_fallback_only_in_affected_package()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","private":true,"types":"./types.ts","typesVersions":{"*":{"*":[]}}}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/types.ts"),
        "export type Shape = string;",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import type { Shape } from '@acme/lib'; export const appDead = 1; const value: Shape = 'x'; console.log(value);",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

    assert_eq!(evidence.dead_code_state(), CapabilityState::Incomplete);
    assert_eq!(evidence.findings.len(), 1);
    assert_eq!(evidence.findings[0].exported_name, "appDead");
    assert!(evidence.limitations.iter().any(|limitation| matches!(
        limitation,
        Limitation::PublicSurfaceUnsupported { detail, .. }
            if detail.contains("typesVersions")
    )));
    Ok(())
}

#[test]
fn exports_pattern_contributes_public_surface_for_existing_target()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("packages/lib/src"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","private":true,"workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","exports":{"./features/*":"./src/*.js"}}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/src/button.ts"),
        "export const Button = 1;",
    )?;

    let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

    assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
    assert!(evidence.findings.is_empty());
    Ok(())
}
