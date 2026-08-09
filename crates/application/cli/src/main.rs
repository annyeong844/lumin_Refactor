#[cfg(feature = "lifecycle-test-fault")]
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};

#[cfg(all(feature = "lifecycle-test-fault", not(debug_assertions)))]
compile_error!("lifecycle-test-fault is restricted to debug test builds");

#[cfg(feature = "lifecycle-test-fault")]
const DELIVERY_FAILURE_ENV: &str = "LUMIN_TEST_FAIL_RESULT_DELIVERY";

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
    #[cfg(feature = "lifecycle-test-fault")]
    if fail_result_delivery && !output.stdout.is_empty() {
        write_diagnostic("lumin: injected result delivery failure after commit");
        return 1;
    }
    let stdout = io::stdout();
    let stderr = io::stderr();
    emit_command_output(&output, &mut stdout.lock(), &mut stderr.lock())
}

fn emit_command_output(
    output: &lumin_cli::CommandOutput,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if !output.stdout.is_empty()
        && let Err(error) = stdout
            .write_all(output.stdout.as_bytes())
            .and_then(|()| stdout.write_all(b"\n"))
            .and_then(|()| stdout.flush())
    {
        if error.kind() != io::ErrorKind::BrokenPipe {
            let _ = writeln!(stderr, "lumin: cannot write stdout: {error}");
            let _ = stderr.flush();
            return 1;
        }
        return output.delivery_failure_exit_code();
    }
    if !output.stderr.is_empty() {
        if stderr.write_all(output.stderr.as_bytes()).is_err() && output.exit_code == 0 {
            return 1;
        }
        if stderr.flush().is_err() && output.exit_code == 0 {
            return 1;
        }
    }
    output.exit_code
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
        };
        let mut stdout = FailingWriter(io::ErrorKind::BrokenPipe);
        let mut stderr = Vec::new();

        assert_eq!(emit_command_output(&output, &mut stdout, &mut stderr), 0);
        assert!(stderr.is_empty());
    }

    #[test]
    fn broken_stdout_pipe_requires_recovery_for_a_committed_mutation() {
        let output = lumin_cli::CommandOutput {
            exit_code: 0,
            stdout: "{\"gateId\":\"gate_1\"}".to_owned(),
            stderr: String::new(),
            result_delivery: lumin_cli::CommandResultDelivery::RecoverableMutation,
        };
        let mut stdout = FailingWriter(io::ErrorKind::BrokenPipe);
        let mut stderr = Vec::new();

        assert_eq!(emit_command_output(&output, &mut stdout, &mut stderr), 1);
        assert!(stderr.is_empty());
    }

    #[test]
    fn non_pipe_stdout_failure_is_reported_as_an_io_error() -> io::Result<()> {
        let output = lumin_cli::CommandOutput {
            exit_code: 0,
            stdout: "{\"status\":\"ok\"}".to_owned(),
            stderr: String::new(),
            result_delivery: lumin_cli::CommandResultDelivery::ReadOnly,
        };
        let mut stdout = FailingWriter(io::ErrorKind::WriteZero);
        let mut stderr = Vec::new();

        assert_eq!(emit_command_output(&output, &mut stdout, &mut stderr), 1);
        assert!(
            String::from_utf8(stderr)
                .map_err(io::Error::other)?
                .starts_with("lumin: cannot write stdout:")
        );
        Ok(())
    }

    #[test]
    fn successful_delivery_preserves_stdout_and_stderr_bytes() {
        let output = lumin_cli::CommandOutput {
            exit_code: 5,
            stdout: "{\"status\":\"stale\"}".to_owned(),
            stderr: "review required\n".to_owned(),
            result_delivery: lumin_cli::CommandResultDelivery::RecoverableMutation,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        assert_eq!(emit_command_output(&output, &mut stdout, &mut stderr), 5);
        assert_eq!(stdout, b"{\"status\":\"stale\"}\n");
        assert_eq!(stderr, b"review required\n");
    }
}
