use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use lumin_model::{PhysicalFileIdentity, RepositoryRootPhysicalIdentity};

use crate::{StoreError, io_error};

#[derive(Clone, Copy)]
pub(crate) enum EntryKind {
    Directory,
    RegularFile,
}

#[derive(Clone, Copy)]
pub(crate) enum EntryAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug)]
pub(crate) struct HeldEntry {
    file: File,
    identity: PhysicalFileIdentity,
    links: u64,
    mount_id: Option<u64>,
}

impl HeldEntry {
    pub(crate) fn open(
        path: &Path,
        kind: EntryKind,
        access: EntryAccess,
        one_link: bool,
        label: &str,
    ) -> Result<Self, StoreError> {
        let file = open_nofollow(path, kind, access)
            .map_err(|error| classify_expected_entry_error(error, kind, label))?;
        Self::from_file(file, kind, one_link, label)
    }

    pub(crate) fn create_new(path: &Path, label: &str) -> Result<Self, StoreError> {
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
            mount_id: facts.mount_id,
        })
    }

    pub(crate) fn file(&self) -> &File {
        &self.file
    }

    pub(crate) fn identity(&self) -> &PhysicalFileIdentity {
        &self.identity
    }

    pub(crate) fn validate_path(
        &self,
        path: &Path,
        kind: EntryKind,
        access: EntryAccess,
        one_link: bool,
        label: &str,
    ) -> Result<(), StoreError> {
        let current = Self::open(path, kind, access, one_link, label)?;
        if current.identity != self.identity
            || current.mount_id != self.mount_id
            || (one_link && current.links != self.links)
        {
            return Err(StoreError::Integrity(format!(
                "{label} physical identity changed"
            )));
        }
        Ok(())
    }

    pub(crate) fn read_all(&self) -> Result<Vec<u8>, StoreError> {
        let mut reader = self.file();
        reader.seek(SeekFrom::Start(0)).map_err(io_error)?;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).map_err(io_error)?;
        Ok(bytes)
    }

    pub(crate) fn replace_contents(&self, bytes: &[u8]) -> Result<(), StoreError> {
        self.file.set_len(0).map_err(io_error)?;
        let mut writer = self.file();
        writer.seek(SeekFrom::Start(0)).map_err(io_error)?;
        writer.write_all(bytes).map_err(io_error)?;
        writer.sync_all().map_err(io_error)
    }

    pub(crate) fn sync(&self) -> Result<(), StoreError> {
        self.file.sync_all().map_err(io_error)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn sync_directory(&self) -> Result<(), StoreError> {
        self.sync()
    }

    #[cfg(windows)]
    pub(crate) fn sync_directory(&self) -> Result<(), StoreError> {
        // Windows rejects FlushFileBuffers on directory handles. The files
        // published into the directory are flushed individually.
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", windows)))]
    pub(crate) fn sync_directory(&self) -> Result<(), StoreError> {
        Err(StoreError::Integrity(
            "managed state directory flush supports Windows and Linux".to_owned(),
        ))
    }
}

fn classify_expected_entry_error(
    error: std::io::Error,
    kind: EntryKind,
    label: &str,
) -> StoreError {
    let redirected_or_wrong_kind = {
        #[cfg(target_os = "linux")]
        {
            matches!(error.raw_os_error(), Some(20 | 40))
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    };
    if redirected_or_wrong_kind {
        return StoreError::Integrity(format!(
            "{label} must be a no-follow real {}",
            match kind {
                EntryKind::Directory => "directory",
                EntryKind::RegularFile => "regular file",
            }
        ));
    }
    if error.kind() == std::io::ErrorKind::NotFound {
        return StoreError::Integrity(format!("{label} is missing"));
    }
    io_error(error)
}

pub(crate) fn same_volume(left: &PhysicalFileIdentity, right: &PhysicalFileIdentity) -> bool {
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

pub(crate) fn same_volume_and_mount(left: &HeldEntry, right: &HeldEntry) -> bool {
    same_volume(left.identity(), right.identity()) && left.mount_id == right.mount_id
}

#[cfg(target_os = "linux")]
pub(crate) fn replace_file_atomic(replaced: &Path, replacement: &Path) -> Result<(), StoreError> {
    std::fs::rename(replacement, replaced).map_err(io_error)
}

#[cfg(windows)]
pub(crate) fn replace_file_atomic(replaced: &Path, replacement: &Path) -> Result<(), StoreError> {
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    move_file_atomic(
        replacement,
        replaced,
        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
    )
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn replace_file_atomic(_replaced: &Path, _replacement: &Path) -> Result<(), StoreError> {
    Err(StoreError::Integrity(
        "lifecycle store replacement supports Windows and Linux".to_owned(),
    ))
}

#[cfg(target_os = "linux")]
pub(crate) fn publish_file_atomic(published: &Path, pending: &Path) -> Result<(), StoreError> {
    std::fs::rename(pending, published).map_err(io_error)
}

#[cfg(windows)]
pub(crate) fn publish_file_atomic(published: &Path, pending: &Path) -> Result<(), StoreError> {
    use windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH;

    move_file_atomic(pending, published, MOVEFILE_WRITE_THROUGH)
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn publish_file_atomic(_published: &Path, _pending: &Path) -> Result<(), StoreError> {
    Err(StoreError::Integrity(
        "lifecycle intent publication supports Windows and Linux".to_owned(),
    ))
}

#[cfg_attr(
    windows,
    allow(
        unsafe_code,
        reason = "durable Windows publication and replacement require MoveFileExW"
    )
)]
#[cfg(windows)]
fn move_file_atomic(source: &Path, destination: &Path, flags: u32) -> Result<(), StoreError> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both vectors are NUL-terminated and remain alive for the call.
    let moved = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) };
    if moved == 0 {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg_attr(
    windows,
    allow(
        unsafe_code,
        reason = "Windows FILE_ID_128 requires GetFileInformationByHandleEx"
    )
)]
pub(super) fn repository_root_physical_identity(
    root: &File,
) -> Result<RepositoryRootPhysicalIdentity, StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let metadata = root.metadata().map_err(io_error)?;
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
        let buffer_size = u32::try_from(size_of::<FILE_ID_INFO>())
            .map_err(|_| StoreError::Integrity("FILE_ID_INFO size exceeds u32".to_owned()))?;
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
            return Err(io_error(std::io::Error::last_os_error()));
        }
        Ok(RepositoryRootPhysicalIdentity::Windows {
            volume_serial: information.VolumeSerialNumber,
            file_id: information.FileId.Identifier,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = root;
        Err(StoreError::Integrity(
            "repository root identity supports Windows and Unix".to_owned(),
        ))
    }
}

