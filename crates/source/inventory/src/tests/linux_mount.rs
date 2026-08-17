use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PRIVATE_NAMESPACE_ENV: &str = "LUMIN_TEST_PRIVATE_MOUNT_NAMESPACE";

pub(super) fn enter_private_namespace(test_name: &str) -> std::io::Result<bool> {
    if std::env::var_os(PRIVATE_NAMESPACE_ENV).is_some() {
        return Ok(false);
    }
    let executable = std::env::current_exe()?;
    let output = Command::new("unshare")
        .args(["-Ur", "-m"])
        .arg(&executable)
        .args(["--exact", test_name, "--nocapture"])
        .env(PRIVATE_NAMESPACE_ENV, "1")
        .output()?;
    if output.status.success() {
        return Ok(true);
    }
    if !unprivileged_namespace_was_denied(&output.stderr) {
        return Err(namespace_error("unprivileged", &output));
    }

    let privileged = Command::new("sudo")
        .args(["--non-interactive", "env"])
        .arg(format!("{PRIVATE_NAMESPACE_ENV}=1"))
        .arg("unshare")
        .arg("-m")
        .arg(executable)
        .args(["--exact", test_name, "--nocapture"])
        .output()?;
    if !privileged.status.success() {
        return Err(std::io::Error::other(format!(
            "{}\n{}",
            namespace_error("unprivileged", &output),
            namespace_error("privileged", &privileged),
        )));
    }
    Ok(true)
}

fn unprivileged_namespace_was_denied(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr)
        .lines()
        .any(|line| line.starts_with("unshare:") && line.contains("Operation not permitted"))
}

fn namespace_error(lane: &str, output: &Output) -> std::io::Error {
    std::io::Error::other(format!(
        "{lane} private mount-namespace test failed with {}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
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

#[test]
fn uid_map_permission_denial_requests_a_privileged_namespace_retry() {
    assert!(unprivileged_namespace_was_denied(
        b"unshare: write failed /proc/self/uid_map: Operation not permitted\n"
    ));
    assert!(!unprivileged_namespace_was_denied(
        b"test fixture failed: mount: Operation not permitted\n"
    ));
}
