//! AST-based source policy checks for production crates.
//!
//! Scans production source files using `syn` to enforce:
//! - No process `Command` imports, re-exports, aliases, or constructors
//! - Exactly one `ThreadPoolBuilder::new()` in the engine with correct chain
//! - No `ScanLock` named types
//! - No global Rayon entry points (join/spawn/scope/scope_fifo)
//! - SHA-256 digest verification of spec artifacts

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use syn::visit::Visit;

/// Violations found by source policy scanning.
#[derive(Debug, Default)]
pub struct SourcePolicyResult {
    pub violations: Vec<String>,
    pub tool_errors: Vec<String>,
    pub deferred: Vec<String>,
}

/// Expected SHA-256 hashes of spec artifacts.
const SPEC_DIGESTS: &[(&str, &str)] = &[
    (
        "specs/repo-path-semantics.v1.json",
        "ee686f81164ff40b281483afaae591793964cc576afaca0ce7b5b51a6798b4a6",
    ),
    (
        "specs/inventory-config-semantics.v1.json",
        "ebca37c3b33f8e4d92ea29e0bcdc51b7cd5ea04a453c4c469a89072f3d2fac02",
    ),
    (
        "specs/resolver-config-semantics.v1.json",
        "41ffa3dcc108e74dca351b4f3a5fa182090e1481ed6d8333235f38f0459a29a1",
    ),
];

const ENGINE_SOURCE_PREFIX: &str = "crates/application/engine/src/";

/// Check whether an attribute is provably test-only cfg.
/// Only `cfg(test)` and `cfg(all(test, ...))` are skipped.
/// `cfg(any(...))`, `cfg(not(...))`, and unknown forms are conservatively scanned.
fn is_cfg_test_only(attr: &syn::Attribute) -> bool {
    if !attr.path().is_ident("cfg") {
        return false;
    }
    let Ok(meta) = attr.parse_args::<syn::Meta>() else {
        return false;
    };
    match &meta {
        syn::Meta::Path(path) => path.is_ident("test"),
        syn::Meta::List(list) if list.path.is_ident("all") => {
            // cfg(all(test, ...)) — still test-only
            let mut has_test = false;
            let nested = syn::parse2::<CfgAllArgs>(list.tokens.clone());
            if let Ok(args) = nested {
                for arg in &args.items {
                    if let syn::Meta::Path(p) = arg {
                        has_test = has_test || p.is_ident("test");
                    }
                }
            }
            has_test
        }
        _ => false,
    }
}

/// Helper to parse `all(...)` arguments.
struct CfgAllArgs {
    items: Vec<syn::Meta>,
}

impl syn::parse::Parse for CfgAllArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let items =
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated(input)?;
        Ok(Self {
            items: items.into_iter().collect(),
        })
    }
}

fn segments_to_strings(path: &syn::Path) -> Vec<String> {
    path.segments.iter().map(|s| s.ident.to_string()).collect()
}

/// AST visitor that collects policy violations from a single file.
struct PolicyVisitor {
    file_display: String,
    /// Violations collected.
    violations: Vec<String>,
    /// Process Command imports or constructors found.
    command_found: bool,
    /// ThreadPoolBuilder::new() call sites found.
    pool_builders: Vec<PoolBuilderInfo>,
    /// Immutable, lexically bound builder chains, before their consuming build call.
    pool_receivers: BTreeMap<String, Vec<ChainStep>>,
    /// Global rayon entry points found.
    rayon_globals: Vec<String>,
    /// ScanLock named types found.
    scan_locks: Vec<String>,
    /// WORKER_STACK_BYTES constant definitions found: (value_literal, is_correct).
    worker_stack_consts: Vec<WorkerStackConst>,
}

#[derive(Debug, PartialEq, Eq)]
struct PoolBuilderInfo {
    /// Ordered chain from outermost to innermost (build -> ... -> ThreadPoolBuilder::new).
    chain: Vec<ChainStep>,
    /// Whether the chain root is `ThreadPoolBuilder::new()`.
    rooted: bool,
}

/// A single step in the method chain.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ChainStep {
    Build,
    ThreadName,
    StackSize { arg_is_constant: bool },
    NumThreads,
    Other(String),
}

/// The canonical binding-design order: build -> thread_name -> stack_size(WORKER_STACK_BYTES) -> num_threads -> ThreadPoolBuilder::new()
const CANONICAL_CHAIN: &[ChainStep] = &[
    ChainStep::Build,
    ChainStep::ThreadName,
    ChainStep::StackSize {
        arg_is_constant: true,
    },
    ChainStep::NumThreads,
];

/// A found `const WORKER_STACK_BYTES: usize = <value>` definition.
#[derive(Debug)]
struct WorkerStackConst {
    file_display: String,
    value: Option<u64>,
    correct: bool,
}

