use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
    Move,
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

    pub(crate) fn create_new_movable(path: &Path, label: &str) -> Result<Self, StoreError> {
        let file = create_new_movable_nofollow(path).map_err(io_error)?;
        Self::from_file(file, EntryKind::RegularFile, true, label)
    }

    pub(crate) fn open_following_file(path: &Path, label: &str) -> Result<Self, StoreError> {
        let file = File::open(path)
            .map_err(|error| classify_expected_entry_error(error, EntryKind::RegularFile, label))?;
        Self::from_file(file, EntryKind::RegularFile, false, label)
    }

    pub(crate) fn validate_following_file_path(
        &self,
        path: &Path,
        label: &str,
    ) -> Result<(), StoreError> {
        let current = Self::open_following_file(path, label)?;
        if current.identity != self.identity || current.mount_id != self.mount_id {
            return Err(StoreError::Integrity(format!(
                "{label} physical identity changed"
            )));
        }
        Ok(())
    }

    pub(crate) fn from_file(
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

    pub(crate) fn links(&self) -> u64 {
        self.links
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

    pub(crate) fn directory_names(&self, label: &str) -> Result<Vec<OsString>, StoreError> {
        directory_names_from_handle(&self.file, label)
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

#[cfg(target_os = "linux")]
fn directory_names_from_handle(file: &File, label: &str) -> Result<Vec<OsString>, StoreError> {
    use std::os::fd::AsRawFd;

    let descriptor_path = format!("/proc/self/fd/{}", file.as_raw_fd());
    let mut names = std::fs::read_dir(descriptor_path)
        .map_err(|error| StoreError::Integrity(format!("cannot enumerate {label}: {error}")))?
        .map(|entry| {
            entry.map(|entry| entry.file_name()).map_err(|error| {
                StoreError::Integrity(format!("cannot enumerate {label}: {error}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "Windows handle-owned directory enumeration requires GetFileInformationByHandleEx"
)]
fn directory_names_from_handle(file: &File, label: &str) -> Result<Vec<OsString>, StoreError> {
    use std::mem::{offset_of, size_of};
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::{ERROR_NO_MORE_FILES, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_BOTH_DIR_INFO, FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo,
        GetFileInformationByHandleEx,
    };

    const BUFFER_SIZE: usize = 64 * 1024;
    let buffer_size = u32::try_from(BUFFER_SIZE)
        .map_err(|_| StoreError::Integrity("directory inventory buffer exceeds u32".to_owned()))?;
    let mut names = Vec::new();
    let mut restart = true;
    loop {
        let mut buffer = vec![0_u8; BUFFER_SIZE];
        let class = if restart {
            FileIdBothDirectoryRestartInfo
        } else {
            FileIdBothDirectoryInfo
        };
        // SAFETY: `file` owns a live directory handle, and `buffer` is a
        // writable allocation whose exact byte length is passed to Windows.
        let succeeded = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle() as HANDLE,
                class,
                buffer.as_mut_ptr().cast(),
                buffer_size,
            )
        };
        if succeeded == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                break;
            }
            return Err(StoreError::Integrity(format!(
                "cannot enumerate {label}: {error}"
            )));
        }
        restart = false;

        let mut offset = 0_usize;
        loop {
            let header_end = offset
                .checked_add(size_of::<FILE_ID_BOTH_DIR_INFO>())
                .ok_or_else(|| {
                    StoreError::Integrity(format!("{label} directory inventory overflow"))
                })?;
            if header_end > buffer.len() {
                return Err(StoreError::Integrity(format!(
                    "{label} returned a truncated directory inventory"
                )));
            }
            // SAFETY: the complete fixed header is within `buffer`; Windows
            // does not promise Rust alignment, so the header is read unaligned.
            let information = unsafe {
                std::ptr::read_unaligned(
                    buffer.as_ptr().add(offset).cast::<FILE_ID_BOTH_DIR_INFO>(),
                )
            };
            let name_length = usize::try_from(information.FileNameLength).map_err(|_| {
                StoreError::Integrity(format!("{label} directory name length overflow"))
            })?;
            if name_length % size_of::<u16>() != 0 {
                return Err(StoreError::Integrity(format!(
                    "{label} returned a malformed directory name"
                )));
            }
            let name_start = offset
                .checked_add(offset_of!(FILE_ID_BOTH_DIR_INFO, FileName))
                .ok_or_else(|| {
                    StoreError::Integrity(format!("{label} directory name offset overflow"))
                })?;
            let name_end = name_start.checked_add(name_length).ok_or_else(|| {
                StoreError::Integrity(format!("{label} directory name length overflow"))
            })?;
            if name_end > buffer.len() {
                return Err(StoreError::Integrity(format!(
                    "{label} returned a truncated directory name"
                )));
            }
            let wide = buffer[name_start..name_end]
                .chunks_exact(size_of::<u16>())
                .map(|bytes| u16::from_ne_bytes([bytes[0], bytes[1]]))
                .collect::<Vec<_>>();
            let name = OsString::from_wide(&wide);
            if name != OsStr::new(".") && name != OsStr::new("..") {
                names.push(name);
            }

            if information.NextEntryOffset == 0 {
                break;
            }
            offset = offset
                .checked_add(information.NextEntryOffset as usize)
                .ok_or_else(|| {
                    StoreError::Integrity(format!("{label} directory inventory offset overflow"))
                })?;
            if offset >= buffer.len() {
                return Err(StoreError::Integrity(format!(
                    "{label} returned an invalid directory inventory offset"
                )));
            }
        }
    }
    names.sort();
    Ok(names)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn directory_names_from_handle(_file: &File, _label: &str) -> Result<Vec<OsString>, StoreError> {
    Err(StoreError::Integrity(
        "handle-owned directory inventory supports Windows and Linux".to_owned(),
    ))
}

#[derive(Debug)]
pub(crate) struct UnpublishedFile {
    entry: HeldEntry,
    parent_path: PathBuf,
    namespace_name: Option<OsString>,
    named_path: Option<tempfile::TempPath>,
}

impl UnpublishedFile {
    pub(crate) fn create(parent_path: &Path, parent: &HeldEntry) -> Result<Self, StoreError> {
        Self::create_with_policy(parent_path, parent, None)
    }

    pub(crate) fn create_with_named_fallback(
        parent_path: &Path,
        parent: &HeldEntry,
        fallback_name: &OsStr,
    ) -> Result<Self, StoreError> {
        require_normal_component(fallback_name, "named unpublished state artifact")?;
        Self::create_with_policy(parent_path, parent, Some(fallback_name))
    }

    fn create_with_policy(
        parent_path: &Path,
        parent: &HeldEntry,
        named_fallback: Option<&OsStr>,
    ) -> Result<Self, StoreError> {
        let (file, namespace_name, named_path) =
            create_unpublished_file_platform(parent_path, named_fallback)?;
        let entry = HeldEntry::from_file(
            file,
            EntryKind::RegularFile,
            false,
            "unpublished state artifact",
        )?;
        if !same_volume_and_mount(&entry, parent) {
            return Err(StoreError::Integrity(
                "unpublished state artifact crossed its bound volume or mount".to_owned(),
            ));
        }
        if let Some(name) = namespace_name.as_ref() {
            register_active_unpublished(parent_path, name, entry.identity())?;
        }
        Ok(Self {
            entry,
            parent_path: parent_path.to_owned(),
            namespace_name,
            named_path,
        })
    }

    pub(crate) fn entry(&self) -> &HeldEntry {
        &self.entry
    }

    pub(crate) fn publish_noreplace(
        mut self,
        parent: &HeldEntry,
        parent_path: &Path,
        name: &OsStr,
        label: &str,
        after_publication: impl FnOnce() -> Result<(), StoreError>,
    ) -> Result<HeldEntry, StoreError> {
        require_normal_component(name, label)?;
        if !same_volume_and_mount(&self.entry, parent) {
            return Err(StoreError::Integrity(format!(
                "{label} crossed its bound volume or mount"
            )));
        }
        if self.parent_path != parent_path {
            return Err(StoreError::Integrity(format!(
                "{label} publication changed its bound parent"
            )));
        }
        if let Some(named_path) = self.named_path.take() {
            let current = HeldEntry::open(
                named_path.as_ref(),
                EntryKind::RegularFile,
                EntryAccess::ReadWrite,
                true,
                "named unpublished state artifact",
            )?;
            if current.identity() != self.entry.identity()
                || !same_volume_and_mount(&current, parent)
            {
                return Err(StoreError::Integrity(
                    "named unpublished state artifact changed physical identity".to_owned(),
                ));
            }
            drop(current);
            named_path
                .persist_noclobber(parent_path.join(name))
                .map_err(|error| io_error(error.error))?;
        } else {
            publish_unpublished_file_platform(&self.entry, parent, name)?;
        }
        after_publication()?;
        let expected_identity = self.entry.identity().clone();
        drop(self);
        let path = parent_path.join(name);
        let published = HeldEntry::open(
            &path,
            EntryKind::RegularFile,
            EntryAccess::ReadWrite,
            true,
            label,
        )?;
        if published.identity() != &expected_identity {
            return Err(StoreError::Integrity(format!(
                "{label} physical identity changed during publication"
            )));
        }
        Ok(published)
    }
}

impl Drop for UnpublishedFile {
    fn drop(&mut self) {
        if let Some(name) = self.namespace_name.as_ref() {
            unregister_active_unpublished(&self.parent_path, name, self.entry.identity());
        }
    }
}

type ActiveUnpublishedRegistry =
    std::collections::BTreeMap<(PathBuf, OsString), PhysicalFileIdentity>;

fn active_unpublished_registry() -> &'static Mutex<ActiveUnpublishedRegistry> {
    static REGISTRY: OnceLock<Mutex<ActiveUnpublishedRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(ActiveUnpublishedRegistry::new()))
}

fn register_active_unpublished(
    parent_path: &Path,
    name: &OsStr,
    identity: &PhysicalFileIdentity,
) -> Result<(), StoreError> {
    let mut registry = active_unpublished_registry()
        .lock()
        .map_err(|_| StoreError::Integrity("active unpublished registry is poisoned".to_owned()))?;
    let key = (parent_path.to_owned(), name.to_owned());
    if registry.contains_key(&key) {
        return Err(StoreError::Integrity(
            "active unpublished state name was registered twice".to_owned(),
        ));
    }
    registry.insert(key, identity.clone());
    Ok(())
}

fn unregister_active_unpublished(
    parent_path: &Path,
    name: &OsStr,
    identity: &PhysicalFileIdentity,
) {
    let Ok(mut registry) = active_unpublished_registry().lock() else {
        return;
    };
    let key = (parent_path.to_owned(), name.to_owned());
    if registry.get(&key) == Some(identity) {
        registry.remove(&key);
    }
}

pub(crate) fn validate_active_unpublished_name(
    parent_path: &Path,
    parent: &HeldEntry,
    name: &OsStr,
) -> Result<bool, StoreError> {
    let expected = active_unpublished_registry()
        .lock()
        .map_err(|_| StoreError::Integrity("active unpublished registry is poisoned".to_owned()))?
        .get(&(parent_path.to_owned(), name.to_owned()))
        .cloned();
    let Some(expected) = expected else {
        return Ok(false);
    };
    let entry = HeldEntry::open(
        &parent_path.join(name),
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "active unpublished state artifact",
    )?;
    if entry.identity() != &expected || !same_volume_and_mount(&entry, parent) {
        return Err(StoreError::Integrity(
            "active unpublished state artifact changed physical identity".to_owned(),
        ));
    }
    Ok(true)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn create_unpublished_file_platform(
    parent_path: &Path,
    named_fallback: Option<&OsStr>,
) -> Result<(File, Option<OsString>, Option<tempfile::TempPath>), StoreError> {
    use std::os::unix::fs::OpenOptionsExt;

    const O_TMPFILE: i32 = 4_259_840;
    if let Some(name) = named_fallback {
        let path = parent_path.join(name);
        match open_nofollow(&path, EntryKind::RegularFile, EntryAccess::ReadWrite) {
            Ok(file) => return durable_named_unpublished(file, path, name),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(classify_expected_entry_error(
                    error,
                    EntryKind::RegularFile,
                    "named unpublished state artifact",
                ));
            }
        }
    }

    let force_named = named_fallback.is_some() && {
        #[cfg(feature = "namespace-test-crash")]
        {
            std::env::var_os("LUMIN_TEST_NAMESPACE_FORCE_NAMED_UNPUBLISHED").is_some()
        }
        #[cfg(not(feature = "namespace-test-crash"))]
        {
            false
        }
    };
    if !force_named {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_TMPFILE)
            .open(parent_path)
        {
            Ok(file) => return Ok((file, None, None)),
            Err(error)
                if named_fallback.is_some()
                    && matches!(error.raw_os_error(), Some(2 | 21 | 22 | 95)) => {}
            Err(error) => return Err(io_error(error)),
        }
    }

    let name = named_fallback.ok_or_else(|| {
        StoreError::Integrity("unnamed state publication is unavailable".to_owned())
    })?;
    let path = parent_path.join(name);
    match create_new_nofollow(&path) {
        Ok(file) => durable_named_unpublished(file, path, name),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let file = open_nofollow(&path, EntryKind::RegularFile, EntryAccess::ReadWrite)
                .map_err(|error| {
                    classify_expected_entry_error(
                        error,
                        EntryKind::RegularFile,
                        "named unpublished state artifact",
                    )
                })?;
            durable_named_unpublished(file, path, name)
        }
        Err(error) => Err(io_error(error)),
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn durable_named_unpublished(
    file: File,
    path: PathBuf,
    name: &OsStr,
) -> Result<(File, Option<OsString>, Option<tempfile::TempPath>), StoreError> {
    let mut named_path = tempfile::TempPath::try_from_path(path).map_err(io_error)?;
    // This name is a durable bootstrap recovery artifact. An ordinary error
    // must preserve it for the next locked admission or for integrity review.
    named_path.disable_cleanup(true);
    Ok((file, Some(name.to_owned()), Some(named_path)))
}

#[cfg(not(any(all(target_os = "linux", target_arch = "x86_64"), windows)))]
fn create_unpublished_file_platform(
    parent_path: &Path,
    _named_fallback: Option<&OsStr>,
) -> Result<(File, Option<OsString>, Option<tempfile::TempPath>), StoreError> {
    tempfile::tempfile_in(parent_path)
        .map(|file| (file, None, None))
        .map_err(io_error)
}

#[cfg(windows)]
fn create_unpublished_file_platform(
    parent_path: &Path,
    _named_fallback: Option<&OsStr>,
) -> Result<(File, Option<OsString>, Option<tempfile::TempPath>), StoreError> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    static NEXT_UNPUBLISHED: AtomicU64 = AtomicU64::new(1);
    for _ in 0..1024 {
        let sequence = NEXT_UNPUBLISHED.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            ".lumin-unpublished-{:08x}-{sequence:016x}",
            std::process::id()
        );
        match OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE)
            .open(parent_path.join(&name))
        {
            Ok(file) => return Ok((file, Some(OsString::from(name)), None)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(error)),
        }
    }
    Err(StoreError::Integrity(
        "could not allocate a unique handle-owned unpublished state object".to_owned(),
    ))
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

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[allow(
    unsafe_code,
    reason = "Linux handle-owned publication requires linkat with AT_EMPTY_PATH"
)]
fn publish_unpublished_file_platform(
    unpublished: &HeldEntry,
    parent: &HeldEntry,
    name: &OsStr,
) -> Result<(), StoreError> {
    use std::ffi::{CString, c_char, c_int};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    const AT_FDCWD: c_int = -100;
    const AT_SYMLINK_FOLLOW: c_int = 0x400;
    const AT_EMPTY_PATH: c_int = 0x1000;
    unsafe extern "C" {
        fn linkat(
            olddirfd: c_int,
            oldpath: *const c_char,
            newdirfd: c_int,
            newpath: *const c_char,
            flags: c_int,
        ) -> c_int;
    }

    let empty = CString::new(Vec::<u8>::new())
        .map_err(|_| StoreError::Integrity("empty publication path contains NUL".to_owned()))?;
    let name = CString::new(name.as_bytes())
        .map_err(|_| StoreError::Integrity("publication name contains NUL".to_owned()))?;
    let unpublished_fd = unpublished.file().as_raw_fd();
    let parent_fd = parent.file().as_raw_fd();
    // SAFETY: both descriptors and NUL-terminated strings remain live for the
    // call; AT_EMPTY_PATH names the opened unpublished object and linkat does
    // not replace an existing destination.
    let result = unsafe {
        linkat(
            unpublished_fd,
            empty.as_ptr(),
            parent_fd,
            name.as_ptr(),
            AT_EMPTY_PATH,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let direct_error = std::io::Error::last_os_error();
    if !matches!(direct_error.raw_os_error(), Some(1 | 2)) {
        return Err(io_error(direct_error));
    }

    let proc_path = CString::new(format!("/proc/self/fd/{unpublished_fd}")).map_err(|_| {
        StoreError::Integrity("publication descriptor path contains NUL".to_owned())
    })?;
    // SAFETY: the proc descriptor path names the same live O_TMPFILE handle;
    // AT_SYMLINK_FOLLOW dereferences that handle while the held destination
    // directory keeps no-replace publication relative to the bound parent.
    let result = unsafe {
        linkat(
            AT_FDCWD,
            proc_path.as_ptr(),
            parent_fd,
            name.as_ptr(),
            AT_SYMLINK_FOLLOW,
        )
    };
    if result != 0 {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "Windows handle-owned publication requires NtSetInformationFile on the unnamed delete-on-close handle"
)]
fn publish_unpublished_file_platform(
    unpublished: &HeldEntry,
    parent: &HeldEntry,
    name: &OsStr,
) -> Result<(), StoreError> {
    use std::mem::{offset_of, size_of};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::FILE_RENAME_INFO;

    #[repr(C)]
    struct IoStatusBlock {
        status_or_pointer: usize,
        information: usize,
    }

    #[repr(C)]
    struct FileDispositionInformationEx {
        flags: u32,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtSetInformationFile(
            file_handle: HANDLE,
            io_status_block: *mut IoStatusBlock,
            file_information: *const core::ffi::c_void,
            length: u32,
            file_information_class: i32,
        ) -> i32;
    }

    const FILE_RENAME_INFORMATION: i32 = 10;
    const FILE_DISPOSITION_INFORMATION_EX: i32 = 64;
    const FILE_DISPOSITION_ON_CLOSE: u32 = 0x0000_0008;

    let name = name.encode_wide().collect::<Vec<_>>();
    let name_bytes = name
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|length| u32::try_from(length).ok())
        .ok_or_else(|| StoreError::Integrity("publication name is too long".to_owned()))?;
    let bytes = offset_of!(FILE_RENAME_INFO, FileName)
        .checked_add(name.len() * size_of::<u16>())
        .ok_or_else(|| StoreError::Integrity("publication buffer overflow".to_owned()))?;
    let words = bytes.div_ceil(size_of::<u64>());
    let mut buffer = vec![0_u64; words];
    let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: the aligned buffer is sized for the fixed header and exact
    // UTF-16 component. Both handles remain live for both calls.
    unsafe {
        (*information).Anonymous.ReplaceIfExists = false;
        (*information).RootDirectory = parent.file().as_raw_handle() as HANDLE;
        (*information).FileNameLength = name_bytes;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*information).FileName).cast::<u16>(),
            name.len(),
        );
        let mut io_status = IoStatusBlock {
            status_or_pointer: 0,
            information: 0,
        };
        let status = NtSetInformationFile(
            unpublished.file().as_raw_handle() as HANDLE,
            &mut io_status,
            information.cast(),
            u32::try_from(bytes)
                .map_err(|_| StoreError::Integrity("publication buffer exceeds u32".to_owned()))?,
            FILE_RENAME_INFORMATION,
        );
        if status < 0 {
            return Err(StoreError::Io(format!(
                "handle-owned publication failed with NTSTATUS 0x{:08x}",
                status as u32
            )));
        }
        // FILE_FLAG_DELETE_ON_CLOSE cannot be cancelled by the legacy boolean
        // disposition class. The extended class with ON_CLOSE and no DELETE
        // bit clears that create-time state after the no-replace rename.
        let disposition = FileDispositionInformationEx {
            flags: FILE_DISPOSITION_ON_CLOSE,
        };
        let status = NtSetInformationFile(
            unpublished.file().as_raw_handle() as HANDLE,
            &mut io_status,
            std::ptr::addr_of!(disposition).cast(),
            u32::try_from(size_of::<FileDispositionInformationEx>()).map_err(|_| {
                StoreError::Integrity("publication disposition buffer exceeds u32".to_owned())
            })?,
            FILE_DISPOSITION_INFORMATION_EX,
        );
        if status < 0 {
            return Err(StoreError::Io(format!(
                "handle-owned publication could not clear delete-on-close: NTSTATUS 0x{:08x}",
                status as u32
            )));
        }
    }
    Ok(())
}

