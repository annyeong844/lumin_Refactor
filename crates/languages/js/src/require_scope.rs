use std::collections::BTreeSet;

use oxc_ast::ast_kind::AstKind;

mod collector;
mod mapped_arguments;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrackedName {
    Require,
    Eval,
    Module,
    Exports,
    Arguments,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct NameBindings {
    require: bool,
    eval: bool,
    module: bool,
    exports: bool,
    arguments: bool,
}

impl NameBindings {
    fn mark(&mut self, name: TrackedName) {
        match name {
            TrackedName::Require => self.require = true,
            TrackedName::Eval => self.eval = true,
            TrackedName::Module => self.module = true,
            TrackedName::Exports => self.exports = true,
            TrackedName::Arguments => self.arguments = true,
        }
    }

    fn contains(self, name: TrackedName) -> bool {
        match name {
            TrackedName::Require => self.require,
            TrackedName::Eval => self.eval,
            TrackedName::Module => self.module,
            TrackedName::Exports => self.exports,
            TrackedName::Arguments => self.arguments,
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
    deferred_execution: bool,
    wrapper_this: bool,
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
    ordered_execution: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum MutationTiming {
    After(u32),
    RhsBeforeTarget {
        target_position: u32,
        rhs_start: u32,
        rhs_end: u32,
    },
}

impl MutationTiming {
    fn precedes_call(self, call_position: u32) -> bool {
        match self {
            Self::After(position) => position < call_position,
            Self::RhsBeforeTarget {
                target_position,
                rhs_start,
                rhs_end,
            } => !(rhs_start..rhs_end).contains(&call_position) && target_position < call_position,
        }
    }

    fn target_position(self) -> u32 {
        match self {
            Self::After(position)
            | Self::RhsBeforeTarget {
                target_position: position,
                ..
            } => position,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AssignmentWriteContext {
    AfterExpression(u32),
    RhsBeforeTarget {
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
    deferred_require_call_positions: BTreeSet<u32>,
    non_wrapper_this_ranges: BTreeSet<(u32, u32)>,
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
        if self
            .deferred_require_call_positions
            .contains(&call_position)
            || self
                .scopes
                .get(scope)
                .is_none_or(|scope| scope.deferred_execution)
        {
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
    pub(super) fn analyze(
        program: &oxc_ast::ast::Program<'_>,
        root_has_commonjs_wrapper: bool,
        root_this_may_be_wrapper: bool,
    ) -> Self {
        Self {
            model: collector::collect(program, root_has_commonjs_wrapper, root_this_may_be_wrapper),
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

    pub(super) fn this_may_be_wrapper(&self, position: u32) -> bool {
        if self.tracking_failed {
            return true;
        }
        if self
            .model
            .non_wrapper_this_ranges
            .iter()
            .any(|(start, end)| *start <= position && position <= *end)
        {
            return false;
        }
        self.scope_stack
            .last()
            .and_then(|scope| self.model.scopes.get(*scope))
            .is_none_or(|scope| scope.wrapper_this)
    }

    pub(super) fn mapped_wrapper_export_object_may_be_visible(
        &self,
        expression: &oxc_ast::ast::MemberExpression<'_>,
    ) -> bool {
        if !mapped_arguments::member_is_wrapper_export_slot(expression) {
            return false;
        }
        self.wrapper_name_may_be_implicit(TrackedName::Arguments)
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