impl PolicyVisitor {
    fn new(file_display: String) -> Self {
        Self {
            file_display,
            violations: Vec::new(),
            command_found: false,
            pool_builders: Vec::new(),
            pool_receivers: BTreeMap::new(),
            rayon_globals: Vec::new(),
            scan_locks: Vec::new(),
            worker_stack_consts: Vec::new(),
        }
    }
}

impl<'ast> Visit<'ast> for PolicyVisitor {
    fn visit_block(&mut self, node: &'ast syn::Block) {
        let enclosing = self.pool_receivers.clone();
        syn::visit::visit_block(self, node);
        self.pool_receivers = enclosing;
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        // Visit the initializer before introducing the new binding (shadowing).
        syn::visit::visit_local(self, node);
        if let syn::Pat::Ident(binding) = &node.pat {
            let mut chain = Vec::new();
            let rooted = binding.mutability.is_none()
                && binding.by_ref.is_none()
                && binding.subpat.is_none()
                && node.init.as_ref().is_some_and(|init| {
                    collect_chain_steps(&init.expr, &mut chain, &self.pool_receivers)
                });
            self.pool_receivers.remove(&binding.ident.to_string());
            if rooted {
                self.pool_receivers.insert(binding.ident.to_string(), chain);
            }
        } else {
            // Typed/destructured patterns are not proof that an earlier receiver
            // is still in scope; never trust a potentially shadowed binding.
            self.pool_receivers.clear();
        }
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        // Skip test-only items
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        check_use_tree_for_command(
            &node.tree,
            &mut self.command_found,
            &mut self.violations,
            &self.file_display,
        );
        syn::visit::visit_item_use(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        // Skip cfg(test) modules entirely
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        if node.ident == "WORKER_STACK_BYTES" {
            // Check type is `usize`
            let type_ok = if let syn::Type::Path(tp) = &*node.ty {
                tp.path.is_ident("usize")
            } else {
                false
            };
            let (value, correct) = if type_ok {
                extract_const_int_value(&node.expr)
            } else {
                (None, false)
            };
            self.worker_stack_consts.push(WorkerStackConst {
                file_display: self.file_display.clone(),
                value,
                correct,
            });
        }
        syn::visit::visit_item_const(self, node);
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        if node.ident == "ScanLock" {
            self.scan_locks
                .push(format!("{}: struct ScanLock", self.file_display));
        }
        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        if node.ident == "ScanLock" {
            self.scan_locks
                .push(format!("{}: enum ScanLock", self.file_display));
        }
        syn::visit::visit_item_enum(self, node);
    }

    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        if node.ident == "ScanLock" {
            self.scan_locks
                .push(format!("{}: type ScanLock", self.file_display));
        }
        syn::visit::visit_item_type(self, node);
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        if node.ident == "ScanLock" {
            self.scan_locks
                .push(format!("{}: trait ScanLock", self.file_display));
        }
        syn::visit::visit_item_trait(self, node);
    }

    fn visit_item_union(&mut self, node: &'ast syn::ItemUnion) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        if node.ident == "ScanLock" {
            self.scan_locks
                .push(format!("{}: union ScanLock", self.file_display));
        }
        syn::visit::visit_item_union(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        check_expr_for_rayon_global(node, &self.file_display, &mut self.rayon_globals);
        check_expr_for_command_new(
            node,
            &self.file_display,
            &mut self.violations,
            &mut self.command_found,
        );
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        // Check for ThreadPoolBuilder chain and rayon globals via method calls
        check_method_for_pool_builder(node, &mut self.pool_builders, &self.pool_receivers);
        check_method_for_rayon_global_method(node, &self.file_display, &mut self.rayon_globals);
        syn::visit::visit_expr_method_call(self, node);
    }
}

/// Reject explicit `Command` imports/re-exports from std or third-party crates.
/// A std::process glob is also rejected because it necessarily imports Command.
fn check_use_tree_for_command(
    tree: &syn::UseTree,
    found: &mut bool,
    violations: &mut Vec<String>,
    file_display: &str,
) {
    if explicitly_imports_command(tree) || std_process_glob(tree) {
        *found = true;
        violations.push(format!(
            "{file_display}: imports or re-exports a process Command"
        ));
    }
}

fn explicitly_imports_command(tree: &syn::UseTree) -> bool {
    match tree {
        syn::UseTree::Name(name) => name.ident == "Command",
        syn::UseTree::Rename(rename) => rename.ident == "Command",
        syn::UseTree::Path(path) => {
            path.ident == "Command" || explicitly_imports_command(&path.tree)
        }
        syn::UseTree::Group(group) => group.items.iter().any(explicitly_imports_command),
        syn::UseTree::Glob(_) => false,
    }
}

fn std_process_glob(tree: &syn::UseTree) -> bool {
    std_process_glob_with_prefix(tree, &mut Vec::new())
}

