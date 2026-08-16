use std::path::{Path, PathBuf};
use std::process::Command;

const PRIVATE_NAMESPACE_ENV: &str = "LUMIN_TEST_PRIVATE_MOUNT_NAMESPACE";

pub(super) fn enter_private_namespace(test_name: &str) -> std::io::Result<bool> {
    if std::env::var_os(PRIVATE_NAMESPACE_ENV).is_some() {
        return Ok(false);
    }
    let output = Command::new("unshare")
        .args(["-Ur", "-m"])
        .arg(std::env::current_exe()?)
        .args(["--exact", test_name, "--nocapture"])
        .env(PRIVATE_NAMESPACE_ENV, "1")
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "private mount-namespace test failed with {}\nstdout={}\nstderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )));
    }
    Ok(true)
}

pub(super) struct DirectoryBindMount {
    target: PathBuf,
    active: bool,
}

impl DirectoryBindMount {
    pub(super) fn install(source: &Path, target: &Path) -> std::io::Result<Self> {
        run("mount", &["--bind"], source, Some(target))?;
        Ok(Self {
            target: target.to_owned(),
            active: true,
        })
    }

    pub(super) fn remove(&mut self) -> std::io::Result<()> {
        if self.active {
            run("umount", &[], &self.target, None)?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for DirectoryBindMount {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

fn run(
    program: &str,
    arguments: &[&str],
    path: &Path,
    second_path: Option<&Path>,
) -> std::io::Result<()> {
    let mut command = Command::new(program);
    command.args(arguments).arg(path);
    if let Some(second_path) = second_path {
        command.arg(second_path);
    }
    let output = command.output()?;
    output.status.success().then_some(()).ok_or_else(|| {
        std::io::Error::other(format!(
            "cannot {program} Linux bind-mount fixture: {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim(),
        ))
    })
}
