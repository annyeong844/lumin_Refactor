//! Corpus foundation orchestration and public-row registry.
//!
//! Exit codes: 0 = all selected rows pass, 1 = behavior failures/unmapped, 2 = tool error.

use std::env;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureSet {
    None,
    LifecycleFault,
    PublicationCrash,
    RetentionCrash,
    PublicationAndRetentionCrash,
}
impl FeatureSet {
    pub fn cargo_features(self) -> &'static [&'static str] {
        match self {
            Self::None => &[],
            Self::LifecycleFault => &["lifecycle-test-fault"],
            Self::PublicationCrash => &["publication-test-crash"],
            Self::RetentionCrash => &["retention-test-crash"],
            Self::PublicationAndRetentionCrash => {
                &["publication-test-crash", "retention-test-crash"]
            }
        }
    }
    pub fn dir_key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::LifecycleFault => "lf",
            Self::PublicationCrash => "pc",
            Self::RetentionCrash => "rc",
            Self::PublicationAndRetentionCrash => "pcrc",
        }
    }
    pub fn is_crash(self) -> bool {
        matches!(
            self,
            Self::PublicationCrash | Self::RetentionCrash | Self::PublicationAndRetentionCrash
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorpusMode {
    Standard,
    Determinism,
    StoreCrash,
}
impl fmt::Display for CorpusMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Standard => "standard",
            Self::Determinism => "determinism",
            Self::StoreCrash => "store-crash",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Clone, Debug)]
pub struct CorpusInvocation {
    pub target: &'static str,
    pub filter: &'static str,
    pub features: FeatureSet,
}

/// Per-mode applicability for a registry row.
/// - `None` = mode not applicable to this row (row skipped in that mode).
/// - `Some(&[])` = applicable but unmapped (required, causes exit 1).
/// - `Some(&[..])` = mapped with concrete invocations.
#[derive(Clone, Debug)]
pub struct RegistryRow {
    pub id: &'static str,
    pub standard: Option<&'static [CorpusInvocation]>,
    pub determinism: Option<&'static [CorpusInvocation]>,
    pub store_crash: Option<&'static [CorpusInvocation]>,
}
impl RegistryRow {
    pub fn mode_invocations(&self, mode: CorpusMode) -> Option<&'static [CorpusInvocation]> {
        match mode {
            CorpusMode::Standard => self.standard,
            CorpusMode::Determinism => self.determinism,
            CorpusMode::StoreCrash => self.store_crash,
        }
    }
    pub fn is_applicable(&self, mode: CorpusMode) -> bool {
        self.mode_invocations(mode).is_some()
    }
    pub fn is_mapped(&self, mode: CorpusMode) -> bool {
        matches!(self.mode_invocations(mode), Some(s) if !s.is_empty())
    }
}

macro_rules! inv {
    ($t:expr, $f:expr) => {
        CorpusInvocation {
            target: $t,
            filter: $f,
            features: FeatureSet::None,
        }
    };
    ($t:expr, $f:expr, $feat:ident) => {
        CorpusInvocation {
            target: $t,
            filter: $f,
            features: FeatureSet::$feat,
        }
    };
}

// ---------------------------------------------------------------------------
// Invocation statics — corrected from authoritative `cargo test --list` output.
// ---------------------------------------------------------------------------

