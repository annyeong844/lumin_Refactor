use super::*;
use std::sync::Arc;

#[test]
fn generated_marker_must_be_in_leading_comment() {
    assert!(generated_marker(b"// @generated\nexport const value = 1;"));
    assert!(generated_marker(
        b" /* tool @generated output */\nexport const value = 1;"
    ));
    assert!(!generated_marker(b"const text = '@generated';"));
}

#[test]
fn config_identity_observation_preserves_hard_link_alias_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("config"))?;
    fs::write(root.path().join("config/base.json"), "{}\n")?;
    fs::hard_link(
        root.path().join("config/base.json"),
        root.path().join("config/alias.json"),
    )?;
    let base = RepoPath::from_portable("config/base.json")?;
    let alias = RepoPath::from_portable("config/alias.json")?;

    assert_eq!(
        observe_config_physical_identity(root.path(), &base)?,
        observe_config_physical_identity(root.path(), &alias)?
    );
    Ok(())
}

#[test]
fn hard_link_sources_share_captured_payload_but_keep_logical_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("packages/a/src"))?;
    fs::create_dir_all(root.path().join("packages/b/src"))?;
    fs::write(
        root.path().join("packages/a/src/shared.ts"),
        "export const shared = 1;",
    )?;
    fs::hard_link(
        root.path().join("packages/a/src/shared.ts"),
        root.path().join("packages/b/src/shared.ts"),
    )?;

    let inventory = scan(root.path(), &InventoryRequest::default())?;
    let left = inventory
        .sources
        .iter()
        .find(|source| source.path.display_escaped() == "packages/a/src/shared.ts")
        .ok_or_else(|| std::io::Error::other("missing left logical source"))?;
    let right = inventory
        .sources
        .iter()
        .find(|source| source.path.display_escaped() == "packages/b/src/shared.ts")
        .ok_or_else(|| std::io::Error::other("missing right logical source"))?;

    assert_ne!(left.id, right.id);
    assert_eq!(left.physical_identity, right.physical_identity);
    assert_eq!(left.payload_snapshot_id, right.payload_snapshot_id);
    assert!(Arc::ptr_eq(&left.bytes, &right.bytes));
    Ok(())
}

#[cfg(windows)]
#[test]
fn explicit_case_spelling_is_a_distinct_logical_source_on_case_insensitive_storage()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(root.path().join("src/Case.ts"), "export const value = 1;")?;
    let request = InventoryRequest {
        entries: vec![
            RepoPath::from_portable("src/Case.ts")?,
            RepoPath::from_portable("src/case.ts")?,
        ],
        ..Default::default()
    };

    let inventory = scan(root.path(), &request)?;
    let aliases = inventory
        .sources
        .iter()
        .filter(|source| {
            matches!(
                source.path.display_escaped().as_str(),
                "src/Case.ts" | "src/case.ts"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(aliases.len(), 2);
    assert_ne!(aliases[0].id, aliases[1].id);
    assert_eq!(aliases[0].physical_identity, aliases[1].physical_identity);
    assert_eq!(
        aliases[0].payload_snapshot_id,
        aliases[1].payload_snapshot_id
    );
    assert!(Arc::ptr_eq(&aliases[0].bytes, &aliases[1].bytes));
    Ok(())
}

#[test]
fn demanded_nonregular_configs_keep_fact_owner_limitations()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    for name in ["package.json", "tsconfig.json", "pnpm-workspace.yaml"] {
        fs::create_dir(root.path().join(name))?;
    }

    let package = capture_config(
        root.path(),
        &RepoPath::from_portable("package.json")?,
        ConfigSyntax::StrictJson,
    )?;
    assert!(matches!(
        package.observation,
        ConfigObservation::NonRegular { .. }
    ));
    assert!(matches!(
        package.limitation,
        Some(Limitation::PackageMetadataUnobservable { .. })
    ));

    let tsconfig = capture_config(
        root.path(),
        &RepoPath::from_portable("tsconfig.json")?,
        ConfigSyntax::Jsonc,
    )?;
    assert!(matches!(
        tsconfig.observation,
        ConfigObservation::NonRegular { .. }
    ));
    assert!(tsconfig.limitation.is_none());

    let workspace = capture_config(
        root.path(),
        &RepoPath::from_portable("pnpm-workspace.yaml")?,
        ConfigSyntax::RestrictedYaml,
    )?;
    assert!(matches!(
        workspace.observation,
        ConfigObservation::NonRegular { .. }
    ));
    assert!(matches!(
        workspace.limitation,
        Some(Limitation::WorkspaceOwnershipUnsupported { .. })
    ));
    Ok(())
}

#[test]
fn scans_generated_and_explicit_vendor_roles() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("lumin.json"),
        r#"{"schemaVersion":"lumin-config.v1","scan":{"roles":[{"pattern":"src/vendor.ts","role":"vendor"}]}}"#,
    )?;
    fs::write(
        root.path().join("src/generated.ts"),
        "// @generated\nexport const generated = 1;",
    )?;
    fs::write(
        root.path().join("src/vendor.ts"),
        "export const vendored = 1;",
    )?;

    let inventory = scan(root.path(), &InventoryRequest::default())?;
    assert_eq!(inventory.sources.len(), 2);
    assert_eq!(
        inventory
            .sources
            .iter()
            .filter(|source| source.roles.generated.is_some())
            .count(),
        1
    );
    assert_eq!(
        inventory
            .sources
            .iter()
            .filter(|source| source.roles.vendored.is_some())
            .count(),
        1
    );
    Ok(())
}

