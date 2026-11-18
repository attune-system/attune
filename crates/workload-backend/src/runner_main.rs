#[cfg(target_os = "linux")]
mod gated_runner;
// Public helper bytes only; no control-plane authority/configuration.
use attune_common::child_process_environment::ChildProcessEnvironment;
use attune_common::workload_launch::PACK_ROOT;
#[cfg(target_os = "linux")]
use attune_common::workload_launch::{RUNNER_ROOT, RUNTIME_ROOT};
#[cfg(unix)]
use attune_workload_backend::planning::validate_cli_exec;
use attune_workload_backend::planning::{SetupPlan, SETUP_STDIN_LIMIT};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
};

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1 && (arguments[0] == "--help" || arguments[0] == "-h") {
        println!(
            "attune-workload-runner setup|exec: private selected plan in an admitted OCI boundary"
        );
        return;
    }
    let result = match arguments.first().and_then(|value| value.to_str()) {
        #[cfg(target_os = "linux")]
        Some("gated") if arguments.len() == 1 => match gated_runner::run() {
            Ok(code) => std::process::exit(code),
            Err(error) => Err(error),
        },
        Some("install") if arguments.len() == 1 => {
            attune_workload_backend::transfer_runner::install()
        }
        Some("transfer") if arguments.len() == 1 => boundary_guard_basic().and_then(|_| loop {
            std::thread::park();
        }),
        Some("seed") | Some("archive") => {
            let selected = arguments[1..]
                .iter()
                .map(|v| v.to_str().map(str::to_owned).ok_or_else(invalid))
                .collect::<std::io::Result<Vec<_>>>();
            boundary_guard_basic().and(selected).and_then(|values| {
                if arguments[0] == "seed" {
                    attune_workload_backend::transfer_runner::seed(&values)
                } else {
                    attune_workload_backend::transfer_runner::archive(&values)
                }
            })
        }
        Some("read-artifact") => {
            let selected = arguments[1..]
                .iter()
                .map(|v| v.to_str().map(str::to_owned).ok_or_else(invalid))
                .collect::<std::io::Result<Vec<_>>>();
            match boundary_guard_basic()
                .and(selected)
                .and_then(|values| attune_workload_backend::transfer_runner::read_artifact(&values))
            {
                Ok(true) => Ok(()),
                Ok(false) => std::process::exit(
                    attune_workload_backend::transfer_runner::MISSING_ARTIFACT_EXIT_CODE,
                ),
                Err(error) => Err(error),
            }
        }
        #[cfg(target_os = "linux")]
        Some("terminate") if arguments.len() == 1 => boundary_guard_basic()
            .and_then(|_| attune_workload_backend::transfer_runner::terminate()),
        Some("setup") if arguments.len() == 1 => setup(),
        Some("exec") => execute(&arguments[1..]),
        _ => Err(invalid()),
    };
    if result.is_err() {
        // Never log argv/environment/stdin or the selected program's error.
        eprintln!("workload helper failed or requires an admitted OCI boundary");
        std::process::exit(1);
    }
}
fn setup() -> std::io::Result<()> {
    boundary_guard(false)?;
    let mut input = Vec::new();
    std::io::stdin()
        .take(SETUP_STDIN_LIMIT as u64 + 1)
        .read_to_end(&mut input)?;
    if input.len() > SETUP_STDIN_LIMIT {
        return Err(invalid());
    }
    let plan: SetupPlan = serde_json::from_slice(&input).map_err(|_| invalid())?;
    plan.validate().map_err(|_| invalid())?;
    attune_workload_backend::transfer_runner::prepare_setup_project()?;
    // Empty explicit baseline, not Default/capture of the helper's environment.
    let baseline = ChildProcessEnvironment::from_parent(
        std::iter::empty::<(std::ffi::OsString, std::ffi::OsString)>(),
        &[],
    )
    .map_err(|_| invalid())?;
    for step in plan.steps {
        if let Some(relative) = step.if_pack_file {
            let path = Path::new(attune_workload_backend::planning::PROJECT_ROOT).join(relative);
            match std::fs::metadata(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Ok(metadata) if metadata.is_file() => {}
                _ => return Err(invalid()),
            }
        }
        let mut child = Command::new(&step.argv[0]);
        baseline.apply(&mut child);
        child
            .args(&step.argv[1..])
            .envs(&plan.environment)
            .current_dir(attune_workload_backend::planning::PROJECT_ROOT)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        if !child.status()?.success() {
            return Err(invalid());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn execute(arguments: &[std::ffi::OsString]) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    boundary_guard(true)?;
    if arguments.len() < 3 {
        return Err(invalid());
    }
    let selected = attune_common::action_cli::SelectedProgram {
        sha256: arguments[0].to_str().ok_or_else(invalid)?.into(),
        path: arguments[1].clone().into(),
        invocation_name: arguments[2].to_str().ok_or_else(invalid)?.into(),
        artifact: None,
        variant: None,
    };
    let argv = arguments[3..]
        .iter()
        .map(|argument| argument.to_str().map(str::to_owned).ok_or_else(invalid))
        .collect::<std::io::Result<Vec<_>>>()?;
    validate_cli_exec(&selected, &argv).map_err(|_| invalid())?;
    readonly_path(&selected.path, true)?;
    attune_common::action_cli::verify_selected_program(&selected).map_err(|_| invalid())?;
    let mut command = Command::new(&selected.path);
    // The only inherited environment is the controller's cleared, explicitly
    // selected Engine context. Do not capture a manager/default configuration.
    command
        .env_clear()
        .envs(std::env::vars_os())
        .arg0(&selected.invocation_name)
        .args(&argv)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    Err(command.exec()) // Replaces the helper; selected stdin is never consumed.
}
#[cfg(not(unix))]
fn execute(_: &[std::ffi::OsString]) -> std::io::Result<()> {
    Err(invalid())
}
fn invalid() -> std::io::Error {
    std::io::Error::other("invalid workload boundary or private invocation")
}

/// Fail closed on obvious direct-host/mispackaging use. These checks are not an
/// attestation of isolation: CP/controller immutable policy and real Engine
/// boundary tests establish that contract, not a pack-selected environment flag.
#[cfg(target_os = "linux")]
fn boundary_guard_basic() -> std::io::Result<()> {
    if unsafe { libc::geteuid() } != 1000 || unsafe { libc::getegid() } != 1000 {
        return Err(invalid());
    }
    let status = std::fs::read_to_string("/proc/self/status")?;
    if !status.lines().any(|line| line == "NoNewPrivs:\t1")
        || !status
            .lines()
            .any(|line| line == "CapEff:\t0000000000000000")
    {
        return Err(invalid());
    }
    readonly_path(Path::new("/"), true)?;
    readonly_path(&std::env::current_exe()?, true)
}

#[cfg(target_os = "linux")]
fn boundary_guard(execution: bool) -> std::io::Result<()> {
    if unsafe { libc::geteuid() } != 1000 || unsafe { libc::getegid() } != 1000 {
        return Err(invalid());
    }
    let status = std::fs::read_to_string("/proc/self/status")?;
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(str::trim)
    };
    if field("NoNewPrivs:") != Some("1")
        || field("CapEff:").and_then(|value| u64::from_str_radix(value, 16).ok()) != Some(0)
    {
        return Err(invalid());
    }
    for (path, readonly) in [("/", true), (PACK_ROOT, true), (RUNNER_ROOT, true)] {
        if !Path::new(path).is_dir() {
            return Err(invalid());
        }
        readonly_path(Path::new(path), readonly)?;
    }
    if !execution || Path::new(RUNTIME_ROOT).exists() {
        if !Path::new(RUNTIME_ROOT).is_dir() {
            return Err(invalid());
        }
        readonly_path(Path::new(RUNTIME_ROOT), execution)?;
    }
    if execution {
        let current = std::env::current_dir()?;
        let pack = Path::new(PACK_ROOT).canonicalize()?;
        let project = Path::new(attune_workload_backend::planning::PROJECT_ROOT)
            .canonicalize()
            .ok();
        if current != pack && project.as_ref() != Some(&current) {
            return Err(invalid());
        }
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn boundary_guard(_: bool) -> std::io::Result<()> {
    Err(invalid())
}

#[cfg(target_os = "linux")]
fn readonly_path(path: &Path, readonly: bool) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| invalid())?;
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    if (info.f_flag & libc::ST_RDONLY != 0) != readonly {
        return Err(invalid());
    }
    Ok(())
}
#[cfg(all(unix, not(target_os = "linux")))]
fn readonly_path(_: &Path, _: bool) -> std::io::Result<()> {
    Err(invalid())
}