struct FileFacts {
    identity: PhysicalFileIdentity,
    links: u64,
    mount_id: Option<u64>,
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
        mount_id: Some(linux_mount_id(file)?),
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
        mount_id: None,
        is_directory: attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        is_regular_file: attributes & FILE_ATTRIBUTE_DIRECTORY == 0,
        is_redirect: attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
    })
}

#[cfg(target_os = "linux")]
fn linux_mount_id(file: &File) -> Result<u64, StoreError> {
    use std::os::fd::AsRawFd;

    let source = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", file.as_raw_fd()))
        .map_err(io_error)?;
    parse_linux_mount_id(&source)
}

#[cfg(target_os = "linux")]
fn parse_linux_mount_id(source: &str) -> Result<u64, StoreError> {
    let mut mount_id = None;
    for line in source.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name != "mnt_id" {
            continue;
        }
        if mount_id.is_some() {
            return Err(StoreError::Integrity(
                "opened state object reported duplicate Linux mount IDs".to_owned(),
            ));
        }
        mount_id = Some(value.trim().parse::<u64>().map_err(|error| {
            StoreError::Integrity(format!(
                "opened state object reported invalid mount ID: {error}"
            ))
        })?);
    }
    mount_id.ok_or_else(|| {
        StoreError::Integrity("opened state object omitted its Linux mount ID".to_owned())
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::parse_linux_mount_id;

    #[test]
    fn parses_exact_linux_mount_identity() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            parse_linux_mount_id("pos:\t0\nflags:\t0100000\nmnt_id:\t47\nino:\t5\n")?,
            47
        );
        assert!(parse_linux_mount_id("pos:\t0\nino:\t5\n").is_err());
        assert!(parse_linux_mount_id("mnt_id:\t47\nmnt_id:\t48\n").is_err());
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn file_facts(_file: &File) -> Result<FileFacts, StoreError> {
    Err(StoreError::Integrity(
        "managed state physical identity supports Windows and Linux".to_owned(),
    ))
}