#[test]
fn same_tier_configuration_role_conflicts_are_order_independent_hard_stops()
-> Result<(), Box<dyn std::error::Error>> {
    let mut errors = Vec::new();
    for roles in [
        r#"[{"pattern":"src/a.ts","role":"generated"},{"pattern":"src/a.ts","role":"authored"}]"#,
        r#"[{"pattern":"src/a.ts","role":"authored"},{"pattern":"src/a.ts","role":"generated"}]"#,
    ] {
        let root = tempfile::tempdir()?;
        fs::create_dir(root.path().join("src"))?;
        fs::write(root.path().join("src/a.ts"), "export const a = 1;\n")?;
        fs::write(
            root.path().join("lumin.json"),
            format!(r#"{{"schemaVersion":"lumin-config.v1","scan":{{"roles":{roles}}}}}"#),
        )?;

        let error = scan(root.path(), &InventoryRequest::default())
            .err()
            .ok_or_else(|| {
                std::io::Error::other("conflicting configuration roles were accepted")
            })?;
        errors.push(error.to_string());
    }

    assert_eq!(errors[0], errors[1]);
    assert_eq!(
        errors[0],
        "malformed lumin.json: contradictory configuration source role declarations for src/a.ts: generated conflicts with authored"
    );
    Ok(())
}

#[test]
fn same_tier_invocation_role_conflicts_are_order_independent_hard_stops()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(root.path().join("src/a.ts"), "export const a = 1;\n")?;
    let mut errors = Vec::new();
    for roles in [
        [ScanRole::Generated, ScanRole::Authored],
        [ScanRole::Authored, ScanRole::Generated],
    ] {
        let request = InventoryRequest {
            role_overrides: roles
                .into_iter()
                .map(|role| RoleOverride {
                    pattern: "src/a.ts".to_owned(),
                    role,
                })
                .collect(),
            ..InventoryRequest::default()
        };
        let error = scan(root.path(), &request)
            .err()
            .ok_or_else(|| std::io::Error::other("conflicting invocation roles were accepted"))?;
        errors.push(error.to_string());
    }

    assert_eq!(errors[0], errors[1]);
    assert_eq!(
        errors[0],
        "malformed lumin.json: contradictory invocation source role declarations for src/a.ts: generated conflicts with authored"
    );
    Ok(())
}

