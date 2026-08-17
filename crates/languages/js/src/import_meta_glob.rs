use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    FileFacts, ImportKind, ImportMetaGlobTargetScope, Limitation, LogicalSourceId,
    ModuleRequestKind, RepoPath, SourceSnapshot, SourceSpan, SourceUseFact, SymbolNamespace,
};
use oxc_ast::ast::{Argument, ArrayExpressionElement, CallExpression, MemberExpression};

use super::dynamic_import::transparent_runtime_expression;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct UnsupportedImportMetaGlobTemplate {
    pub(crate) span: SourceSpan,
    pub(crate) patterns: Vec<String>,
    pub(crate) detail: String,
}

pub(crate) enum ParsedImportMetaGlob {
    Supported {
        span: SourceSpan,
        patterns: Vec<String>,
    },
    Unsupported(UnsupportedImportMetaGlobTemplate),
}

pub(crate) fn parse_call(expression: &CallExpression<'_>) -> ParsedImportMetaGlob {
    let span = SourceSpan {
        start: expression.span.start,
        end: expression.span.end,
    };
    let parsed_patterns = expression.arguments.first().and_then(literal_patterns);
    let patterns = parsed_patterns.clone().unwrap_or_default();

    let unsupported = if expression.optional {
        Some("optional import.meta.glob calls are unsupported")
    } else if !matches!(
        transparent_runtime_expression(&expression.callee).get_member_expr(),
        Some(MemberExpression::StaticMemberExpression(member)) if !member.optional
    ) {
        Some("computed or optional import.meta.glob members are unsupported")
    } else if expression.type_arguments.is_some() {
        Some("typed import.meta.glob calls are unsupported")
    } else if expression.arguments.len() != 1 {
        Some("import.meta.glob requires exactly one pattern argument")
    } else if parsed_patterns.is_none() {
        Some("import.meta.glob patterns must be one literal or a literal array")
    } else if patterns.is_empty() {
        Some("import.meta.glob pattern arrays must not be empty")
    } else if let Err(detail) = validate_patterns(&patterns) {
        return ParsedImportMetaGlob::Unsupported(UnsupportedImportMetaGlobTemplate {
            span,
            patterns,
            detail,
        });
    } else {
        None
    };

    match unsupported {
        Some(detail) => ParsedImportMetaGlob::Unsupported(UnsupportedImportMetaGlobTemplate {
            span,
            patterns,
            detail: detail.to_owned(),
        }),
        None => ParsedImportMetaGlob::Supported { span, patterns },
    }
}

