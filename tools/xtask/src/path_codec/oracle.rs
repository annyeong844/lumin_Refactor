use std::fmt::Write;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

mod checks;
mod native;

pub(super) use checks::check;
pub(super) use native::decode_native_nul_stream;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RepoProjection {
    pub(super) canonical_base64: String,
    pub(super) display: String,
    pub(super) utf8: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RootProjection {
    pub(super) canonical_base64: String,
    pub(super) display: String,
    pub(super) readable_address: Option<String>,
}

pub(super) fn repo_projection(bytes: &[u8]) -> Result<RepoProjection, String> {
    let path = decode_repo(bytes)?;
    Ok(RepoProjection {
        canonical_base64: STANDARD.encode(path.encode()),
        display: path.display(),
        utf8: path.portable(),
    })
}

pub(super) fn root_projection(bytes: &[u8]) -> Result<RootProjection, String> {
    let root = decode_root(bytes)?;
    Ok(RootProjection {
        canonical_base64: STANDARD.encode(root.encode()),
        display: root.display(),
        readable_address: root.readable(),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Platform {
    Unix,
    Windows,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Component {
    Portable(String),
    Unix(Vec<u8>),
    Windows(Vec<u16>),
}

impl Component {
    fn decode(reader: &mut Reader<'_>, platform: Option<Platform>) -> Result<Self, String> {
        let tag = reader.u8()?;
        let length = reader.u32()? as usize;
        let payload = reader.take(length)?;
        match tag {
            1 => {
                let value = std::str::from_utf8(payload)
                    .map_err(|_| "PortableUtf8 payload is malformed".to_owned())?;
                validate_portable(value)?;
                Ok(Self::Portable(value.to_owned()))
            }
            2 if platform != Some(Platform::Windows) => {
                validate_unix(payload)?;
                Ok(Self::Unix(payload.to_vec()))
            }
            3 if platform != Some(Platform::Unix) => {
                if payload.is_empty() || !payload.len().is_multiple_of(2) {
                    return Err("WindowsWtf16 payload length is invalid".to_owned());
                }
                let units = payload
                    .chunks_exact(2)
                    .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                    .collect::<Vec<_>>();
                if units.contains(&0)
                    || units.contains(&(b'/' as u16))
                    || units.contains(&(b'\\' as u16))
                    || String::from_utf16(&units).is_ok()
                {
                    return Err("WindowsWtf16 payload is noncanonical".to_owned());
                }
                Ok(Self::Windows(units))
            }
            _ => Err("component tag is unknown or belongs to another platform".to_owned()),
        }
    }

    fn encode(&self, output: &mut Vec<u8>) {
        let (tag, payload) = match self {
            Self::Portable(value) => (1, value.as_bytes().to_vec()),
            Self::Unix(value) => (2, value.clone()),
            Self::Windows(units) => (
                3,
                units
                    .iter()
                    .flat_map(|unit| unit.to_be_bytes())
                    .collect::<Vec<_>>(),
            ),
        };
        output.push(tag);
        output.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        output.extend_from_slice(&payload);
    }

    fn display(&self) -> String {
        match self {
            Self::Portable(value) => value.clone(),
            Self::Unix(bytes) => {
                let mut output = String::from("$'");
                for byte in bytes {
                    let _ = write!(output, "\\x{byte:02x}");
                }
                output.push('\'');
                output
            }
            Self::Windows(units) => {
                let body = units
                    .iter()
                    .map(|unit| format!("{unit:04x}"))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("wtf16[{body}]")
            }
        }
    }

    fn portable(&self) -> Option<&str> {
        match self {
            Self::Portable(value) => Some(value),
            Self::Unix(_) | Self::Windows(_) => None,
        }
    }

    fn native_bytes(&self, platform: Platform, output: &mut Vec<u8>) -> Result<(), String> {
        match self {
            Self::Portable(value) => output.extend_from_slice(value.as_bytes()),
            Self::Unix(bytes) if platform == Platform::Unix => output.extend_from_slice(bytes),
            Self::Windows(units) if platform == Platform::Windows => encode_wtf8(units, output),
            Self::Unix(_) | Self::Windows(_) => {
                return Err("component belongs to another platform".to_owned());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Repo {
    components: Vec<Component>,
}

impl Repo {
    fn encode(&self) -> Vec<u8> {
        let mut output = b"LUMRPATH".to_vec();
        output.extend_from_slice(&1_u16.to_be_bytes());
        output.extend_from_slice(&(self.components.len() as u32).to_be_bytes());
        for component in &self.components {
            component.encode(&mut output);
        }
        output
    }

    fn display(&self) -> String {
        self.components
            .iter()
            .map(Component::display)
            .collect::<Vec<_>>()
            .join("/")
    }

    fn portable(&self) -> Option<String> {
        self.components
            .iter()
            .map(Component::portable)
            .collect::<Option<Vec<_>>>()
            .map(|parts| parts.join("/"))
    }

    fn native_bytes(&self, platform: Platform) -> Result<Vec<u8>, String> {
        let mut output = Vec::new();
        for (index, component) in self.components.iter().enumerate() {
            if index > 0 {
                output.push(b'/');
            }
            component.native_bytes(platform, &mut output)?;
        }
        Ok(output)
    }
}

fn decode_repo(bytes: &[u8]) -> Result<Repo, String> {
    let mut reader = Reader::new(bytes);
    if reader.take(8)? != b"LUMRPATH" || reader.u16()? != 1 {
        return Err("repo path tag or version disagrees".to_owned());
    }
    let count = reader.u32()? as usize;
    let components = (0..count)
        .map(|_| Component::decode(&mut reader, None))
        .collect::<Result<Vec<_>, _>>()?;
    reader.finish()?;
    Ok(Repo { components })
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Address {
    Unix(Vec<Component>),
    Drive(u8, Vec<Component>),
    Unc(Component, Component, Vec<Component>),
    Volume([u8; 16], Vec<Component>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Root {
    platform: Platform,
    address: Address,
    physical: Vec<u8>,
}

impl Root {
    fn encode(&self) -> Vec<u8> {
        let mut output = b"LUMRROOT".to_vec();
        output.extend_from_slice(&1_u16.to_be_bytes());
        output.push(match self.platform {
            Platform::Unix => 1,
            Platform::Windows => 2,
        });
        match &self.address {
            Address::Unix(components) => {
                output.push(1);
                encode_components(components, &mut output);
            }
            Address::Drive(drive, components) => {
                output.push(2);
                output.push(*drive);
                encode_components(components, &mut output);
            }
            Address::Unc(server, share, components) => {
                output.push(3);
                server.encode(&mut output);
                share.encode(&mut output);
                encode_components(components, &mut output);
            }
            Address::Volume(guid, components) => {
                output.push(4);
                output.extend_from_slice(guid);
                encode_components(components, &mut output);
            }
        }
        output.extend_from_slice(&self.physical);
        output
    }

    fn display(&self) -> String {
        self.render(false)
            .unwrap_or_else(|| "<nonportable-root>".to_owned())
    }

    fn readable(&self) -> Option<String> {
        self.render(true)
    }

    fn render(&self, portable_only: bool) -> Option<String> {
        let component = |value: &Component| {
            if portable_only {
                value.portable().map(str::to_owned)
            } else {
                Some(value.display())
            }
        };
        let render_tail = |prefix: String, values: &[Component]| {
            values
                .iter()
                .map(&component)
                .collect::<Option<Vec<_>>>()
                .map(|parts| format!("{prefix}{}", parts.join("/")))
        };
        match &self.address {
            Address::Unix(values) => render_tail("/".to_owned(), values),
            Address::Drive(drive, values) => render_tail(format!("{}:/", *drive as char), values),
            Address::Unc(server, share, values) => {
                let base = format!("//{}/{}", component(server)?, component(share)?);
                if values.is_empty() {
                    Some(base)
                } else {
                    render_tail(format!("{base}/"), values)
                }
            }
            Address::Volume(guid, values) => {
                render_tail(format!("//?/Volume{{{}}}/", format_guid(guid)), values)
            }
        }
    }
}

fn decode_root(bytes: &[u8]) -> Result<Root, String> {
    let mut reader = Reader::new(bytes);
    if reader.take(8)? != b"LUMRROOT" || reader.u16()? != 1 {
        return Err("repository root tag or version disagrees".to_owned());
    }
    let platform = match reader.u8()? {
        1 => Platform::Unix,
        2 => Platform::Windows,
        _ => return Err("repository root platform is unknown".to_owned()),
    };
    let kind = reader.u8()?;
    let address = match (platform, kind) {
        (Platform::Unix, 1) => Address::Unix(decode_components(&mut reader, platform)?),
        (Platform::Windows, 2) => {
            let drive = reader.u8()?;
            if !drive.is_ascii_uppercase() {
                return Err("Windows drive is not canonical uppercase".to_owned());
            }
            Address::Drive(drive, decode_components(&mut reader, platform)?)
        }
        (Platform::Windows, 3) => Address::Unc(
            Component::decode(&mut reader, Some(platform))?,
            Component::decode(&mut reader, Some(platform))?,
            decode_components(&mut reader, platform)?,
        ),
        (Platform::Windows, 4) => Address::Volume(
            reader.take(16)?.try_into().map_err(|_| "GUID length")?,
            decode_components(&mut reader, platform)?,
        ),
        _ => return Err("repository root platform/address mismatch".to_owned()),
    };
    let physical_tag = reader.u8()?;
    let physical_payload = match (platform, physical_tag) {
        (Platform::Unix, 1) => reader.take(16)?,
        (Platform::Windows, 2) => reader.take(24)?,
        _ => return Err("repository root physical identity mismatch".to_owned()),
    };
    let mut physical = vec![physical_tag];
    physical.extend_from_slice(physical_payload);
    reader.finish()?;
    Ok(Root {
        platform,
        address,
        physical,
    })
}

fn decode_components(
    reader: &mut Reader<'_>,
    platform: Platform,
) -> Result<Vec<Component>, String> {
    let count = reader.u32()? as usize;
    (0..count)
        .map(|_| Component::decode(reader, Some(platform)))
        .collect()
}

fn encode_components(components: &[Component], output: &mut Vec<u8>) {
    output.extend_from_slice(&(components.len() as u32).to_be_bytes());
    for component in components {
        component.encode(output);
    }
}

fn validate_portable(value: &str) -> Result<(), String> {
    if value.is_empty() || value == "." || value == ".." || value.contains(['\0', '/', '\\']) {
        Err("portable component is noncanonical".to_owned())
    } else {
        Ok(())
    }
}

fn validate_unix(value: &[u8]) -> Result<(), String> {
    if value.is_empty()
        || value == b"."
        || value == b".."
        || value.contains(&0)
        || value.contains(&b'/')
    {
        return Err("UnixBytes component is invalid".to_owned());
    }
    if let Ok(text) = std::str::from_utf8(value)
        && !text.contains('\\')
    {
        return Err("UnixBytes component has a portable encoding".to_owned());
    }
    Ok(())
}

fn encode_wtf8(units: &[u16], output: &mut Vec<u8>) {
    let mut index = 0;
    while index < units.len() {
        let unit = units[index];
        let code_point = if (0xd800..=0xdbff).contains(&unit)
            && units
                .get(index + 1)
                .is_some_and(|low| (0xdc00..=0xdfff).contains(low))
        {
            index += 1;
            0x1_0000 + (((unit as u32 - 0xd800) << 10) | (units[index] as u32 - 0xdc00))
        } else {
            unit as u32
        };
        match code_point {
            0..=0x7f => output.push(code_point as u8),
            0x80..=0x7ff => {
                output.push((0xc0 | (code_point >> 6)) as u8);
                output.push((0x80 | (code_point & 0x3f)) as u8);
            }
            0x800..=0xffff => {
                output.push((0xe0 | (code_point >> 12)) as u8);
                output.push((0x80 | ((code_point >> 6) & 0x3f)) as u8);
                output.push((0x80 | (code_point & 0x3f)) as u8);
            }
            _ => {
                output.push((0xf0 | (code_point >> 18)) as u8);
                output.push((0x80 | ((code_point >> 12) & 0x3f)) as u8);
                output.push((0x80 | ((code_point >> 6) & 0x3f)) as u8);
                output.push((0x80 | (code_point & 0x3f)) as u8);
            }
        }
        index += 1;
    }
}

fn format_guid(guid: &[u8; 16]) -> String {
    let mut output = String::new();
    for (index, byte) in guid.iter().enumerate() {
        if [4, 6, 8, 10].contains(&index) {
            output.push('-');
        }
        let _ = write!(output, "{byte:02x}");
    }
    output
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| "codec length overflow".to_owned())?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| "codec payload is truncated".to_owned())?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn finish(&self) -> Result<(), String> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err("codec payload has trailing bytes".to_owned())
        }
    }
}
