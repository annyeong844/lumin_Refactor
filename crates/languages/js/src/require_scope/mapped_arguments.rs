use oxc_ast::ast::{
    AssignmentOperator, BinaryExpression, BinaryOperator, ComputedMemberExpression, Expression,
    JSXMemberExpressionObject, LogicalExpression, MemberExpression, SimpleAssignmentTarget,
    StaticMemberExpression,
};

use super::{
    MutationObservation, NameResolution, RequireScopeKind, RequireScopeModel, TrackedName,
};

pub(super) fn direct_arguments_reference(expression: &Expression<'_>) -> Option<(u32, u32)> {
    let identifier = expression
        .get_inner_expression()
        .get_identifier_reference()?;
    (identifier.name == "arguments").then_some((identifier.span.start, identifier.span.end))
}

pub(super) fn value_arguments_references(expression: &Expression<'_>) -> Vec<(u32, u32)> {
    let mut references = Vec::new();
    collect_value_arguments_references(expression, &mut references);
    references
}

pub(super) fn definitely_arguments_value(expression: &Expression<'_>) -> bool {
    let expression = expression.get_inner_expression();
    if let Some(identifier) = expression.get_identifier_reference() {
        return identifier.name == "arguments";
    }
    match expression {
        Expression::ConditionalExpression(expression) => {
            definitely_arguments_value(&expression.consequent)
                && definitely_arguments_value(&expression.alternate)
        }
        Expression::LogicalExpression(expression) if expression.operator.is_and() => {
            definitely_arguments_value(&expression.left)
                && definitely_arguments_value(&expression.right)
        }
        Expression::LogicalExpression(expression) => definitely_arguments_value(&expression.left),
        Expression::SequenceExpression(expression) => expression
            .expressions
            .last()
            .is_some_and(definitely_arguments_value),
        Expression::AssignmentExpression(expression) if expression.operator.is_assign() => {
            definitely_arguments_value(&expression.right)
        }
        Expression::AssignmentExpression(expression)
            if matches!(
                expression.operator,
                AssignmentOperator::LogicalOr | AssignmentOperator::LogicalNullish
            ) =>
        {
            expression
                .left
                .as_simple_assignment_target()
                .is_some_and(|target| direct_arguments_target(target).is_some())
        }
        _ => false,
    }
}

pub(super) struct BinaryArgumentsUse {
    pub(super) references: Vec<(u32, u32)>,
    pub(super) may_mutate: bool,
}

pub(super) fn binary_arguments_use(expression: &BinaryExpression<'_>) -> BinaryArgumentsUse {
    let left = value_arguments_references(&expression.left);
    let right = value_arguments_references(&expression.right);
    let may_mutate = match expression.operator {
        BinaryOperator::StrictEquality | BinaryOperator::StrictInequality => false,
        BinaryOperator::Equality | BinaryOperator::Inequality => {
            (!left.is_empty() && abstract_equality_may_coerce_arguments(&expression.right))
                || (!right.is_empty() && abstract_equality_may_coerce_arguments(&expression.left))
        }
        BinaryOperator::In => !left.is_empty(),
        BinaryOperator::Instanceof => {
            !right.is_empty() || (!left.is_empty() && instanceof_rhs_may_observe(&expression.right))
        }
        _ => !left.is_empty() || !right.is_empty(),
    };
    BinaryArgumentsUse {
        references: left.into_iter().chain(right).collect(),
        may_mutate,
    }
}

pub(super) fn logical_rhs_cannot_execute(expression: &LogicalExpression<'_>) -> bool {
    (expression.operator.is_or() && matches!(known_truthiness(&expression.left), Some(true)))
        || (expression.operator.is_and()
            && matches!(known_truthiness(&expression.left), Some(false)))
        || (expression.operator.is_coalesce() && definitely_arguments_value(&expression.left))
}

pub(super) fn logical_assignment_rhs_cannot_execute(
    expression: &oxc_ast::ast::AssignmentExpression<'_>,
) -> bool {
    matches!(
        expression.operator,
        AssignmentOperator::LogicalOr | AssignmentOperator::LogicalNullish
    ) && expression
        .left
        .as_simple_assignment_target()
        .is_some_and(|target| direct_arguments_target(target).is_some())
}