pub(crate) fn scope(
    facts: &mut [FileFacts],
    sources: &[SourceSnapshot],
    is_hard_excluded_component: fn(&str) -> bool,
) {
    let paths = sources
        .iter()
        .map(|source| (source.id.clone(), &source.path))
        .collect::<BTreeMap<_, _>>();

    for file in facts {
        let mut groups = BTreeMap::<(u32, u32), Vec<String>>::new();
        file.uses.retain(|source_use| {
            if source_use.request_kind != ModuleRequestKind::ImportMetaGlob {
                return true;
            }
            groups
                .entry((source_use.span.start, source_use.span.end))
                .or_default()
                .push(source_use.specifier.clone());
            false
        });

        let Some(importer_path) = paths.get(&file.source_id).copied() else {
            for ((start, end), mut patterns) in groups {
                canonicalize_patterns(&mut patterns);
                file.limitations
                    .push(Limitation::ImportMetaGlobUnsupported {
                        source_id: file.source_id.clone(),
                        source_unit: Box::new(file.source_unit.clone()),
                        span: SourceSpan { start, end },
                        patterns: patterns.into_boxed_slice(),
                        candidates: Vec::new(),
                        target_scope: ImportMetaGlobTargetScope::Package,
                        detail: "import.meta.glob importer path is unavailable".to_owned(),
                    });
            }
            continue;
        };

        for ((start, end), mut patterns) in groups {
            canonicalize_patterns(&mut patterns);
            if patterns_enter_hard_excluded_context(&patterns, is_hard_excluded_component) {
                file.limitations
                    .push(Limitation::ImportMetaGlobUnsupported {
                        source_id: file.source_id.clone(),
                        source_unit: Box::new(file.source_unit.clone()),
                        span: SourceSpan { start, end },
                        patterns: patterns.into_boxed_slice(),
                        candidates: Vec::new(),
                        target_scope: ImportMetaGlobTargetScope::Package,
                        detail: "import.meta.glob cannot observe hard-excluded inventory contexts"
                            .to_owned(),
                    });
                continue;
            }
            let Some(expanded) = expand_patterns(importer_path, &patterns, sources) else {
                file.limitations
                    .push(Limitation::ImportMetaGlobUnsupported {
                        source_id: file.source_id.clone(),
                        source_unit: Box::new(file.source_unit.clone()),
                        span: SourceSpan { start, end },
                        patterns: patterns.into_boxed_slice(),
                        candidates: Vec::new(),
                        target_scope: ImportMetaGlobTargetScope::Package,
                        detail: "import.meta.glob pattern escapes the observed repository"
                            .to_owned(),
                    });
                continue;
            };
            for candidate in expanded {
                let Some(specifier) = relative_specifier(importer_path, &candidate.path) else {
                    continue;
                };
                file.uses.push(SourceUseFact {
                    importer: file.source_id.clone(),
                    specifier,
                    imported_name: None,
                    local_name: None,
                    namespace: SymbolNamespace::Value,
                    kind: ImportKind::DynamicBroad,
                    request_kind: ModuleRequestKind::ImportMetaGlob,
                    span: SourceSpan { start, end },
                });
            }
        }

        for limitation in &mut file.limitations {
            let Limitation::ImportMetaGlobUnsupported {
                patterns,
                candidates,
                target_scope,
                ..
            } = limitation
            else {
                continue;
            };
            candidates.clear();
            *target_scope = ImportMetaGlobTargetScope::Package;
            if patterns_enter_hard_excluded_context(patterns, is_hard_excluded_component) {
                continue;
            }
            let Some(scoped) = unsupported_candidates(importer_path, patterns, sources) else {
                continue;
            };
            candidates.extend(scoped);
            *target_scope = ImportMetaGlobTargetScope::ExplicitTargets;
        }

        file.uses.sort_by(|left, right| {
            left.specifier
                .cmp(&right.specifier)
                .then_with(|| left.namespace.cmp(&right.namespace))
                .then_with(|| left.imported_name.cmp(&right.imported_name))
                .then_with(|| left.span.start.cmp(&right.span.start))
                .then_with(|| left.span.end.cmp(&right.span.end))
        });
        file.uses.dedup();
    }
}

