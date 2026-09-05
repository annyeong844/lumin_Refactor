//! A create-new, out-of-scratch evidence packet. Initial inventory survives abrupt exit;
//! a final manifest is authoritative only when every expected cell completed.
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) struct CaptureArchive {
    pub(super) root: PathBuf,
    cells: BTreeMap<String, Value>,
    active: Option<String>,
}

impl CaptureArchive {
    pub(super) fn from_environment(
        workspace: &Path,
        package: &Path,
        scratch: &Path,
        expected: &[String],
        required: bool,
    ) -> Result<Option<Self>, String> {
        let Some(root) = std::env::var_os("LUMIN_BENCHMARK_CAPTURE_ROOT") else {
            return if required {
                Err("LUMIN_BENCHMARK_CAPTURE_ROOT is required for diagnostics".to_owned())
            } else {
                Ok(None)
            };
        };
        Self::create(Path::new(&root), workspace, package, scratch, expected).map(Some)
    }

    fn create(
        root: &Path,
        workspace: &Path,
        package: &Path,
        scratch: &Path,
        expected: &[String],
    ) -> Result<Self, String> {
        super::require_external_absolute_path(workspace, root, "benchmark archive")?;
        let root = root
            .parent()
            .ok_or("archive has no parent")?
            .canonicalize()
            .map_err(|error| error.to_string())?
            .join(root.file_name().ok_or("archive has no name")?);
        for excluded in [workspace, package, scratch] {
            let excluded = excluded.canonicalize().map_err(|error| error.to_string())?;
            if root.starts_with(&excluded) || excluded.starts_with(&root) {
                return Err("benchmark archive overlaps checkout, package, or scratch".to_owned());
            }
        }
        let cells = expected
            .iter()
            .map(|name| (name.clone(), serde_json::json!({"status":"not-run"})))
            .collect::<BTreeMap<_, _>>();
        if cells.is_empty() || cells.len() != expected.len() {
            return Err("empty or duplicated archive cell inventory".to_owned());
        }
        fs::create_dir(&root)
            .map_err(|error| format!("cannot create new benchmark archive: {error}"))?;
        write_json(
            &root.join("inventory.json"),
            &serde_json::json!({
                "schemaVersion":"lumin.benchmark-capture-inventory.v1", "status":"incomplete",
                "expectedOrder": expected, "cells": cells,
            }),
        )?;
        Ok(Self {
            root,
            cells,
            active: None,
        })
    }

    pub(super) fn begin(&mut self, name: &str) -> Result<PathBuf, String> {
        if self.active.is_some()
            || self
                .cells
                .get(name)
                .and_then(|cell| cell["status"].as_str())
                != Some("not-run")
        {
            return Err(format!("unexpected or repeated archive cell {name}"));
        }
        self.active = Some(name.to_owned());
        Ok(self.root.join(name))
    }

    pub(super) fn complete(&mut self) -> Result<(), String> {
        let name = self
            .active
            .take()
            .ok_or("no active capture cell to complete")?;
        self.cells
            .insert(name, serde_json::json!({"status":"completed"}));
        Ok(())
    }

    pub(super) fn finish(&mut self, failure: Option<&str>) -> Result<(), String> {
        if let Some(name) = self.active.take() {
            self.cells.insert(name, serde_json::json!({"status":"invalid", "reason":failure.unwrap_or("cell did not finish")}));
        }
        let complete = failure.is_none()
            && self
                .cells
                .values()
                .all(|cell| cell["status"] == "completed");
        let mut files = BTreeMap::new();
        collect_hashes(&self.root, &self.root, &mut files)?;
        if files.len() <= 1 {
            return Err("benchmark archive has no process evidence".to_owned());
        }
        write_json(
            &self.root.join("manifest.json"),
            &serde_json::json!({
                "schemaVersion":"lumin.benchmark-capture-manifest.v1",
                "status":if complete { "complete" } else { "incomplete" },
                "reason":failure, "cells": self.cells, "captures": files,
            }),
        )?;
        if failure.is_none() && !complete {
            return Err("benchmark capture inventory is incomplete".to_owned());
        }
        Ok(())
    }
}

fn collect_hashes(
    root: &Path,
    path: &Path,
    files: &mut BTreeMap<String, Value>,
) -> Result<(), String> {
    for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_dir() {
            collect_hashes(root, &entry.path(), files)?;
        } else if kind.is_file() {
            let bytes = fs::read(entry.path()).map_err(|error| error.to_string())?;
            let name = entry
                .path()
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_str()
                .ok_or("non-UTF-8 archive name")?
                .replace('\\', "/");
            files.insert(
                name,
                serde_json::json!({"bytes":bytes.len(), "sha256":super::sha256_hex(&bytes)}),
            );
        } else {
            return Err("redirected/unsupported entry in benchmark archive".to_owned());
        }
    }
    Ok(())
}

pub(super) fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create capture {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot flush capture {}: {error}", path.display()))
}

