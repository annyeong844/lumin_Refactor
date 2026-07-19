use std::collections::BTreeMap;

use lumin_model::{
    CapabilityState, EmbeddedSourceUnit, ExternalEmbeddedSourceRef, Limitation, LogicalSourceId,
    RepoPath, SfcDecomposition, SfcDialect, SfcResourceUse, SfcTemplateUse, SfcTemplateUseKind,
    SourceKind, SourceSnapshot, SourceSpan, digest_hex,
};

use crate::markup::{Attribute, MarkupBlock, template_tags, top_level_blocks};

pub(crate) fn decompose(
    snapshot: &SourceSnapshot,
    source_index: &BTreeMap<RepoPath, (LogicalSourceId, SourceKind)>,
) -> SfcDecomposition {
    let source = match std::str::from_utf8(&snapshot.bytes) {
        Ok(source) => source,
        Err(error) => {
            return decomposition_unknown(snapshot, format!("Vue source is not UTF-8: {error}"));
        }
    };
    let blocks = match top_level_blocks(source) {
        Ok(blocks) => blocks,
        Err(detail) => return decomposition_unknown(snapshot, detail),
    };
    if let Some(detail) = invalid_block_layout(&blocks) {
        return decomposition_unknown(snapshot, detail);
    }

    let mut inline_scripts = Vec::new();
    let mut external_scripts = Vec::new();
    let mut template_uses = Vec::new();
    let mut resource_uses = Vec::new();
    let mut limitations = Vec::new();

    for block in &blocks {
        match block.name.as_str() {
            "script" => decompose_script(
                snapshot,
                block,
                source_index,
                &mut inline_scripts,
                &mut external_scripts,
                &mut limitations,
            ),
            "template" => decompose_template(snapshot, block, &mut template_uses, &mut limitations),
            "style" => decompose_style(snapshot, block, &mut resource_uses, &mut limitations),
            _ => {}
        }
    }

    inline_scripts.sort_by_key(|unit| unit.parent_span.start);
    external_scripts.sort_by_key(|reference| reference.parent_span.start);
    template_uses.sort_by_key(|usage| usage.span.start);
    resource_uses.sort_by(|left, right| {
        left.span
            .start
            .cmp(&right.span.start)
            .then_with(|| left.specifier.cmp(&right.specifier))
    });
    resource_uses.dedup();
    limitations.sort_by_key(|limitation| format!("{limitation:?}"));
    limitations.dedup();

    SfcDecomposition {
        source_id: snapshot.id.clone(),
        dialect: SfcDialect::Vue,
        state: if limitations.is_empty() {
            CapabilityState::Complete
        } else {
            CapabilityState::Incomplete
        },
        module_export_known: true,
        inline_scripts,
        external_scripts,
        template_uses,
        resource_uses,
        limitations,
    }
}

