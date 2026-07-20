use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::codec::{CanonicalReadError, CanonicalReader};

const MAGIC: &[u8; 8] = b"LUMRPATH";
const VERSION: u16 = 1;

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct RepoPath {
    components: Vec<RepoPathComponent>,
    canonical: Vec<u8>,
}

#[derive(Clone, Eq, Hash, PartialEq)]
enum RepoPathComponent {
    PortableUtf8(String),
    #[cfg(unix)]
    UnixBytes(Vec<u8>),
    #[cfg(windows)]
    WindowsWtf16(Vec<u16>),
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RepoPathError {
    #[error("repository path must be relative")]
    NotRelative,
    #[error("repository path contains a forbidden component")]
    ForbiddenComponent,
    #[error("repository path component contains NUL or a separator")]
    InvalidComponent,
    #[error("portable repository path is not slash-normalized")]
    NonCanonicalPortablePath,
    #[error("repository path has too many or oversized components")]
    EncodingOverflow,
    #[error("repository path canonical bytes are malformed or noncanonical")]
    InvalidCanonicalEncoding,
}

impl From<CanonicalReadError> for RepoPathError {
    fn from(_: CanonicalReadError) -> Self {
        Self::InvalidCanonicalEncoding
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "platform", rename_all = "kebab-case")]
pub enum PhysicalFileIdentity {
    Unix { device: u64, inode: u64 },
    Windows { volume_serial: u32, file_index: u64 },
}

impl PhysicalFileIdentity {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(17);
        match self {
            Self::Unix { device, inode } => {
                bytes.push(1);
                bytes.extend_from_slice(&device.to_be_bytes());
                bytes.extend_from_slice(&inode.to_be_bytes());
            }
            Self::Windows {
                volume_serial,
                file_index,
            } => {
                bytes.push(2);
                bytes.extend_from_slice(&volume_serial.to_be_bytes());
                bytes.extend_from_slice(&file_index.to_be_bytes());
            }
        }
        bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicalAliasWriteClosure {
    pub physical_identity: PhysicalFileIdentity,
    pub members: Vec<RepoPath>,
}

impl RepoPath {
    pub fn empty() -> Self {
        let mut canonical = Vec::new();
        canonical.extend_from_slice(MAGIC);
        canonical.extend_from_slice(&VERSION.to_be_bytes());
        canonical.extend_from_slice(&0_u32.to_be_bytes());
        Self {
            components: Vec::new(),
            canonical,
        }
    }

    pub fn from_native_relative(path: &Path) -> Result<Self, RepoPathError> {
        if path.is_absolute() {
            return Err(RepoPathError::NotRelative);
        }

        let mut components = Vec::new();
        for component in path.components() {
            match component {
                Component::Normal(value) => components.push(native_component(value)?),
                Component::CurDir if components.is_empty() => {}
                Component::CurDir | Component::ParentDir => {
                    return Err(RepoPathError::ForbiddenComponent);
                }
                Component::Prefix(_) | Component::RootDir => {
                    return Err(RepoPathError::NotRelative);
                }
            }
        }
        Self::from_components(components)
    }

