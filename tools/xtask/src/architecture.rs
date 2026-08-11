//! Structural architecture-check orchestrator.
//!
//! Cargo dependency admission is a separate pre-Cargo CI verdict owned by the
//! Python guard. This command never recreates or claims that verdict.
//! Exit codes: 0 = pass, 1 = violations, 2 = tool error.

use std::process::ExitCode;

use crate::cargo_bootstrap;
use crate::generated_tables;
use crate::limitation_registry;
use crate::metadata;
use crate::path_codec;
use crate::path_owner;
use crate::source_policy;

/// Run the repository-owned structural architecture checks.
pub fn run() -> ExitCode {
    let workspace_root = match metadata::find_workspace_root() {
        Ok(root) => root,
        Err(e) => {
            eprintln!("[TOOL ERROR] {e}");
            return ExitCode::from(2);
        }
    };

    println!("=== lumin-xtask architecture-check ===");
    println!("workspace: {}", workspace_root.display());
    println!("scope: structural only; dependency admission is a separate CI guard verdict");
    println!();

    println!("[CHECK] CI routing for the separate dependency-admission guard");
    let bootstrap_result = cargo_bootstrap::check_cargo_bootstrap(&workspace_root);
    if !bootstrap_result.tool_errors.is_empty() {
        for error in bootstrap_result.tool_errors {
            eprintln!("[TOOL ERROR] {error}");
        }
        return ExitCode::from(2);
    }
    if !bootstrap_result.violations.is_empty() {
        for violation in bootstrap_result.violations {
            eprintln!("[VIOLATION] {violation}");
        }
        return ExitCode::from(1);
    }

    println!("[CHECK] static workspace source ownership");
    let workspace = match metadata::inspect_workspace(&workspace_root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[TOOL ERROR] {e}");
            return ExitCode::from(2);
        }
    };

    println!(
        "  members: {} total, {} production",
        workspace.all_members.len(),
        workspace.production_members.len()
    );

    // Repository-owned source and generated-artifact checks.
    println!("[CHECK] source policy: process Command and third-party re-exports");
    println!("[CHECK] source policy: Rayon ThreadPoolBuilder");
    println!("[CHECK] source policy: ScanLock types");
    println!("[CHECK] source policy: global rayon entry points");
    println!("[CHECK] source policy: spec artifact SHA-256 digests");
    println!("[CHECK] path identity declaration and native-lowering owners");
    println!("[CHECK] path/root codec artifact, vectors, DTOs, and generated digest");
    println!("[CHECK] generated configuration tables and owner partitions");
    println!("[CHECK] exhaustive limitation registry and fact owners");

    let effective_root = &workspace.workspace_root;
    let source_result =
        source_policy::scan_production_sources(&workspace.production_members, effective_root);
    let path_owner_result =
        path_owner::scan_path_ownership(&workspace.production_members, effective_root);
    let path_codec_result = path_codec::check_path_codec(effective_root);
    let generated_table_result = generated_tables::check_generated_tables(effective_root);
    let limitation_registry_result = limitation_registry::check_limitation_registry(
        &workspace.production_members,
        effective_root,
    );

    // Collect results
    let mut all_violations = Vec::new();
    let mut all_tool_errors = Vec::new();

    all_violations.extend(workspace.violations);
    all_violations.extend(source_result.violations);
    all_violations.extend(path_owner_result.violations);
    all_violations.extend(path_codec_result.violations);
    all_violations.extend(generated_table_result.violations);
    all_violations.extend(limitation_registry_result.violations);
    all_tool_errors.extend(source_result.tool_errors);
    all_tool_errors.extend(path_owner_result.tool_errors);
    all_tool_errors.extend(path_codec_result.tool_errors);
    all_tool_errors.extend(generated_table_result.tool_errors);
    all_tool_errors.extend(limitation_registry_result.tool_errors);

    // Print results
    println!();
    if !all_violations.is_empty() {
        println!("--- VIOLATIONS ({}) ---", all_violations.len());
        for v in &all_violations {
            println!("  FAIL: {v}");
        }
        println!();
    }

    if !all_tool_errors.is_empty() {
        println!("--- TOOL ERRORS ({}) ---", all_tool_errors.len());
        for e in &all_tool_errors {
            println!("  ERROR: {e}");
        }
        println!();
    }

    // Print deferred items
    println!("--- DEFERRED ---");
    for d in &source_result.deferred {
        println!("  DEFERRED: {d}");
    }
    println!("  DEFERRED: corpus/package/benchmark checks");
    println!();

    // Determine exit code
    if !all_tool_errors.is_empty() {
        println!("RESULT: STRUCTURAL TOOL ERROR (exit 2)");
        ExitCode::from(2)
    } else if !all_violations.is_empty() {
        println!("RESULT: STRUCTURAL VIOLATIONS FOUND (exit 1)");
        ExitCode::from(1)
    } else {
        println!("RESULT: STRUCTURAL PASS (exit 0); dependency admission not evaluated here");
        ExitCode::from(0)
    }
}
