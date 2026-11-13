pub const RESERVED_EXECUTION_ENV_PREFIX: &str = "ATTUNE_";

pub fn is_reserved_execution_env_var(name: &str) -> bool {
    name.starts_with(RESERVED_EXECUTION_ENV_PREFIX)
}

pub fn validate_execution_env_var(name: &str, value: &str) -> crate::Result<()> {
    if is_reserved_execution_env_var(name) {
        return Err(crate::Error::Validation(format!(
            "Environment variable '{name}' uses the reserved ATTUNE_ prefix; these variables are for internal use"
        )));
    }
    validate_caller_env_name(name)?;
    if value.contains('\0') {
        return Err(crate::Error::Validation(format!(
            "Environment variable '{name}' cannot contain a NUL value"
        )));
    }
    Ok(())
}

/// Admission metadata alone must not turn a native runtime into a shell wrapper.
/// Preserve the existing treatment of all actual/unknown process configuration.
pub fn has_runtime_process_configuration(config: &serde_json::Value) -> bool {
    config
        .as_object()
        .map(|config| config.keys().any(|key| key != "permitted_caller_env"))
        .unwrap_or(true)
}

/// Caller values may never change runtime startup, code loading or worker context.
/// Case folding also protects Windows' case-insensitive environment namespace.
pub fn validate_caller_env_name(name: &str) -> crate::Result<()> {
    let upper = name.to_ascii_uppercase();
    let valid = !name.is_empty()
        && name
            .bytes()
            .enumerate()
            .all(|(i, c)| c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()));
    let forbidden = [
        "ATTUNE_", "LD_", "DYLD_", "PYTHON", "NODE_", "BASH", "PERL", "RUBY", "GEM_", "BUNDLE_",
        "JAVA_", "JDK_", "JRE_", "DOTNET_", "COMPLUS_", "GCONV_", "LC_", "LDR_", "_RLD_",
        "MALLOC_", "R_", "CGO_",
    ]
    .iter()
    .any(|prefix| upper.starts_with(prefix))
        || matches!(
            upper.as_str(),
            "GOENV"
                | "GOCACHEPROG"
                | "GOFLAGS"
                | "GOROOT"
                | "GOPATH"
                | "GOTOOLCHAIN"
                | "GOWORK"
                | "GOMODCACHE"
                | "GOCACHE"
                | "GOTMPDIR"
                | "GODEBUG"
                | "CC"
                | "CXX"
                | "AR"
                | "PKG_CONFIG"
                | "PKG_CONFIG_PATH"
                | "COMPILER_PATH"
                | "GCC_EXEC_PREFIX"
                | "LIBRARY_PATH"
                | "CPATH"
                | "C_INCLUDE_PATH"
                | "CPLUS_INCLUDE_PATH"
                | "GLIBC_TUNABLES"
                | "LIBPATH"
                | "SHLIB_PATH"
                | "PS0"
                | "PS1"
                | "PS2"
                | "PS3"
                | "PS4"
                | "PROMPT_COMMAND"
                | "INPUTRC"
                | "HISTFILE"
                | "PATH"
                | "PATHEXT"
                | "HOME"
                | "USERPROFILE"
                | "ENV"
                | "SHELLOPTS"
                | "BASHOPTS"
                | "IFS"
                | "CDPATH"
                | "FPATH"
                | "ZDOTDIR"
                | "SHELL"
                | "COMSPEC"
                | "SYSTEMROOT"
                | "SYSTEMDRIVE"
                | "WINDIR"
                | "LANG"
                | "LANGUAGE"
                | "LOCPATH"
                | "NLSPATH"
                | "TZ"
                | "TZDIR"
                | "TMPDIR"
                | "TMP"
                | "TEMP"
                | "CLASSPATH"
                | "JDK_JAVA_OPTIONS"
                | "_JAVA_OPTIONS"
                | "OPENSSL_CONF"
                | "OPENSSL_ENGINES"
                | "OPENSSL_MODULES"
                | "SSLKEYLOGFILE"
        );
    if !valid || forbidden {
        return Err(crate::Error::Validation(format!(
            "Environment variable '{name}' is not permitted for callers"
        )));
    }
    Ok(())
}

