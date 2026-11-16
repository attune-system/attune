//! Optional Linux isolation for final workload processes. Setup commands remain
//! outside this boundary; see docs/deployment/workload-isolation.md.
use std::io;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy, Debug, Default)]
pub enum WorkloadSandbox {
    #[default]
    Disabled,
    Bubblewrap,
}

impl WorkloadSandbox {
    /// Operator opt-in. Never silently downgrade an explicitly requested boundary.
    pub fn from_environment(api_artifacts: bool) -> io::Result<Self> {
        match std::env::var("ATTUNE_WORKLOAD_SANDBOX") {
            Err(std::env::VarError::NotPresent) => Ok(Self::Disabled),
            Ok(value) if value == "disabled" => Ok(Self::Disabled),
            Ok(value) if value == "bubblewrap" => {
                if !cfg!(target_os = "linux") || !api_artifacts {
                    return Err(io::Error::other(
                        "bubblewrap workloads require Linux and API artifact transport",
                    ));
                }
                Ok(Self::Bubblewrap)
            }
            _ => Err(io::Error::other("invalid ATTUNE_WORKLOAD_SANDBOX mode")),
        }
    }

    /// Call before configuring stdio, process groups or pre_exec hooks. Only
    /// trusted selected pack/runtime paths may be supplied, never a shared root.
    pub fn wrap(self, command: Command, read_paths: &[&Path]) -> io::Result<Command> {
        if matches!(self, Self::Disabled) {
            return Ok(command);
        }
        if !cfg!(target_os = "linux") {
            return Err(io::Error::other("bubblewrap workloads require Linux"));
        }
        let mut sandbox = Command::new("/usr/bin/bwrap");
        sandbox.args([
            "--unshare-user",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--die-with-parent",
            "--new-session",
            "--cap-drop",
            "ALL",
            "--disable-userns",
            "--ro-bind",
            "/usr",
            "/usr",
        ]);
        for path in ["/bin", "/sbin", "/lib", "/lib64"] {
            let path = Path::new(path);
            if path.exists() {
                if let Ok(target) = std::fs::read_link(path) {
                    sandbox.arg("--symlink").arg(target).arg(path);
                } else {
                    sandbox.arg("--ro-bind").arg(path).arg(path);
                }
            }
        }
        for path in [
            "/etc/resolv.conf",
            "/etc/hosts",
            "/etc/nsswitch.conf",
            "/etc/ssl/certs",
            "/etc/ld.so.cache",
        ] {
            if Path::new(path).exists() {
                sandbox.args(["--ro-bind", path, path]);
            }
        }
        sandbox.args([
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--dir",
            "/tmp/artifacts",
        ]);
        for path in read_paths {
            if !path.exists() {
                return Err(io::Error::other("sandbox input path does not exist"));
            }
            // Reject aliases and broad/system-root mounts rather than exposing a
            // credential-bearing tree through a selected pack/runtime alias.
            let canonical = path.canonicalize()?;
            if !path.is_absolute()
                || canonical != *path
                || path.components().count() < 3
                || ["/proc", "/dev", "/etc", "/usr", "/bin", "/lib", "/lib64"]
                    .iter()
                    .any(|root| path.starts_with(root))
            {
                return Err(io::Error::other("unsafe sandbox input path"));
            }
            sandbox.arg("--ro-bind").arg(path).arg(path);
        }
        sandbox
            .arg("--chdir")
            .arg(command.get_current_dir().unwrap_or(Path::new("/tmp")));
        sandbox
            .arg("--")
            .arg(command.get_program())
            .args(command.get_args());
        sandbox.env_clear();
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                sandbox.env(key, value);
            }
        }
        // No inherited home/config or host scratch/artifact tree is mounted.
        sandbox
            .env("HOME", "/tmp")
            .env("TMPDIR", "/tmp")
            .env("ATTUNE_ARTIFACTS_DIR", "/tmp/artifacts");
        Ok(sandbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;

    #[test]
    fn rejects_unsafe_mounts_without_running_workload() {
        for path in ["/", "/etc", "/proc", "/usr", "/dev", "/tmp/../etc"] {
            assert!(WorkloadSandbox::Bubblewrap
                .wrap(Command::new("true"), &[Path::new(path)])
                .is_err());
        }
    }

    #[test]
    fn disabled_preserves_command() {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("exit 0")
            .env_clear()
            .env("TEST", "value");
        let command = WorkloadSandbox::Disabled.wrap(command, &[]).unwrap();
        assert_eq!(command.get_program(), "/bin/sh");
        assert_eq!(command.get_envs().count(), 1);
    }

    // This is a required boundary test when explicitly selected. Missing bwrap
    // or disabled user namespaces fail it, rather than yielding passing evidence.
    #[test]
    #[ignore = "requires Linux bubblewrap and unprivileged user/PID namespaces"]
    fn bubblewrap_blocks_sibling_procfs_and_runtime_poisoning() {
        let root = std::env::temp_dir().join(format!("attune-sandbox-test-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let pack = root.join("pack");
        let runtime = root.join("runtime");
        std::fs::create_dir(&pack).unwrap();
        std::fs::create_dir(&runtime).unwrap();
        std::fs::write(runtime.join("published"), "original").unwrap();
        let secret = root.join("manager-secret");
        std::fs::write(&secret, "owned-dummy-manager-secret").unwrap();
        let mut sibling = Command::new("/bin/sh")
            .args(["-c", "echo ready; read release"])
            .env_clear()
            .env("DUMMY_MANAGER_SECRET", "owned-dummy-sibling-secret")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        BufReader::new(sibling.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        let pid = sibling.id();
        // Establish that this host permits the original same-UID sibling read.
        let baseline = std::fs::read(format!("/proc/{pid}/environ")).unwrap();
        let baseline_readable = baseline
            .windows(b"owned-dummy-sibling-secret".len())
            .any(|s| s == b"owned-dummy-sibling-secret");
        let script = format!(
            r#"
set -eu
[ ! -e '{secret}' ]
[ ! -e '/proc/{pid}/environ' ]
[ ! -e '/proc/{pid}/root' ]
[ "$(cat '{runtime}/published')" = original ]
if echo poisoned > '{runtime}/published' 2>/dev/null; then exit 41; fi
[ "$ATTUNE_API_TOKEN" = scoped-dummy-token ]
[ -z "${{JWT_SECRET+x}}${{ENCRYPTION_KEY+x}}${{DATABASE_URL+x}}${{MQ_URL+x}}${{DUMMY_MANAGER_SECRET+x}}" ]
read payload
[ "$payload" = '{{"message":"allowed"}}' ]
echo private > /tmp/private
[ "$(cat /tmp/private)" = private ]
echo boundary-ok
"#,
            secret = secret.display(),
            runtime = runtime.display()
        );
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", &script])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("ATTUNE_API_TOKEN", "scoped-dummy-token")
            .current_dir(&pack);
        let result = (|| -> io::Result<std::process::Output> {
            let mut sandbox = WorkloadSandbox::Bubblewrap.wrap(command, &[&pack, &runtime])?;
            let mut child = sandbox
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"{\"message\":\"allowed\"}\n")?;
            child.wait_with_output()
        })();
        // Await the exact owned process before removing its files, even on failure.
        drop(sibling.stdin.take());
        sibling.wait().unwrap();
        assert_eq!(
            std::fs::read_to_string(runtime.join("published")).unwrap(),
            "original"
        );
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(ready, "ready\n");
        assert!(
            baseline_readable,
            "owned sibling initial environment must be readable for this test"
        );
        let output = result.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"boundary-ok\n");
    }
}
