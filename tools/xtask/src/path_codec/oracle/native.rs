use super::{Component, Platform, Repo, validate_portable, validate_unix};

pub(in crate::path_codec) fn decode_native_nul_stream(
    bytes: &[u8],
    platform: Platform,
) -> Result<Vec<Repo>, String> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let records = bytes
        .strip_suffix(&[0])
        .ok_or_else(|| "native NUL stream is unterminated".to_owned())?;
    records
        .split(|byte| *byte == 0)
        .map(|record| decode_native_record(record, platform))
        .collect()
}

fn decode_native_record(bytes: &[u8], platform: Platform) -> Result<Repo, String> {
    if bytes.is_empty() {
        return Ok(Repo {
            components: Vec::new(),
        });
    }
    let components = bytes
        .split(|byte| *byte == b'/')
        .map(|component| decode_native_component(component, platform))
        .collect::<Result<Vec<_>, _>>()?;
    let repo = Repo { components };
    if repo.native_bytes(platform)? != bytes {
        return Err("native path record is not canonical".to_owned());
    }
    Ok(repo)
}

fn decode_native_component(bytes: &[u8], platform: Platform) -> Result<Component, String> {
    match platform {
        Platform::Unix => {
            if let Ok(text) = std::str::from_utf8(bytes)
                && !text.contains('\\')
                && validate_portable(text).is_ok()
            {
                return Ok(Component::Portable(text.to_owned()));
            }
            validate_unix(bytes)?;
            Ok(Component::Unix(bytes.to_vec()))
        }
        Platform::Windows => {
            let units = decode_wtf8(bytes)?;
            if let Ok(text) = String::from_utf16(&units) {
                validate_portable(&text)?;
                Ok(Component::Portable(text))
            } else if units.is_empty()
                || units.contains(&0)
                || units.contains(&(b'/' as u16))
                || units.contains(&(b'\\' as u16))
            {
                Err("Windows native component is invalid".to_owned())
            } else {
                Ok(Component::Windows(units))
            }
        }
    }
}

fn decode_wtf8(bytes: &[u8]) -> Result<Vec<u16>, String> {
    let mut units = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let (code_point, width) = decode_wtf8_code_point(&bytes[index..])?;
        if (0xd800..=0xdbff).contains(&code_point) {
            let next = bytes.get(index + width..).unwrap_or_default();
            if !next.is_empty() {
                let (next_code_point, _) = decode_wtf8_code_point(next)?;
                if (0xdc00..=0xdfff).contains(&next_code_point) {
                    return Err("CESU-8 surrogate pair is not canonical WTF-8".to_owned());
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

fn decode_wtf8_code_point(bytes: &[u8]) -> Result<(u32, usize), String> {
    let first = bytes
        .first()
        .copied()
        .ok_or_else(|| "WTF-8 code point is missing".to_owned())?;
    let (mut value, width, minimum) = match first {
        0x00..=0x7f => return Ok((first as u32, 1)),
        0xc2..=0xdf => ((first & 0x1f) as u32, 2, 0x80),
        0xe0..=0xef => ((first & 0x0f) as u32, 3, 0x800),
        0xf0..=0xf4 => ((first & 0x07) as u32, 4, 0x1_0000),
        _ => return Err("WTF-8 leading byte is invalid".to_owned()),
    };
    if bytes.len() < width {
        return Err("WTF-8 code point is truncated".to_owned());
    }
    for continuation in &bytes[1..width] {
        if continuation & 0xc0 != 0x80 {
            return Err("WTF-8 continuation byte is invalid".to_owned());
        }
        value = (value << 6) | (continuation & 0x3f) as u32;
    }
    if value < minimum || value > 0x10ffff {
        return Err("WTF-8 code point is noncanonical".to_owned());
    }
    Ok((value, width))
}
