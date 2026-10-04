use anyhow::{bail, Result};
use clap::Subcommand;
use reqwest::StatusCode;
use serde_json::json;

use super::iam::{self, Identity, IdentityRole, IdentitySelector, IdentitySummary, MutationResult};
use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Debug, Subcommand)]
pub enum IdentityCommands {
    /// List identities and their role names
    #[command(
        after_help = "Examples:\n  attune identity list\n  attune identity list --login alice@example.com --json"
    )]
    List {
        /// Filter by exact login
        #[arg(long)]
        login: Option<String>,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        page: u32,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100))]
        per_page: u32,
    },
    /// Show identity details, direct permission assignments, and role membership
    #[command(
        after_help = "Examples:\n  attune identity show alice@example.com\n  attune identity show --identity-id 42 --json"
    )]
    Show {
        #[command(flatten)]
        identity: IdentitySelector,
    },
    /// Inspect and manage manual role membership
    #[command(
        after_help = "Examples:\n  attune identity role list alice@example.com\n  attune identity role add alice@example.com deployment-operators"
    )]
    Role {
        #[command(subcommand)]
        command: IdentityRoleCommands,
    },
    /// Freeze an identity
    #[command(
        after_help = "Examples:\n  attune identity freeze alice@example.com --dry-run\n  attune identity freeze --identity-id 42"
    )]
    Freeze {
        #[command(flatten)]
        identity: IdentitySelector,
        #[arg(long)]
        dry_run: bool,
    },
    /// Unfreeze an identity
    #[command(
        after_help = "Examples:\n  attune identity unfreeze alice@example.com\n  attune identity unfreeze --identity-id 42 --dry-run"
    )]
    Unfreeze {
        #[command(flatten)]
        identity: IdentitySelector,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum IdentityRoleCommands {
    /// List role membership, provider source, and managed status
    #[command(
        after_help = "Examples:\n  attune identity role list alice@example.com\n  attune identity role list --identity-id 42 --json"
    )]
    List {
        #[command(flatten)]
        identity: IdentitySelector,
    },
    /// Add manual role membership; an existing membership is a no-op
    #[command(
        after_help = "Examples:\n  attune identity role add alice@example.com deployment-operators\n  attune identity role add --identity-id 42 deployment-operators --dry-run"
    )]
    #[command(allow_missing_positional = true)]
    Add {
        #[command(flatten)]
        identity: IdentitySelector,
        role: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove manual membership; provider-managed roles must use provider sync
    #[command(
        after_help = "Examples:\n  attune identity role remove alice@example.com deployment-operators\n  attune identity role remove --identity-id 42 deployment-operators --dry-run"
    )]
    #[command(allow_missing_positional = true)]
    Remove {
        #[command(flatten)]
        identity: IdentitySelector,
        role: String,
        #[arg(long)]
        dry_run: bool,
    },
}

pub async fn handle_identity_command(
    profile: &Option<String>,
    command: IdentityCommands,
    api_url: &Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    run(&mut client, command, format).await
}

pub(super) async fn run(
    client: &mut ApiClient,
    command: IdentityCommands,
    format: OutputFormat,
) -> Result<()> {
    match command {
        IdentityCommands::List {
            login,
            page,
            per_page,
        } => {
            let mut query = vec![
                ("page", page.to_string()),
                ("page_size", per_page.to_string()),
            ];
            if let Some(login) = login {
                query.push(("login", login));
            }
            let response = client
                .get_paginated_response::<IdentitySummary>(&iam::query_path("/identities", &query))
                .await?;
            if format != OutputFormat::Table {
                return output::print_output(&response, format);
            }
            let mut table = output::create_table();
            output::add_header(
                &mut table,
                vec!["ID", "Login", "Display name", "Frozen", "Roles"],
            );
            for identity in response.items {
                table.add_row(vec![
                    identity.id.to_string(),
                    identity.login,
                    identity.display_name.unwrap_or_default(),
                    identity.frozen.to_string(),
                    identity.roles.join(", "),
                ]);
            }
            println!("{table}");
            Ok(())
        }
        IdentityCommands::Show { identity } => {
            let identity = lookup(client, &identity).await?;
            if format != OutputFormat::Table {
                return output::print_output(&identity, format);
            }
            output::print_key_value_table(vec![
                ("ID", identity.id.to_string()),
                ("Login", identity.login),
                ("Display name", identity.display_name.unwrap_or_default()),
                ("Frozen", identity.frozen.to_string()),
            ]);
            print_roles(&identity.roles, format)?;
            output::print_output(&identity.direct_permissions, OutputFormat::Yaml)
        }
        IdentityCommands::Role { command } => match command {
            IdentityRoleCommands::List { identity } => {
                print_roles(&lookup(client, &identity).await?.roles, format)
            }
            IdentityRoleCommands::Add {
                identity,
                role,
                dry_run,
            } => change_role(client, &identity, &role, true, dry_run)
                .await?
                .print(format),
            IdentityRoleCommands::Remove {
                identity,
                role,
                dry_run,
            } => change_role(client, &identity, &role, false, dry_run)
                .await?
                .print(format),
        },
        IdentityCommands::Freeze { identity, dry_run } => {
            set_frozen(client, &identity, true, dry_run)
                .await?
                .print(format)
        }
        IdentityCommands::Unfreeze { identity, dry_run } => {
            set_frozen(client, &identity, false, dry_run)
                .await?
                .print(format)
        }
    }
}

