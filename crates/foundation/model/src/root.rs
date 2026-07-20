use std::ffi::OsStr;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::codec::{CanonicalReadError, CanonicalReader};
use crate::{RepoPath, RepoPathError, RepositoryId};

const ROOT_MAGIC: &[u8; 8] = b"LUMRROOT";
const ROOT_VERSION: u16 = 1;
const UNIX_PLATFORM: u8 = 1;
const WINDOWS_PLATFORM: u8 = 2;
const UNIX_ABSOLUTE: u8 = 1;
const WINDOWS_DRIVE: u8 = 2;
const WINDOWS_UNC: u8 = 3;
const WINDOWS_VOLUME_GUID: u8 = 4;

#[cfg(windows)]
struct WindowsRootAddress {
    kind: u8,
    prefix: Vec<u8>,
    components: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(tag = "platform", rename_all = "kebab-case")]
pub enum RepositoryRootPhysicalIdentity {
    Unix {
        device: u64,
        inode: u64,
    },
    Windows {
        volume_serial: u64,
        file_id: [u8; 16],
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RepositoryRootIdentity {
    canonical: Vec<u8>,
    physical_identity: RepositoryRootPhysicalIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryBinding {
    repository_id: RepositoryId,
    root: RepositoryRootIdentity,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RepositoryRootError {
    #[error("repository root must be an absolute canonical native path")]
    NotAbsolute,
    #[error("repository root address form is unsupported")]
    UnsupportedAddress,
    #[error("repository root platform and physical identity disagree")]
    PlatformMismatch,
    #[error("repository root canonical bytes are malformed or noncanonical")]
    InvalidCanonicalEncoding,
    #[error(transparent)]
    InvalidComponent(#[from] RepoPathError),
}

impl From<CanonicalReadError> for RepositoryRootError {
    fn from(_: CanonicalReadError) -> Self {
        Self::InvalidCanonicalEncoding
    }
}

impl RepositoryBinding {
    pub fn new(root: RepositoryRootIdentity) -> Self {
        let repository_id = RepositoryId::for_root(&root);
        Self {
            repository_id,
            root,
        }
    }

    pub fn repository_id(&self) -> &RepositoryId {
        &self.repository_id
    }

    pub fn root(&self) -> &RepositoryRootIdentity {
        &self.root
    }
}

impl RepositoryRootIdentity {
    pub fn from_native_absolute(
        path: &Path,
        physical_identity: RepositoryRootPhysicalIdentity,
    ) -> Result<Self, RepositoryRootError> {
        #[cfg(unix)]
        {
            let components = unix_components(path)?;
            Self::encode(
                UNIX_PLATFORM,
                UNIX_ABSOLUTE,
                &[],
                &components,
                physical_identity,
            )
        }
        #[cfg(windows)]
        {
            let address = windows_components(path)?;
            Self::encode(
                WINDOWS_PLATFORM,
                address.kind,
                &address.prefix,
                &address.components,
                physical_identity,
            )
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (path, physical_identity);
            Err(RepositoryRootError::UnsupportedAddress)
        }
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, RepositoryRootError> {
        let mut reader = CanonicalReader::new(bytes);
        if reader.take(ROOT_MAGIC.len())? != ROOT_MAGIC || reader.read_u16()? != ROOT_VERSION {
            return Err(RepositoryRootError::InvalidCanonicalEncoding);
        }
        let platform = reader.read_u8()?;
        let address_kind = reader.read_u8()?;
        read_address_prefix(&mut reader, platform, address_kind)?;
        let component_count = reader.read_u32()?;
        for _ in 0..component_count {
            read_component(&mut reader, platform)?;
        }
        let physical_identity = read_physical_identity(&mut reader, platform)?;
        if !reader.is_finished() {
            return Err(RepositoryRootError::InvalidCanonicalEncoding);
        }
        Ok(Self {
            canonical: bytes.to_vec(),
            physical_identity,
        })
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical
    }

    pub fn physical_identity(&self) -> &RepositoryRootPhysicalIdentity {
        &self.physical_identity
    }

    fn encode(
        platform: u8,
        address_kind: u8,
        prefix: &[u8],
        components: &[Vec<u8>],
        physical_identity: RepositoryRootPhysicalIdentity,
    ) -> Result<Self, RepositoryRootError> {
        validate_platform_identity(platform, &physical_identity)?;
        let count = u32::try_from(components.len())
            .map_err(|_| RepositoryRootError::InvalidCanonicalEncoding)?;
        let mut canonical = Vec::new();
        canonical.extend_from_slice(ROOT_MAGIC);
        canonical.extend_from_slice(&ROOT_VERSION.to_be_bytes());
        canonical.push(platform);
        canonical.push(address_kind);
        canonical.extend_from_slice(prefix);
        canonical.extend_from_slice(&count.to_be_bytes());
        for component in components {
            canonical.extend_from_slice(component);
        }
        append_physical_identity(&mut canonical, &physical_identity);
        Self::from_canonical_bytes(&canonical)
    }
}

#[cfg(unix)]
fn unix_components(path: &Path) -> Result<Vec<Vec<u8>>, RepositoryRootError> {
    if !path.is_absolute() {
        return Err(RepositoryRootError::NotAbsolute);
    }
    let mut saw_root = false;
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir if !saw_root => saw_root = true,
            Component::Normal(value) if saw_root => components.push(component_record(value)?),
            _ => return Err(RepositoryRootError::NotAbsolute),
        }
    }
    if !saw_root {
        return Err(RepositoryRootError::NotAbsolute);
    }
    Ok(components)
}

#[cfg(windows)]
fn windows_components(path: &Path) -> Result<WindowsRootAddress, RepositoryRootError> {
    use std::path::Prefix;

    let mut parts = path.components();
    let prefix = match parts.next() {
        Some(Component::Prefix(prefix)) => prefix.kind(),
        _ => return Err(RepositoryRootError::NotAbsolute),
    };
    if !matches!(parts.next(), Some(Component::RootDir)) {
        return Err(RepositoryRootError::NotAbsolute);
    }
    let (address_kind, address_prefix) = match prefix {
        Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
            let drive = drive.to_ascii_uppercase();
            if !drive.is_ascii_uppercase() {
                return Err(RepositoryRootError::UnsupportedAddress);
            }
            (WINDOWS_DRIVE, vec![drive])
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            let mut value = component_record(server)?;
            value.extend_from_slice(&component_record(share)?);
            (WINDOWS_UNC, value)
        }
        Prefix::Verbatim(value) => (
            WINDOWS_VOLUME_GUID,
            parse_volume_guid(value).ok_or(RepositoryRootError::UnsupportedAddress)?,
        ),
        Prefix::DeviceNS(_) => return Err(RepositoryRootError::UnsupportedAddress),
    };
    let components = parts
        .map(|component| match component {
            Component::Normal(value) => component_record(value),
            _ => Err(RepositoryRootError::NotAbsolute),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(WindowsRootAddress {
        kind: address_kind,
        prefix: address_prefix,
        components,
    })
}

fn component_record(value: &OsStr) -> Result<Vec<u8>, RepositoryRootError> {
    let path = RepoPath::from_native_relative(Path::new(value))?;
    let key = path
        .component_keys()
        .into_iter()
        .next()
        .ok_or(RepositoryRootError::InvalidCanonicalEncoding)?;
    let (tag, payload) = key
        .split_first()
        .ok_or(RepositoryRootError::InvalidCanonicalEncoding)?;
    let length =
        u32::try_from(payload.len()).map_err(|_| RepositoryRootError::InvalidCanonicalEncoding)?;
    let mut record = Vec::with_capacity(payload.len() + 5);
    record.push(*tag);
    record.extend_from_slice(&length.to_be_bytes());
    record.extend_from_slice(payload);
    Ok(record)
}

fn read_address_prefix(
    reader: &mut CanonicalReader<'_>,
    platform: u8,
    address_kind: u8,
) -> Result<(), RepositoryRootError> {
    match (platform, address_kind) {
        (UNIX_PLATFORM, UNIX_ABSOLUTE) => Ok(()),
        (WINDOWS_PLATFORM, WINDOWS_DRIVE) => {
            let drive = reader.read_u8()?;
            if drive.is_ascii_uppercase() {
                Ok(())
            } else {
                Err(RepositoryRootError::InvalidCanonicalEncoding)
            }
        }
        (WINDOWS_PLATFORM, WINDOWS_UNC) => {
            read_component(reader, WINDOWS_PLATFORM)?;
            read_component(reader, WINDOWS_PLATFORM)
        }
        (WINDOWS_PLATFORM, WINDOWS_VOLUME_GUID) => reader.take(16).map(|_| ()).map_err(Into::into),
        _ => Err(RepositoryRootError::InvalidCanonicalEncoding),
    }
}

fn read_component(
    reader: &mut CanonicalReader<'_>,
    platform: u8,
) -> Result<(), RepositoryRootError> {
    let tag = reader.read_u8()?;
    let length = usize::try_from(reader.read_u32()?)
        .map_err(|_| RepositoryRootError::InvalidCanonicalEncoding)?;
    let payload = reader.take(length)?;
    validate_component(platform, tag, payload)
}

fn validate_component(platform: u8, tag: u8, payload: &[u8]) -> Result<(), RepositoryRootError> {
    match tag {
        1 => validate_portable(payload),
        2 if platform == UNIX_PLATFORM => {
            validate_native_bytes(payload, b'/')?;
            if let Ok(value) = std::str::from_utf8(payload)
                && !value.contains('\\')
                && valid_portable(value)
            {
                return Err(RepositoryRootError::InvalidCanonicalEncoding);
            }
            Ok(())
        }
        3 if platform == WINDOWS_PLATFORM => {
            if payload.is_empty() || !payload.len().is_multiple_of(2) {
                return Err(RepositoryRootError::InvalidCanonicalEncoding);
            }
            let units = payload
                .chunks_exact(2)
                .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                .collect::<Vec<_>>();
            if units.contains(&0)
                || units.contains(&(b'/' as u16))
                || units.contains(&(b'\\' as u16))
            {
                return Err(RepositoryRootError::InvalidCanonicalEncoding);
            }
            if String::from_utf16(&units).is_ok() {
                return Err(RepositoryRootError::InvalidCanonicalEncoding);
            }
            Ok(())
        }
        _ => Err(RepositoryRootError::InvalidCanonicalEncoding),
    }
}

fn validate_portable(payload: &[u8]) -> Result<(), RepositoryRootError> {
    let value =
        std::str::from_utf8(payload).map_err(|_| RepositoryRootError::InvalidCanonicalEncoding)?;
    if valid_portable(value) {
        Ok(())
    } else {
        Err(RepositoryRootError::InvalidCanonicalEncoding)
    }
}

fn valid_portable(value: &str) -> bool {
    !value.is_empty() && value != "." && value != ".." && !value.contains(['\0', '/', '\\'])
}

fn validate_native_bytes(payload: &[u8], separator: u8) -> Result<(), RepositoryRootError> {
    if payload.is_empty() || payload.contains(&0) || payload.contains(&separator) {
        Err(RepositoryRootError::InvalidCanonicalEncoding)
    } else {
        Ok(())
    }
}

fn read_physical_identity(
    reader: &mut CanonicalReader<'_>,
    platform: u8,
) -> Result<RepositoryRootPhysicalIdentity, RepositoryRootError> {
    match (platform, reader.read_u8()?) {
        (UNIX_PLATFORM, 1) => Ok(RepositoryRootPhysicalIdentity::Unix {
            device: reader.read_u64()?,
            inode: reader.read_u64()?,
        }),
        (WINDOWS_PLATFORM, 2) => {
            let volume_serial = reader.read_u64()?;
            let file_id: [u8; 16] = reader
                .take(16)?
                .try_into()
                .map_err(|_| RepositoryRootError::InvalidCanonicalEncoding)?;
            Ok(RepositoryRootPhysicalIdentity::Windows {
                volume_serial,
                file_id,
            })
        }
        _ => Err(RepositoryRootError::InvalidCanonicalEncoding),
    }
}

fn append_physical_identity(output: &mut Vec<u8>, identity: &RepositoryRootPhysicalIdentity) {
    match identity {
        RepositoryRootPhysicalIdentity::Unix { device, inode } => {
            output.push(1);
            output.extend_from_slice(&device.to_be_bytes());
            output.extend_from_slice(&inode.to_be_bytes());
        }
        RepositoryRootPhysicalIdentity::Windows {
            volume_serial,
            file_id,
        } => {
            output.push(2);
            output.extend_from_slice(&volume_serial.to_be_bytes());
            output.extend_from_slice(file_id);
        }
    }
}

fn validate_platform_identity(
    platform: u8,
    identity: &RepositoryRootPhysicalIdentity,
) -> Result<(), RepositoryRootError> {
    if matches!(
        (platform, identity),
        (UNIX_PLATFORM, RepositoryRootPhysicalIdentity::Unix { .. })
            | (
                WINDOWS_PLATFORM,
                RepositoryRootPhysicalIdentity::Windows { .. }
            )
    ) {
        Ok(())
    } else {
        Err(RepositoryRootError::PlatformMismatch)
    }
}

#[cfg(windows)]
fn parse_volume_guid(value: &OsStr) -> Option<Vec<u8>> {
    let value = value.to_str()?;
    let body = value.strip_prefix("Volume{")?.strip_suffix('}')?;
    if body.len() != 36
        || ![8, 13, 18, 23]
            .into_iter()
            .all(|index| body.as_bytes()[index] == b'-')
    {
        return None;
    }
    let hex = body.replace('-', "");
    (0..16)
        .map(|index| u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests;
