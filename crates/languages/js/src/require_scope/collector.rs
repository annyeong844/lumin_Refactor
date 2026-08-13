use std::collections::BTreeSet;

use oxc_ast::ast::{
    BindingPattern, FormalParameters, ImportDeclarationSpecifier, ImportOrExportKind,
};
use oxc_ast::ast_kind::AstKind;
use oxc_ast_visit::{Visit, walk};
use oxc_span::GetSpan;

use super::{
    AssignmentWriteContext, MutationObservation, MutationTiming, NameBindings, NameResolution,
    RequireScope, RequireScopeKind, RequireScopeModel, TrackedName,
    class_evaluation::ClassEvaluationPhases, mapped_arguments, scope_kind,
};

const TRACKED_NAMES: [TrackedName; 5] = [
    TrackedName::Require,
    TrackedName::Eval,
    TrackedName::Module,
    TrackedName::Exports,
    TrackedName::Arguments,
];

#[derive(Default)]
struct RequireScopeCollector {
    scopes: Vec<RequireScope>,
    scope_stack: Vec<usize>,
    require_writes: Vec<MutationObservation>,
    eval_calls: Vec<MutationObservation>,
    arguments_escapes: Vec<MutationObservation>,
    require_reference_scopes: Vec<(usize, u32, u32)>,
    direct_require_callees: BTreeSet<(u32, u32)>,
    direct_require_call_positions: BTreeSet<u32>,
    non_escaping_require_references: BTreeSet<(u32, u32)>,
    non_escaping_arguments_references: BTreeSet<(u32, u32)>,
    deferred_execution_ranges: BTreeSet<(u32, u32)>,
    repeated_execution_ranges: BTreeSet<(u32, u32)>,
    non_wrapper_this_ranges: BTreeSet<(u32, u32)>,
    deferred_require_call_positions: BTreeSet<u32>,
    assignment_write_context: Option<AssignmentWriteContext>,
    destructuring_target_position: Option<u32>,
    binding_write_context: Option<AssignmentWriteContext>,
    binding_target_position: Option<u32>,
    loop_head_write_range: Option<(u32, u32)>,
    loop_head_is_root_ordered: bool,
    arguments_escape_timing: Option<MutationTiming>,
    computed_property_key_timing: Option<MutationTiming>,
    suppress_arguments_mutations: usize,
    tagged_template_quasi: Option<(u32, u32)>,
    jsx_invocation_end: Option<u32>,
    root_has_commonjs_wrapper: bool,
    root_this_may_be_wrapper: bool,
    class_evaluation_phases: Vec<ClassEvaluationPhases>,
}

impl RequireScopeCollector {
    fn new(root_has_commonjs_wrapper: bool, root_this_may_be_wrapper: bool) -> Self {
        Self {
            root_has_commonjs_wrapper,
            root_this_may_be_wrapper,
            ..Self::default()
        }
    }