async fn lookup(client: &mut ApiClient, selector: &IdentitySelector) -> Result<Identity> {
    iam::resolve_identity(client, selector.login.as_deref(), selector.identity_id).await
}

fn print_roles(roles: &[IdentityRole], format: OutputFormat) -> Result<()> {
    if format != OutputFormat::Table {
        return output::print_output(&roles, format);
    }
    let mut table = output::create_table();
    output::add_header(&mut table, vec!["ID", "Role", "Source", "Managed"]);
    for role in roles {
        table.add_row(vec![
            role.id.to_string(),
            role.role.clone(),
            role.source.clone(),
            role.managed.to_string(),
        ]);
    }
    println!("{table}");
    Ok(())
}

pub(super) async fn change_role(
    client: &mut ApiClient,
    selector: &IdentitySelector,
    role: &str,
    add: bool,
    dry_run: bool,
) -> Result<MutationResult> {
    iam::validate_role(role)?;
    let identity = lookup(client, selector).await?;
    let membership = identity
        .roles
        .iter()
        .find(|membership| membership.role == role);
    if !add && membership.is_some_and(|membership| membership.managed) {
        bail!(
            "Role '{role}' is provider-managed; update membership through identity provider sync"
        );
    }
    let mut before = membership.is_some();
    if !dry_run {
        if add && !before {
            let response: Result<IdentityRole> = client
                .post(
                    &format!("/identities/{}/roles", identity.id),
                    &json!({"role": role}),
                )
                .await;
            match response {
                Ok(_) => {}
                Err(error) if iam::has_status(&error, StatusCode::CONFLICT) => {
                    let current: Identity =
                        client.get(&format!("/identities/{}", identity.id)).await?;
                    if !current
                        .roles
                        .iter()
                        .any(|membership| membership.role == role)
                    {
                        return Err(error);
                    }
                    before = true;
                }
                Err(error) => return Err(error),
            }
        } else if let Some(membership) = membership.filter(|_| !add) {
            if !iam::delete_if_present(client, &format!("/identities/roles/{}", membership.id))
                .await?
            {
                before = false;
            }
        }
    }
    Ok(MutationResult::new(
        if add { "role_add" } else { "role_remove" },
        json!({"identity_id": identity.id, "login": identity.login, "role": role}),
        dry_run,
        json!(before),
        json!(add),
    ))
}

async fn set_frozen(
    client: &mut ApiClient,
    selector: &IdentitySelector,
    frozen: bool,
    dry_run: bool,
) -> Result<MutationResult> {
    let identity = lookup(client, selector).await?;
    if identity.frozen != frozen && !dry_run {
        client
            .post_no_response(
                &format!(
                    "/identities/{}/{}",
                    identity.id,
                    if frozen { "freeze" } else { "unfreeze" }
                ),
                &json!({}),
            )
            .await?;
    }
    Ok(MutationResult::new(
        if frozen { "freeze" } else { "unfreeze" },
        json!({"identity_id": identity.id, "login": identity.login}),
        dry_run,
        json!(identity.frozen),
        json!(frozen),
    ))
}