/// Exact names, no wildcards or duplicates; omission denies all caller overrides.
pub fn permitted_caller_env(config: &serde_json::Value) -> crate::Result<Vec<String>> {
    let Some(value) = config.get("permitted_caller_env") else {
        return Ok(vec![]);
    };
    let names: Vec<String> = serde_json::from_value(value.clone()).map_err(|_| {
        crate::Error::Validation("permitted_caller_env must be an array of names".into())
    })?;
    let mut seen = std::collections::HashSet::new();
    for name in &names {
        validate_caller_env_name(name)?;
        if !seen.insert(name.to_ascii_uppercase()) {
            return Err(crate::Error::Validation(
                "permitted_caller_env contains duplicate names".into(),
            ));
        }
    }
    Ok(names)
}

pub fn validate_caller_environment(
    config: &serde_json::Value,
    vars: &serde_json::Value,
) -> crate::Result<()> {
    let permitted = permitted_caller_env(config)?;
    let vars: std::collections::BTreeMap<String, String> = serde_json::from_value(vars.clone())
        .map_err(|_| {
            crate::Error::Validation(
                "Execution env_vars must be an object with string values".into(),
            )
        })?;
    for (name, value) in vars {
        validate_execution_env_var(&name, &value)?;
        if !permitted.contains(&name) {
            return Err(crate::Error::Validation(format!(
                "Environment variable '{name}' is not declared in permitted_caller_env"
            )));
        }
    }
    Ok(())
}

/// Admission precedes worker placement: require the captured parent and every
/// eligible captured version to permit a value, so placement cannot widen it.
pub fn validate_snapshot_environment(
    snapshot: Option<&crate::models::ExecutionExecutableSnapshot>,
    vars: Option<&serde_json::Value>,
) -> crate::Result<()> {
    let Some(vars) = vars else {
        return Ok(());
    };
    let empty = serde_json::json!({});
    let Some(snapshot) = snapshot else {
        return validate_caller_environment(&empty, vars);
    };
    validate_caller_environment(
        snapshot
            .executable
            .runtime
            .as_ref()
            .map(|r| &r.execution_config)
            .unwrap_or(&empty),
        vars,
    )?;
    for version in &snapshot.executable.runtime_versions {
        let eligible = snapshot
            .executable
            .action
            .runtime_version_constraint
            .as_deref()
            .map(|constraint| {
                crate::version_matching::matches_constraint(&version.version, constraint)
                    .unwrap_or(false)
            })
            .unwrap_or(true);
        if eligible && version.available {
            validate_caller_environment(&version.execution_config, vars)?;
        }
    }
    Ok(())
}

/// Worker check after placement, using only the admitted executable selection.
pub fn validate_selected_environment(
    snapshot: Option<&crate::models::ExecutionExecutableSnapshot>,
    selected_version: Option<&str>,
    vars: Option<&serde_json::Value>,
) -> crate::Result<()> {
    let Some(vars) = vars else {
        return Ok(());
    };
    let config = match selected_version {
        Some(version) => Some(
            snapshot
                .and_then(|snapshot| {
                    snapshot
                        .executable
                        .runtime_versions
                        .iter()
                        .find(|candidate| candidate.version == version)
                })
                .map(|candidate| &candidate.execution_config)
                .ok_or_else(|| {
                    crate::Error::Validation(
                        "Selected runtime version is absent from executable snapshot".into(),
                    )
                })?,
        ),
        None => snapshot
            .and_then(|snapshot| snapshot.executable.runtime.as_ref())
            .map(|runtime| &runtime.execution_config),
    };
    validate_caller_environment(config.unwrap_or(&serde_json::json!({})), vars)
}

/// Inspect metadata locations only, never action inputs, schemas, or payload data.
pub fn component_environment_errors(component: &str, value: &serde_json::Value) -> Vec<String> {
    let mut errors = Vec::new();
    if component == "runtimes" {
        inspect_runtime_config(
            value.get("execution_config"),
            "execution_config",
            &mut errors,
        );
        if let Some(versions) = value.get("versions").and_then(serde_json::Value::as_array) {
            for (index, version) in versions.iter().enumerate() {
                inspect_runtime_config(
                    version.get("execution_config"),
                    &format!("versions[{index}].execution_config"),
                    &mut errors,
                );
            }
        }
    } else if matches!(component, "actions" | "rules" | "queues" | "workflows") {
        inspect_unsupported_environment(value, "", &mut errors);
        if component == "workflows" {
            inspect_tasks(value.get("tasks"), "tasks", &mut errors);
        }
    }
    errors
}

pub fn runtime_environment_errors(config: &serde_json::Value) -> Vec<String> {
    let mut errors = Vec::new();
    inspect_runtime_config(Some(config), "execution_config", &mut errors);
    errors
}