pub(super) fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    write_bytes(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::measurement;

    #[test]
    fn numeric_miss_and_early_errors_retain_raw_evidence_after_scratch_cleanup()
    -> Result<(), String> {
        let workspace =
            crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
        let helper = workspace.join("tools/xtask/benchmark/measure-process.py");
        let python = measurement::require_python()?;
        for mode in [
            "numeric-miss",
            "malformed-stdout",
            "truth-query-failure",
            "diagnostic-frame-failure",
        ] {
            let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
            let scratch = temporary.path().join("scratch");
            let package = temporary.path().join("package");
            fs::create_dir(&scratch).map_err(|error| error.to_string())?;
            fs::create_dir(&package).map_err(|error| error.to_string())?;
            let root = temporary.path().join("archive");
            let mut archive = CaptureArchive::create(
                &root,
                &workspace,
                &package,
                &scratch,
                &["cell".to_owned()],
            )?;
            let capture = archive.begin("cell")?;
            let code = match mode {
                "malformed-stdout" => "import sys; sys.stdout.buffer.write(b'not json\\n')",
                "diagnostic-frame-failure" => {
                    "import sys; sys.stdout.buffer.write(b'{}\\n'); sys.stderr.buffer.write(b'broken frame\\n')"
                }
                _ => "import sys; sys.stdout.buffer.write(b'{}\\n')",
            };
            let args = measurement::arguments(&["-I", "-S", "-c", code]);
            let measured = if mode == "diagnostic-frame-failure" {
                measurement::measure_diagnostic(
                    &python, &helper, &python, &scratch, &args, &capture,
                )
            } else {
                measurement::measure_product(&python, &helper, &python, &scratch, &args, &capture)
            };
            let result = match mode {
                "malformed-stdout" => measured.map(|_| ()),
                "diagnostic-frame-failure" => {
                    measured?;
                    lumin_protocol::audit_diagnostic::decode(
                        &fs::read(capture.join("stderr")).map_err(|error| error.to_string())?,
                    )
                    .map(|_| ())
                }
                "truth-query-failure" => {
                    measured?;
                    measurement::run_query(&python, &scratch, &measurement::arguments(&["-I", "-S", "-c",
                        "import sys; sys.stdout.buffer.write(b'query prefix'); sys.stderr.buffer.write(b'query failed'); sys.exit(7)"]),
                        &capture.join("truth-query")).map(|_| ())
                }
                _ => {
                    measured?;
                    let times = [
                        "cold-audit-default",
                        "cold-audit-jobs-1",
                        "warm-audit-default",
                        "cold-pre-write-default",
                        "warm-pre-write-default",
                        "post-write-one-file-default",
                        "post-write-32-files-default",
                    ]
                    .into_iter()
                    .map(|mode| (mode, vec![100; 3]))
                    .collect();
                    let report = crate::benchmark::summarize(&times, 1, 1, 4, true)?;
                    assert!(
                        !report["targetMisses"]
                            .as_array()
                            .ok_or("missing budget misses")?
                            .is_empty()
                    );
                    write_json(&root.join("numeric-report.json"), &report)?;
                    archive.complete()?;
                    Ok(())
                }
            };
            assert_eq!(result.is_ok(), mode == "numeric-miss", "{mode}");
            let stdout_before =
                fs::read(capture.join("stdout")).map_err(|error| error.to_string())?;
            let stderr_before =
                fs::read(capture.join("stderr")).map_err(|error| error.to_string())?;
            archive.finish(result.as_ref().err().map(String::as_str))?;
            fs::remove_dir_all(&scratch).map_err(|error| error.to_string())?;
            assert_eq!(
                fs::read(capture.join("stdout")).map_err(|error| error.to_string())?,
                stdout_before
            );
            assert_eq!(
                fs::read(capture.join("stderr")).map_err(|error| error.to_string())?,
                stderr_before
            );
            let manifest: Value = serde_json::from_slice(
                &fs::read(root.join("manifest.json")).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            assert_eq!(
                manifest["captures"]["cell/stdout"]["sha256"],
                crate::benchmark::sha256_hex(&stdout_before)
            );
            assert_eq!(
                manifest["cells"]["cell"]["status"],
                if mode == "numeric-miss" {
                    "completed"
                } else {
                    "invalid"
                }
            );
            if mode == "truth-query-failure" {
                assert_eq!(
                    fs::read(capture.join("truth-query/stdout"))
                        .map_err(|error| error.to_string())?,
                    b"query prefix"
                );
                assert_eq!(
                    fs::read(capture.join("truth-query/stderr"))
                        .map_err(|error| error.to_string())?,
                    b"query failed"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn archives_are_create_new_and_missing_cells_never_become_complete() -> Result<(), String> {
        let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
        let workspace = temporary.path().join("workspace");
        let package = temporary.path().join("package");
        let scratch = temporary.path().join("scratch");
        for path in [&workspace, &package, &scratch] {
            fs::create_dir(path).map_err(|error| error.to_string())?;
        }
        let root = temporary.path().join("archive");
        let expected = ["first".to_owned(), "second".to_owned()];
        let mut archive = CaptureArchive::create(&root, &workspace, &package, &scratch, &expected)?;
        assert!(CaptureArchive::create(&root, &workspace, &package, &scratch, &expected).is_err());
        let capture = archive.begin("first")?;
        fs::create_dir(&capture).map_err(|error| error.to_string())?;
        write_bytes(&capture.join("stdout"), b"partial")?;
        archive.complete()?;
        assert!(archive.finish(None).is_err());
        let manifest: Value = serde_json::from_slice(
            &fs::read(root.join("manifest.json")).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(manifest["cells"]["second"]["status"], "not-run");
        assert_eq!(manifest["status"], "incomplete");
        assert!(
            CaptureArchive::create(
                &scratch.join("nested"),
                &workspace,
                &package,
                &scratch,
                &expected
            )
            .is_err()
        );
        Ok(())
    }
}
