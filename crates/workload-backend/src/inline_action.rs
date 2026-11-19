//! Fixed private Runner entrypoint for legacy inline action execution.
use attune_common::{
    child_process_environment::ChildProcessEnvironment,
    models::runtime::InlineExecutionStrategy,
    workload_launch::{PACK_ROOT, RUNTIME_ROOT},
};
use attune_workload_backend::{inline_action_plan::InlineActionPlan, planning::PROJECT_ROOT};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    io::{self, Read, Write},
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    path::Path,
    process::{Command, Stdio},
};

pub(super) fn run(arguments: &[OsString]) -> io::Result<i32> {
    super::boundary_guard(true)?;
    attune_common::parent_process_protection::protect_service_parent()
        .map_err(|_| super::invalid())?;
    let arguments = arguments
        .iter()
        .map(|value| value.to_str().map(str::to_owned).ok_or_else(super::invalid))
        .collect::<io::Result<Vec<_>>>()?;
    let plan = InlineActionPlan::decode(&arguments).map_err(|_| super::invalid())?;
    let working_dir = if plan.has_runtime {
        PROJECT_ROOT
    } else {
        PACK_ROOT
    };
    if Path::new(RUNTIME_ROOT).exists() != plan.has_runtime
        || std::env::current_dir()? != Path::new(working_dir).canonicalize()?
    {
        return Err(super::invalid());
    }
    let mut environment: BTreeMap<String, String> = std::env::vars().collect();
    // Reuse the existing invocation environment validator, including scoped API
    // access. No new runtime/control variable is introduced by this helper.
    attune_workload_backend::planning::runtime_recipe_environment(&environment)
        .map_err(|_| super::invalid())?;
    let child_environment = ChildProcessEnvironment::from_parent(
        environment
            .iter()
            .map(|(key, value)| (key.into(), value.into())),
        &[],
    )
    .map_err(|_| super::invalid())?;
    // Resolve only inside the admitted guest image/prepared read-only mounts.
    let interpreter =
        attune_common::runtime_lifecycle::resolve_executable(&plan.interpreter, &child_environment)
            .map_err(|_| super::invalid())?;
    super::readonly_path(&interpreter, true)?;
    super::readonly_path(&interpreter.canonicalize()?, true)?;
    let input = prepare_input(&mut environment, plan.shell_helpers, std::io::stdin())?;
    let directory = tempfile::Builder::new()
        .prefix("attune-inline-")
        .tempdir_in("/tmp")?;
    let file = materialize(&plan, input.bytes.as_deref(), directory.path())?;
    let mut command = Command::new(interpreter);
    command
        .env_clear()
        .envs(&environment)
        .args(final_arguments(
            &plan,
            file.as_ref().map(|file| file.path()),
        )?)
        .current_dir(working_dir)
        .stdin(
            if plan.shell_helpers
                || environment
                    .get("ATTUNE_PARAMETER_DELIVERY")
                    .is_some_and(|delivery| delivery == "none")
            {
                Stdio::null()
            } else if let Some(prepared) = &input.formatted {
                Stdio::from(prepared.stdin.try_clone()?)
            } else {
                Stdio::inherit()
            },
        )
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // The provider retains the original full-attempt deadline and stops all
    // descendants. This helper never mints another lease or manager process.
    let status = command.status()?;
    drop(file);
    drop(input);
    directory.close()?;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)))
}

struct InlineInput {
    bytes: Option<Vec<u8>>,
    formatted: Option<super::parameter_input::PreparedInput>,
}
fn prepare_input(
    environment: &mut BTreeMap<String, String>,
    shell_helpers: bool,
    input: impl Read,
) -> io::Result<InlineInput> {
    let needs_format = super::parameter_input::required(environment)?;
    let bytes = if shell_helpers || needs_format {
        Some(read_parameters(input)?)
    } else {
        None
    };
    // Retain private parameter-file ownership until the application is joined,
    // even when shell helpers consume its stdin separately.
    let formatted = if needs_format {
        Some(super::parameter_input::prepare(
            environment,
            bytes.as_deref().ok_or_else(super::invalid)?,
        )?)
    } else {
        None
    };
    Ok(InlineInput { bytes, formatted })
}

