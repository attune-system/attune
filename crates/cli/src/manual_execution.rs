use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use attune_common::execution_env::validate_execution_env_var;
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum CliRetentionPolicy {
    Versions,
    Days,
    Hours,
    Minutes,
}

#[derive(Debug, Clone, Default, Args)]
pub struct ManualExecutionOptions {
    /// Environment variable in KEY=VALUE format. Repeat for multiple values.
    #[arg(long = "env", value_name = "KEY=VALUE", conflicts_with = "env_json")]
    pub env: Vec<String>,

    /// Environment variables as a JSON object with string values.
    #[arg(long, value_name = "JSON", conflicts_with = "env")]
    pub env_json: Option<String>,

    /// Permission set for the execution-scoped API token. Repeat for multiple refs.
    #[arg(
        long = "permission-set",
        value_name = "REF",
        conflicts_with = "no_api_token"
    )]
    pub permission_sets: Vec<String>,

    /// Disable the execution-scoped API token, overriding action defaults.
    #[arg(long, conflicts_with = "permission_sets")]
    pub no_api_token: bool,

    /// Retention policy for non-log artifacts created by this execution.
    #[arg(long, value_enum)]
    pub artifact_retention_policy: Option<CliRetentionPolicy>,

    /// Retention limit for non-log artifacts created by this execution.
    #[arg(long)]
    pub artifact_retention_limit: Option<i32>,

    /// Worker label selector as JSON (e.g. '{"pool":"gpu"}').
    #[arg(long)]
    pub worker_selector: Option<String>,

    /// Worker tolerations as a JSON array.
    #[arg(long)]
    pub worker_tolerations: Option<String>,

    /// Worker affinity as a JSON object.
    #[arg(long)]
    pub worker_affinity: Option<String>,

    /// Execution timeout override in seconds, snapshotted onto the execution.
    #[arg(long)]
    pub execution_timeout: Option<i32>,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedExecutionOptions {
    pub env_vars: Option<BTreeMap<String, String>>,
    pub permission_set_refs: Option<Vec<String>>,
    pub artifact_retention_policy: Option<CliRetentionPolicy>,
    pub artifact_retention_limit: Option<i32>,
    pub worker_selector: Option<Value>,
    pub worker_tolerations: Option<Value>,
    pub worker_affinity: Option<Value>,
    pub timeout_seconds: Option<i32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManualExecutionRequest {
    pub action_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_vars: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_set_refs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_retention_policy: Option<CliRetentionPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_retention_limit: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_selector: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_tolerations: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_affinity: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<i32>,
}

impl ManualExecutionRequest {
    pub fn validate(&self) -> Result<()> {
        if self.action_ref.trim().is_empty() {
            bail!("action_ref must not be empty");
        }
        if self
            .parameters
            .as_ref()
            .is_some_and(|value| !value.is_object())
        {
            bail!("parameters must be a JSON object");
        }
        if let Some(vars) = &self.env_vars {
            validate_env_vars(vars.clone())?;
        }
        if self
            .permission_set_refs
            .as_ref()
            .is_some_and(|refs| refs.iter().any(|permission| permission.is_empty()))
        {
            bail!("permission_set_refs cannot contain empty refs");
        }
        if self
            .artifact_retention_limit
            .is_some_and(|limit| limit <= 0)
        {
            bail!("artifact_retention_limit must be greater than zero");
        }
        if self.timeout_seconds.is_some_and(|timeout| timeout <= 0) {
            bail!("timeout_seconds must be greater than zero");
        }
        if let Some(selector) = &self.worker_selector {
            attune_common::scheduling::parse_worker_selector(selector)
                .context("Invalid worker_selector")?;
        }
        if let Some(tolerations) = &self.worker_tolerations {
            attune_common::scheduling::parse_worker_tolerations(tolerations)
                .context("Invalid worker_tolerations")?;
        }
        if let Some(affinity) = &self.worker_affinity {
            attune_common::scheduling::parse_worker_affinity(affinity)
                .context("Invalid worker_affinity")?;
        }
        Ok(())
    }

    pub fn new(action_ref: String, parameters: Value, options: ParsedExecutionOptions) -> Self {
        Self {
            action_ref,
            parameters: Some(parameters),
            env_vars: options.env_vars,
            permission_set_refs: options.permission_set_refs,
            artifact_retention_policy: options.artifact_retention_policy,
            artifact_retention_limit: options.artifact_retention_limit,
            worker_selector: options.worker_selector,
            worker_tolerations: options.worker_tolerations,
            worker_affinity: options.worker_affinity,
            timeout_seconds: options.timeout_seconds,
        }
    }
}

impl ManualExecutionOptions {
    pub fn parse(self) -> Result<ParsedExecutionOptions> {
        let env_vars = if let Some(json) = self.env_json {
            let vars = serde_json::from_str(&json).context("Invalid --env-json")?;
            Some(validate_env_vars(vars)?)
        } else if self.env.is_empty() {
            None
        } else {
            let mut vars = BTreeMap::new();
            for assignment in self.env {
                let (key, value) = assignment.split_once('=').ok_or_else(|| {
                    anyhow::anyhow!(
                        "Invalid environment variable '{assignment}'. Expected KEY=VALUE"
                    )
                })?;
                if key.is_empty() {
                    bail!("Environment variable name cannot be empty");
                }
                vars.insert(key.to_string(), value.to_string());
            }
            Some(validate_env_vars(vars)?)
        };

        let permission_set_refs = if self.no_api_token {
            Some(Vec::new())
        } else if self.permission_sets.is_empty() {
            None
        } else {
            if self
                .permission_sets
                .iter()
                .any(|permission| permission.is_empty())
            {
                bail!("Permission set refs cannot be empty");
            }
            Some(self.permission_sets)
        };

        if self
            .artifact_retention_limit
            .is_some_and(|limit| limit <= 0)
        {
            bail!("--artifact-retention-limit must be greater than zero");
        }
        if self.execution_timeout.is_some_and(|timeout| timeout <= 0) {
            bail!("--execution-timeout must be greater than zero");
        }

        Ok(ParsedExecutionOptions {
            env_vars,
            permission_set_refs,
            artifact_retention_policy: self.artifact_retention_policy,
            artifact_retention_limit: self.artifact_retention_limit,
            worker_selector: parse_json_option(self.worker_selector, "--worker-selector")?,
            worker_tolerations: parse_json_option(self.worker_tolerations, "--worker-tolerations")?,
            worker_affinity: parse_json_option(self.worker_affinity, "--worker-affinity")?,
            timeout_seconds: self.execution_timeout,
        })
    }
}

fn parse_json_option(value: Option<String>, flag: &str) -> Result<Option<Value>> {
    value
        .map(|json| serde_json::from_str(&json).with_context(|| format!("Invalid {flag} JSON")))
        .transpose()
}

pub fn validate_env_vars(vars: BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    for (key, value) in &vars {
        validate_execution_env_var(key, value)?;
    }
    Ok(vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omission_and_explicit_empty_permissions_remain_distinct() {
        assert!(ManualExecutionOptions::default()
            .parse()
            .unwrap()
            .permission_set_refs
            .is_none());

        let parsed = ManualExecutionOptions {
            no_api_token: true,
            ..Default::default()
        }
        .parse()
        .unwrap();
        assert_eq!(parsed.permission_set_refs, Some(Vec::new()));
    }

    #[test]
    fn reserved_environment_variables_are_rejected() {
        let error = ManualExecutionOptions {
            env: vec!["ATTUNE_API_TOKEN=bad".to_string()],
            ..Default::default()
        }
        .parse()
        .unwrap_err();
        assert!(error.to_string().contains("reserved ATTUNE_ prefix"));
    }

    #[test]
    fn environment_assignments_preserve_strings_and_split_only_the_first_equals() {
        let options = ManualExecutionOptions {
            env: vec![
                "COUNT=3".into(),
                "DEBUG=true".into(),
                "EMPTY=".into(),
                "VALUE=a=b".into(),
            ],
            ..Default::default()
        }
        .parse()
        .unwrap();
        let vars = options.env_vars.unwrap();
        assert_eq!(vars["COUNT"], "3");
        assert_eq!(vars["DEBUG"], "true");
        assert_eq!(vars["EMPTY"], "");
        assert_eq!(vars["VALUE"], "a=b");
    }

    #[test]
    fn environment_json_requires_string_values() {
        assert!(ManualExecutionOptions {
            env_json: Some(r#"{"COUNT":3}"#.into()),
            ..Default::default()
        }
        .parse()
        .is_err());
    }
}
