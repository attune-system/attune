//! Optional Linux isolation for pack-controlled local processes.
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
    pub async fn from_environment(api_artifacts: bool) -> io::Result<Self> {
        match std::env::var("ATTUNE_WORKLOAD_SANDBOX") {
            Err(std::env::VarError::NotPresent) => Ok(Self::Disabled),
            Ok(value) if value == "disabled" => Ok(Self::Disabled),
            Ok(value) if value == "bubblewrap" => {
                if !cfg!(target_os = "linux") || !api_artifacts {
                    return Err(io::Error::other(
                        "bubblewrap workloads require Linux and API artifact transport",
                    ));
                }
                Self::Bubblewrap.verify_available().await?;
                Ok(Self::Bubblewrap)
            }
            _ => Err(io::Error::other("invalid ATTUNE_WORKLOAD_SANDBOX mode")),
        }
    }

    async fn verify_available(self) -> io::Result<()> {
        let command = self.wrap(Command::new("true"), &[])?;
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::process::Command::from(command)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| io::Error::other("Bubblewrap startup probe timed out"))?
        .map_err(|error| io::Error::other(format!("Cannot start /usr/bin/bwrap: {error}")))?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "Bubblewrap startup probe failed: {}",
                String::from_utf8_lossy(&output.stderr).trim(),
            )));
        }
        Ok(())
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
        self.wrap_command(command, read_paths, &[], &[], invocation_name, false, false)
    }

    /// Expose only the selected read-only inputs and caller-owned writable
    /// directories. The caller configures HOME, caches and artifact paths.
    pub(crate) fn wrap_staged(
        self,
        command: Command,
        read_paths: &[&Path],
        write_paths: &[&Path],
        tools: &[(&Path, &Path)],
        offline: bool,
    ) -> io::Result<Command> {
        self.wrap_command(command, read_paths, write_paths, tools, None, true, offline)
    }

    #[allow(clippy::too_many_arguments)]
    fn wrap_command(
        self,
        command: Command,
        read_paths: &[&Path],
        write_paths: &[&Path],
        tools: &[(&Path, &Path)],
        invocation_name: Option<&std::ffi::OsStr>,
        preserve_writable_paths: bool,
        offline: bool,
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
        if offline {
            sandbox.arg("--unshare-net");
        }
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
            "/etc/passwd",
            "/etc/group",
            "/etc/alternatives",
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
            validate_mount_path(path)?;
            sandbox.arg("--ro-bind").arg(path).arg(path);
        }
        for path in write_paths {
            validate_mount_path(path)?;
            if !path.is_dir()
                || read_paths
                    .iter()
                    .any(|input| input.starts_with(path) || path.starts_with(input))
            {
                return Err(io::Error::other("unsafe writable sandbox path"));
            }
            sandbox.arg("--bind").arg(path).arg(path);
        }
        for &(invocation, identity) in tools {
            add_tool_mounts(&mut sandbox, invocation, identity)?;
        }
        sandbox
            .arg("--chdir")
            .arg(command.get_current_dir().unwrap_or(Path::new("/tmp")));
        install_workload_environment(&mut sandbox, &command, preserve_writable_paths)?;
        Ok(sandbox)
    }
}

fn add_tool_mounts(sandbox: &mut Command, invocation: &Path, identity: &Path) -> io::Result<()> {
    const SYSTEM_TREES: &[&str] = &["/usr", "/bin", "/sbin", "/lib", "/lib64"];
    if !SYSTEM_TREES.iter().any(|root| identity.starts_with(root)) {
        validate_mount_path(identity)?;
        sandbox.arg("--ro-bind").arg(identity).arg(identity);
    }
    if invocation != identity && !SYSTEM_TREES.iter().any(|root| invocation.starts_with(root)) {
        // The alias is a destination in the private root. Bubblewrap binds
        // the already verified executable without mounting its host parent.
        if !is_allowed_mount_location(invocation) || invocation.canonicalize()? != identity {
            return Err(io::Error::other("unsafe sandbox executable path"));
        }
        sandbox.arg("--ro-bind").arg(identity).arg(invocation);
    }
    Ok(())
}

fn validate_mount_path(path: &Path) -> io::Result<()> {
    // Reject aliases and broad/system-root mounts rather than exposing a
    // credential-bearing tree through a selected pack/runtime alias.
    if !is_allowed_mount_location(path) || path.canonicalize()? != path {
        return Err(io::Error::other("unsafe sandbox input path"));
    }
    Ok(())
}

fn is_allowed_mount_location(path: &Path) -> bool {
    path.is_absolute()
        && path.components().count() >= 3
        && !path
            .components()
            .any(|component| component == std::path::Component::ParentDir)
        && ![
            "/proc", "/sys", "/dev", "/etc", "/usr", "/bin", "/sbin", "/lib", "/lib64",
        ]
        .iter()
        .any(|root| path.starts_with(root))
}

#[cfg(target_os = "linux")]
fn install_workload_environment(
    sandbox: &mut Command,
    command: &Command,
    preserve_writable_paths: bool,
) -> io::Result<()> {
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
        if preserve_writable_paths && command.get_envs().any(|(name, _)| name == key) {
            continue;
        }
        append("--setenv".as_ref())?;
        append(key.as_ref())?;
        append(value.as_ref())?;
    }
    if !command
        .get_envs()
        .any(|(key, value)| key == "PYTHONPYCACHEPREFIX" && value.is_some())
    {
        append("--setenv".as_ref())?;
        append("PYTHONPYCACHEPREFIX".as_ref())?;
        append("/tmp/pycache".as_ref())?;
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
    // SAFETY: the descriptor is owned by this closure throughout spawn. The
    // syscalls below affect the forked child's descriptors only.
    unsafe {
        sandbox.pre_exec(move || {
            // Bubblewrap can forward inherited descriptors to the workload.
            // Pass only the controlled stdio streams and Bubblewrap's args.
            if libc::syscall(
                libc::SYS_close_range,
                3u32,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            ) < 0
                || libc::fcntl(arguments.as_raw_fd(), libc::F_SETFD, 0) < 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn install_workload_environment(_: &mut Command, _: &Command, _: bool) -> io::Result<()> {
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
    #[ignore = "requires Linux Bubblewrap with unprivileged namespaces"]
    fn bubblewrap_closes_inherited_descriptors() {
        use std::os::{fd::AsRawFd, unix::process::CommandExt};

        const FD_ENV: &str = "ATTUNE_SANDBOX_TEST_INHERITED_FD";
        if let Some(fd) = std::env::var_os(FD_ENV) {
            let fd: i32 = fd.to_str().unwrap().parse().unwrap();
            assert!(unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0);
            let mut workload = Command::new("/bin/sh");
            workload.args(["-c", &format!("test ! -e /proc/self/fd/{fd}")]);
            let output = WorkloadSandbox::Bubblewrap
                .wrap(workload, &[])
                .unwrap()
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            return;
        }

        let fixture = tempfile::tempfile().unwrap();
        let fd = fixture.as_raw_fd();
        let mut test_process = Command::new(std::env::current_exe().unwrap());
        test_process
            .args([
                "workload_sandbox::tests::bubblewrap_closes_inherited_descriptors",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .env(FD_ENV, fd.to_string());
        // SAFETY: fixture owns fd throughout spawn. Only the forked test
        // process inherits it.
        unsafe {
            test_process.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let output = test_process.output().unwrap();
        assert!(output.status.success(), "{output:?}");
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
if kill -0 {pid} 2>/dev/null; then exit 40; fi
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
