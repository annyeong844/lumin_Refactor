use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use file_id::FileId;
use lumin_model::{PhysicalFileIdentity, RepositoryRootPhysicalIdentity};

use crate::{StoreError, io_error};

#[derive(Clone, Copy)]
pub(super) enum EntryKind {
    Directory,
    RegularFile,
}

#[derive(Clone, Copy)]
pub(super) enum EntryAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug)]
pub(super) struct HeldEntry {
    file: File,
    identity: PhysicalFileIdentity,
    links: u64,
}

impl HeldEntry {
    pub(super) fn open(
        path: &Path,
        kind: EntryKind,
        access: EntryAccess,
        one_link: bool,
        label: &str,
    ) -> Result<Self, StoreError> {
        let file = open_nofollow(path, kind, access).map_err(io_error)?;
        Self::from_file(file, kind, one_link, label)
    }

    pub(super) fn create_new(path: &Path, label: &str) -> Result<Self, StoreError> {
        let file = create_new_nofollow(path).map_err(io_error)?;
        Self::from_file(file, EntryKind::RegularFile, true, label)
    }

    fn from_file(
        file: File,
        kind: EntryKind,
        one_link: bool,
        label: &str,
    ) -> Result<Self, StoreError> {
        let facts = file_facts(&file)?;
        let expected_kind = match kind {
            EntryKind::Directory => facts.is_directory,
            EntryKind::RegularFile => facts.is_regular_file,
        };
        if facts.is_redirect || !expected_kind {
            return Err(StoreError::Integrity(format!(
                "{label} must be a no-follow real {}",
                match kind {
                    EntryKind::Directory => "directory",
                    EntryKind::RegularFile => "regular file",
                }
            )));
        }
        if one_link && facts.links != 1 {
            return Err(StoreError::Integrity(format!(
                "{label} must have exactly one physical link"
            )));
        }
        Ok(Self {
            file,
            identity: facts.identity,
            links: facts.links,
        })
    }

    pub(super) fn file(&self) -> &File {
        &self.file
    }

    pub(super) fn identity(&self) -> &PhysicalFileIdentity {
        &self.identity
    }

    pub(super) fn validate_path(
        &self,
        path: &Path,
        kind: EntryKind,
        access: EntryAccess,
        one_link: bool,
        label: &str,
    ) -> Result<(), StoreError> {
        let current = Self::open(path, kind, access, one_link, label)?;
        if current.identity != self.identity || (one_link && current.links != self.links) {
            return Err(StoreError::Integrity(format!(
                "{label} physical identity changed"
            )));
        }
        Ok(())
    }

