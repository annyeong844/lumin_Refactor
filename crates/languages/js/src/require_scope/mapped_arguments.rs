use oxc_ast::ast::{
    ComputedMemberExpression, Expression, JSXMemberExpressionObject, SimpleAssignmentTarget,
    StaticMemberExpression,
};

use super::{
    MutationObservation, NameResolution, RequireScopeKind, RequireScopeModel, TrackedName,
};

pub(super) fn target_is_require_slot(
    target: &SimpleAssignmentTarget<'_>,
    root_has_commonjs_wrapper: bool,
    current_strict: bool,
    arguments_is_shadowed: bool,
) -> bool {
    if !root_has_commonjs_wrapper || current_strict || arguments_is_shadowed {
        return false;
    }
    let SimpleAssignmentTarget::ComputedMemberExpression(member) = target else {
        return false;
    };
    if !member
        .object
        .without_parentheses()
        .is_specific_id("arguments")
    {
        return false;
    }
    member
        .static_property_name()
        .is_some_and(|name| name.as_str() == "1")
        || matches!(
            member.expression.without_parentheses(),
            oxc_ast::ast::Expression::NumericLiteral(literal) if literal.value == 1.0
        )
}

pub(super) fn mapped_require_mutations(
    root_has_commonjs_wrapper: bool,
    model: &RequireScopeModel,
    observations: &[MutationObservation],
) -> Vec<MutationObservation> {
    let root_arguments_are_mapped = root_has_commonjs_wrapper
        && model
            .scopes
            .first()
            .is_some_and(|scope| scope.kind == RequireScopeKind::Root && !scope.strict);
    observations
        .iter()
        .copied()
        .filter(|observation| {
            root_arguments_are_mapped
                && model.resolve_name(observation.scope, TrackedName::Arguments)
                    != NameResolution::Bound
                && !model.has_binding(observation.scope, TrackedName::Require)
        })
        .collect()
}

pub(super) fn non_require_computed_object_span(
    expression: &ComputedMemberExpression<'_>,
) -> Option<(u32, u32)> {
    let identifier = expression
        .object
        .without_parentheses()
        .get_identifier_reference()?;
    (identifier.name == "arguments"
        && expression
            .static_property_name()
            .is_some_and(|name| name.as_str() != "1"))
    .then_some((identifier.span.start, identifier.span.end))
}

pub(super) fn static_object_span(expression: &StaticMemberExpression<'_>) -> Option<(u32, u32)> {
    let identifier = expression
        .object
        .without_parentheses()
        .get_identifier_reference()?;
    (identifier.name == "arguments").then_some((identifier.span.start, identifier.span.end))
}

pub(super) fn callee_invokes_arguments_receiver(callee: &Expression<'_>) -> bool {
    callee
        .get_member_expr()
        .and_then(|member| {
            member
                .object()
                .without_parentheses()
                .get_identifier_reference()
        })
        .is_some_and(|identifier| identifier.name == "arguments")
}

pub(super) fn jsx_member_object_span(
    expression: &JSXMemberExpressionObject<'_>,
) -> Option<(u32, u32)> {
    let JSXMemberExpressionObject::IdentifierReference(identifier) = expression else {
        return None;
    };
    (identifier.name == "arguments").then_some((identifier.span.start, identifier.span.end))
}
