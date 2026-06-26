use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Subcommand)]
pub enum WorkflowCommands {
    /// Upload a workflow action from local YAML files to an existing pack.
    ///
    /// Reads the action YAML file, finds the referenced workflow YAML file
    /// via its `workflow_file` field, and uploads both to the API. The pack
    /// is determined from the action ref (e.g. `mypack.deploy` → pack `mypack`).
    Upload {
        /// Path to the action YAML file (e.g. actions/deploy.yaml).
        /// Must contain a `workflow_file` field pointing to the workflow YAML.
        action_file: String,

        /// Force update if the workflow already exists
        #[arg(short, long)]
        force: bool,
    },
    /// List workflows
    List {
        /// Filter by pack reference
        #[arg(long)]
        pack: Option<String>,

        /// Filter by tag (comma-separated)
        #[arg(long)]
        tags: Option<String>,

        /// Search term (matches label/description)
        #[arg(long)]
        search: Option<String>,
    },
    /// Show details of a specific workflow
    Show {
        /// Workflow reference (e.g. core.install_packs)
        workflow_ref: String,
    },
    /// Delete a workflow
    Delete {
        /// Workflow reference (e.g. core.install_packs)
        workflow_ref: String,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

// ── Local YAML models (for parsing action YAML files) ──────────────────

/// Minimal representation of an action YAML file, capturing only the fields
/// we need to build a `SaveWorkflowFileRequest`.
#[derive(Debug, Deserialize)]
struct ActionYaml {
    /// Full action ref, e.g. `python_example.timeline_demo`
    #[serde(rename = "ref")]
    action_ref: String,

    /// Human-readable label
    #[serde(default)]
    label: String,

    /// Description
    #[serde(default)]
    description: Option<String>,

    /// Relative path to the workflow YAML from the `actions/` directory
    workflow_file: Option<String>,

    /// Optional enabled flag for the companion workflow action
    #[serde(default)]
    enabled: Option<bool>,

    /// Parameter schema (flat format)
    #[serde(default)]
    parameters: Option<serde_json::Value>,

    /// Output schema (flat format)
    #[serde(default)]
    output: Option<serde_json::Value>,

    /// Tags
    #[serde(default)]
    tags: Option<Vec<String>>,
}

// ── API DTOs ────────────────────────────────────────────────────────────

/// Mirrors the API's `SaveWorkflowFileRequest`.
#[derive(Debug, Serialize)]
struct SaveWorkflowFileRequest {
    name: String,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    version: String,
    pack_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    definition: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    param_schema: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    out_schema: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkflowResponse {
    id: i64,
    #[serde(rename = "ref")]
    workflow_ref: String,
    pack: i64,
    pack_ref: String,
    label: String,
    description: Option<String>,
    version: String,
    param_schema: Option<serde_json::Value>,
    out_schema: Option<serde_json::Value>,
    definition: serde_json::Value,
    tags: Vec<String>,
    created: String,
    updated: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkflowSummary {
    id: i64,
    #[serde(rename = "ref")]
    workflow_ref: String,
    pack_ref: String,
    label: String,
    description: Option<String>,
    version: String,
    tags: Vec<String>,
    created: String,
    updated: String,
}

// ── Command dispatch ────────────────────────────────────────────────────

pub async fn handle_workflow_command(
    profile: &Option<String>,
    command: WorkflowCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        WorkflowCommands::Upload { action_file, force } => {
            handle_upload(profile, action_file, force, api_url, output_format).await
        }
        WorkflowCommands::List { pack, tags, search } => {
            handle_list(profile, pack, tags, search, api_url, output_format).await
        }
        WorkflowCommands::Show { workflow_ref } => {
            handle_show(profile, workflow_ref, api_url, output_format).await
        }
        WorkflowCommands::Delete { workflow_ref, yes } => {
            handle_delete(profile, workflow_ref, yes, api_url, output_format).await
        }
    }
}

// ── Upload ──────────────────────────────────────────────────────────────

async fn handle_upload(
    profile: &Option<String>,
    action_file: String,
    force: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Workflow upload reads local files chosen by the CLI operator; it is not a server-side path sink.
    let action_path = Path::new(&action_file);

    // ── 1. Validate & read the action YAML ──────────────────────────────
    if !action_path.exists() {
        anyhow::bail!("Action YAML file not found: {}", action_file);
    }
    if !action_path.is_file() {
        anyhow::bail!("Path is not a file: {}", action_file);
    }

    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- The action YAML is intentionally read from the validated local CLI path.
    let action_yaml_content =
        std::fs::read_to_string(action_path).context("Failed to read action YAML file")?;

    let action: ActionYaml = serde_yaml_ng::from_str(&action_yaml_content)
        .context("Failed to parse action YAML file")?;

    // ── 2. Extract pack_ref and workflow name from the action ref ────────
    let (pack_ref, workflow_name) = split_action_ref(&action.action_ref)?;

    // ── 3. Resolve the workflow_file path ───────────────────────────────
    let workflow_file_rel = action.workflow_file.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "Action YAML does not contain a 'workflow_file' field. \
             This command requires a workflow action — regular actions should be \
             uploaded as part of a pack."
        )
    })?;