#[rustfmt::skip]
static INV_BNQ: &[CorpusInvocation] = &[
    inv!("bounded_nested_query", "cursor_integrity::cross_collection_cursor_rejected"),
    inv!("bounded_nested_query", "cursor_integrity::cross_finding_evidence_cursor_rejected"),
    inv!("bounded_nested_query", "cursor_integrity::cross_gate_and_run_vs_gate_cursor_rejected"),
    inv!("bounded_nested_query", "cursor_integrity::cross_gate_cursor_rejected"),
    inv!("bounded_nested_query", "cursor_integrity::cross_repository_cursor_rejected"),
    inv!("bounded_nested_query", "cursor_integrity::cross_run_cursor_rejected"),
    inv!("bounded_nested_query", "cursor_integrity::gate_revision_boundary_immutability"),
    inv!("bounded_nested_query", "cursor_integrity::run_cursor_survives_second_audit_mutation"),
    inv!("bounded_nested_query", "cursor_integrity::structured_tampered_cursor_rejected"),
    inv!("bounded_nested_query", "run_explain_evidence_pages_102_as_100_plus_2"),
    inv!("bounded_nested_query", "run_explain_relations_pages_101_as_100_plus_1"),
    inv!("bounded_nested_query", "run_findings_pages_102_as_100_plus_2"),
];
#[rustfmt::skip]
static INV_COLL: &[CorpusInvocation] = &[
    inv!("collection_ordering", "related_returns_relation_collection_for_run_finding"),
    inv!("collection_ordering", "related_missing_run_exits_2"),
    inv!("collection_ordering", "files_returns_file_findings_collection"),
    inv!("collection_ordering", "files_zero_match_exits_0_empty"),
    inv!("collection_ordering", "files_invalid_path_exits_2"),
    inv!("collection_ordering", "gate_list_requires_active_flag"),
    inv!("collection_ordering", "gate_list_active_returns_empty_collection"),
    inv!("collection_ordering", "gate_list_active_shows_open_gate"),
    inv!("collection_ordering", "gate_list_active_malformed_cursor_exits_2"),
];
#[rustfmt::skip]
static INV_CAP: &[CorpusInvocation] = &[
    inv!("capabilities_pagination", "binary_capabilities_pagination_without_state_directory"),
    inv!("capabilities_pagination", "binary_cursor_works_across_directories"),
    inv!("capabilities_pagination", "run_capabilities_pagination_and_cursor_survives_audit"),
    inv!("capabilities_pagination", "binary_and_run_cursors_cannot_cross_scope"),
    inv!("capabilities_pagination", "malformed_binary_cursor_rejected"),
    inv!("capabilities_pagination", "cross_run_capabilities_cursor_rejected"),
    inv!("capabilities_pagination", "cross_repository_run_cursor_rejected"),
    inv!("capabilities_pagination", "binary_cursor_is_repeatable_read_only_continuation"),
];
#[rustfmt::skip]
static INV_GATE_EFFECTS: &[CorpusInvocation] = &[
    inv!("write_gate", "introduced_grounded_finding_denies_and_records_its_delta"),
    inv!("write_gate", "resolved_grounded_finding_authorizes_and_remains_queryable"),
    inv!("write_gate", "unchanged_grounded_finding_remains_an_advisory_warning"),
    inv!("write_gate", "bounded_unresolved_edge_is_advisory_and_comparable"),
    inv!("write_gate", "unsupported_config_remains_a_required_evidence_gap"),
];
#[rustfmt::skip]
static INV_UNPLANNED: &[CorpusInvocation] = &[
    inv!("write_gate", "unexpected_new_source_denies_and_keeps_the_gate_active"),
    inv!("write_gate", "new_source_path_is_admitted_before_it_exists"),
    inv!("write_gate", "directory_lease_covers_new_descendants_and_conflicts_with_them"),
    inv!("write_gate", "empty_directory_gate_protects_all_opening_sources"),
    inv!("write_gate", "nonempty_directory_gate_protects_sources_outside_the_directory"),
];
#[rustfmt::skip]
static INV_ALIAS: &[CorpusInvocation] = &[
    inv!("write_gate", "physical_alias_closure_is_visible_and_rejects_a_late_unleased_alias"),
    inv!("write_gate", "physical_alias_members_are_reanalyzed_as_one_leased_payload"),
];
#[rustfmt::skip]
static INV_SEM_READ: &[CorpusInvocation] = &[
    inv!("write_gate", "semantic_demands::pre_write_reserves_semantic_demands_before_capture_and_retries_after_writer_terminal"),
    inv!("write_gate", "semantic_demands::close_time_new_semantic_demand_outside_lease_stays_unplanned_on_retry"),
];
#[rustfmt::skip]
static INV_IDEMP: &[CorpusInvocation] = &[
    inv!("lifecycle_operation_idempotency", "gate::gate_mutations_recover_post_commit_delivery_failure_without_duplication", LifecycleFault),
    inv!("lifecycle_operation_idempotency", "gate_retention::gate_retention_mutations_recover_post_commit_delivery_failure_without_duplication", LifecycleFault),
    inv!("lifecycle_operation_idempotency", "run_retention::retention_mutations_recover_post_commit_delivery_failure_without_duplication", LifecycleFault),
];
#[rustfmt::skip]
static INV_RET_LATEST: &[CorpusInvocation] = &[
    inv!("retention", "lifecycle::latest_attempt_and_completed_closures_survive_stale_confirmation"),
];
#[rustfmt::skip]
static INV_RET_PAGINATION: &[CorpusInvocation] = &[
    inv!("retention", "pagination::retention_plan_pages_survive_unrelated_repository_mutation_and_reject_cross_plan_cursor"),
];
#[rustfmt::skip]
static INV_RET_LOOKUP: &[CorpusInvocation] = &[
    inv!("retention", "lifecycle::retention_truth_survives_public_process_reopen"),
];
#[rustfmt::skip]
static INV_RET_PINS: &[CorpusInvocation] = &[
    inv!("retention", "pins::independent_public_pins_keep_a_run_protected_until_the_last_unpin"),
];
#[rustfmt::skip]
static INV_CRASH_PUB: &[CorpusInvocation] = &[
    inv!("publication_faults", "pre_start_crashes_preserve_sequence_rules_without_inventing_attempts", PublicationCrash),
    inv!("publication_faults", "running_crashes_become_interrupted_without_replacing_the_completed_run", PublicationCrash),
    inv!("publication_faults", "terminal_crashes_recover_the_completed_run_and_monotonic_pointers", PublicationCrash),
    inv!("publication_faults", "explicit_run_overview_does_not_mix_in_repository_latest_attempt", PublicationCrash),
    inv!("publication_faults", "unknown_crash_selector_fails_before_attempt_allocation", PublicationCrash),
    inv!("publication_faults", "malformed_latest_pending_file_is_not_silently_discarded", PublicationCrash),
    inv!("publication_faults", "empty_unreferenced_liveness_lock_is_not_silently_discarded", PublicationCrash),
    inv!("publication_faults", "analysis_failure_publishes_a_terminal_attempt_and_releases_liveness", PublicationCrash),
];
#[rustfmt::skip]
static INV_CONC_PUB: &[CorpusInvocation] = &[
    inv!("publication_concurrency", "concurrent_latest_publication_preserves_monotonic_fields", PublicationCrash),
    inv!("publication_concurrency", "concurrent_latest_publication_merges_attempt_and_completed_independently", PublicationCrash),
];
#[rustfmt::skip]
static INV_PUB_RET_RACE: &[CorpusInvocation] = &[
    inv!("publication_retention_race", "publication_first_makes_retention_confirmation_stale", PublicationAndRetentionCrash),
    inv!("publication_retention_race", "retention_first_prevents_pointer_publication_with_typed_result", PublicationAndRetentionCrash),
    inv!("publication_retention_race", "retention_cannot_adopt_the_latest_uncatalogued_attempt_as_an_orphan", PublicationAndRetentionCrash),
    inv!("publication_retention_race", "retention_rejects_corrupt_latest_uncatalogued_run_payload", PublicationAndRetentionCrash),
    inv!("publication_retention_race", "pruning_crash_and_publisher_death_cannot_recover_a_pointer", PublicationAndRetentionCrash),
];
#[rustfmt::skip]
static INV_RET_CRASH: &[CorpusInvocation] = &[
    inv!("retention_faults", "plan_commit_death_leaves_no_partial_plan_or_operation", RetentionCrash),
    inv!("retention_faults", "run_retention_recovers_every_physical_crash_boundary", RetentionCrash),
    inv!("retention_faults", "gate_retention_recovers_logical_crash_boundaries", RetentionCrash),
    inv!("retention_faults", "unknown_crash_selector_fails_before_retention_mutation", RetentionCrash),
];

