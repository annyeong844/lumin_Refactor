use super::{CorpusInvocation, FeatureSet, RegistryRow, RequiredCheck};

const ARCHITECTURE_CHECK: &[RequiredCheck] = &[RequiredCheck::ArchitectureCheck];

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
    inv!("write_gate", "semantic_demands::failed_pre_write_rechecks_a_semantic_conflict_and_retains_prior_reservations"),
    inv!("write_gate", "semantic_demands::failed_close_rechecks_a_semantic_conflict_at_the_final_barrier"),
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
static INV_CACHE_CLEANUP_PUB_RACE: &[CorpusInvocation] = &[
    inv!("cache_cleanup_publication_race", "cache_cleanup_and_publication_serialize_through_one_exclusive_guard", LifecycleAndPublicationCrash),
];
#[rustfmt::skip]
static INV_RET_CRASH: &[CorpusInvocation] = &[
    inv!("retention_faults", "plan_commit_death_leaves_no_partial_plan_or_operation", RetentionCrash),
    inv!("retention_faults", "run_retention_recovers_every_physical_crash_boundary", RetentionCrash),
    inv!("retention_faults", "gate_retention_recovers_logical_crash_boundaries", RetentionCrash),
    inv!("retention_faults", "unknown_crash_selector_fails_before_retention_mutation", RetentionCrash),
];

