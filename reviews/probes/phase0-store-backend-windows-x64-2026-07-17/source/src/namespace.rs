use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::backend;
use crate::model::{BackendKind, FaultCaseResult};
use crate::util::{create_durable_marker, wait_for_path, write_json_durable};

const PARENT_KINDS: [&str; 4] = ["attempts", "runs", "trash", "cache"];

pub const NAMESPACE_FAULTS: [&str; 14] = [
    "state-directory-copy-swap",
    "lifecycle-lock-replacement",
    "lifecycle-lock-content-mutation",
    "lifecycle-lock-extra-link",
    "attempts-parent-replacement",
    "runs-parent-replacement",
    "trash-parent-replacement",
    "cache-parent-replacement",
    "attempts-anchor-replacement",
    "runs-anchor-replacement",
    "trash-anchor-replacement",
    "cache-anchor-replacement",
    "runs-anchor-content-mutation",
    "runs-anchor-extra-link",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PhysicalIdentity {
    volume: u64,
    file: u64,
    links: u64,
    kind: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ParentBinding {
    directory: PhysicalIdentity,
    anchor: PhysicalIdentity,
    nonce: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct NamespaceMarker {
    state_directory: PhysicalIdentity,
    lifecycle_lock: PhysicalIdentity,
    namespace_nonce: String,
    parents: BTreeMap<String, ParentBinding>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StoreHeader {
    marker_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct NamespaceChildResult {
    hard_stop: bool,
    diagnostic: String,
    canonical_mutation_written: bool,
}

pub fn run_namespace_cases(
    backend_kind: BackendKind,
    watchdog: Duration,
    backend_root: &Path,
) -> Vec<FaultCaseResult> {
    NAMESPACE_FAULTS
        .into_iter()
        .map(|fault| {
            let started = Instant::now();
            match run_namespace_case(backend_kind, fault, watchdog, backend_root) {
                Ok(observation) => FaultCaseResult {
                    domain: "namespace".to_owned(),
                    crash_point: fault.to_owned(),
                    status: "PASS".to_owned(),
                    error: None,
                    elapsed_micros: started.elapsed().as_micros(),
                    observation,
                },
                Err(error) => FaultCaseResult {
                    domain: "namespace".to_owned(),
                    crash_point: fault.to_owned(),
                    status: "FAIL".to_owned(),
                    error: Some(format!("{error:#}")),
                    elapsed_micros: started.elapsed().as_micros(),
                    observation: serde_json::Value::Null,
                },
            }
        })
        .collect()
}

pub fn run_namespace_child(
    backend_kind: BackendKind,
    repository_root: &Path,
    ready: &Path,
    go: &Path,
    result: &Path,
    watchdog: Duration,
) -> Result<()> {
    let token = verify_namespace(backend_kind, repository_root)?;
    create_durable_marker(ready, b"validated\n")?;
    wait_for_path(go, watchdog)?;

    let mutation = state_directory(repository_root).join("canonical-mutation.must-not-exist");
    let child_result = match verify_namespace(backend_kind, repository_root) {
        Ok(current) if current == token => {
            fs::write(&mutation, b"invalid-success")?;
            NamespaceChildResult {
                hard_stop: false,
                diagnostic: "replacement was not detected".to_owned(),
                canonical_mutation_written: true,
            }
        }
        Ok(_) => NamespaceChildResult {
            hard_stop: true,
            diagnostic: "binding token changed".to_owned(),
            canonical_mutation_written: false,
        },
        Err(error) => NamespaceChildResult {
            hard_stop: true,
            diagnostic: format!("{error:#}"),
            canonical_mutation_written: false,
        },
    };
    write_json_durable(result, &child_result)
}

fn run_namespace_case(
    backend_kind: BackendKind,
    fault: &str,
    watchdog: Duration,
    backend_root: &Path,
) -> Result<serde_json::Value> {
    let repository_root = backend_root.join("namespace").join(fault);
    if repository_root.exists() {
        fs::remove_dir_all(&repository_root)?;
    }
    fs::create_dir_all(&repository_root)?;
    initialize_namespace(backend_kind, &repository_root)?;

    let coordination = repository_root.join("coordination");
    fs::create_dir_all(&coordination)?;
    let ready = coordination.join("ready");
    let go = coordination.join("go");
    let result = coordination.join("result.json");
    let mut child = Command::new(env::current_exe()?)
        .arg("child-namespace")
        .arg("--backend")
        .arg(backend_kind.to_string())
        .arg("--repository-root")
        .arg(&repository_root)
        .arg("--ready")
        .arg(&ready)
        .arg("--go")
        .arg(&go)
        .arg("--result")
        .arg(&result)
        .arg("--watchdog-ms")
        .arg(watchdog.as_millis().to_string())
        .spawn()
        .context("spawn namespace verifier child")?;
    wait_for_path(&ready, watchdog)?;
    inject_namespace_fault(&repository_root, fault)?;
    create_durable_marker(&go, b"go\n")?;
    let status = wait_for_child(&mut child, watchdog)?;
    ensure!(status.success(), "namespace child failed with {status}");

    let child_result: NamespaceChildResult = serde_json::from_slice(&fs::read(&result)?)?;
    ensure!(child_result.hard_stop, "namespace replacement was accepted");
    ensure!(!child_result.canonical_mutation_written);
    ensure!(
        !state_directory(&repository_root)
            .join("canonical-mutation.must-not-exist")
            .exists()
    );
    Ok(serde_json::to_value(child_result)?)
}

fn initialize_namespace(backend_kind: BackendKind, repository_root: &Path) -> Result<()> {
    let state = state_directory(repository_root);
    fs::create_dir_all(&state)?;
    let lock = state.join("lifecycle.lock");
    fs::write(&lock, b"immutable-lifecycle-lock\n")?;
    let namespace_nonce = unique_nonce()?;
    for kind in PARENT_KINDS {
        let parent = state.join(kind);
        fs::create_dir_all(&parent)?;
        fs::write(
            parent.join("namespace.anchor"),
            format!("{namespace_nonce}:{kind}\n"),
        )?;
    }
    let marker = capture_marker(&state, &lock, &namespace_nonce)?;
    let marker_bytes = serde_json::to_vec(&marker)?;
    write_json_durable(&state.join("repository.json"), &marker)?;
    let header = StoreHeader {
        marker_sha256: format!("{:x}", Sha256::digest(&marker_bytes)),
    };
    let database = state.join("lifecycle.store");
    backend::initialize(backend_kind, &database)?;
    let header_bytes = serde_json::to_vec(&header)?;
    ensure!(backend::compare_exchange_catalog(
        backend_kind,
        &database,
        None,
        &header_bytes
    )?);
    verify_namespace(backend_kind, repository_root)?;
    Ok(())
}

fn verify_namespace(backend_kind: BackendKind, repository_root: &Path) -> Result<NamespaceMarker> {
    let state = state_directory(repository_root);
    let marker_path = state.join("repository.json");
    let marker_bytes =
        fs::read(&marker_path).with_context(|| format!("read marker {}", marker_path.display()))?;
    let marker: NamespaceMarker = serde_json::from_slice(&marker_bytes)?;
    let current = capture_marker(
        &state,
        &state.join("lifecycle.lock"),
        &marker.namespace_nonce,
    )?;
    ensure!(current == marker, "namespace physical binding mismatch");
    let database = state.join("lifecycle.store");
    let header_bytes = backend::read_catalog(backend_kind, &database)?
        .context("store namespace header is missing")?;
    let header: StoreHeader = serde_json::from_slice(&header_bytes)?;
    let canonical_marker = serde_json::to_vec(&marker)?;
    ensure!(
        header.marker_sha256 == format!("{:x}", Sha256::digest(canonical_marker)),
        "store header disagrees with namespace marker"
    );
    Ok(marker)
}

fn capture_marker(state: &Path, lock: &Path, nonce: &str) -> Result<NamespaceMarker> {
    let state_identity = physical_identity(state)?;
    ensure!(
        state_identity.kind == "directory",
        "state object is not a directory"
    );
    let lock_identity = physical_identity(lock)?;
    ensure!(
        lock_identity.kind == "file",
        "lifecycle lock is not a regular file"
    );
    ensure!(lock_identity.links == 1, "lifecycle lock has extra links");
    ensure!(
        fs::read(lock)? == b"immutable-lifecycle-lock\n",
        "lifecycle lock header changed"
    );
    let mut parents = BTreeMap::new();
    for kind in PARENT_KINDS {
        let parent = state.join(kind);
        let directory_identity = physical_identity(&parent)?;
        ensure!(
            directory_identity.kind == "directory",
            "managed parent {kind} is not a directory"
        );
        let anchor_path = parent.join("namespace.anchor");
        let anchor_identity = physical_identity(&anchor_path)?;
        ensure!(
            anchor_identity.kind == "file",
            "managed parent {kind} anchor is not a regular file"
        );
        ensure!(
            anchor_identity.links == 1,
            "managed parent {kind} anchor has extra links"
        );
        let expected_header = format!("{nonce}:{kind}\n");
        ensure!(
            fs::read(&anchor_path)? == expected_header.as_bytes(),
            "managed parent {kind} anchor header changed"
        );
        parents.insert(
            kind.to_owned(),
            ParentBinding {
                directory: directory_identity,
                anchor: anchor_identity,
                nonce: format!("{nonce}:{kind}"),
            },
        );
    }
    Ok(NamespaceMarker {
        state_directory: state_identity,
        lifecycle_lock: lock_identity,
        namespace_nonce: nonce.to_owned(),
        parents,
    })
}

fn inject_namespace_fault(repository_root: &Path, fault: &str) -> Result<()> {
    let state = state_directory(repository_root);
    match fault {
        "state-directory-copy-swap" => replace_directory_with_copy(&state),
        "lifecycle-lock-replacement" => replace_file_with_copy(&state.join("lifecycle.lock")),
        "lifecycle-lock-content-mutation" => {
            fs::write(state.join("lifecycle.lock"), b"mutated-lifecycle-lock\n")?;
            Ok(())
        }
        "lifecycle-lock-extra-link" => {
            fs::hard_link(
                state.join("lifecycle.lock"),
                state.join("lifecycle.lock.extra-link"),
            )?;
            Ok(())
        }
        "runs-anchor-content-mutation" => {
            fs::write(
                state.join("runs").join("namespace.anchor"),
                b"mutated-anchor\n",
            )?;
            Ok(())
        }
        "runs-anchor-extra-link" => {
            fs::hard_link(
                state.join("runs").join("namespace.anchor"),
                state.join("runs").join("namespace.anchor.extra-link"),
            )?;
            Ok(())
        }
        value if value.ends_with("-parent-replacement") => {
            let kind = value.trim_end_matches("-parent-replacement");
            ensure!(PARENT_KINDS.contains(&kind));
            replace_directory_with_copy(&state.join(kind))
        }
        value if value.ends_with("-anchor-replacement") => {
            let kind = value.trim_end_matches("-anchor-replacement");
            ensure!(PARENT_KINDS.contains(&kind));
            replace_file_with_copy(&state.join(kind).join("namespace.anchor"))
        }
        _ => bail!("unknown namespace fault {fault}"),
    }
}

fn replace_directory_with_copy(path: &Path) -> Result<()> {
    let backup = sibling_backup(path);
    fs::rename(path, &backup)?;
    copy_directory(&backup, path)
}

fn replace_file_with_copy(path: &Path) -> Result<()> {
    let backup = sibling_backup(path);
    fs::rename(path, &backup)?;
    fs::copy(&backup, path)?;
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn sibling_backup(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        "{}.replaced",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("object")
    ))
}

fn state_directory(repository_root: &Path) -> PathBuf {
    repository_root.join(".lumin")
}

fn unique_nonce() -> Result<String> {
    Ok(format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ))
}

fn wait_for_child(child: &mut Child, watchdog: Duration) -> Result<ExitStatus> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if started.elapsed() >= watchdog {
            child.kill()?;
            child.wait()?;
            bail!("namespace child watchdog expired");
        }
        thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(windows)]
fn physical_identity(path: &Path) -> Result<PhysicalIdentity> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        GetFileInformationByHandle, OPEN_EXISTING,
    };

    let wide_path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: the path buffer is NUL-terminated and valid for the duration of the call.
    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "open state object without following links: {}",
                path.display()
            )
        });
    }
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: handle is valid and information points to writable initialized storage.
    let result = unsafe { GetFileInformationByHandle(handle, &mut information) };
    // SAFETY: handle was returned by CreateFileW and is closed exactly once here.
    let close_result = unsafe { CloseHandle(handle) };
    if result == 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("read physical identity: {}", path.display()));
    }
    ensure!(close_result != 0, "failed to close state-object handle");
    ensure!(
        information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "reparse-point state object is forbidden: {}",
        path.display()
    );
    let kind = if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        "directory"
    } else {
        "file"
    };
    Ok(PhysicalIdentity {
        volume: information.dwVolumeSerialNumber as u64,
        file: ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64,
        links: information.nNumberOfLinks as u64,
        kind: kind.to_owned(),
    })
}

#[cfg(unix)]
fn physical_identity(path: &Path) -> Result<PhysicalIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(path)?;
    ensure!(!metadata.file_type().is_symlink());
    let kind = if metadata.is_dir() {
        "directory"
    } else {
        "file"
    };
    Ok(PhysicalIdentity {
        volume: metadata.dev(),
        file: metadata.ino(),
        links: metadata.nlink(),
        kind: kind.to_owned(),
    })
}
