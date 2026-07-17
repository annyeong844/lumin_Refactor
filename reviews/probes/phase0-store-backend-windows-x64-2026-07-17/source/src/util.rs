use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::model::SourceFileHash;

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

pub fn source_hashes() -> Result<Vec<SourceFileHash>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let relative_paths = [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "rustfmt.toml",
        "README.md",
        "PROBE-CONTRACT.md",
        "scripts/collect-build-metrics.ps1",
        "scripts/package-evidence.ps1",
        "src/main.rs",
        "src/lifecycle.rs",
        "src/namespace.rs",
        "src/model.rs",
        "src/runner.rs",
        "src/util.rs",
        "src/backend/mod.rs",
        "src/backend_contract.rs",
        "src/benchmark.rs",
        "src/backend/redb_backend.rs",
        "src/backend/sqlite_backend.rs",
    ];
    relative_paths
        .into_iter()
        .map(|relative| {
            let path = root.join(relative);
            let bytes = fs::read(&path)
                .with_context(|| format!("read source file for hashing {}", path.display()))?;
            Ok(SourceFileHash {
                path: relative.replace('\\', "/"),
                sha256: format!("{:x}", Sha256::digest(bytes)),
            })
        })
        .collect()
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
