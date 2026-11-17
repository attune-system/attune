//! Linux isolation for pack-controlled executable stages.
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
    /// Isolated by default; an explicit trusted-only deployment accepts host execution.
    pub fn from_environment(api_artifacts: bool) -> io::Result<Self> {
        match std::env::var("ATTUNE_WORKLOAD_SANDBOX") {
            Ok(value) if value == "trusted-only" => Ok(Self::Disabled),
            value
                if matches!(&value, Err(std::env::VarError::NotPresent))
                    || matches!(&value, Ok(value) if value == "bubblewrap") =>
            {
                if !cfg!(target_os = "linux") || !api_artifacts {
                    return Err(io::Error::other(
                        "bubblewrap workloads require Linux and API artifact transport",
                    ));
                }
                let mut probe = Self::Bubblewrap.wrap(Command::new("/bin/true"), &[])?;
                let result = probe.output().map_err(|error| {
                    io::Error::other(format!(
                        "isolated workloads require usable /usr/bin/bwrap: {error}"
                    ))
                })?;
                if !result.status.success() {
                    return Err(io::Error::other(format!(
                        "workload isolation prerequisite check failed: {}",
                        String::from_utf8_lossy(&result.stderr)
                    )));
                }
                Ok(Self::Bubblewrap)
            }
            _ => Err(io::Error::other("invalid ATTUNE_WORKLOAD_SANDBOX mode")),
        }
    }

    /// Call before configuring stdio, process groups or pre_exec hooks. Only
    /// trusted selected pack/runtime paths may be supplied, never a shared root.
    pub fn wrap(self, command: Command, read_paths: &[&Path]) -> io::Result<Command> {
        self.wrap_with_invocation_name(command, read_paths, None)
    }

    /// Preserve a static executable invocation name through optional isolation.
    /// The name is metadata, never an execution parameter.
    pub fn wrap_with_invocation_name(
        self,
        command: Command,
        read_paths: &[&Path],
        invocation_name: Option<&std::ffi::OsStr>,
    ) -> io::Result<Command> {
        self.wrap_stage_with_invocation_name(command, read_paths, &[], invocation_name)
    }

    /// Writable paths must be unique unpublished staging directories. Never
    /// supply a shared parent or a published runtime here.
    pub fn wrap_stage(
        self,
        command: Command,
        read_paths: &[&Path],
        write_paths: &[&Path],
    ) -> io::Result<Command> {
        self.wrap_stage_with_invocation_name(command, read_paths, write_paths, None)
    }

    fn wrap_stage_with_invocation_name(
        self,
        command: Command,
        read_paths: &[&Path],
        write_paths: &[&Path],
        invocation_name: Option<&std::ffi::OsStr>,
    ) -> io::Result<Command> {
        #[cfg(unix)]
        let command = {
            use std::os::unix::process::CommandExt;
            let mut command = command;
            if let Some(name) = invocation_name {
                command.arg0(name);
            }
            command
        };
        if matches!(self, Self::Disabled) {
            return Ok(command);
        }
        if !cfg!(target_os = "linux") {
            return Err(io::Error::other("bubblewrap workloads require Linux"));
        }
        let mut sandbox = Command::new("/usr/bin/bwrap");
        if let Some(name) = invocation_name {
            sandbox.arg("--argv0").arg(name);
        }
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
        for path in read_paths.iter().chain(write_paths.iter()) {
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
            sandbox
                .arg(if write_paths.contains(path) {
                    "--bind"
                } else {
                    "--ro-bind"
                })
                .arg(path)
                .arg(path);
        }
        sandbox
            .arg("--chdir")
            .arg(command.get_current_dir().unwrap_or(Path::new("/tmp")));
        install_workload_environment(&mut sandbox, &command)?;
        Ok(sandbox)
    }
}

/// Inspect completed staging before any trusted marker write or publication.
/// Setup descendants must have exited before this walk begins.
pub fn validate_staging(root: &Path) -> io::Result<()> {
    let canonical_root = root.canonicalize()?;
    if canonical_root != root || !std::fs::symlink_metadata(root)?.is_dir() {
        return Err(io::Error::other("unsafe runtime staging root"));
    }
    fn visit(path: &Path, root: &Path) -> io::Result<()> {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                let resolved = path.canonicalize()?;
                // Interpreter links into the immutable system tree are needed
                // by Python venvs. Never admit arbitrary host paths.
                if !resolved.starts_with(root) && !resolved.starts_with("/usr") {
                    return Err(io::Error::other(
                        "runtime staging symlink escapes allowed trees",
                    ));
                }
                if path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(".attune"))
                {
                    return Err(io::Error::other("runtime marker must not be a symlink"));
                }
            } else if metadata.is_dir() {
                visit(&path, root)?;
            } else if metadata.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if metadata.nlink() != 1 {
                        return Err(io::Error::other(
                            "runtime staging hardlinks are unsupported",
                        ));
                    }
                }
            } else {
                return Err(io::Error::other("runtime staging contains a special file"));
            }
        }
        Ok(())
    }
    visit(root, root)
}

