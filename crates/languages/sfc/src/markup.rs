use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(crate) struct Attribute {
    pub(crate) value: Option<String>,
    pub(crate) quoted: bool,
}

#[derive(Debug)]
pub(crate) struct MarkupBlock<'a> {
    pub(crate) name: String,
    pub(crate) attributes: BTreeMap<String, Attribute>,
    pub(crate) content: &'a str,
    pub(crate) open_start: usize,
    pub(crate) content_start: usize,
    pub(crate) content_end: usize,
}

#[derive(Debug)]
pub(crate) struct TemplateTag {
    pub(crate) name: String,
    pub(crate) attributes: BTreeMap<String, Attribute>,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

pub(crate) fn top_level_blocks(source: &str) -> Result<Vec<MarkupBlock<'_>>, String> {
    let bytes = source.as_bytes();
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while let Some(open) = find_byte(bytes, cursor, b'<') {
        if starts_with(bytes, open, b"<!--") {
            cursor = comment_end(bytes, open)?;
            continue;
        }
        let Some((name, name_end)) = tag_name(source, open + 1) else {
            cursor = open + 1;
            continue;
        };
        if name.starts_with('/') || !is_owned_block_name(name) {
            cursor = name_end;
            continue;
        }
        let open_end = tag_end(bytes, name_end)?;
        let attributes = parse_attributes(&source[name_end..open_end])?;
        if is_self_closing(bytes, name_end, open_end) {
            blocks.push(MarkupBlock {
                name: name.to_ascii_lowercase(),
                attributes,
                content: "",
                open_start: open,
                content_start: open_end + 1,
                content_end: open_end + 1,
            });
            cursor = open_end + 1;
            continue;
        }

        let content_start = open_end + 1;
        let (close_start, close_end) = closing_tag(source, name, content_start)?;
        blocks.push(MarkupBlock {
            name: name.to_ascii_lowercase(),
            attributes,
            content: &source[content_start..close_start],
            open_start: open,
            content_start,
            content_end: close_start,
        });
        cursor = close_end;
    }
    Ok(blocks)
}

pub(crate) fn template_tags(source: &str, base: usize) -> Result<Vec<TemplateTag>, String> {
    let bytes = source.as_bytes();
    let mut tags = Vec::new();
    let mut cursor = 0;
    while let Some(open) = find_byte(bytes, cursor, b'<') {
        if starts_with(bytes, open, b"<!--") {
            cursor = comment_end(bytes, open)?;
            continue;
        }
        let Some((name, name_end)) = tag_name(source, open + 1) else {
            cursor = open + 1;
            continue;
        };
        if name.starts_with('/') || name.starts_with('!') || name.starts_with('?') {
            cursor = name_end;
            continue;
        }
        let end = tag_end(bytes, name_end)?;
        tags.push(TemplateTag {
            name: name.to_owned(),
            attributes: parse_attributes(&source[name_end..end])?,
            start: base + open,
            end: base + end + 1,
        });
        cursor = end + 1;
    }
    Ok(tags)
}

fn parse_attributes(source: &str) -> Result<BTreeMap<String, Attribute>, String> {
    let bytes = source.as_bytes();
    let mut attributes = BTreeMap::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        cursor = skip_space(bytes, cursor);
        if cursor == bytes.len() || bytes[cursor] == b'/' {
            break;
        }
        let start = cursor;
        while cursor < bytes.len() && is_attribute_name_byte(bytes[cursor]) {
            cursor += 1;
        }
        if start == cursor {
            return Err("SFC start tag contains a malformed attribute".to_owned());
        }
        let name = source[start..cursor].to_ascii_lowercase();
        cursor = skip_space(bytes, cursor);
        let attribute = if bytes.get(cursor) == Some(&b'=') {
            cursor = skip_space(bytes, cursor + 1);
            let (value, end, quoted) = attribute_value(source, cursor)?;
            cursor = end;
            Attribute {
                value: Some(value),
                quoted,
            }
        } else {
            Attribute {
                value: None,
                quoted: false,
            }
        };
        if attributes.insert(name.clone(), attribute).is_some() {
            return Err(format!("SFC start tag repeats attribute `{name}`"));
        }
    }
    Ok(attributes)
}

