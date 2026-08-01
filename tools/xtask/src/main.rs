//! `lumin-xtask` — development-only architecture verification and corpus orchestration tool.
//!
//! Exit codes: 0 = pass, 1 = violations/failures found, 2 = tool/parse/metadata/registry error.

mod architecture;
mod corpus;
mod metadata;
mod source_policy;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("architecture-check") => architecture::run(),
        Some("corpus") => corpus::run(&args[1..]),
        _ => {
            eprintln!(
                "usage: lumin-xtask <command>\n\n\
                 commands:\n  \
                 architecture-check\n  \
                 corpus foundation [--determinism|--store-crash] [--format human|json]"
            );
            ExitCode::from(2)
        }
    }
}
