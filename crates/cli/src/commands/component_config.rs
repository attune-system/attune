//! Shared pack/sensor field override commands. Never print submitted or returned values.
use std::{collections::BTreeSet, io::Read};

use anyhow::{anyhow, bail, Result};
use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};

use crate::{
    client::{ApiClient, ApiRequestError},
    config::CliConfig,
    output::{self, OutputFormat},
};

#[derive(Clone, Copy)]
pub enum ConfigurationOwner {
    Pack,
    Sensor,
}

impl ConfigurationOwner {
    fn path(self, reference: &str) -> Result<String> {
        let valid = match self {
            Self::Pack => attune_common::schema::RefValidator::validate_pack_ref(reference),
            Self::Sensor => attune_common::schema::RefValidator::validate_component_ref(reference),
        };
        valid.map_err(|_| anyhow!("Invalid configuration owner reference"))?;
        Ok(format!(
            "/{}/{}",
            match self {
                Self::Pack => "packs",
                Self::Sensor => "sensors",
            },
            reference
        ))
    }
}

#[derive(Subcommand)]
pub enum ConfigurationCommands {
    /// Persist supplied whole-field overrides; omitted fields remain unchanged
    #[command(
        after_help = "Examples:\n  attune pack config set example --set-string endpoint=https://example.test --set-json retries=3\n  attune sensor config set example.reader --null optional_value\n  cat overrides.json | attune pack config set example --file - --dry-run\n\nObjects/lists replace a complete field. --null sets a value; --reset removes an override. Prefer stdin/files for secrets. Dry-run prints field names only and does not check server authorization or schema compatibility."
    )]
    Set {
        reference: String,
        #[command(flatten)]
        values: OverrideArgs,
        /// Preview field names only, without contacting the API
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove overrides and use current release defaults, without fetching plaintext
    #[command(
        after_help = "Examples:\n  attune pack config reset example retry_policy endpoint --dry-run\n  attune pack config reset example retry_policy --yes\n  attune sensor config reset example.reader interval --yes\n\nFields are literal top-level names, not JSON Pointers. Declared/default fields can be reset repeatedly; unknown fields are rejected. A required field without a default can still fail server validation. Capability selectors require deployment readmission."
    )]
    Reset {
        reference: String,
        #[arg(required = true, num_args = 1..)]
        fields: Vec<String>,
        #[arg(long)]
        dry_run: bool,
        /// Confirm reset without an interactive prompt
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args, Default)]
pub struct OverrideArgs {
    /// Replace a field with a literal string, FIELD=VALUE; repeatable
    #[arg(long = "set-string", value_name = "FIELD=VALUE")]
    strings: Vec<String>,
    /// Replace a field with a JSON value, FIELD=JSON; repeatable
    #[arg(long = "set-json", value_name = "FIELD=JSON")]
    json_values: Vec<String>,
    /// Set a field to explicit null; repeatable, never a reset
    #[arg(long = "null", value_name = "FIELD")]
    null_fields: Vec<String>,
    /// JSON object of whole-field overrides; '-' reads bounded stdin
    #[arg(long, value_name = "PATH")]
    file: Option<String>,
    /// Remove a different field's override in the same atomic request; repeatable
    #[arg(long = "reset", value_name = "FIELD")]
    reset_fields: Vec<String>,
}

pub(super) fn read_json(file: &str) -> Result<Value> {
    let mut reader: Box<dyn Read> = if file == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(std::fs::File::open(file).map_err(|_| anyhow!("Cannot open input file"))?)
    };
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("Cannot read input file"))?;
    if bytes.len() > 1_048_576 {
        bail!("Input exceeds 1 MiB");
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("Input must be valid JSON; values omitted from diagnostics"))
}

fn field_name(field: &str) -> Result<()> {
    if field.is_empty() || field.len() > 255 || field.chars().any(char::is_control) {
        bail!("Configuration fields must be nonempty names of at most 255 bytes without control characters");
    }
    Ok(())
}

