use lumin_model::{
    EmbeddedSourceUnit, ExportFact, FileFacts, ImportKind, Limitation, LogicalSourceId,
    ModuleRequestKind, SourceKind, SourceSnapshot, SourceSpan, SourceUnitId, SourceUseFact,
    SymbolNamespace,
};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Declaration, ExportNamedDeclaration, ImportDeclaration, ImportDeclarationSpecifier,
    ImportOrExportKind, ModuleExportName, Statement,
};
use oxc_ast::ast_kind::AstKind;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{SourceType, Span};

mod require_scope;

use require_scope::RequireScopeTracker;

const REQUIRE_ATTRIBUTION_OPAQUE: &str = "shadowed, mutated, dynamically resolved, or escaped require makes CommonJS module-use attribution opaque";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JsExtractError {
    kind: SourceKind,
}

impl std::fmt::Display for JsExtractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "source kind {} was routed to the JS owner",
            source_kind_name(self.kind)
        )
    }
}

impl std::error::Error for JsExtractError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsPayloadFacts {
    exports: Vec<ExportTemplate>,
    uses: Vec<SourceUseTemplate>,
    limitation_details: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExportTemplate {
    exported_name: String,
    local_name: Option<String>,
    namespace: SymbolNamespace,
    span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceUseTemplate {
    specifier: String,
    imported_name: Option<String>,
    local_name: Option<String>,
    namespace: SymbolNamespace,
    kind: ImportKind,
    request_kind: ModuleRequestKind,
    span: SourceSpan,
}

pub fn extract(snapshot: &SourceSnapshot) -> Result<FileFacts, JsExtractError> {
    let payload = parse_payload(snapshot.kind, &snapshot.bytes)?;
    Ok(bind_payload(
        &payload,
        &snapshot.id,
        SourceUnitId::Logical(snapshot.id.clone()),
    ))
}

pub fn extract_embedded(unit: &EmbeddedSourceUnit) -> Result<FileFacts, JsExtractError> {
    let payload = parse_payload(unit.kind, &unit.bytes)?;
    Ok(bind_payload(
        &payload,
        &unit.parent_source_id,
        SourceUnitId::Embedded(unit.id.clone()),
    ))
}

pub fn parse_payload(kind: SourceKind, bytes: &[u8]) -> Result<JsPayloadFacts, JsExtractError> {
    if !kind.is_js_family() {
        return Err(JsExtractError { kind });
    }

    let source = match std::str::from_utf8(bytes) {
        Ok(source) => source,
        Err(error) => {
            return Ok(unknown_payload(format!("source is not UTF-8: {error}")));
        }
    };

    let source_type = match source_type(kind) {
        Ok(source_type) => source_type,
        Err(detail) => return Ok(unknown_payload(detail)),
    };

    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.panicked || !parsed.errors.is_empty() {
        let detail = parsed
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Ok(unknown_payload(format!(
            "OXC parse did not complete cleanly: {detail}"
        )));
    }

    let mut facts = JsPayloadFacts {
        exports: Vec::new(),
        uses: Vec::new(),
        limitation_details: Vec::new(),
    };
    if matches!(kind, SourceKind::CommonJs | SourceKind::Cts) {
        facts.limitation_details.push(
            "CommonJS export lowering is not implemented in the first audit increment".to_owned(),
        );
    }
    for statement in &parsed.program.body {
        lower_statement(statement, &mut facts);
    }

    let require_scopes = RequireScopeTracker::analyze(&parsed.program);
    let escaped_require = require_scopes.has_unattributed_require_escape();
    let mut detector = DynamicUseDetector {
        uses: Vec::new(),
        unknown_details: Vec::new(),
        require_scopes,
        opaque_require_reported: false,
    };
    if escaped_require {
        detector.report_opaque_require();
    }
    detector.visit_program(&parsed.program);
    facts.uses.extend(detector.uses);
    facts.limitation_details.extend(detector.unknown_details);
    canonicalize(&mut facts);
    Ok(facts)
}

pub fn bind_payload(
    payload: &JsPayloadFacts,
    source_id: &LogicalSourceId,
    source_unit: SourceUnitId,
) -> FileFacts {
    FileFacts {
        source_id: source_id.clone(),
        source_unit,
        exports: payload
            .exports
            .iter()
            .map(|export| ExportFact {
                source_id: source_id.clone(),
                exported_name: export.exported_name.clone(),
                local_name: export.local_name.clone(),
                namespace: export.namespace,
                span: export.span.clone(),
            })
            .collect(),
        uses: payload
            .uses
            .iter()
            .map(|source_use| SourceUseFact {
                importer: source_id.clone(),
                specifier: source_use.specifier.clone(),
                imported_name: source_use.imported_name.clone(),
                local_name: source_use.local_name.clone(),
                namespace: source_use.namespace,
                kind: source_use.kind,
                request_kind: source_use.request_kind,
                span: source_use.span.clone(),
            })
            .collect(),
        limitations: payload
            .limitation_details
            .iter()
            .map(|detail| Limitation::JsModuleUseUnknown {
                source_id: source_id.clone(),
                detail: detail.clone(),
            })
            .collect(),
    }
}

fn lower_statement(statement: &Statement<'_>, facts: &mut JsPayloadFacts) {
    match statement {
        Statement::ImportDeclaration(declaration) => lower_import(declaration, facts),
        Statement::TSImportEqualsDeclaration(declaration) => {
            lower_import_equals(declaration, facts);
        }
        Statement::ExportNamedDeclaration(declaration) => {
            lower_named_export(declaration, facts);
        }
        Statement::ExportDefaultDeclaration(declaration) => {
            facts.exports.push(ExportTemplate {
                exported_name: "default".to_owned(),
                local_name: None,
                namespace: if matches!(
                    declaration.declaration,
                    oxc_ast::ast::ExportDefaultDeclarationKind::TSInterfaceDeclaration(_)
                ) {
                    SymbolNamespace::Type
                } else {
                    SymbolNamespace::Value
                },
                span: span(declaration.span),
            });
        }
        Statement::ExportAllDeclaration(declaration) => {
            facts.uses.push(SourceUseTemplate {
                specifier: declaration.source.value.to_string(),
                imported_name: None,
                local_name: None,
                namespace: namespace(declaration.export_kind),
                kind: ImportKind::ReExportAll,
                request_kind: ModuleRequestKind::StaticImport,
                span: span(declaration.span),
            });
            facts.limitation_details.push(format!(
                "export-all from {} requires graph expansion not implemented in this increment",
                declaration.source.value
            ));
        }
        Statement::TSExportAssignment(_) | Statement::TSNamespaceExportDeclaration(_) => {
            facts
                .limitation_details
                .push("TypeScript export assignment/namespace export is not lowered".to_owned());
        }
        _ => {}
    }
}

fn lower_import(declaration: &ImportDeclaration<'_>, facts: &mut JsPayloadFacts) {
    let specifier = declaration.source.value.to_string();
    let declaration_namespace = namespace(declaration.import_kind);
    let specifiers = match &declaration.specifiers {
        Some(specifiers) if !specifiers.is_empty() => specifiers,
        _ => {
            facts.uses.push(SourceUseTemplate {
                specifier,
                imported_name: None,
                local_name: None,
                namespace: declaration_namespace,
                kind: ImportKind::SideEffect,
                request_kind: ModuleRequestKind::StaticImport,
                span: span(declaration.span),
            });
            return;
        }
    };

    for import in specifiers {
        match import {
            ImportDeclarationSpecifier::ImportSpecifier(import) => {
                facts.uses.push(SourceUseTemplate {
                    specifier: specifier.clone(),
                    imported_name: Some(module_export_name(&import.imported)),
                    local_name: Some(import.local.name.to_string()),
                    namespace: if declaration.import_kind == ImportOrExportKind::Type
                        || import.import_kind == ImportOrExportKind::Type
                    {
                        SymbolNamespace::Type
                    } else {
                        SymbolNamespace::Value
                    },
                    kind: ImportKind::Named,
                    request_kind: ModuleRequestKind::StaticImport,
                    span: span(import.span),
                });
            }
            ImportDeclarationSpecifier::ImportDefaultSpecifier(import) => {
                facts.uses.push(SourceUseTemplate {
                    specifier: specifier.clone(),
                    imported_name: Some("default".to_owned()),
                    local_name: Some(import.local.name.to_string()),
                    namespace: declaration_namespace,
                    kind: ImportKind::Default,
                    request_kind: ModuleRequestKind::StaticImport,
                    span: span(import.span),
                });
            }
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(import) => {
                facts.uses.push(SourceUseTemplate {
                    specifier: specifier.clone(),
                    imported_name: None,
                    local_name: Some(import.local.name.to_string()),
                    namespace: declaration_namespace,
                    kind: ImportKind::Namespace,
                    request_kind: ModuleRequestKind::StaticImport,
                    span: span(import.span),
                });
            }
        }
    }
}

fn lower_named_export(declaration: &ExportNamedDeclaration<'_>, facts: &mut JsPayloadFacts) {
    if let Some(inner) = &declaration.declaration {
        lower_declaration(inner, facts);
    }

    for export in &declaration.specifiers {
        let namespace = if declaration.export_kind == ImportOrExportKind::Type
            || export.export_kind == ImportOrExportKind::Type
        {
            SymbolNamespace::Type
        } else {
            SymbolNamespace::Value
        };
        let exported_name = module_export_name(&export.exported);
        let local_name = module_export_name(&export.local);
        facts.exports.push(ExportTemplate {
            exported_name,
            local_name: Some(local_name.clone()),
            namespace,
            span: span(export.span),
        });
        if let Some(source) = &declaration.source {
            facts.uses.push(SourceUseTemplate {
                specifier: source.value.to_string(),
                imported_name: Some(local_name),
                local_name: None,
                namespace,
                kind: ImportKind::ReExportNamed,
                request_kind: ModuleRequestKind::StaticImport,
                span: span(export.span),
            });
        }
    }

    if declaration.specifiers.is_empty()
        && let Some(source) = &declaration.source
    {
        facts.uses.push(SourceUseTemplate {
            specifier: source.value.to_string(),
            imported_name: None,
            local_name: None,
            namespace: namespace(declaration.export_kind),
            kind: ImportKind::SideEffect,
            request_kind: ModuleRequestKind::StaticImport,
            span: span(declaration.span),
        });
    }
}

fn lower_import_equals(
    declaration: &oxc_ast::ast::TSImportEqualsDeclaration<'_>,
    facts: &mut JsPayloadFacts,
) {
    if declaration.import_kind == ImportOrExportKind::Value
        && let oxc_ast::ast::TSModuleReference::ExternalModuleReference(reference) =
            &declaration.module_reference
    {
        facts.uses.push(SourceUseTemplate {
            specifier: reference.expression.value.to_string(),
            imported_name: None,
            local_name: Some(declaration.id.name.to_string()),
            namespace: SymbolNamespace::Value,
            kind: ImportKind::Namespace,
            request_kind: ModuleRequestKind::Require,
            span: span(declaration.span),
        });
        return;
    }

    facts.limitation_details.push(
        "non-external or type-only TypeScript import-equals declaration is not lowered".to_owned(),
    );
}

fn lower_declaration(declaration: &Declaration<'_>, facts: &mut JsPayloadFacts) {
    match declaration {
        Declaration::VariableDeclaration(declaration) => {
            for declarator in &declaration.declarations {
                for identifier in declarator.id.get_binding_identifiers() {
                    facts.exports.push(ExportTemplate {
                        exported_name: identifier.name.to_string(),
                        local_name: Some(identifier.name.to_string()),
                        namespace: SymbolNamespace::Value,
                        span: span(identifier.span),
                    });
                }
            }
        }
        Declaration::FunctionDeclaration(declaration) => {
            if let Some(identifier) = &declaration.id {
                push_named_declaration(
                    facts,
                    identifier.name.as_str(),
                    SymbolNamespace::Value,
                    declaration.span,
                );
            }
        }
        Declaration::ClassDeclaration(declaration) => {
            if let Some(identifier) = &declaration.id {
                push_named_declaration(
                    facts,
                    identifier.name.as_str(),
                    SymbolNamespace::Value,
                    declaration.span,
                );
            }
        }
        Declaration::TSTypeAliasDeclaration(declaration) => push_named_declaration(
            facts,
            declaration.id.name.as_str(),
            SymbolNamespace::Type,
            declaration.span,
        ),
        Declaration::TSInterfaceDeclaration(declaration) => push_named_declaration(
            facts,
            declaration.id.name.as_str(),
            SymbolNamespace::Type,
            declaration.span,
        ),
        Declaration::TSEnumDeclaration(declaration) => {
            push_named_declaration(
                facts,
                declaration.id.name.as_str(),
                SymbolNamespace::Value,
                declaration.span,
            );
            push_named_declaration(
                facts,
                declaration.id.name.as_str(),
                SymbolNamespace::Type,
                declaration.span,
            );
        }
        Declaration::TSImportEqualsDeclaration(declaration) => {
            if declaration.import_kind == ImportOrExportKind::Value
                && matches!(
                    declaration.module_reference,
                    oxc_ast::ast::TSModuleReference::ExternalModuleReference(_)
                )
            {
                push_named_declaration(
                    facts,
                    declaration.id.name.as_str(),
                    SymbolNamespace::Value,
                    declaration.span,
                );
            }
            lower_import_equals(declaration, facts);
        }
        Declaration::TSModuleDeclaration(_) | Declaration::TSGlobalDeclaration(_) => {
            facts
                .limitation_details
                .push("TypeScript module/global declaration is not lowered".to_owned());
        }
    }
}

fn push_named_declaration(
    facts: &mut JsPayloadFacts,
    name: &str,
    namespace: SymbolNamespace,
    declaration_span: Span,
) {
    facts.exports.push(ExportTemplate {
        exported_name: name.to_owned(),
        local_name: Some(name.to_owned()),
        namespace,
        span: span(declaration_span),
    });
}

struct DynamicUseDetector {
    uses: Vec<SourceUseTemplate>,
    unknown_details: Vec<String>,
    require_scopes: RequireScopeTracker,
    opaque_require_reported: bool,
}

impl DynamicUseDetector {
    fn require_is_opaque(&self) -> bool {
        self.require_scopes.require_is_opaque()
    }

    fn report_opaque_require(&mut self) {
        if !self.opaque_require_reported {
            self.unknown_details
                .push(REQUIRE_ATTRIBUTION_OPAQUE.to_owned());
            self.opaque_require_reported = true;
        }
    }
}

impl<'a> Visit<'a> for DynamicUseDetector {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        self.require_scopes.enter_node(kind);
    }

    fn leave_node(&mut self, kind: AstKind<'a>) {
        self.require_scopes.leave_node(kind);
    }

    fn visit_import_expression(&mut self, expression: &oxc_ast::ast::ImportExpression<'a>) {
        match &expression.source {
            oxc_ast::ast::Expression::StringLiteral(source) => {
                self.uses.push(SourceUseTemplate {
                    specifier: source.value.to_string(),
                    imported_name: None,
                    local_name: None,
                    namespace: SymbolNamespace::Value,
                    kind: ImportKind::DynamicBroad,
                    request_kind: ModuleRequestKind::DynamicImport,
                    span: span(expression.span),
                });
            }
            _ => self
                .unknown_details
                .push("nonliteral dynamic import may hide an internal consumer".to_owned()),
        }
        walk::walk_import_expression(self, expression);
    }

    fn visit_call_expression(&mut self, expression: &oxc_ast::ast::CallExpression<'a>) {
        if expression.callee.is_specific_id("require") && self.require_is_opaque() {
            self.report_opaque_require();
        } else if let Some(source) = expression.common_js_require() {
            self.uses.push(SourceUseTemplate {
                specifier: source.value.to_string(),
                imported_name: None,
                local_name: None,
                namespace: SymbolNamespace::Value,
                kind: ImportKind::DynamicBroad,
                request_kind: ModuleRequestKind::Require,
                span: span(expression.span),
            });
        } else if expression.callee.is_specific_id("require") {
            self.unknown_details
                .push("nonliteral CommonJS require may hide an internal consumer".to_owned());
        } else if is_import_meta_glob(&expression.callee) {
            self.unknown_details.push(
                "import.meta.glob target expansion is not implemented in this increment".to_owned(),
            );
        }
        walk::walk_call_expression(self, expression);
    }
}

fn is_import_meta_glob(expression: &oxc_ast::ast::Expression<'_>) -> bool {
    let Some(member) = expression.as_member_expression() else {
        return false;
    };
    if member.static_property_name() != Some("glob") {
        return false;
    }
    matches!(
        member.object(),
        oxc_ast::ast::Expression::MetaProperty(meta)
            if meta.meta.name == "import" && meta.property.name == "meta"
    )
}

fn source_type(kind: SourceKind) -> Result<SourceType, String> {
    let synthetic_name = match kind {
        SourceKind::JavaScript => "source.js",
        SourceKind::Jsx => "source.jsx",
        SourceKind::Mjs => "source.mjs",
        SourceKind::CommonJs => "source.cjs",
        SourceKind::TypeScript => "source.ts",
        SourceKind::Tsx => "source.tsx",
        SourceKind::Mts => "source.mts",
        SourceKind::Cts => "source.cts",
        SourceKind::DeclarationTs => "source.d.ts",
        SourceKind::DeclarationMts => "source.d.mts",
        SourceKind::DeclarationCts => "source.d.cts",
        SourceKind::Vue | SourceKind::Svelte | SourceKind::Astro => {
            return Err("SFC source was routed to the JS owner".to_owned());
        }
    };
    SourceType::from_path(synthetic_name)
        .map_err(|error| format!("OXC source type selection failed: {error}"))
}

fn source_kind_name(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Vue => "vue",
        SourceKind::Svelte => "svelte",
        SourceKind::Astro => "astro",
        _ => "javascript-typescript",
    }
}

