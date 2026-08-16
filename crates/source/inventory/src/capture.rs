use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use lumin_model::PhysicalFileIdentity;

use crate::InventoryError;

pub(crate) struct OpenedSource {
    file: File,
    observation: PhysicalFileObservation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PhysicalFileObservation {
    pub(crate) identity: PhysicalFileIdentity,
    pub(crate) links: u64,
    pub(crate) mount_id: Option<u64>,
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
        let observation = physical_file_observation_from_file(&file)?;
        Ok(Self { file, observation })
    }

    pub(crate) fn observation(&self) -> &PhysicalFileObservation {
        &self.observation
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
    ) -> Result<PhysicalFileObservation, InventoryError> {
        ensure_contained(canonical_root, native_path, logical_path)?;
        let current = physical_file_observation(native_path)?;
        if current.identity != self.observation.identity {
            return Err(source_capture_error(
                logical_path,
                "source path changed physical identity during capture".to_owned(),
            ));
        }
        Ok(current)
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
    physical_file_observation_from_file(file).map(|observation| observation.identity)
}

pub(crate) fn physical_file_observation(
    path: &Path,
) -> Result<PhysicalFileObservation, InventoryError> {
    #[cfg(not(windows))]
    {
        let file = File::open(path)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        physical_file_observation_from_file(&file)
    }
    #[cfg(windows)]
    {
        let handle = winapi_util::Handle::from_path_any(path)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        let information = winapi_util::file::information(&handle)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        windows_observation(&information)
    }
}

pub(crate) fn physical_file_observation_from_file(
    file: &File,
) -> Result<PhysicalFileObservation, InventoryError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let metadata = file
            .metadata()
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        Ok(PhysicalFileObservation {
            identity: PhysicalFileIdentity::Unix {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            links: metadata.nlink(),
            mount_id: linux_mount_id(file)?,
        })
    }
    #[cfg(windows)]
    {
        let information = winapi_util::file::information(file)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        windows_observation(&information)
    }
}

#[cfg(windows)]
fn windows_observation(
    information: &winapi_util::file::Information,
) -> Result<PhysicalFileObservation, InventoryError> {
    let volume_serial = u32::try_from(information.volume_serial_number()).map_err(|_| {
        InventoryError::PhysicalIdentity("volume serial number exceeds u32".to_owned())
    })?;
    Ok(PhysicalFileObservation {
        identity: PhysicalFileIdentity::Windows {
            volume_serial,
            file_index: information.file_index(),
        },
        links: information.number_of_links(),
        mount_id: None,
    })
}

#[cfg(target_os = "linux")]
fn linux_mount_id(file: &File) -> Result<Option<u64>, InventoryError> {
    use std::os::fd::AsRawFd;

    let source = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", file.as_raw_fd()))
        .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
    let mut mount_id = None;
    for line in source.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name != "mnt_id" {
            continue;
        }
        if mount_id.is_some() {
            return Err(InventoryError::PhysicalIdentity(
                "opened input reported duplicate Linux mount IDs".to_owned(),
            ));
        }
        mount_id = Some(value.trim().parse::<u64>().map_err(|error| {
            InventoryError::PhysicalIdentity(format!(
                "opened input reported invalid Linux mount ID: {error}"
            ))
        })?);
    }
    mount_id.map(Some).ok_or_else(|| {
        InventoryError::PhysicalIdentity("opened input omitted its Linux mount ID".to_owned())
    })
}

#[cfg(all(unix, not(target_os = "linux")))]
fn linux_mount_id(_file: &File) -> Result<Option<u64>, InventoryError> {
    Ok(None)
}