    pub fn from_portable(value: &str) -> Result<Self, RepoPathError> {
        if value.starts_with('/') || value.ends_with('/') || value.contains('\\') {
            return Err(RepoPathError::NonCanonicalPortablePath);
        }
        if value.is_empty() {
            return Ok(Self::empty());
        }

        let mut components = Vec::new();
        for component in value.split('/') {
            validate_scalar_component(component)?;
            components.push(RepoPathComponent::PortableUtf8(component.to_owned()));
        }
        Self::from_components(components)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, RepoPathError> {
        let mut reader = CanonicalReader::new(bytes);
        if reader.take(MAGIC.len())? != MAGIC {
            return Err(RepoPathError::InvalidCanonicalEncoding);
        }
        if reader.read_u16()? != VERSION {
            return Err(RepoPathError::InvalidCanonicalEncoding);
        }
        let component_count = usize::try_from(reader.read_u32()?)
            .map_err(|_| RepoPathError::InvalidCanonicalEncoding)?;
        let mut components = Vec::with_capacity(component_count);
        for _ in 0..component_count {
            let tag = reader.read_u8()?;
            let payload_len = usize::try_from(reader.read_u32()?)
                .map_err(|_| RepoPathError::InvalidCanonicalEncoding)?;
            let payload = reader.take(payload_len)?;
            components.push(decode_component(tag, payload)?);
        }
        if !reader.is_finished() {
            return Err(RepoPathError::InvalidCanonicalEncoding);
        }
        let path = Self::from_components(components)?;
        if path.canonical != bytes {
            return Err(RepoPathError::InvalidCanonicalEncoding);
        }
        Ok(path)
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical
    }

    pub fn component_keys(&self) -> Vec<Vec<u8>> {
        self.components
            .iter()
            .map(|component| {
                let (tag, payload) = component_payload(component);
                let mut key = Vec::with_capacity(payload.len() + 1);
                key.push(tag);
                key.extend_from_slice(&payload);
                key
            })
            .collect()
    }

    pub fn portable(&self) -> Option<String> {
        portable_components(&self.components)
    }

    pub fn portable_relative_to(&self, ancestor: &Self) -> Option<String> {
        let relative = self
            .components
            .strip_prefix(ancestor.components.as_slice())?;
        portable_components(relative)
    }

    pub fn display_escaped(&self) -> String {
        self.components
            .iter()
            .map(display_component)
            .collect::<Vec<_>>()
            .join("/")
    }

    pub fn file_name_portable(&self) -> Option<&str> {
        match self.components.last()? {
            RepoPathComponent::PortableUtf8(value) => Some(value),
            #[cfg(unix)]
            RepoPathComponent::UnixBytes(_) => None,
            #[cfg(windows)]
            RepoPathComponent::WindowsWtf16(_) => None,
        }
    }

    pub fn parent(&self) -> Option<Self> {
        if self.components.is_empty() {
            return None;
        }
        Self::from_components(self.components[..self.components.len() - 1].to_vec()).ok()
    }

    pub fn join_portable(&self, component: &str) -> Result<Self, RepoPathError> {
        validate_scalar_component(component)?;
        let mut components = self.components.clone();
        components.push(RepoPathComponent::PortableUtf8(component.to_owned()));
        Self::from_components(components)
    }

    pub fn resolve_portable_relative(&self, specifier: &str) -> Option<Self> {
        if specifier.starts_with('/') || specifier.starts_with('\\') || specifier.contains('\\') {
            return None;
        }
        let mut current = self.parent()?;
        for component in specifier.split('/') {
            match component {
                "" | "." => {}
                ".." => current = current.parent()?,
                value => current = current.join_portable(value).ok()?,
            }
        }
        Some(current)
    }

    pub fn components_len(&self) -> usize {
        self.components.len()
    }

    pub fn is_within(&self, ancestor: &Self) -> bool {
        self.components.starts_with(&ancestor.components)
    }

    pub fn to_native_relative(&self) -> PathBuf {
        let mut path = PathBuf::new();
        for component in &self.components {
            path.push(native_os_string(component));
        }
        path
    }

    fn from_components(components: Vec<RepoPathComponent>) -> Result<Self, RepoPathError> {
        let count = u32::try_from(components.len()).map_err(|_| RepoPathError::EncodingOverflow)?;
        let mut canonical = Vec::new();
        canonical.extend_from_slice(MAGIC);
        canonical.extend_from_slice(&VERSION.to_be_bytes());
        canonical.extend_from_slice(&count.to_be_bytes());

        for component in &components {
            let (tag, payload) = component_payload(component);
            let length =
                u32::try_from(payload.len()).map_err(|_| RepoPathError::EncodingOverflow)?;
            canonical.push(tag);
            canonical.extend_from_slice(&length.to_be_bytes());
            canonical.extend_from_slice(&payload);
        }

        Ok(Self {
            components,
            canonical,
        })
    }
}

fn decode_component(tag: u8, payload: &[u8]) -> Result<RepoPathComponent, RepoPathError> {
    match tag {
        1 => {
            let value = std::str::from_utf8(payload)
                .map_err(|_| RepoPathError::InvalidCanonicalEncoding)?;
            validate_scalar_component(value)?;
            Ok(RepoPathComponent::PortableUtf8(value.to_owned()))
        }
        #[cfg(unix)]
        2 => {
            use std::os::unix::ffi::OsStrExt;

            match native_component(OsStr::from_bytes(payload))? {
                component @ RepoPathComponent::UnixBytes(_) => Ok(component),
                RepoPathComponent::PortableUtf8(_) => Err(RepoPathError::InvalidCanonicalEncoding),
            }
        }
        #[cfg(windows)]
        3 => {
            use std::os::windows::ffi::OsStringExt;

            if !payload.len().is_multiple_of(2) {
                return Err(RepoPathError::InvalidCanonicalEncoding);
            }
            let units = payload
                .chunks_exact(2)
                .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                .collect::<Vec<_>>();
            let value = OsString::from_wide(&units);
            match native_component(&value)? {
                component @ RepoPathComponent::WindowsWtf16(_) => Ok(component),
                RepoPathComponent::PortableUtf8(_) => Err(RepoPathError::InvalidCanonicalEncoding),
            }
        }
        _ => Err(RepoPathError::InvalidCanonicalEncoding),
    }
}

fn portable_components(components: &[RepoPathComponent]) -> Option<String> {
    components
        .iter()
        .map(|component| match component {
            RepoPathComponent::PortableUtf8(value) => Some(value.as_str()),
            #[cfg(unix)]
            RepoPathComponent::UnixBytes(_) => None,
            #[cfg(windows)]
            RepoPathComponent::WindowsWtf16(_) => None,
        })
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.join("/"))
}

impl Ord for RepoPath {
    fn cmp(&self, other: &Self) -> Ordering {
        self.canonical.cmp(&other.canonical)
    }
}

impl PartialOrd for RepoPath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Debug for RepoPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RepoPath")
            .field(&self.display_escaped())
            .finish()
    }
}

