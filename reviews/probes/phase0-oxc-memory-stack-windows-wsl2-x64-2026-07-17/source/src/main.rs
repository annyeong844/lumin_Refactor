mod corpus;
mod memory;
mod model;
mod probe;
mod util;

use std::{collections::BTreeMap, env, path::PathBuf};

use anyhow::{Context, Result, bail};

use crate::{
    model::IdentityReport,
    probe::{RunOptions, run},
    util::{executable_identity, source_hashes, source_manifest_sha256, write_json},
};

pub const ARCHITECTURE_COMMIT: &str = "65e60216891bb3d826a4778f84cb8aaa377abe92";
pub const ARCHITECTURE_MANIFEST: &str =
    "66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0";

fn main() {
    if let Err(error) = execute() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn execute() -> Result<()> {
    let mut arguments = env::args().skip(1);
    let command = arguments
        .next()
        .context("missing command: identity or run")?;
    let flags = parse_flags(arguments.collect())?;
    match command.as_str() {
        "identity" => identity(&flags),
        "run" => run_command(&flags),
        _ => bail!("unsupported command {command:?}"),
    }
}

fn identity(flags: &BTreeMap<String, String>) -> Result<()> {
    ensure_only(flags, &["output"])?;
    let source_files = source_hashes();
    let report = IdentityReport {
        probe_id: "lumin-phase0-oxc-identity-v1",
        architecture_commit: ARCHITECTURE_COMMIT,
        architecture_manifest_sha256: ARCHITECTURE_MANIFEST,
        host_os: env::consts::OS,
        host_arch: env::consts::ARCH,
        available_parallelism: std::thread::available_parallelism()?.get(),
        executable: executable_identity()?,
        source_manifest_sha256: source_manifest_sha256(&source_files),
        source_files,
    };
    write_json(&required_path(flags, "output")?, &report)
}

fn run_command(flags: &BTreeMap<String, String>) -> Result<()> {
    ensure_only(
        flags,
        &[
            "corpus-root",
            "manifest",
            "workers",
            "stack-bytes",
            "waves",
            "platform",
            "filesystem-class",
            "output",
        ],
    )?;
    let corpus_root = required_path(flags, "corpus-root")?;
    let manifest = required_path(flags, "manifest")?;
    let corpus = corpus::load_and_validate(&corpus_root, &manifest)?;
    let report = run(RunOptions {
        corpus_root,
        corpus,
        workers: required_usize(flags, "workers")?,
        stack_bytes: required_usize(flags, "stack-bytes")?,
        waves: required_usize(flags, "waves")?,
        platform_label: required(flags, "platform")?.to_owned(),
        filesystem_class: required(flags, "filesystem-class")?.to_owned(),
    })?;
    write_json(&required_path(flags, "output")?, &report)
}

fn parse_flags(arguments: Vec<String>) -> Result<BTreeMap<String, String>> {
    if !arguments.len().is_multiple_of(2) {
        bail!("flags must be supplied as --name value pairs");
    }
    let mut flags = BTreeMap::new();
    for pair in arguments.chunks_exact(2) {
        let name = pair[0]
            .strip_prefix("--")
            .with_context(|| format!("expected flag, got {:?}", pair[0]))?;
        if flags.insert(name.to_owned(), pair[1].clone()).is_some() {
            bail!("duplicate flag --{name}");
        }
    }
    Ok(flags)
}

fn ensure_only(flags: &BTreeMap<String, String>, allowed: &[&str]) -> Result<()> {
    for name in flags.keys() {
        if !allowed.contains(&name.as_str()) {
            bail!("unsupported flag --{name}");
        }
    }
    Ok(())
}

fn required<'a>(flags: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str> {
    flags
        .get(name)
        .map(String::as_str)
        .with_context(|| format!("missing --{name}"))
}

fn required_path(flags: &BTreeMap<String, String>, name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(required(flags, name)?))
}

fn required_usize(flags: &BTreeMap<String, String>, name: &str) -> Result<usize> {
    required(flags, name)?
        .parse()
        .with_context(|| format!("invalid --{name}"))
}
