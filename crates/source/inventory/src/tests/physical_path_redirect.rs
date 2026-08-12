use std::fs;
use std::path::Path;

use lumin_model::{PhysicalPathRedirectKind, PhysicalPathRedirectTarget, RepoPath};

use super::super::{InventoryRequest, scan};

#[test]
fn scan_records_directory_redirect_entry_and_target_identity_without_following()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let outside_after = tempfile::tempdir()?;
    let package = root.path().join("packages").join("lib");
    let inside = package.join("inside");
    let local_link = package.join("local-link");
    let escape = package.join("escape");
    let old_escape = package.join("old-escape");
    fs::create_dir_all(&inside)?;
    create_directory_redirect(&local_link, &inside)?;
    create_directory_redirect(&escape, outside.path())?;

    let before = scan(root.path(), &InventoryRequest::default())?;
    let escape_path = RepoPath::from_portable("packages/lib/escape")?;
    let local_path = RepoPath::from_portable("packages/lib/local-link")?;
    let escape_before = redirect(&before.physical_path_redirects, &escape_path, "outside")?;
    assert_eq!(
        escape_before.target,
        PhysicalPathRedirectTarget::OutsideRepository
    );
    assert_eq!(escape_before.kind, PhysicalPathRedirectKind::Directory);
    assert!(escape_before.entry_physical_identity.is_some());
    assert!(escape_before.target_physical_identity.is_some());
    assert!(!escape_before.target_identity_sha256.is_empty());
    let local = redirect(&before.physical_path_redirects, &local_path, "inside")?;
    assert_eq!(
        local.target,
        PhysicalPathRedirectTarget::Repository(RepoPath::from_portable("packages/lib/inside")?)
    );
    assert_eq!(local.kind, PhysicalPathRedirectKind::Directory);

    fs::rename(&escape, &old_escape)?;
    create_directory_redirect(&escape, outside.path())?;
    let same_target = scan(root.path(), &InventoryRequest::default())?;
    let escape_same_target = redirect(
        &same_target.physical_path_redirects,
        &escape_path,
        "same-target replacement",
    )?;
    assert_eq!(
        escape_before.target_physical_identity,
        escape_same_target.target_physical_identity
    );
    assert_ne!(
        escape_before.entry_physical_identity,
        escape_same_target.entry_physical_identity
    );
    assert_ne!(
        escape_before.semantic_sha256(),
        escape_same_target.semantic_sha256(),
        "replacing a redirect entry must change semantic identity even with the same target",
    );

    remove_directory_redirect(&escape)?;
    create_directory_redirect(&escape, outside_after.path())?;
    let after = scan(root.path(), &InventoryRequest::default())?;
    let escape_after = redirect(&after.physical_path_redirects, &escape_path, "retargeted")?;
    assert_eq!(
        escape_after.target,
        PhysicalPathRedirectTarget::OutsideRepository
    );
    assert_ne!(
        escape_same_target.semantic_sha256(),
        escape_after.semantic_sha256(),
        "same-category redirect retargeting must change semantic identity",
    );

    remove_directory_redirect(&escape)?;
    remove_directory_redirect(&old_escape)?;
    remove_directory_redirect(&local_link)?;
    Ok(())
}

fn redirect<'a>(
    redirects: &'a [lumin_model::PhysicalPathRedirect],
    path: &RepoPath,
    label: &str,
) -> Result<&'a lumin_model::PhysicalPathRedirect, std::io::Error> {
    redirects
        .iter()
        .find(|redirect| redirect.path == *path)
        .ok_or_else(|| std::io::Error::other(format!("{label} redirect fact is missing")))
}

#[cfg(unix)]
fn create_directory_redirect(path: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}

#[cfg(windows)]
fn create_directory_redirect(path: &Path, target: &Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(path)
        .arg(target)
        .status()?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| std::io::Error::other(format!("mklink /J exited with {status}")))
}

#[cfg(unix)]
fn remove_directory_redirect(path: &Path) -> std::io::Result<()> {
    fs::remove_file(path)
}

#[cfg(windows)]
fn remove_directory_redirect(path: &Path) -> std::io::Result<()> {
    fs::remove_dir(path)
}