#[test]
fn workspace_object_form_selects_only_matching_package_roots()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("packages/a"))?;
    fs::create_dir_all(root.path().join("tools/b"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"root","workspaces":{"packages":["packages/*"]}}"#,
    )?;
    fs::write(
        root.path().join("packages/a/package.json"),
        r#"{"name":"package-a"}"#,
    )?;
    fs::write(
        root.path().join("tools/b/package.json"),
        r#"{"name":"tool-b"}"#,
    )?;

    let inventory = scan(root.path(), &InventoryRequest::default())?;
    let package_a = inventory
        .config
        .packages
        .iter()
        .find(|package| package.root.display_escaped() == "packages/a")
        .ok_or("package-a missing")?;
    let tool_b = inventory
        .config
        .packages
        .iter()
        .find(|package| package.root.display_escaped() == "tools/b")
        .ok_or("tool-b missing")?;

    assert_eq!(
        package_a
            .workspace_root
            .as_ref()
            .map(RepoPath::display_escaped),
        Some(String::new())
    );
    assert!(tool_b.workspace_root.is_none());
    Ok(())
}

#[test]
fn pnpm_membership_replaces_package_workspaces_and_applies_exclusions()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("packages/a"))?;
    fs::create_dir_all(root.path().join("tools/included"))?;
    fs::create_dir_all(root.path().join("tools/excluded"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"root","workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("pnpm-workspace.yaml"),
        "packages:\n  - '!tools/excluded'\n  - tools/**\n",
    )?;
    fs::write(
        root.path().join("packages/a/package.json"),
        r#"{"name":"package-a"}"#,
    )?;
    fs::write(
        root.path().join("tools/included/package.json"),
        r#"{"name":"included"}"#,
    )?;
    fs::write(
        root.path().join("tools/excluded/package.json"),
        r#"{"name":"excluded"}"#,
    )?;

    let inventory = scan(root.path(), &InventoryRequest::default())?;
    let package_a = inventory
        .config
        .packages
        .iter()
        .find(|package| package.root.display_escaped() == "packages/a")
        .ok_or("package-a missing")?;
    let included = inventory
        .config
        .packages
        .iter()
        .find(|package| package.root.display_escaped() == "tools/included")
        .ok_or("included package missing")?;
    let excluded = inventory
        .config
        .packages
        .iter()
        .find(|package| package.root.display_escaped() == "tools/excluded")
        .ok_or("excluded package missing")?;

    assert!(package_a.workspace_root.is_none());
    assert_eq!(
        included
            .workspace_root
            .as_ref()
            .map(RepoPath::display_escaped),
        Some(String::new())
    );
    assert!(excluded.workspace_root.is_none());
    assert!(inventory.limitations.is_empty());
    Ok(())
}

#[test]
fn pnpm_missing_packages_keeps_only_the_root_member() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("packages/a"))?;
    fs::write(root.path().join("package.json"), r#"{"name":"root"}"#)?;
    fs::write(root.path().join("pnpm-workspace.yaml"), "{}\n")?;
    fs::write(
        root.path().join("packages/a/package.json"),
        r#"{"name":"package-a"}"#,
    )?;

    let inventory = scan(root.path(), &InventoryRequest::default())?;
    let workspace = inventory
        .config
        .workspaces
        .iter()
        .find(|workspace| workspace.source == lumin_model::WorkspaceSource::PnpmWorkspace)
        .ok_or("pnpm workspace missing")?;
    assert_eq!(workspace.members, vec![RepoPath::empty()]);
    Ok(())
}

#[test]
fn pnpm_package_configs_forms_are_visible_typed_limitations()
-> Result<(), Box<dyn std::error::Error>> {
    for yaml in [
        "packageConfigs:\n  project-1:\n    saveExact: true\n",
        "packageConfigs:\n  - match: [project-1, project-2]\n    saveExact: true\n",
    ] {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("package.json"), r#"{"name":"root"}"#)?;
        fs::write(root.path().join("pnpm-workspace.yaml"), yaml)?;

        let inventory = scan(root.path(), &InventoryRequest::default())?;
        assert!(inventory.limitations.iter().any(|limitation| matches!(
            limitation,
            Limitation::PnpmDependencySemanticsUnsupported { path, .. }
                if path == "pnpm-workspace.yaml"
        )));
    }
    Ok(())
}

