use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{ImportKind, Limitation, ResolutionOutcome, ResolvedSourceUse};
use oxc_ast::ast::{
    ArrayAssignmentTarget, ArrayPattern, AssignmentExpression, AssignmentPattern, AssignmentTarget,
    AssignmentTargetMaybeDefault, AssignmentTargetProperty, BindingIdentifier, BindingPattern,
    ClassType, ComputedMemberExpression, Expression, FunctionType, ImportDeclaration,
    ImportOrExportKind, ImportSpecifier, ObjectAssignmentTarget, ObjectPattern, Program,
    TSEnumDeclaration, TSImportEqualsDeclaration, TSInterfaceDeclaration, TSModuleDeclarationName,
    TSType, TSTypeAliasDeclaration, TSTypeParameter, VariableDeclaration, VariableDeclarator,
    WithStatement,
};
use oxc_ast::ast_kind::AstKind;
use oxc_ast_visit::{Visit, walk};

use crate::dynamic_import::transparent_runtime_expression;

type SpanKey = (u32, u32);

pub(super) fn collect_computed_require_calls(program: &Program<'_>) -> BTreeSet<SpanKey> {
    let mut collector = ComputedRequireCollector::default();
    collector.visit_program(program);

    let bindings = RequireBindingCollector::collect(program);
    if !bindings.complete {
        collector.calls.extend(bindings.require_calls);
        return collector.calls;
    }
    let mut bound_collector = BoundComputedRequireCollector::new(&bindings.scopes);
    bound_collector.visit_program(program);
    if bound_collector.complete() {
        collector.calls.extend(bound_collector.calls);
    } else {
        // Scope disagreement must not turn a known require-result binding into clean evidence.
        collector.calls.extend(bindings.require_calls);
    }
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

fn identifier_reference<'a, 'b>(expression: &'b Expression<'a>) -> Option<&'b str> {
    transparent_runtime_expression(expression)
        .get_identifier_reference()
        .map(|identifier| identifier.name.as_str())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScopeKind {
    Root,
    FunctionParameters,
    VarEnvironment,
    Lexical,
}

impl ScopeKind {
    fn is_var_environment(self) -> bool {
        matches!(self, Self::Root | Self::VarEnvironment)
    }
}

#[derive(Debug)]
struct BindingScope {
    parent: Option<usize>,
    kind: ScopeKind,
    bindings: BTreeMap<String, BTreeSet<SpanKey>>,
    dynamic_lookup: bool,
    strict: bool,
    ambient: bool,
}

struct RequireBindings {
    scopes: Vec<BindingScope>,
    require_calls: BTreeSet<SpanKey>,
    complete: bool,
}

#[derive(Default)]
struct RequireBindingCollector {
    scopes: Vec<BindingScope>,
    scope_stack: Vec<usize>,
    require_calls: BTreeSet<SpanKey>,
    binding_context: Option<(usize, Option<SpanKey>)>,
    type_only_depth: usize,
    tracking_failed: bool,
}

impl RequireBindingCollector {
    fn collect(program: &Program<'_>) -> RequireBindings {
        let mut collector = Self::default();
        collector.visit_program(program);
        let complete = collector.complete();
        RequireBindings {
            scopes: collector.scopes,
            require_calls: collector.require_calls,
            complete,
        }
    }

    fn complete(&self) -> bool {
        !self.tracking_failed
            && self.scope_stack.is_empty()
            && !self.scopes.is_empty()
            && self.type_only_depth == 0
    }

    fn current_scope(&mut self) -> Option<usize> {
        let scope = self.scope_stack.last().copied();
        if scope.is_none() {
            self.tracking_failed = true;
        }
        scope
    }

    fn current_strict(&self) -> bool {
        self.scope_stack
            .last()
            .and_then(|scope| self.scopes.get(*scope))
            .is_some_and(|scope| scope.strict)
    }

    fn current_ambient(&self) -> bool {
        self.scope_stack
            .last()
            .and_then(|scope| self.scopes.get(*scope))
            .is_some_and(|scope| scope.ambient)
    }

    fn nearest_var_environment(&mut self) -> Option<usize> {
        let scope = self.scope_stack.iter().rev().find_map(|scope| {
            self.scopes
                .get(*scope)
                .is_some_and(|scope| scope.kind.is_var_environment())
                .then_some(*scope)
        });
        if scope.is_none() {
            self.tracking_failed = true;
        }
        scope
    }

    fn push_scope(&mut self, kind: ScopeKind, dynamic_lookup: bool, strict: bool, ambient: bool) {
        let index = self.scopes.len();
        self.scopes.push(BindingScope {
            parent: self.scope_stack.last().copied(),
            kind,
            bindings: BTreeMap::new(),
            dynamic_lookup,
            strict,
            ambient,
        });
        self.scope_stack.push(index);
    }

    fn push_inherited_scope(&mut self, kind: ScopeKind, dynamic_lookup: bool) {
        self.push_scope(
            kind,
            dynamic_lookup,
            self.current_strict(),
            self.current_ambient(),
        );
    }

    fn pop_scope(&mut self) {
        if self.scope_stack.pop().is_none() {
            self.tracking_failed = true;
        }
    }

    fn declare(&mut self, scope: usize, name: &str, require: Option<SpanKey>) {
        let Some(scope) = self.scopes.get_mut(scope) else {
            self.tracking_failed = true;
            return;
        };
        let binding = scope.bindings.entry(name.to_owned()).or_default();
        if let Some(require) = require {
            binding.insert(require);
            self.require_calls.insert(require);
        }
    }

    fn declare_outer_definition(&mut self, name: &str, annex_b_possible: bool) {
        let Some(current) = self.scope_stack.last().copied() else {
            self.tracking_failed = true;
            return;
        };
        self.declare(current, name, None);
        if annex_b_possible
            && let Some(var_scope) = self.nearest_var_environment()
            && var_scope != current
        {
            self.declare(var_scope, name, None);
        }
    }

    fn annex_b_function_binding_possible(&self) -> bool {
        !self.current_strict()
            && !self.current_ambient()
            && self
                .scope_stack
                .last()
                .and_then(|scope| self.scopes.get(*scope))
                .is_some_and(|scope| scope.kind == ScopeKind::Lexical)
    }

    fn enter_type_only(&mut self) {
        self.type_only_depth += 1;
    }

    fn leave_type_only(&mut self) {
        let Some(depth) = self.type_only_depth.checked_sub(1) else {
            self.tracking_failed = true;
            return;
        };
        self.type_only_depth = depth;
    }
}

impl<'a> Visit<'a> for RequireBindingCollector {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        match kind {
            AstKind::Program(program) => {
                self.push_scope(
                    ScopeKind::Root,
                    false,
                    program.source_type.is_strict() || program.has_use_strict_directive(),
                    program.source_type.is_typescript_definition(),
                );
                return;
            }
            AstKind::Function(function) => {
                let ambient = self.current_ambient()
                    || function.declare
                    || function.r#type.is_typescript_syntax()
                    || function.body.is_none();
                if !ambient
                    && function.r#type == FunctionType::FunctionDeclaration
                    && let Some(identifier) = &function.id
                {
                    self.declare_outer_definition(
                        identifier.name.as_str(),
                        self.annex_b_function_binding_possible(),
                    );
                }
                self.push_scope(
                    ScopeKind::FunctionParameters,
                    false,
                    self.current_strict() || function.has_use_strict_directive(),
                    ambient,
                );
                return;
            }
            AstKind::ArrowFunctionExpression(function) => {
                self.push_scope(
                    ScopeKind::FunctionParameters,
                    false,
                    self.current_strict() || function.has_use_strict_directive(),
                    self.current_ambient(),
                );
                return;
            }
            AstKind::FunctionBody(_) => {
                self.push_inherited_scope(ScopeKind::VarEnvironment, false);
                return;
            }
            AstKind::Class(class) => {
                let ambient = self.current_ambient() || class.declare;
                if !ambient
                    && class.r#type == ClassType::ClassDeclaration
                    && let Some(identifier) = &class.id
                {
                    self.declare_outer_definition(identifier.name.as_str(), false);
                }
                self.push_scope(ScopeKind::Lexical, false, true, ambient);
                return;
            }
            AstKind::StaticBlock(_) => {
                self.push_scope(
                    ScopeKind::VarEnvironment,
                    false,
                    true,
                    self.current_ambient(),
                );
                return;
            }
            AstKind::TSModuleDeclaration(declaration) => {
                let ambient = self.current_ambient() || declaration.declare;
                if !ambient
                    && let TSModuleDeclarationName::Identifier(identifier) = &declaration.id
                    && let Some(scope) = self.current_scope()
                {
                    self.declare(scope, identifier.name.as_str(), None);
                }
                self.push_scope(
                    ScopeKind::VarEnvironment,
                    false,
                    self.current_strict() || declaration.has_use_strict_directive(),
                    ambient,
                );
                return;
            }
            AstKind::TSGlobalDeclaration(declaration) => {
                self.push_scope(
                    ScopeKind::VarEnvironment,
                    false,
                    self.current_strict() || declaration.body.has_use_strict_directive(),
                    true,
                );
                return;
            }
            _ => {}
        }
        if let Some(kind) = scope_kind(kind) {
            self.push_inherited_scope(kind, false);
        }
    }

    fn leave_node(&mut self, kind: AstKind<'a>) {
        if scope_kind(kind).is_some() {
            self.pop_scope();
        }
    }

    fn visit_binding_identifier(&mut self, identifier: &BindingIdentifier<'a>) {
        if self.type_only_depth == 0 && !self.current_ambient() {
            let binding = self.binding_context.and_then(|(_, binding)| binding);
            let scope = self
                .binding_context
                .map(|(scope, _)| scope)
                .or_else(|| self.current_scope());
            if let Some(scope) = scope {
                self.declare(scope, identifier.name.as_str(), binding);
            }
        }
        walk::walk_binding_identifier(self, identifier);
    }

    fn visit_variable_declarator(&mut self, declaration: &VariableDeclarator<'a>) {
        let target_scope = if declaration.kind.is_var() {
            self.nearest_var_environment()
        } else {
            self.current_scope()
        };
        let binding = match (&declaration.id, &declaration.init) {
            (BindingPattern::BindingIdentifier(_), Some(initializer)) => {
                literal_require_span(initializer)
            }
            _ => None,
        };
        let previous = self.binding_context;
        if let Some(target_scope) = target_scope {
            self.binding_context = Some((target_scope, binding));
        }
        self.visit_binding_pattern(&declaration.id);
        self.binding_context = previous;
        if let Some(type_annotation) = &declaration.type_annotation {
            self.visit_ts_type_annotation(type_annotation);
        }
        if let Some(initializer) = &declaration.init {
            self.visit_expression(initializer);
        }
    }

    fn visit_import_declaration(&mut self, declaration: &ImportDeclaration<'a>) {
        if declaration.import_kind == ImportOrExportKind::Type {
            self.enter_type_only();
        }
        walk::walk_import_declaration(self, declaration);
        if declaration.import_kind == ImportOrExportKind::Type {
            self.leave_type_only();
        }
    }

    fn visit_import_specifier(&mut self, specifier: &ImportSpecifier<'a>) {
        if specifier.import_kind == ImportOrExportKind::Type {
            self.enter_type_only();
        }
        walk::walk_import_specifier(self, specifier);
        if specifier.import_kind == ImportOrExportKind::Type {
            self.leave_type_only();
        }
    }

    fn visit_ts_import_equals_declaration(&mut self, declaration: &TSImportEqualsDeclaration<'a>) {
        if declaration.import_kind == ImportOrExportKind::Type {
            self.enter_type_only();
        }
        walk::walk_ts_import_equals_declaration(self, declaration);
        if declaration.import_kind == ImportOrExportKind::Type {
            self.leave_type_only();
        }
    }

    fn visit_variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
        if declaration.declare {
            self.enter_type_only();
        }
        walk::walk_variable_declaration(self, declaration);
        if declaration.declare {
            self.leave_type_only();
        }
    }

    fn visit_ts_enum_declaration(&mut self, declaration: &TSEnumDeclaration<'a>) {
        if declaration.declare {
            self.enter_type_only();
        }
        walk::walk_ts_enum_declaration(self, declaration);
        if declaration.declare {
            self.leave_type_only();
        }
    }

    fn visit_ts_type_alias_declaration(&mut self, declaration: &TSTypeAliasDeclaration<'a>) {
        self.enter_type_only();
        walk::walk_ts_type_alias_declaration(self, declaration);
        self.leave_type_only();
    }

    fn visit_ts_interface_declaration(&mut self, declaration: &TSInterfaceDeclaration<'a>) {
        self.enter_type_only();
        walk::walk_ts_interface_declaration(self, declaration);
        self.leave_type_only();
    }

    fn visit_ts_type_parameter(&mut self, parameter: &TSTypeParameter<'a>) {
        self.enter_type_only();
        walk::walk_ts_type_parameter(self, parameter);
        self.leave_type_only();
    }

    fn visit_ts_type(&mut self, ty: &TSType<'a>) {
        self.enter_type_only();
        walk::walk_ts_type(self, ty);
        self.leave_type_only();
    }

    fn visit_with_statement(&mut self, statement: &WithStatement<'a>) {
        self.visit_expression(&statement.object);
        self.push_inherited_scope(ScopeKind::Lexical, true);
        self.visit_statement(&statement.body);
        self.pop_scope();
    }
}

