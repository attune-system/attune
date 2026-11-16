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
        install_workload_arguments(&mut sandbox, &command)?;
        Ok(sandbox)
    }
}

#[cfg(target_os = "linux")]
fn install_workload_arguments(sandbox: &mut Command, command: &Command) -> io::Result<()> {
    use std::io::{Seek, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::{ffi::OsStrExt, process::CommandExt};

    // Workload loader/runtime controls must never affect the host-side launcher.
    // Pass all variable values and executable arguments through an unlinked file,
    // rather than publishing tokens/inline source in Bubblewrap's command line.
    let mut arguments = tempfile::tempfile()?;
    let mut append = |value: &std::ffi::OsStr| -> io::Result<()> {
        if value.as_bytes().contains(&0) {
            return Err(io::Error::other("NUL in workload argument"));
        }
        arguments.write_all(value.as_bytes())?;
        arguments.write_all(&[0])
    };
    append("--clearenv".as_ref())?;
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            append("--setenv".as_ref())?;
            append(key)?;
            append(value)?;
        }
    }
    for (key, value) in [
        ("HOME", "/tmp"),
        ("TMPDIR", "/tmp"),
        ("ATTUNE_ARTIFACTS_DIR", "/tmp/artifacts"),
    ] {
        append("--setenv".as_ref())?;
        append(key.as_ref())?;
        append(value.as_ref())?;
    }
    append("--".as_ref())?;
    append(command.get_program())?;
    for argument in command.get_args() {
        append(argument)?;
    }
    arguments.rewind()?;
    let fd = arguments.as_raw_fd();
    sandbox.env_clear().env("PATH", "/usr/bin:/bin");
    sandbox.arg("--args").arg(fd.to_string());
    // SAFETY: the descriptor is owned by this closure throughout spawn. Only
    // async-signal-safe fcntl executes after fork; parent CLOEXEC is unchanged.
    unsafe {
        sandbox.pre_exec(move || {
            let flags = libc::fcntl(arguments.as_raw_fd(), libc::F_GETFD);
            if flags < 0
                || libc::fcntl(
                    arguments.as_raw_fd(),
                    libc::F_SETFD,
                    flags & !libc::FD_CLOEXEC,
                ) < 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn install_workload_arguments(_: &mut Command, _: &Command) -> io::Result<()> {
    Err(io::Error::other("bubblewrap workloads require Linux"))
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

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires Linux bubblewrap, cc and unprivileged namespaces"]
    fn bubblewrap_loader_environment_is_installed_after_isolation() {
        let root = tempfile::TempDir::new().unwrap();
        let pack = root.path().join("pack");
        std::fs::create_dir(&pack).unwrap();
        let marker = root.path().join("host-only-marker");
        let source = pack.join("preload.c");
        let library = pack.join("preload.so");
        std::fs::write(
            &source,
            format!(
                r#"#include <fcntl.h>
#include <unistd.h>
__attribute__((constructor)) static void probe(void) {{
 int fd = open("{}", O_WRONLY|O_CREAT, 0600);
 if (fd >= 0) {{ write(fd, "escaped", 7); close(fd); }}
}}
"#,
                marker.display()
            ),
        )
        .unwrap();
        assert!(Command::new("cc")
            .args(["-shared", "-fPIC"])
            .arg(&source)
            .arg("-o")
            .arg(&library)
            .status()
            .unwrap()
            .success());
        let mut plain = Command::new("/bin/true");
        plain.env_clear().env("LD_PRELOAD", &library);
        assert!(plain.status().unwrap().success());
        assert!(marker.exists(), "loader probe must run without sandbox");
        std::fs::remove_file(&marker).unwrap();
        let mut workload = Command::new("/bin/sh");
        workload
            .args([
                "-c",
                "test -n \"$LD_PRELOAD\" && test \"$ATTUNE_API_TOKEN\" = dummy-scoped-token",
            ])
            .env_clear()
            .env("LD_PRELOAD", &library)
            .env("ATTUNE_API_TOKEN", "dummy-scoped-token");
        let mut sandbox = WorkloadSandbox::Bubblewrap
            .wrap(workload, &[&pack])
            .unwrap();
        assert!(sandbox.get_envs().all(|(key, _)| key == "PATH"));
        assert!(
            !format!("{:?}", sandbox.get_args().collect::<Vec<_>>()).contains("dummy-scoped-token")
        );
        let output = sandbox.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!marker.exists(), "loader must not run in host namespaces");
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