fn read_parameters(input: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input
        .take(attune_common::workload_stdin::MAX_PRIVATE_STDIN_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > attune_common::workload_stdin::MAX_PRIVATE_STDIN_BYTES {
        return Err(super::invalid());
    }
    Ok(bytes)
}

fn materialize(
    plan: &InlineActionPlan,
    parameters: Option<&[u8]>,
    directory: &Path,
) -> io::Result<Option<tempfile::NamedTempFile>> {
    plan.validate().map_err(|_| super::invalid())?;
    if plan.strategy == InlineExecutionStrategy::Direct {
        return Ok(None);
    }
    let source = if plan.shell_helpers {
        let parameters: HashMap<String, serde_json::Value> =
            serde_json::from_slice(parameters.ok_or_else(super::invalid)?)
                .map_err(|_| super::invalid())?;
        attune_common::inline_action::shell_wrapper(&parameters, &plan.source)
            .map_err(|_| super::invalid())?
    } else {
        plan.source.clone()
    };
    // Single-quote escaping can expand each value byte fourfold, and legacy
    // emits both PARAM_* and bare assignments. Retain that eightfold ceiling.
    if source.len()
        > attune_common::workload_stdin::MAX_PRIVATE_STDIN_BYTES * 8 + plan.source.len() + 1024
    {
        return Err(super::invalid());
    }
    let mut file = tempfile::Builder::new()
        .prefix("source-")
        .suffix(&plan.extension)
        .tempfile_in(directory)?;
    file.write_all(source.as_bytes())?;
    file.flush()?;
    file.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(Some(file))
}

fn final_arguments(plan: &InlineActionPlan, path: Option<&Path>) -> io::Result<Vec<String>> {
    match plan.strategy {
        InlineExecutionStrategy::Direct => Ok(vec!["-c".into(), plan.source.clone()]),
        InlineExecutionStrategy::TempFile => {
            let mut arguments = plan.arguments.clone();
            arguments.push(
                path.and_then(Path::to_str)
                    .ok_or_else(super::invalid)?
                    .to_owned(),
            );
            Ok(arguments)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_workload_backend::inline_action_plan::LiteralInlineSource;

    fn plan(helpers: bool) -> InlineActionPlan {
        let config = serde_json::from_value(serde_json::json!({"interpreter":{"binary":"/bin/sh","args":["-e","literal{pack_dir}"]},"inline_execution":{"strategy":"temp_file","extension":"sh","inject_shell_helpers":helpers}})).unwrap();
        InlineActionPlan::new(
            &config,
            LiteralInlineSource::from_snapshot(
                "printf '%s' \"$name\"",
                "printf '%s' \"$name\"",
                Some(&["$.name".into()]),
            )
            .unwrap(),
            false,
        )
        .unwrap()
    }
    #[test]
    fn shell_helpers_materialize_only_in_private_source_and_never_arguments() {
        let directory = tempfile::tempdir().unwrap();
        let plan = plan(true);
        let file = materialize(
            &plan,
            Some(br#"{"name":"secret'$(touch marker)"}"#),
            directory.path(),
        )
        .unwrap()
        .unwrap();
        assert!(file.path().starts_with(directory.path()));
        assert_eq!(file.path().extension().unwrap(), "sh");
        assert_eq!(
            file.as_file().metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        let source = std::fs::read_to_string(file.path()).unwrap();
        assert!(source.contains("name='secret'\\''$(touch marker)'"));
        assert!(!source.contains("export "));
        let arguments = final_arguments(&plan, Some(file.path())).unwrap();
        assert_eq!(arguments[0..2], ["-e", "literal{pack_dir}"]);
        assert!(!arguments
            .iter()
            .any(|arg| arg.contains("secret") || arg.contains("touch marker")));
        // Legacy inline files are writable by their owning payload. This is
        // still private scratch, never the read-only Pack or Runtime input.
        std::fs::write(file.path(), "owned private rewrite").unwrap();
        assert_eq!(
            std::fs::read_to_string(file.path()).unwrap(),
            "owned private rewrite"
        );
        let path = file.path().to_owned();
        drop(file);
        assert!(!path.exists());
        directory.close().unwrap();
    }
    #[test]
    fn direct_ignores_helpers_and_does_not_materialize_or_consume_input() {
        let directory = tempfile::tempdir().unwrap();
        let mut plan = plan(false);
        plan.strategy = InlineExecutionStrategy::Direct;
        plan.arguments.clear();
        plan.extension.clear();
        assert!(materialize(&plan, None, directory.path())
            .unwrap()
            .is_none());
        assert_eq!(
            final_arguments(&plan, None).unwrap(),
            ["-c", plan.source.as_str()]
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        directory.close().unwrap();
    }
    #[test]
    fn malformed_or_missing_helper_input_creates_no_source_file() {
        let directory = tempfile::tempdir().unwrap();
        let plan = plan(true);
        for bytes in [None, Some(b"[]".as_slice()), Some(b"not json".as_slice())] {
            assert!(materialize(&plan, bytes, directory.path()).is_err());
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        }
        directory.close().unwrap();
    }
    #[test]
    fn declared_file_formats_keep_private_ownership_with_shell_helpers() {
        for format in ["json", "yaml", "dotenv"] {
            let mut environment = BTreeMap::from([
                ("ATTUNE_PARAMETER_DELIVERY".into(), "file".into()),
                ("ATTUNE_PARAMETER_FORMAT".into(), format.into()),
            ]);
            let input = prepare_input(
                &mut environment,
                true,
                br#"{"name":"private-input"}"#.as_slice(),
            )
            .unwrap();
            let path = std::path::PathBuf::from(&environment["ATTUNE_PARAMETER_FILE"]);
            let bytes = std::fs::read_to_string(&path).unwrap();
            assert!(bytes.contains("private-input"));
            assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o400);
            let mut formatted_stdin = input.formatted.as_ref().unwrap().stdin.try_clone().unwrap();
            assert_eq!(formatted_stdin.read(&mut [0; 1]).unwrap(), 0);
            assert_eq!(
                input.bytes.as_deref(),
                Some(br#"{"name":"private-input"}"#.as_slice())
            );
            drop(input);
            assert!(!path.exists());
        }
    }
    #[test]
    fn ordinary_json_stdin_is_left_unread_and_formatter_runs_only_when_declared() {
        struct Unread;
        impl Read for Unread {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("ordinary stdin was consumed");
            }
        }
        let mut environment = BTreeMap::from([
            ("ATTUNE_PARAMETER_DELIVERY".into(), "stdin".into()),
            ("ATTUNE_PARAMETER_FORMAT".into(), "json".into()),
        ]);
        let untouched = prepare_input(&mut environment, false, Unread).unwrap();
        assert!(untouched.bytes.is_none() && untouched.formatted.is_none());
        for format in ["yaml", "dotenv"] {
            environment.insert("ATTUNE_PARAMETER_FORMAT".into(), format.into());
            let input = prepare_input(
                &mut environment,
                false,
                br#"{"name":"private-input"}"#.as_slice(),
            )
            .unwrap();
            let mut bytes = String::new();
            input
                .formatted
                .as_ref()
                .unwrap()
                .stdin
                .try_clone()
                .unwrap()
                .read_to_string(&mut bytes)
                .unwrap();
            assert!(bytes.contains("private-input"));
            assert!(!environment.contains_key("ATTUNE_PARAMETER_FILE"));
        }
    }
}