    // workflow_file is relative to the actions/ directory. The action YAML is
    // typically at `<pack>/actions/<name>.yaml`, so the workflow file is
    // resolved relative to the action YAML's parent directory.
    let workflow_path = resolve_workflow_path(action_path, workflow_file_rel)?;

    if !workflow_path.exists() {
        anyhow::bail!(
            "Workflow file not found: {}\n  \
             (resolved from workflow_file: '{}' relative to '{}')",
            workflow_path.display(),
            workflow_file_rel,
            action_path.parent().unwrap_or(Path::new(".")).display()
        );
    }

    // ── 4. Read and parse the workflow YAML ─────────────────────────────
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- The workflow file path is confined to the pack directory before this local read occurs.
    let workflow_yaml_content =
        std::fs::read_to_string(&workflow_path).context("Failed to read workflow YAML file")?;

    let workflow_definition: serde_json::Value = serde_yaml_ng::from_str(&workflow_yaml_content)
        .context(format!(
            "Failed to parse workflow YAML file: {}",
            workflow_path.display()
        ))?;

    // Extract version from the workflow definition, defaulting to "1.0.0"
    let version = workflow_definition
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("1.0.0")
        .to_string();

    // ── 5. Build the API request ────────────────────────────────────────
    //
    // Merge the action-level fields from the workflow definition back into the
    // definition payload (the API's SaveWorkflowFileRequest.definition carries
    // the full blob; write_workflow_yaml on the server side strips the action-
    // level fields before writing the graph-only file).
    let mut definition_map: serde_json::Map<String, serde_json::Value> =
        if let Some(obj) = workflow_definition.as_object() {
            obj.clone()
        } else {
            serde_json::Map::new()
        };

    // Ensure action-level fields are present in the definition (the API and
    // web UI store the combined form in the database; the server splits them
    // into two files on disk).
    if let Some(params) = &action.parameters {
        definition_map
            .entry("parameters".to_string())
            .or_insert_with(|| params.clone());
    }
    if let Some(out) = &action.output {
        definition_map
            .entry("output".to_string())
            .or_insert_with(|| out.clone());
    }

    let request = SaveWorkflowFileRequest {
        name: workflow_name.clone(),
        label: if action.label.is_empty() {
            workflow_name.clone()
        } else {
            action.label.clone()
        },
        description: action.description.clone(),
        version,
        pack_ref: pack_ref.clone(),
        enabled: action.enabled,
        definition: serde_json::Value::Object(definition_map),
        param_schema: action.parameters.clone(),
        out_schema: action.output.clone(),
        tags: action.tags.clone(),
    };

    // ── 6. Print progress ───────────────────────────────────────────────
    if output_format == OutputFormat::Table {
        output::print_info(&format!(
            "Uploading workflow action '{}.{}' to pack '{}'",
            pack_ref, workflow_name, pack_ref,
        ));
        output::print_info(&format!("  Action YAML:   {}", action_path.display()));
        output::print_info(&format!("  Workflow YAML: {}", workflow_path.display()));
    }

    // ── 7. Send to API ──────────────────────────────────────────────────
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let workflow_ref = format!("{}.{}", pack_ref, workflow_name);