pub(super) fn condition_truthiness(expression: &Expression<'_>) -> Option<bool> {
    known_truthiness(expression)
}

fn known_truthiness(expression: &Expression<'_>) -> Option<bool> {
    let expression = expression.get_inner_expression();
    if definitely_arguments_value(expression) {
        return Some(true);
    }
    match expression {
        Expression::BooleanLiteral(literal) => Some(literal.value),
        Expression::BigIntLiteral(literal) => Some(literal.value.as_str() != "0"),
        Expression::NullLiteral(_) => Some(false),
        Expression::NumericLiteral(literal) => {
            Some(literal.value != 0.0 && !literal.value.is_nan())
        }
        Expression::StringLiteral(literal) => Some(!literal.value.is_empty()),
        Expression::TemplateLiteral(literal) if literal.expressions.is_empty() => literal
            .quasis
            .first()
            .and_then(|quasi| quasi.value.cooked.as_ref())
            .map(|value| !value.is_empty()),
        Expression::ArrayExpression(_)
        | Expression::ArrowFunctionExpression(_)
        | Expression::ClassExpression(_)
        | Expression::FunctionExpression(_)
        | Expression::NewExpression(_)
        | Expression::ObjectExpression(_)
        | Expression::RegExpLiteral(_) => Some(true),
        Expression::UnaryExpression(expression) if expression.operator.is_void() => Some(false),
        Expression::SequenceExpression(expression) => {
            expression.expressions.last().and_then(known_truthiness)
        }
        Expression::ConditionalExpression(expression) => {
            let consequent = known_truthiness(&expression.consequent)?;
            (known_truthiness(&expression.alternate) == Some(consequent)).then_some(consequent)
        }
        Expression::AssignmentExpression(expression) if expression.operator.is_assign() => {
            known_truthiness(&expression.right)
        }
        _ => None,
    }
}

fn abstract_equality_may_coerce_arguments(peer: &Expression<'_>) -> bool {
    let peer = peer.get_inner_expression();
    if peer.is_null()
        || peer.is_void()
        || peer
            .get_identifier_reference()
            .is_some_and(|identifier| identifier.name == "arguments")
        || matches!(
            peer,
            Expression::ArrayExpression(_)
                | Expression::ArrowFunctionExpression(_)
                | Expression::ClassExpression(_)
                | Expression::FunctionExpression(_)
                | Expression::NewExpression(_)
                | Expression::ObjectExpression(_)
                | Expression::RegExpLiteral(_)
        )
    {
        return false;
    }
    match peer {
        Expression::ConditionalExpression(expression) => match known_truthiness(&expression.test) {
            Some(true) => abstract_equality_may_coerce_arguments(&expression.consequent),
            Some(false) => abstract_equality_may_coerce_arguments(&expression.alternate),
            None => {
                abstract_equality_may_coerce_arguments(&expression.consequent)
                    || abstract_equality_may_coerce_arguments(&expression.alternate)
            }
        },
        Expression::LogicalExpression(expression) => {
            abstract_equality_may_coerce_arguments(&expression.left)
                || (!logical_rhs_cannot_execute(expression)
                    && abstract_equality_may_coerce_arguments(&expression.right))
        }
        Expression::SequenceExpression(expression) => expression
            .expressions
            .last()
            .is_none_or(abstract_equality_may_coerce_arguments),
        Expression::AssignmentExpression(expression) if expression.operator.is_assign() => {
            abstract_equality_may_coerce_arguments(&expression.right)
        }
        _ => true,
    }
}

fn instanceof_rhs_may_observe(rhs: &Expression<'_>) -> bool {
    !expression_is_definitely_primitive(rhs.get_inner_expression())
}

