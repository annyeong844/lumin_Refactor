use std::ffi::OsStr;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::codec::{CanonicalReadError, CanonicalReader};
use crate::generated_path_codec::{
    REPOSITORY_ROOT_MAGIC, REPOSITORY_ROOT_VERSION, UNIX_ABSOLUTE_TAG, UNIX_PHYSICAL_IDENTITY_TAG,
    UNIX_PLATFORM_TAG, WINDOWS_DRIVE_TAG, WINDOWS_PHYSICAL_IDENTITY_TAG, WINDOWS_PLATFORM_TAG,
    WINDOWS_UNC_TAG, WINDOWS_VOLUME_GUID_TAG,
};
use crate::path::{RepoPathComponent, decode_component, display_component, portable_component};
use crate::{RepoPath, RepoPathError, RepositoryId};

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
    address: RepositoryRootAddress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootedPathRelation {
    NotRooted,
    Contained,
    Outside,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum RepositoryRootAddress {
    UnixAbsolute {
        components: Vec<RepoPathComponent>,
    },
    WindowsDrive {
        drive: u8,
        components: Vec<RepoPathComponent>,
    },
    WindowsUnc {
        server: RepoPathComponent,
        share: RepoPathComponent,
        components: Vec<RepoPathComponent>,
    },
    WindowsVolumeGuid {
        guid: [u8; 16],
        components: Vec<RepoPathComponent>,
    },
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
                UNIX_PLATFORM_TAG,
                UNIX_ABSOLUTE_TAG,
                &[],
                &components,
                physical_identity,
            )
        }
        #[cfg(windows)]
        {
            let address = windows_components(path)?;
            Self::encode(
                WINDOWS_PLATFORM_TAG,
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
        if reader.take(REPOSITORY_ROOT_MAGIC.len())? != REPOSITORY_ROOT_MAGIC
            || reader.read_u16()? != REPOSITORY_ROOT_VERSION
        {
            return Err(RepositoryRootError::InvalidCanonicalEncoding);
        }
        let platform = reader.read_u8()?;
        let address_kind = reader.read_u8()?;
        let address = read_address(&mut reader, platform, address_kind)?;
        let physical_identity = read_physical_identity(&mut reader, platform)?;
        if !reader.is_finished() {
            return Err(RepositoryRootError::InvalidCanonicalEncoding);
        }
        Ok(Self {
            canonical: bytes.to_vec(),
            physical_identity,
            address,
        })
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical
    }

    pub fn physical_identity(&self) -> &RepositoryRootPhysicalIdentity {
        &self.physical_identity
    }

    pub fn display_escaped(&self) -> String {
        render_address(&self.address, false).unwrap_or_else(|| "<nonportable-root>".to_owned())
    }

    pub fn readable_address(&self) -> Option<String> {
        render_address(&self.address, true)
    }

    pub fn classify_rooted_utf8_path(&self, value: &str) -> RootedPathRelation {
        let normalized = value.replace('\\', "/");
        let Some(address) = parse_rooted_utf8_address(&normalized, &self.address) else {
            return if rooted_utf8_syntax(&normalized) {
                RootedPathRelation::Outside
            } else {
                RootedPathRelation::NotRooted
            };
        };
        if rooted_address_is_within(&self.address, &address) {
            RootedPathRelation::Contained
        } else {
            RootedPathRelation::Outside
        }
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
        canonical.extend_from_slice(REPOSITORY_ROOT_MAGIC);
        canonical.extend_from_slice(&REPOSITORY_ROOT_VERSION.to_be_bytes());
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
            (WINDOWS_DRIVE_TAG, vec![drive])
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            let mut value = component_record(server)?;
            value.extend_from_slice(&component_record(share)?);
            (WINDOWS_UNC_TAG, value)
        }
        Prefix::Verbatim(value) => (
            WINDOWS_VOLUME_GUID_TAG,
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

fn read_address(
    reader: &mut CanonicalReader<'_>,
    platform: u8,
    address_kind: u8,
) -> Result<RepositoryRootAddress, RepositoryRootError> {
    match (platform, address_kind) {
        (UNIX_PLATFORM_TAG, UNIX_ABSOLUTE_TAG) => {
            let components = read_components(reader, UNIX_PLATFORM_TAG)?;
            Ok(RepositoryRootAddress::UnixAbsolute { components })
        }
        (WINDOWS_PLATFORM_TAG, WINDOWS_DRIVE_TAG) => {
            let drive = reader.read_u8()?;
            if !drive.is_ascii_uppercase() {
                return Err(RepositoryRootError::InvalidCanonicalEncoding);
            }
            let components = read_components(reader, WINDOWS_PLATFORM_TAG)?;
            Ok(RepositoryRootAddress::WindowsDrive { drive, components })
        }
        (WINDOWS_PLATFORM_TAG, WINDOWS_UNC_TAG) => {
            let server = read_component(reader, WINDOWS_PLATFORM_TAG)?;
            let share = read_component(reader, WINDOWS_PLATFORM_TAG)?;
            let components = read_components(reader, WINDOWS_PLATFORM_TAG)?;
            Ok(RepositoryRootAddress::WindowsUnc {
                server,
                share,
                components,
            })
        }
        (WINDOWS_PLATFORM_TAG, WINDOWS_VOLUME_GUID_TAG) => {
            let guid = reader
                .take(16)?
                .try_into()
                .map_err(|_| RepositoryRootError::InvalidCanonicalEncoding)?;
            let components = read_components(reader, WINDOWS_PLATFORM_TAG)?;
            Ok(RepositoryRootAddress::WindowsVolumeGuid { guid, components })
        }
        _ => Err(RepositoryRootError::InvalidCanonicalEncoding),
    }
}

fn read_components(
    reader: &mut CanonicalReader<'_>,
    platform: u8,
) -> Result<Vec<RepoPathComponent>, RepositoryRootError> {
    let component_count = usize::try_from(reader.read_u32()?)
        .map_err(|_| RepositoryRootError::InvalidCanonicalEncoding)?;
    (0..component_count)
        .map(|_| read_component(reader, platform))
        .collect()
}

fn read_component(
    reader: &mut CanonicalReader<'_>,
    platform: u8,
) -> Result<RepoPathComponent, RepositoryRootError> {
    let tag = reader.read_u8()?;
    let length = usize::try_from(reader.read_u32()?)
        .map_err(|_| RepositoryRootError::InvalidCanonicalEncoding)?;
    let payload = reader.take(length)?;
    validate_component(platform, tag, payload)?;
    decode_component(tag, payload).map_err(Into::into)
}

fn validate_component(platform: u8, tag: u8, payload: &[u8]) -> Result<(), RepositoryRootError> {
    match tag {
        1 => validate_portable(payload),
        2 if platform == UNIX_PLATFORM_TAG => {
            validate_native_bytes(payload, b'/')?;
            if let Ok(value) = std::str::from_utf8(payload)
                && !value.contains('\\')
                && valid_portable(value)
            {
                return Err(RepositoryRootError::InvalidCanonicalEncoding);
            }
            Ok(())
        }
        3 if platform == WINDOWS_PLATFORM_TAG => {
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
        (UNIX_PLATFORM_TAG, UNIX_PHYSICAL_IDENTITY_TAG) => {
            Ok(RepositoryRootPhysicalIdentity::Unix {
                device: reader.read_u64()?,
                inode: reader.read_u64()?,
            })
        }
        (WINDOWS_PLATFORM_TAG, WINDOWS_PHYSICAL_IDENTITY_TAG) => {
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
            output.push(UNIX_PHYSICAL_IDENTITY_TAG);
            output.extend_from_slice(&device.to_be_bytes());
            output.extend_from_slice(&inode.to_be_bytes());
        }
        RepositoryRootPhysicalIdentity::Windows {
            volume_serial,
            file_id,
        } => {
            output.push(WINDOWS_PHYSICAL_IDENTITY_TAG);
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
        (
            UNIX_PLATFORM_TAG,
            RepositoryRootPhysicalIdentity::Unix { .. }
        ) | (
            WINDOWS_PLATFORM_TAG,
            RepositoryRootPhysicalIdentity::Windows { .. }
        )
    ) {
        Ok(())
    } else {
        Err(RepositoryRootError::PlatformMismatch)
    }
}

fn render_address(address: &RepositoryRootAddress, portable_only: bool) -> Option<String> {
    match address {
        RepositoryRootAddress::UnixAbsolute { components } => {
            render_components("/", components, portable_only)
        }
        RepositoryRootAddress::WindowsDrive { drive, components } => {
            render_components(&format!("{}:/", *drive as char), components, portable_only)
        }
        RepositoryRootAddress::WindowsUnc {
            server,
            share,
            components,
        } => {
            let server = render_root_component(server, portable_only)?;
            let share = render_root_component(share, portable_only)?;
            let prefix = format!("//{server}/{share}");
            if components.is_empty() {
                Some(prefix)
            } else {
                render_components(&format!("{prefix}/"), components, portable_only)
            }
        }
        RepositoryRootAddress::WindowsVolumeGuid { guid, components } => render_components(
            &format!("//?/Volume{{{}}}/", format_guid(guid)),
            components,
            portable_only,
        ),
    }
}

enum Utf8RootedAddress<'a> {
    Unix(Vec<&'a str>),
    WindowsDrive {
        drive: u8,
        components: Vec<&'a str>,
    },
    WindowsUnc {
        server: &'a str,
        share: &'a str,
        components: Vec<&'a str>,
    },
    WindowsVolumeGuid {
        guid: [u8; 16],
        components: Vec<&'a str>,
    },
}

fn parse_rooted_utf8_address<'a>(
    value: &'a str,
    repository: &RepositoryRootAddress,
) -> Option<Utf8RootedAddress<'a>> {
    if let Some(rest) = value.strip_prefix("//?/Volume{") {
        let (guid, components) = rest.split_once("}/")?;
        return Some(Utf8RootedAddress::WindowsVolumeGuid {
            guid: parse_guid_body(guid)?,
            components: normalize_utf8_components(components)?,
        });
    }
    if let Some(rest) = value
        .strip_prefix("//?/UNC/")
        .or_else(|| value.strip_prefix("//./UNC/"))
    {
        return parse_unc_address(rest);
    }
    if let Some(rest) = value
        .strip_prefix("//?/")
        .or_else(|| value.strip_prefix("//./"))
    {
        return parse_drive_address(rest);
    }
    if let Some(rest) = value.strip_prefix("//") {
        return parse_unc_address(rest);
    }
    if value.as_bytes().get(1) == Some(&b':') && value.as_bytes().get(2) == Some(&b'/') {
        return parse_drive_address(value);
    }
    let rest = value.strip_prefix('/')?;
    let components = normalize_utf8_components(rest)?;
    match repository {
        RepositoryRootAddress::UnixAbsolute { .. } => Some(Utf8RootedAddress::Unix(components)),
        RepositoryRootAddress::WindowsDrive { drive, .. } => {
            Some(Utf8RootedAddress::WindowsDrive {
                drive: *drive,
                components,
            })
        }
        RepositoryRootAddress::WindowsUnc { .. }
        | RepositoryRootAddress::WindowsVolumeGuid { .. } => None,
    }
}

fn rooted_utf8_syntax(value: &str) -> bool {
    value.starts_with('/') || value.as_bytes().get(1) == Some(&b':')
}

fn parse_drive_address(value: &str) -> Option<Utf8RootedAddress<'_>> {
    let bytes = value.as_bytes();
    if bytes.get(1) != Some(&b':') || bytes.get(2) != Some(&b'/') {
        return None;
    }
    let drive = bytes[0].to_ascii_uppercase();
    if !drive.is_ascii_uppercase() {
        return None;
    }
    Some(Utf8RootedAddress::WindowsDrive {
        drive,
        components: normalize_utf8_components(&value[3..])?,
    })
}

fn parse_unc_address(value: &str) -> Option<Utf8RootedAddress<'_>> {
    let mut parts = value.splitn(3, '/');
    let server = parts.next()?;
    let share = parts.next()?;
    if server.is_empty() || share.is_empty() {
        return None;
    }
    Some(Utf8RootedAddress::WindowsUnc {
        server,
        share,
        components: normalize_utf8_components(parts.next().unwrap_or_default())?,
    })
}