struct BoundComputedRequireCollector<'m> {
    scopes: &'m [BindingScope],
    scope_stack: Vec<usize>,
    next_scope: usize,
    calls: BTreeSet<SpanKey>,
    tracking_failed: bool,
}

impl<'m> BoundComputedRequireCollector<'m> {
    fn new(scopes: &'m [BindingScope]) -> Self {
        Self {
            scopes,
            scope_stack: Vec::new(),
            next_scope: 0,
            calls: BTreeSet::new(),
            tracking_failed: false,
        }
    }

    fn complete(&self) -> bool {
        !self.tracking_failed && self.scope_stack.is_empty() && self.next_scope == self.scopes.len()
    }

    fn push_next_scope(&mut self, dynamic_lookup: bool) {
        let Some(scope) = self.scopes.get(self.next_scope) else {
            self.tracking_failed = true;
            return;
        };
        if scope.dynamic_lookup != dynamic_lookup
            || scope.parent != self.scope_stack.last().copied()
        {
            self.tracking_failed = true;
            return;
        }
        self.scope_stack.push(self.next_scope);
        self.next_scope += 1;
    }

    fn pop_scope(&mut self) {
        if self.scope_stack.pop().is_none() {
            self.tracking_failed = true;
        }
    }

    fn mark_identifier(&mut self, expression: &Expression<'_>) {
        let Some(name) = identifier_reference(expression) else {
            return;
        };
        let Some(mut cursor) = self.scope_stack.last().copied() else {
            self.tracking_failed = true;
            return;
        };
        loop {
            let Some(scope) = self.scopes.get(cursor) else {
                self.tracking_failed = true;
                return;
            };
            if let Some(binding) = scope.bindings.get(name) {
                self.calls.extend(binding);
                return;
            }
            let Some(parent) = scope.parent else {
                return;
            };
            cursor = parent;
        }
    }
}

