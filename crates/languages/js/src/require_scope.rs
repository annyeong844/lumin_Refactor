use std::collections::BTreeSet;

use oxc_ast::ast::{
    BindingPattern, FormalParameters, ImportDeclarationSpecifier, ImportOrExportKind, Statement,
};
use oxc_ast::ast_kind::AstKind;
use oxc_ast_visit::{Visit, walk};
use oxc_span::GetSpan;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrackedName {
    Require,
    Eval,
    Module,
    Exports,
}

const TRACKED_NAMES: [TrackedName; 4] = [
    TrackedName::Require,
    TrackedName::Eval,
    TrackedName::Module,
    TrackedName::Exports,
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct NameBindings {
    require: bool,
    eval: bool,
    module: bool,
    exports: bool,
}

impl NameBindings {
    fn mark(&mut self, name: TrackedName) {
        match name {
            TrackedName::Require => self.require = true,
            TrackedName::Eval => self.eval = true,
            TrackedName::Module => self.module = true,
            TrackedName::Exports => self.exports = true,
        }
    }

    fn contains(self, name: TrackedName) -> bool {
        match name {
            TrackedName::Require => self.require,
            TrackedName::Eval => self.eval,
            TrackedName::Module => self.module,
            TrackedName::Exports => self.exports,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MutationObservation {
    scope: usize,
    timing: MutationTiming,
    ordered_root_statement: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum MutationTiming {
    After(u32),
    Destructuring {
        target_position: u32,
        rhs_start: u32,
        rhs_end: u32,
    },
}

impl MutationTiming {
    fn precedes_call(self, call_position: u32) -> bool {
        match self {
            Self::After(position) => position < call_position,
            Self::Destructuring {
                target_position,
                rhs_start,
                rhs_end,
            } => !(rhs_start..rhs_end).contains(&call_position) && target_position < call_position,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AssignmentWriteContext {
    AfterExpression(u32),
    Destructuring {
        lhs_start: u32,
        lhs_end: u32,
        rhs_start: u32,
        rhs_end: u32,
    },
}

#[derive(Debug)]
struct RequireScopeModel {
    scopes: Vec<RequireScope>,
    unordered_implicit_require_write: bool,
    ordered_root_require_writes: Vec<MutationTiming>,
    unattributed_require_escape: bool,
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

    fn require_is_opaque(&self, scope: usize, call_position: u32) -> bool {
        if self.resolve_name(scope, TrackedName::Require) != NameResolution::Unbound
            || self.unordered_implicit_require_write
        {
            return true;
        }
        if self.ordered_root_require_writes.is_empty() {
            return false;
        }
        if scope != 0 {
            return true;
        }
        self.ordered_root_require_writes
            .iter()
            .any(|timing| timing.precedes_call(call_position))
    }

    fn may_resolve_to_wrapper(&self, scope: usize, name: TrackedName) -> bool {
        self.resolve_name(scope, name) != NameResolution::Bound
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
            self.enter_next_scope(false);
        }
    }

    pub(super) fn leave_node(&mut self, kind: AstKind<'_>) {
        if scope_kind(kind).is_some() && self.scope_stack.pop().is_none() {
            self.tracking_failed = true;
        }
    }

    pub(super) fn enter_with_body(&mut self) {
        self.enter_next_scope(true);
    }

    pub(super) fn leave_with_body(&mut self) {
        if self.scope_stack.pop().is_none() {
            self.tracking_failed = true;
        }
    }

    pub(super) fn require_is_opaque(&self, call_position: u32) -> bool {
        self.tracking_failed
            || self
                .scope_stack
                .last()
                .is_none_or(|scope| self.model.require_is_opaque(*scope, call_position))
    }

    pub(super) fn has_unattributed_require_escape(&self) -> bool {
        self.model.unattributed_require_escape
    }

    pub(super) fn module_may_be_wrapper(&self) -> bool {
        self.wrapper_name_may_be_implicit(TrackedName::Module)
    }

    pub(super) fn exports_may_be_wrapper(&self) -> bool {
        self.wrapper_name_may_be_implicit(TrackedName::Exports)
    }

    fn wrapper_name_may_be_implicit(&self, name: TrackedName) -> bool {
        self.tracking_failed
            || self
                .scope_stack
                .last()
                .is_none_or(|scope| self.model.may_resolve_to_wrapper(*scope, name))
    }

    fn enter_next_scope(&mut self, require_dynamic: bool) {
        let Some(scope) = self.model.scopes.get(self.next_scope) else {
            self.tracking_failed = true;
            return;
        };
        if require_dynamic && !scope.dynamic_lookup {
            self.tracking_failed = true;
            return;
        }
        self.scope_stack.push(self.next_scope);
        self.next_scope += 1;
    }
}

#[derive(Default)]
struct RequireScopeCollector {
    scopes: Vec<RequireScope>,
    scope_stack: Vec<usize>,
    require_writes: Vec<MutationObservation>,
    eval_calls: Vec<MutationObservation>,
    require_reference_scopes: Vec<(usize, u32, u32)>,
    direct_require_callees: BTreeSet<(u32, u32)>,
    non_escaping_require_references: BTreeSet<(u32, u32)>,
    ordered_root_statement_spans: BTreeSet<(u32, u32)>,
    inside_ordered_root_statement: bool,
    assignment_write_context: Option<AssignmentWriteContext>,
    destructuring_target_position: Option<u32>,
    binding_write_context: Option<AssignmentWriteContext>,
    binding_target_position: Option<u32>,
    root_has_commonjs_wrapper: bool,
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

    fn nearest_var_environment_is_root(&self) -> bool {
        self.scope_stack.iter().rev().find_map(|index| {
            self.scopes
                .get(*index)
                .filter(|scope| scope.kind.is_var_environment())
                .map(|scope| scope.kind == RequireScopeKind::Root)
        }) == Some(true)
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

    fn record_require_write(&mut self, timing: MutationTiming) {
        if !self.current_ambient()
            && let Some(scope) = self.scope_stack.last().copied()
        {
            self.require_writes.push(MutationObservation {
                scope,
                timing,
                ordered_root_statement: scope == 0 && self.inside_ordered_root_statement,
            });
        }
    }

    fn record_eval_call(&mut self, timing: MutationTiming) {
        if !self.current_ambient()
            && let Some(scope) = self.scope_stack.last().copied()
        {
            self.eval_calls.push(MutationObservation {
                scope,
                timing,
                ordered_root_statement: scope == 0 && self.inside_ordered_root_statement,
            });
        }
    }

    fn assignment_target_timing(&self, fallback_position: u32) -> MutationTiming {
        Self::target_timing(
            self.assignment_write_context,
            self.destructuring_target_position,
            fallback_position,
        )
    }

    fn binding_target_timing(&self, fallback_position: u32) -> MutationTiming {
        Self::target_timing(
            self.binding_write_context,
            self.binding_target_position,
            fallback_position,
        )
    }

    fn target_timing(
        context: Option<AssignmentWriteContext>,
        target_position: Option<u32>,
        fallback_position: u32,
    ) -> MutationTiming {
        match context {
            Some(AssignmentWriteContext::AfterExpression(position)) => {
                MutationTiming::After(position)
            }
            Some(AssignmentWriteContext::Destructuring {
                lhs_start,
                lhs_end,
                rhs_start,
                rhs_end,
            }) if (lhs_start..lhs_end).contains(&fallback_position) => {
                MutationTiming::Destructuring {
                    target_position: target_position.unwrap_or(fallback_position),
                    rhs_start,
                    rhs_end,
                }
            }
            Some(AssignmentWriteContext::Destructuring { .. }) => {
                MutationTiming::After(fallback_position)
            }
            None => MutationTiming::After(fallback_position),
        }
    }

    fn expression_mutation_timing(&self, position: u32) -> MutationTiming {
        match self.assignment_write_context {
            Some(AssignmentWriteContext::Destructuring {
                lhs_start,
                lhs_end,
                rhs_start,
                rhs_end,
            }) if (lhs_start..lhs_end).contains(&position) => MutationTiming::Destructuring {
                target_position: position,
                rhs_start,
                rhs_end,
            },
            _ => MutationTiming::After(position),
        }
    }

    fn into_model(self) -> RequireScopeModel {
        let mut model = RequireScopeModel {
            scopes: self.scopes,
            unordered_implicit_require_write: false,
            ordered_root_require_writes: Vec::new(),
            unattributed_require_escape: false,
        };
        let mut implicit_mutations = self
            .require_writes
            .iter()
            .copied()
            .filter(|observation| !model.has_binding(observation.scope, TrackedName::Require))
            .collect::<Vec<_>>();
        let intrinsic_eval_mutations = self
            .eval_calls
            .iter()
            .copied()
            .filter(|observation| {
                model.resolve_name(observation.scope, TrackedName::Eval) != NameResolution::Bound
                    && !model.has_binding(observation.scope, TrackedName::Require)
            })
            .collect::<Vec<_>>();
        implicit_mutations.extend(intrinsic_eval_mutations.iter().copied());
        for observation in implicit_mutations {
            if observation.ordered_root_statement {
                model.ordered_root_require_writes.push(observation.timing);
            } else {
                model.unordered_implicit_require_write = true;
            }
        }
        model.ordered_root_require_writes.sort_unstable();
        model.ordered_root_require_writes.dedup();

        let escaped_reference = self
            .require_reference_scopes
            .iter()
            .any(|(scope, start, end)| {
                !self.direct_require_callees.contains(&(*start, *end))
                    && !self
                        .non_escaping_require_references
                        .contains(&(*start, *end))
                    && model.resolve_name(*scope, TrackedName::Require) != NameResolution::Bound
            });
        model.unattributed_require_escape =
            escaped_reference || !intrinsic_eval_mutations.is_empty();
        model
    }
}

fn tracked_name(name: &str) -> Option<TrackedName> {
    match name {
        "require" => Some(TrackedName::Require),
        "eval" => Some(TrackedName::Eval),
        "module" => Some(TrackedName::Module),
        "exports" => Some(TrackedName::Exports),
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
        for name in TRACKED_NAMES {
            if pattern_bindings(&parameter.pattern).contains(name) {
                bindings.mark(name);
            }
        }
    }
    if let Some(rest) = &parameters.rest {
        let rest_bindings = pattern_bindings(&rest.rest.argument);
        for name in TRACKED_NAMES {
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
                self.root_has_commonjs_wrapper = program.source_type.is_commonjs();
                self.ordered_root_statement_spans
                    .extend(program.body.iter().filter_map(|statement| match statement {
                        Statement::ExpressionStatement(statement) => {
                            Some((statement.span.start, statement.span.end))
                        }
                        Statement::VariableDeclaration(declaration) => {
                            Some((declaration.span.start, declaration.span.end))
                        }
                        _ => None,
                    }));
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
                        for name in TRACKED_NAMES {
                            if bindings.contains(name) {
                                if declaration.kind.is_var() {
                                    let wrapper_redeclaration = self.root_has_commonjs_wrapper
                                        && self.nearest_var_environment_is_root()
                                        && matches!(
                                            name,
                                            TrackedName::Require
                                                | TrackedName::Module
                                                | TrackedName::Exports
                                        );
                                    if !wrapper_redeclaration {
                                        self.mark_nearest_var_environment(name);
                                    }
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
            self.record_require_write(self.assignment_target_timing(target.span().end));
        }
        walk::walk_simple_assignment_target(self, target);
    }

    fn visit_assignment_target_property_identifier(
        &mut self,
        property: &oxc_ast::ast::AssignmentTargetPropertyIdentifier<'a>,
    ) {
        if property.binding.name == "require" {
            self.record_require_write(self.assignment_target_timing(property.span.end));
        }
        walk::walk_assignment_target_property_identifier(self, property);
    }

    fn visit_assignment_expression(&mut self, expression: &oxc_ast::ast::AssignmentExpression<'a>) {
        let previous = self.assignment_write_context;
        self.assignment_write_context =
            Some(if expression.left.as_simple_assignment_target().is_some() {
                AssignmentWriteContext::AfterExpression(expression.span.end)
            } else {
                let lhs = expression.left.span();
                let rhs = expression.right.span();
                AssignmentWriteContext::Destructuring {
                    lhs_start: lhs.start,
                    lhs_end: lhs.end,
                    rhs_start: rhs.start,
                    rhs_end: rhs.end,
                }
            });
        walk::walk_assignment_expression(self, expression);
        self.assignment_write_context = previous;
    }

    fn visit_assignment_target_with_default(
        &mut self,
        target: &oxc_ast::ast::AssignmentTargetWithDefault<'a>,
    ) {
        let previous = self.destructuring_target_position;
        self.destructuring_target_position = Some(target.span.end);
        walk::walk_assignment_target_with_default(self, target);
        self.destructuring_target_position = previous;
    }

    fn visit_call_expression(&mut self, expression: &oxc_ast::ast::CallExpression<'a>) {
        if let Some(identifier) = expression.callee.get_identifier_reference()
            && identifier.name == "require"
        {
            self.direct_require_callees
                .insert((identifier.span.start, identifier.span.end));
        }
        if !expression.optional && expression.callee.is_specific_id("eval") {
            self.record_eval_call(self.expression_mutation_timing(expression.span.end));
        }
        walk::walk_call_expression(self, expression);
    }

    fn visit_with_statement(&mut self, statement: &oxc_ast::ast::WithStatement<'a>) {
        self.visit_expression(&statement.object);
        self.push_inherited_scope(RequireScopeKind::Lexical, NameBindings::default(), true);
        self.visit_statement(&statement.body);
        self.pop_scope();
    }

    fn visit_expression_statement(&mut self, statement: &oxc_ast::ast::ExpressionStatement<'a>) {
        let previous = self.inside_ordered_root_statement;
        self.inside_ordered_root_statement = self
            .ordered_root_statement_spans
            .contains(&(statement.span.start, statement.span.end));
        walk::walk_expression_statement(self, statement);
        self.inside_ordered_root_statement = previous;
    }

    fn visit_variable_declaration(&mut self, declaration: &oxc_ast::ast::VariableDeclaration<'a>) {
        let previous = self.inside_ordered_root_statement;
        self.inside_ordered_root_statement = self
            .ordered_root_statement_spans
            .contains(&(declaration.span.start, declaration.span.end));
        walk::walk_variable_declaration(self, declaration);
        self.inside_ordered_root_statement = previous;
    }

    fn visit_variable_declarator(&mut self, declarator: &oxc_ast::ast::VariableDeclarator<'a>) {
        let previous = self.binding_write_context;
        if declarator.kind.is_var()
            && self.root_has_commonjs_wrapper
            && self.nearest_var_environment_is_root()
            && pattern_bindings(&declarator.id).contains(TrackedName::Require)
            && let Some(init) = &declarator.init
        {
            self.binding_write_context = Some(if declarator.id.is_binding_identifier() {
                AssignmentWriteContext::AfterExpression(declarator.span.end)
            } else {
                let lhs = declarator.id.span();
                let rhs = init.span();
                AssignmentWriteContext::Destructuring {
                    lhs_start: lhs.start,
                    lhs_end: lhs.end,
                    rhs_start: rhs.start,
                    rhs_end: rhs.end,
                }
            });
        }
        walk::walk_variable_declarator(self, declarator);
        self.binding_write_context = previous;
    }

    fn visit_binding_identifier(&mut self, identifier: &oxc_ast::ast::BindingIdentifier<'a>) {
        if identifier.name == "require" && self.binding_write_context.is_some() {
            self.record_require_write(self.binding_target_timing(identifier.span.end));
        }
        walk::walk_binding_identifier(self, identifier);
    }

    fn visit_assignment_pattern(&mut self, pattern: &oxc_ast::ast::AssignmentPattern<'a>) {
        let previous = self.binding_target_position;
        self.binding_target_position = Some(pattern.span.end);
        walk::walk_assignment_pattern(self, pattern);
        self.binding_target_position = previous;
    }

    fn visit_unary_expression(&mut self, expression: &oxc_ast::ast::UnaryExpression<'a>) {
        if expression.operator.is_typeof()
            && let Some(identifier) = expression
                .argument
                .without_parentheses()
                .get_identifier_reference()
            && identifier.name == "require"
        {
            self.non_escaping_require_references
                .insert((identifier.span.start, identifier.span.end));
        }
        walk::walk_unary_expression(self, expression);
    }

    fn visit_identifier_reference(&mut self, identifier: &oxc_ast::ast::IdentifierReference<'a>) {
        if identifier.name == "require"
            && !self.current_ambient()
            && let Some(scope) = self.scope_stack.last().copied()
        {
            self.require_reference_scopes
                .push((scope, identifier.span.start, identifier.span.end));
        }
        walk::walk_identifier_reference(self, identifier);
    }
}