fn validate_scalar_component(value: &str) -> Result<(), RepoPathError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('\0')
        || value.contains('/')
        || value.contains('\\')
    {
        return Err(RepoPathError::InvalidComponent);
    }
    Ok(())
}

#[cfg(unix)]
fn native_component(value: &OsStr) -> Result<RepoPathComponent, RepoPathError> {
    use std::os::unix::ffi::OsStrExt;

    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.contains(&0) || bytes.contains(&b'/') {
        return Err(RepoPathError::InvalidComponent);
    }
    if let Ok(text) = std::str::from_utf8(bytes)
        && !text.contains('\\')
    {
        validate_scalar_component(text)?;
        return Ok(RepoPathComponent::PortableUtf8(text.to_owned()));
    }
    Ok(RepoPathComponent::UnixBytes(bytes.to_vec()))
}

#[cfg(windows)]
fn native_component(value: &OsStr) -> Result<RepoPathComponent, RepoPathError> {
    use std::os::windows::ffi::OsStrExt;

    let units: Vec<u16> = value.encode_wide().collect();
    if units.is_empty() || units.contains(&0) || units.contains(&(b'\\' as u16)) {
        return Err(RepoPathError::InvalidComponent);
    }
    if let Ok(text) = String::from_utf16(&units) {
        validate_scalar_component(&text)?;
        return Ok(RepoPathComponent::PortableUtf8(text));
    }
    Ok(RepoPathComponent::WindowsWtf16(units))
}

fn component_payload(component: &RepoPathComponent) -> (u8, Vec<u8>) {
    match component {
        RepoPathComponent::PortableUtf8(value) => (1, value.as_bytes().to_vec()),
        #[cfg(unix)]
        RepoPathComponent::UnixBytes(value) => (2, value.clone()),
        #[cfg(windows)]
        RepoPathComponent::WindowsWtf16(value) => {
            let mut bytes = Vec::with_capacity(value.len() * 2);
            for unit in value {
                bytes.extend_from_slice(&unit.to_be_bytes());
            }
            (3, bytes)
        }
    }
}