    fn push_scope(
        &mut self,
        kind: RequireScopeKind,
        bindings: NameBindings,
        dynamic_lookup: bool,
        strict: bool,
        ambient: bool,
    ) {
        let index = self.scopes.len();
        let wrapper_this = self
            .current_scope()
            .map_or(self.root_this_may_be_wrapper, |scope| scope.wrapper_this);
        let deferred_execution = self
            .current_scope()
            .is_some_and(|scope| scope.deferred_execution)
            || kind == RequireScopeKind::FunctionParameters;
        self.scopes.push(RequireScope {
            parent: self.scope_stack.last().copied(),
            kind,
            bindings,
            dynamic_lookup,
            strict,
            ambient,
            deferred_execution,
            wrapper_this,
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

    fn suppress_wrapper_this(&mut self) {
        if let Some(scope) = self
            .scope_stack
            .last()
            .and_then(|index| self.scopes.get_mut(*index))
        {
            scope.wrapper_this = false;
        }
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

    fn current_has_binding(&self, name: TrackedName) -> bool {
        let mut cursor = self.scope_stack.last().copied();
        while let Some(index) = cursor {
            let Some(scope) = self.scopes.get(index) else {
                return true;
            };
            if scope.bindings.contains(name) {
                return true;
            }
            cursor = scope.parent;
        }
        false
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
            let loop_head_ordered = self.loop_head_is_root_ordered
                && self.loop_head_write_range.is_some_and(|(start, end)| {
                    position_within_span(start, end, timing.target_position())
                });
            let ordered_timing = self.ordered_mutation_timing(scope, timing, loop_head_ordered);
            self.require_writes.push(MutationObservation {
                scope,
                timing: ordered_timing.unwrap_or(timing),
                ordered_execution: ordered_timing.is_some(),
            });
        }
    }

    fn record_eval_call(&mut self, timing: MutationTiming) {
        if !self.current_ambient()
            && let Some(scope) = self.scope_stack.last().copied()
        {
            let ordered_timing = self.ordered_mutation_timing(scope, timing, false);
            self.eval_calls.push(MutationObservation {
                scope,
                timing: ordered_timing.unwrap_or(timing),
                ordered_execution: ordered_timing.is_some(),
            });
        }
    }

    fn record_arguments_escape(&mut self, timing: MutationTiming) {
        if self.suppress_arguments_mutations == 0
            && !self.current_ambient()
            && let Some(scope) = self.scope_stack.last().copied()
        {
            let ordered_timing = self.ordered_mutation_timing(scope, timing, false);
            self.arguments_escapes.push(MutationObservation {
                scope,
                timing: ordered_timing.unwrap_or(timing),
                ordered_execution: ordered_timing.is_some(),
            });
        }
    }

    fn ordered_mutation_timing(
        &self,
        scope: usize,
        timing: MutationTiming,
        preserve_repeated_timing: bool,
    ) -> Option<MutationTiming> {
        if self.position_has_deferred_execution(timing.target_position())
            || (!preserve_repeated_timing
                && self.position_has_repeated_execution(timing.target_position()))
            || self
                .scopes
                .get(scope)
                .is_none_or(|scope| scope.deferred_execution)
        {
            return None;
        }
        Some(timing)
    }

    fn position_has_deferred_execution(&self, position: u32) -> bool {
        self.deferred_execution_ranges
            .iter()
            .any(|(start, end)| position_within_span(*start, *end, position))
    }

    fn position_has_repeated_execution(&self, position: u32) -> bool {
        self.repeated_execution_ranges
            .iter()
            .any(|(start, end)| position_within_span(*start, *end, position))
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
            Some(AssignmentWriteContext::RhsBeforeTarget {
                lhs_start,
                lhs_end,
                rhs_start,
                rhs_end,
            }) if position_within_span(lhs_start, lhs_end, fallback_position) => {
                MutationTiming::RhsBeforeTarget {
                    target_position: target_position.unwrap_or(fallback_position),
                    rhs_start,
                    rhs_end,
                }
            }
            Some(AssignmentWriteContext::RhsBeforeTarget { .. }) => {
                MutationTiming::After(fallback_position)
            }
            None => MutationTiming::After(fallback_position),
        }
    }

    fn expression_mutation_timing(&self, position: u32) -> MutationTiming {
        match self.assignment_write_context {
            Some(AssignmentWriteContext::RhsBeforeTarget {
                lhs_start,
                lhs_end,
                rhs_start,
                rhs_end,
            }) if position_within_span(lhs_start, lhs_end, position) => {
                MutationTiming::RhsBeforeTarget {
                    target_position: position,
                    rhs_start,
                    rhs_end,
                }
            }
            _ => MutationTiming::After(position),
        }
    }

    fn into_model(self) -> RequireScopeModel {
        let mut model = RequireScopeModel {
            scopes: self.scopes,
            unordered_implicit_require_write: false,
            ordered_root_require_writes: Vec::new(),
            deferred_require_call_positions: self.deferred_require_call_positions,
            class_phase_opaque_require_call_positions: BTreeSet::new(),
            class_phase_nonpreceding_mutations: BTreeSet::new(),
            non_wrapper_this_ranges: self.non_wrapper_this_ranges,
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
        let mapped_arguments_mutations = mapped_arguments::mapped_require_mutations(
            self.root_has_commonjs_wrapper,
            &model,
            &self.arguments_escapes,
        );
        implicit_mutations.extend(intrinsic_eval_mutations.iter().copied());
        implicit_mutations.extend(mapped_arguments_mutations.iter().copied());
        for phases in &self.class_evaluation_phases {
            let computed_key_mutates_require = implicit_mutations.iter().any(|observation| {
                observation.ordered_execution
                    && phases.computed_key_contains(observation.timing.target_position())
            });
            if computed_key_mutates_require {
                model.class_phase_opaque_require_call_positions.extend(
                    self.direct_require_call_positions
                        .iter()
                        .copied()
                        .filter(|position| phases.static_execution_contains(*position)),
                );
            }
            let computed_key_calls = self
                .direct_require_call_positions
                .iter()
                .copied()
                .filter(|position| phases.computed_key_contains(*position));
            for observation in implicit_mutations.iter().filter(|observation| {
                observation.ordered_execution
                    && phases.static_execution_contains(observation.timing.target_position())
            }) {
                model.class_phase_nonpreceding_mutations.extend(
                    computed_key_calls
                        .clone()
                        .map(|position| (observation.timing, position)),
                );
            }
        }
        for observation in implicit_mutations {
            if observation.ordered_execution {
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
        model.unattributed_require_escape = escaped_reference
            || !intrinsic_eval_mutations.is_empty()
            || !mapped_arguments_mutations.is_empty();
        model
    }
}

pub(super) fn collect(
    program: &oxc_ast::ast::Program<'_>,
    root_has_commonjs_wrapper: bool,
    root_this_may_be_wrapper: bool,
) -> RequireScopeModel {
    let mut collector =
        RequireScopeCollector::new(root_has_commonjs_wrapper, root_this_may_be_wrapper);
    collector.visit_program(program);
    collector.into_model()
}

fn tracked_name(name: &str) -> Option<TrackedName> {
    match name {
        "require" => Some(TrackedName::Require),
        "eval" => Some(TrackedName::Eval),
        "module" => Some(TrackedName::Module),
        "exports" => Some(TrackedName::Exports),
        "arguments" => Some(TrackedName::Arguments),
        _ => None,
    }
}

fn position_within_span(start: u32, end: u32, position: u32) -> bool {
    start <= position && position <= end
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
                if !ambient {
                    bindings.mark(TrackedName::Arguments);
                }
                self.push_scope(
                    RequireScopeKind::FunctionParameters,
                    bindings,
                    false,
                    self.current_strict() || function.has_use_strict_directive(),
                    ambient,
                );
                self.suppress_wrapper_this();
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
                if let Some(phases) = ClassEvaluationPhases::from_class(class) {
                    self.class_evaluation_phases.push(phases);
                }
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
                self.suppress_wrapper_this();
                return;
            }
            AstKind::PropertyDefinition(property) => {
                if let Some(value) = &property.value {
                    let span = value.span();
                    self.non_wrapper_this_ranges.insert((span.start, span.end));
                    if !property.r#static {
                        self.deferred_execution_ranges
                            .insert((span.start, span.end));
                    }
                }
            }
            AstKind::AccessorProperty(property) => {
                if let Some(value) = &property.value {
                    let span = value.span();
                    self.non_wrapper_this_ranges.insert((span.start, span.end));
                    if !property.r#static {
                        self.deferred_execution_ranges
                            .insert((span.start, span.end));
                    }
                }
            }
            AstKind::ForStatement(statement) => {
                for expression in [statement.test.as_ref(), statement.update.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    let span = expression.span();
                    self.repeated_execution_ranges
                        .insert((span.start, span.end));
                }
                let span = statement.body.span();
                self.repeated_execution_ranges
                    .insert((span.start, span.end));
            }
            AstKind::ForInStatement(statement) => {
                let span = statement.body.span();
                self.repeated_execution_ranges
                    .insert((span.start, span.end));
            }
            AstKind::ForOfStatement(statement) => {
                let span = statement.body.span();
                self.repeated_execution_ranges
                    .insert((span.start, span.end));
            }
            AstKind::WhileStatement(statement) => {
                for span in [statement.test.span(), statement.body.span()] {
                    self.repeated_execution_ranges
                        .insert((span.start, span.end));
                }
            }
            AstKind::DoWhileStatement(statement) => {
                for span in [statement.body.span(), statement.test.span()] {
                    self.repeated_execution_ranges
                        .insert((span.start, span.end));
                }
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
                                                | TrackedName::Arguments
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
                self.suppress_wrapper_this();
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
                self.suppress_wrapper_this();
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

    fn visit_for_in_statement(&mut self, statement: &oxc_ast::ast::ForInStatement<'a>) {
        let lhs = statement.left.span();
        let rhs = statement.right.span();
        let context = AssignmentWriteContext::RhsBeforeTarget {
            lhs_start: lhs.start,
            lhs_end: lhs.end,
            rhs_start: rhs.start,
            rhs_end: rhs.end,
        };
        let previous_assignment = self.assignment_write_context;
        let previous_binding = self.binding_write_context;
        let previous_range = self.loop_head_write_range;
        let previous_ordered = self.loop_head_is_root_ordered;
        self.assignment_write_context = Some(context);
        self.binding_write_context = Some(context);
        self.loop_head_write_range = Some((lhs.start, lhs.end));
        self.loop_head_is_root_ordered = self
            .current_scope()
            .is_some_and(|scope| scope.kind == RequireScopeKind::Root);
        walk::walk_for_in_statement(self, statement);
        self.assignment_write_context = previous_assignment;
        self.binding_write_context = previous_binding;
        self.loop_head_write_range = previous_range;
        self.loop_head_is_root_ordered = previous_ordered;
    }

    fn visit_for_of_statement(&mut self, statement: &oxc_ast::ast::ForOfStatement<'a>) {
        let lhs = statement.left.span();
        let rhs = statement.right.span();
        let context = AssignmentWriteContext::RhsBeforeTarget {
            lhs_start: lhs.start,
            lhs_end: lhs.end,
            rhs_start: rhs.start,
            rhs_end: rhs.end,
        };
        let previous_assignment = self.assignment_write_context;
        let previous_binding = self.binding_write_context;
        let previous_range = self.loop_head_write_range;
        let previous_ordered = self.loop_head_is_root_ordered;
        self.assignment_write_context = Some(context);
        self.binding_write_context = Some(context);
        self.loop_head_write_range = Some((lhs.start, lhs.end));
        self.loop_head_is_root_ordered = self
            .current_scope()
            .is_some_and(|scope| scope.kind == RequireScopeKind::Root);
        walk::walk_for_of_statement(self, statement);
        self.assignment_write_context = previous_assignment;
        self.binding_write_context = previous_binding;
        self.loop_head_write_range = previous_range;
        self.loop_head_is_root_ordered = previous_ordered;
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
        let mapped_arguments_write = mapped_arguments::target_is_require_slot(
            target,
            self.root_has_commonjs_wrapper,
            self.current_strict(),
            self.current_has_binding(TrackedName::Arguments),
        );
        if direct_write || wrapped_write {
            self.record_require_write(self.assignment_target_timing(target.span().end));
        }
        if mapped_arguments_write && self.suppress_arguments_mutations == 0 {
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
        let previous_escape = self.arguments_escape_timing;
        if let Some(span) = expression
            .left
            .as_simple_assignment_target()
            .and_then(mapped_arguments::direct_arguments_target)
            .filter(|_| expression.operator.is_arithmetic() || expression.operator.is_bitwise())
        {
            self.non_escaping_arguments_references.insert(span);
            self.record_arguments_escape(self.expression_mutation_timing(expression.span.end));
        }
        let suppress_mutations =
            mapped_arguments::logical_assignment_rhs_cannot_execute(expression);
        if suppress_mutations {
            self.suppress_arguments_mutations += 1;
        }
        self.assignment_write_context =
            Some(if expression.left.as_simple_assignment_target().is_some() {
                AssignmentWriteContext::AfterExpression(expression.span.end)
            } else {
                let lhs = expression.left.span();
                let rhs = expression.right.span();
                AssignmentWriteContext::RhsBeforeTarget {
                    lhs_start: lhs.start,
                    lhs_end: lhs.end,
                    rhs_start: rhs.start,
                    rhs_end: rhs.end,
                }
            });
        self.arguments_escape_timing = Some(MutationTiming::After(expression.span.end));
        walk::walk_assignment_expression(self, expression);
        if suppress_mutations {
            self.suppress_arguments_mutations -= 1;
        }
        self.assignment_write_context = previous;
        self.arguments_escape_timing = previous_escape;
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
            self.direct_require_call_positions
                .insert(expression.span.start);
            if self.position_has_deferred_execution(expression.span.start) {
                self.deferred_require_call_positions
                    .insert(expression.span.start);
            }
        }
        if !expression.optional && expression.callee.is_specific_id("eval") {
            self.record_eval_call(self.expression_mutation_timing(expression.span.end));
        }
        let mutation_timing = self.expression_mutation_timing(expression.span.end);
        if mapped_arguments::callee_invokes_arguments_receiver(&expression.callee) {
            self.record_arguments_escape(mutation_timing);
        }
        let previous_escape = self.arguments_escape_timing;
        self.arguments_escape_timing = Some(mutation_timing);
        walk::walk_call_expression(self, expression);
        self.arguments_escape_timing = previous_escape;
    }

    fn visit_new_expression(&mut self, expression: &oxc_ast::ast::NewExpression<'a>) {
        let previous_escape = self.arguments_escape_timing;
        self.arguments_escape_timing = Some(self.expression_mutation_timing(expression.span.end));
        walk::walk_new_expression(self, expression);
        self.arguments_escape_timing = previous_escape;
    }

    fn visit_tagged_template_expression(
        &mut self,
        expression: &oxc_ast::ast::TaggedTemplateExpression<'a>,
    ) {
        let previous_escape = self.arguments_escape_timing;
        let previous_quasi = self.tagged_template_quasi;
        self.arguments_escape_timing = Some(self.expression_mutation_timing(expression.span.end));
        self.tagged_template_quasi = Some((expression.quasi.span.start, expression.quasi.span.end));
        walk::walk_tagged_template_expression(self, expression);
        self.arguments_escape_timing = previous_escape;
        self.tagged_template_quasi = previous_quasi;
    }

    fn visit_template_literal(&mut self, literal: &oxc_ast::ast::TemplateLiteral<'a>) {
        if self.tagged_template_quasi == Some((literal.span.start, literal.span.end)) {
            walk::walk_template_literal(self, literal);
            return;
        }

        let kind = AstKind::TemplateLiteral(self.alloc(literal));
        self.enter_node(kind);
        self.visit_span(&literal.span);
        self.visit_template_elements(&literal.quasis);
        for expression in &literal.expressions {
            let previous_escape = self.arguments_escape_timing;
            self.arguments_escape_timing =
                Some(self.expression_mutation_timing(expression.span().end));
            self.visit_expression(expression);
            self.arguments_escape_timing = previous_escape;
        }
        self.leave_node(kind);
    }

    fn visit_jsx_element(&mut self, expression: &oxc_ast::ast::JSXElement<'a>) {
        let previous_end = self.jsx_invocation_end;
        self.jsx_invocation_end = Some(expression.span.end);
        walk::walk_jsx_element(self, expression);
        self.jsx_invocation_end = previous_end;
    }

    fn visit_jsx_fragment(&mut self, expression: &oxc_ast::ast::JSXFragment<'a>) {
        let previous_end = self.jsx_invocation_end;
        self.jsx_invocation_end = Some(expression.span.end);
        walk::walk_jsx_fragment(self, expression);
        self.jsx_invocation_end = previous_end;
    }

    fn visit_jsx_expression_container(
        &mut self,
        expression: &oxc_ast::ast::JSXExpressionContainer<'a>,
    ) {
        let previous_escape = self.arguments_escape_timing;
        self.arguments_escape_timing = self
            .jsx_invocation_end
            .map(|end| self.expression_mutation_timing(end));
        walk::walk_jsx_expression_container(self, expression);
        self.arguments_escape_timing = previous_escape;
    }

    fn visit_jsx_member_expression_object(
        &mut self,
        expression: &oxc_ast::ast::JSXMemberExpressionObject<'a>,
    ) {
        if let Some(span) = mapped_arguments::jsx_member_object_span(expression) {
            self.non_escaping_arguments_references.insert(span);
        }
        walk::walk_jsx_member_expression_object(self, expression);
    }

    fn visit_jsx_spread_attribute(&mut self, attribute: &oxc_ast::ast::JSXSpreadAttribute<'a>) {
        let previous_escape = self.arguments_escape_timing;
        self.arguments_escape_timing = self
            .jsx_invocation_end
            .map(|end| self.expression_mutation_timing(end));
        walk::walk_jsx_spread_attribute(self, attribute);
        self.arguments_escape_timing = previous_escape;
    }

    fn visit_jsx_spread_child(&mut self, child: &oxc_ast::ast::JSXSpreadChild<'a>) {
        let previous_escape = self.arguments_escape_timing;
        self.arguments_escape_timing = self
            .jsx_invocation_end
            .map(|end| self.expression_mutation_timing(end));
        walk::walk_jsx_spread_child(self, child);
        self.arguments_escape_timing = previous_escape;
    }

    fn visit_return_statement(&mut self, statement: &oxc_ast::ast::ReturnStatement<'a>) {
        let previous_escape = self.arguments_escape_timing;
        self.arguments_escape_timing = Some(MutationTiming::After(statement.span.end));
        walk::walk_return_statement(self, statement);
        self.arguments_escape_timing = previous_escape;
    }

    fn visit_throw_statement(&mut self, statement: &oxc_ast::ast::ThrowStatement<'a>) {
        let previous_escape = self.arguments_escape_timing;
        self.arguments_escape_timing = Some(MutationTiming::After(statement.span.end));
        walk::walk_throw_statement(self, statement);
        self.arguments_escape_timing = previous_escape;
    }

    fn visit_with_statement(&mut self, statement: &oxc_ast::ast::WithStatement<'a>) {
        self.visit_expression(&statement.object);
        self.push_inherited_scope(RequireScopeKind::Lexical, NameBindings::default(), true);
        self.visit_statement(&statement.body);
        self.pop_scope();
    }

    fn visit_variable_declarator(&mut self, declarator: &oxc_ast::ast::VariableDeclarator<'a>) {
        let previous = self.binding_write_context;
        let previous_escape = self.arguments_escape_timing;
        if let Some(init) = &declarator.init {
            self.binding_write_context = Some(if declarator.id.is_binding_identifier() {
                AssignmentWriteContext::AfterExpression(declarator.span.end)
            } else {
                let lhs = declarator.id.span();
                let rhs = init.span();
                AssignmentWriteContext::RhsBeforeTarget {
                    lhs_start: lhs.start,
                    lhs_end: lhs.end,
                    rhs_start: rhs.start,
                    rhs_end: rhs.end,
                }
            });
        }
        if declarator.init.is_some() {
            self.arguments_escape_timing = Some(MutationTiming::After(declarator.span.end));
        }
        walk::walk_variable_declarator(self, declarator);
        self.binding_write_context = previous;
        self.arguments_escape_timing = previous_escape;
    }

    fn visit_binding_identifier(&mut self, identifier: &oxc_ast::ast::BindingIdentifier<'a>) {
        let inside_write_target = self
            .binding_write_context
            .is_some_and(|context| match context {
                AssignmentWriteContext::AfterExpression(_) => true,
                AssignmentWriteContext::RhsBeforeTarget {
                    lhs_start, lhs_end, ..
                } => position_within_span(lhs_start, lhs_end, identifier.span.start),
            });
        if identifier.name == "require" && inside_write_target {
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
        let arguments = mapped_arguments::value_arguments_references(&expression.argument);
        self.non_escaping_arguments_references
            .extend(arguments.iter().copied());
        if !arguments.is_empty()
            && (expression.operator.is_arithmetic() || expression.operator.is_bitwise())
        {
            self.record_arguments_escape(self.expression_mutation_timing(expression.span.end));
        }
        walk::walk_unary_expression(self, expression);
    }

    fn visit_update_expression(&mut self, expression: &oxc_ast::ast::UpdateExpression<'a>) {
        if let Some(span) = mapped_arguments::direct_arguments_target(&expression.argument) {
            self.non_escaping_arguments_references.insert(span);
            self.record_arguments_escape(self.expression_mutation_timing(expression.span.end));
        }
        walk::walk_update_expression(self, expression);
    }

    fn visit_binary_expression(&mut self, expression: &oxc_ast::ast::BinaryExpression<'a>) {
        let arguments = mapped_arguments::binary_arguments_use(expression);
        self.non_escaping_arguments_references
            .extend(arguments.references);
        if arguments.may_mutate {
            self.record_arguments_escape(self.expression_mutation_timing(expression.span.end));
        }
        walk::walk_binary_expression(self, expression);
    }

    fn visit_logical_expression(&mut self, expression: &oxc_ast::ast::LogicalExpression<'a>) {
        if expression.operator.is_and() {
            self.non_escaping_arguments_references.extend(
                mapped_arguments::value_arguments_references(&expression.left),
            );
        }
        let kind = AstKind::LogicalExpression(self.alloc(expression));
        self.enter_node(kind);
        self.visit_span(&expression.span);
        self.visit_expression(&expression.left);
        let suppress_rhs = mapped_arguments::logical_rhs_cannot_execute(expression);
        if suppress_rhs {
            self.suppress_arguments_mutations += 1;
        }
        self.visit_expression(&expression.right);
        if suppress_rhs {
            self.suppress_arguments_mutations -= 1;
        }
        self.leave_node(kind);
    }

    fn visit_conditional_expression(
        &mut self,
        expression: &oxc_ast::ast::ConditionalExpression<'a>,
    ) {
        self.non_escaping_arguments_references.extend(
            mapped_arguments::value_arguments_references(&expression.test),
        );
        let truthiness = mapped_arguments::condition_truthiness(&expression.test);
        let kind = AstKind::ConditionalExpression(self.alloc(expression));
        self.enter_node(kind);
        self.visit_span(&expression.span);
        self.visit_expression(&expression.test);
        if truthiness == Some(false) {
            self.suppress_arguments_mutations += 1;
        }
        self.visit_expression(&expression.consequent);
        if truthiness == Some(false) {
            self.suppress_arguments_mutations -= 1;
        }
        if truthiness == Some(true) {
            self.suppress_arguments_mutations += 1;
        }
        self.visit_expression(&expression.alternate);
        if truthiness == Some(true) {
            self.suppress_arguments_mutations -= 1;
        }
        self.leave_node(kind);
    }

    fn visit_sequence_expression(&mut self, expression: &oxc_ast::ast::SequenceExpression<'a>) {
        for discarded in expression.expressions.iter().rev().skip(1) {
            self.non_escaping_arguments_references
                .extend(mapped_arguments::value_arguments_references(discarded));
        }
        walk::walk_sequence_expression(self, expression);
    }

    fn visit_object_property(&mut self, property: &oxc_ast::ast::ObjectProperty<'a>) {
        let previous = self.computed_property_key_timing;
        self.computed_property_key_timing = property
            .computed
            .then_some(MutationTiming::After(property.key.span().end));
        walk::walk_object_property(self, property);
        self.computed_property_key_timing = previous;
    }

    fn visit_method_definition(&mut self, method: &oxc_ast::ast::MethodDefinition<'a>) {
        let previous = self.computed_property_key_timing;
        self.computed_property_key_timing = method
            .computed
            .then_some(MutationTiming::After(method.key.span().end));
        walk::walk_method_definition(self, method);
        self.computed_property_key_timing = previous;
    }

    fn visit_property_definition(&mut self, property: &oxc_ast::ast::PropertyDefinition<'a>) {
        let previous = self.computed_property_key_timing;
        self.computed_property_key_timing = property
            .computed
            .then_some(MutationTiming::After(property.key.span().end));
        walk::walk_property_definition(self, property);
        self.computed_property_key_timing = previous;
    }

    fn visit_accessor_property(&mut self, property: &oxc_ast::ast::AccessorProperty<'a>) {
        let previous = self.computed_property_key_timing;
        self.computed_property_key_timing = property
            .computed
            .then_some(MutationTiming::After(property.key.span().end));
        walk::walk_accessor_property(self, property);
        self.computed_property_key_timing = previous;
    }

    fn visit_assignment_target_property_property(
        &mut self,
        property: &oxc_ast::ast::AssignmentTargetPropertyProperty<'a>,
    ) {
        let previous = self.computed_property_key_timing;
        self.computed_property_key_timing = property
            .computed
            .then_some(self.assignment_target_timing(property.name.span().end));
        walk::walk_assignment_target_property_property(self, property);
        self.computed_property_key_timing = previous;
    }

    fn visit_binding_property(&mut self, property: &oxc_ast::ast::BindingProperty<'a>) {
        let previous = self.computed_property_key_timing;
        self.computed_property_key_timing = property
            .computed
            .then_some(self.binding_target_timing(property.key.span().end));
        walk::walk_binding_property(self, property);
        self.computed_property_key_timing = previous;
    }

    fn visit_property_key(&mut self, key: &oxc_ast::ast::PropertyKey<'a>) {
        let key_timing = self.computed_property_key_timing.take();
        let previous_escape = self.arguments_escape_timing;
        if let Some(timing) = key_timing {
            self.arguments_escape_timing = Some(timing);
        }
        walk::walk_property_key(self, key);
        self.arguments_escape_timing = previous_escape;
    }

    fn visit_identifier_reference(&mut self, identifier: &oxc_ast::ast::IdentifierReference<'a>) {
        if identifier.name == "require"
            && !self.current_ambient()
            && let Some(scope) = self.scope_stack.last().copied()
        {
            self.require_reference_scopes
                .push((scope, identifier.span.start, identifier.span.end));
        }
        if identifier.name == "arguments"
            && !self.current_ambient()
            && !self
                .non_escaping_arguments_references
                .contains(&(identifier.span.start, identifier.span.end))
            && let Some(timing) = self.arguments_escape_timing
        {
            self.record_arguments_escape(timing);
        }
        walk::walk_identifier_reference(self, identifier);
    }

    fn visit_computed_member_expression(
        &mut self,
        expression: &oxc_ast::ast::ComputedMemberExpression<'a>,
    ) {
        if let Some(span) = mapped_arguments::non_require_computed_object_span(expression) {
            self.non_escaping_arguments_references.insert(span);
        }
        walk::walk_computed_member_expression(self, expression);
    }

    fn visit_static_member_expression(
        &mut self,
        expression: &oxc_ast::ast::StaticMemberExpression<'a>,
    ) {
        if let Some(span) = mapped_arguments::static_object_span(expression) {
            self.non_escaping_arguments_references.insert(span);
        }
        walk::walk_static_member_expression(self, expression);
    }
}
