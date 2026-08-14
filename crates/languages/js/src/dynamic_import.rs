use std::collections::BTreeSet;

use lumin_model::{ImportKind, ModuleRequestKind, SourceSpan, SymbolNamespace};
use oxc_ast::ast::{
    Argument, BindingIdentifier, BindingPattern, CallExpression, ExportNamedDeclaration,
    ExportSpecifier, Expression, ImportExpression, ImportOrExportKind, JSXElementName,
    JSXMemberExpression, JSXMemberExpressionObject, MemberExpression, ModuleExportName,
    TSClassImplements, TSEnumDeclaration, TSInterfaceHeritage, TSType, TaggedTemplateExpression,
    VariableDeclaration, WithStatement,
};
use oxc_ast::ast_kind::AstKind;
use oxc_ast_visit::{Visit, walk};
use oxc_span::GetSpan;

use super::SourceUseTemplate;

mod bindings;
mod nonliteral;

use bindings::{Binding, BindingCollection, Scope, ScopeKind, collect as collect_bindings};
pub(crate) use nonliteral::scope_limitations;
pub(super) use nonliteral::{NonLiteralDynamicImportTemplate, nonliteral_template};

type SpanKey = (u32, u32);

#[derive(Default)]
pub(super) struct DynamicImportAnalysis {
    pub(super) uses: Vec<SourceUseTemplate>,
    pub(super) handled_imports: BTreeSet<SpanKey>,
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
    let Some(BindingCollection {
        scopes,
        mut records,
        handled_imports,
    }) = collect_bindings(program)
    else {
        return DynamicImportAnalysis::default();
    };
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

struct UseAnalyzer<'m, 'r> {
    scopes: &'m [Scope],
    records: &'r mut [DynamicRecord],
    scope_stack: Vec<usize>,
    next_scope: usize,
    handled_member_objects: BTreeSet<SpanKey>,
    handled_arguments_objects: BTreeSet<SpanKey>,
    type_only_depth: usize,
    tracking_failed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Resolution {
    Dynamic(usize),
    AmbiguousDynamic(usize),
    AmbiguousOther,
    Other,
    Unbound,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MappedArgumentProperty {
    Index(usize),
    KnownNonIndex,
    Dynamic,
}

impl<'m, 'r> UseAnalyzer<'m, 'r> {
    fn new(scopes: &'m [Scope], records: &'r mut [DynamicRecord]) -> Self {
        Self {
            scopes,
            records,
            scope_stack: Vec::new(),
            next_scope: 0,
            handled_member_objects: BTreeSet::new(),
            handled_arguments_objects: BTreeSet::new(),
            type_only_depth: 0,
            tracking_failed: false,
        }
    }

    fn complete(&self) -> bool {
        !self.tracking_failed
            && self.scope_stack.is_empty()
            && self.next_scope == self.scopes.len()
            && self.type_only_depth == 0
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
        let ambient = scope.ambient;
        self.scope_stack.push(self.next_scope);
        self.next_scope += 1;
        if ambient {
            self.enter_type_only();
        }
    }

    fn pop_scope(&mut self) {
        let Some(scope) = self.scope_stack.last().copied() else {
            self.tracking_failed = true;
            return;
        };
        if self.scopes.get(scope).is_some_and(|scope| scope.ambient) {
            self.leave_type_only();
        }
        self.scope_stack.pop();
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
                    Binding::Other if dynamic => Resolution::AmbiguousOther,
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
            Resolution::AmbiguousOther | Resolution::Other | Resolution::Unbound => {}
        }
    }

    fn degrade_identifier(&mut self, identifier: &oxc_ast::ast::IdentifierReference<'_>) {
        match self.resolve(identifier.name.as_str()) {
            Resolution::Dynamic(record) | Resolution::AmbiguousDynamic(record) => {
                self.degrade(record);
            }
            Resolution::AmbiguousOther | Resolution::Other | Resolution::Unbound => {}
        }
    }

    fn degrade_member_receiver(&mut self, member: &MemberExpression<'_>) {
        if let Some(identifier) =
            transparent_runtime_expression(member.object()).get_identifier_reference()
        {
            self.degrade_identifier(identifier);
        }
    }

    fn degrade_mapped_arguments_member(&mut self, member: &MemberExpression<'_>) {
        let Some(identifier) = transparent_runtime_expression(member.object())
            .get_identifier_reference()
            .filter(|identifier| identifier.name == "arguments")
        else {
            return;
        };
        self.handled_arguments_objects
            .insert(span_key(identifier.span));
        if !member.is_computed() {
            return;
        }
        let Some((body_scope, names)) = self.mapped_arguments() else {
            return;
        };
        let selected = match mapped_argument_property(member) {
            MappedArgumentProperty::Index(index) => names
                .get(index)
                .and_then(Clone::clone)
                .into_iter()
                .collect(),
            MappedArgumentProperty::KnownNonIndex => Vec::new(),
            MappedArgumentProperty::Dynamic => names.into_iter().flatten().collect::<Vec<_>>(),
        };
        self.degrade_mapped_argument_bindings(body_scope, selected);
    }

    fn degrade_all_mapped_arguments(&mut self) {
        if let Some((body_scope, names)) = self.mapped_arguments() {
            self.degrade_mapped_argument_bindings(body_scope, names.into_iter().flatten());
        }
    }

    fn degrade_mapped_argument_bindings(
        &mut self,
        body_scope: usize,
        names: impl IntoIterator<Item = String>,
    ) {
        let Some(scope) = self.scopes.get(body_scope) else {
            self.tracking_failed = true;
            return;
        };
        let records = names
            .into_iter()
            .filter_map(|name| match scope.bindings.get(&name) {
                Some(Binding::Dynamic(record)) => Some(*record),
                Some(Binding::Other) | None => None,
            })
            .collect::<BTreeSet<_>>();
        for record in records {
            self.degrade(record);
        }
    }

    fn mapped_arguments(&mut self) -> Option<(usize, Vec<Option<String>>)> {
        let Some(mut cursor) = self.scope_stack.last().copied() else {
            self.tracking_failed = true;
            return None;
        };
        let mut child: Option<usize> = None;
        loop {
            let Some(scope) = self.scopes.get(cursor) else {
                self.tracking_failed = true;
                return None;
            };
            if scope.bindings.contains_key("arguments") {
                return None;
            }
            if scope.kind == ScopeKind::FunctionParameters
                && let Some(names) = &scope.mapped_parameter_names
            {
                let Some(body_scope) = child.filter(|child| {
                    self.scopes.get(*child).is_some_and(|scope| {
                        scope.parent == Some(cursor) && scope.kind == ScopeKind::VarEnvironment
                    })
                }) else {
                    if !names.is_empty() {
                        self.tracking_failed = true;
                    }
                    return None;
                };
                return Some((body_scope, names.clone()));
            }
            let parent = scope.parent?;
            child = Some(cursor);
            cursor = parent;
        }
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
        if self.type_only_depth == 0 {
            self.degrade_mapped_arguments_member(expression);
        }
        if let Some(identifier) =
            transparent_runtime_expression(expression.object()).get_identifier_reference()
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
        let callee = transparent_runtime_expression(&expression.callee);
        if !expression.optional
            && callee.is_specific_id("eval")
            && matches!(
                self.resolve("eval"),
                Resolution::Unbound | Resolution::AmbiguousDynamic(_) | Resolution::AmbiguousOther
            )
        {
            self.degrade_all();
        }
        if let Some(member) = callee.get_member_expr() {
            self.degrade_member_receiver(member);
        }
        walk::walk_call_expression(self, expression);
    }

    fn visit_tagged_template_expression(&mut self, expression: &TaggedTemplateExpression<'a>) {
        if let Some(member) = transparent_runtime_expression(&expression.tag).get_member_expr() {
            self.degrade_member_receiver(member);
        }
        walk::walk_tagged_template_expression(self, expression);
    }

    fn visit_export_named_declaration(&mut self, declaration: &ExportNamedDeclaration<'a>) {
        if self.type_only_depth == 0
            && declaration.source.is_none()
            && declaration.export_kind == ImportOrExportKind::Value
        {
            for specifier in &declaration.specifiers {
                if specifier.export_kind == ImportOrExportKind::Value
                    && let ModuleExportName::IdentifierReference(identifier) = &specifier.local
                {
                    self.degrade_identifier(identifier);
                }
            }
        }
        if declaration.export_kind == ImportOrExportKind::Type {
            self.enter_type_only();
        }
        walk::walk_export_named_declaration(self, declaration);
        if declaration.export_kind == ImportOrExportKind::Type {
            self.leave_type_only();
        }
    }

    fn visit_export_specifier(&mut self, specifier: &ExportSpecifier<'a>) {
        if specifier.export_kind == ImportOrExportKind::Type {
            self.enter_type_only();
        }
        walk::walk_export_specifier(self, specifier);
        if specifier.export_kind == ImportOrExportKind::Type {
            self.leave_type_only();
        }
    }

    fn visit_jsx_element_name(&mut self, name: &JSXElementName<'a>) {
        if let JSXElementName::IdentifierReference(identifier) = name {
            self.degrade_identifier(identifier);
        }
        walk::walk_jsx_element_name(self, name);
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
        if self.type_only_depth == 0 {
            if identifier.name == "arguments"
                && !self
                    .handled_arguments_objects
                    .contains(&span_key(identifier.span))
            {
                self.degrade_all_mapped_arguments();
            }
            if !self
                .handled_member_objects
                .contains(&span_key(identifier.span))
            {
                self.degrade_identifier(identifier);
            }
        }
        walk::walk_identifier_reference(self, identifier);
    }

    fn visit_ts_type(&mut self, ty: &TSType<'a>) {
        self.enter_type_only();
        walk::walk_ts_type(self, ty);
        self.leave_type_only();
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

    fn visit_ts_class_implements(&mut self, implementation: &TSClassImplements<'a>) {
        self.enter_type_only();
        walk::walk_ts_class_implements(self, implementation);
        self.leave_type_only();
    }

    fn visit_ts_interface_heritage(&mut self, heritage: &TSInterfaceHeritage<'a>) {
        self.enter_type_only();
        walk::walk_ts_interface_heritage(self, heritage);
        self.leave_type_only();
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
        if !record.members.iter().any(|(member, _, _)| member == "then") {
            uses.push(SourceUseTemplate {
                specifier: record.specifier.clone(),
                imported_name: Some("then".to_owned()),
                local_name: None,
                namespace: SymbolNamespace::Value,
                kind: ImportKind::Named,
                request_kind: ModuleRequestKind::DynamicImport,
                span: record.import_span.clone(),
            });
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

fn transparent_runtime_expression<'a, 'b>(expression: &'b Expression<'a>) -> &'b Expression<'a> {
    let expression = expression.without_parentheses();
    match expression {
        Expression::TSAsExpression(wrapper) => transparent_runtime_expression(&wrapper.expression),
        Expression::TSSatisfiesExpression(wrapper) => {
            transparent_runtime_expression(&wrapper.expression)
        }
        Expression::TSTypeAssertion(wrapper) => transparent_runtime_expression(&wrapper.expression),
        Expression::TSNonNullExpression(wrapper) => {
            transparent_runtime_expression(&wrapper.expression)
        }
        Expression::TSInstantiationExpression(wrapper) => {
            transparent_runtime_expression(&wrapper.expression)
        }
        _ => expression,
    }
}

fn then_callback_binding<'a>(
    expression: &'a CallExpression<'a>,
) -> Option<(
    &'a ImportExpression<'a>,
    &'a str,
    &'a BindingIdentifier<'a>,
    bool,
)> {
    let member = transparent_runtime_expression(&expression.callee).get_member_expr()?;
    if member.static_property_name() != Some("then") {
        return None;
    }
    let (import_expression, specifier) = literal_import(member.object())?;
    let callback = expression.arguments.first()?;
    let (parameters, callback_arguments_are_visible) = match callback {
        Argument::ArrowFunctionExpression(callback) => (&callback.params, false),
        Argument::FunctionExpression(callback) => (&callback.params, true),
        _ => return None,
    };
    let first = parameters.items.first()?;
    let BindingPattern::BindingIdentifier(identifier) = &first.pattern else {
        return None;
    };
    Some((
        import_expression,
        specifier,
        identifier,
        callback_arguments_are_visible,
    ))
}

fn awaited_literal_import<'a>(
    expression: &'a Expression<'a>,
) -> Option<(&'a ImportExpression<'a>, &'a str)> {
    let Expression::AwaitExpression(await_expression) = transparent_runtime_expression(expression)
    else {
        return None;
    };
    literal_import(&await_expression.argument)
}

fn literal_import<'a>(
    expression: &'a Expression<'a>,
) -> Option<(&'a ImportExpression<'a>, &'a str)> {
    let Expression::ImportExpression(import_expression) =
        transparent_runtime_expression(expression)
    else {
        return None;
    };
    if import_expression.options.is_some() || import_expression.phase.is_some() {
        return None;
    }
    Some((
        import_expression,
        literal_dynamic_import_specifier(&import_expression.source)?,
    ))
}

pub(super) fn literal_dynamic_import_specifier<'a>(
    expression: &'a Expression<'a>,
) -> Option<&'a str> {
    match transparent_runtime_expression(expression) {
        Expression::StringLiteral(source) => Some(source.value.as_str()),
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => template
            .quasis
            .first()
            .and_then(|quasi| quasi.value.cooked.as_ref())
            .map(|value| value.as_str()),
        _ => None,
    }
}

fn mapped_argument_property(member: &MemberExpression<'_>) -> MappedArgumentProperty {
    let MemberExpression::ComputedMemberExpression(member) = member else {
        return MappedArgumentProperty::KnownNonIndex;
    };
    if let Some(name) = member.static_property_name() {
        return parse_argument_index(name.as_str()).map_or(
            MappedArgumentProperty::KnownNonIndex,
            MappedArgumentProperty::Index,
        );
    }
    let Expression::NumericLiteral(literal) = member.expression.without_parentheses() else {
        return MappedArgumentProperty::Dynamic;
    };
    if literal.value.is_finite()
        && literal.value >= 0.0
        && literal.value.fract() == 0.0
        && literal.value <= usize::MAX as f64
    {
        MappedArgumentProperty::Index(literal.value as usize)
    } else {
        MappedArgumentProperty::KnownNonIndex
    }
}

fn parse_argument_index(name: &str) -> Option<usize> {
    if name != "0" && name.starts_with('0') {
        return None;
    }
    name.parse().ok()
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