    // Try create first; if 409 Conflict and --force, fall back to update.
    let create_path = format!("/packs/{}/workflow-files", pack_ref);

    let result: Result<WorkflowResponse> = client.post(&create_path, &request).await;

    let response: WorkflowResponse = match result {
        Ok(resp) => resp,
        Err(err) => {
            let err_str = err.to_string();
            if err_str.contains("409") || err_str.to_lowercase().contains("conflict") {
                if !force {
                    anyhow::bail!(
                        "Workflow '{}' already exists. Use --force to update it.",
                        workflow_ref
                    );
                }

                if output_format == OutputFormat::Table {
                    output::print_info("Workflow already exists, updating...");
                }

                let update_path = format!("/workflows/{}/file", workflow_ref);
                client.put(&update_path, &request).await.context(
                    "Failed to update existing workflow. \
                     Check that the pack exists and the workflow ref is correct.",
                )?
            } else {
                return Err(err).context("Failed to upload workflow");
            }
        }
    };

    // ── 8. Print result ─────────────────────────────────────────────────
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&response, output_format)?;
        }
        OutputFormat::Table => {
            println!();
            output::print_success(&format!(
                "Workflow '{}' uploaded successfully",
                response.workflow_ref
            ));
            output::print_key_value_table(vec![
                ("Reference", response.workflow_ref.clone()),
                ("Pack", response.pack_ref.clone()),
                ("Label", response.label.clone()),
                ("Version", response.version.clone()),
                (
                    "Tags",
                    if response.tags.is_empty() {
                        "none".to_string()
                    } else {
                        response.tags.join(", ")
                    },
                ),
            ]);
        }
    }

    Ok(())
}

// ── List ────────────────────────────────────────────────────────────────

async fn handle_list(
    profile: &Option<String>,
    pack: Option<String>,
    tags: Option<String>,
    search: Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = if let Some(ref pack_ref) = pack {
        format!("/packs/{}/workflows", pack_ref)
    } else {
        let mut query_parts: Vec<String> = Vec::new();
        if let Some(ref t) = tags {
            query_parts.push(format!("tags={}", urlencoding::encode(t)));
        }
        if let Some(ref s) = search {
            query_parts.push(format!("search={}", urlencoding::encode(s)));
        }
        if query_parts.is_empty() {
            "/workflows".to_string()
        } else {
            format!("/workflows?{}", query_parts.join("&"))
        }
    };

    let workflows: Vec<WorkflowSummary> = client.get_paginated(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&workflows, output_format)?;
        }
        OutputFormat::Table => {
            if workflows.is_empty() {
                output::print_info("No workflows found");
            } else {
                let mut table = output::create_table();
                output::add_header(
                    &mut table,
                    vec!["Reference", "Pack", "Label", "Version", "Tags"],
                );

                for wf in &workflows {
                    table.add_row(vec![
                        wf.workflow_ref.clone(),
                        wf.pack_ref.clone(),
                        output::truncate(&wf.label, 30),
                        wf.version.clone(),
                        if wf.tags.is_empty() {
                            "-".to_string()
                        } else {
                            output::truncate(&wf.tags.join(", "), 25)
                        },
                    ]);
                }

                println!("{}", table);
                output::print_info(&format!("{} workflow(s) found", workflows.len()));
            }
        }
    }

    Ok(())
}

// ── Show ────────────────────────────────────────────────────────────────