// ---------------------------------------------------------------------------
// Registry — 91 Section 9 IDs, canonical order.
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
            required_checks: &[],
            determinism_shard_weight: 0,
        }
    };
    ($id:expr, $inv:expr) => {
        RegistryRow {
            id: $id,
            standard: Some($inv),
            determinism: Some($inv),
            store_crash: None,
            required_checks: &[],
            determinism_shard_weight: 0,
        }
    };
}
/// Standard + determinism row with a reviewed determinism scheduling cost.
macro_rules! row_sd_determinism_weight {
    ($id:expr, $inv:expr, $weight:expr) => {
        RegistryRow {
            id: $id,
            standard: Some($inv),
            determinism: Some($inv),
            store_crash: None,
            required_checks: &[],
            determinism_shard_weight: $weight,
        }
    };
}
/// Standard + determinism rows whose truth also requires architecture-check.
macro_rules! row_sd_arch {
    ($id:expr) => {
        RegistryRow {
            id: $id,
            standard: Some(&[]),
            determinism: Some(&[]),
            store_crash: None,
            required_checks: ARCHITECTURE_CHECK,
            determinism_shard_weight: 0,
        }
    };
    ($id:expr, $inv:expr) => {
        RegistryRow {
            id: $id,
            standard: Some($inv),
            determinism: Some($inv),
            store_crash: None,
            required_checks: ARCHITECTURE_CHECK,
            determinism_shard_weight: 0,
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
            required_checks: &[],
            determinism_shard_weight: 0,
        }
    };
    ($id:expr, $inv:expr) => {
        RegistryRow {
            id: $id,
            standard: None,
            determinism: None,
            store_crash: Some($inv),
            required_checks: &[],
            determinism_shard_weight: 0,
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
            required_checks: &[],
            determinism_shard_weight: 0,
        }
    };
    ($id:expr, $inv:expr) => {
        RegistryRow {
            id: $id,
            standard: Some($inv),
            determinism: Some($inv),
            store_crash: Some(&[]),
            required_checks: &[],
            determinism_shard_weight: 0,
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
    row_sd!("logical-source-physical-aliases", &[inv!("logical_source_aliases", "logical_source_physical_aliases_keep_context_and_reuse_payload")]),
    row_sd!("physical-alias-write-closure", INV_ALIAS),
    row_sd!("repo-path-codec-golden-vectors", &[inv!("repo_path_codec", "repo_path_codec_golden_vectors_round_trip_through_public_binary")]),
    row_sd!("extension-probe-precedence", &[inv!("extension_probe", "relative_extension_and_directory_probes_follow_frozen_precedence")]),
    row_sd!("declaration-type-space", &[inv!("path_and_declaration", "declaration_facts_satisfy_type_space_only")]),
    row_sd!("tsconfig-aliases", &[inv!("tsconfig_aliases", "tsconfig_aliases_follow_exact_wildcard_base_url_and_extends_precedence")]),
    row_sd!("tsconfig-extends-specifier-selection", &[
        inv!("tsconfig_extends_selection", "relative_extends_uses_exact_then_one_json_fallback"),
        inv!("tsconfig_extends_selection", "unsupported_extends_forms_create_no_hidden_probe"),
        inv!("tsconfig_extends_selection", "malformed_and_root_escaping_extends_hard_stop"),
        inv!("tsconfig_extends_selection", "workspace_identity_is_exact_and_duplicate_identity_keeps_inventory_ownership"),
        inv!("tsconfig_extends_selection", "missing_extends_reservation_conflicts_through_parent_alias_identity"),
    ]),
    row_sd!("workspace-package-extends-tsconfig-field", &[
        inv!("workspace_package_tsconfig", "custom_field_fallback_and_child_override_apply_through_public_behavior"),
        inv!("workspace_package_tsconfig", "malformed_and_package_escaping_fields_create_no_hidden_probe"),
        inv!("workspace_package_tsconfig", "repository_escaping_workspace_targets_hard_stop"),
        inv!("workspace_package_tsconfig", "missing_nonregular_and_cyclic_targets_are_scoped"),
        inv!("workspace_package_tsconfig", "selected_workspace_target_is_reserved_before_capture_and_retry_is_idempotent"),
    ]),
    row_sd!("tsconfig-module-suffixes-unsupported", &[
        inv!("tsconfig_unsupported_options", "module_suffixes_blocks_relative_probes_without_hiding_unaffected_fan_in"),
        inv!("tsconfig_unsupported_options", "module_suffixes_prewrite_withholds_authorization_and_retry_is_idempotent"),
    ]),
    row_sd!("tsconfig-custom-conditions-unsupported", &[
        inv!("tsconfig_unsupported_options", "custom_conditions_blocks_node_and_default_without_hiding_unaffected_selection"),
        inv!("tsconfig_unsupported_options", "custom_conditions_prewrite_withholds_authorization_and_retry_is_idempotent"),
    ]),
    row_sd!("tsconfig-root-dirs-unsupported", &[
        inv!("tsconfig_unsupported_options", "root_dirs::root_dirs_blocks_relative_probes_and_disables_only_affected_absence"),
        inv!("tsconfig_unsupported_options", "root_dirs::root_dirs_prewrite_excludes_candidate_reads_and_retry_is_idempotent"),
    ]),
    row_sd!("resolver-config-registry", &[
        inv!("resolver_config_registry", "supported_and_neutral_fields_follow_registry"),
        inv!("resolver_config_registry", "registry_failures_block_before_probing_and_override_cannot_hide"),
    ]),
    row_sd_arch!("resolver-config-registry-artifact", &[
        inv!("resolver_config_registry", "resolver_artifact_identity_is_public_and_frozen"),
    ]),
    row_sd_arch!("pnpm-workspace-registry-and-precedence", &[
        inv!("pnpm_workspace_public", "pnpm_precedence_and_missing_packages_are_public"),
        inv!("pnpm_workspace_public", "package_configs_pinned_forms_emit_typed_limitations"),
        inv!("pnpm_workspace_public", "malformed_pnpm_hard_stops_without_fallback"),
    ]),
    row_sd!("package-field-shape-families", &[
        inv!("package_field_shapes", "inventory_owned_shape_families_emit_exact_limitations_before_resolution"),
        inv!("package_field_shapes", "malformed_package_type_blocks_node_resolution_before_target_selection"),
        inv!("package_field_shapes", "malformed_public_entry_fields_stop_before_later_fallbacks"),
        inv!("package_unsupported_public", "unsupported_exports_shapes_never_select_fallbacks"),
        inv!("workspace_package_tsconfig", "malformed_and_package_escaping_fields_create_no_hidden_probe"),
        inv!("tsconfig_extends_selection", "workspace_identity_is_exact_and_duplicate_identity_keeps_inventory_ownership"),
    ]),
    row_sd!("workspace-package-exports", &[
        inv!("workspace_package_exports", "exact_and_pattern_exports_follow_edge_specific_conditions"),
        inv!("workspace_package_exports", "closed_package_exports_do_not_withhold_unrelated_dead_findings"),
        inv!("workspace_package_exports", "exports_protect_only_selected_public_identities"),
    ]),
    row_sd!("bundler-condition-excludes-node", &[inv!("package_condition_public", "bundler_excludes_node_in_value_and_type_lanes")]),
    row_sd!("legacy-node-exports-disabled", &[
        inv!("legacy_node_package_fields", "legacy_node_ignores_valid_and_malformed_fields_and_uses_main_and_typings"),
        inv!("legacy_node_package_fields", "enabled_profile_retains_field_applicability_after_legacy_run"),
    ]),
    row_sd!("exports-overlapping-patterns", &[
        inv!("workspace_package_exports", "overlapping_patterns_follow_comparator_independent_of_source_order"),
        inv!("package_unsupported_public", "invalid_exports_subpath_components_are_package_scoped_unsupported"),
    ]),
    row_sd!("exports-target-path-lowering", &[
        inv!("package_export_target_lowering", "one_star_target_lowers_to_the_expected_package_source"),
        inv!("package_export_target_lowering", "invalid_target_strings_are_package_scoped_and_never_publish_candidates"),
        inv!("package_export_target_lowering", "invalid_target_prewrite_excludes_the_candidate_and_retry_is_idempotent"),
        inv!("package_export_target_lowering", "physical_escape_is_unsupported_before_candidate_publication"),
        inv!("package_export_target_lowering", "empty_wildcard_target_still_reports_its_physical_escape"),
        inv!("package_export_target_lowering", "same_category_redirect_retargeting_invalidates_the_active_gate"),
        inv!("package_export_target_lowering", "hard_excluded_target_is_rejected_before_topology_pruning"),
        inv!("package_export_target_lowering", "hard_excluded_descendant_redirect_is_rejected_without_traversal"),
        inv!("package_export_target_lowering", "literal_target_escape_is_checked_before_extension_probe"),
        inv!("package_export_target_lowering", "more_specific_null_pattern_prevents_a_general_escape_probe"),
        inv!("package_export_target_lowering", "static_target_prefix_redirect_is_probed_without_sources"),
        inv!("package_export_target_lowering", "public_pattern_probe_avoids_exact_and_more_specific_key_collisions"),
        inv!("package_export_target_lowering", "redirect_into_hard_excluded_namespace_is_rejected_after_lowering"),
        inv!("package_export_target_lowering", "same_target_redirect_replacement_invalidates_the_active_gate"),
        inv!("package_export_target_lowering", "redirect_target_identity_blocks_a_physical_directory_writer"),
    ]),
    row_sd!("package-types-versions-unsupported", &[inv!("package_unsupported_public", "types_versions_blocks_unspecialized_type_fallback")]),
    row_sd!("package-exports-unsupported-shapes", &[inv!("package_unsupported_public", "unsupported_exports_shapes_never_select_fallbacks")]),
    row_sd!("module-format-conditions", &[
        inv!("module_format_conditions", "node_profiles_select_conditions_from_importer_format_and_edge_syntax"),
        inv!("module_format_scope_boundaries", "commonjs_wrapper_mutations_preserve_only_grounded_public_edges"),
        inv!("vue_public", "vue_inline_script_inherits_the_parent_node_importer_format"),
    ]),
    row_sd!("public-condition-union", &[
        inv!("package_condition_public", "supported_public_condition_lanes_union_only_selected_identity_namespaces"),
    ]),
    row_sd!("package-fields-no-exports", &[
        inv!("package_fallback_public", "package_fields_without_exports_select_role_scoped_public_targets"),
    ]),
    row_sd!("resolution-profile-selection", &[
        inv!("resolution_profile_selection", "resolution_profiles_follow_override_nearest_config_default_and_unsupported_rules"),
    ]),
    row_sd!("explicit-entry-selection", &[inv!("explicit_entry_selection", "explicit_entries_replace_deduplicate_and_preserve_alias_contexts")]),
    row_sd!("reachable-dead-sibling", &[inv!("core_dead_evidence", "reachable_module_keeps_zero_fan_in_sibling")]),
    row_sd!("public-reexport-sibling", &[inv!("core_dead_evidence", "public_reexport_protects_only_selected_identity")]),
    row_sd!("vue-entry", &[inv!("vue_public", "vue_entry_resolves_and_graph_completes")]),
    row_sd!("vue-inline-script-setup", &[inv!("vue_public", "vue_inline_script_setup_binds_template_components")]),
    row_sd!("vue-external-script", &[inv!("vue_public", "vue_external_script_attach_and_mode_conflict")]),
    row_sd!("vue-resolution-override", &[
        inv!("vue_resolution_override", "vue_embedded_scripts_follow_invocation_extension_rules_without_a_template_lane"),
        inv!("vue_resolution_override", "external_vue_template_binding_uses_attached_script_facts"),
        inv!("vue_resolution_override", "vue_resolution_profile_changes_sealed_analysis_input_identity"),
    ]),
    row_sd!("vue-missing-target", &[inv!("vue_public", "vue_missing_target_is_scoped_without_aborting_graph")]),
    row_sd!("vue-non-source-asset", &[inv!("vue_public", "vue_non_source_asset_does_not_probe_declarations")]),
    row_sd!("sfc-dialect-boundary", &[inv!("vue_public", "sfc_dialect_boundary_vue_complete_svelte_astro_unavailable")]),
    row_sd!("next-route-group", &[inv!("path_and_declaration", "next_route_group_characters_are_ordinary_path_bytes")]),
    row_sd!("dynamic-literal-member", &[
        inv!("dynamic_literal_member", "literal_dynamic_members_preserve_precision_across_bindings_callbacks_and_shadowing"),
    ]),
    row_sd!("dynamic-nonliteral", &[
        inv!("dynamic_nonliteral", "nonliteral_dynamic_imports_preserve_bounded_and_workspace_opacity"),
    ]),
    row_sd!("import-meta-glob", &[
        inv!("import_meta_glob", "relative_import_meta_globs_expand_and_unsupported_patterns_remain_scoped"),
    ]),
    row_sd!("cjs-computed", &[
        inv!("cjs_computed", "computed_commonjs_access_is_module_scoped_broad_value_evidence"),
    ]),
    row_sd!("parse-failure-propagation", &[
        inv!("parse_failure_propagation", "recoverable_parse_failures_preserve_module_uses_and_remain_file_scoped"),
        inv!("parse_failure_propagation", "unrecoverable_parse_failures_block_workspace_absence_and_gates"),
    ]),
    row_sd_arch!("limitation-scope-exhaustiveness"),
    row_sd!("nearest-manifest", &[
        inv!("nearest_manifest", "dependency_intents_lease_each_nearest_manifest_and_lockfile"),
        inv!("nearest_manifest", "dependency_owner_uncertainty_never_infers_a_lockfile"),
    ]),
    row_sd!("parallel-gates", &[inv!("write_gate", "overlapping_gate_is_rejected_and_operation_reuse_is_malformed")]),
    row_sd!("intervening-gate-transitions", &[inv!("write_gate", "transition_retention::disjoint_gates_reconcile_a_terminal_transition_on_retry")]),
    row_sd!("gate-path-identity", &[inv!("write_gate", "new_source_path_is_admitted_before_it_exists")]),
    row_sd!("repo-path-lossless", &[
        inv!("repo_path_lossless", "native_repository_paths_round_trip_through_public_queries_and_cursors"),
    ]),
    row_sd!("reserved-state-namespace"),
    row_sdc!("state-namespace-initialization"),
    row_sdc!("state-lock-replacement-split-brain"),
    row_sdc!("state-managed-parent-replacement"),
    row_sd!("gate-config-drift", &[inv!("write_gate", "protected_input_drift_is_stale")]),
    row_sd!("gate-self-semantic-write", &[inv!("write_gate", "planned_semantic_config_write_is_recaptured_and_attributed")]),
    row_sd!("gate-prewrite-observation", &[
        inv!("write_gate", "pre_write_observation_binds_promotion_and_interrupted_admission_leaves_no_active_lease"),
    ]),
    row_sd!("gate-semantic-read-closure", INV_SEM_READ),
    row_sd!("gate-semantic-read-closure-warm-cache"),
    row_sd!("cache-gate-context-projection"),
    row_sd_arch!("capability-availability-authority"),
    row_sd!("gate-unsealed-observation"),
    row_sd!("gate-analysis-input-reconciliation"),
    row_sd!("gate-final-observation"),
    row_sd_arch!("gate-lifecycle-effects", INV_GATE_EFFECTS),
    row_sd!("gate-immutable-opening-delta"),
    row_sd!("lifecycle-operation-idempotency", INV_IDEMP),
    row_sd!("gate-reopen-after-process-exit", &[inv!("write_gate", "pre_and_post_survive_process_reopen")]),
    row_sd!("unplanned-edit", INV_UNPLANNED),
    row_sd!("mixed-vue-gate"),
    row_sd!("required-capability-failure"),
    row_sd!("snapshot-and-latest", &[inv!("publication", "first_failed_attempt_remains_visible_without_a_completed_run")]),
    row_sd!("bounded-nested-query", INV_BNQ),
    row_sd!("collection-ordering"),
    row_sd!("capabilities-pagination", INV_CAP),
    row_sd!("request-path-escape", &[
        inv!("request_path_escape", "request_path_escape_distinguishes_malformed_stale_and_blocked_containment"),
    ]),
    row_sd!("corrupt-store"),
    row_c!("crash-publication", INV_CRASH_PUB),
    row_c!("concurrent-latest-publication", INV_CONC_PUB),
    row_c!("publication-retention-race", INV_PUB_RET_RACE),
    row_c!("cache-cleanup-publication-race", INV_CACHE_CLEANUP_PUB_RACE),
    row_sdc!("retention-latest-protection", INV_RET_LATEST),
    // This fixture emits 52 semantic captures per determinism variant. Keep its
    // three child processes off the same two-core runner as unrelated rows.
    row_sd_determinism_weight!("retention-plan-pagination", INV_RET_PAGINATION, 64),
    row_sdc!("retention-public-lookup", INV_RET_LOOKUP),
    row_sd!("retention-independent-pins", INV_RET_PINS),
    row_sd!("retention-active-transition-reference", &[inv!("write_gate", "transition_retention::disjoint_gates_reconcile_a_terminal_transition_on_retry")]),
    row_c!("retention-crash-protocol", INV_RET_CRASH),
    row_sdc!("lifecycle-store-migration"),
];
