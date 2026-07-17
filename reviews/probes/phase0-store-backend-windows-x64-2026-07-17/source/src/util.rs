use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::model::{ExecutableIdentity, SourceFileHash};

const EMBEDDED_SOURCE_FILES: [(&str, &[u8]); 19] = [
    ("Cargo.toml", include_bytes!("../Cargo.toml")),
    ("Cargo.lock", include_bytes!("../Cargo.lock")),
    (
        "rust-toolchain.toml",
        include_bytes!("../rust-toolchain.toml"),
    ),
    ("rustfmt.toml", include_bytes!("../rustfmt.toml")),
    ("README.md", include_bytes!("../README.md")),
    ("PROBE-CONTRACT.md", include_bytes!("../PROBE-CONTRACT.md")),
    (
        "scripts/collect-build-metrics.ps1",
        include_bytes!("../scripts/collect-build-metrics.ps1"),
    ),
    (
        "scripts/package-evidence.ps1",
        include_bytes!("../scripts/package-evidence.ps1"),
    ),
    ("src/main.rs", include_bytes!("main.rs")),
    ("src/lifecycle.rs", include_bytes!("lifecycle.rs")),
    ("src/namespace.rs", include_bytes!("namespace.rs")),
    ("src/model.rs", include_bytes!("model.rs")),
    ("src/runner.rs", include_bytes!("runner.rs")),
    ("src/util.rs", include_bytes!("util.rs")),
    ("src/backend/mod.rs", include_bytes!("backend/mod.rs")),
    (
        "src/backend_contract.rs",
        include_bytes!("backend_contract.rs"),
    ),
    ("src/benchmark.rs", include_bytes!("benchmark.rs")),
    (
        "src/backend/redb_backend.rs",
        include_bytes!("backend/redb_backend.rs"),
    ),
    (
        "src/backend/sqlite_backend.rs",
        include_bytes!("backend/sqlite_backend.rs"),
    ),
];

pub fn unix_millis() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_millis())
}

pub fn create_durable_marker(path: &Path, value: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create marker {}", path.display()))?;
    file.write_all(value)
        .with_context(|| format!("write marker {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("flush marker {}", path.display()))?;
    sync_parent(path)
}

pub fn write_json_durable<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create output directory {}", parent.display()))?;
    let temp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value).context("serialize JSON")?;
    {
        let mut file = File::create(&temp)
            .with_context(|| format!("create temporary output {}", temp.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("write temporary output {}", temp.display()))?;
        file.write_all(b"\n")?;
        file.sync_all()
            .with_context(|| format!("flush temporary output {}", temp.display()))?;
    }
    atomic_replace_file(&temp, path)?;
    sync_parent(path)
}

pub fn atomic_replace_file(source: &Path, destination: &Path) -> Result<()> {
    atomic_replace_file_platform(source, destination).with_context(|| {
        format!(
            "atomically replace {} with {}",
            destination.display(),
            source.display()
        )
    })
}

#[cfg(not(windows))]
fn atomic_replace_file_platform(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination)?;
    Ok(())
}

#[cfg(windows)]
fn atomic_replace_file_platform(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both UTF-16 buffers are NUL-terminated and remain alive for the call.
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

pub fn wait_for_path(path: &Path, timeout: Duration) -> Result<()> {
    let started = Instant::now();
    while !path.exists() {
        if started.elapsed() >= timeout {
            bail!("watchdog expired waiting for {}", path.display());
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

pub fn wait_forever() -> ! {
    loop {
        thread::park_timeout(Duration::from_secs(60));
    }
}

pub fn source_hashes() -> Vec<SourceFileHash> {
    EMBEDDED_SOURCE_FILES
        .into_iter()
        .map(|(path, bytes)| SourceFileHash {
            path: path.to_owned(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
        })
        .collect()
}

pub fn source_manifest_sha256(source_files: &[SourceFileHash]) -> String {
    let mut lines = source_files
        .iter()
        .map(|source| format!("{}  {}\n", source.sha256, source.path))
        .collect::<Vec<_>>();
    lines.sort_unstable();
    format!("{:x}", Sha256::digest(lines.concat().as_bytes()))
}

pub fn executable_identity() -> Result<ExecutableIdentity> {
    let path = std::env::current_exe().context("resolve running executable")?;
    let bytes =
        fs::read(&path).with_context(|| format!("read running executable {}", path.display()))?;
    Ok(ExecutableIdentity {
        path,
        bytes: u64::try_from(bytes.len()).context("executable size exceeds u64")?,
        sha256: format!("{:x}", Sha256::digest(bytes)),
    })
}

pub fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("create copied directory {}", destination.display()))?;
    for entry in fs::read_dir(source)
        .with_context(|| format!("read copied directory {}", source.display()))?
    {
        let entry = entry?;
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&entry.path(), &destination_path)?;
        } else {
            fs::copy(entry.path(), &destination_path).with_context(|| {
                format!("copy fixture file into {}", destination_path.display())
            })?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent", path.display()))?;
    let directory = File::open(parent)
        .with_context(|| format!("open parent directory {}", parent.display()))?;
    directory
        .sync_all()
        .with_context(|| format!("flush parent directory {}", parent.display()))
}

#[cfg(windows)]
fn sync_parent(_path: &Path) -> Result<()> {
    // Parent-directory durability is a later platform probe. These files are
    // test coordination/evidence artifacts, not canonical Lumin state.
    Ok(())
}