pub fn validate_runtime_environment(config: &serde_json::Value) -> crate::Result<()> {
    let errors = runtime_environment_errors(config);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(crate::Error::Validation(errors.join("; ")))
    }
}

fn inspect_runtime_config(
    config: Option<&serde_json::Value>,
    path: &str,
    errors: &mut Vec<String>,
) {
    if let Some(config) = config {
        if let Err(error) = permitted_caller_env(config) {
            errors.push(format!("{path}: {error}"));
        }
    }
    if let Some(vars) = config.and_then(|config| config.get("env_vars")) {
        inspect_environment_keys(vars, &format!("{path}.env_vars"), errors);
    }
}

fn inspect_environment_keys(vars: &serde_json::Value, path: &str, errors: &mut Vec<String>) {
    if vars.is_null() {
        return;
    }
    let Some(map) = vars.as_object() else {
        errors.push(format!("{path} must be an object of environment variables"));
        return;
    };
    let mut keys = map.keys().collect::<Vec<_>>();
    keys.sort();
    for key in keys {
        if is_reserved_execution_env_var(key) {
            errors.push(format!("{path}[{key:?}] uses the reserved ATTUNE_ prefix; these variables are for internal use"));
        }
    }
}

fn inspect_unsupported_environment(
    value: &serde_json::Value,
    path: &str,
    errors: &mut Vec<String>,
) {
    if let Some(vars) = value.get("env_vars") {
        let field = if path.is_empty() {
            "env_vars".to_string()
        } else {
            format!("{path}.env_vars")
        };
        inspect_environment_keys(vars, &field, errors);
        errors.push(format!("{field} is not supported in this component; configure the execution request or runtime environment instead"));
    }
}