fn native_os_string(component: &RepoPathComponent) -> OsString {
    match component {
        RepoPathComponent::PortableUtf8(value) => OsString::from(value),
        #[cfg(unix)]
        RepoPathComponent::UnixBytes(value) => {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(value.clone())
        }
        #[cfg(windows)]
        RepoPathComponent::WindowsWtf16(value) => {
            use std::os::windows::ffi::OsStringExt;
            OsString::from_wide(value)
        }
    }
}

fn display_component(component: &RepoPathComponent) -> String {
    match component {
        RepoPathComponent::PortableUtf8(value) => value.clone(),
        #[cfg(unix)]
        RepoPathComponent::UnixBytes(value) => {
            let mut output = String::from("$'");
            for byte in value {
                use std::fmt::Write;
                let _ = write!(output, "\\x{byte:02x}");
            }
            output.push('\'');
            output
        }
        #[cfg(windows)]
        RepoPathComponent::WindowsWtf16(value) => {
            let mut output = String::from("wtf16[");
            for (index, unit) in value.iter().enumerate() {
                use std::fmt::Write;
                if index > 0 {
                    output.push(',');
                }
                let _ = write!(output, "{unit:04x}");
            }
            output.push(']');
            output
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_path_uses_frozen_framing() -> Result<(), RepoPathError> {
        let path = RepoPath::from_portable("src/main.ts")?;
        assert_eq!(
            path.canonical_bytes(),
            b"LUMRPATH\x00\x01\x00\x00\x00\x02\x01\x00\x00\x00\x03src\x01\x00\x00\x00\x07main.ts"
        );
        assert_eq!(
            RepoPath::from_canonical_bytes(path.canonical_bytes())?,
            path
        );
        Ok(())
    }

    #[test]
    fn canonical_decoder_rejects_alternate_or_trailing_framing() -> Result<(), RepoPathError> {
        let portable_as_unix = b"LUMRPATH\x00\x01\x00\x00\x00\x01\x02\x00\x00\x00\x03src";
        assert!(RepoPath::from_canonical_bytes(portable_as_unix).is_err());

        let mut trailing = RepoPath::from_portable("src")?.canonical_bytes().to_vec();
        trailing.push(0);
        assert!(RepoPath::from_canonical_bytes(&trailing).is_err());
        Ok(())
    }

    #[test]
    fn portable_relative_path_uses_component_identity() -> Result<(), RepoPathError> {
        let root = RepoPath::from_portable("packages/core")?;
        let child = RepoPath::from_portable("packages/core/src/lib.ts")?;
        let sibling = RepoPath::from_portable("packages/core-extra/src/lib.ts")?;

        assert_eq!(
            child.portable_relative_to(&root).as_deref(),
            Some("src/lib.ts")
        );
        assert_eq!(root.portable_relative_to(&root).as_deref(), Some(""));
        assert_eq!(sibling.portable_relative_to(&root), None);
        Ok(())
    }

    #[test]
    fn resolves_relative_specifiers_without_crossing_the_root() -> Result<(), RepoPathError> {
        let importer = RepoPath::from_portable("packages/app/src/App.vue")?;
        assert_eq!(
            importer
                .resolve_portable_relative("../shared/card.ts")
                .map(|path| path.display_escaped())
                .as_deref(),
            Some("packages/app/shared/card.ts")
        );
        assert!(
            RepoPath::from_portable("App.vue")?
                .resolve_portable_relative("../outside.ts")
                .is_none()
        );
        assert!(
            importer
                .resolve_portable_relative(".\\outside.ts")
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn rejects_noncanonical_portable_components() {
        for value in [
            "/src",
            "src/",
            "src//main.ts",
            "src\\main.ts",
            "src/../main.ts",
        ] {
            assert!(RepoPath::from_portable(value).is_err(), "{value}");
        }
    }
}