impl<'a> Visit<'a> for BoundComputedRequireCollector<'_> {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        if scope_kind(kind).is_some() {
            self.push_next_scope(false);
        }
    }

    fn leave_node(&mut self, kind: AstKind<'a>) {
        if scope_kind(kind).is_some() {
            self.pop_scope();
        }
    }

    fn visit_computed_member_expression(&mut self, expression: &ComputedMemberExpression<'a>) {
        if expression.static_property_name().is_none() {
            self.mark_identifier(&expression.object);
        }
        walk::walk_computed_member_expression(self, expression);
    }

    fn visit_variable_declarator(&mut self, declaration: &VariableDeclarator<'a>) {
        if binding_has_dynamic_property(&declaration.id)
            && let Some(initializer) = &declaration.init
        {
            self.mark_identifier(initializer);
        }
        walk::walk_variable_declarator(self, declaration);
    }

    fn visit_assignment_pattern(&mut self, pattern: &AssignmentPattern<'a>) {
        if binding_has_dynamic_property(&pattern.left) {
            self.mark_identifier(&pattern.right);
        }
        walk::walk_assignment_pattern(self, pattern);
    }

    fn visit_assignment_expression(&mut self, expression: &AssignmentExpression<'a>) {
        if assignment_target_has_dynamic_property(&expression.left) {
            self.mark_identifier(&expression.right);
        }
        walk::walk_assignment_expression(self, expression);
    }

    fn visit_with_statement(&mut self, statement: &WithStatement<'a>) {
        self.visit_expression(&statement.object);
        self.push_next_scope(true);
        self.visit_statement(&statement.body);
        self.pop_scope();
    }
}

fn scope_kind(kind: AstKind<'_>) -> Option<ScopeKind> {
    match kind {
        AstKind::Program(_) => Some(ScopeKind::Root),
        AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) => {
            Some(ScopeKind::FunctionParameters)
        }
        AstKind::FunctionBody(_)
        | AstKind::StaticBlock(_)
        | AstKind::TSModuleDeclaration(_)
        | AstKind::TSGlobalDeclaration(_) => Some(ScopeKind::VarEnvironment),
        AstKind::BlockStatement(_)
        | AstKind::ForStatement(_)
        | AstKind::ForInStatement(_)
        | AstKind::ForOfStatement(_)
        | AstKind::SwitchStatement(_)
        | AstKind::CatchClause(_)
        | AstKind::Class(_) => Some(ScopeKind::Lexical),
        _ => None,
    }
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
