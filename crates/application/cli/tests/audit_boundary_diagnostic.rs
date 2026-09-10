//! W9 external release children; the driver has no production instrumentation.
#[path = "support/audit_diagnostic.rs"]
mod probe;

#[test]
fn actual_release_children_report_exact_boundary_costs_and_durable_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    probe::actual_release_children_report_concrete_pool_and_unchanged_semantics(
        probe::DiagnosticVersion::Boundary,
    )
}

#[test]
fn diagnostic_transport_failure_preserves_exactly_one_committed_run()
-> Result<(), Box<dyn std::error::Error>> {
    probe::diagnostic_transport_failure_preserves_exactly_one_committed_run()
}

#[test]
fn original_audit_failure_has_no_completed_diagnostic_frame()
-> Result<(), Box<dyn std::error::Error>> {
    probe::original_audit_failure_has_no_completed_diagnostic_frame()
}
