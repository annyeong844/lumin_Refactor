mod markup;
mod vue;

use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    CapabilityState, EmbeddedSourceUnit, ExportFact, ExternalEmbeddedSourceRef, FileFacts,
    ImportKind, Limitation, LogicalSourceId, ModuleRequestKind, RepoPath, SfcAnalysis,
    SfcComponentUse, SfcDecomposition, SfcDialect, SfcResourceUse, SfcScriptAttachment,
    SfcTemplateUse, SfcTemplateUseKind, SourceKind, SourceSnapshot, SourceSpan, SourceUnitId,
    SourceUseFact, SymbolNamespace,
};
use thiserror::Error;

pub const SFC_OWNER_VERSION: &str = "sfc-owner.v1";
pub const VUE_CAPABILITY_ID: &str = "sfc/vue.v1";
pub const SVELTE_CAPABILITY_ID: &str = "sfc/svelte.v1";
pub const ASTRO_CAPABILITY_ID: &str = "sfc/astro.v1";

pub type SfcSourceIndex = BTreeMap<RepoPath, (LogicalSourceId, SourceKind)>;

#[derive(Debug, Error)]
pub enum SfcError {
    #[error("source kind {0:?} was routed to the SFC owner")]
    WrongOwner(SourceKind),
    #[error("SFC finalization is missing embedded facts for {0}")]
    MissingEmbeddedFacts(String),
    #[error("SFC finalization received unexpected embedded facts for {0}")]
    UnexpectedEmbeddedFacts(String),
    #[error("SFC finalization cannot find external script facts for {0}")]
    MissingExternalFacts(String),
    #[error("SFC parent span overflowed while finalizing {0}")]
    SpanOverflow(String),
}

pub fn source_index(sources: &[SourceSnapshot]) -> SfcSourceIndex {
    sources
        .iter()
        .map(|source| (source.path.clone(), (source.id.clone(), source.kind)))
        .collect()
}

pub fn decompose(
    snapshot: &SourceSnapshot,
    source_index: &SfcSourceIndex,
) -> Result<SfcDecomposition, SfcError> {
    match snapshot.kind {
        SourceKind::Vue => Ok(vue::decompose(snapshot, source_index)),
        SourceKind::Svelte => Ok(unavailable(snapshot, SfcDialect::Svelte, "svelte")),
        SourceKind::Astro => Ok(unavailable(snapshot, SfcDialect::Astro, "astro")),
        kind => Err(SfcError::WrongOwner(kind)),
    }
}

pub fn finalize(
    decomposition: SfcDecomposition,
    mut embedded_facts: Vec<FileFacts>,
    physical_facts: &[FileFacts],
) -> Result<SfcAnalysis, SfcError> {
    let SfcDecomposition {
        source_id,
        dialect,
        mut state,
        module_export_known,
        inline_scripts,
        external_scripts,
        template_uses,
        resource_uses,
        limitations,
    } = decomposition;
    validate_embedded_facts(&source_id, &inline_scripts, &mut embedded_facts)?;
    let mut parent_facts =
        parent_file_facts(&source_id, module_export_known, limitations, resource_uses);
    let (external_binding_facts, mut attachments) =
        bind_external_scripts(&source_id, external_scripts, physical_facts)?;
    let mut component_uses = bind_template_components(
        &source_id,
        template_uses,
        &embedded_facts,
        &external_binding_facts,
        &mut parent_facts.limitations,
    );
    canonicalize(&mut parent_facts);
    if !parent_facts.limitations.is_empty()
        || embedded_facts
            .iter()
            .any(|facts| !facts.limitations.is_empty())
        || external_binding_facts
            .iter()
            .any(|facts| !facts.limitations.is_empty())
    {
        state = CapabilityState::Incomplete;
    }
    let mut file_facts = Vec::with_capacity(embedded_facts.len() + 1);
    file_facts.push(parent_facts);
    file_facts.extend(embedded_facts);
    file_facts.sort_by(|left, right| {
        left.source_id
            .cmp(&right.source_id)
            .then_with(|| left.source_unit.cmp(&right.source_unit))
    });
    attachments.sort_by_key(|attachment| attachment.parent_span.start);
    component_uses.sort_by(|left, right| {
        left.template_span
            .start
            .cmp(&right.template_span.start)
            .then_with(|| left.tag_name.cmp(&right.tag_name))
    });

    Ok(SfcAnalysis {
        source_id,
        dialect,
        state,
        file_facts,
        script_attachments: attachments,
        component_uses,
    })
}

