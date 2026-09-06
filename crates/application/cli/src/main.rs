#[cfg(feature = "lifecycle-test-fault")]
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};

#[cfg(all(target_os = "linux", target_env = "musl"))]
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "lifecycle-test-fault")]
mod delivery_barrier;

#[cfg(all(feature = "lifecycle-test-fault", not(debug_assertions)))]
compile_error!("lifecycle-test-fault is restricted to debug test builds");

#[cfg(feature = "lifecycle-test-fault")]
const DELIVERY_FAILURE_ENV: &str = "LUMIN_TEST_FAIL_RESULT_DELIVERY";
#[cfg(feature = "lifecycle-test-fault")]
const LIFECYCLE_STORE_MIGRATION_DELIVERY: &str = "lifecycle-store-migration";

fn main() {
    let exit_code = run();
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn run() -> i32 {
    let root = match std::env::current_dir() {
        Ok(root) => root,
        Err(error) => {
            write_diagnostic(&format!("lumin: cannot read current directory: {error}"));
            return 1;
        }
    };
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    #[cfg(feature = "lifecycle-test-fault")]
    let fail_result_delivery = delivery_failure_requested(&arguments);
    let output = lumin_cli::execute(&root, arguments);
    let delivery_sequence = match allocate_mutation_delivery(&root, &output) {
        Ok(sequence) => sequence,
        Err(error) => {
            write_diagnostic(&format!("lumin: {error}"));
            return 1;
        }
    };
    #[cfg(feature = "lifecycle-test-fault")]
    if let Err(error) = wait_cache_cleanup_delivery_barrier(
        &output,
        delivery_sequence,
        delivery_barrier::Stage::Allocation,
    ) {
        write_diagnostic(&format!(
            "lumin: cache cleanup delivery barrier failed: {error}"
        ));
        return 1;
    }
    #[cfg(feature = "lifecycle-test-fault")]
    if fail_result_delivery && !output.stdout.is_empty() {
        let _ = complete_mutation_delivery(
            &root,
            &output,
            delivery_sequence,
            lumin_engine::CacheCleanupDeliveryOutcome::Failed,
        );
        write_diagnostic("lumin: injected result delivery failure after commit");
        return 1;
    }
    let stdout = io::stdout();
    let stderr = io::stderr();
    emit_command_output(
        Some(&root),
        &output,
        delivery_sequence,
        &mut stdout.lock(),
        &mut stderr.lock(),
    )
}

fn emit_command_output(
    root: Option<&std::path::Path>,
    output: &lumin_cli::CommandOutput,
    delivery_sequence: Option<u64>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    #[cfg(feature = "audit-execution-test-profile")]
    let stdout_start = output
        .audit_diagnostic
        .as_ref()
        .map(|_| std::time::Instant::now());
    if !output.stdout.is_empty()
        && let Err(error) = write_stdout(output, delivery_sequence, stdout)
    {
        if let Some(root) = root {
            let _ = complete_mutation_delivery(
                root,
                output,
                delivery_sequence,
                lumin_engine::CacheCleanupDeliveryOutcome::Failed,
            );
        }
        if error.kind() != io::ErrorKind::BrokenPipe {
            if output.mutation_delivery.is_some() {
                let _ = stderr.write_all(b"lumin: cannot write stdout\n");
            } else {
                let _ = writeln!(stderr, "lumin: cannot write stdout: {error}");
            }
            let _ = stderr.flush();
            return 1;
        }
        return output.delivery_failure_exit_code();
    }
    #[cfg(feature = "audit-execution-test-profile")]
    let stdout_end = stdout_start.map(|start| (start.elapsed(), std::time::Instant::now()));
    if !output.stderr.is_empty() {
        if stderr.write_all(output.stderr.as_bytes()).is_err() && output.exit_code == 0 {
            return 1;
        }
        if stderr.flush().is_err() && output.exit_code == 0 {
            return 1;
        }
    }
    if let Some(root) = root
        && let Err(error) = complete_mutation_delivery(
            root,
            output,
            delivery_sequence,
            lumin_engine::CacheCleanupDeliveryOutcome::Succeeded,
        )
    {
        let _ = writeln!(stderr, "lumin: {error}");
        let _ = stderr.flush();
        return 1;
    }
    #[cfg(feature = "audit-execution-test-profile")]
    if let Some(diagnostic) = &output.audit_diagnostic {
        let Some((elapsed, command_end)) = stdout_end else {
            return 1;
        };
        if diagnostic.emit(elapsed, command_end, stderr).is_err() {
            return 1;
        }
    }
    output.exit_code
}

fn write_stdout(
    output: &lumin_cli::CommandOutput,
    delivery_sequence: Option<u64>,
    stdout: &mut dyn Write,
) -> Result<(), std::io::Error> {
    write_stdout_payload(output, delivery_sequence, stdout)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    #[cfg(feature = "lifecycle-test-fault")]
    wait_cache_cleanup_delivery_barrier(
        output,
        delivery_sequence,
        delivery_barrier::Stage::CompleteStdout,
    )?;
    Ok(())
}

#[cfg(feature = "lifecycle-test-fault")]
fn write_stdout_payload(
    output: &lumin_cli::CommandOutput,
    delivery_sequence: Option<u64>,
    stdout: &mut dyn Write,
) -> Result<(), std::io::Error> {
    let bytes = output.stdout.as_bytes();
    if cache_cleanup_delivery_identity(output, delivery_sequence).is_some()
        && delivery_barrier::selected(delivery_barrier::Stage::PartialStdout)?
    {
        let split = bytes.len().div_ceil(2);
        stdout.write_all(&bytes[..split])?;
        stdout.flush()?;
        wait_cache_cleanup_delivery_barrier(
            output,
            delivery_sequence,
            delivery_barrier::Stage::PartialStdout,
        )?;
        stdout.write_all(&bytes[split..])?;
    } else {
        stdout.write_all(bytes)?;
    }
    Ok(())
}

#[cfg(not(feature = "lifecycle-test-fault"))]
fn write_stdout_payload(
    output: &lumin_cli::CommandOutput,
    _delivery_sequence: Option<u64>,
    stdout: &mut dyn Write,
) -> Result<(), std::io::Error> {
    stdout.write_all(output.stdout.as_bytes())
}

#[cfg(feature = "lifecycle-test-fault")]
fn cache_cleanup_delivery_identity(
    output: &lumin_cli::CommandOutput,
    delivery_sequence: Option<u64>,
) -> Option<(&lumin_model::OperationId, u64)> {
    match (&output.mutation_delivery, delivery_sequence) {
        (
            Some(lumin_cli::MutationDeliveryRecord::CacheCleanup { operation_id, .. }),
            Some(sequence),
        ) => Some((operation_id, sequence)),
        _ => None,
    }
}

#[cfg(feature = "lifecycle-test-fault")]
fn wait_cache_cleanup_delivery_barrier(
    output: &lumin_cli::CommandOutput,
    delivery_sequence: Option<u64>,
    stage: delivery_barrier::Stage,
) -> Result<(), std::io::Error> {
    let Some((operation_id, sequence)) = cache_cleanup_delivery_identity(output, delivery_sequence)
    else {
        return Ok(());
    };
    delivery_barrier::wait(stage, operation_id, sequence)
}

fn allocate_mutation_delivery(
    root: &std::path::Path,
    output: &lumin_cli::CommandOutput,
) -> Result<Option<u64>, lumin_engine::EngineError> {
    match &output.mutation_delivery {
        Some(lumin_cli::MutationDeliveryRecord::CacheCleanup {
            operation_id,
            request_digest,
        }) => lumin_engine::allocate_cache_cleanup_delivery(root, operation_id, request_digest)
            .map(Some),
        Some(lumin_cli::MutationDeliveryRecord::LifecycleStoreMigration) => Ok(None),
        None => Ok(None),
    }
}

fn complete_mutation_delivery(
    root: &std::path::Path,
    output: &lumin_cli::CommandOutput,
    delivery_sequence: Option<u64>,
    outcome: lumin_engine::CacheCleanupDeliveryOutcome,
) -> Result<(), lumin_engine::EngineError> {
    match &output.mutation_delivery {
        Some(lumin_cli::MutationDeliveryRecord::CacheCleanup {
            operation_id,
            request_digest,
        }) => lumin_engine::record_cache_cleanup_delivery(
            root,
            operation_id,
            request_digest,
            delivery_sequence
                .ok_or(lumin_engine::EngineError::CacheCleanupDeliverySequenceMissing)?,
            outcome,
        ),
        Some(lumin_cli::MutationDeliveryRecord::LifecycleStoreMigration)
            if delivery_sequence.is_none() =>
        {
            Ok(())
        }
        Some(lumin_cli::MutationDeliveryRecord::LifecycleStoreMigration) => {
            Err(lumin_engine::EngineError::UnexpectedMutationDeliverySequence)
        }
        None if delivery_sequence.is_none() => Ok(()),
        None => Err(lumin_engine::EngineError::UnexpectedMutationDeliverySequence),
    }
}

fn write_diagnostic(message: &str) {
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    let _ = writeln!(stderr, "{message}");
    let _ = stderr.flush();
}

#[cfg(feature = "lifecycle-test-fault")]
fn delivery_failure_requested(arguments: &[OsString]) -> bool {
    let Some(selected_operation_id) = std::env::var_os(DELIVERY_FAILURE_ENV) else {
        return false;
    };
    if selected_operation_id == OsStr::new(LIFECYCLE_STORE_MIGRATION_DELIVERY)
        && arguments
            .first()
            .is_some_and(|argument| argument == "store")
        && arguments
            .get(1)
            .is_some_and(|argument| argument == "migrate")
    {
        return true;
    }
    arguments.windows(2).any(|pair| {
        pair[0].as_os_str() == OsStr::new("--operation-id")
            && pair[1].as_os_str() == selected_operation_id.as_os_str()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingWriter(io::ErrorKind);

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(self.0))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(self.0))
        }
    }

    #[test]
    fn broken_stdout_pipe_preserves_the_command_result_without_a_panic() {
        let output = lumin_cli::CommandOutput {
            exit_code: 0,
            stdout: "{\"status\":\"ok\"}".to_owned(),
            stderr: String::new(),
            result_delivery: lumin_cli::CommandResultDelivery::ReadOnly,
            mutation_delivery: None,
        };
        let mut stdout = FailingWriter(io::ErrorKind::BrokenPipe);
        let mut stderr = Vec::new();

        assert_eq!(
            emit_command_output(None, &output, None, &mut stdout, &mut stderr),
            0
        );
        assert!(stderr.is_empty());
    }

    #[test]
    fn broken_stdout_pipe_requires_recovery_for_a_committed_mutation() {
        let output = lumin_cli::CommandOutput {
            exit_code: 0,
            stdout: "{\"gateId\":\"gate_1\"}".to_owned(),
            stderr: String::new(),
            result_delivery: lumin_cli::CommandResultDelivery::RecoverableMutation,
            mutation_delivery: None,
        };
        let mut stdout = FailingWriter(io::ErrorKind::BrokenPipe);
        let mut stderr = Vec::new();

        assert_eq!(
            emit_command_output(None, &output, None, &mut stdout, &mut stderr),
            1
        );
        assert!(stderr.is_empty());
    }

    #[test]
    fn non_pipe_stdout_failure_is_reported_as_an_io_error() -> io::Result<()> {
        let output = lumin_cli::CommandOutput {
            exit_code: 0,
            stdout: "{\"status\":\"ok\"}".to_owned(),
            stderr: String::new(),
            result_delivery: lumin_cli::CommandResultDelivery::ReadOnly,
            mutation_delivery: None,
        };
        let mut stdout = FailingWriter(io::ErrorKind::WriteZero);
        let mut stderr = Vec::new();

        assert_eq!(
            emit_command_output(None, &output, None, &mut stdout, &mut stderr),
            1
        );
        assert!(
            String::from_utf8(stderr)
                .map_err(io::Error::other)?
                .starts_with("lumin: cannot write stdout:")
        );
        Ok(())
    }

    #[test]
    fn cache_cleanup_non_pipe_stdout_failure_has_the_exact_diagnostic() {
        let output = lumin_cli::CommandOutput {
            exit_code: 0,
            stdout: "{\"status\":\"clean\"}".to_owned(),
            stderr: String::new(),
            result_delivery: lumin_cli::CommandResultDelivery::RecoverableMutation,
            mutation_delivery: Some(lumin_cli::MutationDeliveryRecord::CacheCleanup {
                operation_id: lumin_model::OperationId::from_string("cleanup-output".to_owned()),
                request_digest: "digest".to_owned(),
            }),
        };
        let mut stdout = FailingWriter(io::ErrorKind::WriteZero);
        let mut stderr = Vec::new();

        assert_eq!(
            emit_command_output(None, &output, None, &mut stdout, &mut stderr),
            1
        );
        assert_eq!(stderr, b"lumin: cannot write stdout\n");
    }

    #[test]
    fn successful_delivery_preserves_stdout_and_stderr_bytes() {
        let output = lumin_cli::CommandOutput {
            exit_code: 5,
            stdout: "{\"status\":\"stale\"}".to_owned(),
            stderr: "review required\n".to_owned(),
            result_delivery: lumin_cli::CommandResultDelivery::RecoverableMutation,
            mutation_delivery: None,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        assert_eq!(
            emit_command_output(None, &output, None, &mut stdout, &mut stderr),
            5
        );
        assert_eq!(stdout, b"{\"status\":\"stale\"}\n");
        assert_eq!(stderr, b"review required\n");
    }
}
