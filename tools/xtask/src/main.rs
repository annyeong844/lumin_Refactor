//! `lumin-xtask` — development-only architecture verification and corpus orchestration tool.
//!
//! Exit codes: 0 = pass, 1 = violations/failures found, 2 = tool/parse/metadata/registry error.

mod architecture;
mod corpus;
mod generated_tables;
mod limitation_registry;
mod metadata;
mod path_owner;
mod source_policy;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("architecture-check") => architecture::run(),
        Some("corpus") => corpus::run(&args[1..]),
        Some("generated-tables") if args.get(1).map(String::as_str) == Some("--write") => {
            let workspace_root = match architecture::find_workspace_root() {
                Ok(root) => root,
                Err(error) => {
                    eprintln!("[TOOL ERROR] {error}");
                    return ExitCode::from(2);
                }
            };
            match generated_tables::write_generated_tables(&workspace_root) {
                Ok(paths) => {
                    for path in paths {
                        println!("generated {}", path.display());
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("[TOOL ERROR] {error}");
                    ExitCode::from(2)
                }
            }
        }
        _ => {
            eprintln!(
                "usage: lumin-xtask <command>\n\n\
                 commands:\n  \
                 architecture-check\n  \
                 generated-tables --write\n  \
                 corpus foundation [--determinism|--store-crash] [--format human|json]"
            );
            ExitCode::from(2)
        }
    }
}
