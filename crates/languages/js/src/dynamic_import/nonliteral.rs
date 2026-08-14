use std::collections::BTreeMap;

use lumin_model::{
    DynamicImportTargetScope, FileFacts, Limitation, RepoPath, SourceSnapshot, SourceSpan,
};
use oxc_ast::ast::{BinaryOperator, Expression};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct NonLiteralDynamicImportTemplate {
    pub(crate) span: SourceSpan,
    pub(crate) static_prefix: Option<String>,
}

pub(crate) fn nonliteral_template(
    expression: &oxc_ast::ast::ImportExpression<'_>,
) -> NonLiteralDynamicImportTemplate {
    NonLiteralDynamicImportTemplate {
        span: SourceSpan {
            start: expression.span.start,
            end: expression.span.end,
        },
        static_prefix: guaranteed_string_prefix(&expression.source)
            .filter(|prefix| !prefix.is_empty()),
    }
}

pub(crate) fn scope_limitations(facts: &mut [FileFacts], sources: &[SourceSnapshot]) {
    let paths = sources
        .iter()
        .map(|source| (source.id.clone(), &source.path))
        .collect::<BTreeMap<_, _>>();

    for file in facts {
        for limitation in &mut file.limitations {
            let Limitation::DynamicImportNonLiteral {
                source_id,
                static_prefix,
                candidates,
                target_scope,
                ..
            } = limitation
            else {
                continue;
            };

            candidates.clear();
            *target_scope = DynamicImportTargetScope::Workspace;
            let Some(importer_path) = paths.get(source_id) else {
                continue;
            };
            let Some(_domain) = static_prefix
                .as_deref()
                .and_then(|prefix| RelativeInventoryDomain::from_importer(importer_path, prefix))
            else {
                continue;
            };

            // An arbitrary suffix can inject separators and dot segments, so even a
            // relative static prefix can normalize to any repository source. Represent
            // that current-inventory domain once instead of repeating every source ID
            // in every limitation. The gate treats this growing domain as required
            // incomplete evidence because it has no durable future-membership lease.
            *target_scope = DynamicImportTargetScope::SourceInventory;
        }
    }
}

struct RelativeInventoryDomain;

impl RelativeInventoryDomain {
    fn from_importer(importer: &RepoPath, prefix: &str) -> Option<Self> {
        if !(prefix.starts_with("./") || prefix.starts_with("../"))
            || prefix.contains(['\\', '\0', '?', '#', '%'])
        {
            return None;
        }
        let (directory, tail) = prefix.rsplit_once('/')?;
        if matches!(tail, "." | "..") {
            return None;
        }
        importer.resolve_portable_relative(directory)?;
        Some(Self)
    }
}

fn guaranteed_string_prefix(expression: &Expression<'_>) -> Option<String> {
    let expression = expression.get_inner_expression();
    match expression {
        Expression::StringLiteral(literal) => Some(literal.value.to_string()),
        Expression::TemplateLiteral(literal) => literal
            .quasis
            .first()
            .and_then(|quasi| quasi.value.cooked.as_ref())
            .map(ToString::to_string),
        Expression::BinaryExpression(expression)
            if expression.operator == BinaryOperator::Addition =>
        {
            guaranteed_string_prefix(&expression.left)
        }
        Expression::ConditionalExpression(expression) => common_prefix(
            &guaranteed_string_prefix(&expression.consequent)?,
            &guaranteed_string_prefix(&expression.alternate)?,
        ),
        Expression::SequenceExpression(expression) => expression
            .expressions
            .last()
            .and_then(guaranteed_string_prefix),
        Expression::AssignmentExpression(expression) if expression.operator.is_assign() => {
            guaranteed_string_prefix(&expression.right)
        }
        _ => None,
    }
}

fn common_prefix(left: &str, right: &str) -> Option<String> {
    let mut end = left
        .as_bytes()
        .iter()
        .zip(right.as_bytes())
        .take_while(|(left, right)| left == right)
        .count();
    while end > 0 && !left.is_char_boundary(end) {
        end -= 1;
    }
    (end > 0).then(|| left[..end].to_owned())
}
