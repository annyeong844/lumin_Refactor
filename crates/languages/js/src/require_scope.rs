use oxc_ast::ast::{
    BindingPattern, FormalParameters, ImportDeclarationSpecifier, ImportOrExportKind,
};
use oxc_ast::ast_kind::AstKind;
use oxc_ast_visit::{Visit, walk};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrackedName {
    Require,
    Eval,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct NameBindings {
    require: bool,
    eval: bool,
}

impl NameBindings {
    fn mark(&mut self, name: TrackedName) {
        match name {
            TrackedName::Require => self.require = true,
            TrackedName::Eval => self.eval = true,
        }
    }

    fn contains(self, name: TrackedName) -> bool {
        match name {
            TrackedName::Require => self.require,
            TrackedName::Eval => self.eval,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequireScopeKind {
    Root,
    FunctionParameters,
    VarEnvironment,
    Lexical,
}

impl RequireScopeKind {
    fn is_var_environment(self) -> bool {
        matches!(self, Self::Root | Self::VarEnvironment)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RequireScope {
    parent: Option<usize>,
    kind: RequireScopeKind,
    bindings: NameBindings,
    dynamic_lookup: bool,
    strict: bool,
    ambient: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NameResolution {
    Bound,
    Unbound,
    Dynamic,
}

#[derive(Debug)]
struct RequireScopeModel {
    scopes: Vec<RequireScope>,
    implicit_require_written: bool,
}

impl RequireScopeModel {
    fn resolve_name(&self, scope: usize, name: TrackedName) -> NameResolution {
        let mut cursor = Some(scope);
        let mut dynamic = false;
        while let Some(index) = cursor {
            let Some(current) = self.scopes.get(index) else {
                return NameResolution::Dynamic;
            };
            if current.bindings.contains(name) {
                return if dynamic {
                    NameResolution::Dynamic
                } else {
                    NameResolution::Bound
                };
            }
            dynamic |= current.dynamic_lookup;
            cursor = current.parent;
        }
        if dynamic {
            NameResolution::Dynamic
        } else {
            NameResolution::Unbound
        }
    }

    fn has_binding(&self, scope: usize, name: TrackedName) -> bool {
        let mut cursor = Some(scope);
        while let Some(index) = cursor {
            let Some(current) = self.scopes.get(index) else {
                return false;
            };
            if current.bindings.contains(name) {
                return true;
            }
            cursor = current.parent;
        }
        false
    }

    fn require_is_opaque(&self, scope: usize) -> bool {
        self.implicit_require_written
            || self.resolve_name(scope, TrackedName::Require) != NameResolution::Unbound
    }
}

pub(super) struct RequireScopeTracker {
    model: RequireScopeModel,
    scope_stack: Vec<usize>,
    next_scope: usize,
    tracking_failed: bool,
}

impl RequireScopeTracker {
    pub(super) fn analyze(program: &oxc_ast::ast::Program<'_>) -> Self {
        let mut collector = RequireScopeCollector::default();
        collector.visit_program(program);
        Self {
            model: collector.into_model(),
            scope_stack: Vec::new(),
            next_scope: 0,
            tracking_failed: false,
        }
    }

    pub(super) fn enter_node(&mut self, kind: AstKind<'_>) {
        if scope_kind(kind).is_some() {
            if self.next_scope < self.model.scopes.len() {
                self.scope_stack.push(self.next_scope);
                self.next_scope += 1;
            } else {
                self.tracking_failed = true;
            }
        }
    }

    pub(super) fn leave_node(&mut self, kind: AstKind<'_>) {
        if scope_kind(kind).is_some() && self.scope_stack.pop().is_none() {
            self.tracking_failed = true;
        }
    }

    pub(super) fn require_is_opaque(&self) -> bool {
        self.tracking_failed
            || self
                .scope_stack
                .last()
                .is_none_or(|scope| self.model.require_is_opaque(*scope))
    }
}

#[derive(Default)]
struct RequireScopeCollector {
    scopes: Vec<RequireScope>,
    scope_stack: Vec<usize>,
    require_write_scopes: Vec<usize>,
    eval_call_scopes: Vec<usize>,
}

impl RequireScopeCollector {
    fn push_scope(
        &mut self,
        kind: RequireScopeKind,
        bindings: NameBindings,
        dynamic_lookup: bool,
        strict: bool,
        ambient: bool,
    ) {
        let index = self.scopes.len();
        self.scopes.push(RequireScope {
            parent: self.scope_stack.last().copied(),
            kind,
            bindings,
            dynamic_lookup,
            strict,
            ambient,
        });
        self.scope_stack.push(index);
    }

    fn push_inherited_scope(
        &mut self,
        kind: RequireScopeKind,
        bindings: NameBindings,
        dynamic_lookup: bool,
    ) {
        self.push_scope(
            kind,
            bindings,
            dynamic_lookup,
            self.current_strict(),
            self.current_ambient(),
        );
    }

    fn pop_scope(&mut self) {
        self.scope_stack.pop();
    }

    fn current_scope(&self) -> Option<&RequireScope> {
        self.scope_stack
            .last()
            .and_then(|index| self.scopes.get(*index))
    }

    fn current_strict(&self) -> bool {
        self.current_scope().is_some_and(|scope| scope.strict)
    }

    fn current_ambient(&self) -> bool {
        self.current_scope().is_some_and(|scope| scope.ambient)
    }

    fn mark_current_scope(&mut self, name: TrackedName) {
        if let Some(scope) = self
            .scope_stack
            .last()
            .and_then(|index| self.scopes.get_mut(*index))
        {
            scope.bindings.mark(name);
        }
    }

    fn mark_nearest_var_environment(&mut self, name: TrackedName) {
        let scope = self.scope_stack.iter().rev().find_map(|index| {
            self.scopes
                .get(*index)
                .filter(|scope| scope.kind.is_var_environment())
                .map(|_| *index)
        });
        if let Some(scope) = scope.and_then(|index| self.scopes.get_mut(index)) {
            scope.bindings.mark(name);
        }
    }

    fn mark_function_declaration(&mut self, name: TrackedName) {
        let annex_b_binding = !self.current_strict()
            && self
                .current_scope()
                .is_some_and(|scope| scope.kind == RequireScopeKind::Lexical);
        self.mark_current_scope(name);
        if annex_b_binding {
            self.mark_nearest_var_environment(name);
        }
    }

    fn record_require_write(&mut self) {
        if !self.current_ambient()
            && let Some(scope) = self.scope_stack.last().copied()
        {
            self.require_write_scopes.push(scope);
        }
    }

    fn record_eval_call(&mut self) {
        if !self.current_ambient()
            && let Some(scope) = self.scope_stack.last().copied()
        {
            self.eval_call_scopes.push(scope);
        }
    }

    fn into_model(self) -> RequireScopeModel {
        let mut model = RequireScopeModel {
            scopes: self.scopes,
            implicit_require_written: false,
        };
        let implicit_write = self
            .require_write_scopes
            .iter()
            .any(|scope| !model.has_binding(*scope, TrackedName::Require));
        let intrinsic_eval_can_reach_implicit_require = self.eval_call_scopes.iter().any(|scope| {
            model.resolve_name(*scope, TrackedName::Eval) != NameResolution::Bound
                && !model.has_binding(*scope, TrackedName::Require)
        });
        model.implicit_require_written =
            implicit_write || intrinsic_eval_can_reach_implicit_require;
        model
    }
}

fn tracked_name(name: &str) -> Option<TrackedName> {
    match name {
        "require" => Some(TrackedName::Require),
        "eval" => Some(TrackedName::Eval),
        _ => None,
    }
}

fn pattern_bindings(pattern: &BindingPattern<'_>) -> NameBindings {
    let mut bindings = NameBindings::default();
    for identifier in pattern.get_binding_identifiers() {
        if let Some(name) = tracked_name(identifier.name.as_str()) {
            bindings.mark(name);
        }
    }
    bindings
}

fn parameters_bindings(parameters: &FormalParameters<'_>) -> NameBindings {
    let mut bindings = NameBindings::default();
    for parameter in &parameters.items {
        for name in [TrackedName::Require, TrackedName::Eval] {
            if pattern_bindings(&parameter.pattern).contains(name) {
                bindings.mark(name);
            }
        }
    }
    if let Some(rest) = &parameters.rest {
        let rest_bindings = pattern_bindings(&rest.rest.argument);
        for name in [TrackedName::Require, TrackedName::Eval] {
            if rest_bindings.contains(name) {
                bindings.mark(name);
            }
        }
    }
    bindings
}

fn scope_kind(kind: AstKind<'_>) -> Option<RequireScopeKind> {
    match kind {
        AstKind::Program(_) => Some(RequireScopeKind::Root),
        AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) => {
            Some(RequireScopeKind::FunctionParameters)
        }
        AstKind::FunctionBody(_)
        | AstKind::StaticBlock(_)
        | AstKind::TSModuleDeclaration(_)
        | AstKind::TSGlobalDeclaration(_) => Some(RequireScopeKind::VarEnvironment),
        AstKind::BlockStatement(_)
        | AstKind::ForStatement(_)
        | AstKind::ForInStatement(_)
        | AstKind::ForOfStatement(_)
        | AstKind::WithStatement(_)
        | AstKind::SwitchStatement(_)
        | AstKind::CatchClause(_)
        | AstKind::Class(_) => Some(RequireScopeKind::Lexical),
        _ => None,
    }
}

impl<'a> Visit<'a> for RequireScopeCollector {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        match kind {
            AstKind::Program(program) => {
                self.push_scope(
                    RequireScopeKind::Root,
                    NameBindings::default(),
                    false,
                    program.source_type.is_strict() || program.has_use_strict_directive(),
                    program.source_type.is_typescript_definition(),
                );
                return;
            }
            AstKind::Function(function) => {
                let ambient = self.current_ambient()
                    || function.declare
                    || function.r#type.is_typescript_syntax();
                if !ambient
                    && function.r#type == oxc_ast::ast::FunctionType::FunctionDeclaration
                    && let Some(name) = function
                        .id
                        .as_ref()
                        .and_then(|identifier| tracked_name(identifier.name.as_str()))
                {
                    self.mark_function_declaration(name);
                }
                let mut bindings = if ambient {
                    NameBindings::default()
                } else {
                    parameters_bindings(&function.params)
                };
                if !ambient
                    && function.r#type == oxc_ast::ast::FunctionType::FunctionExpression
                    && let Some(name) = function
                        .id
                        .as_ref()
                        .and_then(|identifier| tracked_name(identifier.name.as_str()))
                {
                    bindings.mark(name);
                }
                self.push_scope(
                    RequireScopeKind::FunctionParameters,
                    bindings,
                    false,
                    self.current_strict() || function.has_use_strict_directive(),
                    ambient,
                );
                return;
            }
            AstKind::ArrowFunctionExpression(function) => {
                let ambient = self.current_ambient();
                self.push_scope(
                    RequireScopeKind::FunctionParameters,
                    if ambient {
                        NameBindings::default()
                    } else {
                        parameters_bindings(&function.params)
                    },
                    false,
                    self.current_strict() || function.has_use_strict_directive(),
                    ambient,
                );
                return;
            }
            AstKind::FunctionBody(_) => {
                self.push_inherited_scope(
                    RequireScopeKind::VarEnvironment,
                    NameBindings::default(),
                    false,
                );
                return;
            }
            AstKind::Class(class) => {
                let ambient = self.current_ambient() || class.declare;
                let name = class
                    .id
                    .as_ref()
                    .and_then(|identifier| tracked_name(identifier.name.as_str()));
                if !ambient
                    && class.r#type == oxc_ast::ast::ClassType::ClassDeclaration
                    && let Some(name) = name
                {
                    self.mark_current_scope(name);
                }
                let mut bindings = NameBindings::default();
                if !ambient && let Some(name) = name {
                    bindings.mark(name);
                }
                self.push_scope(RequireScopeKind::Lexical, bindings, false, true, ambient);
                return;
            }
            AstKind::CatchClause(catch) => {
                let ambient = self.current_ambient();
                self.push_scope(
                    RequireScopeKind::Lexical,
                    if ambient {
                        NameBindings::default()
                    } else {
                        catch
                            .param
                            .as_ref()
                            .map_or_else(NameBindings::default, |parameter| {
                                pattern_bindings(&parameter.pattern)
                            })
                    },
                    false,
                    self.current_strict(),
                    ambient,
                );
                return;
            }
            AstKind::WithStatement(_) => {
                self.push_inherited_scope(RequireScopeKind::Lexical, NameBindings::default(), true);
                return;
            }
            AstKind::StaticBlock(_) => {
                self.push_scope(
                    RequireScopeKind::VarEnvironment,
                    NameBindings::default(),
                    false,
                    true,
                    self.current_ambient(),
                );
                return;
            }
            AstKind::VariableDeclaration(declaration) => {
                if !self.current_ambient() && !declaration.declare {
                    for declarator in &declaration.declarations {
                        let bindings = pattern_bindings(&declarator.id);
                        for name in [TrackedName::Require, TrackedName::Eval] {
                            if bindings.contains(name) {
                                if declaration.kind.is_var() {
                                    self.mark_nearest_var_environment(name);
                                } else {
                                    self.mark_current_scope(name);
                                }
                            }
                        }
                    }
                }
            }
            AstKind::ImportDeclaration(declaration) => {
                if !self.current_ambient()
                    && declaration.import_kind == ImportOrExportKind::Value
                    && let Some(specifiers) = &declaration.specifiers
                {
                    for specifier in specifiers {
                        let local = match specifier {
                            ImportDeclarationSpecifier::ImportSpecifier(import)
                                if import.import_kind == ImportOrExportKind::Value =>
                            {
                                Some(&import.local)
                            }
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(import) => {
                                Some(&import.local)
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(import) => {
                                Some(&import.local)
                            }
                            ImportDeclarationSpecifier::ImportSpecifier(_) => None,
                        };
                        if let Some(name) =
                            local.and_then(|identifier| tracked_name(identifier.name.as_str()))
                        {
                            self.mark_current_scope(name);
                        }
                    }
                }
            }
            AstKind::TSEnumDeclaration(declaration) => {
                if !self.current_ambient()
                    && !declaration.declare
                    && let Some(name) = tracked_name(declaration.id.name.as_str())
                {
                    self.mark_current_scope(name);
                }
            }
            AstKind::TSImportEqualsDeclaration(declaration) => {
                if !self.current_ambient()
                    && declaration.import_kind == ImportOrExportKind::Value
                    && let Some(name) = tracked_name(declaration.id.name.as_str())
                {
                    self.mark_current_scope(name);
                }
            }
            AstKind::TSModuleDeclaration(declaration) => {
                let ambient = self.current_ambient() || declaration.declare;
                if !ambient
                    && let oxc_ast::ast::TSModuleDeclarationName::Identifier(identifier) =
                        &declaration.id
                    && let Some(name) = tracked_name(identifier.name.as_str())
                {
                    self.mark_current_scope(name);
                }
                self.push_scope(
                    RequireScopeKind::VarEnvironment,
                    NameBindings::default(),
                    false,
                    self.current_strict() || declaration.has_use_strict_directive(),
                    ambient,
                );
                return;
            }
            AstKind::TSGlobalDeclaration(declaration) => {
                self.push_scope(
                    RequireScopeKind::VarEnvironment,
                    NameBindings::default(),
                    false,
                    self.current_strict() || declaration.body.has_use_strict_directive(),
                    true,
                );
                return;
            }
            _ => {}
        }
        if let Some(kind) = scope_kind(kind) {
            self.push_inherited_scope(kind, NameBindings::default(), false);
        }
    }

    fn leave_node(&mut self, kind: AstKind<'a>) {
        if scope_kind(kind).is_some() {
            self.pop_scope();
        }
    }

    fn visit_simple_assignment_target(
        &mut self,
        target: &oxc_ast::ast::SimpleAssignmentTarget<'a>,
    ) {
        let direct_write = matches!(
            target,
            oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier)
                if identifier.name == "require"
        );
        let wrapped_write = target
            .get_expression()
            .is_some_and(|expression| expression.is_specific_id("require"));
        if direct_write || wrapped_write {
            self.record_require_write();
        }
        walk::walk_simple_assignment_target(self, target);
    }

    fn visit_assignment_target_property_identifier(
        &mut self,
        property: &oxc_ast::ast::AssignmentTargetPropertyIdentifier<'a>,
    ) {
        if property.binding.name == "require" {
            self.record_require_write();
        }
        walk::walk_assignment_target_property_identifier(self, property);
    }

    fn visit_call_expression(&mut self, expression: &oxc_ast::ast::CallExpression<'a>) {
        if !expression.optional && expression.callee.is_specific_id("eval") {
            self.record_eval_call();
        }
        walk::walk_call_expression(self, expression);
    }
}