// ---------------------------------------------------------------------------
// Registry — 90 Section 9 IDs, canonical order.
// Per-mode fields: None = not applicable, Some(&[]) = required but unmapped,
// Some(&[..]) = mapped.
// ---------------------------------------------------------------------------

/// Standard-applicable, determinism-applicable, no store-crash.
macro_rules! row_sd {
    ($id:expr) => {
        RegistryRow {
            id: $id,
            standard: Some(&[]),
            determinism: Some(&[]),
            store_crash: None,
        }
    };
    ($id:expr, $inv:expr) => {
        RegistryRow {
            id: $id,
            standard: Some($inv),
            determinism: Some(&[]),
            store_crash: None,
        }
    };
}
/// Crash-only row: not standard, not determinism, yes store-crash.
macro_rules! row_c {
    ($id:expr) => {
        RegistryRow {
            id: $id,
            standard: None,
            determinism: None,
            store_crash: Some(&[]),
        }
    };
    ($id:expr, $inv:expr) => {
        RegistryRow {
            id: $id,
            standard: None,
            determinism: None,
            store_crash: Some($inv),
        }
    };
}
/// Standard + determinism + store-crash applicable (state/migration/retention rows relevant to crash).
macro_rules! row_sdc {
    ($id:expr) => {
        RegistryRow {
            id: $id,
            standard: Some(&[]),
            determinism: Some(&[]),
            store_crash: Some(&[]),
        }
    };
    ($id:expr, $inv:expr) => {
        RegistryRow {
            id: $id,
            standard: Some($inv),
            determinism: Some(&[]),
            store_crash: Some(&[]),
        }
    };
}