fn normalize_utf8_components(value: &str) -> Option<Vec<&str>> {
    let mut components = Vec::new();
    for component in value.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            value if value.contains('\0') => return None,
            value => components.push(value),
        }
    }
    Some(components)
}

fn rooted_address_is_within(
    repository: &RepositoryRootAddress,
    candidate: &Utf8RootedAddress<'_>,
) -> bool {
    let same_address = match (repository, candidate) {
        (
            RepositoryRootAddress::UnixAbsolute { components: root },
            Utf8RootedAddress::Unix(candidate),
        )
        | (
            RepositoryRootAddress::WindowsDrive {
                components: root, ..
            },
            Utf8RootedAddress::WindowsDrive {
                components: candidate,
                ..
            },
        ) => address_components_start_with(candidate, root),
        (
            RepositoryRootAddress::WindowsUnc {
                server,
                share,
                components: root,
            },
            Utf8RootedAddress::WindowsUnc {
                server: candidate_server,
                share: candidate_share,
                components: candidate,
            },
        ) => {
            component_matches_utf8(server, candidate_server)
                && component_matches_utf8(share, candidate_share)
                && address_components_start_with(candidate, root)
        }
        (
            RepositoryRootAddress::WindowsVolumeGuid {
                guid,
                components: root,
            },
            Utf8RootedAddress::WindowsVolumeGuid {
                guid: candidate_guid,
                components: candidate,
            },
        ) => guid == candidate_guid && address_components_start_with(candidate, root),
        _ => false,
    };
    same_address
        && match (repository, candidate) {
            (
                RepositoryRootAddress::WindowsDrive { drive, .. },
                Utf8RootedAddress::WindowsDrive {
                    drive: candidate_drive,
                    ..
                },
            ) => drive == candidate_drive,
            _ => true,
        }
}

