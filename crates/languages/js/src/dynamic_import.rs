use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{ImportKind, ModuleRequestKind, SourceSpan, SymbolNamespace};
use oxc_ast::ast::{
    Argument, BindingIdentifier, BindingPattern, CallExpression, ClassType, Expression,
    FunctionType, ImportExpression, JSXMemberExpression, JSXMemberExpressionObject,
    MemberExpression, VariableDeclarator, WithStatement,
};
use oxc_ast::ast_kind::AstKind;
use oxc_ast_visit::{Visit, walk};
use oxc_span::GetSpan;

use super::SourceUseTemplate;

type SpanKey = (u32, u32);

#[derive(Default)]
pub(super) struct DynamicImportAnalysis {
    pub(super) uses: Vec<SourceUseTemplate>,
    pub(super) handled_imports: BTreeSet<SpanKey>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Binding {
    Dynamic(usize),
    Other,
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
struct Scope {
    parent: Option<usize>,
    kind: ScopeKind,
    bindings: BTreeMap<String, Binding>,
    dynamic_lookup: bool,
}

#[derive(Debug)]
struct DynamicRecord {
    specifier: String,
    import_span: SourceSpan,
    local_name: Option<String>,
    members: BTreeSet<(String, u32, u32)>,
    broad: bool,
}

impl DynamicRecord {
    fn new(
        specifier: &str,
        import_expression: &ImportExpression<'_>,
        local_name: Option<String>,
    ) -> Self {
        Self {
            specifier: specifier.to_owned(),
            import_span: source_span(import_expression.span),
            local_name,
            members: BTreeSet::new(),
            broad: false,
        }
    }
}

pub(super) fn analyze_literal_dynamic_imports(
    program: &oxc_ast::ast::Program<'_>,
) -> DynamicImportAnalysis {
    let mut collector = BindingCollector::default();
    collector.visit_program(program);
    if !collector.complete() {
        return DynamicImportAnalysis::default();
    }

    let BindingCollector {
        scopes,
        mut records,
        handled_imports,
        tracking_failed: _,
        scope_stack: _,
        binding_context: _,
        special_bindings: _,
    } = collector;
    let mut analyzer = UseAnalyzer::new(&scopes, &mut records);
    analyzer.visit_program(program);
    if !analyzer.complete() {
        return DynamicImportAnalysis::default();
    }

    DynamicImportAnalysis {
        uses: emit_uses(records),
        handled_imports,
    }
}

#[derive(Default)]
struct BindingCollector {
    scopes: Vec<Scope>,
    scope_stack: Vec<usize>,
    records: Vec<DynamicRecord>,
    handled_imports: BTreeSet<SpanKey>,
    special_bindings: BTreeMap<SpanKey, usize>,
    binding_context: Option<(usize, Binding)>,
    tracking_failed: bool,
}

impl BindingCollector {
    fn complete(&self) -> bool {
        !self.tracking_failed && self.scope_stack.is_empty() && !self.scopes.is_empty()
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

    fn push_scope(&mut self, kind: ScopeKind, dynamic_lookup: bool) {
        let index = self.scopes.len();
        self.scopes.push(Scope {
            parent: self.scope_stack.last().copied(),
            kind,
            bindings: BTreeMap::new(),
            dynamic_lookup,
        });
        self.scope_stack.push(index);
    }

    fn pop_scope(&mut self) {
        if self.scope_stack.pop().is_none() {
            self.tracking_failed = true;
        }
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
        let Some((import_expression, specifier, binding)) = then_callback_binding(expression)
        else {
            return;
        };
        let record = self.add_record(specifier, import_expression, Some(binding.name.to_string()));
        let key = span_key(binding.span);
        if let Some(previous) = self.special_bindings.insert(key, record) {
            self.degrade_binding(Binding::Dynamic(previous));
            self.degrade_binding(Binding::Dynamic(record));
        }
    }

    fn collect_immediate_member(&mut self, expression: &MemberExpression<'_>) {
        let Some((import_expression, specifier)) = awaited_literal_import(expression.object())
        else {
            return;
        };
        let Some(property) = expression.static_property_name() else {
            return;
        };
        let record = self.add_record(specifier, import_expression, None);
        if let Some(record) = self.records.get_mut(record) {
            record.members.insert((
                property.to_owned(),
                expression.span().start,
                expression.span().end,
            ));
        }
    }
}

impl<'a> Visit<'a> for BindingCollector {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        match kind {
            AstKind::Function(function) => {
                if matches!(
                    function.r#type,
                    FunctionType::FunctionDeclaration | FunctionType::TSDeclareFunction
                ) && let Some(identifier) = &function.id
                {
                    self.declare_outer_definition(identifier.name.as_str(), true);
                }
            }
            AstKind::Class(class) => {
                if class.r#type == ClassType::ClassDeclaration
                    && let Some(identifier) = &class.id
                {
                    self.declare_outer_definition(identifier.name.as_str(), false);
                }
            }
            _ => {}
        }
        if let Some(kind) = scope_kind(kind) {
            self.push_scope(kind, false);
        }
    }

    fn leave_node(&mut self, kind: AstKind<'a>) {
        if scope_kind(kind).is_some() {
            self.pop_scope();
        }
    }

    fn visit_binding_identifier(&mut self, identifier: &BindingIdentifier<'a>) {
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
            binding = Binding::Dynamic(record);
        }

        let previous = self.binding_context;
        if let Some(target_scope) = target_scope {
            self.binding_context = Some((target_scope, binding));
        }
        self.visit_binding_pattern(&declarator.id);
        self.binding_context = previous;
        if let Some(type_annotation) = &declarator.type_annotation {
            self.visit_ts_type_annotation(type_annotation);
        }
        if let Some(initializer) = &declarator.init {
            self.visit_expression(initializer);
        }
    }

    fn visit_call_expression(&mut self, expression: &CallExpression<'a>) {
        self.collect_then_binding(expression);
        walk::walk_call_expression(self, expression);
    }

    fn visit_member_expression(&mut self, expression: &MemberExpression<'a>) {
        self.collect_immediate_member(expression);
        walk::walk_member_expression(self, expression);
    }

    fn visit_with_statement(&mut self, statement: &WithStatement<'a>) {
        self.visit_expression(&statement.object);
        self.push_scope(ScopeKind::Lexical, true);
        self.visit_statement(&statement.body);
        self.pop_scope();
    }
}

struct UseAnalyzer<'m, 'r> {
    scopes: &'m [Scope],
    records: &'r mut [DynamicRecord],
    scope_stack: Vec<usize>,
    next_scope: usize,
    handled_member_objects: BTreeSet<SpanKey>,
    tracking_failed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Resolution {
    Dynamic(usize),
    AmbiguousDynamic(usize),
    Other,
    Unbound,
}

impl<'m, 'r> UseAnalyzer<'m, 'r> {
    fn new(scopes: &'m [Scope], records: &'r mut [DynamicRecord]) -> Self {
        Self {
            scopes,
            records,
            scope_stack: Vec::new(),
            next_scope: 0,
            handled_member_objects: BTreeSet::new(),
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

    fn resolve(&mut self, name: &str) -> Resolution {
        let Some(mut cursor) = self.scope_stack.last().copied() else {
            self.tracking_failed = true;
            return Resolution::Unbound;
        };
        let mut dynamic = false;
        loop {
            let Some(scope) = self.scopes.get(cursor) else {
                self.tracking_failed = true;
                return Resolution::Unbound;
            };
            if let Some(binding) = scope.bindings.get(name) {
                return match binding {
                    Binding::Dynamic(record) if dynamic => Resolution::AmbiguousDynamic(*record),
                    Binding::Dynamic(record) => Resolution::Dynamic(*record),
                    Binding::Other => Resolution::Other,
                };
            }
            dynamic |= scope.dynamic_lookup;
            let Some(parent) = scope.parent else {
                return Resolution::Unbound;
            };
            cursor = parent;
        }
    }

    fn degrade(&mut self, record: usize) {
        if let Some(record) = self.records.get_mut(record) {
            record.broad = true;
        } else {
            self.tracking_failed = true;
        }
    }

    fn degrade_all(&mut self) {
        for record in &mut *self.records {
            record.broad = true;
        }
    }

    fn record_member(&mut self, record: usize, member: &str, member_span: SourceSpan) {
        if let Some(record) = self.records.get_mut(record) {
            record
                .members
                .insert((member.to_owned(), member_span.start, member_span.end));
        } else {
            self.tracking_failed = true;
        }
    }

    fn inspect_member_object(
        &mut self,
        identifier: &oxc_ast::ast::IdentifierReference<'_>,
        property: Option<&str>,
        member_span: SourceSpan,
    ) {
        match self.resolve(identifier.name.as_str()) {
            Resolution::Dynamic(record) => {
                self.handled_member_objects
                    .insert(span_key(identifier.span));
                if let Some(property) = property {
                    self.record_member(record, property, member_span);
                } else {
                    self.degrade(record);
                }
            }
            Resolution::AmbiguousDynamic(record) => {
                self.handled_member_objects
                    .insert(span_key(identifier.span));
                self.degrade(record);
            }
            Resolution::Other | Resolution::Unbound => {}
        }
    }
}

impl<'a> Visit<'a> for UseAnalyzer<'_, '_> {
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

    fn visit_member_expression(&mut self, expression: &MemberExpression<'a>) {
        if let Some(identifier) = expression
            .object()
            .without_parentheses()
            .get_identifier_reference()
        {
            self.inspect_member_object(
                identifier,
                expression.static_property_name(),
                source_span(expression.span()),
            );
        }
        walk::walk_member_expression(self, expression);
    }

    fn visit_call_expression(&mut self, expression: &CallExpression<'a>) {
        if !expression.optional
            && expression.callee.is_specific_id("eval")
            && matches!(
                self.resolve("eval"),
                Resolution::Unbound | Resolution::AmbiguousDynamic(_)
            )
        {
            self.degrade_all();
        }
        walk::walk_call_expression(self, expression);
    }

    fn visit_jsx_member_expression(&mut self, expression: &JSXMemberExpression<'a>) {
        if let JSXMemberExpressionObject::IdentifierReference(identifier) = &expression.object {
            self.inspect_member_object(
                identifier,
                Some(expression.property.name.as_str()),
                source_span(expression.span),
            );
        }
        walk::walk_jsx_member_expression(self, expression);
    }

    fn visit_identifier_reference(&mut self, identifier: &oxc_ast::ast::IdentifierReference<'a>) {
        if !self
            .handled_member_objects
            .contains(&span_key(identifier.span))
        {
            match self.resolve(identifier.name.as_str()) {
                Resolution::Dynamic(record) | Resolution::AmbiguousDynamic(record) => {
                    self.degrade(record);
                }
                Resolution::Other | Resolution::Unbound => {}
            }
        }
        walk::walk_identifier_reference(self, identifier);
    }

    fn visit_with_statement(&mut self, statement: &WithStatement<'a>) {
        self.visit_expression(&statement.object);
        self.push_next_scope(true);
        self.visit_statement(&statement.body);
        self.pop_scope();
    }
}

fn emit_uses(records: Vec<DynamicRecord>) -> Vec<SourceUseTemplate> {
    let mut uses = Vec::new();
    for record in records {
        if record.broad || record.members.is_empty() {
            uses.push(SourceUseTemplate {
                specifier: record.specifier,
                imported_name: None,
                local_name: record.local_name,
                namespace: SymbolNamespace::Value,
                kind: ImportKind::DynamicBroad,
                request_kind: ModuleRequestKind::DynamicImport,
                span: record.import_span,
            });
            continue;
        }
        for (member, start, end) in record.members {
            uses.push(SourceUseTemplate {
                specifier: record.specifier.clone(),
                imported_name: Some(member),
                local_name: record.local_name.clone(),
                namespace: SymbolNamespace::Value,
                kind: ImportKind::Named,
                request_kind: ModuleRequestKind::DynamicImport,
                span: SourceSpan { start, end },
            });
        }
    }
    uses
}

fn then_callback_binding<'a>(
    expression: &'a CallExpression<'a>,
) -> Option<(&'a ImportExpression<'a>, &'a str, &'a BindingIdentifier<'a>)> {
    let member = expression
        .callee
        .without_parentheses()
        .as_member_expression()?;
    if member.static_property_name() != Some("then") {
        return None;
    }
    let (import_expression, specifier) = literal_import(member.object())?;
    let callback = expression.arguments.first()?;
    let parameters = match callback {
        Argument::ArrowFunctionExpression(callback) => &callback.params,
        Argument::FunctionExpression(callback) => &callback.params,
        _ => return None,
    };
    let first = parameters.items.first()?;
    let BindingPattern::BindingIdentifier(identifier) = &first.pattern else {
        return None;
    };
    Some((import_expression, specifier, identifier))
}

fn awaited_literal_import<'a>(
    expression: &'a Expression<'a>,
) -> Option<(&'a ImportExpression<'a>, &'a str)> {
    let Expression::AwaitExpression(await_expression) = expression.without_parentheses() else {
        return None;
    };
    literal_import(&await_expression.argument)
}

fn literal_import<'a>(
    expression: &'a Expression<'a>,
) -> Option<(&'a ImportExpression<'a>, &'a str)> {
    let Expression::ImportExpression(import_expression) = expression.without_parentheses() else {
        return None;
    };
    if import_expression.options.is_some() || import_expression.phase.is_some() {
        return None;
    }
    let Expression::StringLiteral(source) = import_expression.source.without_parentheses() else {
        return None;
    };
    Some((import_expression, source.value.as_str()))
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

fn source_span(span: oxc_span::Span) -> SourceSpan {
    SourceSpan {
        start: span.start,
        end: span.end,
    }
}

fn span_key(span: oxc_span::Span) -> SpanKey {
    (span.start, span.end)
}