fn namespace(kind: ImportOrExportKind) -> SymbolNamespace {
    match kind {
        ImportOrExportKind::Value => SymbolNamespace::Value,
        ImportOrExportKind::Type => SymbolNamespace::Type,
    }
}

fn module_export_name(name: &ModuleExportName<'_>) -> String {
    match name {
        ModuleExportName::IdentifierName(identifier) => identifier.name.to_string(),
        ModuleExportName::IdentifierReference(identifier) => identifier.name.to_string(),
        ModuleExportName::StringLiteral(value) => value.value.to_string(),
    }
}

fn span(value: Span) -> SourceSpan {
    SourceSpan {
        start: value.start,
        end: value.end,
    }
}

fn unknown_payload(detail: String) -> JsPayloadFacts {
    JsPayloadFacts {
        exports: Vec::new(),
        uses: Vec::new(),
        limitation_details: vec![detail],
    }
}

fn canonicalize(facts: &mut JsPayloadFacts) {
    facts.exports.sort_by(|left, right| {
        left.namespace
            .cmp(&right.namespace)
            .then_with(|| left.exported_name.cmp(&right.exported_name))
            .then_with(|| left.span.start.cmp(&right.span.start))
            .then_with(|| left.span.end.cmp(&right.span.end))
    });
    facts.uses.sort_by(|left, right| {
        left.specifier
            .cmp(&right.specifier)
            .then_with(|| left.namespace.cmp(&right.namespace))
            .then_with(|| left.imported_name.cmp(&right.imported_name))
            .then_with(|| left.span.start.cmp(&right.span.start))
            .then_with(|| left.span.end.cmp(&right.span.end))
    });
}

#[cfg(test)]
mod tests;