fn inspect_tasks(tasks: Option<&serde_json::Value>, path: &str, errors: &mut Vec<String>) {
    if let Some(tasks) = tasks.and_then(serde_json::Value::as_array) {
        for (index, task) in tasks.iter().enumerate() {
            let task_path = format!("{path}[{index}]");
            inspect_unsupported_environment(task, &task_path, errors);
            inspect_tasks(task.get("tasks"), &format!("{task_path}.tasks"), errors);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_policy_metadata_does_not_reclassify_native_runtimes() {
        assert!(!has_runtime_process_configuration(&serde_json::json!({})));
        assert!(!has_runtime_process_configuration(
            &serde_json::json!({"permitted_caller_env":["APP_REGION"]})
        ));
        assert!(has_runtime_process_configuration(
            &serde_json::json!({"permitted_caller_env":["APP_REGION"], "interpreter":{"binary":"python3"}})
        ));
        assert!(has_runtime_process_configuration(
            &serde_json::json!({"env_vars":{"PYTHONPATH":"operator/lib"}})
        ));
    }

    #[test]
    fn pack_preflight_validates_caller_policy_in_parent_and_version_metadata() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("pack.yaml"), "ref: demo\nversion: 1.0.0\n").unwrap();
        std::fs::create_dir(root.path().join("runtimes")).unwrap();
        let runtime = root.path().join("runtimes/app.yaml");
        for declaration in [
            "null",
            "APP_REGION",
            "[APP_REGION, APP_REGION]",
            "[BASH_ENV]",
        ] {
            std::fs::write(&runtime, format!("ref: demo.app\nname: App\nexecution_config:\n  permitted_caller_env: {declaration}\n")).unwrap();
            assert!(crate::pack_check::validate_pack_environment(root.path()).is_err());
        }
        std::fs::write(&runtime, "ref: demo.app\nname: App\nexecution_config:\n  permitted_caller_env: [APP_REGION]\nversions:\n  - version: '1.0.0'\n    execution_config:\n      permitted_caller_env: [NODE_OPTIONS]\n").unwrap();
        let error = crate::pack_check::validate_pack_environment(root.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("versions[0].execution_config"));
        std::fs::write(&runtime, "ref: demo.app\nname: App\nexecution_config:\n  permitted_caller_env: [APP_REGION]\n  env_vars:\n    PYTHONPATH: operator/lib\nversions:\n  - version: '1.0.0'\n    execution_config:\n      permitted_caller_env: [APP_REGION]\n").unwrap();
        assert!(crate::pack_check::validate_pack_environment(root.path()).is_ok());
    }

    #[test]
    fn caller_declarations_fail_closed() {
        for value in [
            serde_json::json!(null),
            serde_json::json!("APP"),
            serde_json::json!([1]),
            serde_json::json!(["APP", "APP"]),
            serde_json::json!(["APP", "app"]),
            serde_json::json!(["*"]),
        ] {
            assert!(
                permitted_caller_env(&serde_json::json!({"permitted_caller_env":value})).is_err()
            );
        }
        for name in [
            "BASH_ENV",
            "ENV",
            "PYTHONPATH",
            "PYTHONSTARTUP",
            "GOFLAGS",
            "GOCACHEPROG",
            "R_PROFILE_USER",
            "CGO_CFLAGS",
            "COMPILER_PATH",
            "NODE_OPTIONS",
            "NODE_PATH",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "DYLD_INSERT_LIBRARIES",
            "GOENV",
            "GOFLAGS",
            "GOROOT",
            "GOPATH",
            "GOTOOLCHAIN",
            "GOWORK",
            "GOMODCACHE",
            "GOCACHE",
            "GOTMPDIR",
            "GODEBUG",
            "CC",
            "CXX",
            "AR",
            "PKG_CONFIG",
            "PKG_CONFIG_PATH",
            "COMPILER_PATH",
            "GCC_EXEC_PREFIX",
            "LIBRARY_PATH",
            "CPATH",
            "C_INCLUDE_PATH",
            "CPLUS_INCLUDE_PATH",
            "GLIBC_TUNABLES",
            "LIBPATH",
            "SHLIB_PATH",
            "PS0",
            "PS1",
            "PS2",
            "PS3",
            "PS4",
            "PROMPT_COMMAND",
            "INPUTRC",
            "HISTFILE",
            "PATH",
            "HOME",
            "ATTUNE_API_TOKEN",
            "attune_api_token",
            "BASH_FUNC_probe%%",
        ] {
            let config = serde_json::json!({"permitted_caller_env":[name]});
            assert!(permitted_caller_env(&config).is_err(), "{name}");
            assert!(
                validate_caller_environment(&config, &serde_json::json!({name:"probe"})).is_err()
            );
        }
        let error = validate_caller_environment(
            &serde_json::json!({"permitted_caller_env":["GOCACHEPROG"]}),
            &serde_json::json!({"GOCACHEPROG":"owned-helper-value-not-for-diagnostics"}),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("GOCACHEPROG"));
        assert!(!error.contains("owned-helper-value-not-for-diagnostics"));
        let vars = serde_json::json!({"APP_REGION":"west", "EMPTY":""});
        assert!(validate_caller_environment(&serde_json::json!({}), &vars).is_err());
        assert!(validate_caller_environment(
            &serde_json::json!({"permitted_caller_env":["APP_REGION", "EMPTY"]}),
            &vars
        )
        .is_ok());
        assert!(validate_runtime_environment(
            &serde_json::json!({"env_vars":{"PYTHONPATH":"operator/lib", "PATH":"operator/bin"}})
        )
        .is_ok());
        assert!(!component_environment_errors("runtimes", &serde_json::json!({"versions":[{"execution_config":{"permitted_caller_env":["PATH"]}}]})).is_empty());
    }

    #[test]
    fn attune_prefix_is_reserved() {
        assert!(is_reserved_execution_env_var("ATTUNE_API_TOKEN"));
        assert!(!is_reserved_execution_env_var("LOG_LEVEL"));
        assert!(!is_reserved_execution_env_var("attune_api_token"));
    }

    #[test]
    fn metadata_validation_checks_versions_and_nested_tasks_without_inspecting_inputs() {
        let runtime = serde_json::json!({
            "execution_config": {"env_vars": {"ATTUNE_API_URL": "hidden"}},
            "versions": [{"execution_config": {"env_vars": {"ATTUNE_EXEC_ID": {"value": "hidden"}}}}]
        });
        let errors = component_environment_errors("runtimes", &runtime);
        assert_eq!(errors.len(), 2);
        assert!(errors[1].contains("versions[0].execution_config.env_vars"));
        assert!(errors.iter().all(|error| !error.contains("hidden")));

        let workflow = serde_json::json!({"tasks": [{"input": {"env_vars": {"ATTUNE_API_URL": "data"}}, "tasks": [{"env_vars": {"ATTUNE_API_URL": "hidden"}}]}]});
        let errors = component_environment_errors("workflows", &workflow);
        assert_eq!(errors.len(), 2);
        assert!(errors[0].contains("tasks[0].tasks[0].env_vars"));
        assert!(errors.iter().all(|error| !error.contains("data")));
    }
}