async fn handle_show(
    profile: &Option<String>,
    workflow_ref: String,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/workflows/{}", workflow_ref);
    let workflow: WorkflowResponse = client.get(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&workflow, output_format)?;
        }
        OutputFormat::Table => {
            output::print_section(&format!("Workflow: {}", workflow.workflow_ref));
            output::print_key_value_table(vec![
                ("Reference", workflow.workflow_ref.clone()),
                ("Pack", workflow.pack_ref.clone()),
                ("Label", workflow.label.clone()),
                (
                    "Description",
                    workflow
                        .description
                        .clone()
                        .unwrap_or_else(|| "-".to_string()),
                ),
                ("Version", workflow.version.clone()),
                (
                    "Tags",
                    if workflow.tags.is_empty() {
                        "none".to_string()
                    } else {
                        workflow.tags.join(", ")
                    },
                ),
                ("Created", output::format_timestamp(&workflow.created)),
                ("Updated", output::format_timestamp(&workflow.updated)),
            ]);

            // Show parameter schema if present
            if let Some(ref params) = workflow.param_schema {
                if !params.is_null() && params.as_object().is_some_and(|o| !o.is_empty()) {
                    output::print_section("Parameters");
                    let yaml = serde_yaml_ng::to_string(params)?;
                    println!("{}", yaml);
                }
            }

            // Show output schema if present
            if let Some(ref out) = workflow.out_schema {
                if !out.is_null() && out.as_object().is_some_and(|o| !o.is_empty()) {
                    output::print_section("Output Schema");
                    let yaml = serde_yaml_ng::to_string(out)?;
                    println!("{}", yaml);
                }
            }

            // Show task summary from definition
            if let Some(tasks) = workflow.definition.get("tasks") {
                if let Some(arr) = tasks.as_array() {
                    output::print_section("Tasks");
                    let mut table = output::create_table();
                    output::add_header(&mut table, vec!["#", "Name", "Action", "Transitions"]);

                    for (i, task) in arr.iter().enumerate() {
                        let name = task.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                        let action = task.get("action").and_then(|v| v.as_str()).unwrap_or("-");

                        let transition_count = task
                            .get("next")
                            .and_then(|v| v.as_array())
                            .map(|a| {
                                // Count total target tasks across all transitions
                                a.iter()
                                    .filter_map(|t| {
                                        t.get("do").and_then(|d| d.as_array()).map(|d| d.len())
                                    })
                                    .sum::<usize>()
                            })
                            .unwrap_or(0);

                        let transitions_str = if transition_count == 0 {
                            "terminal".to_string()
                        } else {
                            format!("{} target(s)", transition_count)
                        };

                        table.add_row(vec![
                            (i + 1).to_string(),
                            name.to_string(),
                            output::truncate(action, 35),
                            transitions_str,
                        ]);
                    }

                    println!("{}", table);
                }
            }
        }
    }

    Ok(())
}

// ── Delete ──────────────────────────────────────────────────────────────

async fn handle_delete(
    profile: &Option<String>,
    workflow_ref: String,
    yes: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    if !yes && output_format == OutputFormat::Table {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Are you sure you want to delete workflow '{}'?",
                workflow_ref
            ))
            .default(false)
            .interact()?;

        if !confirm {
            output::print_info("Delete cancelled");
            return Ok(());
        }
    }

    let path = format!("/workflows/{}", workflow_ref);
    client.delete_no_response(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            let msg =
                serde_json::json!({"message": format!("Workflow '{}' deleted", workflow_ref)});
            output::print_output(&msg, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!("Workflow '{}' deleted successfully", workflow_ref));
        }
    }

    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Split an action ref like `pack_name.action_name` into `(pack_ref, name)`.
///
/// Supports multi-segment pack refs: `org.pack.action` → `("org.pack", "action")`.
/// The last dot-separated segment is the workflow/action name; everything before
/// it is the pack ref.
fn split_action_ref(action_ref: &str) -> Result<(String, String)> {
    let dot_pos = action_ref.rfind('.').ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid action ref '{}': expected format 'pack_ref.name' (at least one dot)",
            action_ref
        )
    })?;

    let pack_ref = &action_ref[..dot_pos];
    let name = &action_ref[dot_pos + 1..];

    if pack_ref.is_empty() || name.is_empty() {
        anyhow::bail!(
            "Invalid action ref '{}': both pack_ref and name must be non-empty",
            action_ref
        );
    }

    Ok((pack_ref.to_string(), name.to_string()))
}

