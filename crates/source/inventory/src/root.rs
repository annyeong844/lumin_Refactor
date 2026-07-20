use std::fs::{self, File};
use std::path::{Path, PathBuf};

use lumin_model::{RepositoryBinding, RepositoryRootIdentity, RepositoryRootPhysicalIdentity};

use crate::{InventoryError, validate_root};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryAdmission {
    pub canonical_root: PathBuf,
    pub binding: RepositoryBinding,
}

pub fn repository_admission(root: &Path) -> Result<RepositoryAdmission, InventoryError> {
    validate_root(root)?;
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?;
    let root_handle = open_repository_root(&canonical_root)
        .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?;
    let physical_identity = observe_root_physical_identity(&root_handle)?;
    let root_identity =
        RepositoryRootIdentity::from_native_absolute(&canonical_root, physical_identity)
            .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?;
    Ok(RepositoryAdmission {
        canonical_root,
        binding: RepositoryBinding::new(root_identity),
    })
}

#[cfg_attr(
    windows,
    allow(
        unsafe_code,
        reason = "Windows FILE_ID_128 requires GetFileInformationByHandleEx"
    )
)]
fn observe_root_physical_identity(
    root: &File,
) -> Result<RepositoryRootPhysicalIdentity, InventoryError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let metadata = root
            .metadata()
            .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?;
        Ok(RepositoryRootPhysicalIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        use std::mem::size_of;
        use std::os::windows::io::AsRawHandle;

        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
        };

        let mut information = FILE_ID_INFO::default();
        let buffer_size = u32::try_from(size_of::<FILE_ID_INFO>()).map_err(|_| {
            InventoryError::RepositoryIdentity("FILE_ID_INFO size exceeds u32".to_owned())
        })?;
        // SAFETY: `root` owns a valid handle for the duration of the call,
        // and `information` is an aligned, writable FILE_ID_INFO buffer.
        let succeeded = unsafe {
            GetFileInformationByHandleEx(
                root.as_raw_handle() as HANDLE,
                FileIdInfo,
                std::ptr::from_mut(&mut information).cast(),
                buffer_size,
            )
        };
        if succeeded == 0 {
            return Err(InventoryError::RepositoryIdentity(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(RepositoryRootPhysicalIdentity::Windows {
            volume_serial: information.VolumeSerialNumber,
            file_id: information.FileId.Identifier,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = root;
        Err(InventoryError::RepositoryIdentity(
            "repository root identity supports Windows and Unix".to_owned(),
        ))
    }
}

#[cfg(unix)]
fn open_repository_root(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(windows)]
fn open_repository_root(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_repository_root(_path: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "repository root identity supports Windows and Unix",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_aliases_share_one_repository_binding() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let direct = repository_admission(root.path())?;
        let dotted = repository_admission(&root.path().join("."))?;

        assert_eq!(direct, dotted);
        let derived = lumin_model::RepositoryId::for_root(direct.binding.root());
        assert_eq!(direct.binding.repository_id(), &derived);
        Ok(())
    }
}