#[rustfmt::skip]
pub static REGISTRY: &[RegistryRow] = &[
    row_sd!("plain-esm", &[inv!("core_dead_evidence", "plain_esm_preserves_namespace_and_side_effect_distinctions")]),
    row_sd!("ignore-precedence", &[inv!("ignore_precedence", "ignore_precedence_follows_section_3_1_scan_admission")]),
    row_sd!("scan-invocation-containment", &[inv!("scan_invocation_containment", "scan_flags_and_containment_round_trip_through_public_gate")]),
    row_sd!("source-role-classification", &[inv!("source_role_public", "source_role_classification_persists_rule_reason_and_source")]),
    row_sd!("source-role-findings-remain-visible", &[inv!("source_role_public", "source_role_findings_remain_visible_and_only_explicit_filtering_narrows")]),
    row_sd!("logical-source-physical-aliases"),
    row_sd!("physical-alias-write-closure", INV_ALIAS),
    row_sd!("repo-path-codec-golden-vectors"),
    row_sd!("extension-probe-precedence", &[inv!("extension_probe", "relative_extension_and_directory_probes_follow_frozen_precedence")]),
    row_sd!("declaration-type-space", &[inv!("path_and_declaration", "declaration_facts_satisfy_type_space_only")]),
    row_sd!("tsconfig-aliases"),
    row_sd!("tsconfig-extends-specifier-selection"),
    row_sd!("workspace-package-extends-tsconfig-field"),
    row_sd!("tsconfig-module-suffixes-unsupported"),
    row_sd!("tsconfig-custom-conditions-unsupported"),
    row_sd!("tsconfig-root-dirs-unsupported"),
    row_sd!("resolver-config-registry"),
    row_sd!("resolver-config-registry-artifact"),
    row_sd!("pnpm-workspace-registry-and-precedence"),
    row_sd!("package-field-shape-families"),
    row_sd!("workspace-package-exports"),
    row_sd!("bundler-condition-excludes-node", &[inv!("package_condition_public", "bundler_excludes_node_in_value_and_type_lanes")]),
    row_sd!("legacy-node-exports-disabled"),
    row_sd!("exports-overlapping-patterns"),
    row_sd!("exports-target-path-lowering"),
    row_sd!("package-types-versions-unsupported", &[inv!("package_unsupported_public", "types_versions_blocks_unspecialized_type_fallback")]),
    row_sd!("package-exports-unsupported-shapes", &[inv!("package_unsupported_public", "unsupported_exports_shapes_never_select_fallbacks")]),
    row_sd!("module-format-conditions"),
    row_sd!("public-condition-union"),
    row_sd!("package-fields-no-exports"),
    row_sd!("resolution-profile-selection"),
    row_sd!("explicit-entry-selection", &[inv!("explicit_entry_selection", "explicit_entries_replace_deduplicate_and_preserve_alias_contexts")]),
    row_sd!("reachable-dead-sibling", &[inv!("core_dead_evidence", "reachable_module_keeps_zero_fan_in_sibling")]),
    row_sd!("public-reexport-sibling", &[inv!("core_dead_evidence", "public_reexport_protects_only_selected_identity")]),
    row_sd!("vue-entry", &[inv!("vue_public", "vue_entry_resolves_and_graph_completes")]),
    row_sd!("vue-inline-script-setup", &[inv!("vue_public", "vue_inline_script_setup_binds_template_components")]),
    row_sd!("vue-external-script", &[inv!("vue_public", "vue_external_script_attach_and_mode_conflict")]),
    row_sd!("vue-resolution-override"),
    row_sd!("vue-missing-target", &[inv!("vue_public", "vue_missing_target_is_scoped_without_aborting_graph")]),
    row_sd!("vue-non-source-asset", &[inv!("vue_public", "vue_non_source_asset_does_not_probe_declarations")]),
    row_sd!("sfc-dialect-boundary", &[inv!("vue_public", "sfc_dialect_boundary_vue_complete_svelte_astro_unavailable")]),
    row_sd!("next-route-group", &[inv!("path_and_declaration", "next_route_group_characters_are_ordinary_path_bytes")]),
    row_sd!("dynamic-literal-member"),
    row_sd!("dynamic-nonliteral"),
    row_sd!("import-meta-glob"),
    row_sd!("cjs-computed"),
    row_sd!("parse-failure-propagation"),
    row_sd!("limitation-scope-exhaustiveness"),
    row_sd!("nearest-manifest"),
    row_sd!("parallel-gates", &[inv!("write_gate", "overlapping_gate_is_rejected_and_operation_reuse_is_malformed")]),
    row_sd!("intervening-gate-transitions", &[inv!("write_gate", "transition_retention::disjoint_gates_reconcile_a_terminal_transition_on_retry")]),
    row_sd!("gate-path-identity", &[inv!("write_gate", "new_source_path_is_admitted_before_it_exists")]),
    row_sd!("repo-path-lossless"),
    row_sd!("reserved-state-namespace"),
    row_sdc!("state-namespace-initialization"),
    row_sdc!("state-lock-replacement-split-brain"),
    row_sdc!("state-managed-parent-replacement"),
    row_sd!("gate-config-drift", &[inv!("write_gate", "protected_input_drift_is_stale")]),
    row_sd!("gate-self-semantic-write", &[inv!("write_gate", "planned_semantic_config_write_is_recaptured_and_attributed")]),
    row_sd!("gate-prewrite-observation"),
    row_sd!("gate-semantic-read-closure", INV_SEM_READ),
    row_sd!("gate-semantic-read-closure-warm-cache"),
    row_sd!("cache-gate-context-projection"),
    row_sd!("capability-availability-authority"),
    row_sd!("gate-unsealed-observation"),
    row_sd!("gate-analysis-input-reconciliation"),
    row_sd!("gate-final-observation"),
    row_sd!("gate-lifecycle-effects", INV_GATE_EFFECTS),
    row_sd!("gate-immutable-opening-delta"),
    row_sd!("lifecycle-operation-idempotency", INV_IDEMP),
    row_sd!("gate-reopen-after-process-exit", &[inv!("write_gate", "pre_and_post_survive_process_reopen")]),
    row_sd!("unplanned-edit", INV_UNPLANNED),
    row_sd!("mixed-vue-gate"),
    row_sd!("required-capability-failure"),
    row_sd!("snapshot-and-latest", &[inv!("publication", "first_failed_attempt_remains_visible_without_a_completed_run")]),
    row_sd!("bounded-nested-query", INV_BNQ),
    row_sd!("collection-ordering", INV_COLL),
    row_sd!("capabilities-pagination", INV_CAP),
    row_sd!("request-path-escape"),
    row_sd!("corrupt-store"),
    row_c!("crash-publication", INV_CRASH_PUB),
    row_c!("concurrent-latest-publication", INV_CONC_PUB),
    row_c!("publication-retention-race", INV_PUB_RET_RACE),
    row_sdc!("retention-latest-protection", INV_RET_LATEST),
    row_sd!("retention-plan-pagination", INV_RET_PAGINATION),
    row_sdc!("retention-public-lookup", INV_RET_LOOKUP),
    row_sd!("retention-independent-pins", INV_RET_PINS),
    row_sd!("retention-active-transition-reference", &[inv!("write_gate", "transition_retention::disjoint_gates_reconcile_a_terminal_transition_on_retry")]),
    row_c!("retention-crash-protocol", INV_RET_CRASH),
    row_sdc!("lifecycle-store-migration"),
];

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

