use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use lumin_model::PhysicalFileIdentity;

use crate::InventoryError;

pub(crate) struct OpenedSource {
    file: File,
    physical_identity: PhysicalFileIdentity,
}

impl OpenedSource {
    pub(crate) fn open(
        canonical_root: &Path,
        native_path: &Path,
        logical_path: &str,
    ) -> Result<Self, InventoryError> {
        ensure_contained(canonical_root, native_path, logical_path)?;
        let file = File::open(native_path).map_err(|error| {
            source_capture_error(logical_path, format!("cannot open source: {error}"))
        })?;
        let metadata = file.metadata().map_err(|error| {
            source_capture_error(
                logical_path,
                format!("cannot inspect opened source: {error}"),
            )
        })?;
        if !metadata.is_file() {
            return Err(source_capture_error(
                logical_path,
                "opened source is not a regular file".to_owned(),
            ));
        }
        let physical_identity = physical_identity_from_file(&file)?;
        Ok(Self {
            file,
            physical_identity,
        })
    }

    pub(crate) fn physical_identity(&self) -> &PhysicalFileIdentity {
        &self.physical_identity
    }

    pub(crate) fn read_payload(&mut self, logical_path: &str) -> Result<Arc<[u8]>, InventoryError> {
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes).map_err(|error| {
            source_capture_error(logical_path, format!("cannot read opened source: {error}"))
        })?;
        Ok(bytes.into())
    }

    pub(crate) fn validate_path(
        &self,
        canonical_root: &Path,
        native_path: &Path,
        logical_path: &str,
    ) -> Result<(), InventoryError> {
        ensure_contained(canonical_root, native_path, logical_path)?;
        let current = crate::physical_file_identity(native_path)?;
        if current != self.physical_identity {
            return Err(source_capture_error(
                logical_path,
                "source path changed physical identity during capture".to_owned(),
            ));
        }
        Ok(())
    }
}

fn ensure_contained(
    canonical_root: &Path,
    native_path: &Path,
    logical_path: &str,
) -> Result<(), InventoryError> {
    let target = std::fs::canonicalize(native_path).map_err(|error| {
        source_capture_error(
            logical_path,
            format!("cannot resolve source containment: {error}"),
        )
    })?;
    if !target.starts_with(canonical_root) {
        return Err(source_capture_error(
            logical_path,
            "source resolves outside the repository root".to_owned(),
        ));
    }
    Ok(())
}

fn source_capture_error(path: &str, detail: String) -> InventoryError {
    InventoryError::PhysicalIdentity(format!("source {path}: {detail}"))
}

pub(crate) fn physical_identity_from_file(
    file: &File,
) -> Result<PhysicalFileIdentity, InventoryError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let metadata = file
            .metadata()
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        Ok(PhysicalFileIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        let information = winapi_util::file::information(file)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        let volume_serial = u32::try_from(information.volume_serial_number()).map_err(|_| {
            InventoryError::PhysicalIdentity("volume serial number exceeds u32".to_owned())
        })?;
        Ok(PhysicalFileIdentity::Windows {
            volume_serial,
            file_index: information.file_index(),
        })
    }
}
