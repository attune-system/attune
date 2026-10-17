//! Explicit current-contract preflight and acknowledgement, separate from deployment force/yes.
use anyhow::{anyhow, bail, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::component_config::{read_json, safe_request_error};
use crate::{
    client::ApiClient,
    config::CliConfig,
    output::{self, OutputFormat},
};

#[derive(Subcommand)]
pub enum RolloutCommands {
    /// Report live admitted emitters without changing the current contract
    #[command(
        after_help = "Examples:\n  attune trigger rollout preflight example.tick --out-schema payload.json --output json\n  attune trigger rollout preflight example.tick --clear-event-config-schema\n\nPreflight is advisory. Apply rechecks the live inventory under activation locks."
    )]
    Preflight {
        reference: String,
        #[command(flatten)]
        schemas: SchemaArgs,
    },
    /// Apply a trigger contract change, draining by default
    #[command(
        after_help = "Examples:\n  attune trigger rollout apply example.tick --out-schema payload.json --dry-run\n  attune trigger rollout apply example.tick --out-schema payload.json --yes\n  attune trigger rollout apply example.tick --out-schema payload.json --yes --acknowledge-incompatible-rollout\n\n--yes confirms mutation only. It NEVER acknowledges incompatibility. Without the distinct acknowledgement, live admitted emitters block activation. Schema files use Attune flat field maps. Neither schemas nor configuration values are printed."
    )]
    Apply {
        reference: String,
        #[command(flatten)]
        schemas: SchemaArgs,
        /// Run server preflight only; no mutation, even with acknowledgement selected
        #[arg(long)]
        dry_run: bool,
        /// Confirm mutation without an interactive prompt; not incompatibility acknowledgement
        #[arg(long)]
        yes: bool,
        /// Explicitly accept possible failures of live emitters against the changed contract
        #[arg(long)]
        acknowledge_incompatible_rollout: bool,
    },
}

#[derive(Args, Default)]
pub struct SchemaArgs {
    /// JSON flat payload schema file; '-' reads bounded stdin
    #[arg(long, conflicts_with = "clear_out_schema")]
    out_schema: Option<String>,
    #[arg(long)]
    clear_out_schema: bool,
    /// JSON flat event-configuration schema file; '-' reads bounded stdin
    #[arg(long, conflicts_with = "clear_event_config_schema")]
    event_config_schema: Option<String>,
    #[arg(long)]
    clear_event_config_schema: bool,
}

#[derive(Deserialize, Serialize)]
struct Impact {
    trigger_id: i64,
    execution_ids: Vec<i64>,
    workload_ids: Vec<i64>,
    requires_acknowledgement: bool,
}

fn schema_patch(file: Option<&str>, clear: bool) -> Result<Option<Value>> {
    match (file, clear) {
        (Some(_), true) => bail!("A schema cannot be set and cleared together"),
        (Some(file), false) => {
            let schema = read_json(file)?;
            if !schema.is_object() {
                bail!("Schema file must contain a flat field map");
            }
            Ok(Some(json!({"op":"set","value":schema})))
        }
        (None, true) => Ok(Some(json!({"op":"clear"}))),
        (None, false) => Ok(None),
    }
}

fn body(args: &SchemaArgs) -> Result<Value> {
    if args.out_schema.as_deref() == Some("-") && args.event_config_schema.as_deref() == Some("-") {
        bail!("Only one schema may read stdin");
    }
    let mut body = Map::new();
    for (name, patch) in [
        (
            "out_schema",
            schema_patch(args.out_schema.as_deref(), args.clear_out_schema)?,
        ),
        (
            "event_config_schema",
            schema_patch(
                args.event_config_schema.as_deref(),
                args.clear_event_config_schema,
            )?,
        ),
    ] {
        if let Some(patch) = patch {
            body.insert(name.into(), patch);
        }
    }
    if body.is_empty() {
        bail!("Supply --out-schema, --event-config-schema or an explicit --clear-*-schema flag");
    }
    Ok(Value::Object(body))
}

fn activation_body(mut body: Value, yes: bool, acknowledge: bool) -> Result<Value> {
    if !yes {
        bail!(
            "Mutation requires --yes; inspect affected emitters first with preflight or --dry-run"
        );
    }
    body.as_object_mut()
        .ok_or_else(|| anyhow!("Invalid rollout request"))?
        .insert(
            "acknowledge_incompatible_rollout".into(),
            Value::Bool(acknowledge),
        );
    Ok(body)
}

pub async fn handle(
    profile: &Option<String>,
    api_url: &Option<String>,
    command: RolloutCommands,
    format: OutputFormat,
) -> Result<()> {
    let (reference, body, preflight) = match command {
        RolloutCommands::Preflight { reference, schemas } => (reference, body(&schemas)?, true),
        RolloutCommands::Apply {
            reference,
            schemas,
            dry_run,
            yes,
            acknowledge_incompatible_rollout,
        } => {
            let body = body(&schemas)?;
            let body = if dry_run {
                body
            } else {
                activation_body(body, yes, acknowledge_incompatible_rollout)?
            };
            (reference, body, dry_run)
        }
    };
    attune_common::schema::RefValidator::validate_component_ref(&reference)
        .map_err(|_| anyhow!("Invalid rollout trigger reference"))?;
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let path = format!("/triggers/{reference}");
    if preflight {
        let impact: Impact = client
            .post(&format!("{path}/rollout-preflight"), &body)
            .await
            .map_err(safe_request_error)?;
        output::print_output(&impact, format)
    } else {
        let _: Value = client.put(&path, &body).await.map_err(safe_request_error)?;
        output::print_output(
            &json!({"path":path,"updated":true,
            "acknowledge_incompatible_rollout":body["acknowledge_incompatible_rollout"]}),
            format,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: RolloutCommands,
    }

    #[test]
    fn ordinary_confirmation_is_not_incompatible_rollout_acknowledgement() {
        let patch = json!({"out_schema":{"op":"clear"}});
        assert_eq!(
            activation_body(patch.clone(), true, false).unwrap()
                ["acknowledge_incompatible_rollout"],
            false
        );
        assert_eq!(
            activation_body(patch.clone(), true, true).unwrap()["acknowledge_incompatible_rollout"],
            true
        );
        assert!(activation_body(patch, false, true).is_err());
    }

    #[test]
    fn omitted_and_explicit_clear_schemas_differ() {
        let patch = body(&SchemaArgs {
            clear_out_schema: true,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(patch, json!({"out_schema":{"op":"clear"}}));
        assert!(body(&SchemaArgs::default()).is_err());
        assert!(body(&SchemaArgs {
            out_schema: Some("-".into()),
            event_config_schema: Some("-".into()),
            ..Default::default()
        })
        .is_err());
    }

    #[test]
    fn help_and_flags_make_the_distinct_acknowledgement_discoverable() {
        let help = TestCli::try_parse_from(["rollout", "apply", "--help"])
            .err()
            .unwrap()
            .to_string();
        assert!(help.contains("Examples:"));
        assert!(help.contains("--acknowledge-incompatible-rollout"));
        assert!(TestCli::try_parse_from([
            "rollout",
            "apply",
            "example.tick",
            "--clear-out-schema",
            "--force"
        ])
        .is_err());
    }
}