fn std_process_glob_with_prefix(tree: &syn::UseTree, prefix: &mut Vec<String>) -> bool {
    match tree {
        syn::UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            let found = std_process_glob_with_prefix(&path.tree, prefix);
            prefix.pop();
            found
        }
        syn::UseTree::Group(group) => group
            .items
            .iter()
            .any(|item| std_process_glob_with_prefix(item, prefix)),
        syn::UseTree::Glob(_) => prefix.len() == 2 && prefix[0] == "std" && prefix[1] == "process",
        syn::UseTree::Name(_) | syn::UseTree::Rename(_) => false,
    }
}

/// Check if an expression call is a global rayon entry point.
fn check_expr_for_rayon_global(
    node: &syn::ExprCall,
    file_display: &str,
    rayon_globals: &mut Vec<String>,
) {
    if let syn::Expr::Path(expr_path) = &*node.func {
        let segs = segments_to_strings(&expr_path.path);
        // rayon::join, rayon::spawn, rayon::scope, rayon::scope_fifo
        let last = segs.last().map(|s| s.as_str());
        if matches!(last, Some("join" | "spawn" | "scope" | "scope_fifo"))
            && segs.len() >= 2
            && segs[segs.len() - 2] == "rayon"
        {
            rayon_globals.push(format!(
                "{file_display}: global rayon::{}",
                segs.last().unwrap_or(&String::new())
            ));
        }
    }
}

/// Reject `Command::new` through std, aliases exposed under that name, or
/// third-party re-exports. Explicit aliases are rejected at the import site.
fn check_expr_for_command_new(
    node: &syn::ExprCall,
    file_display: &str,
    violations: &mut Vec<String>,
    found: &mut bool,
) {
    if let syn::Expr::Path(expr_path) = &*node.func {
        let segs = segments_to_strings(&expr_path.path);
        if segs.len() >= 2 && segs[segs.len() - 2] == "Command" && segs[segs.len() - 1] == "new" {
            *found = true;
            violations.push(format!(
                "{file_display}: constructs a process Command (directly or through a re-export)"
            ));
        }
    }
}

/// Check method calls for rayon global methods used as free-standing.
fn check_method_for_rayon_global_method(
    _node: &syn::ExprMethodCall,
    _file_display: &str,
    _rayon_globals: &mut Vec<String>,
) {
    // Rayon globals (join/spawn/scope/scope_fifo) are function calls, not method calls.
    // This is a no-op but kept for completeness.
}

/// Check for ThreadPoolBuilder pattern in method call chains.
/// Collects from ALL production source (no is_engine_lib gate).
/// Only triggers on chains rooted at `ThreadPoolBuilder::new()`.
fn check_method_for_pool_builder(
    node: &syn::ExprMethodCall,
    pool_builders: &mut Vec<PoolBuilderInfo>,
    receivers: &BTreeMap<String, Vec<ChainStep>>,
) {
    // We look for .build() as the outermost call of the chain
    if node.method != "build" {
        return;
    }
    let mut chain = vec![ChainStep::Build];
    let rooted = collect_chain_steps(&node.receiver, &mut chain, receivers);
    if rooted {
        pool_builders.push(PoolBuilderInfo { chain, rooted });
    }
}

