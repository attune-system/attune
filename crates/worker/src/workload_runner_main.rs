//! Public helper bytes only. Private rendered plan arrives once on stdin inside
//! a CP-admitted OCI container; this binary is not a host isolation backend.
use attune_common::child_process_environment::ChildProcessEnvironment;
use attune_common::workload_launch::{PACK_ROOT, RUNNER_ROOT, RUNTIME_ROOT};
use attune_worker::runtime::launch_planning::{SetupPlan, SETUP_STDIN_LIMIT};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
};

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1 && (arguments[0] == "--help" || arguments[0] == "-h") {
        println!(
            "attune-workload-runner setup: private stdin plan in an admitted OCI Setup container"
        );
        return;
    }
    if arguments.len() != 1 || arguments[0] != "setup" || setup().is_err() {
        // Never log argv/environment/stdin or the selected program's error.
        eprintln!("workload setup failed or requires an admitted OCI Setup boundary");
        std::process::exit(1);
    }
}
fn setup() -> std::io::Result<()> {
    boundary_guard()?;
    let mut input = Vec::new();
    std::io::stdin()
        .take(SETUP_STDIN_LIMIT as u64 + 1)
        .read_to_end(&mut input)?;
    if input.len() > SETUP_STDIN_LIMIT {
        return Err(invalid());
    }
    let plan: SetupPlan = serde_json::from_slice(&input).map_err(|_| invalid())?;
    plan.validate().map_err(|_| invalid())?;
    // Empty explicit baseline, not Default/capture of the helper's environment.
    let baseline = ChildProcessEnvironment::from_parent(
        std::iter::empty::<(std::ffi::OsString, std::ffi::OsString)>(),
        &[],
    )
    .map_err(|_| invalid())?;
    for step in plan.steps {
        if let Some(relative) = step.if_pack_file {
            let path = Path::new(PACK_ROOT).join(relative);
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
            .current_dir(PACK_ROOT)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        if !child.status()?.success() {
            return Err(invalid());
        }
    }
    Ok(())
}
fn invalid() -> std::io::Error {
    std::io::Error::other("invalid workload setup boundary or private plan")
}

/// Fail closed on obvious direct-host/mispackaging use. These checks are not an
/// attestation of isolation: CP/controller immutable policy and real Engine
/// boundary tests establish that contract, not a pack-selected environment flag.
#[cfg(target_os = "linux")]
fn boundary_guard() -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
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
    for (path, readonly) in [
        ("/", true),
        (PACK_ROOT, true),
        (RUNNER_ROOT, true),
        (RUNTIME_ROOT, false),
    ] {
        if !Path::new(path).is_dir() {
            return Err(invalid());
        }
        let path = std::ffi::CString::new(Path::new(path).as_os_str().as_bytes())
            .map_err(|_| invalid())?;
        let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let info = unsafe { info.assume_init() };
        if (info.f_flag & libc::ST_RDONLY != 0) != readonly {
            return Err(invalid());
        }
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn boundary_guard() -> std::io::Result<()> {
    Err(invalid())
}
