#[cfg(feature = "lifecycle-test-fault")]
use std::ffi::{OsStr, OsString};

#[cfg(all(feature = "lifecycle-test-fault", not(debug_assertions)))]
compile_error!("lifecycle-test-fault is restricted to debug test builds");

#[cfg(feature = "lifecycle-test-fault")]
const DELIVERY_FAILURE_ENV: &str = "LUMIN_TEST_FAIL_RESULT_DELIVERY";

fn main() {
    let root = match std::env::current_dir() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("lumin: cannot read current directory: {error}");
            std::process::exit(1);
        }
    };
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    #[cfg(feature = "lifecycle-test-fault")]
    let fail_result_delivery = delivery_failure_requested(&arguments);
    let output = lumin_cli::execute(&root, arguments);
    #[cfg(feature = "lifecycle-test-fault")]
    if fail_result_delivery && !output.stdout.is_empty() {
        eprintln!("lumin: injected result delivery failure after commit");
        std::process::exit(1);
    }
    if !output.stdout.is_empty() {
        println!("{}", output.stdout);
    }
    if !output.stderr.is_empty() {
        eprint!("{}", output.stderr);
    }
    if output.exit_code != 0 {
        std::process::exit(output.exit_code);
    }
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