fn validate_embedded_facts(
    source_id: &LogicalSourceId,
    inline_scripts: &[EmbeddedSourceUnit],
    embedded_facts: &mut [FileFacts],
) -> Result<(), SfcError> {
    let expected_units = inline_scripts
        .iter()
        .map(|unit| (unit.id.clone(), unit.parent_span.start))
        .collect::<BTreeMap<_, _>>();
    let mut observed_units = BTreeSet::new();
    for facts in embedded_facts {
        let unit_id = match &facts.source_unit {
            SourceUnitId::Embedded(unit_id) => unit_id.clone(),
            SourceUnitId::Logical(_) => {
                return Err(SfcError::UnexpectedEmbeddedFacts(
                    facts.source_id.as_str().to_owned(),
                ));
            }
        };
        let Some(offset) = expected_units.get(&unit_id).copied() else {
            return Err(SfcError::UnexpectedEmbeddedFacts(
                unit_id.as_str().to_owned(),
            ));
        };
        if facts.source_id != *source_id || !observed_units.insert(unit_id.clone()) {
            return Err(SfcError::UnexpectedEmbeddedFacts(
                unit_id.as_str().to_owned(),
            ));
        }
        shift_file_spans(facts, offset, unit_id.as_str())?;
    }
    if let Some(missing) = expected_units
        .keys()
        .find(|unit_id| !observed_units.contains(*unit_id))
    {
        return Err(SfcError::MissingEmbeddedFacts(missing.as_str().to_owned()));
    }
    Ok(())
}

fn parent_file_facts(
    source_id: &LogicalSourceId,
    module_export_known: bool,
    limitations: Vec<Limitation>,
    resource_uses: Vec<SfcResourceUse>,
) -> FileFacts {
    let mut facts = FileFacts::physical(source_id.clone());
    facts.limitations = limitations;
    if module_export_known {
        facts.exports.push(ExportFact {
            source_id: source_id.clone(),
            exported_name: "default".to_owned(),
            local_name: None,
            namespace: SymbolNamespace::Value,
            span: SourceSpan { start: 0, end: 0 },
        });
    }
    facts
        .uses
        .extend(resource_uses.into_iter().map(|resource| SourceUseFact {
            importer: source_id.clone(),
            specifier: resource.specifier,
            imported_name: None,
            local_name: None,
            namespace: SymbolNamespace::Value,
            kind: ImportKind::SideEffect,
            request_kind: ModuleRequestKind::StaticImport,
            span: resource.span,
        }));
    facts
}

fn bind_external_scripts<'a>(
    source_id: &LogicalSourceId,
    references: Vec<ExternalEmbeddedSourceRef>,
    physical_facts: &'a [FileFacts],
) -> Result<(Vec<&'a FileFacts>, Vec<SfcScriptAttachment>), SfcError> {
    let mut binding_facts = Vec::new();
    let mut attachments = Vec::new();
    for reference in references {
        let Some(facts) = physical_facts.iter().find(|facts| {
            facts.source_id == reference.target_source_id
                && matches!(facts.source_unit, SourceUnitId::Logical(_))
        }) else {
            return Err(SfcError::MissingExternalFacts(
                reference.target_source_id.as_str().to_owned(),
            ));
        };
        binding_facts.push(facts);
        attachments.push(SfcScriptAttachment {
            parent_source_id: source_id.clone(),
            target_source_id: reference.target_source_id,
            parent_span: reference.parent_span,
        });
    }
    Ok((binding_facts, attachments))
}

