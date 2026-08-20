use std::collections::BTreeSet;

use lumin_model::{ImportKind, Limitation, ResolutionOutcome, ResolvedSourceUse};
use oxc_ast::ast::{
    ArrayAssignmentTarget, ArrayPattern, AssignmentExpression, AssignmentPattern, AssignmentTarget,
    AssignmentTargetMaybeDefault, AssignmentTargetProperty, BindingPattern,
    ComputedMemberExpression, Expression, ObjectAssignmentTarget, ObjectPattern, Program,
    VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};

use crate::dynamic_import::transparent_runtime_expression;

type SpanKey = (u32, u32);

pub(super) fn collect_computed_require_calls(program: &Program<'_>) -> BTreeSet<SpanKey> {
    let mut collector = ComputedRequireCollector::default();
    collector.visit_program(program);
    collector.calls
}

pub fn scope_commonjs_computed_limitations(resolved: &[ResolvedSourceUse]) -> Vec<Limitation> {
    let mut limitations = resolved
        .iter()
        .filter_map(|resolution| {
            if resolution.source_use.kind != ImportKind::CommonJsComputed {
                return None;
            }
            let ResolutionOutcome::Internal { target } = &resolution.outcome else {
                return None;
            };
            Some(Limitation::CommonJsComputedMember {
                source_id: resolution.source_use.importer.clone(),
                specifier: resolution.source_use.specifier.clone(),
                span: resolution.source_use.span.clone(),
                target: target.clone(),
            })
        })
        .collect::<Vec<_>>();
    limitations.sort_by(Limitation::canonical_cmp);
    limitations.dedup();
    limitations
}

#[derive(Default)]
struct ComputedRequireCollector {
    calls: BTreeSet<SpanKey>,
}

impl<'a> Visit<'a> for ComputedRequireCollector {
    fn visit_computed_member_expression(&mut self, expression: &ComputedMemberExpression<'a>) {
        if expression.static_property_name().is_none()
            && let Some(span) = literal_require_span(&expression.object)
        {
            self.calls.insert(span);
        }
        walk::walk_computed_member_expression(self, expression);
    }

    fn visit_variable_declarator(&mut self, declaration: &VariableDeclarator<'a>) {
        if binding_has_dynamic_property(&declaration.id)
            && let Some(initializer) = &declaration.init
            && let Some(span) = literal_require_span(initializer)
        {
            self.calls.insert(span);
        }
        walk::walk_variable_declarator(self, declaration);
    }

    fn visit_assignment_pattern(&mut self, pattern: &AssignmentPattern<'a>) {
        if binding_has_dynamic_property(&pattern.left)
            && let Some(span) = literal_require_span(&pattern.right)
        {
            self.calls.insert(span);
        }
        walk::walk_assignment_pattern(self, pattern);
    }

    fn visit_assignment_expression(&mut self, expression: &AssignmentExpression<'a>) {
        if assignment_target_has_dynamic_property(&expression.left)
            && let Some(span) = literal_require_span(&expression.right)
        {
            self.calls.insert(span);
        }
        walk::walk_assignment_expression(self, expression);
    }
}

fn literal_require_span(expression: &Expression<'_>) -> Option<SpanKey> {
    let Expression::CallExpression(call) = transparent_runtime_expression(expression) else {
        return None;
    };
    call.common_js_require()?;
    Some((call.span.start, call.span.end))
}

fn binding_has_dynamic_property(pattern: &BindingPattern<'_>) -> bool {
    match pattern {
        BindingPattern::BindingIdentifier(_) => false,
        BindingPattern::ObjectPattern(pattern) => object_pattern_has_dynamic_property(pattern),
        BindingPattern::ArrayPattern(pattern) => array_pattern_has_dynamic_property(pattern),
        BindingPattern::AssignmentPattern(pattern) => binding_has_dynamic_property(&pattern.left),
    }
}

fn object_pattern_has_dynamic_property(pattern: &ObjectPattern<'_>) -> bool {
    pattern.properties.iter().any(|property| {
        (property.computed && property.key.static_name().is_none())
            || binding_has_dynamic_property(&property.value)
    }) || pattern
        .rest
        .as_ref()
        .is_some_and(|rest| binding_has_dynamic_property(&rest.argument))
}

fn array_pattern_has_dynamic_property(pattern: &ArrayPattern<'_>) -> bool {
    pattern
        .elements
        .iter()
        .flatten()
        .any(binding_has_dynamic_property)
        || pattern
            .rest
            .as_ref()
            .is_some_and(|rest| binding_has_dynamic_property(&rest.argument))
}

fn assignment_target_has_dynamic_property(target: &AssignmentTarget<'_>) -> bool {
    match target {
        AssignmentTarget::ObjectAssignmentTarget(pattern) => {
            object_assignment_has_dynamic_property(pattern)
        }
        AssignmentTarget::ArrayAssignmentTarget(pattern) => {
            array_assignment_has_dynamic_property(pattern)
        }
        _ => false,
    }
}

fn object_assignment_has_dynamic_property(pattern: &ObjectAssignmentTarget<'_>) -> bool {
    pattern.properties.iter().any(|property| match property {
        AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(_) => false,
        AssignmentTargetProperty::AssignmentTargetPropertyProperty(property) => {
            (property.computed && property.name.static_name().is_none())
                || assignment_maybe_default_has_dynamic_property(&property.binding)
        }
    }) || pattern
        .rest
        .as_ref()
        .is_some_and(|rest| assignment_target_has_dynamic_property(&rest.target))
}

fn array_assignment_has_dynamic_property(pattern: &ArrayAssignmentTarget<'_>) -> bool {
    pattern
        .elements
        .iter()
        .flatten()
        .any(assignment_maybe_default_has_dynamic_property)
        || pattern
            .rest
            .as_ref()
            .is_some_and(|rest| assignment_target_has_dynamic_property(&rest.target))
}

fn assignment_maybe_default_has_dynamic_property(
    target: &AssignmentTargetMaybeDefault<'_>,
) -> bool {
    match target {
        AssignmentTargetMaybeDefault::ObjectAssignmentTarget(pattern) => {
            object_assignment_has_dynamic_property(pattern)
        }
        AssignmentTargetMaybeDefault::ArrayAssignmentTarget(pattern) => {
            array_assignment_has_dynamic_property(pattern)
        }
        AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(pattern) => {
            assignment_target_has_dynamic_property(&pattern.binding)
        }
        _ => false,
    }
}