#[test]
fn malformed_pnpm_is_a_hard_stop_without_package_workspace_fallback()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("packages/a"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"root","workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("pnpm-workspace.yaml"),
        "packages: []\npackages: [packages/*]\n",
    )?;
    fs::write(
        root.path().join("packages/a/package.json"),
        r#"{"name":"package-a"}"#,
    )?;

    let result = scan(root.path(), &InventoryRequest::default());
    assert!(matches!(
        result,
        Err(InventoryError::MalformedConfiguration(_))
    ));
    Ok(())
}

#[test]
fn invocation_entries_replace_config_entries() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("lumin.json"),
        r#"{"schemaVersion":"lumin-config.v1","entries":["src/from-config.ts"]}"#,
    )?;
    fs::write(
        root.path().join("src/from-config.ts"),
        "export const a = 1;",
    )?;
    fs::write(
        root.path().join("src/from-invocation.ts"),
        "export const b = 2;",
    )?;

    // With invocation entries, config entries are replaced
    let invocation_path = RepoPath::from_portable("src/from-invocation.ts")?;
    let request = InventoryRequest {
        entries: vec![invocation_path.clone()],
        ..Default::default()
    };
    let inventory = scan(root.path(), &request)?;
    assert_eq!(inventory.entry_selections.len(), 1);
    assert_eq!(inventory.entry_selections[0].path, invocation_path);
    assert_eq!(
        inventory.entry_selections[0].source,
        lumin_model::EntrySource::Invocation
    );
    Ok(())
}

#[test]
fn config_entries_used_when_no_invocation_entries() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("lumin.json"),
        r#"{"schemaVersion":"lumin-config.v1","entries":["src/from-config.ts"]}"#,
    )?;
    fs::write(
        root.path().join("src/from-config.ts"),
        "export const a = 1;",
    )?;

    let request = InventoryRequest::default();
    let inventory = scan(root.path(), &request)?;
    assert_eq!(inventory.entry_selections.len(), 1);
    assert_eq!(
        inventory.entry_selections[0].path,
        RepoPath::from_portable("src/from-config.ts")?
    );
    assert_eq!(
        inventory.entry_selections[0].source,
        lumin_model::EntrySource::Configuration
    );
    Ok(())
}

#[test]
fn entry_dedup_preserves_lexical_order() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(root.path().join("src/b.ts"), "export const b = 1;")?;
    fs::write(root.path().join("src/a.ts"), "export const a = 1;")?;

    let request = InventoryRequest {
        entries: vec![
            RepoPath::from_portable("src/b.ts")?,
            RepoPath::from_portable("src/a.ts")?,
            RepoPath::from_portable("src/b.ts")?, // duplicate
        ],
        ..Default::default()
    };
    let inventory = scan(root.path(), &request)?;
    assert_eq!(inventory.entry_selections.len(), 2);
    // Sorted lexically
    assert!(
        inventory.entry_selections[0].path.display_escaped()
            <= inventory.entry_selections[1].path.display_escaped()
    );
    Ok(())
}

#[test]
fn valid_missing_entry_emits_typed_limitation() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let request = InventoryRequest {
        entries: vec![RepoPath::from_portable("src/missing.ts")?],
        ..Default::default()
    };
    let inventory = scan(root.path(), &request)?;
    // Entry is recorded but with unavailable_reason
    assert_eq!(inventory.entry_selections.len(), 1);
    assert_eq!(
        inventory.entry_selections[0].unavailable_reason,
        Some(EntryUnavailableReason::Missing)
    );
    assert!(inventory.limitations.iter().any(|limitation| matches!(
        limitation,
        Limitation::ExplicitEntryUnavailable {
            path,
            unavailable_reason: EntryUnavailableReason::Missing,
            ..
        } if path == "src/missing.ts"
    )));
    Ok(())
}

