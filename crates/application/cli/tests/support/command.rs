use std::path::Path;
use std::process::Command;

pub fn lumin_command(root: &Path) -> Result<Command, std::io::Error> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lumin"));
    command.env_clear().current_dir(root);
    #[cfg(windows)]
    command.env(
        "SystemRoot",
        std::env::var_os("SystemRoot").ok_or_else(|| {
            std::io::Error::other("SystemRoot is required to launch the Windows test binary")
        })?,
    );
    Ok(command)
}
