use std::fs;

use anyhow::{Context, Result, ensure};
use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;
use rayon::prelude::*;
use redb::{Database, ReadableDatabase, TableDefinition};
use serde::Serialize;

const PROBE_TABLE: TableDefinition<&str, u64> = TableDefinition::new("probe");
const TYPESCRIPT_FIXTURE: &str = "const answer: number = 42; export { answer };";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeResult {
    schema: &'static str,
    status: &'static str,
    os: &'static str,
    arch: &'static str,
    target_env: &'static str,
    oxc_statement_count: usize,
    rayon_sum: u64,
    redb_value: u64,
}

fn target_env() -> &'static str {
    if cfg!(target_env = "msvc") {
        "msvc"
    } else if cfg!(target_env = "musl") {
        "musl"
    } else if cfg!(target_env = "gnu") {
        "gnu"
    } else {
        "unknown"
    }
}

fn probe_oxc() -> Result<usize> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, TYPESCRIPT_FIXTURE, SourceType::ts()).parse();
    ensure!(
        !parsed.panicked,
        "OXC parser panicked on the constant fixture"
    );
    ensure!(
        parsed.errors.is_empty(),
        "OXC parser returned errors on the constant fixture"
    );
    Ok(parsed.program.body.len())
}

fn probe_rayon() -> Result<u64> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .context("build the local Rayon pool")?;
    Ok(pool.install(|| (0_u64..100).into_par_iter().sum()))
}

fn probe_redb() -> Result<u64> {
    let path = std::env::temp_dir().join(format!(
        "lumin-phase0-static-packaging-{}.redb",
        std::process::id()
    ));
    let _ = fs::remove_file(&path);

    let result = (|| -> Result<u64> {
        let database = Database::create(&path).context("create temporary redb database")?;
        let write = database.begin_write().context("begin redb write")?;
        {
            let mut table = write.open_table(PROBE_TABLE).context("open redb table")?;
            table.insert("answer", 42).context("insert redb value")?;
        }
        write.commit().context("commit redb value")?;

        let read = database.begin_read().context("begin redb read")?;
        let table = read.open_table(PROBE_TABLE).context("reopen redb table")?;
        let value = table
            .get("answer")
            .context("read redb value")?
            .context("redb value is missing")?
            .value();
        Ok(value)
    })();

    let cleanup = fs::remove_file(&path).context("remove temporary redb database");
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn main() -> Result<()> {
    ensure!(
        std::env::args_os().len() == 1,
        "this standalone probe accepts no product-like command surface"
    );

    let result = ProbeResult {
        schema: "lumin-phase0-static-packaging-run-v1",
        status: "PASS",
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        target_env: target_env(),
        oxc_statement_count: probe_oxc()?,
        rayon_sum: probe_rayon()?,
        redb_value: probe_redb()?,
    };
    ensure!(
        result.oxc_statement_count == 2,
        "unexpected OXC statement count"
    );
    ensure!(result.rayon_sum == 4950, "unexpected Rayon sum");
    ensure!(result.redb_value == 42, "unexpected redb value");
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
