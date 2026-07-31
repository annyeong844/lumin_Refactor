use super::*;

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