#[test]
fn excluded_entry_emits_typed_limitation() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("vendor"))?;
    fs::write(root.path().join("vendor/lib.ts"), "export const v = 1;")?;

    let request = InventoryRequest {
        entries: vec![RepoPath::from_portable("vendor/lib.ts")?],
        excludes: vec!["vendor/**".to_owned()],
        ..Default::default()
    };
    let inventory = scan(root.path(), &request)?;
    // Unavailable entries are still recorded with their reason
    assert_eq!(inventory.entry_selections.len(), 1);
    assert_eq!(
        inventory.entry_selections[0].unavailable_reason,
        Some(EntryUnavailableReason::Excluded)
    );
    assert!(inventory.limitations.iter().any(|limitation| matches!(
        limitation,
        Limitation::ExplicitEntryUnavailable {
            unavailable_reason: EntryUnavailableReason::Excluded,
            ..
        }
    )));
    Ok(())
}

#[test]
fn out_of_domain_entry_emits_typed_limitation() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("readme.md"), "# Hello")?;

    let request = InventoryRequest {
        entries: vec![RepoPath::from_portable("readme.md")?],
        ..Default::default()
    };
    let inventory = scan(root.path(), &request)?;
    assert_eq!(inventory.entry_selections.len(), 1);
    assert_eq!(
        inventory.entry_selections[0].unavailable_reason,
        Some(EntryUnavailableReason::OutOfDomain)
    );
    assert!(inventory.limitations.iter().any(|limitation| matches!(
        limitation,
        Limitation::ExplicitEntryUnavailable {
            unavailable_reason: EntryUnavailableReason::OutOfDomain,
            ..
        }
    )));
    Ok(())
}

#[test]
fn lumin_json_missing_policy_input_observed() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const a = 1;")?;

    let inventory = scan(root.path(), &InventoryRequest::default())?;
    let lumin_policy = inventory
        .policy_inputs
        .iter()
        .find(|input| input.path.display_escaped() == "lumin.json")
        .ok_or_else(|| std::io::Error::other("missing lumin.json policy input"))?;
    assert_eq!(lumin_policy.state, SemanticPolicyState::Missing);
    assert!(lumin_policy.payload_sha256.is_none());
    Ok(())
}

#[test]
fn lumin_json_present_policy_input_observed() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("lumin.json"),
        r#"{"schemaVersion":"lumin-config.v1"}"#,
    )?;
    fs::write(root.path().join("lib.ts"), "export const a = 1;")?;

    let inventory = scan(root.path(), &InventoryRequest::default())?;
    let lumin_policy = inventory
        .policy_inputs
        .iter()
        .find(|input| input.path.display_escaped() == "lumin.json")
        .ok_or_else(|| std::io::Error::other("missing lumin.json policy input"))?;
    assert_eq!(lumin_policy.state, SemanticPolicyState::Present);
    assert!(
        lumin_policy
            .payload_sha256
            .as_ref()
            .is_some_and(|s| !s.is_empty())
    );
    assert!(lumin_policy.physical_identity.is_some());
    Ok(())
}

#[test]
fn nested_gitignore_entry_is_ignored_not_available() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    // Scan policy applies repository-owned .gitignore files even without a .git marker.
    fs::create_dir_all(root.path().join("src"))?;
    // Create a nested .gitignore that ignores a specific file
    fs::write(root.path().join("src/.gitignore"), "ignored.ts\n")?;
    fs::write(
        root.path().join("src/ignored.ts"),
        "export const ignored = 1;",
    )?;
    fs::write(
        root.path().join("src/available.ts"),
        "export const available = 1;",
    )?;

    // An entry under src/.gitignore must be classified as Ignored
    let request = InventoryRequest {
        entries: vec![RepoPath::from_portable("src/ignored.ts")?],
        ..Default::default()
    };
    let inventory = scan(root.path(), &request)?;
    assert_eq!(inventory.entry_selections.len(), 1);
    assert_eq!(
        inventory.entry_selections[0].unavailable_reason,
        Some(EntryUnavailableReason::Ignored)
    );
    // The ignored file must NOT appear in sources (WalkBuilder respects .gitignore with .git)
    assert!(
        !inventory
            .sources
            .iter()
            .any(|s| s.path.display_escaped() == "src/ignored.ts")
    );
    // The nested .gitignore must appear in policy_inputs
    let nested = inventory
        .policy_inputs
        .iter()
        .find(|input| input.path.display_escaped() == "src/.gitignore")
        .ok_or_else(|| std::io::Error::other("missing nested .gitignore policy input"))?;
    assert_eq!(nested.state, SemanticPolicyState::Present);
    assert!(nested.payload_sha256.is_some());
    assert!(nested.physical_identity.is_some());
    Ok(())
}

