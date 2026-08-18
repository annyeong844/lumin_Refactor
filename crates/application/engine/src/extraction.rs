use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use lumin_model::{
    CapabilityState, FileFacts, InventoryBoundSourceUse, LogicalSourceId, PayloadSnapshotId,
    ResolutionProfile, SelectedResolutionProfile, SemanticConfigSnapshot, SfcDialect, SourceKind,
    SourceSnapshot, SourceUnitId,
};
use rayon::prelude::*;

use crate::EngineError;

const WORKER_STACK_BYTES: usize = 4_194_304;

pub(super) struct ExtractionOutput {
    pub(super) facts: Vec<FileFacts>,
    pub(super) inventory_bound_uses: Vec<InventoryBoundSourceUse>,
    pub(super) sfc_states: BTreeMap<SfcDialect, CapabilityState>,
    pub(super) js_parse_product_count: usize,
}

pub(super) fn extract_facts(
    sources: &[SourceSnapshot],
    config: &SemanticConfigSnapshot,
    selected_profiles: &[SelectedResolutionProfile],
    jobs: usize,
) -> Result<ExtractionOutput, EngineError> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .stack_size(WORKER_STACK_BYTES)
        .thread_name(|index| format!("lumin-worker-{index}"))
        .build()
        .map_err(|error| EngineError::Scheduler(error.to_string()))?;
    let source_index = lumin_sfc::source_index(sources);
    let profiles = selected_profiles
        .iter()
        .map(|selected| (selected.source_id.clone(), selected.profile))
        .collect::<BTreeMap<_, _>>();
    pool.install(|| {
        let (physical_facts, physical_parse_product_count) =
            extract_physical_js(sources, config, &profiles)?;

        let mut decompositions = sources
            .par_iter()
            .filter(|source| !source.kind.is_js_family())
            .map(|source| {
                lumin_sfc::decompose(source, &source_index).map(|decomposition| {
                    (
                        decomposition,
                        source_module_format(source, config, &profiles),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        decompositions.sort_by(|left, right| left.0.source_id.cmp(&right.0.source_id));

        let embedded_parse_product_count = decompositions
            .iter()
            .map(|(decomposition, _module_format)| decomposition.inline_scripts.len())
            .sum::<usize>();
        let mut embedded_by_parent = BTreeMap::<_, Vec<FileFacts>>::new();
        let mut embedded = decompositions
            .par_iter()
            .flat_map_iter(|(decomposition, module_format)| {
                decomposition
                    .inline_scripts
                    .iter()
                    .map(move |unit| (&decomposition.source_id, unit, *module_format))
            })
            .map(|(parent, unit, module_format)| {
                lumin_js::extract_embedded_with_module_format(unit, module_format)
                    .map(|facts| (parent.clone(), facts))
            })
            .collect::<Result<Vec<_>, _>>()?;
        embedded.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.source_unit.cmp(&right.1.source_unit))
        });
        for (parent, facts) in embedded {
            embedded_by_parent.entry(parent).or_default().push(facts);
        }

        let mut sfc_states = BTreeMap::new();
        for (dialect, _id, initial_state) in lumin_sfc::compiled_dialect_states() {
            sfc_states.insert(dialect, initial_state);
        }
        let mut sfc_facts = Vec::new();
        let mut sfc_dialects_by_source = BTreeMap::<LogicalSourceId, BTreeSet<SfcDialect>>::new();
        for (decomposition, _module_format) in decompositions {
            let parent = decomposition.source_id.clone();
            let analysis = lumin_sfc::finalize(
                decomposition,
                embedded_by_parent.remove(&parent).unwrap_or_default(),
                &physical_facts,
            )?;
            sfc_states
                .entry(analysis.dialect)
                .and_modify(|state| *state = less_complete(*state, analysis.state))
                .or_insert(analysis.state);
            sfc_dialects_by_source
                .entry(analysis.source_id.clone())
                .or_default()
                .insert(analysis.dialect);
            for attachment in &analysis.script_attachments {
                sfc_dialects_by_source
                    .entry(attachment.target_source_id.clone())
                    .or_default()
                    .insert(analysis.dialect);
            }
            sfc_facts.extend(analysis.file_facts);
        }

        let mut facts = physical_facts;
        facts.extend(sfc_facts);
        lumin_js::scope_dynamic_import_limitations(&mut facts, sources);
        let inventory_bound_uses = lumin_js::scope_import_meta_globs(
            &mut facts,
            sources,
            lumin_inventory::HARD_EXCLUDED_COMPONENTS,
        );
        synchronize_sfc_states(&facts, &sfc_dialects_by_source, &mut sfc_states);
        Ok(ExtractionOutput {
            facts: reduce_file_facts(facts),
            inventory_bound_uses,
            sfc_states,
            js_parse_product_count: physical_parse_product_count + embedded_parse_product_count,
        })
    })
}

fn synchronize_sfc_states(
    facts: &[FileFacts],
    dialects_by_source: &BTreeMap<LogicalSourceId, BTreeSet<SfcDialect>>,
    states: &mut BTreeMap<SfcDialect, CapabilityState>,
) {
    for facts in facts.iter().filter(|facts| !facts.limitations.is_empty()) {
        let Some(dialects) = dialects_by_source.get(&facts.source_id) else {
            continue;
        };
        for dialect in dialects {
            states
                .entry(*dialect)
                .and_modify(|state| *state = less_complete(*state, CapabilityState::Incomplete))
                .or_insert(CapabilityState::Incomplete);
        }
    }
}

type PayloadGroup<'a> = (
    SourceKind,
    Arc<[u8]>,
    BTreeMap<lumin_js::JsModuleFormat, Vec<&'a SourceSnapshot>>,
);

