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
    if name.is_empty() || name.contains(['=', '\0']) {
        return Err(crate::Error::Validation(
            "Environment variable names must be non-empty and cannot contain '=' or NUL"
                .to_string(),
        ));
    }
    if value.contains('\0') {
        return Err(crate::Error::Validation(format!(
            "Environment variable '{name}' cannot contain a NUL value"
        )));
    }
    Ok(())
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
