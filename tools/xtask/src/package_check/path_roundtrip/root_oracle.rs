use std::fs;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::{component_record, portable_component};

pub(super) struct RootExpectation {
    pub(super) canonical_base64: String,
    pub(super) display: String,
}

#[cfg(windows)]
#[repr(C)]
struct WindowsFileId128 {
    identifier: [u8; 16],
}

#[cfg(windows)]
#[repr(C)]
struct WindowsFileIdInfo {
    volume_serial_number: u64,
    file_id: WindowsFileId128,
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "the independent Windows root oracle requires one kernel32 query"
)]
#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "GetFileInformationByHandleEx"]
    fn get_file_information_by_handle_ex(
        file: *mut std::ffi::c_void,
        information_class: i32,
        information: *mut std::ffi::c_void,
        buffer_size: u32,
    ) -> i32;
}

#[cfg(unix)]
pub(super) fn native_root_expectation(root: &Path) -> Result<RootExpectation, String> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::Component;

    let canonical = fs::canonicalize(root)
        .map_err(|error| format!("cannot canonicalize packaged native root: {error}"))?;
    let mut records = Vec::new();
    let mut displays = Vec::new();
    for component in canonical.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(value) => {
                let (record, display) = unix_component(value.as_bytes());
                records.push(record);
                displays.push(display);
            }
            _ => return Err("packaged Unix root has a noncanonical component".to_owned()),
        }
    }
    let metadata = fs::metadata(&canonical)
        .map_err(|error| format!("cannot inspect packaged native root identity: {error}"))?;
    let mut bytes = root_address_bytes(1, 1, &[], &records)?;
    bytes.push(1);
    bytes.extend_from_slice(&metadata.dev().to_be_bytes());
    bytes.extend_from_slice(&metadata.ino().to_be_bytes());
    Ok(RootExpectation {
        canonical_base64: STANDARD.encode(bytes),
        display: format!("/{}", displays.join("/")),
    })
}

#[cfg(unix)]
fn unix_component(bytes: &[u8]) -> (Vec<u8>, String) {
    if let Ok(portable) = std::str::from_utf8(bytes)
        && !portable.contains('\\')
    {
        (portable_component(bytes), portable.to_owned())
    } else {
        (
            component_record(2, bytes),
            format!(
                "$'{}'",
                bytes
                    .iter()
                    .map(|byte| format!("\\x{byte:02x}"))
                    .collect::<String>()
            ),
        )
    }
}

