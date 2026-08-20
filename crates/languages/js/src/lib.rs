use std::collections::BTreeSet;

use lumin_model::{
    DynamicImportTargetScope, EmbeddedSourceUnit, ExportFact, FileFacts, ImportKind,
    ImportMetaGlobTargetScope, InventoryBoundSourceUse, Limitation, LogicalSourceId,
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
use oxc_span::{GetSpan, SourceType, Span};

mod commonjs_computed;
mod dynamic_import;
mod import_meta_glob;
mod require_scope;

pub use commonjs_computed::scope_commonjs_computed_limitations;
use dynamic_import::{
    NonLiteralDynamicImportTemplate, analyze_literal_dynamic_imports,
    literal_dynamic_import_specifier, nonliteral_template, transparent_runtime_expression,
};
use import_meta_glob::{ParsedImportMetaGlob, UnsupportedImportMetaGlobTemplate};
use require_scope::RequireScopeTracker;

pub const EXTRACTOR_SEMANTICS_VERSION: &str = "js-extractor-semantics.v27";

const REQUIRE_ATTRIBUTION_OPAQUE: &str = "shadowed, mutated, dynamically resolved, or escaped require makes CommonJS module-use attribution opaque";
const MODULE_REQUIRE_ATTRIBUTION_OPAQUE: &str =
    "module.require cannot be attributed to the CommonJS wrapper loader";
const COMMONJS_EXPORT_LOWERING_UNSUPPORTED: &str =
    "CommonJS export lowering is not implemented in the first audit increment";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JsExtractError {
    kind: SourceKind,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum JsModuleFormat {
    CommonJs,
    EsModule,
    Unknown,
}

impl JsModuleFormat {
    fn from_source_kind(kind: SourceKind) -> Self {
        match kind {
            SourceKind::CommonJs | SourceKind::Cts | SourceKind::DeclarationCts => Self::CommonJs,
            SourceKind::Mjs | SourceKind::Mts | SourceKind::DeclarationMts => Self::EsModule,
            _ => Self::Unknown,
        }
    }
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
    nonliteral_dynamic_imports: Vec<NonLiteralDynamicImportTemplate>,
    unsupported_import_meta_globs: Vec<UnsupportedImportMetaGlobTemplate>,
    recoverable_parse_details: Vec<String>,
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
    extract_embedded_with_module_format(unit, JsModuleFormat::from_source_kind(unit.kind))
}

pub fn extract_embedded_with_module_format(
    unit: &EmbeddedSourceUnit,
    module_format: JsModuleFormat,
) -> Result<FileFacts, JsExtractError> {
    let payload = parse_payload_with_module_format(unit.kind, &unit.bytes, module_format)?;
    Ok(bind_payload(
        &payload,
        &unit.parent_source_id,
        SourceUnitId::Embedded(unit.id.clone()),
    ))
}

pub fn parse_payload(kind: SourceKind, bytes: &[u8]) -> Result<JsPayloadFacts, JsExtractError> {
    parse_payload_with_module_format(kind, bytes, JsModuleFormat::from_source_kind(kind))
}

pub fn parse_payload_with_module_format(
    kind: SourceKind,
    bytes: &[u8],
    module_format: JsModuleFormat,
) -> Result<JsPayloadFacts, JsExtractError> {
    let variants = parse_payload_with_module_formats(kind, bytes, &[module_format])?;
    match variants.into_iter().next() {
        Some((_format, facts)) => Ok(facts),
        None => Err(JsExtractError { kind }),
    }
}

pub fn parse_payload_with_module_formats(
    kind: SourceKind,
    bytes: &[u8],
    module_formats: &[JsModuleFormat],
) -> Result<Vec<(JsModuleFormat, JsPayloadFacts)>, JsExtractError> {
    if !kind.is_js_family() {
        return Err(JsExtractError { kind });
    }
    let mut module_formats = module_formats.to_vec();
    module_formats.sort_unstable();
    module_formats.dedup();
    if module_formats.is_empty() {
        return Err(JsExtractError { kind });
    }

    let source = match std::str::from_utf8(bytes) {
        Ok(source) => source,
        Err(error) => {
            let detail = format!("source is not UTF-8: {error}");
            return Ok(module_formats
                .into_iter()
                .map(|format| (format, unknown_payload(detail.clone())))
                .collect());
        }
    };

    let source_type = match source_type(kind) {
        Ok(source_type) => source_type,
        Err(detail) => {
            return Ok(module_formats
                .into_iter()
                .map(|format| (format, unknown_payload(detail.clone())))
                .collect());
        }
    };

    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    let parse_errors = parsed
        .errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    if parsed.panicked {
        let detail = if parse_errors.is_empty() {
            "OXC parse terminated before producing a complete AST".to_owned()
        } else {
            format!("OXC parse terminated before producing a complete AST: {parse_errors}")
        };
        return Ok(module_formats
            .into_iter()
            .map(|format| (format, unknown_payload(detail.clone())))
            .collect());
    }

    let recoverable_parse_details = (!parsed.errors.is_empty())
        .then(|| {
            format!(
                "OXC recovered a complete AST with parse errors; local definitions are incomplete: {parse_errors}"
            )
        })
        .into_iter()
        .collect();

    let mut base_facts = JsPayloadFacts {
        exports: Vec::new(),
        uses: Vec::new(),
        nonliteral_dynamic_imports: Vec::new(),
        unsupported_import_meta_globs: Vec::new(),
        recoverable_parse_details,
        limitation_details: Vec::new(),
    };
    if matches!(kind, SourceKind::CommonJs | SourceKind::Cts) {
        base_facts
            .limitation_details
            .push(COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned());
    }
    for statement in &parsed.program.body {
        lower_statement(statement, &mut base_facts);
    }

    Ok(module_formats
        .into_iter()
        .map(|module_format| {
            let mut facts =
                contextualize_payload(kind, &parsed.program, base_facts.clone(), module_format);
            if !facts.recoverable_parse_details.is_empty() {
                facts.exports.clear();
            }
            (module_format, facts)
        })
        .collect())
}

fn contextualize_payload(
    kind: SourceKind,
    program: &oxc_ast::ast::Program<'_>,
    mut facts: JsPayloadFacts,
    module_format: JsModuleFormat,
) -> JsPayloadFacts {
    let dynamic_imports = analyze_literal_dynamic_imports(program);
    let computed_commonjs_require_calls =
        commonjs_computed::collect_computed_require_calls(program);
    let commonjs_wrapper_exports_possible = module_format != JsModuleFormat::EsModule;
    let require_scopes = RequireScopeTracker::analyze(
        program,
        module_format == JsModuleFormat::CommonJs,
        commonjs_wrapper_exports_possible,
    );
    let escaped_require = require_scopes.has_unattributed_require_escape();
    let mut detector = DynamicUseDetector {
        uses: Vec::new(),
        unknown_details: Vec::new(),
        require_scopes,
        opaque_require_reported: false,
        opaque_module_require_reported: false,
        commonjs_export_syntax_observed: false,
        commonjs_wrapper_exports_possible,
        module_member_object_references: BTreeSet::new(),
        non_escaping_module_require_members: BTreeSet::new(),
        handled_dynamic_imports: dynamic_imports.handled_imports,
        computed_commonjs_require_calls,
        nonliteral_dynamic_imports: Vec::new(),
        unsupported_import_meta_globs: Vec::new(),
    };
    if escaped_require {
        detector.report_opaque_require();
    }
    detector.visit_program(program);
    if detector.commonjs_export_syntax_observed
        && !facts
            .limitation_details
            .iter()
            .any(|detail| detail == COMMONJS_EXPORT_LOWERING_UNSUPPORTED)
    {
        facts
            .limitation_details
            .push(COMMONJS_EXPORT_LOWERING_UNSUPPORTED.to_owned());
    }
    facts.uses.extend(dynamic_imports.uses);
    facts.uses.extend(detector.uses);
    facts
        .nonliteral_dynamic_imports
        .extend(detector.nonliteral_dynamic_imports);
    facts
        .unsupported_import_meta_globs
        .extend(detector.unsupported_import_meta_globs);
    facts.limitation_details.extend(detector.unknown_details);
    if kind.is_declaration() {
        for export in &mut facts.exports {
            export.namespace = SymbolNamespace::Type;
        }
        for source_use in &mut facts.uses {
            source_use.namespace = SymbolNamespace::Type;
        }
    }
    canonicalize(&mut facts);
    facts
}

pub fn bind_payload(
    payload: &JsPayloadFacts,
    source_id: &LogicalSourceId,
    source_unit: SourceUnitId,
) -> FileFacts {
    FileFacts {
        source_id: source_id.clone(),
        source_unit: source_unit.clone(),
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
            .recoverable_parse_details
            .iter()
            .map(|detail| Limitation::JsRecoverableParseLocal {
                source_id: source_id.clone(),
                detail: detail.clone(),
            })
            .chain(
                payload
                    .limitation_details
                    .iter()
                    .map(|detail| Limitation::JsModuleUseUnknown {
                        source_id: source_id.clone(),
                        detail: detail.clone(),
                    }),
            )
            .chain(payload.nonliteral_dynamic_imports.iter().map(|dynamic| {
                Limitation::DynamicImportNonLiteral {
                    source_id: source_id.clone(),
                    source_unit: source_unit.clone(),
                    span: dynamic.span.clone(),
                    static_prefix: dynamic.static_prefix.clone(),
                    candidates: Vec::new(),
                    target_scope: DynamicImportTargetScope::Workspace,
                }
            }))
            .chain(payload.unsupported_import_meta_globs.iter().map(|glob| {
                Limitation::ImportMetaGlobUnsupported {
                    source_id: source_id.clone(),
                    source_unit: Box::new(source_unit.clone()),
                    span: glob.span.clone(),
                    patterns: glob.patterns.clone().into_boxed_slice(),
                    candidates: Vec::new(),
                    target_scope: ImportMetaGlobTargetScope::Package,
                    detail: glob.detail.clone(),
                }
            }))
            .collect(),
    }
}

pub fn scope_dynamic_import_limitations(facts: &mut [FileFacts], sources: &[SourceSnapshot]) {
    dynamic_import::scope_limitations(facts, sources);
}

pub fn scope_import_meta_globs(
    facts: &mut [FileFacts],
    sources: &[SourceSnapshot],
    hard_excluded_components: &[&str],
) -> Vec<InventoryBoundSourceUse> {
    import_meta_glob::scope(facts, sources, hard_excluded_components)
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
        Statement::ExportAllDeclaration(declaration) => lower_export_all(declaration, facts),
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

fn lower_export_all(
    declaration: &oxc_ast::ast::ExportAllDeclaration<'_>,
    facts: &mut JsPayloadFacts,
) {
    let declaration_namespace = namespace(declaration.export_kind);
    if let Some(exported) = &declaration.exported {
        let exported_name = module_export_name(exported);
        facts.exports.push(ExportTemplate {
            exported_name,
            local_name: None,
            namespace: declaration_namespace,
            span: span(declaration.span),
        });
        facts.uses.push(SourceUseTemplate {
            specifier: declaration.source.value.to_string(),
            imported_name: None,
            local_name: None,
            namespace: declaration_namespace,
            kind: ImportKind::Namespace,
            request_kind: ModuleRequestKind::StaticImport,
            span: span(declaration.span),
        });
        return;
    }

    facts.uses.push(SourceUseTemplate {
        specifier: declaration.source.value.to_string(),
        imported_name: None,
        local_name: None,
        namespace: declaration_namespace,
        kind: ImportKind::ReExportAll,
        request_kind: ModuleRequestKind::StaticImport,
        span: span(declaration.span),
    });
    facts.limitation_details.push(format!(
        "export-all from {} requires graph expansion not implemented in this increment",
        declaration.source.value
    ));
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
    opaque_module_require_reported: bool,
    commonjs_export_syntax_observed: bool,
    commonjs_wrapper_exports_possible: bool,
    module_member_object_references: BTreeSet<(u32, u32)>,
    non_escaping_module_require_members: BTreeSet<(u32, u32)>,
    handled_dynamic_imports: BTreeSet<(u32, u32)>,
    computed_commonjs_require_calls: BTreeSet<(u32, u32)>,
    nonliteral_dynamic_imports: Vec<NonLiteralDynamicImportTemplate>,
    unsupported_import_meta_globs: Vec<UnsupportedImportMetaGlobTemplate>,
}

impl DynamicUseDetector {
    fn require_is_opaque(&self, call_position: u32) -> bool {
        self.require_scopes.require_is_opaque(call_position)
    }

    fn report_opaque_require(&mut self) {
        if !self.opaque_require_reported {
            self.unknown_details
                .push(REQUIRE_ATTRIBUTION_OPAQUE.to_owned());
            self.opaque_require_reported = true;
        }
    }

    fn report_opaque_module_require(&mut self) {
        if !self.opaque_module_require_reported {
            self.unknown_details
                .push(MODULE_REQUIRE_ATTRIBUTION_OPAQUE.to_owned());
            self.opaque_module_require_reported = true;
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
        match literal_dynamic_import_specifier(&expression.source) {
            Some(specifier) if expression.options.is_some() || expression.phase.is_some() => {
                self.uses.push(SourceUseTemplate {
                    specifier: specifier.to_owned(),
                    imported_name: None,
                    local_name: None,
                    namespace: SymbolNamespace::Value,
                    kind: ImportKind::DynamicBroad,
                    request_kind: ModuleRequestKind::DynamicImport,
                    span: span(expression.span),
                });
                self.unknown_details.push(
                    "dynamic import options or phases are not modeled and may change module semantics"
                        .to_owned(),
                );
            }
            Some(specifier)
                if !self
                    .handled_dynamic_imports
                    .contains(&(expression.span.start, expression.span.end)) =>
            {
                self.uses.push(SourceUseTemplate {
                    specifier: specifier.to_owned(),
                    imported_name: None,
                    local_name: None,
                    namespace: SymbolNamespace::Value,
                    kind: ImportKind::DynamicBroad,
                    request_kind: ModuleRequestKind::DynamicImport,
                    span: span(expression.span),
                });
            }
            Some(_) => {}
            None => {
                self.nonliteral_dynamic_imports
                    .push(nonliteral_template(expression));
                if expression.options.is_some() || expression.phase.is_some() {
                    self.unknown_details.push(
                        "dynamic import options or phases are not modeled and may change module semantics"
                            .to_owned(),
                    );
                }
            }
        }
        walk::walk_import_expression(self, expression);
    }

    fn visit_this_expression(&mut self, expression: &oxc_ast::ast::ThisExpression) {
        if self.commonjs_wrapper_exports_possible
            && self
                .require_scopes
                .this_may_be_wrapper(expression.span.start)
        {
            self.commonjs_export_syntax_observed = true;
        }
        walk::walk_this_expression(self, expression);
    }

    fn visit_call_expression(&mut self, expression: &oxc_ast::ast::CallExpression<'a>) {
        if expression.callee.is_specific_id("require")
            && self.require_is_opaque(expression.span.start)
        {
            self.report_opaque_require();
        } else if let Some(source) = expression.common_js_require() {
            self.uses.push(SourceUseTemplate {
                specifier: source.value.to_string(),
                imported_name: None,
                local_name: None,
                namespace: SymbolNamespace::Value,
                kind: if self
                    .computed_commonjs_require_calls
                    .contains(&(expression.span.start, expression.span.end))
                {
                    ImportKind::CommonJsComputed
                } else {
                    ImportKind::DynamicBroad
                },
                request_kind: ModuleRequestKind::Require,
                span: span(expression.span),
            });
        } else if expression.callee.is_specific_id("require") {
            self.unknown_details
                .push("nonliteral CommonJS require may hide an internal consumer".to_owned());
        } else if is_import_meta_glob(&expression.callee) {
            match import_meta_glob::parse_call(expression) {
                ParsedImportMetaGlob::Supported { span, patterns } => {
                    self.uses
                        .extend(patterns.into_iter().map(|specifier| SourceUseTemplate {
                            specifier,
                            imported_name: None,
                            local_name: None,
                            namespace: SymbolNamespace::Value,
                            kind: ImportKind::DynamicBroad,
                            request_kind: ModuleRequestKind::ImportMetaGlob,
                            span: span.clone(),
                        }));
                }
                ParsedImportMetaGlob::Unsupported(glob) => {
                    self.unsupported_import_meta_globs.push(glob);
                }
            }
        }
        walk::walk_call_expression(self, expression);
    }

    fn visit_member_expression(&mut self, expression: &oxc_ast::ast::MemberExpression<'a>) {
        if self.commonjs_wrapper_exports_possible
            && self
                .require_scopes
                .mapped_wrapper_export_object_may_be_visible(expression)
        {
            self.commonjs_export_syntax_observed = true;
        }
        if let Some(identifier) = expression
            .object()
            .without_parentheses()
            .get_identifier_reference()
            && identifier.name == "module"
        {
            self.module_member_object_references
                .insert((identifier.span.start, identifier.span.end));
            let property_name = expression.static_property_name();
            if self.require_scopes.module_may_be_wrapper() {
                if property_name == Some("require")
                    && !self
                        .non_escaping_module_require_members
                        .contains(&(expression.span().start, expression.span().end))
                {
                    self.report_opaque_module_require();
                }
                if self.commonjs_wrapper_exports_possible
                    && matches!(property_name, Some("exports") | None)
                {
                    self.commonjs_export_syntax_observed = true;
                }
            }
        }
        walk::walk_member_expression(self, expression);
    }

    fn visit_identifier_reference(&mut self, identifier: &oxc_ast::ast::IdentifierReference<'a>) {
        let wrapper_export_object = self.commonjs_wrapper_exports_possible
            && ((identifier.name == "exports" && self.require_scopes.exports_may_be_wrapper())
                || (identifier.name == "module"
                    && self.require_scopes.module_may_be_wrapper()
                    && !self
                        .module_member_object_references
                        .contains(&(identifier.span.start, identifier.span.end))));
        if wrapper_export_object {
            self.commonjs_export_syntax_observed = true;
        }
        walk::walk_identifier_reference(self, identifier);
    }

    fn visit_unary_expression(&mut self, expression: &oxc_ast::ast::UnaryExpression<'a>) {
        if expression.operator.is_typeof()
            && let Some(member) = expression.argument.without_parentheses().get_member_expr()
            && is_module_require_member(member)
        {
            self.non_escaping_module_require_members
                .insert((member.span().start, member.span().end));
        }
        walk::walk_unary_expression(self, expression);
    }

    fn visit_with_statement(&mut self, statement: &oxc_ast::ast::WithStatement<'a>) {
        self.visit_expression(&statement.object);
        self.require_scopes.enter_with_body();
        self.visit_statement(&statement.body);
        self.require_scopes.leave_with_body();
    }
}

fn is_module_require_member(expression: &oxc_ast::ast::MemberExpression<'_>) -> bool {
    expression.static_property_name() == Some("require")
        && expression.object().is_specific_id("module")
}

fn is_import_meta_glob(expression: &oxc_ast::ast::Expression<'_>) -> bool {
    let Some(member) = transparent_runtime_expression(expression).get_member_expr() else {
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
        nonliteral_dynamic_imports: Vec::new(),
        unsupported_import_meta_globs: Vec::new(),
        recoverable_parse_details: Vec::new(),
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
    facts.nonliteral_dynamic_imports.sort();
    facts.nonliteral_dynamic_imports.dedup();
    for glob in &mut facts.unsupported_import_meta_globs {
        glob.patterns.sort();
        glob.patterns.dedup();
    }
    facts.unsupported_import_meta_globs.sort();
    facts.unsupported_import_meta_globs.dedup();
    facts.recoverable_parse_details.sort();
    facts.recoverable_parse_details.dedup();
}

#[cfg(test)]
mod tests;
