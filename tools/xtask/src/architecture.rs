//! Architecture-check orchestrator.
//!
//! Combines workspace metadata validation and AST source-policy scanning.
//! Exit codes: 0 = pass, 1 = violations, 2 = tool error.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::generated_tables;
use crate::limitation_registry;
use crate::metadata;
use crate::path_codec;
use crate::path_owner;
use crate::source_policy;

/// Run the full architecture check.
pub fn run() -> ExitCode {
    let workspace_root = match find_workspace_root() {
        Ok(root) => root,
        Err(e) => {
            eprintln!("[TOOL ERROR] {e}");
            return ExitCode::from(2);
        }
    };

    println!("=== lumin-xtask architecture-check ===");
    println!("workspace: {}", workspace_root.display());
    println!();

    // Phase 1: Metadata / dependency edge analysis
    println!("[CHECK] cargo metadata dependency edges");
    let metadata_result = match metadata::analyze_workspace(&workspace_root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[TOOL ERROR] {e}");
            return ExitCode::from(2);
        }
    };

    println!(
        "  members: {} total, {} production",
        metadata_result.all_members.len(),
        metadata_result.production_members.len()
    );

    // Phase 2: AST source policy scanning
    println!("[CHECK] source policy: process Command and third-party re-exports");
    println!("[CHECK] source policy: Rayon ThreadPoolBuilder");
    println!("[CHECK] source policy: ScanLock types");
    println!("[CHECK] source policy: global rayon entry points");
    println!("[CHECK] source policy: spec artifact SHA-256 digests");
    println!("[CHECK] path identity declaration and native-lowering owners");
    println!("[CHECK] path/root codec artifact, vectors, DTOs, and generated digest");
    println!("[CHECK] generated configuration tables and owner partitions");
    println!("[CHECK] exhaustive limitation registry and fact owners");

    let effective_root = &metadata_result.workspace_root;
    let source_result =
        source_policy::scan_production_sources(&metadata_result.production_members, effective_root);
    let path_owner_result =
        path_owner::scan_path_ownership(&metadata_result.production_members, effective_root);
    let path_codec_result = path_codec::check_path_codec(effective_root);
    let generated_table_result = generated_tables::check_generated_tables(effective_root);
    let limitation_registry_result = limitation_registry::check_limitation_registry(
        &metadata_result.production_members,
        effective_root,
    );

    // Collect results
    let mut all_violations = Vec::new();
    let mut all_tool_errors = Vec::new();

    all_violations.extend(metadata_result.violations);
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
        println!("RESULT: TOOL ERROR (exit 2)");
        ExitCode::from(2)
    } else if !all_violations.is_empty() {
        println!("RESULT: VIOLATIONS FOUND (exit 1)");
        ExitCode::from(1)
    } else {
        println!("RESULT: PASS (exit 0)");
        ExitCode::from(0)
    }
}

/// Find the workspace root by looking for the root Cargo.toml with [workspace].
pub(crate) fn find_workspace_root() -> Result<PathBuf, String> {
    // Start from CARGO_MANIFEST_DIR or current directory
    let start = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());

    let mut dir = start.as_path();
    loop {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            // Check if this is a workspace root
            let content = std::fs::read_to_string(&cargo_toml)
                .map_err(|e| format!("cannot read {}: {e}", cargo_toml.display()))?;
            if content.contains("[workspace]") {
                return Ok(dir.to_path_buf());
            }
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    Err("could not find workspace root (no Cargo.toml with [workspace] found)".to_owned())
}
