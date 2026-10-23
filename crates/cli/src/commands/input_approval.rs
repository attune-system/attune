use anyhow::{bail, Result};
use clap::Subcommand;
use serde_json::json;

use crate::{
    client::ApiClient,
    config::CliConfig,
    input_approval::{self, InputApprovalRequest},
    output::{self, OutputFormat},
};

#[derive(Subcommand)]
pub enum InputApprovalCommands {
    /// Discover currently visible exact retained declaration targets before execution
    #[command(
        after_help = "Examples:\n  attune pack input-approval targets example --output json\n  attune pack input-approval targets example --policy-revision 17 --page-size 20\n\nPackRead and current source/action visibility are required. Discovery returns metadata, not consent or execution authority."
    )]
    Targets {
        pack_ref: String,
        #[arg(long)]
        policy_revision: Option<i64>,
        #[arg(long, default_value_t = 1)]
        page: u32,
        #[arg(long, default_value_t = 20)]
        page_size: u32,
    },
    /// List the pack's shared approval ledger, including input, runtime, expired and revoked records
    #[command(
        after_help = "Examples:\n  attune pack input-approval list example --output json\n\nThe API has one shared approval ledger; this is not an input-only filtered list."
    )]
    List { pack_ref: String },
    /// Get an issued approval by ID from the shared ledger, without reconstructing private proof
    #[command(
        alias = "get",
        after_help = "Examples:\n  attune pack input-approval show example 42 --output json"
    )]
    Show { pack_ref: String, id: i64 },
    /// Issue fixed consent for one exact input declaration, never SDK/API permissions
    #[command(
        after_help = "Examples:\n  attune pack input-approval issue example --file input-approval.json\n  cat input-approval.json | attune pack input-approval issue example --file - --dry-run\n\nUse policy_revision + invocation_binding before execution, or renderer_execution for an existing renderer. Include declaration_revision, source_path, expression, grants and optional expires_at. Purpose is bound to the retained declaration, not supplied separately. Use profile credentials; never include tokens in the request."
    )]
    Issue {
        pack_ref: String,
        /// Exact public request JSON; '-' reads bounded stdin, with no interactive prompts
        #[arg(long)]
        file: String,
        /// Validate locally without API access; the expression is omitted from preview output
        #[arg(long)]
        dry_run: bool,
    },
    /// Explicitly revoke consent via the shared approval lifecycle, idempotently
    #[command(
        after_help = "Examples:\n  attune pack input-approval revoke example 42 --dry-run\n  attune pack input-approval revoke example 42 --yes\n\nThis can revoke any approval ID in the pack's shared ledger. Inspect the ID first. Human inactivity does not revoke consent."
    )]
    Revoke {
        pack_ref: String,
        id: i64,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
}

pub async fn handle(
    profile: &Option<String>,
    api_url: &Option<String>,
    command: InputApprovalCommands,
    format: OutputFormat,
) -> Result<()> {
    // Validate and confirm before constructing authenticated transport.
    match command {
        InputApprovalCommands::Targets {
            pack_ref,
            policy_revision,
            page,
            page_size,
        } => {
            let config = CliConfig::load_with_profile(profile.as_deref())?;
            let mut client = ApiClient::from_config(&config, api_url);
            output::print_output(
                &input_approval::discover(&mut client, &pack_ref, policy_revision, page, page_size)
                    .await?,
                format,
            )
        }
        InputApprovalCommands::Issue {
            pack_ref,
            file,
            dry_run,
        } => {
            let endpoint = input_approval::issue_path(&pack_ref)?;
            let body = InputApprovalRequest::parse(&super::pack_approval::read_request(&file)?)?;
            if dry_run {
                return output::print_output(
                    &json!({"dry_run":true,"authorization_checked":false,
                    "operation":"issue_input_only","path":endpoint,"renderer_execution":body.renderer_execution,
                    "policy_revision":body.policy_revision,"invocation_binding":body.invocation_binding,
                    "declaration_revision":body.declaration_revision,"source_path":body.source_path,
                    "expression":"[REDACTED]","grant_count":body.grants.len(),"expires_at":body.expires_at}),
                    format,
                );
            }
            let config = CliConfig::load_with_profile(profile.as_deref())?;
            let mut client = ApiClient::from_config(&config, api_url);
            output::print_output(
                &input_approval::issue(&mut client, &pack_ref, &body).await?,
                format,
            )
        }
        InputApprovalCommands::Revoke {
            pack_ref,
            id,
            dry_run,
            yes,
        } => {
            let endpoint = format!("{}/revoke", input_approval::record_path(&pack_ref, id)?);
            if dry_run {
                return output::print_output(
                    &json!({"dry_run":true,"authorization_checked":false,"operation":"revoke","path":endpoint}),
                    format,
                );
            }
            if !yes {
                bail!("Revocation stops input consent. Pass --yes or preview with --dry-run");
            }
            let config = CliConfig::load_with_profile(profile.as_deref())?;
            let mut client = ApiClient::from_config(&config, api_url);
            output::print_output(
                &input_approval::revoke(&mut client, &pack_ref, id, yes).await?,
                format,
            )
        }
        InputApprovalCommands::List { pack_ref } => {
            input_approval::collection_path(&pack_ref)?;
            let config = CliConfig::load_with_profile(profile.as_deref())?;
            let mut client = ApiClient::from_config(&config, api_url);
            let records = input_approval::list(&mut client, &pack_ref).await?;
            output::print_output(&records, format)
        }
        InputApprovalCommands::Show { pack_ref, id } => {
            input_approval::record_path(&pack_ref, id)?;
            let config = CliConfig::load_with_profile(profile.as_deref())?;
            let mut client = ApiClient::from_config(&config, api_url);
            output::print_output(
                &input_approval::get(&mut client, &pack_ref, id).await?,
                format,
            )
        }
    }
}