pub struct CorpusArgs {
    pub mode: CorpusMode,
    pub format: OutputFormat,
}

pub fn parse_args(args: &[String]) -> Result<CorpusArgs, String> {
    let (mut mode, mut format) = (CorpusMode::Standard, OutputFormat::Human);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--determinism" => {
                if mode != CorpusMode::Standard {
                    return Err("conflicting mode flags".into());
                }
                mode = CorpusMode::Determinism;
            }
            "--store-crash" => {
                if mode != CorpusMode::Standard {
                    return Err("conflicting mode flags".into());
                }
                mode = CorpusMode::StoreCrash;
            }
            "--format" => {
                i += 1;
                match args.get(i).map(|s| s.as_str()) {
                    Some("human") => format = OutputFormat::Human,
                    Some("json") => format = OutputFormat::Json,
                    Some(o) => return Err(format!("unknown format: {o}")),
                    None => return Err("--format requires a value".into()),
                }
            }
            o => return Err(format!("unknown argument: {o}")),
        }
        i += 1;
    }
    Ok(CorpusArgs { mode, format })
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

pub fn validate_registry() -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for row in REGISTRY {
        if row.id.is_empty() {
            return Err("empty corpus ID".into());
        }
        if !seen.insert(row.id) {
            return Err(format!("duplicate ID: {}", row.id));
        }
        // Verify invocation uniqueness per mode and feature consistency.
        for mode in [
            CorpusMode::Standard,
            CorpusMode::Determinism,
            CorpusMode::StoreCrash,
        ] {
            if let Some(invs) = row.mode_invocations(mode) {
                let mut inv_set = std::collections::HashSet::new();
                for inv in invs {
                    if inv.target.is_empty() {
                        return Err(format!("empty target in {}", row.id));
                    }
                    if inv.filter.is_empty() {
                        return Err(format!("empty filter in {}", row.id));
                    }
                    let key = (inv.target, inv.filter);
                    if !inv_set.insert(key) {
                        return Err(format!(
                            "duplicate invocation {}/{} in {} mode {}",
                            inv.target, inv.filter, row.id, mode
                        ));
                    }
                    // Feature consistency: standard allows None and LifecycleFault only.
                    // StoreCrash requires crash features only.
                    match mode {
                        CorpusMode::Standard => {
                            if inv.features.is_crash() {
                                return Err(format!(
                                    "crash feature in standard mode row {}",
                                    row.id
                                ));
                            }
                        }
                        CorpusMode::StoreCrash => {
                            if !inv.features.is_crash() && inv.features != FeatureSet::None {
                                return Err(format!(
                                    "non-crash non-none feature in store-crash row {}",
                                    row.id
                                ));
                            }
                        }
                        CorpusMode::Determinism => {}
                    }
                }
            }
        }
    }
    if REGISTRY.len() != 90 {
        return Err(format!("registry has {} rows, expected 90", REGISTRY.len()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Spec extraction (test-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub fn extract_spec_ids(spec_text: &str) -> Result<Vec<&str>, String> {
    let mut ids = Vec::new();
    let (mut in_s9, mut in_table, mut hdr) = (false, false, false);
    for line in spec_text.lines() {
        if line.starts_with("## 9.") || line.starts_with("## 9 ") {
            in_s9 = true;
            continue;
        }
        if in_s9 && line.starts_with("## ") && !line.starts_with("## 9") {
            break;
        }
        if !in_s9 {
            continue;
        }
        let t = line.trim();
        if t.starts_with("| Corpus case") {
            in_table = true;
            continue;
        }
        if in_table && t.starts_with("| ---") {
            hdr = true;
            continue;
        }
        if in_table && hdr && t.starts_with("| `") {
            let after = &t[3..];
            if let Some(end) = after.find('`') {
                ids.push(&after[..end]);
            }
        }
        if in_table && hdr && !t.starts_with('|') && !t.is_empty() {
            break;
        }
    }
    Ok(ids)
}

// ---------------------------------------------------------------------------
// Marker system
// ---------------------------------------------------------------------------

static MARKER_COUNTER: AtomicU64 = AtomicU64::new(0);

fn marker_path(row_id: &str) -> PathBuf {
    let pid = std::process::id();
    let seq = MARKER_COUNTER.fetch_add(1, Ordering::Relaxed);
    let safe: String = row_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    env::temp_dir().join(format!("lumin_corpus_{pid}_{seq}_{safe}.marker"))
}

/// Validate marker file: at least `expected` lines, every line must equal
/// `row_id`, and no empty or non-matching lines are permitted.
pub fn validate_marker(path: &Path, row_id: &str, expected: usize) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|e| format!("marker read: {e}"))?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() < expected {
        return Err(format!(
            "marker has {} lines, need >= {expected}",
            lines.len()
        ));
    }
    for (i, l) in lines.iter().enumerate() {
        if l.is_empty() {
            return Err(format!("marker line {i} is empty"));
        }
        if *l != row_id {
            return Err(format!("marker line {i} is {:?}, expected {:?}", l, row_id));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Workspace root
// ---------------------------------------------------------------------------

fn find_workspace_root() -> Result<PathBuf, String> {
    let manifest_dir =
        env::var("CARGO_MANIFEST_DIR").map_err(|_| "CARGO_MANIFEST_DIR not set".to_string())?;
    // xtask is at tools/xtask, so parent twice gives workspace root.
    let ws = PathBuf::from(&manifest_dir)
        .parent()
        .and_then(|p| p.parent())
        .ok_or_else(|| format!("cannot derive workspace root from {manifest_dir}"))?
        .to_path_buf();
    let ct = ws.join("Cargo.toml");
    let content =
        fs::read_to_string(&ct).map_err(|e| format!("cannot read {}: {e}", ct.display()))?;
    if !content.contains("[workspace]") {
        return Err(format!("{} does not contain [workspace]", ct.display()));
    }
    Ok(ws)
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

fn target_dir(ws: &Path, mode: CorpusMode, feat: FeatureSet) -> PathBuf {
    let m = match mode {
        CorpusMode::Standard => "s",
        CorpusMode::Determinism => "d",
        CorpusMode::StoreCrash => "c",
    };
    ws.join("target").join("xc").join(m).join(feat.dir_key())
}

struct InvResult {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_inv(
    ws: &Path,
    inv: &CorpusInvocation,
    mode: CorpusMode,
    row_id: &str,
    marker: &Path,
) -> InvResult {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let td = target_dir(ws, mode, inv.features);
    let mut cmd = Command::new(&cargo);
    cmd.current_dir(ws)
        .arg("test")
        .arg("--locked")
        .arg("-p")
        .arg("lumin-cli")
        .arg("--test")
        .arg(inv.target);
    let feats = inv.features.cargo_features();
    if !feats.is_empty() {
        cmd.arg("--features").arg(feats.join(","));
    }
    cmd.arg(inv.filter)
        .arg("--")
        .arg("--exact")
        .arg("--nocapture");
    cmd.env("CARGO_TARGET_DIR", td.to_string_lossy().as_ref());
    cmd.env("LUMIN_CORPUS_ROW", row_id);
    cmd.env(
        "LUMIN_CORPUS_CHILD_MARKER",
        marker.to_string_lossy().as_ref(),
    );
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    match cmd.output() {
        Ok(o) => InvResult {
            success: o.status.success(),
            stdout: o.stdout,
            stderr: o.stderr,
        },
        Err(e) => InvResult {
            success: false,
            stdout: Vec::new(),
            stderr: format!("spawn: {e}").into_bytes(),
        },
    }
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct RowResult {
    id: &'static str,
    mapped: bool,
    passed: bool,
    invocations: usize,
    marker_ok: bool,
}

fn print_human(res: &[RowResult], mode: CorpusMode) {
    let (mapped, passed) = (
        res.iter().filter(|r| r.mapped).count(),
        res.iter().filter(|r| r.passed).count(),
    );
    let (unmapped, failed) = (res.len() - mapped, mapped - passed);
    println!("\n=== corpus foundation: {mode} ===");
    println!(
        "total: {}  mapped: {mapped}  passed: {passed}  unmapped: {unmapped}  failed: {failed}\n",
        res.len()
    );
    if unmapped > 0 {
        println!("unmapped:");
        for r in res.iter().filter(|r| !r.mapped) {
            println!("  {}", r.id);
        }
        println!();
    }
    if failed > 0 {
        println!("failed:");
        for r in res.iter().filter(|r| r.mapped && !r.passed) {
            println!("  {}", r.id);
        }
        println!();
    }
}

fn print_json(res: &[RowResult], mode: CorpusMode) -> Result<(), String> {
    let rows: Vec<serde_json::Value> = res
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "mapped": r.mapped,
                "passed": r.passed,
                "invocations": r.invocations,
                "markerValidated": r.marker_ok,
            })
        })
        .collect();
    let (mapped, passed) = (
        res.iter().filter(|r| r.mapped).count(),
        res.iter().filter(|r| r.passed).count(),
    );
    let s = serde_json::json!({
        "mode": mode.to_string(),
        "totalRows": res.len(),
        "mapped": mapped,
        "passed": passed,
        "unmapped": res.len() - mapped,
        "failed": mapped - passed,
        "rows": rows,
    });
    let text = serde_json::to_string_pretty(&s).map_err(|e| format!("json serialization: {e}"))?;
    println!("{text}");
    Ok(())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(args: &[String]) -> ExitCode {
    if args.first().map(|s| s.as_str()) != Some("foundation") {
        eprintln!("[CORPUS ERROR] subcommand must be 'foundation'");
        return ExitCode::from(2);
    }
    let parsed = match parse_args(&args[1..]) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[CORPUS ERROR] {e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = validate_registry() {
        eprintln!("[REGISTRY ERROR] {e}");
        return ExitCode::from(2);
    }
    let ws = match find_workspace_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[TOOL ERROR] {e}");
            return ExitCode::from(2);
        }
    };
    let selected: Vec<&RegistryRow> = REGISTRY
        .iter()
        .filter(|r| r.is_applicable(parsed.mode))
        .collect();

    if selected.is_empty() {
        eprintln!("[CORPUS ERROR] mode {} selects zero rows", parsed.mode);
        return ExitCode::from(2);
    }

    let (mut results, mut has_fail, mut has_unmap) =
        (Vec::with_capacity(selected.len()), false, false);
    for row in &selected {
        if !row.is_mapped(parsed.mode) {
            results.push(RowResult {
                id: row.id,
                mapped: false,
                passed: false,
                invocations: 0,
                marker_ok: false,
            });
            has_unmap = true;
            continue;
        }
        let Some(invs) = row.mode_invocations(parsed.mode) else {
            eprintln!(
                "[REGISTRY ERROR] mapped row {} lacks mode {} invocations",
                row.id, parsed.mode
            );
            return ExitCode::from(2);
        };
        let mp = marker_path(row.id);
        let _ = fs::remove_file(&mp);
        let (mut ok, mut succ) = (true, 0usize);
        for inv in invs {
            let r = run_inv(&ws, inv, parsed.mode, row.id, &mp);
            if r.success {
                succ += 1;
            } else {
                ok = false;
                eprintln!("--- FAIL: {} / {} {} ---", row.id, inv.target, inv.filter);
                let _ = std::io::stderr().write_all(&r.stderr);
                let _ = std::io::stdout().write_all(&r.stdout);
            }
        }
        let m_ok = if ok && succ > 0 {
            match validate_marker(&mp, row.id, succ) {
                Ok(()) => {
                    let _ = fs::remove_file(&mp);
                    true
                }
                Err(e) => {
                    eprintln!("[MARKER] {}: {e}", row.id);
                    ok = false;
                    false
                }
            }
        } else {
            false
        };
        if !ok {
            has_fail = true;
        }
        results.push(RowResult {
            id: row.id,
            mapped: true,
            passed: ok,
            invocations: invs.len(),
            marker_ok: m_ok,
        });
    }
    match parsed.format {
        OutputFormat::Human => print_human(&results, parsed.mode),
        OutputFormat::Json => {
            if let Err(e) = print_json(&results, parsed.mode) {
                eprintln!("[TOOL ERROR] {e}");
                return ExitCode::from(2);
            }
        }
    }
    if has_fail || has_unmap {
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_default() -> Result<(), String> {
        let a = parse_args(&[])?;
        assert_eq!(a.mode, CorpusMode::Standard);
        assert_eq!(a.format, OutputFormat::Human);
        Ok(())
    }

    #[test]
    fn parse_det_json() -> Result<(), String> {
        let a = parse_args(&["--determinism".into(), "--format".into(), "json".into()])?;
        assert_eq!(a.mode, CorpusMode::Determinism);
        assert_eq!(a.format, OutputFormat::Json);
        Ok(())
    }

    #[test]
    fn parse_crash() -> Result<(), String> {
        let a = parse_args(&["--store-crash".into()])?;
        assert_eq!(a.mode, CorpusMode::StoreCrash);
        Ok(())
    }

    #[test]
    fn conflicting() {
        assert!(parse_args(&["--determinism".into(), "--store-crash".into()]).is_err());
    }

    #[test]
    fn unknown_arg() {
        assert!(parse_args(&["--bogus".into()]).is_err());
    }

    #[test]
    fn unknown_fmt() {
        assert!(parse_args(&["--format".into(), "yaml".into()]).is_err());
    }

    #[test]
    fn missing_fmt() {
        assert!(parse_args(&["--format".into()]).is_err());
    }

    #[test]
    fn registry_90() {
        assert_eq!(REGISTRY.len(), 90);
    }

    #[test]
    fn registry_valid() -> Result<(), String> {
        validate_registry()
    }

    #[test]
    fn registry_unique() {
        let mut s = std::collections::HashSet::new();
        for r in REGISTRY {
            assert!(s.insert(r.id), "dup {}", r.id);
        }
    }

    #[test]
    fn registry_no_empty_invocations() {
        for r in REGISTRY {
            for mode in [
                CorpusMode::Standard,
                CorpusMode::Determinism,
                CorpusMode::StoreCrash,
            ] {
                if let Some(invs) = r.mode_invocations(mode) {
                    for i in invs {
                        assert!(
                            !i.target.is_empty(),
                            "empty target in {} mode {}",
                            r.id,
                            mode
                        );
                        assert!(
                            !i.filter.is_empty(),
                            "empty filter in {} mode {}",
                            r.id,
                            mode
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn standard_has_applicable_rows() {
        let count = REGISTRY
            .iter()
            .filter(|r| r.is_applicable(CorpusMode::Standard))
            .count();
        assert!(count > 50, "standard applicable: {count}");
    }

    #[test]
    fn determinism_has_applicable_rows() {
        let count = REGISTRY
            .iter()
            .filter(|r| r.is_applicable(CorpusMode::Determinism))
            .count();
        assert!(count > 50, "determinism applicable: {count}");
    }

    #[test]
    fn determinism_has_unmapped_rows() {
        let unmapped = REGISTRY
            .iter()
            .filter(|r| {
                r.is_applicable(CorpusMode::Determinism) && !r.is_mapped(CorpusMode::Determinism)
            })
            .count();
        assert!(unmapped > 0, "determinism should have unmapped rows, got 0");
    }

    #[test]
    fn store_crash_has_applicable_rows() {
        let count = REGISTRY
            .iter()
            .filter(|r| r.is_applicable(CorpusMode::StoreCrash))
            .count();
        assert!(count >= 4, "store-crash applicable: {count}");
    }

    #[test]
    fn spec_90() -> Result<(), String> {
        let ids = extract_spec_ids(include_str!("../../../specs/001-foundation-slice.md"))?;
        assert_eq!(ids.len(), 90, "{ids:?}");
        Ok(())
    }

    #[test]
    fn spec_order() -> Result<(), String> {
        let ids = extract_spec_ids(include_str!("../../../specs/001-foundation-slice.md"))?;
        let reg: Vec<&str> = REGISTRY.iter().map(|r| r.id).collect();
        assert_eq!(ids, reg);
        Ok(())
    }

    #[test]
    fn marker_ok() -> Result<(), String> {
        let d = tempfile::tempdir().map_err(|e| e.to_string())?;
        let p = d.path().join("m");
        fs::write(&p, "row-a\nrow-a\nrow-a\n").map_err(|e| e.to_string())?;
        validate_marker(&p, "row-a", 3)
    }

    #[test]
    fn marker_excess_ok() -> Result<(), String> {
        let d = tempfile::tempdir().map_err(|e| e.to_string())?;
        let p = d.path().join("m");
        fs::write(&p, "row-a\nrow-a\nrow-a\nrow-a\n").map_err(|e| e.to_string())?;
        validate_marker(&p, "row-a", 3)
    }

    #[test]
    fn marker_short() -> Result<(), String> {
        let d = tempfile::tempdir().map_err(|e| e.to_string())?;
        let p = d.path().join("m");
        fs::write(&p, "row-a\n").map_err(|e| e.to_string())?;
        assert!(validate_marker(&p, "row-a", 3).is_err());
        Ok(())
    }

    #[test]
    fn marker_missing() -> Result<(), String> {
        let d = tempfile::tempdir().map_err(|e| e.to_string())?;
        assert!(validate_marker(&d.path().join("x"), "row-a", 1).is_err());
        Ok(())
    }

    #[test]
    fn marker_wrong_id() -> Result<(), String> {
        let d = tempfile::tempdir().map_err(|e| e.to_string())?;
        let p = d.path().join("m");
        fs::write(&p, "wrong-id\nwrong-id\n").map_err(|e| e.to_string())?;
        assert!(validate_marker(&p, "row-a", 2).is_err());
        Ok(())
    }

    #[test]
    fn marker_mixed_ids() -> Result<(), String> {
        let d = tempfile::tempdir().map_err(|e| e.to_string())?;
        let p = d.path().join("m");
        fs::write(&p, "row-a\nrow-b\n").map_err(|e| e.to_string())?;
        assert!(validate_marker(&p, "row-a", 2).is_err());
        Ok(())
    }

    #[test]
    fn marker_empty_line() -> Result<(), String> {
        let d = tempfile::tempdir().map_err(|e| e.to_string())?;
        let p = d.path().join("m");
        fs::write(&p, "row-a\n\nrow-a\n").map_err(|e| e.to_string())?;
        assert!(validate_marker(&p, "row-a", 2).is_err());
        Ok(())
    }

    #[test]
    fn feat_flags() {
        assert!(FeatureSet::None.cargo_features().is_empty());
        assert_eq!(
            FeatureSet::PublicationAndRetentionCrash
                .cargo_features()
                .len(),
            2
        );
    }

    #[test]
    fn bnq_has_12_invocations() {
        assert_eq!(INV_BNQ.len(), 12);
    }

    #[test]
    fn mode_counts() {
        let std_applicable = REGISTRY
            .iter()
            .filter(|r| r.is_applicable(CorpusMode::Standard))
            .count();
        let det_applicable = REGISTRY
            .iter()
            .filter(|r| r.is_applicable(CorpusMode::Determinism))
            .count();
        let crash_applicable = REGISTRY
            .iter()
            .filter(|r| r.is_applicable(CorpusMode::StoreCrash))
            .count();
        let std_mapped = REGISTRY
            .iter()
            .filter(|r| r.is_mapped(CorpusMode::Standard))
            .count();
        let det_mapped = REGISTRY
            .iter()
            .filter(|r| r.is_mapped(CorpusMode::Determinism))
            .count();
        let crash_mapped = REGISTRY
            .iter()
            .filter(|r| r.is_mapped(CorpusMode::StoreCrash))
            .count();
        // All modes must have >0 applicable.
        assert!(std_applicable > 0);
        assert!(det_applicable > 0);
        assert!(crash_applicable > 0);
        // Report for task closeout (not actual assertion failure).
        eprintln!(
            "Standard:     applicable={std_applicable} mapped={std_mapped} unmapped={}",
            std_applicable - std_mapped
        );
        eprintln!(
            "Determinism:  applicable={det_applicable} mapped={det_mapped} unmapped={}",
            det_applicable - det_mapped
        );
        eprintln!(
            "StoreCrash:   applicable={crash_applicable} mapped={crash_mapped} unmapped={}",
            crash_applicable - crash_mapped
        );
    }
}
