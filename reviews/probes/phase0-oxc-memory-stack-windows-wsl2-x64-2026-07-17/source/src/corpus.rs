use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};

use crate::{
    model::{CorpusEntry, CorpusManifest},
    util::sha256,
};

const MANIFEST_SCHEMA: &str = "lumin-phase0-oxc-corpus-v1";
const CORPUS_ID: &str = "lumin-lab-35290cb-plus-stack-stress-v1";
const SOURCE_REPOSITORY: &str = "https://github.com/annyeong844/lumin_lab.git";
const SOURCE_COMMIT: &str = "35290cb683a37e83bc8c915d1d0f9ca0f3f96fd0";
const SOURCE_EXTENSIONS: [&str; 6] = ["js", "jsx", "mjs", "cjs", "ts", "tsx"];

pub struct ValidatedCorpus {
    pub manifest: CorpusManifest,
    pub manifest_sha256: String,
    pub entries: Vec<CorpusEntry>,
    pub total_bytes: u64,
}

pub fn load_and_validate(root: &Path, manifest_path: &Path) -> Result<ValidatedCorpus> {
    let manifest_bytes = fs::read(manifest_path)
        .with_context(|| format!("read corpus manifest {}", manifest_path.display()))?;
    let manifest: CorpusManifest = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("parse corpus manifest {}", manifest_path.display()))?;
    ensure!(
        manifest.schema == MANIFEST_SCHEMA,
        "unsupported corpus manifest schema"
    );
    ensure!(
        manifest.corpus_id == CORPUS_ID,
        "unexpected corpus identity"
    );
    ensure!(
        manifest.source_repository == SOURCE_REPOSITORY,
        "unexpected corpus source repository"
    );
    ensure!(
        manifest.source_commit == SOURCE_COMMIT,
        "unexpected corpus source commit"
    );
    ensure!(
        manifest.generator_sha256 == sha256(include_bytes!("../scripts/prepare-corpus.py")),
        "corpus generator identity mismatch"
    );
    ensure!(!manifest.entries.is_empty(), "corpus manifest is empty");

    let mut previous: Option<&str> = None;
    let mut expected_paths = BTreeSet::new();
    let mut total_bytes = 0_u64;
    for entry in &manifest.entries {
        validate_relative_path(&entry.path)?;
        if let Some(previous) = previous {
            ensure!(
                previous < entry.path.as_str(),
                "manifest paths are not strictly ordered"
            );
        }
        previous = Some(&entry.path);
        ensure!(
            expected_paths.insert(entry.path.clone()),
            "duplicate corpus path {}",
            entry.path
        );
        let path = root.join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("inspect corpus file {}", path.display()))?;
        ensure!(
            metadata.file_type().is_file(),
            "corpus entry is not a regular file: {}",
            entry.path
        );
        ensure!(
            metadata.len() == entry.bytes,
            "corpus byte count mismatch: {}",
            entry.path
        );
        let bytes = fs::read(&path)?;
        ensure!(
            sha256(&bytes) == entry.sha256,
            "corpus SHA-256 mismatch: {}",
            entry.path
        );
        total_bytes = total_bytes
            .checked_add(entry.bytes)
            .context("corpus byte total overflow")?;
    }

    let actual_paths = enumerate_source_paths(root)?;
    ensure!(
        actual_paths == expected_paths,
        "corpus source set differs from manifest"
    );
    ensure!(
        manifest.legacy_file_count + manifest.synthetic_file_count == manifest.entries.len(),
        "manifest file totals disagree"
    );
    ensure!(
        manifest.legacy_bytes + manifest.synthetic_bytes == total_bytes,
        "manifest byte totals disagree"
    );

    let entries = manifest.entries.clone();
    Ok(ValidatedCorpus {
        manifest,
        manifest_sha256: sha256(&manifest_bytes),
        entries,
        total_bytes,
    })
}

fn validate_relative_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && !path.contains('\\'),
        "noncanonical corpus path {path:?}"
    );
    let candidate = Path::new(path);
    ensure!(!candidate.is_absolute(), "absolute corpus path {path:?}");
    for component in candidate.components() {
        if !matches!(component, Component::Normal(_)) {
            bail!("noncanonical corpus component in {path:?}");
        }
    }
    let extension = candidate
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    ensure!(
        SOURCE_EXTENSIONS.contains(&extension),
        "unsupported corpus extension in {path:?}"
    );
    Ok(())
}

fn enumerate_source_paths(root: &Path) -> Result<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {
                bail!(
                    "corpus contains a symbolic link: {}",
                    entry.path().display()
                );
            }
            if metadata.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !metadata.is_file() {
                bail!(
                    "corpus contains a nonregular object: {}",
                    entry.path().display()
                );
            }
            let path = entry.path();
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if SOURCE_EXTENSIONS.contains(&extension) {
                paths.insert(relative_portable(root, &path)?);
            }
        }
    }
    Ok(paths)
}

fn relative_portable(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root)?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => parts.push(
                value
                    .to_str()
                    .context("corpus path is not Unicode-representable")?
                    .to_owned(),
            ),
            _ => bail!("noncanonical corpus path {}", path.display()),
        }
    }
    Ok(parts.join("/"))
}

pub fn entry_path(root: &Path, entry: &CorpusEntry) -> PathBuf {
    root.join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR))
}

#[cfg(test)]
mod tests {
    use super::validate_relative_path;

    #[test]
    fn rejects_noncanonical_paths() {
        assert!(validate_relative_path("../escape.ts").is_err());
        assert!(validate_relative_path("root\\file.ts").is_err());
        assert!(validate_relative_path("root/file.rs").is_err());
        assert!(validate_relative_path("root/file.ts").is_ok());
    }
}