fn literal_patterns(argument: &Argument<'_>) -> Option<Vec<String>> {
    match argument {
        Argument::StringLiteral(literal) => Some(vec![literal.value.to_string()]),
        Argument::TemplateLiteral(template) => literal_template(template).map(|value| vec![value]),
        Argument::ArrayExpression(array) => array
            .elements
            .iter()
            .map(|element| match element {
                ArrayExpressionElement::StringLiteral(literal) => Some(literal.value.to_string()),
                ArrayExpressionElement::TemplateLiteral(template) => literal_template(template),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn literal_template(template: &oxc_ast::ast::TemplateLiteral<'_>) -> Option<String> {
    if !template.expressions.is_empty() || template.quasis.len() != 1 {
        return None;
    }
    template.quasis[0]
        .value
        .cooked
        .as_ref()
        .map(ToString::to_string)
}

fn validate_patterns(patterns: &[String]) -> Result<(), String> {
    let mut positive = false;
    for pattern in patterns {
        let (negative, pattern) = strip_negative(pattern);
        if !negative {
            positive = true;
        }
        validate_relative_pattern(pattern)?;
    }
    if !positive {
        return Err("import.meta.glob requires at least one positive pattern".to_owned());
    }
    Ok(())
}

fn validate_relative_pattern(pattern: &str) -> Result<(), String> {
    if !(pattern.starts_with("./") || pattern.starts_with("../")) {
        return Err("import.meta.glob aliases and rooted patterns are unsupported".to_owned());
    }
    if pattern.contains(['\\', '\0', '?', '#', '%', '[', ']', '{', '}']) {
        return Err("import.meta.glob pattern grammar is unsupported".to_owned());
    }
    if pattern.ends_with('/') || pattern.split('/').any(str::is_empty) {
        return Err("import.meta.glob patterns must name a source path".to_owned());
    }
    let mut saw_regular_component = false;
    for component in pattern.split('/') {
        match component {
            "." | ".." if !saw_regular_component => {}
            "." | ".." => {
                return Err(
                    "import.meta.glob dot segments after a path component are unsupported"
                        .to_owned(),
                );
            }
            "**" => saw_regular_component = true,
            value if value.contains("**") => {
                return Err(
                    "import.meta.glob globstar must occupy a whole path component".to_owned(),
                );
            }
            _ => saw_regular_component = true,
        }
    }
    Ok(())
}

fn strip_negative(pattern: &str) -> (bool, &str) {
    match pattern.strip_prefix('!') {
        Some(pattern) => (true, pattern),
        None => (false, pattern),
    }
}

fn canonicalize_patterns(patterns: &mut Vec<String>) {
    patterns.sort();
    patterns.dedup();
}

fn patterns_enter_hard_excluded_context(
    patterns: &[String],
    is_hard_excluded_component: fn(&str) -> bool,
) -> bool {
    patterns.iter().any(|pattern| {
        let (negative, pattern) = strip_negative(pattern);
        !negative
            && pattern
                .split('/')
                .filter(|component| !component.contains('*'))
                .any(is_hard_excluded_component)
    })
}

fn expand_patterns<'a>(
    importer: &RepoPath,
    patterns: &[String],
    sources: &'a [SourceSnapshot],
) -> Option<Vec<&'a SourceSnapshot>> {
    let parsed = patterns
        .iter()
        .map(|pattern| GlobPattern::parse(importer, pattern))
        .collect::<Option<Vec<_>>>()?;
    let mut matched = sources
        .iter()
        .filter(|source| {
            parsed
                .iter()
                .any(|pattern| !pattern.negative && pattern.matches(&source.path))
                && !parsed
                    .iter()
                    .any(|pattern| pattern.negative && pattern.matches(&source.path))
        })
        .collect::<Vec<_>>();
    matched.sort_by(|left, right| left.id.cmp(&right.id));
    matched.dedup_by(|left, right| left.id == right.id);
    Some(matched)
}

fn unsupported_candidates(
    importer: &RepoPath,
    patterns: &[String],
    sources: &[SourceSnapshot],
) -> Option<Vec<LogicalSourceId>> {
    let mut domains = Vec::new();
    for pattern in patterns {
        let (negative, pattern) = strip_negative(pattern);
        if negative {
            continue;
        }
        domains.push(StaticDomain::from_pattern(importer, pattern)?);
    }
    if domains.is_empty() {
        return None;
    }
    let candidates = sources
        .iter()
        .filter(|source| domains.iter().any(|domain| domain.contains(&source.path)))
        .map(|source| source.id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Some(candidates)
}

enum StaticDomain {
    Exact(RepoPath),
    Descendants(RepoPath),
}

impl StaticDomain {
    fn from_pattern(importer: &RepoPath, pattern: &str) -> Option<Self> {
        validate_relative_pattern(pattern).ok()?;
        let components = pattern.split('/').collect::<Vec<_>>();
        let first_meta = components
            .iter()
            .position(|component| component.contains('*'));
        match first_meta {
            None => importer.resolve_portable_relative(pattern).map(Self::Exact),
            Some(index) => {
                let base = components[..index].join("/");
                importer
                    .resolve_portable_relative(&base)
                    .map(Self::Descendants)
            }
        }
    }

    fn contains(&self, path: &RepoPath) -> bool {
        match self {
            Self::Exact(exact) => path == exact,
            Self::Descendants(root) => path.is_within(root),
        }
    }
}

struct GlobPattern {
    negative: bool,
    base: RepoPath,
    components: Vec<GlobComponent>,
}

enum GlobComponent {
    GlobStar,
    Segment(String),
}

impl GlobPattern {
    fn parse(importer: &RepoPath, raw: &str) -> Option<Self> {
        let (negative, pattern) = strip_negative(raw);
        validate_relative_pattern(pattern).ok()?;
        let components = pattern.split('/').collect::<Vec<_>>();
        let first_wildcard = components
            .iter()
            .position(|component| component.contains('*'));
        let tail_start = first_wildcard.unwrap_or(components.len().checked_sub(1)?);
        let base = importer.resolve_portable_relative(&components[..tail_start].join("/"))?;
        let components = components[tail_start..]
            .iter()
            .map(|component| {
                if *component == "**" {
                    GlobComponent::GlobStar
                } else {
                    GlobComponent::Segment((*component).to_owned())
                }
            })
            .collect();
        Some(Self {
            negative,
            base,
            components,
        })
    }

    fn matches(&self, path: &RepoPath) -> bool {
        let Some(relative) = path.portable_relative_to(&self.base) else {
            return false;
        };
        if relative.is_empty() {
            return false;
        }
        let components = relative.split('/').collect::<Vec<_>>();
        let mut memo = vec![vec![None; components.len() + 1]; self.components.len() + 1];
        match_components(&self.components, &components, 0, 0, &mut memo)
    }
}

fn match_components(
    pattern: &[GlobComponent],
    path: &[&str],
    pattern_index: usize,
    path_index: usize,
    memo: &mut [Vec<Option<bool>>],
) -> bool {
    if let Some(cached) = memo[pattern_index][path_index] {
        return cached;
    }
    let matched = match pattern.get(pattern_index) {
        None => path_index == path.len(),
        Some(GlobComponent::Segment(component)) => path.get(path_index).is_some_and(|value| {
            wildcard_segment_matches(component, value)
                && match_components(pattern, path, pattern_index + 1, path_index + 1, memo)
        }),
        Some(GlobComponent::GlobStar) => {
            match_components(pattern, path, pattern_index + 1, path_index, memo)
                || path.get(path_index).is_some_and(|component| {
                    !component.starts_with('.')
                        && match_components(pattern, path, pattern_index, path_index + 1, memo)
                })
        }
    };
    memo[pattern_index][path_index] = Some(matched);
    matched
}

fn wildcard_segment_matches(pattern: &str, value: &str) -> bool {
    if value.starts_with('.') && !pattern.starts_with('.') {
        return false;
    }
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;
    for byte in pattern {
        let mut current = vec![false; value.len() + 1];
        if *byte == b'*' {
            current[0] = previous[0];
            for index in 1..=value.len() {
                current[index] = previous[index] || current[index - 1];
            }
        } else {
            for index in 1..=value.len() {
                current[index] = previous[index - 1] && *byte == value[index - 1];
            }
        }
        previous = current;
    }
    previous[value.len()]
}

fn relative_specifier(importer: &RepoPath, target: &RepoPath) -> Option<String> {
    let mut ancestor = importer.parent()?;
    let mut parent_steps = 0_usize;
    loop {
        if let Some(relative) = target.portable_relative_to(&ancestor) {
            let mut parts = vec!["..".to_owned(); parent_steps];
            parts.extend(
                relative
                    .split('/')
                    .filter(|component| !component.is_empty())
                    .map(str::to_owned),
            );
            if parts.is_empty() {
                return None;
            }
            let relative = parts.join("/");
            return Some(if relative.starts_with("../") {
                relative
            } else {
                format!("./{relative}")
            });
        }
        ancestor = ancestor.parent()?;
        parent_steps += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_and_globstar_match_only_portable_components() {
        assert!(wildcard_segment_matches("*.ts", "one.ts"));
        assert!(!wildcard_segment_matches("*.ts", ".hidden.ts"));
        assert!(!wildcard_segment_matches("*.ts", "one.js"));
    }
}
