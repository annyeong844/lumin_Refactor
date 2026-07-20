use std::fs;
use std::path::{Path, PathBuf};

use file_id::FileId;
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
    let physical_identity = observe_root_physical_identity(&canonical_root)?;
    let root_identity =
        RepositoryRootIdentity::from_native_absolute(&canonical_root, physical_identity)
            .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?;
    Ok(RepositoryAdmission {
        canonical_root,
        binding: RepositoryBinding::new(root_identity),
    })
}

fn observe_root_physical_identity(
    root: &Path,
) -> Result<RepositoryRootPhysicalIdentity, InventoryError> {
    #[cfg(unix)]
    {
        match file_id::get_file_id(root)
            .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?
        {
            FileId::Inode {
                device_id,
                inode_number,
            } => Ok(RepositoryRootPhysicalIdentity::Unix {
                device: device_id,
                inode: inode_number,
            }),
            _ => Err(InventoryError::RepositoryIdentity(
                "Unix root did not yield a device/inode identity".to_owned(),
            )),
        }
    }
    #[cfg(windows)]
    {
        match file_id::get_high_res_file_id(root)
            .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?
        {
            FileId::HighRes {
                volume_serial_number,
                file_id,
            } => Ok(RepositoryRootPhysicalIdentity::Windows {
                volume_serial: volume_serial_number,
                file_id: file_id.to_le_bytes(),
            }),
            _ => Err(InventoryError::RepositoryIdentity(
                "Windows root did not yield FILE_ID_128".to_owned(),
            )),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = root;
        Err(InventoryError::RepositoryIdentity(
            "repository root identity supports Windows and Unix".to_owned(),
        ))
    }
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
