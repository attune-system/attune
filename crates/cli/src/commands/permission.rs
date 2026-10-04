use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use attune_common::models::ManagementOriginKind;
use clap::{Args, Subcommand};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::iam::{self, MutationResult};
use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Debug, Subcommand)]
#[command(
    after_help = "Examples:\n  attune permission set list\n  attune permission assign core.executor --identity alice@example.com\n  attune permission assignment list --role deployment-operators"
)]
pub enum PermissionCommands {
    /// Inspect, export, and update permission set definitions
    #[command(
        after_help = "Examples:\n  attune permission set show core.executor\n  attune permission set export deploy.operator --yaml"
    )]
    Set {
        #[command(subcommand)]
        command: PermissionSetCommands,
    },
    /// Inspect direct identity assignments and role mappings
    #[command(
        after_help = "Examples:\n  attune permission assignment list --identity alice@example.com\n  attune permission assignment list --set core.executor"
    )]
    Assignment {
        #[command(subcommand)]
        command: PermissionAssignmentCommands,
    },
    /// Assign a permission set to an identity or role; existing assignments are a no-op
    #[command(
        after_help = "Examples:\n  attune permission assign core.executor --identity alice@example.com\n  attune permission assign deploy.operator --role deployment-operators --dry-run"
    )]
    Assign {
        permission_set_ref: String,
        #[command(flatten)]
        target: PermissionTargetArgs,
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a direct identity assignment or role mapping; absent assignments are a no-op
    #[command(
        after_help = "Examples:\n  attune permission revoke core.executor --identity alice@example.com\n  attune permission revoke deploy.operator --role deployment-operators --dry-run"
    )]
    Revoke {
        permission_set_ref: String,
        #[command(flatten)]
        target: PermissionTargetArgs,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum PermissionSetCommands {
    /// List permission sets and their ownership
    #[command(
        after_help = "Examples:\n  attune permission set list\n  attune permission set list --pack deploy --include-retired --json"
    )]
    List {
        #[arg(long)]
        pack: Option<String>,
        #[arg(long)]
        include_retired: bool,
    },
    /// Show grants, constraints, ownership, and role mappings
    #[command(
        after_help = "Examples:\n  attune permission set show core.executor\n  attune permission set show deploy.operator --json"
    )]
    Show { permission_set_ref: String },
    /// Export an authorable definition; defaults to YAML
    #[command(
        after_help = "Examples:\n  attune permission set export deploy.operator > operator.yaml\n  attune permission set export deploy.operator --json"
    )]
    Export { permission_set_ref: String },
    /// Replace grants using a YAML/JSON definition; omitted label/description retain their values
    #[command(
        after_help = "Examples:\n  attune permission set update deploy.operator --file operator.yaml --dry-run\n  attune permission set update deploy.operator --file - < operator.yaml"
    )]
    Update {
        permission_set_ref: String,
        /// YAML/JSON definition, or - to read stdin
        #[arg(long)]
        file: PathBuf,
        /// Ask the API to validate and preview without writing
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Args)]
#[group(id = "permission_target", required = true, multiple = false)]
pub struct PermissionTargetArgs {
    /// Exact identity login
    #[arg(long)]
    identity: Option<String>,
    #[arg(long)]
    identity_id: Option<i64>,
    #[arg(long)]
    role: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum PermissionAssignmentCommands {
    /// List assignments; identity filters show direct assignments, not inherited access
    #[command(
        after_help = "Examples:\n  attune permission assignment list --identity alice@example.com\n  attune permission assignment list --role deployment-operators\n  attune permission assignment list --set core.executor --json"
    )]
    List {
        #[arg(long, conflicts_with_all = ["identity_id", "role"])]
        identity: Option<String>,
        #[arg(long, conflicts_with = "role")]
        identity_id: Option<i64>,
        #[arg(long)]
        role: Option<String>,
        #[arg(long = "set")]
        permission_set_ref: Option<String>,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        page: u32,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100))]
        per_page: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PermissionSet {
    id: i64,
    #[serde(rename = "ref")]
    reference: String,
    pack_ref: Option<String>,
    label: Option<String>,
    description: Option<String>,
    grants: Value,
    retired_at: Option<String>,
    management_origin: ManagementOriginKind,
    roles: Vec<PermissionSetRole>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PermissionSetRole {
    id: i64,
    permission_set_id: i64,
    permission_set_ref: Option<String>,
    role: String,
    created: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PermissionSetDefinition {
    #[serde(rename = "ref")]
    reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    grants: Value,
}

impl From<&PermissionSet> for PermissionSetDefinition {
    fn from(set: &PermissionSet) -> Self {
        Self {
            reference: set.reference.clone(),
            label: set.label.clone(),
            description: set.description.clone(),
            grants: set.grants.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PermissionTarget {
    Identity { identity_id: i64, login: String },
    Role { role: String },
}

#[derive(Debug, Serialize, Deserialize)]
struct PermissionBinding {
    id: i64,
    permission_set_id: i64,
    permission_set_ref: String,
    target: PermissionTarget,
    created: String,
}

pub async fn handle_permission_command(
    profile: &Option<String>,
    command: PermissionCommands,
    api_url: &Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    run(&mut client, command, format).await
}

pub(super) async fn run(
    client: &mut ApiClient,
    command: PermissionCommands,
    format: OutputFormat,
) -> Result<()> {
    match command {
        PermissionCommands::Set { command } => match command {
            PermissionSetCommands::List {
                pack,
                include_retired,
            } => {
                let mut query = vec![("include_retired", include_retired.to_string())];
                if let Some(pack) = pack {
                    query.push(("pack_ref", pack));
                }
                // The permission-set list API returns a bare array.
                let sets: Vec<PermissionSet> = client
                    .get_bare(&iam::query_path("/permissions/sets", &query))
                    .await?;
                print_sets(&sets, format)
            }
            PermissionSetCommands::Show { permission_set_ref } => {
                let set = get_set(client, &permission_set_ref).await?;
                output::print_output(
                    &set,
                    if format == OutputFormat::Table {
                        OutputFormat::Yaml
                    } else {
                        format
                    },
                )
            }
            PermissionSetCommands::Export { permission_set_ref } => {
                let set = get_set(client, &permission_set_ref).await?;
                output::print_output(
                    &PermissionSetDefinition::from(&set),
                    if format == OutputFormat::Table {
                        OutputFormat::Yaml
                    } else {
                        format
                    },
                )
            }
            PermissionSetCommands::Update {
                permission_set_ref,
                file,
                dry_run,
            } => update_set(client, &permission_set_ref, &file, dry_run)
                .await?
                .print(format),
        },
        PermissionCommands::Assignment {
            command:
                PermissionAssignmentCommands::List {
                    identity,
                    identity_id,
                    role,
                    permission_set_ref,
                    page,
                    per_page,
                },
        } => {
            let mut query = binding_filters(
                identity.as_deref(),
                identity_id,
                role.as_deref(),
                permission_set_ref.as_deref(),
            );
            query.extend([
                ("page", page.to_string()),
                ("page_size", per_page.to_string()),
            ]);
            let response = client
                .get_paginated_response::<PermissionBinding>(&iam::query_path(
                    "/permissions/assignments",
                    &query,
                ))
                .await?;
            if format != OutputFormat::Table {
                return output::print_output(&response, format);
            }
            print_bindings(&response.items)
        }
        PermissionCommands::Assign {
            permission_set_ref,
            target,
            dry_run,
        } => change_assignment(client, &permission_set_ref, target, true, dry_run)
            .await?
            .print(format),
        PermissionCommands::Revoke {
            permission_set_ref,
            target,
            dry_run,
        } => change_assignment(client, &permission_set_ref, target, false, dry_run)
            .await?
            .print(format),
    }
}

async fn get_set(client: &mut ApiClient, reference: &str) -> Result<PermissionSet> {
    if reference == "standard" {
        bail!(
            "'standard' is reserved for execution tokens and is not an assignable permission set"
        );
    }
    client
        .get(&format!(
            "/permissions/sets/by-ref/{}",
            urlencoding::encode(reference)
        ))
        .await
}

fn read_definition(path: &Path, expected_ref: &str) -> Result<PermissionSetDefinition> {
    let contents = if path == Path::new("-") {
        let mut contents = String::new();
        std::io::stdin()
            .read_to_string(&mut contents)
            .context("Failed to read permission set definition from stdin")?;
        contents
    } else {
        std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?
    };
    let definition: PermissionSetDefinition =
        serde_yaml_ng::from_str(&contents).context("Invalid permission set definition")?;
    if definition.reference != expected_ref {
        bail!(
            "Definition ref '{}' does not match target '{expected_ref}'",
            definition.reference
        );
    }
    if !definition.grants.is_array() {
        bail!("Permission set grants must be an array");
    }
    Ok(definition)
}

async fn update_set(
    client: &mut ApiClient,
    reference: &str,
    path: &Path,
    dry_run: bool,
) -> Result<MutationResult> {
    let definition = read_definition(path, reference)?;
    let before = get_set(client, reference).await?;
    if before.management_origin == ManagementOriginKind::Platform {
        bail!("Permission set '{reference}' is platform-managed and cannot be updated");
    }
    if before.retired_at.is_some() {
        bail!("Permission set '{reference}' is retired and cannot be updated");
    }
    let body = json!({"label": definition.label, "description": definition.description, "grants": definition.grants});
    let after: PermissionSet = client
        .put(
            &format!("/permissions/sets/{}?dry_run={dry_run}", before.id),
            &body,
        )
        .await?;
    if before.management_origin == ManagementOriginKind::Pack {
        eprintln!(
            "Pack-owned definition '{reference}': a later pack update can overwrite this edit."
        );
    }
    Ok(MutationResult::new(
        "set_update",
        json!({"permission_set_ref": reference, "permission_set_id": before.id}),
        dry_run,
        serde_json::to_value(PermissionSetDefinition::from(&before))?,
        serde_json::to_value(PermissionSetDefinition::from(&after))?,
    ))
}

fn binding_filters(
    identity: Option<&str>,
    identity_id: Option<i64>,
    role: Option<&str>,
    reference: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut query = Vec::new();
    if let Some(identity) = identity {
        query.push(("identity_login", identity.to_string()));
    }
    if let Some(identity_id) = identity_id {
        query.push(("identity_id", identity_id.to_string()));
    }
    if let Some(role) = role {
        query.push(("role", role.to_string()));
    }
    if let Some(reference) = reference {
        query.push(("permission_set_ref", reference.to_string()));
    }
    query
}

async fn target(client: &mut ApiClient, args: PermissionTargetArgs) -> Result<PermissionTarget> {
    match (args.identity.as_deref(), args.identity_id, args.role) {
        (None, None, Some(role)) => {
            iam::validate_role(&role)?;
            Ok(PermissionTarget::Role { role })
        }
        (login, id, None) => {
            let identity = iam::resolve_identity(client, login, id).await?;
            Ok(PermissionTarget::Identity {
                identity_id: identity.id,
                login: identity.login,
            })
        }
        _ => bail!("Specify exactly one of --identity, --identity-id, or a nonempty --role"),
    }
}

async fn find_binding(
    client: &mut ApiClient,
    target: &PermissionTarget,
    reference: &str,
) -> Result<Option<PermissionBinding>> {
    let query = match target {
        PermissionTarget::Identity { identity_id, .. } => {
            binding_filters(None, Some(*identity_id), None, Some(reference))
        }
        PermissionTarget::Role { role } => binding_filters(None, None, Some(role), Some(reference)),
    };
    let mut bindings: Vec<PermissionBinding> =
        iam::all_pages(client, "/permissions/assignments", &query).await?;
    Ok(bindings.pop())
}

async fn change_assignment(
    client: &mut ApiClient,
    reference: &str,
    args: PermissionTargetArgs,
    assign: bool,
    dry_run: bool,
) -> Result<MutationResult> {
    let set = get_set(client, reference).await?;
    let target = target(client, args).await?;
    let binding = find_binding(client, &target, reference).await?;
    if assign && binding.is_none() && set.retired_at.is_some() {
        bail!("Permission set '{reference}' is retired and cannot be newly assigned");
    }
    let mut before = binding.is_some();
    if !dry_run {
        if assign && !before {
            let response: Result<Value> =
                match &target {
                    PermissionTarget::Identity { identity_id, .. } => client
                        .post(
                            "/permissions/assignments",
                            &json!({"identity_id": identity_id, "permission_set_ref": reference}),
                        )
                        .await,
                    PermissionTarget::Role { role } => {
                        client
                            .post(
                                &format!("/permissions/sets/{}/roles", set.id),
                                &json!({"role": role}),
                            )
                            .await
                    }
                };
            match response {
                Ok(_) => {}
                Err(error) if iam::has_status(&error, StatusCode::CONFLICT) => {
                    if find_binding(client, &target, reference).await?.is_none() {
                        return Err(error);
                    }
                    before = true;
                }
                Err(error) => return Err(error),
            }
        } else if let Some(binding) = binding.filter(|_| !assign) {
            let path = match target {
                PermissionTarget::Identity { .. } => {
                    format!("/permissions/assignments/{}", binding.id)
                }
                PermissionTarget::Role { .. } => format!("/permissions/sets/roles/{}", binding.id),
            };
            if !iam::delete_if_present(client, &path).await? {
                before = false;
            }
        }
    }
    Ok(MutationResult::new(
        if assign { "assign" } else { "revoke" },
        json!({"permission_set_ref": reference, "assignee": target}),
        dry_run,
        json!(before),
        json!(assign),
    ))
}

fn print_sets(sets: &[PermissionSet], format: OutputFormat) -> Result<()> {
    if format != OutputFormat::Table {
        return output::print_output(&sets, format);
    }
    let mut table = output::create_table();
    output::add_header(
        &mut table,
        vec!["ID", "Ref", "Pack", "Origin", "Status", "Roles"],
    );
    for set in sets {
        table.add_row(vec![
            set.id.to_string(),
            set.reference.clone(),
            set.pack_ref.clone().unwrap_or_default(),
            serde_json::to_value(set.management_origin)?
                .as_str()
                .unwrap_or_default()
                .to_string(),
            if set.retired_at.is_some() {
                "retired"
            } else {
                "active"
            }
            .to_string(),
            set.roles
                .iter()
                .map(|role| role.role.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        ]);
    }
    println!("{table}");
    Ok(())
}

fn print_bindings(bindings: &[PermissionBinding]) -> Result<()> {
    let mut table = output::create_table();
    output::add_header(
        &mut table,
        vec!["ID", "Permission set", "Target type", "Target", "Created"],
    );
    for binding in bindings {
        let (kind, name) = match &binding.target {
            PermissionTarget::Identity { login, .. } => ("identity", login),
            PermissionTarget::Role { role } => ("role", role),
        };
        table.add_row(vec![
            binding.id.to_string(),
            binding.permission_set_ref.clone(),
            kind.to_string(),
            name.clone(),
            binding.created.clone(),
        ]);
    }
    println!("{table}");
    Ok(())
}