fn bind_template_components(
    source_id: &LogicalSourceId,
    template_uses: Vec<SfcTemplateUse>,
    embedded_facts: &[FileFacts],
    external_binding_facts: &[&FileFacts],
    limitations: &mut Vec<Limitation>,
) -> Vec<SfcComponentUse> {
    let mut component_uses = Vec::new();
    for template_use in template_uses {
        if template_use.kind != SfcTemplateUseKind::Static {
            limitations.push(template_limitation(
                source_id,
                format!(
                    "template component `{}` requires dynamic or namespace binding",
                    template_use.tag_name
                ),
            ));
            continue;
        }
        let mut candidates = embedded_facts
            .iter()
            .chain(external_binding_facts.iter().copied())
            .flat_map(|facts| facts.uses.iter())
            .filter(|source_use| {
                source_use.namespace == SymbolNamespace::Value
                    && source_use.local_name.as_deref() == Some(template_use.binding_name.as_str())
            });
        let Some(source_use) = candidates.next() else {
            limitations.push(template_limitation(
                source_id,
                format!(
                    "template component `{}` has no local script binding",
                    template_use.tag_name
                ),
            ));
            continue;
        };
        if candidates.next().is_some() {
            limitations.push(template_limitation(
                source_id,
                format!(
                    "template component `{}` has multiple local script bindings",
                    template_use.tag_name
                ),
            ));
            continue;
        }
        component_uses.push(SfcComponentUse {
            parent_source_id: source_id.clone(),
            tag_name: template_use.tag_name,
            binding_name: template_use.binding_name,
            source_use: source_use.clone(),
            template_span: template_use.span,
        });
    }
    component_uses
}

fn template_limitation(source_id: &LogicalSourceId, detail: String) -> Limitation {
    Limitation::VueTemplateOpaque {
        source_id: source_id.clone(),
        detail,
    }
}

pub fn capability_id(dialect: SfcDialect) -> &'static str {
    match dialect {
        SfcDialect::Vue => VUE_CAPABILITY_ID,
        SfcDialect::Svelte => SVELTE_CAPABILITY_ID,
        SfcDialect::Astro => ASTRO_CAPABILITY_ID,
    }
}

fn unavailable(snapshot: &SourceSnapshot, dialect: SfcDialect, name: &str) -> SfcDecomposition {
    SfcDecomposition {
        source_id: snapshot.id.clone(),
        dialect,
        state: CapabilityState::Unavailable,
        module_export_known: false,
        inline_scripts: Vec::new(),
        external_scripts: Vec::new(),
        template_uses: Vec::new(),
        resource_uses: Vec::new(),
        limitations: vec![Limitation::SfcDialectUnavailable {
            source_id: snapshot.id.clone(),
            dialect: name.to_owned(),
        }],
    }
}

fn shift_file_spans(facts: &mut FileFacts, offset: u32, unit_id: &str) -> Result<(), SfcError> {
    for export in &mut facts.exports {
        shift_span(&mut export.span, offset, unit_id)?;
    }
    for source_use in &mut facts.uses {
        shift_span(&mut source_use.span, offset, unit_id)?;
    }
    Ok(())
}

fn shift_span(span: &mut SourceSpan, offset: u32, unit_id: &str) -> Result<(), SfcError> {
    span.start = span
        .start
        .checked_add(offset)
        .ok_or_else(|| SfcError::SpanOverflow(unit_id.to_owned()))?;
    span.end = span
        .end
        .checked_add(offset)
        .ok_or_else(|| SfcError::SpanOverflow(unit_id.to_owned()))?;
    Ok(())
}

fn canonicalize(facts: &mut FileFacts) {
    facts.exports.sort_by(|left, right| {
        left.namespace
            .cmp(&right.namespace)
            .then_with(|| left.exported_name.cmp(&right.exported_name))
            .then_with(|| left.span.start.cmp(&right.span.start))
    });
    facts.exports.dedup();
    facts.uses.sort_by(|left, right| {
        left.span
            .start
            .cmp(&right.span.start)
            .then_with(|| left.specifier.cmp(&right.specifier))
    });
    facts.uses.dedup();
    facts
        .limitations
        .sort_by_key(|limitation| format!("{limitation:?}"));
    facts.limitations.dedup();
}