fn insert(values: &mut Map<String, Value>, field: &str, value: Value) -> Result<()> {
    field_name(field)?;
    if values.contains_key(field) {
        bail!("A configuration field was supplied more than once");
    }
    values.insert(field.into(), value);
    Ok(())
}

fn assignment(argument: &str) -> Result<(&str, &str)> {
    argument
        .split_once('=')
        .ok_or_else(|| anyhow!("Override requires FIELD=VALUE; submitted values omitted"))
}

fn patch(args: &OverrideArgs, file: Option<Value>) -> Result<Value> {
    let mut values = match file {
        Some(Value::Object(values)) => values,
        Some(_) => bail!("Override file must contain an object; root null is not a reset"),
        None => Map::new(),
    };
    for field in values.keys() {
        field_name(field)?;
    }
    for argument in &args.strings {
        let (field, value) = assignment(argument)?;
        insert(&mut values, field, Value::String(value.into()))?;
    }
    for argument in &args.json_values {
        let (field, value) = assignment(argument)?;
        let value = serde_json::from_str(value)
            .map_err(|_| anyhow!("Invalid JSON override; submitted values omitted"))?;
        insert(&mut values, field, value)?;
    }
    for field in &args.null_fields {
        insert(&mut values, field, Value::Null)?;
    }
    let reset = reset_fields(&args.reset_fields)?;
    if reset.iter().any(|field| values.contains_key(field)) {
        bail!("A configuration field cannot be set and reset together");
    }
    if values.is_empty() && reset.is_empty() {
        bail!("No changes supplied. Use --set-string, --set-json, --null, --file or --reset");
    }
    let mut body = Map::new();
    if !values.is_empty() {
        body.insert("config".into(), Value::Object(values));
    }
    if !reset.is_empty() {
        body.insert("config_reset_fields".into(), json!(reset));
    }
    Ok(Value::Object(body))
}

