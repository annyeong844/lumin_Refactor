use std::path::Path;

use anyhow::Result;

use crate::model::{AdmissionOutcome, BackendKind, HoldPhase};

#[cfg(feature = "redb-backend")]
mod redb_backend;
#[cfg(feature = "sqlite-backend")]
mod sqlite_backend;

pub fn initialize(backend: BackendKind, path: &Path) -> Result<()> {
    match backend {
        BackendKind::Redb => {
            #[cfg(feature = "redb-backend")]
            return redb_backend::initialize(path);
            #[cfg(not(feature = "redb-backend"))]
            anyhow::bail!("redb backend was not compiled")
        }
        BackendKind::Sqlite => {
            #[cfg(feature = "sqlite-backend")]
            return sqlite_backend::initialize(path);
            #[cfg(not(feature = "sqlite-backend"))]
            anyhow::bail!("sqlite backend was not compiled")
        }
    }
}

pub fn admit(
    backend: BackendKind,
    path: &Path,
    key: &str,
    gate_id: &str,
) -> Result<AdmissionOutcome> {
    match backend {
        BackendKind::Redb => {
            #[cfg(feature = "redb-backend")]
            return redb_backend::admit(path, key, gate_id);
            #[cfg(not(feature = "redb-backend"))]
            anyhow::bail!("redb backend was not compiled")
        }
        BackendKind::Sqlite => {
            #[cfg(feature = "sqlite-backend")]
            return sqlite_backend::admit(path, key, gate_id);
            #[cfg(not(feature = "sqlite-backend"))]
            anyhow::bail!("sqlite backend was not compiled")
        }
    }
}

pub fn read_holder(backend: BackendKind, path: &Path, key: &str) -> Result<Option<String>> {
    match backend {
        BackendKind::Redb => {
            #[cfg(feature = "redb-backend")]
            return redb_backend::read_holder(path, key);
            #[cfg(not(feature = "redb-backend"))]
            anyhow::bail!("redb backend was not compiled")
        }
        BackendKind::Sqlite => {
            #[cfg(feature = "sqlite-backend")]
            return sqlite_backend::read_holder(path, key);
            #[cfg(not(feature = "sqlite-backend"))]
            anyhow::bail!("sqlite backend was not compiled")
        }
    }
}

pub fn hold(
    backend: BackendKind,
    path: &Path,
    key: &str,
    gate_id: &str,
    phase: HoldPhase,
    ready_path: &Path,
) -> Result<()> {
    match backend {
        BackendKind::Redb => {
            #[cfg(feature = "redb-backend")]
            return redb_backend::hold(path, key, gate_id, phase, ready_path);
            #[cfg(not(feature = "redb-backend"))]
            anyhow::bail!("redb backend was not compiled")
        }
        BackendKind::Sqlite => {
            #[cfg(feature = "sqlite-backend")]
            return sqlite_backend::hold(path, key, gate_id, phase, ready_path);
            #[cfg(not(feature = "sqlite-backend"))]
            anyhow::bail!("sqlite backend was not compiled")
        }
    }
}

pub fn read_catalog(backend: BackendKind, path: &Path) -> Result<Option<Vec<u8>>> {
    match backend {
        BackendKind::Redb => {
            #[cfg(feature = "redb-backend")]
            return redb_backend::read_catalog(path);
            #[cfg(not(feature = "redb-backend"))]
            anyhow::bail!("redb backend was not compiled")
        }
        BackendKind::Sqlite => {
            #[cfg(feature = "sqlite-backend")]
            return sqlite_backend::read_catalog(path);
            #[cfg(not(feature = "sqlite-backend"))]
            anyhow::bail!("sqlite backend was not compiled")
        }
    }
}

pub fn compare_exchange_catalog(
    backend: BackendKind,
    path: &Path,
    expected: Option<&[u8]>,
    replacement: &[u8],
) -> Result<bool> {
    match backend {
        BackendKind::Redb => {
            #[cfg(feature = "redb-backend")]
            return redb_backend::compare_exchange_catalog(path, expected, replacement);
            #[cfg(not(feature = "redb-backend"))]
            anyhow::bail!("redb backend was not compiled")
        }
        BackendKind::Sqlite => {
            #[cfg(feature = "sqlite-backend")]
            return sqlite_backend::compare_exchange_catalog(path, expected, replacement);
            #[cfg(not(feature = "sqlite-backend"))]
            anyhow::bail!("sqlite backend was not compiled")
        }
    }
}

pub fn prepare_for_replace(backend: BackendKind, path: &Path) -> Result<()> {
    match backend {
        BackendKind::Redb => {
            #[cfg(feature = "redb-backend")]
            return redb_backend::prepare_for_replace(path);
            #[cfg(not(feature = "redb-backend"))]
            anyhow::bail!("redb backend was not compiled")
        }
        BackendKind::Sqlite => {
            #[cfg(feature = "sqlite-backend")]
            return sqlite_backend::prepare_for_replace(path);
            #[cfg(not(feature = "sqlite-backend"))]
            anyhow::bail!("sqlite backend was not compiled")
        }
    }
}

pub fn insert_records(
    backend: BackendKind,
    path: &Path,
    records: &[(String, Vec<u8>)],
) -> Result<()> {
    match backend {
        BackendKind::Redb => {
            #[cfg(feature = "redb-backend")]
            return redb_backend::insert_records(path, records);
            #[cfg(not(feature = "redb-backend"))]
            anyhow::bail!("redb backend was not compiled")
        }
        BackendKind::Sqlite => {
            #[cfg(feature = "sqlite-backend")]
            return sqlite_backend::insert_records(path, records);
            #[cfg(not(feature = "sqlite-backend"))]
            anyhow::bail!("sqlite backend was not compiled")
        }
    }
}

pub fn query_records(
    backend: BackendKind,
    path: &Path,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, Vec<u8>)>> {
    match backend {
        BackendKind::Redb => {
            #[cfg(feature = "redb-backend")]
            return redb_backend::query_records(path, after, limit);
            #[cfg(not(feature = "redb-backend"))]
            anyhow::bail!("redb backend was not compiled")
        }
        BackendKind::Sqlite => {
            #[cfg(feature = "sqlite-backend")]
            return sqlite_backend::query_records(path, after, limit);
            #[cfg(not(feature = "sqlite-backend"))]
            anyhow::bail!("sqlite backend was not compiled")
        }
    }
}