fn attribute_value(source: &str, cursor: usize) -> Result<(String, usize, bool), String> {
    let bytes = source.as_bytes();
    let Some(first) = bytes.get(cursor).copied() else {
        return Err("SFC attribute is missing its value".to_owned());
    };
    if first == b'\'' || first == b'"' {
        let mut end = cursor + 1;
        while end < bytes.len() && bytes[end] != first {
            end += 1;
        }
        if end == bytes.len() {
            return Err("SFC attribute has an unterminated quoted value".to_owned());
        }
        return Ok((source[cursor + 1..end].to_owned(), end + 1, true));
    }
    let mut end = cursor;
    while end < bytes.len() && !bytes[end].is_ascii_whitespace() && bytes[end] != b'/' {
        end += 1;
    }
    if end == cursor {
        return Err("SFC attribute is missing its value".to_owned());
    }
    Ok((source[cursor..end].to_owned(), end, false))
}

fn closing_tag(source: &str, name: &str, cursor: usize) -> Result<(usize, usize), String> {
    let bytes = source.as_bytes();
    let mut search = cursor;
    while let Some(open) = find_byte(bytes, search, b'<') {
        if starts_with(bytes, open, b"<!--") {
            search = comment_end(bytes, open)?;
            continue;
        }
        if bytes.get(open + 1) == Some(&b'/') {
            let Some((candidate, name_end)) = tag_name(source, open + 1) else {
                search = open + 1;
                continue;
            };
            if candidate[1..].eq_ignore_ascii_case(name) {
                let close_end = tag_end(bytes, name_end)?;
                return Ok((open, close_end + 1));
            }
        }
        search = open + 1;
    }
    Err(format!("SFC block `<{name}>` is not closed"))
}

fn tag_name(source: &str, cursor: usize) -> Option<(&str, usize)> {
    let bytes = source.as_bytes();
    let mut end = cursor;
    if bytes.get(end) == Some(&b'/') {
        end += 1;
    }
    let name_start = end;
    while bytes.get(end).is_some_and(|byte| is_tag_name_byte(*byte)) {
        end += 1;
    }
    (end > name_start).then(|| (&source[cursor..end], end))
}

fn tag_end(bytes: &[u8], mut cursor: usize) -> Result<usize, String> {
    let mut quote = None;
    while let Some(byte) = bytes.get(cursor).copied() {
        match (quote, byte) {
            (Some(expected), value) if value == expected => quote = None,
            (None, b'\'' | b'"') => quote = Some(byte),
            (None, b'>') => return Ok(cursor),
            _ => {}
        }
        cursor += 1;
    }
    Err("SFC start tag is not closed".to_owned())
}

fn comment_end(bytes: &[u8], open: usize) -> Result<usize, String> {
    find_subslice(bytes, open + 4, b"-->")
        .map(|end| end + 3)
        .ok_or_else(|| "SFC markup comment is not closed".to_owned())
}

fn is_self_closing(bytes: &[u8], start: usize, end: usize) -> bool {
    let mut cursor = end;
    while cursor > start && bytes[cursor - 1].is_ascii_whitespace() {
        cursor -= 1;
    }
    cursor > start && bytes[cursor - 1] == b'/'
}

fn is_owned_block_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("template")
        || name.eq_ignore_ascii_case("script")
        || name.eq_ignore_ascii_case("style")
}

fn find_byte(bytes: &[u8], start: usize, target: u8) -> Option<usize> {
    bytes[start..]
        .iter()
        .position(|byte| *byte == target)
        .map(|offset| start + offset)
}

fn find_subslice(bytes: &[u8], start: usize, target: &[u8]) -> Option<usize> {
    bytes[start..]
        .windows(target.len())
        .position(|window| window == target)
        .map(|offset| start + offset)
}

fn starts_with(bytes: &[u8], start: usize, target: &[u8]) -> bool {
    bytes.get(start..start + target.len()) == Some(target)
}

fn skip_space(bytes: &[u8], mut cursor: usize) -> usize {
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    cursor
}

fn is_tag_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':' | b'.')
}

fn is_attribute_name_byte(byte: u8) -> bool {
    !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'>' | b'/' | b'\'' | b'"' | b'<' | b'`')
}