/// Walk the receiver chain from outside in, collecting steps.
/// Returns `true` if the root is `ThreadPoolBuilder::new()`.
fn collect_chain_steps(
    expr: &syn::Expr,
    chain: &mut Vec<ChainStep>,
    receivers: &BTreeMap<String, Vec<ChainStep>>,
) -> bool {
    match expr {
        syn::Expr::MethodCall(method) => {
            let step = match method.method.to_string().as_str() {
                "thread_name" => ChainStep::ThreadName,
                "stack_size" => {
                    let arg_is_constant = method
                        .args
                        .first()
                        .map(is_stack_size_path_constant)
                        .unwrap_or(false);
                    ChainStep::StackSize { arg_is_constant }
                }
                "num_threads" => ChainStep::NumThreads,
                other => ChainStep::Other(other.to_owned()),
            };
            chain.push(step);
            collect_chain_steps(&method.receiver, chain, receivers)
        }
        syn::Expr::Call(call) => {
            if let syn::Expr::Path(path) = &*call.func {
                let segs = segments_to_strings(&path.path);
                if segs.len() >= 2
                    && segs[segs.len() - 2] == "ThreadPoolBuilder"
                    && segs[segs.len() - 1] == "new"
                {
                    return true;
                }
            }
            false
        }
        syn::Expr::Path(path) if path.path.segments.len() == 1 => {
            if let Some(bound) = receivers.get(&path.path.segments[0].ident.to_string()) {
                chain.extend_from_slice(bound);
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Check that the stack_size argument is the path `WORKER_STACK_BYTES` (not a literal).
fn is_stack_size_path_constant(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Path(path) => {
            let segs = segments_to_strings(&path.path);
            segs.last()
                .map(|s| s == "WORKER_STACK_BYTES")
                .unwrap_or(false)
        }
        // Direct literal is rejected by binding design
        _ => false,
    }
}

/// Extract the integer literal value from a const initializer expression.
/// Returns (Some(value), correct) where correct means value == 4_194_304.
fn extract_const_int_value(expr: &syn::Expr) -> (Option<u64>, bool) {
    if let syn::Expr::Lit(lit) = expr
        && let syn::Lit::Int(int_lit) = &lit.lit
        && let Ok(val) = int_lit.base10_parse::<u64>()
    {
        return (Some(val), val == 4_194_304);
    }
    (None, false)
}

/// Scan all production source files for policy violations.
pub fn scan_production_sources(
    members: &[crate::metadata::WorkspaceMember],
    workspace_root: &Path,
) -> SourcePolicyResult {
    let mut result = SourcePolicyResult::default();
    let mut all_pool_builders: Vec<(String, PoolBuilderInfo)> = Vec::new();
    let mut all_worker_stack_consts: Vec<WorkerStackConst> = Vec::new();

    for member in members {
        let src_root = &member.src_root;
        if !src_root.exists() {
            result.tool_errors.push(format!(
                "src root missing for {}: {}",
                member.name,
                crate::metadata::relative_display(workspace_root, src_root)
            ));
            continue;
        }

        let files = match collect_rs_files(src_root) {
            Ok(f) => f,
            Err(e) => {
                result.tool_errors.push(format!(
                    "failed to read src root for {} ({}): {e}",
                    member.name,
                    crate::metadata::relative_display(workspace_root, src_root)
                ));
                continue;
            }
        };

        for file_path in &files {
            let file_display = crate::metadata::relative_display(workspace_root, file_path);

            let source = match std::fs::read_to_string(file_path) {
                Ok(s) => s,
                Err(e) => {
                    result
                        .tool_errors
                        .push(format!("cannot read {file_display}: {e}"));
                    continue;
                }
            };

            let syntax = match syn::parse_file(&source) {
                Ok(f) => f,
                Err(e) => {
                    result
                        .tool_errors
                        .push(format!("syn parse error in {file_display}: {e}"));
                    continue;
                }
            };

            let mut visitor = PolicyVisitor::new(file_display.clone());
            visitor.visit_file(&syntax);

            result.violations.extend(visitor.violations);
            for scan_lock in visitor.scan_locks {
                result
                    .violations
                    .push(format!("HARD-STOP: ScanLock type defined: {scan_lock}"));
            }
            for rayon_global in visitor.rayon_globals {
                result.violations.push(format!("FORBIDDEN: {rayon_global}"));
            }
            for builder in visitor.pool_builders {
                all_pool_builders.push((file_display.clone(), builder));
            }
            all_worker_stack_consts.extend(visitor.worker_stack_consts);
        }
    }

    // Validate Rayon ThreadPoolBuilder policy
    validate_pool_builders(&all_pool_builders, &mut result);

    // Validate WORKER_STACK_BYTES constant
    validate_worker_stack_const(&all_worker_stack_consts, &mut result);

    // Verify spec artifact digests
    verify_spec_digests(workspace_root, &mut result);

    result
}

fn validate_pool_builders(builders: &[(String, PoolBuilderInfo)], result: &mut SourcePolicyResult) {
    let engine_builders: Vec<&(String, PoolBuilderInfo)> = builders
        .iter()
        .filter(|(path, _)| is_engine_source(path))
        .collect();

    let non_engine_builders: Vec<&(String, PoolBuilderInfo)> = builders
        .iter()
        .filter(|(path, _)| !is_engine_source(path))
        .collect();

    // Reject builders outside engine
    for (path, _) in &non_engine_builders {
        result.violations.push(format!(
            "FORBIDDEN: ThreadPoolBuilder outside engine: {path}"
        ));
    }

    // Exactly one in engine with canonical chain order
    match engine_builders.len() {
        0 => {
            result
                .violations
                .push("MISSING: no ThreadPoolBuilder::new() in engine source tree".to_owned());
        }
        1 => {
            let (_, info) = engine_builders[0];
            if info.chain != CANONICAL_CHAIN {
                result.violations.push(format!(
                    "engine ThreadPoolBuilder chain order violation: expected \
                     [build, thread_name, stack_size(WORKER_STACK_BYTES), num_threads] \
                     got {:?}",
                    info.chain
                ));
            }
        }
        n => {
            result.violations.push(format!(
                "FORBIDDEN: {n} ThreadPoolBuilder::new() in engine (expected exactly 1)"
            ));
        }
    }
}

fn is_engine_source(path: &str) -> bool {
    path.replace('\\', "/").contains(ENGINE_SOURCE_PREFIX)
}

fn validate_worker_stack_const(consts: &[WorkerStackConst], result: &mut SourcePolicyResult) {
    match consts.len() {
        0 => {
            result.violations.push(
                "MISSING: const WORKER_STACK_BYTES: usize not found in any production source"
                    .to_owned(),
            );
        }
        1 => {
            let c = &consts[0];
            if !c.correct {
                let val_str = c
                    .value
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "non-integer".to_owned());
                result.violations.push(format!(
                    "WORKER_STACK_BYTES in {} must be exactly 4_194_304, got {val_str}",
                    c.file_display
                ));
            }
        }
        n => {
            let locations: Vec<&str> = consts.iter().map(|c| c.file_display.as_str()).collect();
            result.violations.push(format!(
                "FORBIDDEN: {n} WORKER_STACK_BYTES definitions (expected exactly 1): {locations:?}"
            ));
        }
    }
}

fn verify_spec_digests(workspace_root: &Path, result: &mut SourcePolicyResult) {
    for (rel_path, expected_hex) in SPEC_DIGESTS {
        let full_path = workspace_root.join(rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let content = match std::fs::read(&full_path) {
            Ok(c) => c,
            Err(e) => {
                result
                    .tool_errors
                    .push(format!("cannot read spec artifact {rel_path}: {e}"));
                continue;
            }
        };
        let mut hasher = Sha256::new();
        hasher.update(&content);
        let actual = format!("{:x}", hasher.finalize());
        if actual != *expected_hex {
            result.violations.push(format!(
                "SPEC DIGEST MISMATCH: {rel_path} expected {expected_hex} got {actual}"
            ));
        }
    }
}

/// Recursively collect `.rs` files, excluding `tests/` path segments and `tests.rs`.
fn collect_rs_files(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    collect_rs_files_recursive(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_rs_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if path.is_dir() {
            // Exclude `tests` directory segments
            if name == "tests" {
                continue;
            }
            collect_rs_files_recursive(&path, files)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            // Exclude tests.rs files
            if name == "tests.rs" {
                continue;
            }
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn parse_and_visit(source: &str) -> PolicyVisitor {
        let syntax = syn::parse_file(source).expect("test source must parse");
        let mut visitor = PolicyVisitor::new("test.rs".to_owned());
        visitor.visit_file(&syntax);
        visitor
    }

    fn parse_and_visit_engine(source: &str) -> PolicyVisitor {
        let syntax = syn::parse_file(source).expect("test source must parse");
        let mut visitor = PolicyVisitor::new("engine/lib.rs".to_owned());
        visitor.visit_file(&syntax);
        visitor
    }

    #[test]
    fn command_import_detected() {
        let v = parse_and_visit("use std::process::Command;");
        assert!(v.command_found);
        assert!(!v.violations.is_empty());
    }

    #[test]
    fn command_alias_detected() {
        let v = parse_and_visit("use std::process::Command as Cmd;");
        assert!(v.command_found);
        assert!(!v.violations.is_empty());
    }

    #[test]
    fn command_in_group_import() {
        let v = parse_and_visit("use std::process::{Command, ExitCode};");
        assert!(v.command_found);
    }

    #[test]
    fn command_glob_import() {
        let v = parse_and_visit("use std::process::*;");
        assert!(v.command_found);
    }

    #[test]
    fn command_grouped_glob_import() {
        let v = parse_and_visit("use std::{path::Path, process::*};");
        assert!(v.command_found);
    }

    #[test]
    fn third_party_command_reexport_import_is_detected() {
        let v = parse_and_visit("use process_wrapper::Command as Wrapped;");
        assert!(v.command_found);
        assert!(!v.violations.is_empty());
    }

    #[test]
    fn third_party_command_reexport_constructor_is_detected() {
        let v = parse_and_visit(
            "fn f() { let _c = process_wrapper::process::Command::new(\"tool\"); }",
        );
        assert!(v.command_found);
        assert!(!v.violations.is_empty());
    }

    #[test]
    fn command_new_fully_qualified() {
        let v = parse_and_visit("fn f() { let _c = std::process::Command::new(\"ls\"); }");
        assert!(v.command_found);
    }

    #[test]
    fn command_new_after_import() {
        let v = parse_and_visit(
            "use std::process::Command;\nfn f() { let _c = Command::new(\"ls\"); }",
        );
        assert!(v.command_found);
        // Two violations: import + call
        assert!(v.violations.len() >= 2);
    }

    #[test]
    fn no_false_positive_on_string_containing_command() {
        let v = parse_and_visit(r#"fn f() { let _s = "std::process::Command::new"; }"#);
        assert!(!v.command_found);
        assert!(v.violations.is_empty());
    }

    #[test]
    fn no_false_positive_on_comment_containing_command() {
        let v = parse_and_visit("// use std::process::Command;\nfn f() {}");
        assert!(!v.command_found);
        assert!(v.violations.is_empty());
    }

    #[test]
    fn command_in_cfg_test_skipped() {
        let v = parse_and_visit("#[cfg(test)]\nmod tests { use std::process::Command; }");
        assert!(!v.command_found);
    }

    #[test]
    fn command_in_cfg_all_test_skipped() {
        let v = parse_and_visit(
            "#[cfg(all(test, feature = \"x\"))]\nmod tests { use std::process::Command; }",
        );
        assert!(!v.command_found);
    }

    #[test]
    fn command_in_cfg_any_not_skipped() {
        let v = parse_and_visit(
            "#[cfg(any(test, feature = \"x\"))]\nmod m { use std::process::Command; }",
        );
        // cfg(any(...)) is conservatively scanned
        assert!(v.command_found);
    }

    #[test]
    fn scan_lock_struct_detected() {
        let v = parse_and_visit("struct ScanLock { field: u32 }");
        assert_eq!(v.scan_locks.len(), 1);
    }

    #[test]
    fn scan_lock_enum_detected() {
        let v = parse_and_visit("enum ScanLock { A, B }");
        assert_eq!(v.scan_locks.len(), 1);
    }

    #[test]
    fn scan_lock_type_alias_detected() {
        let v = parse_and_visit("type ScanLock = std::sync::Mutex<()>;");
        assert_eq!(v.scan_locks.len(), 1);
    }

    #[test]
    fn scan_lock_trait_detected() {
        let v = parse_and_visit("trait ScanLock {}");
        assert_eq!(v.scan_locks.len(), 1);
    }

    #[test]
    fn scan_lock_union_detected() {
        let v = parse_and_visit("union ScanLock { a: u32, b: f32 }");
        assert_eq!(v.scan_locks.len(), 1);
    }

    #[test]
    fn scan_lock_in_cfg_test_not_flagged() {
        let v = parse_and_visit("#[cfg(test)]\nmod tests { struct ScanLock {} }");
        assert!(v.scan_locks.is_empty());
    }

    #[test]
    fn rayon_global_join_detected() {
        let v = parse_and_visit("fn f() { rayon::join(|| {}, || {}); }");
        assert_eq!(v.rayon_globals.len(), 1);
    }

    #[test]
    fn rayon_global_spawn_detected() {
        let v = parse_and_visit("fn f() { rayon::spawn(|| {}); }");
        assert_eq!(v.rayon_globals.len(), 1);
    }

    #[test]
    fn rayon_global_scope_detected() {
        let v = parse_and_visit("fn f() { rayon::scope(|_s| {}); }");
        assert_eq!(v.rayon_globals.len(), 1);
    }

    #[test]
    fn rayon_global_scope_fifo_detected() {
        let v = parse_and_visit("fn f() { rayon::scope_fifo(|_s| {}); }");
        assert_eq!(v.rayon_globals.len(), 1);
    }

    #[test]
    fn pool_builder_valid_chain_in_engine() {
        let source = r#"
            const WORKER_STACK_BYTES: usize = 4_194_304;
            fn f() {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(4)
                    .stack_size(WORKER_STACK_BYTES)
                    .thread_name(|i| format!("w-{i}"))
                    .build()
                    .unwrap();
            }
        "#;
        let v = parse_and_visit_engine(source);
        assert_eq!(v.pool_builders.len(), 1);
        assert_eq!(v.pool_builders[0].chain, CANONICAL_CHAIN);
        assert!(v.pool_builders[0].rooted);
    }

    #[test]
    fn immutable_pool_receiver_preserves_the_exact_builder_chain() {
        let source = r#"
            fn f() {
                let builder = rayon::ThreadPoolBuilder::new()
                    .num_threads(4).stack_size(WORKER_STACK_BYTES)
                    .thread_name(|i| format!("w-{i}"));
                observe_before_build();
                let pool = builder.build();
            }
        "#;
        let observed = parse_and_visit_engine(source);
        assert_eq!(observed.pool_builders.len(), 1);
        assert_eq!(observed.pool_builders[0].chain, CANONICAL_CHAIN);
        for changed in [
            source.replace("WORKER_STACK_BYTES", "1024"),
            source.replace("builder.build()", "builder.use_current_thread().build()"),
        ] {
            let observed = parse_and_visit_engine(&changed);
            assert_eq!(observed.pool_builders.len(), 1);
            assert_ne!(observed.pool_builders[0].chain, CANONICAL_CHAIN);
        }
    }

    #[test]
    fn shadowed_or_mutable_pool_receivers_are_not_authenticated() {
        for replacement in [
            "let builder = foreign();",
            "let mut builder = foreign();",
            "let builder: Other = foreign();",
            "let (builder, other) = foreign();",
        ] {
            let source = format!(
                r#"
                fn f() {{
                    let builder = rayon::ThreadPoolBuilder::new()
                        .num_threads(4).stack_size(WORKER_STACK_BYTES)
                        .thread_name(|i| format!("w-{{i}}"));
                    {replacement}
                    builder.build();
                }}
            "#
            );
            assert!(parse_and_visit_engine(&source).pool_builders.is_empty());
        }
        let observed = parse_and_visit_engine(
            r#"
            fn f() {
                { let builder = rayon::ThreadPoolBuilder::new().num_threads(4); }
                builder.build();
            }
        "#,
        );
        assert!(observed.pool_builders.is_empty());
    }

    #[test]
    fn pool_builder_wrong_stack_size() {
        let source = r#"
            fn f() {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(4)
                    .stack_size(1024)
                    .thread_name(|i| format!("w-{i}"))
                    .build()
                    .unwrap();
            }
        "#;
        let v = parse_and_visit_engine(source);
        assert_eq!(v.pool_builders.len(), 1);
        // Direct literal → arg_is_constant = false
        let expected = vec![
            ChainStep::Build,
            ChainStep::ThreadName,
            ChainStep::StackSize {
                arg_is_constant: false,
            },
            ChainStep::NumThreads,
        ];
        assert_eq!(v.pool_builders[0].chain, expected);
    }

    #[test]
    fn pool_builder_literal_4194304_rejected() {
        // Binding design: direct literal is NOT accepted, must use WORKER_STACK_BYTES path
        let source = r#"
            fn f() {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(4)
                    .stack_size(4_194_304)
                    .thread_name(|i| format!("w-{i}"))
                    .build()
                    .unwrap();
            }
        "#;
        let v = parse_and_visit_engine(source);
        assert_eq!(v.pool_builders.len(), 1);
        // Direct literal → arg_is_constant = false, chain mismatch
        assert_ne!(v.pool_builders[0].chain, CANONICAL_CHAIN);
    }

    #[test]
    fn pool_builder_missing_thread_name() {
        let source = r#"
            fn f() {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(4)
                    .stack_size(WORKER_STACK_BYTES)
                    .build()
                    .unwrap();
            }
        "#;
        let v = parse_and_visit_engine(source);
        assert_eq!(v.pool_builders.len(), 1);
        // Missing thread_name → chain != canonical
        assert_ne!(v.pool_builders[0].chain, CANONICAL_CHAIN);
    }

    #[test]
    fn pool_builder_duplicate_in_engine() {
        let source = r#"
            const WORKER_STACK_BYTES: usize = 4_194_304;
            fn f() {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(4)
                    .stack_size(WORKER_STACK_BYTES)
                    .thread_name(|i| format!("w-{i}"))
                    .build()
                    .unwrap();
            }
            fn g() {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(2)
                    .stack_size(WORKER_STACK_BYTES)
                    .thread_name(|i| format!("w-{i}"))
                    .build()
                    .unwrap();
            }
        "#;
        let v = parse_and_visit_engine(source);
        assert_eq!(v.pool_builders.len(), 2);
    }

    #[test]
    fn pool_builder_outside_engine_detected() {
        // Non-engine file with ThreadPoolBuilder → collected (will be rejected by validate)
        let source = r#"
            fn f() {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(2)
                    .stack_size(WORKER_STACK_BYTES)
                    .thread_name(|i| format!("w-{i}"))
                    .build()
                    .unwrap();
            }
        "#;
        let v = parse_and_visit(source);
        assert_eq!(v.pool_builders.len(), 1);
        assert!(v.pool_builders[0].rooted);
    }

    #[test]
    fn pool_builder_owner_is_the_engine_source_tree_not_one_file() {
        let source = r#"
            fn f() {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(2)
                    .stack_size(WORKER_STACK_BYTES)
                    .thread_name(|i| format!("w-{i}"))
                    .build()
                    .unwrap();
            }
        "#;
        let mut visitor = parse_and_visit(source);
        let builder = visitor
            .pool_builders
            .pop()
            .expect("canonical builder must be observed");
        let builders = vec![(
            "crates/application/engine/src/extraction.rs".to_owned(),
            builder,
        )];
        let mut result = SourcePolicyResult::default();
        validate_pool_builders(&builders, &mut result);
        assert!(result.violations.is_empty(), "{:?}", result.violations);

        let mut visitor = parse_and_visit(source);
        let builder = visitor
            .pool_builders
            .pop()
            .expect("canonical builder must be observed");
        let builders = vec![("crates/languages/js/src/extraction.rs".to_owned(), builder)];
        let mut result = SourcePolicyResult::default();
        validate_pool_builders(&builders, &mut result);
        assert!(
            result
                .violations
                .iter()
                .any(|violation| violation.contains("outside engine"))
        );
    }

    #[test]
    fn unrelated_builder_no_false_positive() {
        // A .build() call that is NOT rooted at ThreadPoolBuilder::new()
        let source = r#"
            fn f() {
                SomeOtherBuilder::new()
                    .num_threads(4)
                    .stack_size(1024)
                    .thread_name("foo")
                    .build();
            }
        "#;
        let v = parse_and_visit(source);
        assert!(
            v.pool_builders.is_empty(),
            "should not detect unrelated builder"
        );
    }

    #[test]
    fn pool_builder_reordered_chain_fails() {
        // stack_size and thread_name swapped vs canonical
        let source = r#"
            fn f() {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(4)
                    .thread_name(|i| format!("w-{i}"))
                    .stack_size(WORKER_STACK_BYTES)
                    .build()
                    .unwrap();
            }
        "#;
        let v = parse_and_visit_engine(source);
        assert_eq!(v.pool_builders.len(), 1);
        assert_ne!(v.pool_builders[0].chain, CANONICAL_CHAIN);
    }

    #[test]
    fn pool_builder_extra_method_fails() {
        // Additional method in chain
        let source = r#"
            fn f() {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(4)
                    .stack_size(WORKER_STACK_BYTES)
                    .thread_name(|i| format!("w-{i}"))
                    .panic_handler(|_| {})
                    .build()
                    .unwrap();
            }
        "#;
        let v = parse_and_visit_engine(source);
        assert_eq!(v.pool_builders.len(), 1);
        assert_ne!(v.pool_builders[0].chain, CANONICAL_CHAIN);
    }

    #[test]
    fn digest_tamper_detected() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let spec_dir = dir.path().join("specs");
        std::fs::create_dir_all(&spec_dir).expect("create specs dir");
        std::fs::write(spec_dir.join("repo-path-semantics.v1.json"), b"tampered")
            .expect("write test file");
        std::fs::write(
            spec_dir.join("inventory-config-semantics.v1.json"),
            b"tampered",
        )
        .expect("write test file");
        std::fs::write(
            spec_dir.join("resolver-config-semantics.v1.json"),
            b"tampered",
        )
        .expect("write test file");

        let mut result = SourcePolicyResult::default();
        verify_spec_digests(dir.path(), &mut result);
        assert_eq!(result.violations.len(), 3);
        for v in &result.violations {
            assert!(v.contains("SPEC DIGEST MISMATCH"));
        }
    }

    #[test]
    fn cfg_test_only_detection() {
        let attr: syn::Attribute = syn::parse_quote!(#[cfg(test)]);
        assert!(is_cfg_test_only(&attr));

        let attr: syn::Attribute = syn::parse_quote!(#[cfg(all(test, feature = "x"))]);
        assert!(is_cfg_test_only(&attr));

        let attr: syn::Attribute = syn::parse_quote!(#[cfg(any(test, feature = "x"))]);
        assert!(!is_cfg_test_only(&attr));

        let attr: syn::Attribute = syn::parse_quote!(#[cfg(not(test))]);
        assert!(!is_cfg_test_only(&attr));

        let attr: syn::Attribute = syn::parse_quote!(#[cfg(feature = "x")]);
        assert!(!is_cfg_test_only(&attr));
    }

    #[test]
    fn worker_stack_const_correct() {
        let source = "const WORKER_STACK_BYTES: usize = 4_194_304;";
        let v = parse_and_visit_engine(source);
        assert_eq!(v.worker_stack_consts.len(), 1);
        assert!(v.worker_stack_consts[0].correct);
        assert_eq!(v.worker_stack_consts[0].value, Some(4_194_304));
    }

    #[test]
    fn worker_stack_const_wrong_value() {
        let source = "const WORKER_STACK_BYTES: usize = 2_097_152;";
        let v = parse_and_visit_engine(source);
        assert_eq!(v.worker_stack_consts.len(), 1);
        assert!(!v.worker_stack_consts[0].correct);
        assert_eq!(v.worker_stack_consts[0].value, Some(2_097_152));
    }

    #[test]
    fn worker_stack_const_duplicate_detected() {
        let source = r#"
            const WORKER_STACK_BYTES: usize = 4_194_304;
            mod inner { const WORKER_STACK_BYTES: usize = 4_194_304; }
        "#;
        // Both are at top-level from syn visit perspective
        let syntax = syn::parse_file(source).expect("parse");
        let mut visitor = PolicyVisitor::new("test.rs".to_owned());
        visitor.visit_file(&syntax);
        // The inner mod const will also be visited
        assert!(visitor.worker_stack_consts.len() >= 2);
    }

    #[test]
    fn worker_stack_const_missing_validation() {
        let consts: Vec<WorkerStackConst> = vec![];
        let mut result = SourcePolicyResult::default();
        validate_worker_stack_const(&consts, &mut result);
        assert_eq!(result.violations.len(), 1);
        assert!(result.violations[0].contains("MISSING"));
    }

    #[test]
    fn worker_stack_const_wrong_value_validation() {
        let consts = vec![WorkerStackConst {
            file_display: "test.rs".to_owned(),
            value: Some(1024),
            correct: false,
        }];
        let mut result = SourcePolicyResult::default();
        validate_worker_stack_const(&consts, &mut result);
        assert_eq!(result.violations.len(), 1);
        assert!(result.violations[0].contains("must be exactly 4_194_304"));
    }

    #[test]
    fn worker_stack_const_in_cfg_test_skipped() {
        let source = "#[cfg(test)]\nmod tests { const WORKER_STACK_BYTES: usize = 4_194_304; }";
        let v = parse_and_visit_engine(source);
        assert!(v.worker_stack_consts.is_empty());
    }
}