#[cfg(target_os = "linux")]
fn install_workload_environment(sandbox: &mut Command, command: &Command) -> io::Result<()> {
    use std::io::{Seek, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::{ffi::OsStrExt, process::CommandExt};

    // Workload loader/runtime controls must never affect the host-side launcher.
    // Pass variable values through an unlinked file instead of publishing API
    // tokens in Bubblewrap's command line. Bubblewrap's --args accepts options;
    // the executable and its ordinary arguments must follow on the main argv.
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
    arguments.rewind()?;
    let fd = arguments.as_raw_fd();
    sandbox.env_clear().env("PATH", "/usr/bin:/bin");
    sandbox
        .arg("--args")
        .arg(fd.to_string())
        .arg("--")
        .arg(command.get_program())
        .args(command.get_args());
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
fn install_workload_environment(_: &mut Command, _: &Command) -> io::Result<()> {
    Err(io::Error::other("bubblewrap workloads require Linux"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;

    #[cfg(unix)]
    #[test]
    fn publication_rejects_escape_links_and_special_files() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("stage");
        std::fs::create_dir(&staging).unwrap();
        let outside = root.path().join("secret");
        std::fs::write(&outside, "dummy").unwrap();
        symlink(&outside, staging.join("escape")).unwrap();
        assert!(validate_staging(&staging).is_err());
        std::fs::remove_file(staging.join("escape")).unwrap();
        std::fs::hard_link(&outside, staging.join("hardlink")).unwrap();
        assert!(validate_staging(&staging).is_err());
        std::fs::remove_file(staging.join("hardlink")).unwrap();
        symlink("/usr/bin", staging.join("bin")).unwrap();
        assert!(validate_staging(&staging).is_ok());
        symlink("/usr/bin/true", staging.join(".attune-runtime-ready")).unwrap();
        assert!(validate_staging(&staging).is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "dummy");
    }

    #[test]
    #[ignore = "requires Linux bubblewrap and unprivileged namespaces"]
    fn bubblewrap_setup_exposes_only_owned_stage() {
        let root = tempfile::tempdir().unwrap();
        let pack = root.path().join("pack");
        let stage = root.path().join("stage");
        let sibling = root.path().join("sibling");
        for path in [&pack, &stage, &sibling] {
            std::fs::create_dir(path).unwrap();
        }
        std::fs::write(pack.join("published"), "original").unwrap();
        std::fs::write(sibling.join("token"), "dummy-sibling-token").unwrap();
        let script = format!(
            "set -eu; test ! -e '{}/token'; if echo poisoned > '{}/published'; then exit 41; fi; echo installed > '{}/result'; echo private > /tmp/artifacts/result",
            sibling.display(), pack.display(), stage.display()
        );
        let mut command = Command::new("/bin/sh");
        command.args(["-c", &script]).env_clear().current_dir(&pack);
        let output = WorkloadSandbox::Bubblewrap
            .wrap_stage(command, &[&pack], &[&stage])
            .unwrap()
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(stage.join("result")).unwrap(),
            "installed\n"
        );
        assert_eq!(
            std::fs::read_to_string(pack.join("published")).unwrap(),
            "original"
        );
        validate_staging(&stage).unwrap();
    }

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
    fn named_wrapper_preserves_program_identity_without_reordering_arguments() {
        let mut command = Command::new("/usr/bin/printf");
        command.arg("%s").arg("a b; $(false)");
        let command = WorkloadSandbox::Bubblewrap
            .wrap_with_invocation_name(command, &[], Some("printf".as_ref()))
            .unwrap();
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(&args[..2], &["--argv0", "printf"]);
        assert_eq!(
            &args[args.len() - 3..],
            &["/usr/bin/printf", "%s", "a b; $(false)"]
        );
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
                "test -n \"$LD_PRELOAD\" && test -n \"$ATTUNE_API_TOKEN\"",
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
if kill -0 {pid} 2>/dev/null; then exit 42; fi
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