    pub(super) fn read_all(&self) -> Result<Vec<u8>, StoreError> {
        let mut reader = self.file();
        reader.seek(SeekFrom::Start(0)).map_err(io_error)?;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).map_err(io_error)?;
        Ok(bytes)
    }

    pub(super) fn replace_contents(&self, bytes: &[u8]) -> Result<(), StoreError> {
        self.file.set_len(0).map_err(io_error)?;
        let mut writer = self.file();
        writer.seek(SeekFrom::Start(0)).map_err(io_error)?;
        writer.write_all(bytes).map_err(io_error)?;
        writer.sync_all().map_err(io_error)
    }

    pub(super) fn sync(&self) -> Result<(), StoreError> {
        self.file.sync_all().map_err(io_error)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn sync_directory(&self) -> Result<(), StoreError> {
        self.sync()
    }

    #[cfg(windows)]
    pub(super) fn sync_directory(&self) -> Result<(), StoreError> {
        // Windows rejects FlushFileBuffers on directory handles. The files
        // published into the directory are flushed individually.
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", windows)))]
    pub(super) fn sync_directory(&self) -> Result<(), StoreError> {
        Err(StoreError::Integrity(
            "managed state directory flush supports Windows and Linux".to_owned(),
        ))
    }
}

pub(super) fn same_volume(left: &PhysicalFileIdentity, right: &PhysicalFileIdentity) -> bool {
    match (left, right) {
        (
            PhysicalFileIdentity::Unix { device: left, .. },
            PhysicalFileIdentity::Unix { device: right, .. },
        ) => left == right,
        (
            PhysicalFileIdentity::Windows {
                volume_serial: left,
                ..
            },
            PhysicalFileIdentity::Windows {
                volume_serial: right,
                ..
            },
        ) => left == right,
        _ => false,
    }
}

pub(super) fn repository_root_physical_identity(
    path: &Path,
) -> Result<RepositoryRootPhysicalIdentity, StoreError> {
    #[cfg(unix)]
    {
        match file_id::get_file_id(path).map_err(io_error)? {
            FileId::Inode {
                device_id,
                inode_number,
            } => Ok(RepositoryRootPhysicalIdentity::Unix {
                device: device_id,
                inode: inode_number,
            }),
            _ => Err(StoreError::Integrity(
                "Unix repository root omitted its device/inode identity".to_owned(),
            )),
        }
    }
    #[cfg(windows)]
    {
        match file_id::get_high_res_file_id(path).map_err(io_error)? {
            FileId::HighRes {
                volume_serial_number,
                file_id,
            } => Ok(RepositoryRootPhysicalIdentity::Windows {
                volume_serial: volume_serial_number,
                file_id: file_id.to_le_bytes(),
            }),
            _ => Err(StoreError::Integrity(
                "Windows repository root omitted FILE_ID_128".to_owned(),
            )),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(StoreError::Integrity(
            "repository root identity supports Windows and Unix".to_owned(),
        ))
    }
}

struct FileFacts {
    identity: PhysicalFileIdentity,
    links: u64,
    is_directory: bool,
    is_regular_file: bool,
    is_redirect: bool,
}

#[cfg(target_os = "linux")]
fn open_nofollow(path: &Path, kind: EntryKind, access: EntryAccess) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    const O_DIRECTORY: i32 = 0x1_0000;
    const O_NOFOLLOW: i32 = 0x2_0000;

    let mut options = OpenOptions::new();
    options.read(true);
    if matches!(access, EntryAccess::ReadWrite) {
        options.write(true);
    }
    let directory = if matches!(kind, EntryKind::Directory) {
        O_DIRECTORY
    } else {
        0
    };
    options.custom_flags(O_NOFOLLOW | directory).open(path)
}

#[cfg(windows)]
fn open_nofollow(path: &Path, kind: EntryKind, access: EntryAccess) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

    let mut options = OpenOptions::new();
    options.read(true);
    if matches!(access, EntryAccess::ReadWrite) {
        options.write(true);
    }
    let directory = if matches!(kind, EntryKind::Directory) {
        FILE_FLAG_BACKUP_SEMANTICS
    } else {
        0
    };
    options
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | directory)
        .open(path)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn open_nofollow(_path: &Path, _kind: EntryKind, _access: EntryAccess) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "managed state no-follow admission supports Windows and Linux",
    ))
}

#[cfg(target_os = "linux")]
fn create_new_nofollow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    const O_NOFOLLOW: i32 = 0x2_0000;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .custom_flags(O_NOFOLLOW)
        .open(path)
}

#[cfg(windows)]
fn create_new_nofollow(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn create_new_nofollow(_path: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "managed state no-follow initialization supports Windows and Linux",
    ))
}

#[cfg(target_os = "linux")]
fn file_facts(file: &File) -> Result<FileFacts, StoreError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata().map_err(io_error)?;
    Ok(FileFacts {
        identity: PhysicalFileIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
        links: metadata.nlink(),
        is_directory: metadata.is_dir(),
        is_regular_file: metadata.is_file(),
        is_redirect: metadata.file_type().is_symlink(),
    })
}

#[cfg(windows)]
fn file_facts(file: &File) -> Result<FileFacts, StoreError> {
    const FILE_ATTRIBUTE_DIRECTORY: u64 = 0x10;
    const FILE_ATTRIBUTE_REPARSE_POINT: u64 = 0x400;

    let information = winapi_util::file::information(file).map_err(io_error)?;
    let attributes = information.file_attributes();
    let volume_serial = u32::try_from(information.volume_serial_number()).map_err(|_| {
        StoreError::Integrity(
            "Windows volume serial exceeded its physical identity field".to_owned(),
        )
    })?;
    Ok(FileFacts {
        identity: PhysicalFileIdentity::Windows {
            volume_serial,
            file_index: information.file_index(),
        },
        links: information.number_of_links(),
        is_directory: attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        is_regular_file: attributes & FILE_ATTRIBUTE_DIRECTORY == 0,
        is_redirect: attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
    })
}

#[cfg(not(any(target_os = "linux", windows)))]
fn file_facts(_file: &File) -> Result<FileFacts, StoreError> {
    Err(StoreError::Integrity(
        "managed state physical identity supports Windows and Linux".to_owned(),
    ))
}