fn invalid_block_layout(blocks: &[MarkupBlock<'_>]) -> Option<String> {
    let template_count = blocks
        .iter()
        .filter(|block| block.name == "template")
        .count();
    if template_count > 1 {
        return Some("Vue SFC contains more than one top-level template block".to_owned());
    }
    let regular_scripts = blocks
        .iter()
        .filter(|block| block.name == "script" && !block.attributes.contains_key("setup"))
        .count();
    let setup_scripts = blocks
        .iter()
        .filter(|block| block.name == "script" && block.attributes.contains_key("setup"))
        .count();
    if regular_scripts > 1 || setup_scripts > 1 {
        return Some("Vue SFC repeats a top-level script or script-setup block".to_owned());
    }
    None
}

fn decompose_script(
    snapshot: &SourceSnapshot,
    block: &MarkupBlock<'_>,
    source_index: &BTreeMap<RepoPath, (LogicalSourceId, SourceKind)>,
    inline_scripts: &mut Vec<EmbeddedSourceUnit>,
    external_scripts: &mut Vec<ExternalEmbeddedSourceRef>,
    limitations: &mut Vec<Limitation>,
) {
    let (setup, declared_kind) = match script_mode(snapshot, block) {
        Ok(mode) => mode,
        Err(limitation) => {
            limitations.push(limitation);
            return;
        }
    };
    if let Some(src) = block.attributes.get("src") {
        match external_script(snapshot, block, src, setup, declared_kind, source_index) {
            Ok(reference) => external_scripts.push(reference),
            Err(limitation) => limitations.push(limitation),
        }
        return;
    }
    match inline_script(snapshot, block, declared_kind) {
        Ok(unit) => inline_scripts.push(unit),
        Err(limitation) => limitations.push(limitation),
    }
}

fn script_mode(
    snapshot: &SourceSnapshot,
    block: &MarkupBlock<'_>,
) -> Result<(bool, SourceKind), Limitation> {
    if block.attributes.contains_key(":src") || block.attributes.contains_key("v-bind:src") {
        return Err(decomposition_limitation(
            snapshot,
            "Vue script uses a dynamic src attribute",
        ));
    }
    let setup = block.attributes.get("setup");
    if setup.is_some_and(|attribute| attribute.value.is_some()) {
        return Err(decomposition_limitation(
            snapshot,
            "Vue script setup must be a boolean attribute",
        ));
    }
    let kind = script_kind(block.attributes.get("lang"))
        .map_err(|detail| decomposition_limitation(snapshot, detail))?;
    Ok((setup.is_some(), kind))
}

fn external_script(
    snapshot: &SourceSnapshot,
    block: &MarkupBlock<'_>,
    src: &Attribute,
    setup: bool,
    declared_kind: SourceKind,
    source_index: &BTreeMap<RepoPath, (LogicalSourceId, SourceKind)>,
) -> Result<ExternalEmbeddedSourceRef, Limitation> {
    if setup {
        return Err(decomposition_limitation(
            snapshot,
            "Vue script setup cannot use an external src in this slice",
        ));
    }
    if !block.content.trim().is_empty() {
        return Err(decomposition_limitation(
            snapshot,
            "Vue external script block also contains inline source",
        ));
    }
    let specifier = literal_relative(src).ok_or_else(|| {
        decomposition_limitation(
            snapshot,
            "Vue external script src must be one quoted relative path",
        )
    })?;
    let target_path = snapshot
        .path
        .resolve_portable_relative(specifier)
        .ok_or_else(|| external_script_unresolved(snapshot, specifier))?;
    let (target_source_id, target_kind) = source_index
        .get(&target_path)
        .ok_or_else(|| external_script_unresolved(snapshot, specifier))?;
    if !target_kind.is_js_family()
        || target_kind.is_declaration()
        || (block.attributes.contains_key("lang")
            && !compatible_script_kinds(declared_kind, *target_kind))
    {
        return Err(external_script_mode_conflict(
            snapshot,
            target_source_id,
            declared_kind,
            *target_kind,
        ));
    }
    let parent_span = source_span(block.open_start, block.content_end).ok_or_else(|| {
        decomposition_limitation(snapshot, "Vue external script span exceeds the model limit")
    })?;
    Ok(ExternalEmbeddedSourceRef {
        parent_source_id: snapshot.id.clone(),
        target_source_id: target_source_id.clone(),
        target_kind: *target_kind,
        specifier: specifier.to_owned(),
        parent_span,
    })
}

fn inline_script(
    snapshot: &SourceSnapshot,
    block: &MarkupBlock<'_>,
    declared_kind: SourceKind,
) -> Result<EmbeddedSourceUnit, Limitation> {
    let parent_span = source_span(block.content_start, block.content_end).ok_or_else(|| {
        decomposition_limitation(snapshot, "Vue inline script span exceeds the model limit")
    })?;
    let bytes = block.content.as_bytes().to_vec();
    let payload_sha256 = digest_hex(&bytes);
    let id = lumin_model::EmbeddedSourceUnitId::for_parent_span(
        &snapshot.id,
        parent_span.start,
        parent_span.end,
        &payload_sha256,
    );
    Ok(EmbeddedSourceUnit {
        id,
        parent_source_id: snapshot.id.clone(),
        parent_span,
        kind: declared_kind,
        payload_sha256,
        bytes,
    })
}

fn external_script_unresolved(snapshot: &SourceSnapshot, specifier: &str) -> Limitation {
    Limitation::SfcExternalScriptUnresolved {
        source_id: snapshot.id.clone(),
        specifier: specifier.to_owned(),
    }
}

fn external_script_mode_conflict(
    snapshot: &SourceSnapshot,
    target_source_id: &LogicalSourceId,
    declared: SourceKind,
    actual: SourceKind,
) -> Limitation {
    Limitation::VueExternalScriptModeConflict {
        source_id: snapshot.id.clone(),
        target_source_id: target_source_id.clone(),
        declared: source_kind_name(declared).to_owned(),
        actual: source_kind_name(actual).to_owned(),
    }
}

fn decompose_template(
    snapshot: &SourceSnapshot,
    block: &MarkupBlock<'_>,
    template_uses: &mut Vec<SfcTemplateUse>,
    limitations: &mut Vec<Limitation>,
) {
    let tags = match template_tags(block.content, block.content_start) {
        Ok(tags) => tags,
        Err(detail) => {
            limitations.push(Limitation::VueTemplateOpaque {
                source_id: snapshot.id.clone(),
                detail,
            });
            return;
        }
    };
    for tag in tags {
        if is_native_or_vue_builtin(&tag.name) {
            if tag.name.eq_ignore_ascii_case("component")
                && (tag.attributes.contains_key(":is")
                    || tag.attributes.contains_key("v-bind:is")
                    || tag.attributes.contains_key("is"))
            {
                push_template_use(
                    &tag.name,
                    "component",
                    SfcTemplateUseKind::Dynamic,
                    tag.start,
                    tag.end,
                    template_uses,
                );
            }
            continue;
        }
        let inactive = tag
            .attributes
            .get("v-if")
            .and_then(|attribute| attribute.value.as_deref())
            .is_some_and(|value| value.trim() == "false");
        let kind = if inactive {
            SfcTemplateUseKind::Dynamic
        } else if tag.name.contains('.') {
            SfcTemplateUseKind::Namespace
        } else {
            SfcTemplateUseKind::Static
        };
        let binding_name = tag
            .name
            .split('.')
            .next()
            .map(pascal_case)
            .unwrap_or_default();
        push_template_use(
            &tag.name,
            &binding_name,
            kind,
            tag.start,
            tag.end,
            template_uses,
        );
    }
}

fn push_template_use(
    tag_name: &str,
    binding_name: &str,
    kind: SfcTemplateUseKind,
    start: usize,
    end: usize,
    template_uses: &mut Vec<SfcTemplateUse>,
) {
    if let Some(span) = source_span(start, end) {
        template_uses.push(SfcTemplateUse {
            tag_name: tag_name.to_owned(),
            binding_name: binding_name.to_owned(),
            kind,
            span,
        });
    }
}

fn decompose_style(
    snapshot: &SourceSnapshot,
    block: &MarkupBlock<'_>,
    resource_uses: &mut Vec<SfcResourceUse>,
    limitations: &mut Vec<Limitation>,
) {
    if let Some(src) = block.attributes.get("src") {
        if let Some(specifier) = literal_relative(src) {
            if let Some(span) = source_span(block.open_start, block.content_start) {
                resource_uses.push(SfcResourceUse {
                    specifier: specifier.to_owned(),
                    span,
                });
            }
        } else {
            limitations.push(decomposition_limitation(
                snapshot,
                "Vue style src must be one quoted relative path",
            ));
        }
    }
    match css_references(block.content, block.content_start) {
        Ok(references) => resource_uses.extend(references),
        Err(detail) => limitations.push(decomposition_limitation(snapshot, detail)),
    }
}

fn css_references(source: &str, base: usize) -> Result<Vec<SfcResourceUse>, String> {
    let bytes = source.as_bytes();
    let mut references = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes.get(cursor..cursor + 2) == Some(b"/*") {
            let Some(end) = source[cursor + 2..].find("*/") else {
                return Err("Vue style contains an unterminated comment".to_owned());
            };
            cursor += end + 4;
            continue;
        }
        if starts_ascii_case_insensitive(bytes, cursor, b"@import") {
            let mut value_start = cursor + 7;
            while bytes.get(value_start).is_some_and(u8::is_ascii_whitespace) {
                value_start += 1;
            }
            if bytes
                .get(value_start)
                .is_some_and(|byte| matches!(byte, b'\'' | b'"'))
            {
                let quote = bytes[value_start];
                let start = value_start + 1;
                let Some(relative_end) = bytes[start..].iter().position(|byte| *byte == quote)
                else {
                    return Err("Vue style contains an unterminated @import reference".to_owned());
                };
                let end = start + relative_end;
                let value = source[start..end].to_owned();
                if is_relative(&value) {
                    references.push(SfcResourceUse {
                        specifier: value,
                        span: source_span(base + start, base + end).ok_or_else(|| {
                            "Vue style reference span exceeds the model limit".to_owned()
                        })?,
                    });
                }
                cursor = end + 1;
                continue;
            }
        }
        let at_token_boundary =
            cursor == 0 || !bytes[cursor - 1].is_ascii_alphanumeric() && bytes[cursor - 1] != b'_';
        if at_token_boundary && starts_ascii_case_insensitive(bytes, cursor, b"url") {
            let mut open = cursor + 3;
            while bytes.get(open).is_some_and(u8::is_ascii_whitespace) {
                open += 1;
            }
            if bytes.get(open) == Some(&b'(') {
                let (value, start, end, next) = css_value(source, open + 1, b')')?;
                if is_relative(&value) {
                    references.push(SfcResourceUse {
                        specifier: value,
                        span: source_span(base + start, base + end).ok_or_else(|| {
                            "Vue style reference span exceeds the model limit".to_owned()
                        })?,
                    });
                }
                cursor = next;
                continue;
            }
        }
        cursor += 1;
    }
    Ok(references)
}