/// Resolve the workflow YAML path from the action YAML's location and the
/// `workflow_file` value.
///
/// `workflow_file` is relative to the `actions/` directory. Since the action
/// YAML is typically at `<pack>/actions/<name>.yaml`, the workflow path is
/// resolved relative to the action YAML's parent directory.
fn resolve_workflow_path(action_yaml_path: &Path, workflow_file: &str) -> Result<PathBuf> {
    let action_dir = action_yaml_path.parent().unwrap_or(Path::new("."));
    let pack_root = action_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Action YAML must live inside a pack actions/ directory"))?;
    let canonical_pack_root = pack_root
        .canonicalize()
        .context("Failed to resolve pack root for workflow file")?;
    let canonical_action_dir = action_dir
        .canonicalize()
        .context("Failed to resolve action directory for workflow file")?;
    let canonical_workflow_path = normalize_path_from_base(&canonical_action_dir, workflow_file);

    if !canonical_workflow_path.starts_with(&canonical_pack_root) {
        anyhow::bail!(
            "Workflow file resolves outside the pack directory: {}",
            workflow_file
        );
    }

    Ok(canonical_workflow_path)
}

fn normalize_path_from_base(base: &Path, relative_path: &str) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in base.join(relative_path).components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR.to_string()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_action_ref_simple() {
        let (pack, name) = split_action_ref("core.echo").unwrap();
        assert_eq!(pack, "core");
        assert_eq!(name, "echo");
    }

    #[test]
    fn test_split_action_ref_multi_segment_pack() {
        let (pack, name) = split_action_ref("org.infra.deploy").unwrap();
        assert_eq!(pack, "org.infra");
        assert_eq!(name, "deploy");
    }

    #[test]
    fn test_split_action_ref_no_dot() {
        assert!(split_action_ref("nodot").is_err());
    }

    #[test]
    fn test_split_action_ref_empty_parts() {
        assert!(split_action_ref(".name").is_err());
        assert!(split_action_ref("pack.").is_err());
    }

    #[test]
    fn test_resolve_workflow_path() {
        let temp = tempfile::tempdir().unwrap();
        let pack_dir = temp.path().join("mypack");
        let actions_dir = pack_dir.join("actions");
        let workflow_dir = actions_dir.join("workflows");
        std::fs::create_dir_all(&workflow_dir).unwrap();

        let action_path = actions_dir.join("deploy.yaml");
        let workflow_path = workflow_dir.join("deploy.workflow.yaml");
        std::fs::write(
            &action_path,
            "ref: mypack.deploy\nworkflow_file: workflows/deploy.workflow.yaml\n",
        )
        .unwrap();
        std::fs::write(&workflow_path, "version: 1.0.0\n").unwrap();

        let resolved =
            resolve_workflow_path(&action_path, "workflows/deploy.workflow.yaml").unwrap();
        assert_eq!(resolved, workflow_path.canonicalize().unwrap());
    }

    #[test]
    fn test_resolve_workflow_path_relative() {
        let temp = tempfile::tempdir().unwrap();
        let pack_dir = temp.path().join("mypack");
        let actions_dir = pack_dir.join("actions");
        let workflows_dir = pack_dir.join("workflows");
        std::fs::create_dir_all(&actions_dir).unwrap();
        std::fs::create_dir_all(&workflows_dir).unwrap();

        let action_path = actions_dir.join("deploy.yaml");
        let workflow_path = workflows_dir.join("deploy.workflow.yaml");
        std::fs::write(
            &action_path,
            "ref: mypack.deploy\nworkflow_file: ../workflows/deploy.workflow.yaml\n",
        )
        .unwrap();
        std::fs::write(&workflow_path, "version: 1.0.0\n").unwrap();

        let resolved =
            resolve_workflow_path(&action_path, "../workflows/deploy.workflow.yaml").unwrap();
        assert_eq!(resolved, workflow_path.canonicalize().unwrap());
    }

    #[test]
    fn test_resolve_workflow_path_rejects_traversal_outside_pack() {
        let temp = tempfile::tempdir().unwrap();
        let pack_dir = temp.path().join("mypack");
        let actions_dir = pack_dir.join("actions");
        std::fs::create_dir_all(&actions_dir).unwrap();

        let action_path = actions_dir.join("deploy.yaml");
        let outside = temp.path().join("outside.yaml");
        std::fs::write(&action_path, "ref: mypack.deploy\n").unwrap();
        std::fs::write(&outside, "version: 1.0.0\n").unwrap();

        let err = resolve_workflow_path(&action_path, "../../outside.yaml").unwrap_err();
        assert!(err.to_string().contains("outside the pack directory"));
    }
}
