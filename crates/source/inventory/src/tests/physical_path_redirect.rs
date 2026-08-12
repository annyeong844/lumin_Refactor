#[cfg(unix)]
use std::fs;

#[cfg(unix)]
use lumin_model::{PhysicalPathRedirectTarget, RepoPath};

#[cfg(unix)]
use super::super::{InventoryRequest, scan};

#[cfg(unix)]
#[test]
fn scan_records_directory_redirect_targets_without_following_them()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let outside_after = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("packages/lib/inside"))?;
    std::os::unix::fs::symlink(
        root.path().join("packages/lib/inside"),
        root.path().join("packages/lib/local-link"),
    )?;
    std::os::unix::fs::symlink(outside.path(), root.path().join("packages/lib/escape"))?;

    let before = scan(root.path(), &InventoryRequest::default())?;
    let escape_path = RepoPath::from_portable("packages/lib/escape")?;
    let local_path = RepoPath::from_portable("packages/lib/local-link")?;
    let escape_before = before
        .physical_path_redirects
        .iter()
        .find(|redirect| redirect.path == escape_path)
        .ok_or_else(|| std::io::Error::other("outside redirect fact is missing"))?;
    assert_eq!(
        escape_before.target,
        PhysicalPathRedirectTarget::OutsideRepository
    );
    assert!(!escape_before.target_identity_sha256.is_empty());
    let local = before
        .physical_path_redirects
        .iter()
        .find(|redirect| redirect.path == local_path)
        .ok_or_else(|| std::io::Error::other("inside redirect fact is missing"))?;
    assert_eq!(
        local.target,
        PhysicalPathRedirectTarget::Repository(RepoPath::from_portable("packages/lib/inside")?)
    );

    fs::remove_file(root.path().join("packages/lib/escape"))?;
    std::os::unix::fs::symlink(
        outside_after.path(),
        root.path().join("packages/lib/escape"),
    )?;
    let after = scan(root.path(), &InventoryRequest::default())?;
    let escape_after = after
        .physical_path_redirects
        .iter()
        .find(|redirect| redirect.path == escape_path)
        .ok_or_else(|| std::io::Error::other("retargeted redirect fact is missing"))?;
    assert_eq!(
        escape_after.target,
        PhysicalPathRedirectTarget::OutsideRepository
    );
    assert_ne!(
        escape_before.semantic_sha256(),
        escape_after.semantic_sha256(),
        "same-category redirect retargeting must change semantic identity",
    );
    Ok(())
}