fn reset_fields(fields: &[String]) -> Result<Vec<String>> {
    for field in fields {
        field_name(field)?;
    }
    Ok(fields
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

fn summary(endpoint: &str, body: &Value, dry_run: bool) -> Value {
    json!({"path":endpoint, "dry_run":dry_run, "server_validation_checked":!dry_run,
        "set_fields":body.get("config").and_then(Value::as_object).map(|values| values.keys().collect::<Vec<_>>()).unwrap_or_default(),
        "reset_fields":body.get("config_reset_fields").cloned().unwrap_or_else(|| json!([]))})
}

pub(super) fn safe_request_error(error: anyhow::Error) -> anyhow::Error {
    if let Some(error) = error.downcast_ref::<ApiRequestError>() {
        anyhow!("API operation failed with HTTP {}; values omitted. Check authorization, schema compatibility and readmission requirements before retrying", error.status.as_u16())
    } else {
        anyhow!("API operation did not complete; values omitted. Check API availability and current state before retrying")
    }
}

pub async fn handle(
    profile: &Option<String>,
    api_url: &Option<String>,
    owner: ConfigurationOwner,
    command: ConfigurationCommands,
    format: OutputFormat,
) -> Result<()> {
    let (reference, body, dry_run) = match command {
        ConfigurationCommands::Set {
            reference,
            values,
            dry_run,
        } => {
            let file = values.file.as_deref().map(read_json).transpose()?;
            (reference, patch(&values, file)?, dry_run)
        }
        ConfigurationCommands::Reset {
            reference,
            fields,
            dry_run,
            yes,
        } => {
            if !dry_run && !yes {
                bail!("Reset requires --yes; inspect field names first with --dry-run");
            }
            if fields.is_empty() {
                bail!("At least one reset field is required");
            }
            (
                reference,
                json!({"config_reset_fields":reset_fields(&fields)?}),
                dry_run,
            )
        }
    };
    let endpoint = owner.path(&reference)?;
    if !dry_run {
        let config = CliConfig::load_with_profile(profile.as_deref())?;
        let mut client = ApiClient::from_config(&config, api_url);
        let _: Value = client
            .put(&endpoint, &body)
            .await
            .map_err(safe_request_error)?;
    }
    output::print_output(&summary(&endpoint, &body, dry_run), format)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: ConfigurationCommands,
    }

    #[test]
    fn omitted_null_reset_and_whole_values_are_distinct() {
        let args = OverrideArgs {
            json_values: vec!["object={\"only\":1}".into(), "list=[2]".into()],
            null_fields: vec!["nullable".into()],
            reset_fields: vec!["old".into()],
            ..Default::default()
        };
        assert_eq!(
            patch(&args, None).unwrap(),
            json!({"config":{"object":{"only":1},"list":[2],"nullable":null},"config_reset_fields":["old"]})
        );
        let body = patch(
            &OverrideArgs {
                reset_fields: vec!["old".into()],
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert!(body.get("config").is_none());
        assert!(patch(&OverrideArgs::default(), Some(Value::Null)).is_err());
    }

    #[test]
    fn duplicates_conflicts_and_errors_never_disclose_values() {
        let args = OverrideArgs {
            strings: vec!["token=private-cli-fixture".into()],
            null_fields: vec!["token".into()],
            ..Default::default()
        };
        assert!(!patch(&args, None)
            .unwrap_err()
            .to_string()
            .contains("private-cli-fixture"));
        let args = OverrideArgs {
            json_values: vec!["token=private-cli-fixture".into()],
            ..Default::default()
        };
        assert!(!patch(&args, None)
            .unwrap_err()
            .to_string()
            .contains("private-cli-fixture"));
        let args = OverrideArgs {
            null_fields: vec!["token".into()],
            reset_fields: vec!["token".into()],
            ..Default::default()
        };
        assert!(patch(&args, None).is_err());
        let body = json!({"config":{"token":"private-cli-fixture"}});
        assert!(!summary("/packs/example", &body, true)
            .to_string()
            .contains("private-cli-fixture"));
    }

    #[test]
    fn reset_needs_no_json_and_help_has_examples() {
        assert!(TestCli::try_parse_from(["config", "reset", "example", "token", "--yes"]).is_ok());
        assert!(TestCli::try_parse_from(["config", "reset", "example"]).is_err());
        let help = TestCli::try_parse_from(["config", "reset", "--help"])
            .err()
            .unwrap()
            .to_string();
        assert!(help.contains("Examples:"));
        assert!(help.contains("--dry-run"));
    }

    #[test]
    fn pack_and_sensor_paths_share_the_same_value_contract() {
        assert_eq!(
            ConfigurationOwner::Pack.path("example").unwrap(),
            "/packs/example"
        );
        assert_eq!(
            ConfigurationOwner::Sensor.path("example.reader").unwrap(),
            "/sensors/example.reader"
        );
        assert!(ConfigurationOwner::Pack.path("../foreign").is_err());
        assert!(ConfigurationOwner::Sensor.path("../foreign").is_err());
        let args = OverrideArgs {
            strings: vec!["public=changed".into()],
            ..Default::default()
        };
        assert_eq!(
            patch(&args, Some(json!({"secret":null}))).unwrap(),
            json!({"config":{"secret":null,"public":"changed"}})
        );
    }

    #[test]
    fn server_errors_cannot_echo_submitted_secrets() {
        let error = ApiRequestError::from_response(
            reqwest::StatusCode::BAD_REQUEST,
            None,
            r#"{"error":"invalid private-cli-fixture","details":{"value":"private-cli-fixture"}}"#,
        );
        let sanitized = safe_request_error(error.into()).to_string();
        assert!(sanitized.contains("400"));
        assert!(!sanitized.contains("private-cli-fixture"));
    }
}
