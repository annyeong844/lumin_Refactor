use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::codec::{CanonicalReadError, CanonicalReader};
use crate::generated_path_codec::{
    PORTABLE_UTF8_TAG, REPO_PATH_MAGIC, REPO_PATH_VERSION, UNIX_BYTES_TAG, WINDOWS_WTF16_TAG,
};

mod native_io;

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct RepoPath {
    components: Vec<RepoPathComponent>,
    canonical: Vec<u8>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RepoPathMatchBytes(Vec<u8>);

impl RepoPathMatchBytes {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn components(&self) -> impl Iterator<Item = &[u8]> {
        self.0.split(|byte| *byte == b'/')
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum RepoPathComponent {
    PortableUtf8(String),
    UnixBytes(Vec<u8>),
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
    #[error("repository path contains a component for another native platform")]
    ForeignPlatformComponent,
    #[error("native NUL path stream is malformed or noncanonical")]
    InvalidNativeNulStream,
}

pub fn encode_native_path_component(value: &OsStr) -> Result<Vec<u8>, RepoPathError> {
    let component = native_io::native_component(value)?;
    let (tag, payload) = component_payload(&component);
    let mut canonical = Vec::with_capacity(payload.len() + 1);
    canonical.push(tag);
    canonical.extend_from_slice(&payload);
    Ok(canonical)
}

pub fn decode_native_path_component(canonical: &[u8]) -> Result<OsString, RepoPathError> {
    let (&tag, payload) = canonical
        .split_first()
        .ok_or(RepoPathError::InvalidCanonicalEncoding)?;
    let component = decode_component(tag, payload)?;
    let (observed_tag, observed_payload) = component_payload(&component);
    if observed_tag != tag || observed_payload != payload {
        return Err(RepoPathError::InvalidCanonicalEncoding);
    }
    native_io::native_os_string(&component)
}

pub fn portable_path_component(canonical: &[u8]) -> Result<Option<String>, RepoPathError> {
    let (&tag, payload) = canonical
        .split_first()
        .ok_or(RepoPathError::InvalidCanonicalEncoding)?;
    let component = decode_component(tag, payload)?;
    let (observed_tag, observed_payload) = component_payload(&component);
    if observed_tag != tag || observed_payload != payload {
        return Err(RepoPathError::InvalidCanonicalEncoding);
    }
    Ok(portable_component(&component).map(str::to_owned))
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PhysicalPathRedirectTarget {
    Repository(RepoPath),
    OutsideRepository,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PhysicalPathRedirectKind {
    File,
    Directory,
    Other,
    Unavailable,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PhysicalPathRedirect {
    pub path: RepoPath,
    pub target: PhysicalPathRedirectTarget,
    pub kind: PhysicalPathRedirectKind,
    pub entry_physical_identity: Option<PhysicalFileIdentity>,
    pub target_physical_identity: Option<PhysicalFileIdentity>,
    pub target_identity_sha256: String,
}

impl PhysicalPathRedirect {
    pub fn semantic_sha256(&self) -> String {
        let mut framed = Vec::new();
        crate::append_length_prefixed(&mut framed, self.path.canonical_bytes());
        crate::append_length_prefixed(&mut framed, self.target_identity_sha256.as_bytes());
        framed.push(match self.kind {
            PhysicalPathRedirectKind::File => 1,
            PhysicalPathRedirectKind::Directory => 2,
            PhysicalPathRedirectKind::Other => 3,
            PhysicalPathRedirectKind::Unavailable => 4,
        });
        match &self.entry_physical_identity {
            Some(identity) => {
                framed.push(1);
                crate::append_length_prefixed(&mut framed, &identity.canonical_bytes());
            }
            None => framed.push(0),
        }
        match &self.target_physical_identity {
            Some(identity) => {
                framed.push(1);
                crate::append_length_prefixed(&mut framed, &identity.canonical_bytes());
            }
            None => framed.push(0),
        }
        match &self.target {
            PhysicalPathRedirectTarget::Repository(target) => {
                framed.push(1);
                crate::append_length_prefixed(&mut framed, target.canonical_bytes());
            }
            PhysicalPathRedirectTarget::OutsideRepository => framed.push(2),
            PhysicalPathRedirectTarget::Unavailable => framed.push(3),
        }
        crate::digest_hex(&framed)
    }
}

impl RepoPath {
    pub fn empty() -> Self {
        let mut canonical = Vec::new();
        canonical.extend_from_slice(REPO_PATH_MAGIC);
        canonical.extend_from_slice(&REPO_PATH_VERSION.to_be_bytes());
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
                Component::Normal(value) => components.push(native_io::native_component(value)?),
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
        if reader.take(REPO_PATH_MAGIC.len())? != REPO_PATH_MAGIC {
            return Err(RepoPathError::InvalidCanonicalEncoding);
        }
        if reader.read_u16()? != REPO_PATH_VERSION {
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
            RepoPathComponent::UnixBytes(_) => None,
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

    pub fn to_native_relative(&self) -> Result<PathBuf, RepoPathError> {
        let mut path = PathBuf::new();
        for component in &self.components {
            path.push(native_io::native_os_string(component)?);
        }
        Ok(path)
    }

    pub fn decode_native_nul_stream(bytes: &[u8]) -> Result<Vec<Self>, RepoPathError> {
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        let Some(records) = bytes.strip_suffix(&[0]) else {
            return Err(RepoPathError::InvalidNativeNulStream);
        };
        records
            .split(|byte| *byte == 0)
            .map(native_io::from_native_io_bytes)
            .collect()
    }

    pub fn encode_native_nul_stream(paths: &[Self]) -> Result<Vec<u8>, RepoPathError> {
        let mut output = Vec::new();
        for path in paths {
            output.extend_from_slice(&path.native_io_bytes()?);
            output.push(0);
        }
        Ok(output)
    }

    pub fn native_match_bytes(&self) -> Result<Vec<u8>, RepoPathError> {
        self.match_bytes().map(|bytes| bytes.0)
    }

    pub fn match_bytes(&self) -> Result<RepoPathMatchBytes, RepoPathError> {
        self.native_io_bytes().map(RepoPathMatchBytes)
    }

    fn native_io_bytes(&self) -> Result<Vec<u8>, RepoPathError> {
        let mut output = Vec::new();
        for (index, component) in self.components.iter().enumerate() {
            if index > 0 {
                output.push(b'/');
            }
            native_io::append_native_io_component(&mut output, component)?;
        }
        Ok(output)
    }

    fn from_components(components: Vec<RepoPathComponent>) -> Result<Self, RepoPathError> {
        let count = u32::try_from(components.len()).map_err(|_| RepoPathError::EncodingOverflow)?;
        let mut canonical = Vec::new();
        canonical.extend_from_slice(REPO_PATH_MAGIC);
        canonical.extend_from_slice(&REPO_PATH_VERSION.to_be_bytes());
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

pub(crate) fn decode_component(
    tag: u8,
    payload: &[u8],
) -> Result<RepoPathComponent, RepoPathError> {
    match tag {
        PORTABLE_UTF8_TAG => {
            let value = std::str::from_utf8(payload)
                .map_err(|_| RepoPathError::InvalidCanonicalEncoding)?;
            validate_scalar_component(value)?;
            Ok(RepoPathComponent::PortableUtf8(value.to_owned()))
        }
        UNIX_BYTES_TAG => {
            validate_native_bytes(payload, b'/')?;
            if payload == b"." || payload == b".." {
                return Err(RepoPathError::InvalidCanonicalEncoding);
            }
            if let Ok(value) = std::str::from_utf8(payload)
                && !value.contains('\\')
                && validate_scalar_component(value).is_ok()
            {
                return Err(RepoPathError::InvalidCanonicalEncoding);
            }
            Ok(RepoPathComponent::UnixBytes(payload.to_vec()))
        }
        WINDOWS_WTF16_TAG => {
            if !payload.len().is_multiple_of(2) {
                return Err(RepoPathError::InvalidCanonicalEncoding);
            }
            let units = payload
                .chunks_exact(2)
                .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                .collect::<Vec<_>>();
            if units.is_empty()
                || units.contains(&0)
                || units.contains(&(b'/' as u16))
                || units.contains(&(b'\\' as u16))
                || String::from_utf16(&units).is_ok()
            {
                return Err(RepoPathError::InvalidCanonicalEncoding);
            }
            Ok(RepoPathComponent::WindowsWtf16(units))
        }
        _ => Err(RepoPathError::InvalidCanonicalEncoding),
    }
}

fn portable_components(components: &[RepoPathComponent]) -> Option<String> {
    components
        .iter()
        .map(|component| match component {
            RepoPathComponent::PortableUtf8(value) => Some(value.as_str()),
            RepoPathComponent::UnixBytes(_) => None,
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

fn component_payload(component: &RepoPathComponent) -> (u8, Vec<u8>) {
    match component {
        RepoPathComponent::PortableUtf8(value) => (PORTABLE_UTF8_TAG, value.as_bytes().to_vec()),
        RepoPathComponent::UnixBytes(value) => (UNIX_BYTES_TAG, value.clone()),
        RepoPathComponent::WindowsWtf16(value) => {
            let mut bytes = Vec::with_capacity(value.len() * 2);
            for unit in value {
                bytes.extend_from_slice(&unit.to_be_bytes());
            }
            (WINDOWS_WTF16_TAG, bytes)
        }
    }
}

pub(crate) fn display_component(component: &RepoPathComponent) -> String {
    match component {
        RepoPathComponent::PortableUtf8(value) => value.clone(),
        RepoPathComponent::UnixBytes(value) => {
            let mut output = String::from("$'");
            for byte in value {
                use std::fmt::Write;
                let _ = write!(output, "\\x{byte:02x}");
            }
            output.push('\'');
            output
        }
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

pub(crate) fn portable_component(component: &RepoPathComponent) -> Option<&str> {
    match component {
        RepoPathComponent::PortableUtf8(value) => Some(value),
        RepoPathComponent::UnixBytes(_) | RepoPathComponent::WindowsWtf16(_) => None,
    }
}

fn validate_native_bytes(payload: &[u8], separator: u8) -> Result<(), RepoPathError> {
    if payload.is_empty() || payload.contains(&0) || payload.contains(&separator) {
        Err(RepoPathError::InvalidCanonicalEncoding)
    } else {
        Ok(())
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

    #[test]
    fn native_nul_stream_preserves_record_order_and_requires_terminator()
    -> Result<(), RepoPathError> {
        let paths = [
            RepoPath::from_portable("src/z.ts")?,
            RepoPath::from_portable("src/a.ts")?,
        ];
        let encoded = RepoPath::encode_native_nul_stream(&paths)?;
        assert_eq!(encoded, b"src/z.ts\0src/a.ts\0");
        assert_eq!(RepoPath::decode_native_nul_stream(&encoded)?, paths);
        assert_eq!(
            RepoPath::decode_native_nul_stream(b"src/a.ts"),
            Err(RepoPathError::InvalidNativeNulStream)
        );
        assert_eq!(
            RepoPath::decode_native_nul_stream(b"./src/a.ts\0"),
            Err(RepoPathError::InvalidNativeNulStream)
        );
        Ok(())
    }

    #[test]
    fn native_component_codec_round_trips_portable_names() -> Result<(), RepoPathError> {
        let canonical = encode_native_path_component(OsStr::new("payload.bin"))?;
        assert_eq!(canonical, b"\x01payload.bin");
        assert_eq!(
            decode_native_path_component(&canonical)?,
            OsString::from("payload.bin")
        );
        assert_eq!(
            portable_path_component(&canonical)?.as_deref(),
            Some("payload.bin")
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unix_non_utf8_native_stream_round_trips_exact_bytes() -> Result<(), RepoPathError> {
        let encoded = b"f\x80o\0";
        let decoded = RepoPath::decode_native_nul_stream(encoded)?;
        assert_eq!(RepoPath::encode_native_nul_stream(&decoded)?, encoded);
        assert_eq!(decoded[0].native_match_bytes()?, b"f\x80o");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unix_native_component_codec_preserves_non_utf8_bytes() -> Result<(), RepoPathError> {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let name = OsStr::from_bytes(b"f\x80o");
        let canonical = encode_native_path_component(name)?;
        assert_eq!(canonical, b"\x02f\x80o");
        assert_eq!(
            decode_native_path_component(&canonical)?.into_vec(),
            b"f\x80o"
        );
        assert_eq!(portable_path_component(&canonical)?, None);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_wtf8_round_trips_unpaired_surrogate_and_rejects_cesu_pair()
    -> Result<(), RepoPathError> {
        let unpaired = b"\xed\xa0\x80a\0";
        let decoded = RepoPath::decode_native_nul_stream(unpaired)?;
        assert_eq!(RepoPath::encode_native_nul_stream(&decoded)?, unpaired);
        assert_eq!(decoded[0].match_bytes()?.as_bytes(), b"\xed\xa0\x80a");

        let cesu_pair = b"\xed\xa0\xbd\xed\xb8\x80\0";
        assert_eq!(
            RepoPath::decode_native_nul_stream(cesu_pair),
            Err(RepoPathError::InvalidNativeNulStream)
        );
        assert_eq!(
            RepoPath::decode_native_nul_stream(b"src\\a.ts\0"),
            Err(RepoPathError::InvalidNativeNulStream)
        );
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_native_component_codec_preserves_unpaired_surrogates() -> Result<(), RepoPathError> {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let name = OsString::from_wide(&[0xd800, b'a' as u16]);
        let canonical = encode_native_path_component(&name)?;
        assert_eq!(
            canonical,
            [vec![WINDOWS_WTF16_TAG], vec![0xd8, 0x00, 0x00, b'a']].concat()
        );
        assert_eq!(
            decode_native_path_component(&canonical)?
                .encode_wide()
                .collect::<Vec<_>>(),
            [0xd800, b'a' as u16]
        );
        assert_eq!(portable_path_component(&canonical)?, None);
        Ok(())
    }
}