fn expression_is_definitely_primitive(expression: &Expression<'_>) -> bool {
    match expression.get_inner_expression() {
        Expression::BigIntLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::PrivateInExpression(_)
        | Expression::StringLiteral(_)
        | Expression::TemplateLiteral(_)
        | Expression::UnaryExpression(_)
        | Expression::UpdateExpression(_)
        | Expression::BinaryExpression(_) => true,
        Expression::AssignmentExpression(expression) => {
            expression.operator.is_arithmetic()
                || expression.operator.is_bitwise()
                || (expression.operator.is_assign()
                    && expression_is_definitely_primitive(&expression.right))
        }
        Expression::ConditionalExpression(expression) => {
            expression_is_definitely_primitive(&expression.consequent)
                && expression_is_definitely_primitive(&expression.alternate)
        }
        Expression::LogicalExpression(expression) => {
            expression_is_definitely_primitive(&expression.left)
                && expression_is_definitely_primitive(&expression.right)
        }
        Expression::SequenceExpression(expression) => expression
            .expressions
            .last()
            .is_some_and(expression_is_definitely_primitive),
        _ => false,
    }
}

fn collect_value_arguments_references(
    expression: &Expression<'_>,
    references: &mut Vec<(u32, u32)>,
) {
    let expression = expression.get_inner_expression();
    if let Some(identifier) = expression.get_identifier_reference() {
        if identifier.name == "arguments" {
            references.push((identifier.span.start, identifier.span.end));
        }
        return;
    }

    match expression {
        Expression::ConditionalExpression(expression) => {
            let truthiness = known_truthiness(&expression.test);
            if truthiness != Some(false) {
                collect_value_arguments_references(&expression.consequent, references);
            }
            if truthiness != Some(true) {
                collect_value_arguments_references(&expression.alternate, references);
            }
        }
        Expression::LogicalExpression(expression) => {
            let rhs_cannot_execute = logical_rhs_cannot_execute(expression);
            if !expression.operator.is_and() {
                collect_value_arguments_references(&expression.left, references);
            }
            if !rhs_cannot_execute {
                collect_value_arguments_references(&expression.right, references);
            }
        }
        Expression::SequenceExpression(expression) => {
            if let Some(last) = expression.expressions.last() {
                collect_value_arguments_references(last, references);
            }
        }
        Expression::AssignmentExpression(expression) if expression.operator.is_assign() => {
            collect_value_arguments_references(&expression.right, references);
        }
        Expression::AssignmentExpression(expression) if expression.operator.is_logical() => {
            if matches!(
                expression.operator,
                AssignmentOperator::LogicalOr | AssignmentOperator::LogicalNullish
            ) && let Some(target) = expression.left.as_simple_assignment_target()
                && let Some(reference) = direct_arguments_target(target)
            {
                references.push(reference);
            }
            if !logical_assignment_rhs_cannot_execute(expression) {
                collect_value_arguments_references(&expression.right, references);
            }
        }
        _ => {}
    }
}

pub(super) fn direct_arguments_target(target: &SimpleAssignmentTarget<'_>) -> Option<(u32, u32)> {
    match target {
        SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier)
            if identifier.name == "arguments" =>
        {
            Some((identifier.span.start, identifier.span.end))
        }
        _ => target.get_expression().and_then(direct_arguments_reference),
    }
}

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
    computed_arguments_slot(member) == Some(1)
}

pub(super) fn member_is_wrapper_export_slot(expression: &MemberExpression<'_>) -> bool {
    let MemberExpression::ComputedMemberExpression(member) = expression else {
        return false;
    };
    matches!(computed_arguments_slot(member), Some(0 | 2))
}

fn computed_arguments_slot(member: &ComputedMemberExpression<'_>) -> Option<u8> {
    if !member
        .object
        .without_parentheses()
        .is_specific_id("arguments")
    {
        return None;
    }
    if let Some(name) = member.static_property_name() {
        return match name.as_str() {
            "0" => Some(0),
            "1" => Some(1),
            "2" => Some(2),
            _ => None,
        };
    }
    match member.expression.without_parentheses() {
        oxc_ast::ast::Expression::NumericLiteral(literal) => match literal.value {
            0.0 => Some(0),
            1.0 => Some(1),
            2.0 => Some(2),
            _ => None,
        },
        _ => None,
    }
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
        && (computed_arguments_slot(expression).is_some_and(|slot| slot != 1)
            || expression
                .static_property_name()
                .is_some_and(|name| name.as_str() != "1")))
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
