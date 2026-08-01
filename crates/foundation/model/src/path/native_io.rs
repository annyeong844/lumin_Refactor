use std::ffi::{OsStr, OsString};
use std::path::Path;

use super::{RepoPath, RepoPathComponent, RepoPathError, validate_scalar_component};

#[cfg(unix)]
pub(super) fn native_component(value: &OsStr) -> Result<RepoPathComponent, RepoPathError> {
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
pub(super) fn native_component(value: &OsStr) -> Result<RepoPathComponent, RepoPathError> {
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

#[cfg(not(any(unix, windows)))]
pub(super) fn native_component(_: &OsStr) -> Result<RepoPathComponent, RepoPathError> {
    Err(RepoPathError::ForeignPlatformComponent)
}

pub(super) fn native_os_string(component: &RepoPathComponent) -> Result<OsString, RepoPathError> {
    match component {
        RepoPathComponent::PortableUtf8(value) => Ok(OsString::from(value)),
        RepoPathComponent::UnixBytes(value) => {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                Ok(OsString::from_vec(value.clone()))
            }
            #[cfg(not(unix))]
            {
                let _ = value;
                Err(RepoPathError::ForeignPlatformComponent)
            }
        }
        RepoPathComponent::WindowsWtf16(value) => {
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStringExt;
                Ok(OsString::from_wide(value))
            }
            #[cfg(not(windows))]
            {
                let _ = value;
                Err(RepoPathError::ForeignPlatformComponent)
            }
        }
    }
}

#[cfg(unix)]
pub(super) fn from_native_io_bytes(bytes: &[u8]) -> Result<RepoPath, RepoPathError> {
    use std::os::unix::ffi::OsStrExt;

    if bytes.contains(&0) {
        return Err(RepoPathError::InvalidNativeNulStream);
    }
    let path = RepoPath::from_native_relative(Path::new(OsStr::from_bytes(bytes)))?;
    require_canonical_native_record(bytes, path)
}

#[cfg(windows)]
pub(super) fn from_native_io_bytes(bytes: &[u8]) -> Result<RepoPath, RepoPathError> {
    use std::os::windows::ffi::OsStringExt;

    if bytes.contains(&0) {
        return Err(RepoPathError::InvalidNativeNulStream);
    }
    let units = decode_wtf8(bytes)?;
    let path = RepoPath::from_native_relative(Path::new(&OsString::from_wide(&units)))?;
    require_canonical_native_record(bytes, path)
}

#[cfg(not(any(unix, windows)))]
pub(super) fn from_native_io_bytes(_: &[u8]) -> Result<RepoPath, RepoPathError> {
    Err(RepoPathError::ForeignPlatformComponent)
}

pub(super) fn append_native_io_component(
    output: &mut Vec<u8>,
    component: &RepoPathComponent,
) -> Result<(), RepoPathError> {
    match component {
        RepoPathComponent::PortableUtf8(value) => output.extend_from_slice(value.as_bytes()),
        RepoPathComponent::UnixBytes(value) => {
            #[cfg(unix)]
            output.extend_from_slice(value);
            #[cfg(not(unix))]
            {
                let _ = value;
                return Err(RepoPathError::ForeignPlatformComponent);
            }
        }
        RepoPathComponent::WindowsWtf16(value) => {
            #[cfg(windows)]
            encode_wtf8(output, value);
            #[cfg(not(windows))]
            {
                let _ = value;
                return Err(RepoPathError::ForeignPlatformComponent);
            }
        }
    }
    Ok(())
}

fn require_canonical_native_record(
    original: &[u8],
    path: RepoPath,
) -> Result<RepoPath, RepoPathError> {
    if path.native_io_bytes()? == original {
        Ok(path)
    } else {
        Err(RepoPathError::InvalidNativeNulStream)
    }
}

#[cfg(windows)]
fn encode_wtf8(output: &mut Vec<u8>, units: &[u16]) {
    let mut index = 0;
    while index < units.len() {
        let unit = units[index];
        let code_point = if (0xd800..=0xdbff).contains(&unit)
            && units
                .get(index + 1)
                .is_some_and(|low| (0xdc00..=0xdfff).contains(low))
        {
            let low = units[index + 1];
            index += 1;
            0x1_0000 + (((unit as u32 - 0xd800) << 10) | (low as u32 - 0xdc00))
        } else {
            unit as u32
        };
        append_wtf8_code_point(output, code_point);
        index += 1;
    }
}

#[cfg(windows)]
fn append_wtf8_code_point(output: &mut Vec<u8>, code_point: u32) {
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
}

#[cfg(windows)]
fn decode_wtf8(bytes: &[u8]) -> Result<Vec<u16>, RepoPathError> {
    let mut units = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let (code_point, width) = decode_wtf8_code_point(&bytes[index..])?;
        if (0xd800..=0xdbff).contains(&code_point) {
            let next = bytes.get(index + width..).unwrap_or_default();
            if !next.is_empty() {
                let (next_code_point, _) = decode_wtf8_code_point(next)?;
                if (0xdc00..=0xdfff).contains(&next_code_point) {
                    return Err(RepoPathError::InvalidNativeNulStream);
                }
            }
        }
        if code_point <= 0xffff {
            units.push(code_point as u16);
        } else {
            let scalar = code_point - 0x1_0000;
            units.push(0xd800 | (scalar >> 10) as u16);
            units.push(0xdc00 | (scalar & 0x3ff) as u16);
        }
        index += width;
    }
    Ok(units)
}

#[cfg(windows)]
fn decode_wtf8_code_point(bytes: &[u8]) -> Result<(u32, usize), RepoPathError> {
    let Some(first) = bytes.first().copied() else {
        return Err(RepoPathError::InvalidNativeNulStream);
    };
    let (mut value, width, minimum) = match first {
        0x00..=0x7f => return Ok((first as u32, 1)),
        0xc2..=0xdf => ((first & 0x1f) as u32, 2, 0x80),
        0xe0..=0xef => ((first & 0x0f) as u32, 3, 0x800),
        0xf0..=0xf4 => ((first & 0x07) as u32, 4, 0x1_0000),
        _ => return Err(RepoPathError::InvalidNativeNulStream),
    };
    if bytes.len() < width {
        return Err(RepoPathError::InvalidNativeNulStream);
    }
    for continuation in &bytes[1..width] {
        if continuation & 0xc0 != 0x80 {
            return Err(RepoPathError::InvalidNativeNulStream);
        }
        value = (value << 6) | (continuation & 0x3f) as u32;
    }
    if value < minimum || value > 0x10ffff {
        return Err(RepoPathError::InvalidNativeNulStream);
    }
    Ok((value, width))
}
