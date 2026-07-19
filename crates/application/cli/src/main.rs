fn main() {
    let root = match std::env::current_dir() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("lumin: cannot read current directory: {error}");
            std::process::exit(1);
        }
    };
    let output = lumin_cli::execute(&root, std::env::args_os().skip(1).collect());
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
