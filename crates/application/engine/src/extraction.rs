use std::collections::BTreeMap;
use std::sync::Arc;

use lumin_model::{
    CapabilityState, FileFacts, PayloadSnapshotId, ResolutionProfile, SemanticConfigSnapshot,
    SfcDialect, SourceKind, SourceSnapshot, SourceUnitId,
};
use rayon::prelude::*;

use crate::EngineError;

const WORKER_STACK_BYTES: usize = 4_194_304;

pub(super) struct ExtractionOutput {
    pub(super) facts: Vec<FileFacts>,
    pub(super) sfc_states: BTreeMap<SfcDialect, CapabilityState>,
    pub(super) js_parse_product_count: usize,
}

pub(super) fn extract_facts(
    sources: &[SourceSnapshot],
    config: &SemanticConfigSnapshot,
    resolution_profile: Option<ResolutionProfile>,
    jobs: usize,
) -> Result<ExtractionOutput, EngineError> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .stack_size(WORKER_STACK_BYTES)
        .thread_name(|index| format!("lumin-worker-{index}"))
        .build()
        .map_err(|error| EngineError::Scheduler(error.to_string()))?;
    let source_index = lumin_sfc::source_index(sources);
    pool.install(|| {
        let (physical_facts, physical_parse_product_count) =
            extract_physical_js(sources, config, resolution_profile)?;

        let mut decompositions = sources
            .par_iter()
            .filter(|source| !source.kind.is_js_family())
            .map(|source| lumin_sfc::decompose(source, &source_index))
            .collect::<Result<Vec<_>, _>>()?;
        decompositions.sort_by(|left, right| left.source_id.cmp(&right.source_id));

        let embedded_parse_product_count = decompositions
            .iter()
            .map(|decomposition| decomposition.inline_scripts.len())
            .sum::<usize>();
        let mut embedded_by_parent = BTreeMap::<_, Vec<FileFacts>>::new();
        let mut embedded = decompositions
            .par_iter()
            .flat_map_iter(|decomposition| {
                decomposition
                    .inline_scripts
                    .iter()
                    .map(move |unit| (&decomposition.source_id, unit))
            })
            .map(|(parent, unit)| {
                lumin_js::extract_embedded(unit).map(|facts| (parent.clone(), facts))
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
        for decomposition in decompositions {
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
            sfc_facts.extend(analysis.file_facts);
        }

        let mut facts = physical_facts;
        facts.extend(sfc_facts);
        Ok(ExtractionOutput {
            facts: reduce_file_facts(facts),
            sfc_states,
            js_parse_product_count: physical_parse_product_count + embedded_parse_product_count,
        })
    })
}

type PayloadGroup<'a> = (
    SourceKind,
    lumin_js::JsModuleFormat,
    Arc<[u8]>,
    Vec<&'a SourceSnapshot>,
);

fn extract_physical_js(
    sources: &[SourceSnapshot],
    config: &SemanticConfigSnapshot,
    resolution_profile: Option<ResolutionProfile>,
) -> Result<(Vec<FileFacts>, usize), lumin_js::JsExtractError> {
    let mut grouped = BTreeMap::<
        (PayloadSnapshotId, SourceKind, lumin_js::JsModuleFormat),
        (Arc<[u8]>, Vec<&SourceSnapshot>),
    >::new();
    for source in sources.iter().filter(|source| source.kind.is_js_family()) {
        let module_format = source_module_format(source, config, resolution_profile);
        grouped
            .entry((
                source.payload_snapshot_id.clone(),
                source.kind,
                module_format,
            ))
            .or_insert_with(|| (Arc::clone(&source.bytes), Vec::new()))
            .1
            .push(source);
    }
    let parse_product_count = grouped.len();
    let groups = grouped
        .into_iter()
        .map(|((_payload_id, kind, module_format), (bytes, sources))| {
            (kind, module_format, bytes, sources)
        })
        .collect::<Vec<PayloadGroup<'_>>>();
    let facts = groups
        .into_par_iter()
        .map(|(kind, module_format, bytes, sources)| {
            let payload = lumin_js::parse_payload_with_module_format(kind, &bytes, module_format)?;
            Ok(sources
                .into_iter()
                .map(|source| {
                    lumin_js::bind_payload(
                        &payload,
                        &source.id,
                        SourceUnitId::Logical(source.id.clone()),
                    )
                })
                .collect::<Vec<_>>())
        })
        .collect::<Result<Vec<_>, lumin_js::JsExtractError>>()?
        .into_iter()
        .flatten()
        .collect();
    Ok((reduce_file_facts(facts), parse_product_count))
}

fn source_module_format(
    source: &SourceSnapshot,
    config: &SemanticConfigSnapshot,
    resolution_profile: Option<ResolutionProfile>,
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
    let node_profile_defines_package_format = matches!(
        resolution_profile,
        Some(ResolutionProfile::Node16 | ResolutionProfile::NodeNext)
    );
    if !extension_defines_format && !node_profile_defines_package_format {
        return lumin_js::JsModuleFormat::Unknown;
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