fn extract_physical_js(
    sources: &[SourceSnapshot],
    config: &SemanticConfigSnapshot,
    profiles: &BTreeMap<lumin_model::LogicalSourceId, ResolutionProfile>,
) -> Result<(Vec<FileFacts>, usize), EngineError> {
    let mut grouped = BTreeMap::<
        (PayloadSnapshotId, SourceKind),
        (
            Arc<[u8]>,
            BTreeMap<lumin_js::JsModuleFormat, Vec<&SourceSnapshot>>,
        ),
    >::new();
    for source in sources.iter().filter(|source| source.kind.is_js_family()) {
        let module_format = source_module_format(source, config, profiles);
        grouped
            .entry((source.payload_snapshot_id.clone(), source.kind))
            .or_insert_with(|| (Arc::clone(&source.bytes), BTreeMap::new()))
            .1
            .entry(module_format)
            .or_default()
            .push(source);
    }
    let parse_product_count = grouped.len();
    let groups = grouped
        .into_iter()
        .map(|((_payload_id, kind), (bytes, sources))| (kind, bytes, sources))
        .collect::<Vec<PayloadGroup<'_>>>();
    let facts = groups
        .into_par_iter()
        .map(|(kind, bytes, sources_by_format)| {
            let formats = sources_by_format.keys().copied().collect::<Vec<_>>();
            let mut payloads = lumin_js::parse_payload_with_module_formats(kind, &bytes, &formats)?
                .into_iter()
                .collect::<BTreeMap<_, _>>();
            let mut facts = Vec::new();
            for (format, sources) in sources_by_format {
                let payload = payloads.remove(&format).ok_or_else(|| {
                    EngineError::ExtractionProductMissing(format!("{kind:?}/{format:?}"))
                })?;
                for source in sources {
                    facts.push(lumin_js::bind_payload(
                        &payload,
                        &source.id,
                        SourceUnitId::Logical(source.id.clone()),
                    ));
                }
            }
            Ok(facts)
        })
        .collect::<Result<Vec<_>, EngineError>>()?
        .into_iter()
        .flatten()
        .collect();
    Ok((reduce_file_facts(facts), parse_product_count))
}

fn source_module_format(
    source: &SourceSnapshot,
    config: &SemanticConfigSnapshot,
    profiles: &BTreeMap<lumin_model::LogicalSourceId, ResolutionProfile>,
) -> lumin_js::JsModuleFormat {
    let extension_defines_format = matches!(
        source.kind,
        SourceKind::CommonJs
            | SourceKind::Cts
            | SourceKind::DeclarationCts
            | SourceKind::Mjs
            | SourceKind::Mts
            | SourceKind::DeclarationMts
    );
    let profile = profiles.get(&source.id).copied();
    let node_profile_defines_package_format = matches!(
        profile,
        Some(ResolutionProfile::Node | ResolutionProfile::Node16 | ResolutionProfile::NodeNext)
    );
    if !extension_defines_format && !node_profile_defines_package_format {
        return lumin_js::JsModuleFormat::Unknown;
    }
    if !extension_defines_format && profile == Some(ResolutionProfile::Node) {
        return lumin_js::JsModuleFormat::CommonJs;
    }
    match lumin_resolve::classify_importer_format(source, config) {
        lumin_resolve::ImporterFormatClassification::CommonJs => lumin_js::JsModuleFormat::CommonJs,
        lumin_resolve::ImporterFormatClassification::EsModule => lumin_js::JsModuleFormat::EsModule,
        lumin_resolve::ImporterFormatClassification::Unavailable
        | lumin_resolve::ImporterFormatClassification::Unsupported { .. } => {
            lumin_js::JsModuleFormat::Unknown
        }
    }
}

pub(super) fn reduce_file_facts(mut facts: Vec<FileFacts>) -> Vec<FileFacts> {
    facts.sort_by(|left, right| {
        left.source_id
            .cmp(&right.source_id)
            .then_with(|| left.source_unit.cmp(&right.source_unit))
    });
    facts
}

fn less_complete(left: CapabilityState, right: CapabilityState) -> CapabilityState {
    fn rank(state: CapabilityState) -> u8 {
        match state {
            CapabilityState::Complete => 0,
            CapabilityState::Incomplete => 1,
            CapabilityState::Unavailable => 2,
            CapabilityState::Failed => 3,
        }
    }
    if rank(left) >= rank(right) {
        left
    } else {
        right
    }
}
