use std::{fs, path::Path};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::model::{ExecutableIdentity, SourceFileHash};

const SOURCE_FILES: [(&str, &[u8]); 16] = [
    (".gitignore", include_bytes!("../.gitignore")),
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
        "scripts/prepare-corpus.py",
        include_bytes!("../scripts/prepare-corpus.py"),
    ),
    (
        "scripts/run-matrix.py",
        include_bytes!("../scripts/run-matrix.py"),
    ),
    (
        "scripts/package-evidence.py",
        include_bytes!("../scripts/package-evidence.py"),
    ),
    ("src/main.rs", include_bytes!("main.rs")),
    ("src/model.rs", include_bytes!("model.rs")),
    ("src/corpus.rs", include_bytes!("corpus.rs")),
    ("src/probe.rs", include_bytes!("probe.rs")),
    ("src/memory.rs", include_bytes!("memory.rs")),
    ("src/util.rs", include_bytes!("util.rs")),
];

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn source_hashes() -> Vec<SourceFileHash> {
    SOURCE_FILES
        .into_iter()
        .map(|(path, bytes)| SourceFileHash {
            path: path.to_owned(),
            sha256: sha256(bytes),
        })
        .collect()
}

pub fn source_manifest_sha256(files: &[SourceFileHash]) -> String {
    let mut lines = files
        .iter()
        .map(|file| format!("{}  {}\n", file.sha256, file.path))
        .collect::<Vec<_>>();
    lines.sort_unstable();
    sha256(lines.concat().as_bytes())
}

pub fn executable_identity() -> Result<ExecutableIdentity> {
    let path = std::env::current_exe().context("resolve current executable")?;
    let bytes = fs::read(&path).with_context(|| format!("read executable {}", path.display()))?;
    Ok(ExecutableIdentity {
        path: path.display().to_string(),
        bytes: u64::try_from(bytes.len())?,
        sha256: sha256(&bytes),
    })
}

pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().context("output path has no parent")?;
    fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec_pretty(value)?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temp, [&bytes[..], b"\n"].concat())?;
    fs::rename(&temp, path)?;
    Ok(())
}