#[test]
fn nested_gitignore_appears_in_semantic_input_records() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(root.path().join("src/.gitignore"), "*.log\n")?;
    fs::write(root.path().join("src/app.ts"), "export const app = 1;")?;

    let inventory = scan(root.path(), &InventoryRequest::default())?;
    // Verify the nested .gitignore is captured in policy_inputs
    let nested = inventory
        .policy_inputs
        .iter()
        .find(|input| input.path.display_escaped() == "src/.gitignore")
        .ok_or_else(|| std::io::Error::other("missing nested .gitignore policy input"))?;
    assert_eq!(nested.state, SemanticPolicyState::Present);
    assert!(nested.physical_identity.is_some());
    Ok(())
}

#[test]
fn explicit_include_reinclude_gitignored_entry() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    // Root .gitignore ignores dist/ files
    fs::write(root.path().join(".gitignore"), "dist/\n")?;
    fs::create_dir_all(root.path().join("dist"))?;
    fs::write(
        root.path().join("dist/output.ts"),
        "export const output = 1;",
    )?;
    fs::write(root.path().join("src/app.ts"), "export const app = 1;")?;

    // Without includes, entry under dist is Ignored
    let request_no_include = InventoryRequest {
        entries: vec![RepoPath::from_portable("dist/output.ts")?],
        ..Default::default()
    };
    let inv1 = scan(root.path(), &request_no_include)?;
    assert_eq!(
        inv1.entry_selections[0].unavailable_reason,
        Some(EntryUnavailableReason::Ignored)
    );

    // With explicit includes matching the path, entry is reincluded (Available)
    let request_with_include = InventoryRequest {
        entries: vec![RepoPath::from_portable("dist/output.ts")?],
        includes: vec!["dist/**".to_owned()],
        ..Default::default()
    };
    let inv2 = scan(root.path(), &request_with_include)?;
    assert_eq!(inv2.entry_selections.len(), 1);
    assert_eq!(inv2.entry_selections[0].unavailable_reason, None);
    assert!(
        inv2.sources
            .iter()
            .any(|source| source.path.display_escaped() == "dist/output.ts")
    );
    Ok(())
}

#[test]
fn entry_alone_does_not_override_gitignore() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("build"))?;
    fs::write(root.path().join(".gitignore"), "build/\n")?;
    fs::write(root.path().join("build/gen.ts"), "export const gen = 1;")?;

    // Entry pointing at a gitignored file without explicit include is still Ignored
    let request = InventoryRequest {
        entries: vec![RepoPath::from_portable("build/gen.ts")?],
        ..Default::default()
    };
    let inventory = scan(root.path(), &request)?;
    assert_eq!(inventory.entry_selections.len(), 1);
    assert_eq!(
        inventory.entry_selections[0].unavailable_reason,
        Some(EntryUnavailableReason::Ignored)
    );
    Ok(())
}

#[test]
fn includes_nonempty_nonmatching_entry_is_out_of_domain() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("other"))?;
    fs::write(root.path().join("src/app.ts"), "export const app = 1;")?;
    fs::write(root.path().join("other/lib.ts"), "export const lib = 1;")?;

    // includes only matches src/**, entry under other/ is OutOfDomain
    let request = InventoryRequest {
        entries: vec![RepoPath::from_portable("other/lib.ts")?],
        includes: vec!["src/**".to_owned()],
        ..Default::default()
    };
    let inventory = scan(root.path(), &request)?;
    assert_eq!(inventory.entry_selections.len(), 1);
    assert_eq!(
        inventory.entry_selections[0].unavailable_reason,
        Some(EntryUnavailableReason::OutOfDomain)
    );
    Ok(())
}

