//! Materialize declared action input only inside the attempt's private guest.
//! CP transport remains the exact admitted JSON; no manager path is exposed.
use attune_common::{models::ParameterFormat, workload_launch::LaunchInvocation};
use std::{
    collections::BTreeMap,
    fs::{File, Permissions},
    io::{self, Seek, SeekFrom, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
};
use tempfile::NamedTempFile;

pub(super) struct PreparedInput {
    pub stdin: File,
    // Retain the named file until the payload has stopped, including on a
    // normal nonzero exit. A killed gate leaves bytes only in private scratch.
    _parameter_file: Option<NamedTempFile>,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid declared action input")
}

pub(super) fn required(environment: &BTreeMap<String, String>) -> io::Result<bool> {
    match environment
        .get("ATTUNE_PARAMETER_DELIVERY")
        .map(String::as_str)
    {
        None | Some("none") => Ok(false),
        Some("stdin") => Ok(format(environment)? != ParameterFormat::Json),
        Some("file") => {
            format(environment)?;
            Ok(true)
        }
        _ => Err(invalid()),
    }
}

pub(super) fn required_at_gate(invocation: &LaunchInvocation) -> io::Result<bool> {
    use attune_common::workload_launch::RUNNER_ROOT;
    let private_preparation = ["linux-amd64", "linux-arm64"]
        .iter()
        .any(|file| invocation.argv.first() == Some(&format!("{RUNNER_ROOT}/{file}")))
        && matches!(
            invocation.argv.get(1).map(String::as_str),
            Some(
                "run-runtime-recipe"
                    | "run-candidate-runtime-recipe"
                    | "run-candidate-python"
                    | "run-inline-action"
            )
        );
    Ok(required(&invocation.env)? && !private_preparation)
}

fn format(environment: &BTreeMap<String, String>) -> io::Result<ParameterFormat> {
    match environment
        .get("ATTUNE_PARAMETER_FORMAT")
        .map(String::as_str)
    {
        Some("json") => Ok(ParameterFormat::Json),
        Some("yaml") => Ok(ParameterFormat::Yaml),
        Some("dotenv") => Ok(ParameterFormat::Dotenv),
        _ => Err(invalid()),
    }
}

pub(super) fn prepare(
    environment: &mut BTreeMap<String, String>,
    bytes: &[u8],
) -> io::Result<PreparedInput> {
    prepare_in(environment, bytes, Path::new("/tmp"))
}

fn prepare_in(
    environment: &mut BTreeMap<String, String>,
    bytes: &[u8],
    private_scratch: &Path,
) -> io::Result<PreparedInput> {
    if !required(environment)?
        || bytes.len() > attune_common::workload_stdin::MAX_PRIVATE_STDIN_BYTES
    {
        return Err(invalid());
    }
    let parameters = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let formatted =
        attune_common::action_input::format_parameters(&parameters, format(environment)?)
            .map_err(|_| invalid())?;
    // A derived bound for escaping/quoting the already bounded input; this is
    // not another public input ceiling or an unbounded output allocation.
    if formatted.len() > attune_common::workload_stdin::MAX_PRIVATE_STDIN_BYTES * 6 + 1024 {
        return Err(invalid());
    }
    if environment["ATTUNE_PARAMETER_DELIVERY"] == "file" {
        let mut file = NamedTempFile::new_in(private_scratch)?;
        file.as_file()
            .set_permissions(Permissions::from_mode(0o400))?;
        file.write_all(formatted.as_bytes())?;
        file.flush()?;
        environment.insert(
            "ATTUNE_PARAMETER_FILE".into(),
            file.path().to_str().ok_or_else(invalid)?.to_owned(),
        );
        Ok(PreparedInput {
            stdin: File::open("/dev/null")?,
            _parameter_file: Some(file),
        })
    } else {
        let mut file = tempfile::tempfile_in(private_scratch)?;
        file.write_all(formatted.as_bytes())?;
        file.flush()?;
        file.set_permissions(Permissions::from_mode(0o400))?;
        file.seek(SeekFrom::Start(0))?;
        Ok(PreparedInput {
            stdin: file,
            _parameter_file: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::workload_launch::LaunchRoot;
    use std::io::Read;

    fn invocation(delivery: &str, format: &str) -> LaunchInvocation {
        LaunchInvocation {
            argv: vec!["/bin/selected".into()],
            working_root: LaunchRoot::Pack,
            env: BTreeMap::from([
                ("ATTUNE_PARAMETER_DELIVERY".into(), delivery.into()),
                ("ATTUNE_PARAMETER_FORMAT".into(), format.into()),
            ]),
        }
    }

    #[test]
    fn parameter_file_is_private_retained_and_removed_after_child_barrier() {
        let root = tempfile::tempdir().unwrap();
        let mut invocation = invocation("file", "json");
        let mut prepared = prepare_in(
            &mut invocation.env,
            br#"{"value":"private-input"}"#,
            root.path(),
        )
        .unwrap();
        let path = Path::new(&invocation.env["ATTUNE_PARAMETER_FILE"]);
        assert!(path.starts_with(root.path()));
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o400);
        let value: serde_json::Value = serde_json::from_reader(File::open(path).unwrap()).unwrap();
        assert_eq!(value["value"], "private-input");
        assert_eq!(prepared.stdin.read(&mut [0; 1]).unwrap(), 0);
        drop(prepared);
        assert!(!path.exists());
        root.close().unwrap();
    }

    #[test]
    fn formatted_stdin_reuses_existing_encodings_without_exposing_a_path() {
        let root = tempfile::tempdir().unwrap();
        for format in ["yaml", "dotenv"] {
            let mut invocation = invocation("stdin", format);
            let mut prepared =
                prepare_in(&mut invocation.env, br#"{"name":"value"}"#, root.path()).unwrap();
            let mut actual = String::new();
            prepared.stdin.read_to_string(&mut actual).unwrap();
            if format == "yaml" {
                let value: serde_json::Value = serde_yaml_ng::from_str(&actual).unwrap();
                assert_eq!(value["name"], "value");
            } else {
                assert_eq!(actual, "name='value'");
            }
            assert!(!invocation.env.contains_key("ATTUNE_PARAMETER_FILE"));
        }
        root.close().unwrap();
    }

    #[test]
    fn ordinary_json_and_non_action_stdin_are_not_consumed() {
        let ordinary = invocation("stdin", "json");
        assert!(!required(&ordinary.env).unwrap());
        let mut setup = ordinary.clone();
        setup.env.clear();
        assert!(!required(&setup.env).unwrap());
        let none = invocation("none", "json");
        assert!(!required(&none.env).unwrap());
    }

    #[test]
    fn fixed_helper_materialization_is_deferred_until_after_preparation() {
        let mut invocation = invocation("file", "json");
        assert!(required_at_gate(&invocation).unwrap());
        for mode in [
            "run-runtime-recipe",
            "run-candidate-runtime-recipe",
            "run-candidate-python",
            "run-inline-action",
        ] {
            invocation.argv = vec![
                format!(
                    "{}/linux-amd64",
                    attune_common::workload_launch::RUNNER_ROOT
                ),
                mode.into(),
            ];
            assert!(!required_at_gate(&invocation).unwrap());
            assert!(required(&invocation.env).unwrap());
        }
        invocation.argv[0] = "/bin/ordinary".into();
        assert!(required_at_gate(&invocation).unwrap());
    }

    #[test]
    fn invalid_input_or_declared_format_never_creates_a_parameter_file() {
        let root = tempfile::tempdir().unwrap();
        for (format, bytes) in [
            ("yaml", b"[]".as_slice()),
            ("dotenv", br#"{"x":"private\ncontrol"}"#.as_slice()),
            ("unknown", br#"{"x":"private"}"#.as_slice()),
        ] {
            let mut invocation = invocation("file", format);
            let error = prepare_in(&mut invocation.env, bytes, root.path())
                .err()
                .unwrap();
            assert!(!error.to_string().contains("private"));
            assert_eq!(root.path().read_dir().unwrap().count(), 0);
        }
        root.close().unwrap();
    }
}
