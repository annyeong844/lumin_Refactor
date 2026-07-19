use lumin_model::{
    ExportFact, FileFacts, ImportKind, Limitation, SourceKind, SourceSnapshot, SourceSpan,
    SourceUseFact, SymbolNamespace,
};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Declaration, ExportNamedDeclaration, ImportDeclaration, ImportDeclarationSpecifier,
    ImportOrExportKind, ModuleExportName, Statement,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{SourceType, Span};

pub const JS_EXTRACTOR_VERSION: &str = "js-module-facts.v1";

pub fn extract(snapshot: &SourceSnapshot) -> FileFacts {
    if !snapshot.kind.is_js_family() {
        return FileFacts {
            source_id: snapshot.id.clone(),
            exports: Vec::new(),
            uses: Vec::new(),
            limitations: vec![Limitation::SfcDialectUnavailable {
                source_id: snapshot.id.clone(),
                dialect: source_kind_name(snapshot.kind).to_owned(),
            }],
        };
    }

    let source = match std::str::from_utf8(&snapshot.bytes) {
        Ok(source) => source,
        Err(error) => {
            return unknown_file(snapshot, format!("source is not UTF-8: {error}"));
        }
    };

    if snapshot.kind == SourceKind::CommonJs || snapshot.kind == SourceKind::Cts {
        return unknown_file(
            snapshot,
            "CommonJS export lowering is not implemented in the first audit increment".to_owned(),
        );
    }

    let source_type = match source_type(snapshot.kind) {
        Ok(source_type) => source_type,
        Err(detail) => return unknown_file(snapshot, detail),
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
        return unknown_file(
            snapshot,
            format!("OXC parse did not complete cleanly: {detail}"),
        );
    }

    let mut facts = FileFacts {
        source_id: snapshot.id.clone(),
        exports: Vec::new(),
        uses: Vec::new(),
        limitations: Vec::new(),
    };
    for statement in &parsed.program.body {
        lower_statement(statement, snapshot, &mut facts);
    }

    let mut detector = DynamicUseDetector {
        snapshot,
        uses: Vec::new(),
        unknown_details: Vec::new(),
    };
    detector.visit_program(&parsed.program);
    facts.uses.extend(detector.uses);
    for detail in detector.unknown_details {
        facts.limitations.push(Limitation::JsModuleUseUnknown {
            source_id: snapshot.id.clone(),
            detail,
        });
    }
    canonicalize(&mut facts);
    facts
}