#[test]
fn validate_caller_entries_rejects_lumin_namespace() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let entry = RepoPath::from_portable(".lumin/state.ts")?;
    let result = validate_caller_entries(root.path(), &[entry]);
    assert!(matches!(result, Err(InventoryError::ReservedEntryPath(_))));
    Ok(())
}

#[test]
fn validate_caller_entries_accepts_normal_paths() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(root.path().join("src/app.ts"), "export const app = 1;")?;
    let entry = RepoPath::from_portable("src/app.ts")?;
    let result = validate_caller_entries(root.path(), &[entry]);
    assert!(result.is_ok());
    Ok(())
}

#[test]
fn validate_caller_entries_accepts_missing_path_under_existing_parent()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    let entry = RepoPath::from_portable("src/missing.ts")?;
    assert!(validate_caller_entries(root.path(), &[entry]).is_ok());
    Ok(())
}

#[cfg(unix)]
#[test]
fn validate_caller_entries_rejects_symlink_escape() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    fs::write(outside.path().join("escape.ts"), "export const x = 1;")?;
    std::os::unix::fs::symlink(
        outside.path().join("escape.ts"),
        root.path().join("escape.ts"),
    )?;
    let entry = RepoPath::from_portable("escape.ts")?;
    let result = validate_caller_entries(root.path(), &[entry]);
    assert!(matches!(result, Err(InventoryError::EntryEscapesRoot(_))));
    Ok(())
}

#[cfg(unix)]
#[test]
fn validate_caller_entries_rejects_parent_alias_escape_for_existing_and_missing_children()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    fs::write(outside.path().join("existing.ts"), "export const x = 1;")?;
    std::os::unix::fs::symlink(outside.path(), root.path().join("alias"))?;

    for portable in ["alias/existing.ts", "alias/missing.ts"] {
        let entry = RepoPath::from_portable(portable)?;
        assert!(matches!(
            validate_caller_entries(root.path(), &[entry]),
            Err(InventoryError::EntryEscapesRoot(_))
        ));
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn classify_entry_rejects_outside_root_symlink() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    fs::write(outside.path().join("link.ts"), "export const x = 1;")?;
    std::os::unix::fs::symlink(outside.path().join("link.ts"), root.path().join("link.ts"))?;
    let ignore = ApplicableIgnore::build(root.path())?;
    let patterns = PatternSet::compile(root.path(), None, &InventoryRequest::default())?;
    let path = RepoPath::from_portable("link.ts")?;
    let classification = classify_entry(root.path(), &path, &patterns, &ignore);
    assert!(matches!(
        classification,
        Err(InventoryError::EntryEscapesRoot(_))
    ));
    Ok(())
}

#[test]
fn unreadable_gitignore_returns_error() -> Result<(), Box<dyn std::error::Error>> {
    // On all platforms we test via a directory named .gitignore which cannot be read as a file
    // This is a portable seam: fs::read on a directory fails with an error.
    let root = tempfile::tempdir()?;
    // Create .gitignore as a directory (unreadable as file)
    fs::create_dir(root.path().join(".gitignore"))?;
    fs::write(root.path().join("app.ts"), "export const x = 1;")?;

    let result = ApplicableIgnore::build(root.path());
    // Should error because .gitignore exists but is unreadable (it's a directory)
    assert!(
        result.is_err(),
        "unreadable .gitignore should error, not silently swallow"
    );
    Ok(())
}

#[test]
fn root_config_symlink_is_malformed() -> Result<(), Box<dyn std::error::Error>> {
    let _root = tempfile::tempdir()?;
    // Create a regular file that we'll symlink to
    let _outside = tempfile::tempdir()?;
    #[cfg(unix)]
    {
        fs::write(
            _outside.path().join("lumin.json"),
            r#"{"schemaVersion":"lumin-config.v1"}"#,
        )?;
        std::os::unix::fs::symlink(
            _outside.path().join("lumin.json"),
            _root.path().join("lumin.json"),
        )?;
        let result = scan(_root.path(), &InventoryRequest::default());
        assert!(
            matches!(result, Err(InventoryError::MalformedConfiguration(_))),
            "symlink lumin.json should be rejected as malformed"
        );
    }
    Ok(())
}
