use std::io::Read;

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    client::ApiClient,
    config::CliConfig,
    output::{self, OutputFormat},
};

#[derive(Subcommand)]
pub enum ApprovalCommands {
    /// List approvals, including expired and revoked records
    #[command(after_help = "Examples:\n  attune pack approval list example --output json")]
    List { pack_ref: String },
    /// Inspect fixed scope, issuer attribution, expiry and revocation
    #[command(after_help = "Examples:\n  attune pack approval show example 42 --output json")]
    Show { pack_ref: String, id: i64 },
    /// Issue fixed grants after server-side authority validation
    #[command(
        after_help = "Examples:\n  attune pack approval issue example --file approval.json\n  cat approval.json | attune pack approval issue example --file - --dry-run"
    )]
    Issue {
        pack_ref: String,
        /// JSON request file containing grants and optional expires_at; '-' reads stdin
        #[arg(long)]
        file: String,
        /// Validate and print the request without issuing authority or contacting the API
        #[arg(long)]
        dry_run: bool,
    },
    /// Issue a new approval and explicitly revoke its predecessor atomically
    #[command(
        after_help = "Examples:\n  attune pack approval replace example 42 --file approval.json --dry-run\n  attune pack approval replace example 42 --file approval.json --yes\n\nExisting executions do not automatically acquire the replacement approval."
    )]
    Replace {
        pack_ref: String,
        id: i64,
        #[arg(long)]
        file: String,
        #[arg(long)]
        dry_run: bool,
        /// Confirm explicit revocation of the old approval, without an interactive prompt
        #[arg(long)]
        yes: bool,
    },
    /// Explicitly revoke an approval, idempotently stopping its runtime effects
    #[command(
        after_help = "Examples:\n  attune pack approval revoke example 42 --dry-run\n  attune pack approval revoke example 42 --yes"
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalBody {
    grants: Vec<attune_common::runtime_authority::PermissionGrantDefinition>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn parse_body(bytes: &[u8]) -> Result<Value> {
    let body: ApprovalBody = serde_json::from_slice(bytes).context(
        "Approval must be JSON with a nonempty grants array and optional RFC3339 expires_at",
    )?;
    if body.grants.is_empty() {
        bail!("Approval requires at least one fixed grant");
    }
    if body
        .expires_at
        .is_some_and(|expiry| expiry <= chrono::Utc::now())
    {
        bail!("expires_at must be in the future or omitted");
    }
    // Use the common fixed-scope validator, without inventing a human IAM context.
    let grants = body
        .grants
        .into_iter()
        .map(|grant| grant.into_external_approval_grant())
        .collect::<attune_common::Result<Vec<_>>>()?;
    attune_common::runtime_authority::validate_external_approval_grants(&grants)?;
    Ok(json!({"grants": grants, "expires_at": body.expires_at}))
}

fn read_body(file: &str) -> Result<Value> {
    parse_body(&read_request(file)?)
}

pub(super) fn read_request(file: &str) -> Result<Vec<u8>> {
    let mut reader: Box<dyn Read> = if file == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(
            std::fs::File::open(file)
                .with_context(|| format!("Cannot open approval file {file}"))?,
        )
    };
    let mut bytes = Vec::new();
    reader.by_ref().take(1_048_577).read_to_end(&mut bytes)?;
    if bytes.len() > 1_048_576 {
        bail!("Approval request exceeds 1 MiB");
    }
    Ok(bytes)
}

fn path(pack: &str, id: Option<i64>) -> Result<String> {
    attune_common::schema::RefValidator::validate_pack_ref(pack)?;
    if id.is_some_and(|id| id <= 0) {
        bail!("Approval ID must be positive");
    }
    let base = format!("/packs/{pack}/external-approvals");
    Ok(match id {
        Some(id) => format!("{base}/{id}"),
        None => base,
    })
}

pub async fn handle(
    profile: &Option<String>,
    api_url: &Option<String>,
    command: ApprovalCommands,
    format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    match command {
        ApprovalCommands::List { pack_ref } => {
            let records: Vec<Value> = client.get_paginated(&path(&pack_ref, None)?).await?;
            output::print_output(&records, format)
        }
        ApprovalCommands::Show { pack_ref, id } => {
            let record: Value = client.get(&path(&pack_ref, Some(id))?).await?;
            output::print_output(&record, format)
        }
        ApprovalCommands::Issue {
            pack_ref,
            file,
            dry_run,
        } => {
            let endpoint = path(&pack_ref, None)?;
            let body = read_body(&file)?;
            if dry_run {
                return output::print_output(
                    &json!({"dry_run": true, "authorization_checked": false, "operation": "issue", "path": endpoint, "request": body}),
                    format,
                );
            }
            let record: Value = client.post(&endpoint, &body).await?;
            output::print_output(&record, format)
        }
        ApprovalCommands::Replace {
            pack_ref,
            id,
            file,
            dry_run,
            yes,
        } => {
            let endpoint = format!("{}/replace", path(&pack_ref, Some(id))?);
            if !dry_run && !yes {
                bail!("Replacement revokes the old approval. Pass --yes or preview with --dry-run");
            }
            let body = read_body(&file)?;
            if dry_run {
                return output::print_output(
                    &json!({"dry_run": true, "authorization_checked": false, "operation": "replace_and_revoke", "path": endpoint, "request": body}),
                    format,
                );
            }
            let record: Value = client.post(&endpoint, &body).await?;
            output::print_output(&record, format)
        }
        ApprovalCommands::Revoke {
            pack_ref,
            id,
            dry_run,
            yes,
        } => {
            let endpoint = format!("{}/revoke", path(&pack_ref, Some(id))?);
            if !dry_run && !yes {
                bail!("Revocation stops admitted effects. Pass --yes or preview with --dry-run");
            }
            if dry_run {
                return output::print_output(
                    &json!({"dry_run": true, "authorization_checked": false, "operation": "revoke", "path": endpoint}),
                    format,
                );
            }
            let record: Value = client.post(&endpoint, &json!({})).await?;
            output::print_output(&record, format)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn approval_paths_reject_untrusted_refs_and_ids() {
        assert!(path("../other", None).is_err());
        assert!(path("example", Some(0)).is_err());
        assert_eq!(
            path("example", Some(42)).unwrap(),
            "/packs/example/external-approvals/42"
        );
    }
    #[test]
    fn approval_json_requires_fixed_nonempty_grants() {
        assert!(parse_body(br#"{"grants":[]}"#).is_err());
        assert!(parse_body(br#"{"grants":[{"resource":"keys","actions":["read"],"constraints":{"owner":"self"}}]}"#).is_err());
        assert!(parse_body(
            br#"{"grants":[{"resource":"events","actions":["create"]}],"unexpected":true}"#
        )
        .is_err());
        assert!(parse_body(br#"{"grants":[{"resource":"events","actions":["create"]}]}"#).is_ok());
    }
}
