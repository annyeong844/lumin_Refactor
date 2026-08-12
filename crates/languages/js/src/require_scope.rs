use oxc_ast::ast::{
    BindingPattern, FormalParameters, ImportDeclarationSpecifier, ImportOrExportKind,
};
use oxc_ast::ast_kind::AstKind;
use oxc_ast_visit::{Visit, walk};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequireScopeKind {
    Root,
    Function,
    Lexical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RequireScope {
    parent: Option<usize>,
    kind: RequireScopeKind,
    binds_require: bool,
    dynamic_lookup: bool,
}

#[derive(Debug)]
struct RequireScopeModel {
    scopes: Vec<RequireScope>,
    implicit_require_written: bool,
}

impl RequireScopeModel {
    fn require_is_opaque(&self, scope: usize) -> bool {
        let mut cursor = Some(scope);
        while let Some(index) = cursor {
            let Some(current) = self.scopes.get(index) else {
                return true;
            };
            if current.binds_require || current.dynamic_lookup {
                return true;
            }
            cursor = current.parent;
        }
        self.implicit_require_written
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
    write_scopes: Vec<usize>,
    dynamic_require_write: bool,
}

impl RequireScopeCollector {
    fn push_scope(&mut self, kind: RequireScopeKind, binds_require: bool, dynamic_lookup: bool) {
        let index = self.scopes.len();
        self.scopes.push(RequireScope {
            parent: self.scope_stack.last().copied(),
            kind,
            binds_require,
            dynamic_lookup,
        });
        self.scope_stack.push(index);
    }

    fn pop_scope(&mut self) {
        self.scope_stack.pop();
    }

    fn mark_current_scope(&mut self) {
        if let Some(scope) = self
            .scope_stack
            .last()
            .and_then(|index| self.scopes.get_mut(*index))
        {
            scope.binds_require = true;
        }
    }

    fn mark_nearest_function_scope(&mut self) {
        let scope = self.scope_stack.iter().rev().find_map(|index| {
            self.scopes
                .get(*index)
                .filter(|scope| {
                    matches!(
                        scope.kind,
                        RequireScopeKind::Root | RequireScopeKind::Function
                    )
                })
                .map(|_| *index)
        });
        if let Some(scope) = scope.and_then(|index| self.scopes.get_mut(index)) {
            scope.binds_require = true;
        }
    }

    fn record_write(&mut self) {
        if let Some(scope) = self.scope_stack.last().copied() {
            self.write_scopes.push(scope);
        }
    }

    fn into_model(self) -> RequireScopeModel {
        let implicit_require_written = self.dynamic_require_write
            || self.write_scopes.iter().any(|scope| {
                let mut cursor = Some(*scope);
                while let Some(index) = cursor {
                    let Some(current) = self.scopes.get(index) else {
                        return true;
                    };
                    if current.binds_require {
                        return false;
                    }
                    cursor = current.parent;
                }
                true
            });
        RequireScopeModel {
            scopes: self.scopes,
            implicit_require_written,
        }
    }
}

fn pattern_binds_require(pattern: &BindingPattern<'_>) -> bool {
    pattern
        .get_binding_identifiers()
        .into_iter()
        .any(|identifier| identifier.name == "require")
}

fn parameters_bind_require(parameters: &FormalParameters<'_>) -> bool {
    parameters
        .items
        .iter()
        .any(|parameter| pattern_binds_require(&parameter.pattern))
        || parameters
            .rest
            .as_ref()
            .is_some_and(|rest| pattern_binds_require(&rest.rest.argument))
}

fn scope_kind(kind: AstKind<'_>) -> Option<RequireScopeKind> {
    match kind {
        AstKind::Program(_) => Some(RequireScopeKind::Root),
        AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) => {
            Some(RequireScopeKind::Function)
        }
        AstKind::BlockStatement(_)
        | AstKind::ForStatement(_)
        | AstKind::ForInStatement(_)
        | AstKind::ForOfStatement(_)
        | AstKind::WithStatement(_)
        | AstKind::SwitchStatement(_)
        | AstKind::CatchClause(_)
        | AstKind::Class(_)
        | AstKind::StaticBlock(_)
        | AstKind::TSModuleDeclaration(_) => Some(RequireScopeKind::Lexical),
        _ => None,
    }
}

impl<'a> Visit<'a> for RequireScopeCollector {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        match kind {
            AstKind::Function(function) => {
                if matches!(
                    function.r#type,
                    oxc_ast::ast::FunctionType::FunctionDeclaration
                        | oxc_ast::ast::FunctionType::TSDeclareFunction
                ) && function
                    .id
                    .as_ref()
                    .is_some_and(|identifier| identifier.name == "require")
                {
                    self.mark_nearest_function_scope();
                }
                self.push_scope(RequireScopeKind::Function, false, false);
                if function.r#type == oxc_ast::ast::FunctionType::FunctionExpression
                    && function
                        .id
                        .as_ref()
                        .is_some_and(|identifier| identifier.name == "require")
                    || parameters_bind_require(&function.params)
                {
                    self.mark_current_scope();
                }
                return;
            }
            AstKind::ArrowFunctionExpression(function) => {
                self.push_scope(
                    RequireScopeKind::Function,
                    parameters_bind_require(&function.params),
                    false,
                );
                return;
            }
            AstKind::Class(class) => {
                if class.r#type == oxc_ast::ast::ClassType::ClassDeclaration
                    && class
                        .id
                        .as_ref()
                        .is_some_and(|identifier| identifier.name == "require")
                {
                    self.mark_current_scope();
                }
                self.push_scope(
                    RequireScopeKind::Lexical,
                    class.r#type == oxc_ast::ast::ClassType::ClassExpression
                        && class
                            .id
                            .as_ref()
                            .is_some_and(|identifier| identifier.name == "require"),
                    false,
                );
                return;
            }
            AstKind::CatchClause(catch) => {
                self.push_scope(
                    RequireScopeKind::Lexical,
                    catch
                        .param
                        .as_ref()
                        .is_some_and(|parameter| pattern_binds_require(&parameter.pattern)),
                    false,
                );
                return;
            }
            AstKind::WithStatement(_) => {
                self.push_scope(RequireScopeKind::Lexical, false, true);
                return;
            }
            AstKind::VariableDeclaration(declaration) => {
                if declaration
                    .declarations
                    .iter()
                    .any(|declarator| pattern_binds_require(&declarator.id))
                {
                    if declaration.kind.is_var() {
                        self.mark_nearest_function_scope();
                    } else {
                        self.mark_current_scope();
                    }
                }
            }
            AstKind::ImportDeclaration(declaration) => {
                if declaration.import_kind == ImportOrExportKind::Value
                    && declaration.specifiers.as_ref().is_some_and(|specifiers| {
                        specifiers.iter().any(|specifier| match specifier {
                            ImportDeclarationSpecifier::ImportSpecifier(import) => {
                                import.import_kind == ImportOrExportKind::Value
                                    && import.local.name == "require"
                            }
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(import) => {
                                import.local.name == "require"
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(import) => {
                                import.local.name == "require"
                            }
                        })
                    })
                {
                    self.mark_current_scope();
                }
            }
            AstKind::TSEnumDeclaration(declaration) => {
                if declaration.id.name == "require" {
                    self.mark_current_scope();
                }
            }
            AstKind::TSImportEqualsDeclaration(declaration) => {
                if declaration.import_kind == ImportOrExportKind::Value
                    && declaration.id.name == "require"
                {
                    self.mark_current_scope();
                }
            }
            AstKind::TSModuleDeclaration(declaration)
                if !declaration.declare
                    && matches!(
                        &declaration.id,
                        oxc_ast::ast::TSModuleDeclarationName::Identifier(identifier)
                            if identifier.name == "require"
                    ) =>
            {
                self.mark_current_scope();
            }
            _ => {}
        }
        if let Some(kind) = scope_kind(kind) {
            self.push_scope(kind, false, false);
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
            self.record_write();
        }
        walk::walk_simple_assignment_target(self, target);
    }

    fn visit_assignment_target_property_identifier(
        &mut self,
        property: &oxc_ast::ast::AssignmentTargetPropertyIdentifier<'a>,
    ) {
        if property.binding.name == "require" {
            self.record_write();
        }
        walk::walk_assignment_target_property_identifier(self, property);
    }

    fn visit_call_expression(&mut self, expression: &oxc_ast::ast::CallExpression<'a>) {
        if expression.callee.is_specific_id("eval") {
            self.dynamic_require_write = true;
        }
        walk::walk_call_expression(self, expression);
    }
}
