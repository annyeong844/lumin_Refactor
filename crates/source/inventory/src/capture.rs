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
        let file = open_source_file(native_path).map_err(|error| {
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
    #[cfg(target_os = "linux")]
    {
        let file = open_identity_file(path)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        physical_file_observation_from_file(&file)
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        let metadata = std::fs::metadata(path)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        Ok(unix_observation(&metadata, None))
    }
    #[cfg(windows)]
    {
        let handle = winapi_util::Handle::from_path_any(path)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        let information = winapi_util::file::information(&handle)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        windows_observation(&information)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let file = File::open(path)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        physical_file_observation_from_file(&file)
    }
}

pub(crate) fn physical_file_observation_from_file(
    file: &File,
) -> Result<PhysicalFileObservation, InventoryError> {
    #[cfg(unix)]
    {
        let metadata = file
            .metadata()
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        Ok(unix_observation(&metadata, linux_mount_id(file)?))
    }
    #[cfg(windows)]
    {
        let information = winapi_util::file::information(file)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        windows_observation(&information)
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn open_source_file(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    const O_NONBLOCK: i32 = 0x800;
    File::options()
        .read(true)
        .custom_flags(O_NONBLOCK)
        .open(path)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn open_source_file(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(target_os = "linux")]
fn open_identity_file(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    const O_PATH: i32 = 0x20_0000;
    File::options().read(true).custom_flags(O_PATH).open(path)
}

#[cfg(unix)]
fn unix_observation(
    metadata: &std::fs::Metadata,
    mount_id: Option<u64>,
) -> PhysicalFileObservation {
    use std::os::unix::fs::MetadataExt;

    PhysicalFileObservation {
        identity: PhysicalFileIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
        links: metadata.nlink(),
        mount_id,
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

    match linux_statx_mount_id(file) {
        Ok(mount_id) => Ok(Some(mount_id)),
        Err(statx_error) => {
            let source = std::fs::read_to_string(format!(
                "/proc/self/fdinfo/{}",
                file.as_raw_fd()
            ))
            .map_err(|proc_error| {
                InventoryError::PhysicalIdentity(format!(
                    "cannot observe Linux mount ID through statx ({statx_error}) or procfs ({proc_error})"
                ))
            })?;
            parse_linux_mount_id(&source).map(Some).map_err(|proc_error| {
                InventoryError::PhysicalIdentity(format!(
                    "cannot observe Linux mount ID through statx ({statx_error}) or procfs ({proc_error})"
                ))
            })
        }
    }
}

#[cfg(target_os = "linux")]
fn parse_linux_mount_id(source: &str) -> Result<u64, InventoryError> {
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
    mount_id.ok_or_else(|| {
        InventoryError::PhysicalIdentity("opened input omitted its Linux mount ID".to_owned())
    })
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

#[cfg(all(unix, not(target_os = "linux")))]
fn linux_mount_id(_file: &File) -> Result<Option<u64>, InventoryError> {
    Ok(None)
}