fn address_components_start_with(candidate: &[&str], root: &[RepoPathComponent]) -> bool {
    candidate.len() >= root.len()
        && root
            .iter()
            .zip(candidate)
            .all(|(root, candidate)| component_matches_utf8(root, candidate))
}

fn component_matches_utf8(component: &RepoPathComponent, value: &str) -> bool {
    match component {
        RepoPathComponent::PortableUtf8(component) => component == value,
        RepoPathComponent::UnixBytes(component) => component == value.as_bytes(),
        RepoPathComponent::WindowsWtf16(component) => {
            component.iter().copied().eq(value.encode_utf16())
        }
    }
}

fn render_components(
    prefix: &str,
    components: &[RepoPathComponent],
    portable_only: bool,
) -> Option<String> {
    let rendered = components
        .iter()
        .map(|component| render_root_component(component, portable_only))
        .collect::<Option<Vec<_>>>()?;
    Some(format!("{prefix}{}", rendered.join("/")))
}

fn render_root_component(component: &RepoPathComponent, portable_only: bool) -> Option<String> {
    if portable_only {
        portable_component(component).map(str::to_owned)
    } else {
        Some(display_component(component))
    }
}

fn format_guid(guid: &[u8; 16]) -> String {
    use std::fmt::Write;

    let mut output = String::with_capacity(36);
    for (index, byte) in guid.iter().enumerate() {
        if [4, 6, 8, 10].contains(&index) {
            output.push('-');
        }
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(windows)]
fn parse_volume_guid(value: &OsStr) -> Option<Vec<u8>> {
    let value = value.to_str()?;
    let body = value.strip_prefix("Volume{")?.strip_suffix('}')?;
    Some(parse_guid_body(body)?.to_vec())
}

fn parse_guid_body(body: &str) -> Option<[u8; 16]> {
    let bytes = body.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    let mut parsed = [0_u8; 16];
    let mut digit_index = 0;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            if byte != b'-' {
                return None;
            }
            continue;
        }
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        let output = parsed.get_mut(digit_index / 2)?;
        if digit_index % 2 == 0 {
            *output = digit << 4;
        } else {
            *output |= digit;
        }
        digit_index += 1;
    }
    (digit_index == 32).then_some(parsed)
}

#[cfg(test)]
mod tests;
