use std::collections::{BTreeMap, BTreeSet};

use oxc_ast::ast::{
    BindingIdentifier, BindingPattern, CallExpression, ClassType, ExportNamedDeclaration,
    Expression, ForStatementLeft, FormalParameters, Function, FunctionType, ImportDeclaration,
    ImportExpression, ImportOrExportKind, ImportSpecifier, MemberExpression, TSEnumDeclaration,
    TSImportEqualsDeclaration, TSInterfaceDeclaration, TSModuleDeclarationName, TSType,
    TSTypeAliasDeclaration, TSTypeParameter, TaggedTemplateExpression, VariableDeclaration,
    VariableDeclarator, WithStatement,
};
use oxc_ast::ast_kind::AstKind;
use oxc_ast_visit::{Visit, walk};
use oxc_span::GetSpan;

use super::{
    DynamicRecord, SpanKey, awaited_literal_import, scope_kind, span_key, then_callback_binding,
    transparent_runtime_expression,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Binding {
    Dynamic(usize),
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ScopeKind {
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
pub(super) struct Scope {
    pub(super) parent: Option<usize>,
    pub(super) kind: ScopeKind,
    pub(super) bindings: BTreeMap<String, Binding>,
    pub(super) dynamic_lookup: bool,
    pub(super) ambient: bool,
    pub(super) mapped_parameter_names: Option<Vec<Option<String>>>,
    mapped_arguments_opaque: bool,
    strict: bool,
}

pub(super) struct BindingCollection {
    pub(super) scopes: Vec<Scope>,
    pub(super) records: Vec<DynamicRecord>,
    pub(super) handled_imports: BTreeSet<SpanKey>,
}

pub(super) fn collect(program: &oxc_ast::ast::Program<'_>) -> Option<BindingCollection> {
    let mut collector = BindingCollector::default();
    collector.visit_program(program);
    if !collector.complete() {
        return None;
    }
    collector.degrade_opaque_mapped_arguments();
    (!collector.tracking_failed).then_some(BindingCollection {
        scopes: collector.scopes,
        records: collector.records,
        handled_imports: collector.handled_imports,
    })
}

#[derive(Default)]
struct BindingCollector {
    scopes: Vec<Scope>,
    scope_stack: Vec<usize>,
    records: Vec<DynamicRecord>,
    handled_imports: BTreeSet<SpanKey>,
    special_bindings: BTreeMap<SpanKey, usize>,
    binding_context: Option<(usize, Binding)>,
    exported_declaration: bool,
    type_only_binding_depth: usize,
    for_binding_assignment: bool,
    tracking_failed: bool,
}

impl BindingCollector {
    fn complete(&self) -> bool {
        !self.tracking_failed
            && self.scope_stack.is_empty()
            && !self.scopes.is_empty()
            && self.type_only_binding_depth == 0
    }

    fn current_scope(&mut self) -> Option<usize> {
        let scope = self.scope_stack.last().copied();
        if scope.is_none() {
            self.tracking_failed = true;
        }
        scope
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

    fn push_scope(
        &mut self,
        kind: ScopeKind,
        dynamic_lookup: bool,
        strict: bool,
        ambient: bool,
        mapped_parameter_names: Option<Vec<Option<String>>>,
    ) {
        let index = self.scopes.len();
        self.scopes.push(Scope {
            parent: self.scope_stack.last().copied(),
            kind,
            bindings: BTreeMap::new(),
            dynamic_lookup,
            strict,
            ambient,
            mapped_parameter_names,
            mapped_arguments_opaque: false,
        });
        self.scope_stack.push(index);
    }

    fn push_inherited_scope(&mut self, kind: ScopeKind, dynamic_lookup: bool) {
        self.push_scope(
            kind,
            dynamic_lookup,
            self.current_strict(),
            self.current_ambient(),
            None,
        );
    }

    fn pop_scope(&mut self) {
        if self.scope_stack.pop().is_none() {
            self.tracking_failed = true;
        }
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

    fn annex_b_function_binding_possible(&self) -> bool {
        !self.current_strict()
            && !self.current_ambient()
            && self
                .scope_stack
                .last()
                .and_then(|scope| self.scopes.get(*scope))
                .is_some_and(|scope| scope.kind == ScopeKind::Lexical)
    }

    fn add_record(
        &mut self,
        specifier: &str,
        import_expression: &ImportExpression<'_>,
        local_name: Option<String>,
    ) -> usize {
        let record = self.records.len();
        self.records
            .push(DynamicRecord::new(specifier, import_expression, local_name));
        self.handled_imports
            .insert(span_key(import_expression.span));
        record
    }

    fn declare(&mut self, scope: usize, name: &str, binding: Binding) {
        let Some(existing) = self
            .scopes
            .get(scope)
            .and_then(|scope| scope.bindings.get(name))
            .copied()
        else {
            if let Some(scope) = self.scopes.get_mut(scope) {
                scope.bindings.insert(name.to_owned(), binding);
            } else {
                self.tracking_failed = true;
            }
            return;
        };
        if existing == binding {
            return;
        }
        self.degrade_binding(existing);
        self.degrade_binding(binding);
        if let Some(scope) = self.scopes.get_mut(scope) {
            scope.bindings.insert(name.to_owned(), Binding::Other);
        } else {
            self.tracking_failed = true;
        }
    }

    fn degrade_binding(&mut self, binding: Binding) {
        if let Binding::Dynamic(record) = binding {
            if let Some(record) = self.records.get_mut(record) {
                record.broad = true;
            } else {
                self.tracking_failed = true;
            }
        }
    }

    fn declare_outer_definition(&mut self, name: &str, annex_b_possible: bool) {
        let Some(current) = self.scope_stack.last().copied() else {
            self.tracking_failed = true;
            return;
        };
        self.declare(current, name, Binding::Other);
        if annex_b_possible
            && let Some(var_scope) = self.nearest_var_environment()
            && var_scope != current
        {
            self.declare(var_scope, name, Binding::Other);
        }
    }

    fn collect_then_binding(&mut self, expression: &CallExpression<'_>) {
        let Some((import_expression, specifier, binding, callback_arguments_are_visible)) =
            then_callback_binding(expression)
        else {
            return;
        };
        let record = self.add_record(specifier, import_expression, Some(binding.name.to_string()));
        if callback_arguments_are_visible {
            self.degrade_binding(Binding::Dynamic(record));
        }
        let key = span_key(binding.span);
        if let Some(previous) = self.special_bindings.insert(key, record) {
            self.degrade_binding(Binding::Dynamic(previous));
            self.degrade_binding(Binding::Dynamic(record));
        }
    }

    fn collect_immediate_member(&mut self, expression: &MemberExpression<'_>) -> Option<usize> {
        let (import_expression, specifier) = awaited_literal_import(expression.object())?;
        let property = expression.static_property_name()?;
        if self
            .handled_imports
            .contains(&span_key(import_expression.span))
        {
            return None;
        }
        let record = self.add_record(specifier, import_expression, None);
        if let Some(record) = self.records.get_mut(record) {
            record.members.insert((
                property.to_owned(),
                expression.span().start,
                expression.span().end,
            ));
        }
        Some(record)
    }

    fn collect_immediate_receiver(&mut self, expression: &Expression<'_>) {
        if let Some(member) = transparent_runtime_expression(expression).get_member_expr()
            && let Some(record) = self.collect_immediate_member(member)
        {
            self.degrade_binding(Binding::Dynamic(record));
        }
    }

    fn enter_type_only_binding(&mut self) {
        self.type_only_binding_depth += 1;
    }

    fn leave_type_only_binding(&mut self) {
        let Some(depth) = self.type_only_binding_depth.checked_sub(1) else {
            self.tracking_failed = true;
            return;
        };
        self.type_only_binding_depth = depth;
    }

    fn mapped_arguments_owner(&self, body_scope: usize) -> Option<usize> {
        let parameter_scope = self.scopes.get(body_scope)?.parent?;
        self.scopes
            .get(parameter_scope)
            .is_some_and(|scope| {
                scope.kind == ScopeKind::FunctionParameters
                    && scope.mapped_parameter_names.is_some()
            })
            .then_some(parameter_scope)
    }

    fn mark_mapped_arguments_opaque(&mut self, body_scope: usize) {
        let Some(owner) = self.mapped_arguments_owner(body_scope) else {
            return;
        };
        if let Some(scope) = self.scopes.get_mut(owner) {
            scope.mapped_arguments_opaque = true;
        } else {
            self.tracking_failed = true;
        }
    }

    fn degrade_opaque_mapped_arguments(&mut self) {
        let mut records = BTreeSet::new();
        for (parameter_scope, scope) in self.scopes.iter().enumerate() {
            if !scope.mapped_arguments_opaque {
                continue;
            }
            let Some(names) = &scope.mapped_parameter_names else {
                self.tracking_failed = true;
                return;
            };
            let Some(body) = self.scopes.iter().find(|candidate| {
                candidate.parent == Some(parameter_scope)
                    && candidate.kind == ScopeKind::VarEnvironment
            }) else {
                self.tracking_failed = true;
                return;
            };
            records.extend(names.iter().flatten().filter_map(
                |name| match body.bindings.get(name) {
                    Some(Binding::Dynamic(record)) => Some(*record),
                    Some(Binding::Other) | None => None,
                },
            ));
        }
        for record in records {
            self.degrade_binding(Binding::Dynamic(record));
        }
    }
}

impl<'a> Visit<'a> for BindingCollector {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        match kind {
            AstKind::Program(program) => {
                self.push_scope(
                    ScopeKind::Root,
                    false,
                    program.source_type.is_strict() || program.has_use_strict_directive(),
                    program.source_type.is_typescript_definition(),
                    None,
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
                let strict = self.current_strict() || function.has_use_strict_directive();
                self.push_scope(
                    ScopeKind::FunctionParameters,
                    false,
                    strict,
                    ambient,
                    mapped_parameter_names(function, strict, ambient),
                );
                return;
            }
            AstKind::ArrowFunctionExpression(function) => {
                self.push_scope(
                    ScopeKind::FunctionParameters,
                    false,
                    self.current_strict() || function.has_use_strict_directive(),
                    self.current_ambient(),
                    None,
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
                self.push_scope(ScopeKind::Lexical, false, true, ambient, None);
                return;
            }
            AstKind::StaticBlock(_) => {
                self.push_scope(
                    ScopeKind::VarEnvironment,
                    false,
                    true,
                    self.current_ambient(),
                    None,
                );
                return;
            }
            AstKind::TSModuleDeclaration(declaration) => {
                let ambient = self.current_ambient() || declaration.declare;
                if !ambient
                    && let TSModuleDeclarationName::Identifier(identifier) = &declaration.id
                    && let Some(scope) = self.current_scope()
                {
                    self.declare(scope, identifier.name.as_str(), Binding::Other);
                }
                self.push_scope(
                    ScopeKind::VarEnvironment,
                    false,
                    self.current_strict() || declaration.has_use_strict_directive(),
                    ambient,
                    None,
                );
                return;
            }
            AstKind::TSGlobalDeclaration(declaration) => {
                self.push_scope(
                    ScopeKind::VarEnvironment,
                    false,
                    self.current_strict() || declaration.body.has_use_strict_directive(),
                    true,
                    None,
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
        if self.type_only_binding_depth > 0 || self.current_ambient() {
            walk::walk_binding_identifier(self, identifier);
            return;
        }
        let binding = self
            .special_bindings
            .get(&span_key(identifier.span))
            .copied()
            .map(Binding::Dynamic)
            .or_else(|| self.binding_context.map(|(_, binding)| binding))
            .unwrap_or(Binding::Other);
        let scope = self
            .binding_context
            .map(|(scope, _)| scope)
            .or_else(|| self.current_scope());
        if let Some(scope) = scope {
            self.declare(scope, identifier.name.as_str(), binding);
        }
        walk::walk_binding_identifier(self, identifier);
    }

    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        let target_scope = if declarator.kind.is_var() {
            self.nearest_var_environment()
        } else {
            self.current_scope()
        };
        let mut binding = Binding::Other;
        if let (Some(_), BindingPattern::BindingIdentifier(identifier), Some(initializer)) =
            (target_scope, &declarator.id, &declarator.init)
            && let Some((import_expression, specifier)) = awaited_literal_import(initializer)
        {
            let record = self.add_record(
                specifier,
                import_expression,
                Some(identifier.name.to_string()),
            );
            if self.exported_declaration {
                self.degrade_binding(Binding::Dynamic(record));
            }
            binding = Binding::Dynamic(record);
        }

        let previous = self.binding_context;
        if let Some(target_scope) = target_scope {
            self.binding_context = Some((target_scope, binding));
        }
        let binds_arguments = declarator
            .id
            .get_binding_identifiers()
            .iter()
            .any(|identifier| identifier.name == "arguments");
        let preserves_implicit_arguments = declarator.kind.is_var()
            && !self.for_binding_assignment
            && declarator.init.is_none()
            && matches!(
                &declarator.id,
                BindingPattern::BindingIdentifier(identifier) if identifier.name == "arguments"
            )
            && target_scope.is_some_and(|scope| self.mapped_arguments_owner(scope).is_some());
        if declarator.kind.is_var()
            && binds_arguments
            && !preserves_implicit_arguments
            && let Some(scope) = target_scope
        {
            self.mark_mapped_arguments_opaque(scope);
        }
        if !preserves_implicit_arguments {
            self.visit_binding_pattern(&declarator.id);
        }
        self.binding_context = previous;
        let exported_declaration = self.exported_declaration;
        self.exported_declaration = false;
        if let Some(type_annotation) = &declarator.type_annotation {
            self.visit_ts_type_annotation(type_annotation);
        }
        if let Some(initializer) = &declarator.init {
            self.visit_expression(initializer);
        }
        self.exported_declaration = exported_declaration;
    }

    fn visit_call_expression(&mut self, expression: &CallExpression<'a>) {
        self.collect_then_binding(expression);
        self.collect_immediate_receiver(&expression.callee);
        walk::walk_call_expression(self, expression);
    }

    fn visit_tagged_template_expression(&mut self, expression: &TaggedTemplateExpression<'a>) {
        self.collect_immediate_receiver(&expression.tag);
        walk::walk_tagged_template_expression(self, expression);
    }

    fn visit_import_declaration(&mut self, declaration: &ImportDeclaration<'a>) {
        if declaration.import_kind == ImportOrExportKind::Type {
            self.enter_type_only_binding();
        }
        walk::walk_import_declaration(self, declaration);
        if declaration.import_kind == ImportOrExportKind::Type {
            self.leave_type_only_binding();
        }
    }

    fn visit_for_statement_left(&mut self, left: &ForStatementLeft<'a>) {
        let previous = self.for_binding_assignment;
        self.for_binding_assignment = true;
        walk::walk_for_statement_left(self, left);
        self.for_binding_assignment = previous;
    }

    fn visit_variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
        if declaration.declare {
            self.enter_type_only_binding();
        }
        walk::walk_variable_declaration(self, declaration);
        if declaration.declare {
            self.leave_type_only_binding();
        }
    }

    fn visit_ts_enum_declaration(&mut self, declaration: &TSEnumDeclaration<'a>) {
        if declaration.declare {
            self.enter_type_only_binding();
        }
        walk::walk_ts_enum_declaration(self, declaration);
        if declaration.declare {
            self.leave_type_only_binding();
        }
    }

    fn visit_import_specifier(&mut self, specifier: &ImportSpecifier<'a>) {
        if specifier.import_kind == ImportOrExportKind::Type {
            self.enter_type_only_binding();
        }
        walk::walk_import_specifier(self, specifier);
        if specifier.import_kind == ImportOrExportKind::Type {
            self.leave_type_only_binding();
        }
    }

    fn visit_ts_import_equals_declaration(&mut self, declaration: &TSImportEqualsDeclaration<'a>) {
        if declaration.import_kind == ImportOrExportKind::Type {
            self.enter_type_only_binding();
        }
        walk::walk_ts_import_equals_declaration(self, declaration);
        if declaration.import_kind == ImportOrExportKind::Type {
            self.leave_type_only_binding();
        }
    }

    fn visit_ts_type_alias_declaration(&mut self, declaration: &TSTypeAliasDeclaration<'a>) {
        self.enter_type_only_binding();
        walk::walk_ts_type_alias_declaration(self, declaration);
        self.leave_type_only_binding();
    }

    fn visit_ts_interface_declaration(&mut self, declaration: &TSInterfaceDeclaration<'a>) {
        self.enter_type_only_binding();
        walk::walk_ts_interface_declaration(self, declaration);
        self.leave_type_only_binding();
    }

    fn visit_ts_type_parameter(&mut self, parameter: &TSTypeParameter<'a>) {
        self.enter_type_only_binding();
        walk::walk_ts_type_parameter(self, parameter);
        self.leave_type_only_binding();
    }

    fn visit_ts_type(&mut self, ty: &TSType<'a>) {
        self.enter_type_only_binding();
        walk::walk_ts_type(self, ty);
        self.leave_type_only_binding();
    }

    fn visit_export_named_declaration(&mut self, declaration: &ExportNamedDeclaration<'a>) {
        let previous = self.exported_declaration;
        self.exported_declaration = declaration.declaration.is_some();
        walk::walk_export_named_declaration(self, declaration);
        self.exported_declaration = previous;
    }

    fn visit_member_expression(&mut self, expression: &MemberExpression<'a>) {
        let _ = self.collect_immediate_member(expression);
        walk::walk_member_expression(self, expression);
    }

    fn visit_with_statement(&mut self, statement: &WithStatement<'a>) {
        self.visit_expression(&statement.object);
        self.push_inherited_scope(ScopeKind::Lexical, true);
        self.visit_statement(&statement.body);
        self.pop_scope();
    }
}

fn mapped_parameter_names(
    function: &Function<'_>,
    strict: bool,
    ambient: bool,
) -> Option<Vec<Option<String>>> {
    if strict || ambient || function.params.rest.is_some() {
        return Some(Vec::new());
    }
    Some(simple_parameter_names(&function.params).unwrap_or_default())
}

fn simple_parameter_names(parameters: &FormalParameters<'_>) -> Option<Vec<Option<String>>> {
    let mut authored_names = Vec::new();
    for parameter in &parameters.items {
        if parameter.initializer.is_some() {
            return None;
        }
        let BindingPattern::BindingIdentifier(identifier) = &parameter.pattern else {
            return None;
        };
        authored_names.push(identifier.name.to_string());
    }
    let mut seen = BTreeSet::new();
    let mut mapped = authored_names
        .into_iter()
        .rev()
        .map(|name| seen.insert(name.clone()).then_some(name))
        .collect::<Vec<_>>();
    mapped.reverse();
    Some(mapped)
}