#[cfg(windows)]
struct WindowsPrefixExpectation {
    address_kind: u8,
    bytes: Vec<u8>,
    display: String,
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "the independent Windows root oracle requires FILE_ID_128"
)]
pub(super) fn native_root_expectation(root: &Path) -> Result<RootExpectation, String> {
    use std::mem::size_of;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Component, Prefix};

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_ID_INFO_CLASS: i32 = 18;

    let canonical = fs::canonicalize(root)
        .map_err(|error| format!("cannot canonicalize packaged native root: {error}"))?;
    let mut parts = canonical.components();
    let prefix = match parts.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
                let drive = drive.to_ascii_uppercase();
                if !drive.is_ascii_uppercase() {
                    return Err("packaged Windows root has an invalid drive prefix".to_owned());
                }
                WindowsPrefixExpectation {
                    address_kind: 2,
                    bytes: vec![drive],
                    display: format!("{}:/", char::from(drive)),
                }
            }
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                let (server_record, server_display) = windows_component(server, "UNC server")?;
                let (share_record, share_display) = windows_component(share, "UNC share")?;
                let mut bytes = server_record;
                bytes.extend_from_slice(&share_record);
                WindowsPrefixExpectation {
                    address_kind: 3,
                    bytes,
                    display: format!("//{server_display}/{share_display}"),
                }
            }
            Prefix::Verbatim(value) => {
                let guid = parse_volume_guid(value).ok_or_else(|| {
                    "packaged Windows root has an unsupported verbatim prefix".to_owned()
                })?;
                WindowsPrefixExpectation {
                    address_kind: 4,
                    bytes: guid.to_vec(),
                    display: format!("//?/Volume{{{}}}/", format_volume_guid(&guid)),
                }
            }
            Prefix::DeviceNS(_) => {
                return Err("packaged Windows root has an unsupported device prefix".to_owned());
            }
        },
        _ => return Err("packaged Windows root omitted its absolute prefix".to_owned()),
    };
    if !matches!(parts.next(), Some(Component::RootDir)) {
        return Err("packaged Windows root omitted its root directory".to_owned());
    }
    let mut records = Vec::new();
    let mut displays = Vec::new();
    for component in parts {
        match component {
            Component::Normal(value) => {
                let (record, display) = windows_component(value, "root component")?;
                records.push(record);
                displays.push(display);
            }
            _ => return Err("packaged Windows root has a noncanonical component".to_owned()),
        }
    }
    let root_handle = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(&canonical)
        .map_err(|error| format!("cannot open packaged native root identity: {error}"))?;
    let mut information = WindowsFileIdInfo {
        volume_serial_number: 0,
        file_id: WindowsFileId128 {
            identifier: [0; 16],
        },
    };
    let buffer_size = u32::try_from(size_of::<WindowsFileIdInfo>())
        .map_err(|_| "Windows root identity buffer exceeds u32".to_owned())?;
    // SAFETY: `root_handle` remains open, and `information` is an aligned writable buffer.
    let succeeded = unsafe {
        get_file_information_by_handle_ex(
            root_handle.as_raw_handle(),
            FILE_ID_INFO_CLASS,
            std::ptr::from_mut(&mut information).cast(),
            buffer_size,
        )
    };
    if succeeded == 0 {
        return Err(format!(
            "cannot read packaged native root identity: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut bytes = root_address_bytes(2, prefix.address_kind, &prefix.bytes, &records)?;
    bytes.push(2);
    bytes.extend_from_slice(&information.volume_serial_number.to_be_bytes());
    bytes.extend_from_slice(&information.file_id.identifier);
    let components = displays.join("/");
    let display = if components.is_empty() || prefix.display.ends_with('/') {
        format!("{}{components}", prefix.display)
    } else {
        format!("{}/{components}", prefix.display)
    };
    Ok(RootExpectation {
        canonical_base64: STANDARD.encode(bytes),
        display,
    })
}

#[cfg(windows)]
fn windows_component(value: &std::ffi::OsStr, label: &str) -> Result<(Vec<u8>, String), String> {
    use std::os::windows::ffi::OsStrExt;

    let units = value.encode_wide().collect::<Vec<_>>();
    if units.is_empty()
        || units.contains(&0)
        || units.contains(&(b'/' as u16))
        || units.contains(&(b'\\' as u16))
    {
        return Err(format!("packaged Windows {label} is invalid"));
    }
    if let Ok(portable) = String::from_utf16(&units) {
        if matches!(portable.as_str(), "." | "..") {
            return Err(format!("packaged Windows {label} is noncanonical"));
        }
        return Ok((portable_component(portable.as_bytes()), portable));
    }
    let mut payload = Vec::with_capacity(units.len() * 2);
    for unit in &units {
        payload.extend_from_slice(&unit.to_be_bytes());
    }
    Ok((
        component_record(3, &payload),
        format!(
            "wtf16[{}]",
            units
                .iter()
                .map(|unit| format!("{unit:04x}"))
                .collect::<Vec<_>>()
                .join(",")
        ),
    ))
}

#[cfg(windows)]
fn parse_volume_guid(value: &std::ffi::OsStr) -> Option<[u8; 16]> {
    let value = value.to_str()?;
    let body = value.strip_prefix("Volume{")?.strip_suffix('}')?;
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

#[cfg(windows)]
fn format_volume_guid(guid: &[u8; 16]) -> String {
    use std::fmt::Write;

    let mut output = String::with_capacity(36);
    for (index, byte) in guid.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            output.push('-');
        }
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn root_address_bytes(
    platform: u8,
    address_kind: u8,
    prefix: &[u8],
    components: &[Vec<u8>],
) -> Result<Vec<u8>, String> {
    let count = u32::try_from(components.len())
        .map_err(|_| "packaged native root component count exceeds u32".to_owned())?;
    let mut bytes = b"LUMRROOT".to_vec();
    bytes.extend_from_slice(&1_u16.to_be_bytes());
    bytes.push(platform);
    bytes.push(address_kind);
    bytes.extend_from_slice(prefix);
    bytes.extend_from_slice(&count.to_be_bytes());
    for component in components {
        bytes.extend_from_slice(component);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn unix_backslash_component_uses_native_encoding() {
        let bytes = b"root\\component";
        let (record, display) = super::unix_component(bytes);
        assert_eq!(record, super::component_record(2, bytes));
        assert_eq!(
            display,
            "$'\\x72\\x6f\\x6f\\x74\\x5c\\x63\\x6f\\x6d\\x70\\x6f\\x6e\\x65\\x6e\\x74'"
        );
    }

    #[cfg(windows)]
    #[test]
    fn volume_guid_prefix_uses_network_byte_order() -> Result<(), String> {
        let value = std::ffi::OsStr::new("Volume{00112233-4455-6677-8899-AABBCCDDEEFF}");
        let guid = super::parse_volume_guid(value)
            .ok_or_else(|| "independent volume GUID oracle rejected a valid prefix".to_owned())?;
        assert_eq!(
            guid,
            [
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ]
        );
        assert_eq!(
            super::format_volume_guid(&guid),
            "00112233-4455-6677-8899-aabbccddeeff"
        );
        Ok(())
    }
}