fn css_value(
    source: &str,
    mut cursor: usize,
    terminator: u8,
) -> Result<(String, usize, usize, usize), String> {
    let bytes = source.as_bytes();
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    let quote = bytes
        .get(cursor)
        .copied()
        .filter(|byte| matches!(byte, b'\'' | b'"'));
    if quote.is_some() {
        cursor += 1;
    }
    let start = cursor;
    while let Some(byte) = bytes.get(cursor).copied() {
        if quote.is_some_and(|expected| byte == expected) || (quote.is_none() && byte == terminator)
        {
            break;
        }
        cursor += 1;
    }
    if cursor == bytes.len() {
        return Err("Vue style contains an unterminated url() reference".to_owned());
    }
    let end = cursor;
    if quote.is_some() {
        cursor += 1;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&terminator) {
            return Err("Vue style url() has trailing unsupported syntax".to_owned());
        }
    }
    Ok((source[start..end].trim().to_owned(), start, end, cursor + 1))
}

fn script_kind(attribute: Option<&Attribute>) -> Result<SourceKind, &'static str> {
    let Some(attribute) = attribute else {
        return Ok(SourceKind::JavaScript);
    };
    let Some(value) = attribute.value.as_deref() else {
        return Err("Vue script lang is missing its value");
    };
    match value.to_ascii_lowercase().as_str() {
        "js" | "javascript" => Ok(SourceKind::JavaScript),
        "jsx" => Ok(SourceKind::Jsx),
        "ts" | "typescript" => Ok(SourceKind::TypeScript),
        "tsx" => Ok(SourceKind::Tsx),
        _ => Err("Vue script lang is unsupported"),
    }
}