#[cfg(not(any(all(target_os = "linux", target_arch = "x86_64"), windows)))]
fn publish_unpublished_file_platform(
    _unpublished: &HeldEntry,
    _parent: &HeldEntry,
    _name: &OsStr,
) -> Result<(), StoreError> {
    Err(StoreError::Integrity(
        "handle-owned publication supports Windows and Linux x64".to_owned(),
    ))
}

pub(crate) fn move_entry_noreplace(
    source_parent: &HeldEntry,
    source_name: &OsStr,
    source: &HeldEntry,
    destination_parent: &HeldEntry,
    destination_name: &OsStr,
) -> Result<(), StoreError> {
    require_normal_component(source_name, "cache move source")?;
    require_normal_component(destination_name, "cache move destination")?;
    if !same_volume_and_mount(source_parent, destination_parent)
        || !same_volume_and_mount(source_parent, source)
    {
        return Err(StoreError::Integrity(
            "cache move crossed its bound volume or mount".to_owned(),
        ));
    }
    move_entry_noreplace_platform(
        source_parent,
        source_name,
        source,
        destination_parent,
        destination_name,
    )
}

pub(crate) fn replace_entry_atomic(
    parent: &HeldEntry,
    source_name: &OsStr,
    source: &HeldEntry,
    destination_name: &OsStr,
) -> Result<(), StoreError> {
    require_normal_component(source_name, "state replacement source")?;
    require_normal_component(destination_name, "state replacement destination")?;
    if !same_volume_and_mount(parent, source) {
        return Err(StoreError::Integrity(
            "state replacement crossed its bound volume or mount".to_owned(),
        ));
    }
    replace_entry_atomic_platform(parent, source_name, source, destination_name)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub(crate) fn exchange_entries(
    parent: &HeldEntry,
    left_name: &OsStr,
    left: &HeldEntry,
    right_name: &OsStr,
    right: &HeldEntry,
) -> Result<(), StoreError> {
    require_normal_component(left_name, "migration exchange source")?;
    require_normal_component(right_name, "migration exchange target")?;
    if !same_volume_and_mount(parent, left) || !same_volume_and_mount(parent, right) {
        return Err(StoreError::Integrity(
            "migration exchange crossed its bound volume or mount".to_owned(),
        ));
    }
    exchange_entries_platform(parent, left_name, right_name)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[allow(
    unsafe_code,
    reason = "the supported Linux x64 lane requires renameat2 RENAME_EXCHANGE so neither migration object is disposed"
)]
fn exchange_entries_platform(
    parent: &HeldEntry,
    left_name: &OsStr,
    right_name: &OsStr,
) -> Result<(), StoreError> {
    use std::ffi::{CString, c_char, c_int, c_long, c_uint};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    const SYS_RENAMEAT2: c_long = 316;
    const RENAME_EXCHANGE: c_uint = 2;
    unsafe extern "C" {
        fn syscall(number: c_long, ...) -> c_long;
    }

    let left = CString::new(left_name.as_bytes())
        .map_err(|_| StoreError::Integrity("migration exchange source contains NUL".to_owned()))?;
    let right = CString::new(right_name.as_bytes())
        .map_err(|_| StoreError::Integrity("migration exchange target contains NUL".to_owned()))?;
    // SAFETY: the directory descriptor and both NUL-terminated component names
    // remain live for the syscall. RENAME_EXCHANGE preserves both objects.
    let result = unsafe {
        syscall(
            SYS_RENAMEAT2,
            parent.file().as_raw_fd() as c_int,
            left.as_ptr().cast::<c_char>(),
            parent.file().as_raw_fd() as c_int,
            right.as_ptr().cast::<c_char>(),
            RENAME_EXCHANGE,
        )
    };
    if result != 0 {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    Ok(())
}

fn require_normal_component(component: &OsStr, label: &str) -> Result<(), StoreError> {
    let path = Path::new(component);
    let mut components = path.components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(StoreError::Integrity(format!(
            "{label} must be one normal component"
        )));
    }
    Ok(())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[allow(
    unsafe_code,
    reason = "the supported Linux x64 lane requires renameat2 for a no-replace relative move"
)]
fn move_entry_noreplace_platform(
    source_parent: &HeldEntry,
    source_name: &OsStr,
    _source: &HeldEntry,
    destination_parent: &HeldEntry,
    destination_name: &OsStr,
) -> Result<(), StoreError> {
    use std::ffi::{CString, c_char, c_int, c_long, c_uint};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    const SYS_RENAMEAT2: c_long = 316;
    const RENAME_NOREPLACE: c_uint = 1;
    unsafe extern "C" {
        fn syscall(number: c_long, ...) -> c_long;
    }

    let source_name = CString::new(source_name.as_bytes())
        .map_err(|_| StoreError::Integrity("cache move source contains NUL".to_owned()))?;
    let destination_name = CString::new(destination_name.as_bytes())
        .map_err(|_| StoreError::Integrity("cache move destination contains NUL".to_owned()))?;
    // SAFETY: both parent descriptors and NUL-terminated component names remain
    // live for the syscall; RENAME_NOREPLACE forbids destination replacement.
    let result = unsafe {
        syscall(
            SYS_RENAMEAT2,
            source_parent.file().as_raw_fd() as c_int,
            source_name.as_ptr().cast::<c_char>(),
            destination_parent.file().as_raw_fd() as c_int,
            destination_name.as_ptr().cast::<c_char>(),
            RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[allow(
    unsafe_code,
    reason = "the supported Linux x64 lane requires a directory-handle-relative atomic replacement"
)]
fn replace_entry_atomic_platform(
    parent: &HeldEntry,
    source_name: &OsStr,
    _source: &HeldEntry,
    destination_name: &OsStr,
) -> Result<(), StoreError> {
    use std::ffi::{CString, c_char, c_int, c_long, c_uint};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    const SYS_RENAMEAT2: c_long = 316;
    const REPLACE_EXISTING: c_uint = 0;
    unsafe extern "C" {
        fn syscall(number: c_long, ...) -> c_long;
    }

    let source_name = CString::new(source_name.as_bytes())
        .map_err(|_| StoreError::Integrity("state replacement source contains NUL".to_owned()))?;
    let destination_name = CString::new(destination_name.as_bytes()).map_err(|_| {
        StoreError::Integrity("state replacement destination contains NUL".to_owned())
    })?;
    // SAFETY: the held directory and both component names remain live for the
    // syscall. Both lookups are relative to the authenticated directory.
    let result = unsafe {
        syscall(
            SYS_RENAMEAT2,
            parent.file().as_raw_fd() as c_int,
            source_name.as_ptr().cast::<c_char>(),
            parent.file().as_raw_fd() as c_int,
            destination_name.as_ptr().cast::<c_char>(),
            REPLACE_EXISTING,
        )
    };
    if result != 0 {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "Windows requires NtSetInformationFile to bind a no-replace move to the opened source and destination-parent handles"
)]
fn move_entry_noreplace_platform(
    _source_parent: &HeldEntry,
    _source_name: &OsStr,
    source: &HeldEntry,
    destination_parent: &HeldEntry,
    destination_name: &OsStr,
) -> Result<(), StoreError> {
    move_entry_windows(
        source,
        destination_parent,
        destination_name,
        false,
        "cache handle-bound move",
    )
}

#[cfg(windows)]
fn replace_entry_atomic_platform(
    parent: &HeldEntry,
    _source_name: &OsStr,
    source: &HeldEntry,
    destination_name: &OsStr,
) -> Result<(), StoreError> {
    move_entry_windows(
        source,
        parent,
        destination_name,
        true,
        "state handle-bound replacement",
    )
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "Windows requires NtSetInformationFile to bind replacement to the opened source and destination-parent handles"
)]
fn move_entry_windows(
    source: &HeldEntry,
    destination_parent: &HeldEntry,
    destination_name: &OsStr,
    replace_existing: bool,
    label: &str,
) -> Result<(), StoreError> {
    use std::mem::{offset_of, size_of};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::FILE_RENAME_INFO;

    #[repr(C)]
    struct IoStatusBlock {
        status_or_pointer: usize,
        information: usize,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtSetInformationFile(
            file_handle: HANDLE,
            io_status_block: *mut IoStatusBlock,
            file_information: *const core::ffi::c_void,
            length: u32,
            file_information_class: i32,
        ) -> i32;
    }

    const FILE_RENAME_INFORMATION: i32 = 10;
    const FILE_RENAME_INFORMATION_EX: i32 = 65;
    const FILE_RENAME_REPLACE_IF_EXISTS: u32 = 0x0000_0001;
    const FILE_RENAME_POSIX_SEMANTICS: u32 = 0x0000_0002;

    let name = destination_name.encode_wide().collect::<Vec<_>>();
    let name_bytes = name
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|length| u32::try_from(length).ok())
        .ok_or_else(|| StoreError::Integrity(format!("{label} destination is too long")))?;
    let bytes = offset_of!(FILE_RENAME_INFO, FileName)
        .checked_add(name.len() * size_of::<u16>())
        .ok_or_else(|| StoreError::Integrity(format!("{label} buffer overflow")))?;
    let words = bytes.div_ceil(size_of::<u64>());
    let mut buffer = vec![0_u64; words];
    let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: `buffer` is aligned by u64 and sized for the fixed header plus
    // the exact UTF-16 component payload. Both handles remain live for the call.
    unsafe {
        if replace_existing {
            (*information).Anonymous.Flags =
                FILE_RENAME_REPLACE_IF_EXISTS | FILE_RENAME_POSIX_SEMANTICS;
        } else {
            (*information).Anonymous.ReplaceIfExists = false;
        }
        (*information).RootDirectory = destination_parent.file().as_raw_handle() as HANDLE;
        (*information).FileNameLength = name_bytes;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*information).FileName).cast::<u16>(),
            name.len(),
        );
        let mut io_status = IoStatusBlock {
            status_or_pointer: 0,
            information: 0,
        };
        let status = NtSetInformationFile(
            source.file().as_raw_handle() as HANDLE,
            &mut io_status,
            information.cast(),
            u32::try_from(bytes)
                .map_err(|_| StoreError::Integrity(format!("{label} buffer exceeds u32")))?,
            if replace_existing {
                FILE_RENAME_INFORMATION_EX
            } else {
                FILE_RENAME_INFORMATION
            },
        );
        if status < 0 {
            return Err(StoreError::Io(format!(
                "{label} failed with NTSTATUS 0x{:08x}",
                status as u32
            )));
        }
    }
    Ok(())
}

#[cfg(not(any(all(target_os = "linux", target_arch = "x86_64"), windows)))]
fn move_entry_noreplace_platform(
    _source_parent: &HeldEntry,
    _source_name: &OsStr,
    _source: &HeldEntry,
    _destination_parent: &HeldEntry,
    _destination_name: &OsStr,
) -> Result<(), StoreError> {
    Err(StoreError::Integrity(
        "cache no-replace movement supports Windows and Linux x64".to_owned(),
    ))
}

#[cfg(not(any(all(target_os = "linux", target_arch = "x86_64"), windows)))]
fn replace_entry_atomic_platform(
    _parent: &HeldEntry,
    _source_name: &OsStr,
    _source: &HeldEntry,
    _destination_name: &OsStr,
) -> Result<(), StoreError> {
    Err(StoreError::Integrity(
        "state replacement supports Windows and Linux x64".to_owned(),
    ))
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
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_WRITE_THROUGH, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    if matches!(access, EntryAccess::ReadWrite) {
        options.write(true);
    } else if matches!(access, EntryAccess::Move) {
        let write = if matches!(kind, EntryKind::RegularFile) {
            GENERIC_WRITE
        } else {
            0
        };
        options.access_mode(GENERIC_READ | write | DELETE);
    }
    let directory = if matches!(kind, EntryKind::Directory) {
        FILE_FLAG_BACKUP_SEMANTICS
    } else {
        0
    };
    let write_through = if matches!(access, EntryAccess::Move) {
        FILE_FLAG_WRITE_THROUGH
    } else {
        0
    };
    options
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | directory | write_through)
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
fn create_new_movable_nofollow(path: &Path) -> std::io::Result<File> {
    create_new_nofollow(path)
}

#[cfg(windows)]
fn create_new_movable_nofollow(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_WRITE_THROUGH, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH)
        .open(path)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn create_new_movable_nofollow(_path: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "movable state publication supports Windows and Linux",
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

    match linux_statx_mount_id(file) {
        Ok(mount_id) => Ok(mount_id),
        Err(statx_error) => {
            let source = std::fs::read_to_string(format!(
                "/proc/self/fdinfo/{}",
                file.as_raw_fd()
            ))
            .map_err(|proc_error| {
                StoreError::Integrity(format!(
                    "cannot observe Linux mount ID through statx ({statx_error}) or procfs ({proc_error})"
                ))
            })?;
            parse_linux_mount_id(&source).map_err(|proc_error| {
                StoreError::Integrity(format!(
                    "cannot observe Linux mount ID through statx ({statx_error}) or procfs ({proc_error})"
                ))
            })
        }
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[repr(C)]
#[derive(Default)]
struct LinuxStatxTimestamp {
    _seconds: i64,
    _nanoseconds: u32,
    _reserved: i32,
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[repr(C)]
#[derive(Default)]
struct LinuxStatx {
    mask: u32,
    _block_size: u32,
    _attributes: u64,
    _links: u32,
    _user: u32,
    _group: u32,
    _mode: u16,
    _spare0: u16,
    _inode: u64,
    _size: u64,
    _blocks: u64,
    _attributes_mask: u64,
    _accessed: LinuxStatxTimestamp,
    _created: LinuxStatxTimestamp,
    _changed: LinuxStatxTimestamp,
    _modified: LinuxStatxTimestamp,
    _device_major: u32,
    _device_minor: u32,
    _filesystem_device_major: u32,
    _filesystem_device_minor: u32,
    mount_id: u64,
    _direct_io_memory_alignment: u32,
    _direct_io_offset_alignment: u32,
    _spare3: [u64; 12],
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const _: [(); 256] = [(); std::mem::size_of::<LinuxStatx>()];

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[allow(
    unsafe_code,
    reason = "the supported Linux x64 lane has no standard-library statx wrapper"
)]
fn linux_statx_mount_id(file: &File) -> std::io::Result<u64> {
    use std::ffi::{c_char, c_int, c_long};
    use std::os::fd::AsRawFd;

    const SYS_STATX: c_long = 332;
    const AT_EMPTY_PATH: c_int = 0x1000;
    const STATX_MNT_ID: u32 = 0x1000;
    const EMPTY_PATH: &[u8] = b"\0";

    unsafe extern "C" {
        fn syscall(number: c_long, ...) -> c_long;
    }

    let mut facts = LinuxStatx::default();
    // SAFETY: `file` owns a live descriptor, `EMPTY_PATH` is NUL-terminated,
    // and `LinuxStatx` is compile-time checked against the 256-byte Linux ABI.
    let result = unsafe {
        syscall(
            SYS_STATX,
            file.as_raw_fd(),
            EMPTY_PATH.as_ptr().cast::<c_char>(),
            AT_EMPTY_PATH,
            STATX_MNT_ID,
            &mut facts as *mut LinuxStatx,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if facts.mask & STATX_MNT_ID == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "statx omitted the Linux mount ID",
        ));
    }
    Ok(facts.mount_id)
}

#[cfg(all(target_os = "linux", not(target_arch = "x86_64")))]
fn linux_statx_mount_id(_file: &File) -> std::io::Result<u64> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "statx mount observation is supported on the required Linux x64 lane",
    ))
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
    use std::fs::File;

    use super::{linux_statx_mount_id, parse_linux_mount_id};

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

    #[test]
    fn observes_mount_identity_without_procfs() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("mount-id");
        std::fs::write(&path, b"identity")?;
        let file = File::open(path)?;

        assert!(linux_statx_mount_id(&file)? > 0);
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn file_facts(_file: &File) -> Result<FileFacts, StoreError> {
    Err(StoreError::Integrity(
        "managed state physical identity supports Windows and Linux".to_owned(),
    ))
}