fn lower_statement(statement: &Statement<'_>, snapshot: &SourceSnapshot, facts: &mut FileFacts) {
    match statement {
        Statement::ImportDeclaration(declaration) => lower_import(declaration, snapshot, facts),
        Statement::ExportNamedDeclaration(declaration) => {
            lower_named_export(declaration, snapshot, facts);
        }
        Statement::ExportDefaultDeclaration(declaration) => {
            facts.exports.push(ExportFact {
                source_id: snapshot.id.clone(),
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
            facts.limitations.push(Limitation::JsModuleUseUnknown {
                source_id: snapshot.id.clone(),
                detail: format!(
                    "export-all from {} requires graph expansion not implemented in this increment",
                    declaration.source.value
                ),
            });
        }
        Statement::TSExportAssignment(_) | Statement::TSNamespaceExportDeclaration(_) => {
            facts.limitations.push(Limitation::JsModuleUseUnknown {
                source_id: snapshot.id.clone(),
                detail: "TypeScript export assignment/namespace export is not lowered".to_owned(),
            });
        }
        _ => {}
    }
}

fn lower_import(
    declaration: &ImportDeclaration<'_>,
    snapshot: &SourceSnapshot,
    facts: &mut FileFacts,
) {
    let specifier = declaration.source.value.to_string();
    let declaration_namespace = namespace(declaration.import_kind);
    let Some(specifiers) = &declaration.specifiers else {
        facts.uses.push(SourceUseFact {
            importer: snapshot.id.clone(),
            specifier,
            imported_name: None,
            namespace: declaration_namespace,
            kind: ImportKind::SideEffect,
            span: span(declaration.span),
        });
        return;
    };

    for import in specifiers {
        match import {
            ImportDeclarationSpecifier::ImportSpecifier(import) => {
                facts.uses.push(SourceUseFact {
                    importer: snapshot.id.clone(),
                    specifier: specifier.clone(),
                    imported_name: Some(module_export_name(&import.imported)),
                    namespace: if declaration.import_kind == ImportOrExportKind::Type
                        || import.import_kind == ImportOrExportKind::Type
                    {
                        SymbolNamespace::Type
                    } else {
                        SymbolNamespace::Value
                    },
                    kind: ImportKind::Named,
                    span: span(import.span),
                });
            }
            ImportDeclarationSpecifier::ImportDefaultSpecifier(import) => {
                facts.uses.push(SourceUseFact {
                    importer: snapshot.id.clone(),
                    specifier: specifier.clone(),
                    imported_name: Some("default".to_owned()),
                    namespace: declaration_namespace,
                    kind: ImportKind::Default,
                    span: span(import.span),
                });
            }
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(import) => {
                facts.uses.push(SourceUseFact {
                    importer: snapshot.id.clone(),
                    specifier: specifier.clone(),
                    imported_name: None,
                    namespace: declaration_namespace,
                    kind: ImportKind::Namespace,
                    span: span(import.span),
                });
            }
        }
    }
}

fn lower_named_export(
    declaration: &ExportNamedDeclaration<'_>,
    snapshot: &SourceSnapshot,
    facts: &mut FileFacts,
) {
    if let Some(inner) = &declaration.declaration {
        lower_declaration(inner, snapshot, facts);
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
        facts.exports.push(ExportFact {
            source_id: snapshot.id.clone(),
            exported_name,
            local_name: Some(local_name.clone()),
            namespace,
            span: span(export.span),
        });
        if let Some(source) = &declaration.source {
            facts.uses.push(SourceUseFact {
                importer: snapshot.id.clone(),
                specifier: source.value.to_string(),
                imported_name: Some(local_name),
                namespace,
                kind: ImportKind::ReExportNamed,
                span: span(export.span),
            });
        }
    }
}

fn lower_declaration(
    declaration: &Declaration<'_>,
    snapshot: &SourceSnapshot,
    facts: &mut FileFacts,
) {
    match declaration {
        Declaration::VariableDeclaration(declaration) => {
            for declarator in &declaration.declarations {
                for identifier in declarator.id.get_binding_identifiers() {
                    facts.exports.push(ExportFact {
                        source_id: snapshot.id.clone(),
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
                    snapshot,
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
                    snapshot,
                    facts,
                    identifier.name.as_str(),
                    SymbolNamespace::Value,
                    declaration.span,
                );
            }
        }
        Declaration::TSTypeAliasDeclaration(declaration) => push_named_declaration(
            snapshot,
            facts,
            declaration.id.name.as_str(),
            SymbolNamespace::Type,
            declaration.span,
        ),
        Declaration::TSInterfaceDeclaration(declaration) => push_named_declaration(
            snapshot,
            facts,
            declaration.id.name.as_str(),
            SymbolNamespace::Type,
            declaration.span,
        ),
        Declaration::TSEnumDeclaration(declaration) => {
            push_named_declaration(
                snapshot,
                facts,
                declaration.id.name.as_str(),
                SymbolNamespace::Value,
                declaration.span,
            );
            push_named_declaration(
                snapshot,
                facts,
                declaration.id.name.as_str(),
                SymbolNamespace::Type,
                declaration.span,
            );
        }
        Declaration::TSModuleDeclaration(_)
        | Declaration::TSGlobalDeclaration(_)
        | Declaration::TSImportEqualsDeclaration(_) => {
            facts.limitations.push(Limitation::JsModuleUseUnknown {
                source_id: snapshot.id.clone(),
                detail: "TypeScript module/global/import-equals declaration is not lowered"
                    .to_owned(),
            });
        }
    }
}

fn push_named_declaration(
    snapshot: &SourceSnapshot,
    facts: &mut FileFacts,
    name: &str,
    namespace: SymbolNamespace,
    declaration_span: Span,
) {
    facts.exports.push(ExportFact {
        source_id: snapshot.id.clone(),
        exported_name: name.to_owned(),
        local_name: Some(name.to_owned()),
        namespace,
        span: span(declaration_span),
    });
}

struct DynamicUseDetector<'a> {
    snapshot: &'a SourceSnapshot,
    uses: Vec<SourceUseFact>,
    unknown_details: Vec<String>,
}

impl<'a> Visit<'a> for DynamicUseDetector<'_> {
    fn visit_import_expression(&mut self, expression: &oxc_ast::ast::ImportExpression<'a>) {
        match &expression.source {
            oxc_ast::ast::Expression::StringLiteral(source) => {
                self.uses.push(SourceUseFact {
                    importer: self.snapshot.id.clone(),
                    specifier: source.value.to_string(),
                    imported_name: None,
                    namespace: SymbolNamespace::Value,
                    kind: ImportKind::DynamicBroad,
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
        if let Some(source) = expression.common_js_require() {
            self.uses.push(SourceUseFact {
                importer: self.snapshot.id.clone(),
                specifier: source.value.to_string(),
                imported_name: None,
                namespace: SymbolNamespace::Value,
                kind: ImportKind::DynamicBroad,
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

fn unknown_file(snapshot: &SourceSnapshot, detail: String) -> FileFacts {
    FileFacts {
        source_id: snapshot.id.clone(),
        exports: Vec::new(),
        uses: Vec::new(),
        limitations: vec![Limitation::JsModuleUseUnknown {
            source_id: snapshot.id.clone(),
            detail,
        }],
    }
}

fn canonicalize(facts: &mut FileFacts) {
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
mod tests {
    use lumin_model::{RepoPath, SourceRoles};

    use super::*;

    #[test]
    fn lowers_named_imports_and_exports() -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = SourceSnapshot::new(
            RepoPath::from_portable("src/main.ts")?,
            SourceKind::TypeScript,
            SourceRoles::default(),
            b"import { used } from './lib.js'; export const alive = used; export const dead = 1;"
                .to_vec(),
        );
        let facts = extract(&snapshot);
        assert!(facts.limitations.is_empty());
        assert_eq!(facts.uses.len(), 1);
        assert_eq!(facts.exports.len(), 2);
        assert_eq!(facts.uses[0].imported_name.as_deref(), Some("used"));
        Ok(())
    }

    #[test]
    fn parse_failure_is_visible_and_not_empty_success() -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = SourceSnapshot::new(
            RepoPath::from_portable("broken.ts")?,
            SourceKind::TypeScript,
            SourceRoles::default(),
            b"export const = ;".to_vec(),
        );
        let facts = extract(&snapshot);
        assert!(facts.exports.is_empty());
        assert_eq!(facts.limitations.len(), 1);
        Ok(())
    }
}