fn literal_relative(attribute: &Attribute) -> Option<&str> {
    let value = attribute.value.as_deref()?;
    (attribute.quoted && !value.is_empty() && is_relative(value)).then_some(value)
}

fn is_relative(value: &str) -> bool {
    value.starts_with("./") || value.starts_with("../")
}

fn compatible_script_kinds(declared: SourceKind, actual: SourceKind) -> bool {
    script_family(declared) == script_family(actual)
}

fn script_family(kind: SourceKind) -> u8 {
    match kind {
        SourceKind::JavaScript | SourceKind::Mjs | SourceKind::CommonJs => 1,
        SourceKind::Jsx => 2,
        SourceKind::TypeScript | SourceKind::Mts | SourceKind::Cts => 3,
        SourceKind::Tsx => 4,
        _ => 0,
    }
}

fn source_kind_name(kind: SourceKind) -> &'static str {
    match script_family(kind) {
        1 => "js",
        2 => "jsx",
        3 => "ts",
        4 => "tsx",
        _ => "non-script",
    }
}

fn is_native_or_vue_builtin(tag: &str) -> bool {
    let lower = tag.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "a" | "abbr"
            | "article"
            | "aside"
            | "audio"
            | "b"
            | "body"
            | "button"
            | "canvas"
            | "code"
            | "div"
            | "em"
            | "fieldset"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "html"
            | "i"
            | "iframe"
            | "img"
            | "input"
            | "label"
            | "li"
            | "link"
            | "main"
            | "nav"
            | "ol"
            | "option"
            | "p"
            | "path"
            | "section"
            | "select"
            | "slot"
            | "small"
            | "span"
            | "strong"
            | "style"
            | "svg"
            | "table"
            | "tbody"
            | "td"
            | "template"
            | "textarea"
            | "th"
            | "thead"
            | "tr"
            | "transition"
            | "ul"
            | "video"
            | "component"
            | "keep-alive"
            | "suspense"
            | "teleport"
            | "transition-group"
    )
}

fn pascal_case(value: &str) -> String {
    let mut output = String::new();
    let mut uppercase = true;
    for character in value.chars() {
        if character == '-' || character == '_' {
            uppercase = true;
        } else if uppercase {
            output.extend(character.to_uppercase());
            uppercase = false;
        } else {
            output.push(character);
        }
    }
    output
}

fn source_span(start: usize, end: usize) -> Option<SourceSpan> {
    Some(SourceSpan {
        start: u32::try_from(start).ok()?,
        end: u32::try_from(end).ok()?,
    })
}

fn decomposition_unknown(snapshot: &SourceSnapshot, detail: String) -> SfcDecomposition {
    SfcDecomposition {
        source_id: snapshot.id.clone(),
        dialect: SfcDialect::Vue,
        state: CapabilityState::Incomplete,
        module_export_known: false,
        inline_scripts: Vec::new(),
        external_scripts: Vec::new(),
        template_uses: Vec::new(),
        resource_uses: Vec::new(),
        limitations: vec![Limitation::SfcDecompositionUnknown {
            source_id: snapshot.id.clone(),
            detail,
        }],
    }
}

fn decomposition_limitation(snapshot: &SourceSnapshot, detail: impl Into<String>) -> Limitation {
    Limitation::SfcDecompositionUnknown {
        source_id: snapshot.id.clone(),
        detail: detail.into(),
    }
}

fn starts_ascii_case_insensitive(bytes: &[u8], start: usize, target: &[u8]) -> bool {
    bytes
        .get(start..start + target.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(target))
}
