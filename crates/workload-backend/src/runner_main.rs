#[cfg(target_os = "linux")]
mod gated_runner;
#[cfg(target_os = "linux")]
mod parameter_input;
#[cfg(unix)]
mod recipe_probe;
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
        println!("attune-workload-runner setup|exec|inspect-command-program|inspect-runtime-tools|run-runtime-recipe: admitted OCI effects");
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
        Some("inspect-command-program") => inspect_command_program(&arguments[1..]),
        #[cfg(unix)]
        Some("inspect-runtime-tools") => recipe_probe::inspect(&arguments[1..]),
        #[cfg(unix)]
        Some("inspect-runtime-verification") => recipe_probe::inspect_legacy(&arguments[1..]),
        #[cfg(unix)]
        Some("run-runtime-recipe") => recipe_probe::run(&arguments[1..]),
        #[cfg(unix)]
        Some("run-candidate-runtime-recipe") => recipe_probe::run_candidate(&arguments[1..]),
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

const INSPECTION_OUTPUT_LIMIT: usize = 16 * 1024;
/// Fixed helper effect in an admitted Probe. No subprocess or manager-side
/// discovery; selected executable metadata alone grants no execution authority.
#[cfg(unix)]
fn inspect_command_program(arguments: &[std::ffi::OsString]) -> std::io::Result<()> {
    boundary_guard(true)?;
    if arguments.len() != 1 {
        return Err(invalid());
    }
    let program = arguments[0].to_str().ok_or_else(invalid)?;
    let environment =
        ChildProcessEnvironment::from_parent(std::env::vars_os(), &[]).map_err(|_| invalid())?;
    let (selected, body) = inspection_result(program, Path::new(PACK_ROOT), &environment)?;
    readonly_path(&selected.path, true)?;
    use std::io::Write;
    std::io::stdout().lock().write_all(&body)
}
#[cfg(not(unix))]
fn inspect_command_program(_: &[std::ffi::OsString]) -> std::io::Result<()> {
    Err(invalid())
}
#[cfg(unix)]
fn inspection_result(
    program: &str,
    release_root: &Path,
    environment: &ChildProcessEnvironment,
) -> std::io::Result<(attune_common::action_cli::SelectedProgram, Vec<u8>)> {
    let launch = attune_common::pack_format::LaunchSpec::Command {
        program: program.into(),
        arguments: Vec::new(),
    };
    launch
        .validate_action_source(
            None,
            attune_common::models::ParameterDelivery::None,
            attune_common::models::ParameterFormat::Json,
        )
        .map_err(|_| invalid())?;
    let selected =
        attune_common::action_cli::select_program(&launch, release_root, None, "", environment)
            .map_err(|_| invalid())?;
    let body = serde_json::to_vec(&selected).map_err(|_| invalid())?;
    if body.is_empty() || body.len() > INSPECTION_OUTPUT_LIMIT {
        return Err(invalid());
    }
    Ok((selected, body))
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

#[cfg(all(test, unix))]
mod inspection_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn inspection_preserves_static_alias_hashes_without_execution_and_uses_only_selected_path() {
        let root = tempfile::tempdir().unwrap();
        let pack = root.path().join("pack");
        let image = root.path().join("image");
        std::fs::create_dir(&pack).unwrap();
        std::fs::create_dir(&image).unwrap();
        let executable = image.join("canonical-program");
        // Not a runnable ELF/script: successful inspection demonstrates that
        // executable selection hashes bytes without attempting a child launch.
        let bytes = b"owned inspection bytes, not an executable format";
        std::fs::write(&executable, bytes).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o555)).unwrap();
        std::os::unix::fs::symlink(&executable, image.join("static-alias")).unwrap();
        let env = ChildProcessEnvironment::from_parent(
            [(
                std::ffi::OsString::from("PATH"),
                image.as_os_str().to_owned(),
            )],
            &[],
        )
        .unwrap();
        let (selected, body) = inspection_result("static-alias", &pack, &env).unwrap();
        assert_eq!(selected.path, executable);
        assert_eq!(selected.invocation_name, "static-alias");
        assert_eq!(
            selected.sha256,
            attune_common::workload_launch::Sha256Digest::of_bytes(bytes).as_str()
        );
        assert!(body.len() <= INSPECTION_OUTPUT_LIMIT);
        assert_eq!(
            serde_json::from_slice::<attune_common::action_cli::SelectedProgram>(&body).unwrap(),
            selected
        );
        let empty = ChildProcessEnvironment::from_parent(std::iter::empty(), &[]).unwrap();
        assert!(inspection_result("static-alias", &pack, &empty).is_err());
        std::fs::write(pack.join("pack-program"), bytes).unwrap();
        std::fs::set_permissions(
            pack.join("pack-program"),
            std::fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        assert!(
            inspection_result(pack.join("pack-program").to_str().unwrap(), &pack, &env).is_err()
        );
        for program in [
            "../program",
            "relative/program",
            "",
            "owned-private\0program",
        ] {
            let error = inspection_result(program, &pack, &env).unwrap_err();
            assert!(!error.to_string().contains("owned-private"));
        }
    }
}
