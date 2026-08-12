use anyhow::{Context, Result};
use clap::Subcommand;
use flate2::{write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::client::ApiClient;
use crate::commands::pack_index;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Subcommand)]
pub enum PackCommands {
    /// Create an empty pack
    ///
    /// Creates a new pack with no actions, triggers, rules, or sensors.
    /// Use --interactive (-i) to be prompted for each field, or provide
    /// fields via flags. Only --ref is required in non-interactive mode
    /// (--label defaults to a title-cased ref, version defaults to 0.1.0).
    Create {
        /// Unique reference identifier (e.g., "my_pack", "slack")
        #[arg(long, short = 'r')]
        r#ref: Option<String>,

        /// Human-readable label (defaults to title-cased ref)
        #[arg(long, short)]
        label: Option<String>,

        /// Pack description
        #[arg(long, short)]
        description: Option<String>,

        /// Pack version (semver format recommended)
        #[arg(long = "pack-version", default_value = "0.1.0")]
        pack_version: String,

        /// Tags for categorization (comma-separated)
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,

        /// Interactive mode — prompt for each field
        #[arg(long, short)]
        interactive: bool,
    },
    /// List all installed packs
    List {
        /// Filter by pack name
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Show details of a specific pack
    Show {
        /// Pack reference (name or ID)
        pack_ref: String,
    },
    /// Install a pack from various sources (registry, git, URL, or local)
    Install {
        /// Source (git URL, archive URL, local path, or registry reference)
        #[arg(value_name = "SOURCE")]
        source: String,

        /// Git reference (branch, tag, or commit) for git sources
        #[arg(short, long)]
        ref_spec: Option<String>,

        /// Force reinstall even if pack already exists
        #[arg(short, long)]
        force: bool,

        /// Skip running pack tests after installation
        #[arg(long)]
        skip_tests: bool,

        /// Skip dependency validation (not recommended)
        #[arg(long)]
        skip_deps: bool,

        /// Don't search registries (treat source as explicit URL/path)
        #[arg(long)]
        no_registry: bool,
    },
    /// Update a pack
    Update {
        /// Pack reference (name or ID)
        pack_ref: String,

        /// Update label
        #[arg(long)]
        label: Option<String>,

        /// Update description
        #[arg(long)]
        description: Option<String>,

        /// Update version
        #[arg(long)]
        version: Option<String>,
    },
    /// Uninstall a pack
    Uninstall {
        /// Pack reference (name or ID)
        pack_ref: String,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Register a pack from a local directory (path must be accessible by the API server)
    Register {
        /// Path to pack directory (must be a path the API server can access)
        path: String,

        /// Force re-registration if pack already exists
        #[arg(short, long)]
        force: bool,

        /// Skip running pack tests during registration
        #[arg(long)]
        skip_tests: bool,
    },
    /// Upload a local pack directory to the API server and register it
    ///
    /// This command tarballs the local directory and streams it to the API,
    /// so it works regardless of whether the API is local or running in Docker.
    Upload {
        /// Path to the local pack directory (must contain pack.yaml)
        path: String,

        /// Force re-registration if a pack with the same ref already exists
        #[arg(short, long)]
        force: bool,

        /// Skip running pack tests after upload
        #[arg(long)]
        skip_tests: bool,
    },
    /// Check a local pack's metadata without contacting an Attune server
    Check {
        /// Absolute or relative path to the local pack directory
        #[arg(value_name = "PATH")]
        path: String,
    },
    /// Test a pack's test suite
    Test {
        /// Pack reference (name) or path to pack directory
        pack: String,

        /// Show verbose test output
        #[arg(short, long)]
        verbose: bool,

        /// Show detailed test results
        #[arg(short, long)]
        detailed: bool,
    },
    /// List configured registries
    Registries,
    /// Search for packs in registries
    Search {
        /// Search keyword
        keyword: String,

        /// Search in specific registry only
        #[arg(short, long)]
        registry: Option<String>,
    },
    /// Manage configured pack indices and browse indexed packs
    Index {
        #[command(subcommand)]
        command: PackIndexApiCommands,
    },
    /// Calculate checksum of a pack directory or archive
    Checksum {
        /// Path to pack directory or archive file
        path: String,

        /// Output format for registry index entry
        #[arg(long)]
        json: bool,
    },
    /// Generate registry index entry from pack.yaml
    IndexEntry {
        /// Path to pack directory
        path: String,

        /// Git repository URL for the pack
        #[arg(short = 'g', long)]
        git_url: Option<String>,

        /// Git ref (tag/branch) for the pack
        #[arg(short = 'r', long)]
        git_ref: Option<String>,

        /// Archive URL for the pack
        #[arg(short, long)]
        archive_url: Option<String>,

        /// Output format (JSON by default)
        #[arg(short, long, default_value = "json")]
        format: String,
    },
    /// Update a registry index file with a new pack entry
    IndexUpdate {
        /// Path to existing index.json file
        #[arg(short, long)]
        index: String,

        /// Path to pack directory
        path: String,

        /// Git repository URL for the pack
        #[arg(short = 'g', long)]
        git_url: Option<String>,

        /// Git ref (tag/branch) for the pack
        #[arg(short = 'r', long)]
        git_ref: Option<String>,

        /// Archive URL for the pack
        #[arg(short, long)]
        archive_url: Option<String>,

        /// Update existing entry if pack ref already exists
        #[arg(short, long)]
        update: bool,
    },
    /// Merge multiple registry index files into one
    IndexMerge {
        /// Output file path for merged index
        #[arg(short = 'o', long = "file")]
        file: String,

        /// Input index files to merge
        #[arg(required = true)]
        inputs: Vec<String>,

        /// Overwrite output file if it exists
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Subcommand)]
pub enum PackIndexApiCommands {
    /// List configured pack indices
    List,
    /// Add a configured pack index
    Add {
        /// Index URL (https://, http://, or file://)
        url: String,
        /// Human-readable name
        #[arg(short, long)]
        name: Option<String>,
        /// Search order position; lower values are checked first. Omit to append.
        #[arg(short, long)]
        position: Option<i32>,
        /// Add the index disabled
        #[arg(long)]
        disabled: bool,
    },
    /// Update a configured pack index
    Update {
        id: i64,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        clear_name: bool,
        #[arg(short, long)]
        position: Option<i32>,
        #[arg(long)]
        enabled: Option<bool>,
    },
    /// Delete a configured pack index
    Delete { id: i64 },
    /// Browse packs from configured indices
    Browse {
        /// Optional search text
        query: Option<String>,
    },
    /// Show an indexed pack summary
    Show {
        /// Pack ref to look up using configured index order
        pack_ref: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Pack {
    id: i64,
    #[serde(rename = "ref")]
    pack_ref: String,
    label: String,
    description: Option<String>,
    version: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    keywords: Option<Vec<String>>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    created: String,
    updated: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PackInstallResponse {
    pack: Pack,
    test_result: Option<serde_json::Value>,
    tests_skipped: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct PackDetail {
    id: i64,
    #[serde(rename = "ref")]
    pack_ref: String,
    label: String,
    description: Option<String>,
    version: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    keywords: Option<Vec<String>>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    created: String,
    updated: String,
    #[serde(default)]
    action_count: Option<i64>,
    #[serde(default)]
    trigger_count: Option<i64>,
    #[serde(default)]
    rule_count: Option<i64>,
    #[serde(default)]
    sensor_count: Option<i64>,
    #[serde(default)]
    runtime_deps: Vec<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

#[derive(Debug, Serialize)]
struct InstallPackRequest {
    source: String,
    ref_spec: Option<String>,
    force: bool,
    skip_tests: bool,
    skip_deps: bool,
    no_registry: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PackRegistryIndexResponse {
    id: i64,
    name: Option<String>,
    url: String,
    position: i32,
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct CreatePackRegistryIndexRequest {
    name: Option<String>,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<i32>,
    enabled: bool,
    headers: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct UpdatePackRegistryIndexRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedPackResponse {
    pack: serde_json::Value,
    registry: IndexedPackRegistry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedPackRegistry {
    id: Option<i64>,
    name: Option<String>,
    url: String,
    position: i32,
}

#[derive(Debug, Serialize)]
struct RegisterPackRequest {
    path: String,
    force: bool,
    skip_tests: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct UploadPackResponse {
    pack: Pack,
    #[serde(default)]
    test_result: Option<serde_json::Value>,
    #[serde(default)]
    tests_skipped: bool,
}

#[derive(Debug, Serialize)]
struct CreatePackBody {
    r#ref: String,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    version: String,
    #[serde(default)]
    tags: Vec<String>,
}

pub async fn handle_pack_command(
    profile: &Option<String>,
    command: PackCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        PackCommands::Create {
            r#ref,
            label,
            description,
            pack_version,
            tags,
            interactive,
        } => {
            handle_create(
                profile,
                r#ref,
                label,
                description,
                pack_version,
                tags,
                interactive,
                api_url,
                output_format,
            )
            .await
        }
        PackCommands::List { name } => handle_list(profile, name, api_url, output_format).await,
        PackCommands::Show { pack_ref } => {
            handle_show(profile, pack_ref, api_url, output_format).await
        }
        PackCommands::Install {
            source,
            ref_spec,
            force,
            skip_tests,
            skip_deps,
            no_registry,
        } => {
            handle_install(
                profile,
                source,
                ref_spec,
                force,
                skip_tests,
                skip_deps,
                no_registry,
                api_url,
                output_format,
            )
            .await
        }
        PackCommands::Uninstall { pack_ref, yes } => {
            handle_uninstall(profile, pack_ref, yes, api_url, output_format).await
        }
        PackCommands::Register {
            path,
            force,
            skip_tests,
        } => handle_register(profile, path, force, skip_tests, api_url, output_format).await,
        PackCommands::Upload {
            path,
            force,
            skip_tests,
        } => handle_upload(profile, path, force, skip_tests, api_url, output_format).await,
        PackCommands::Check { path } => handle_check(path, output_format),
        PackCommands::Test {
            pack,
            verbose,
            detailed,
        } => handle_test(pack, verbose, detailed, output_format).await,
        PackCommands::Registries => handle_registries(output_format).await,
        PackCommands::Search { keyword, registry } => {
            handle_search(profile, keyword, registry, output_format).await
        }
        PackCommands::Index { command } => {
            handle_index_api(profile, command, api_url, output_format).await
        }
        PackCommands::Update {
            pack_ref,
            label,
            description,
            version,
        } => {
            handle_update(
                profile,
                pack_ref,
                label,
                description,
                version,
                api_url,
                output_format,
            )
            .await
        }
        PackCommands::Checksum { path, json } => handle_checksum(path, json, output_format).await,
        PackCommands::IndexEntry {
            path,
            git_url,
            git_ref,
            archive_url,
            format,
        } => {
            handle_index_entry(
                profile,
                path,
                git_url,
                git_ref,
                archive_url,
                format,
                output_format,
            )
            .await
        }
        PackCommands::IndexUpdate {
            index,
            path,
            git_url,
            git_ref,
            archive_url,
            update,
        } => {
            pack_index::handle_index_update(
                index,
                path,
                git_url,
                git_ref,
                archive_url,
                update,
                output_format,
            )
            .await
        }
        PackCommands::IndexMerge {
            file,
            inputs,
            force,
        } => pack_index::handle_index_merge(file, inputs, force, output_format).await,
    }
}

fn handle_check(path: String, output_format: OutputFormat) -> Result<()> {
    let report = attune_common::pack_check::check_pack(path);

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(&report, output_format)?,
        OutputFormat::Table => {
            output::print_key_value_table(vec![
                ("Path", report.path.display().to_string()),
                (
                    "Pack",
                    report.pack_ref.clone().unwrap_or_else(|| "-".to_string()),
                ),
                (
                    "Version",
                    report.version.clone().unwrap_or_else(|| "-".to_string()),
                ),
                ("Files checked", report.files_checked.to_string()),
                ("Errors", report.errors.to_string()),
                ("Warnings", report.warnings.to_string()),
                ("Valid", report.valid.to_string()),
            ]);

            if !report.diagnostics.is_empty() {
                let mut table = output::create_table();
                output::add_header(&mut table, vec!["Severity", "Path", "Code", "Message"]);
                for diagnostic in &report.diagnostics {
                    table.add_row(vec![
                        format!("{:?}", diagnostic.severity).to_lowercase(),
                        diagnostic.path.as_deref().unwrap_or("-").to_string(),
                        diagnostic.code.clone(),
                        diagnostic.message.clone(),
                    ]);
                }
                println!("{table}");
            }
        }
    }

    if report.valid {
        Ok(())
    } else {
        anyhow::bail!("Pack check failed with {} error(s)", report.errors)
    }
}

/// Derive a human-readable label from a pack ref.
///
/// Splits on `_`, `-`, or `.` and title-cases each word.
fn label_from_ref(r: &str) -> String {
    r.split(['_', '-', '.'])
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    let upper: String = first.to_uppercase().collect();
                    format!("{}{}", upper, chars.as_str())
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[allow(clippy::too_many_arguments)]
async fn handle_create(
    profile: &Option<String>,
    ref_flag: Option<String>,
    label_flag: Option<String>,
    description_flag: Option<String>,
    version_flag: String,
    tags_flag: Vec<String>,
    interactive: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    // ── Collect field values ────────────────────────────────────────
    let (pack_ref, label, description, version, tags) = if interactive {
        // Interactive prompts
        let pack_ref: String = match ref_flag {
            Some(r) => r,
            None => dialoguer::Input::new()
                .with_prompt("Pack ref (unique identifier, e.g. \"my_pack\")")
                .interact_text()?,
        };

        let default_label = label_flag
            .clone()
            .unwrap_or_else(|| label_from_ref(&pack_ref));
        let label: String = dialoguer::Input::new()
            .with_prompt("Label")
            .default(default_label)
            .interact_text()?;

        let default_desc = description_flag.clone().unwrap_or_default();
        let description: String = dialoguer::Input::new()
            .with_prompt("Description (optional, Enter to skip)")
            .default(default_desc)
            .allow_empty(true)
            .interact_text()?;
        let description = if description.is_empty() {
            None
        } else {
            Some(description)
        };

        let version: String = dialoguer::Input::new()
            .with_prompt("Version")
            .default(version_flag)
            .interact_text()?;

        let default_tags = if tags_flag.is_empty() {
            String::new()
        } else {
            tags_flag.join(", ")
        };
        let tags_input: String = dialoguer::Input::new()
            .with_prompt("Tags (comma-separated, optional)")
            .default(default_tags)
            .allow_empty(true)
            .interact_text()?;
        let tags: Vec<String> = tags_input
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // Show summary and confirm
        println!();
        output::print_section("New Pack Summary");
        output::print_key_value_table(vec![
            ("Ref", pack_ref.clone()),
            ("Label", label.clone()),
            (
                "Description",
                description.clone().unwrap_or_else(|| "(none)".to_string()),
            ),
            ("Version", version.clone()),
            (
                "Tags",
                if tags.is_empty() {
                    "(none)".to_string()
                } else {
                    tags.join(", ")
                },
            ),
        ]);
        println!();

        let confirm = dialoguer::Confirm::new()
            .with_prompt("Create this pack?")
            .default(true)
            .interact()?;

        if !confirm {
            output::print_info("Pack creation cancelled");
            return Ok(());
        }

        (pack_ref, label, description, version, tags)
    } else {
        // Non-interactive: ref is required
        let pack_ref = ref_flag.ok_or_else(|| {
            anyhow::anyhow!(
                "Pack ref is required. Provide --ref <value> or use --interactive mode."
            )
        })?;

        let label = label_flag.unwrap_or_else(|| label_from_ref(&pack_ref));
        let description = description_flag;
        let version = version_flag;
        let tags = tags_flag;

        (pack_ref, label, description, version, tags)
    };

    // ── Send request ────────────────────────────────────────────────
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let body = CreatePackBody {
        r#ref: pack_ref,
        label,
        description,
        version,
        tags,
    };

    let pack: Pack = client.post("/packs", &body).await?;

    // ── Output ──────────────────────────────────────────────────────
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&pack, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!(
                "Pack '{}' created successfully (id: {})",
                pack.pack_ref, pack.id
            ));
        }
    }

    Ok(())
}

async fn handle_list(
    profile: &Option<String>,
    name: Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let mut path = "/packs".to_string();
    if let Some(name_filter) = name {
        path = format!("{}?name={}", path, name_filter);
    }

    let packs: Vec<Pack> = client.get_paginated(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&packs, output_format)?;
        }
        OutputFormat::Table => {
            if packs.is_empty() {
                output::print_info("No packs found");
            } else {
                let mut table = output::create_table();
                output::add_header(&mut table, vec!["Ref", "Version", "Description"]);

                for pack in packs {
                    table.add_row(vec![
                        pack.pack_ref,
                        pack.version,
                        output::truncate(&pack.description.unwrap_or_default(), 50),
                    ]);
                }

                println!("{}", table);
            }
        }
    }

    Ok(())
}

async fn handle_show(
    profile: &Option<String>,
    pack_ref: String,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/packs/{}", pack_ref);
    let pack: PackDetail = client.get(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&pack, output_format)?;
        }
        OutputFormat::Table => {
            output::print_section(&format!("Pack: {}", pack.label));
            output::print_key_value_table(vec![
                ("Ref", pack.pack_ref.clone()),
                ("Label", pack.label.clone()),
                ("Version", pack.version),
                (
                    "Author",
                    pack.author.unwrap_or_else(|| "Unknown".to_string()),
                ),
                (
                    "Description",
                    pack.description.unwrap_or_else(|| "None".to_string()),
                ),
                ("Actions", pack.action_count.unwrap_or(0).to_string()),
                ("Triggers", pack.trigger_count.unwrap_or(0).to_string()),
                ("Rules", pack.rule_count.unwrap_or(0).to_string()),
                ("Sensors", pack.sensor_count.unwrap_or(0).to_string()),
                ("Created", output::format_timestamp(&pack.created)),
                ("Updated", output::format_timestamp(&pack.updated)),
            ]);

            if let Some(keywords) = pack.keywords {
                if !keywords.is_empty() {
                    output::print_section("Keywords");
                    output::print_list(keywords);
                }
            }

            if !pack.runtime_deps.is_empty() {
                output::print_section("Runtime Dependencies");
                output::print_list(pack.runtime_deps);
            }

            if !pack.dependencies.is_empty() {
                output::print_section("Pack Dependencies");
                output::print_list(pack.dependencies);
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_install(
    profile: &Option<String>,
    source: String,
    ref_spec: Option<String>,
    force: bool,
    skip_tests: bool,
    skip_deps: bool,
    no_registry: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    // Detect source type
    let source_type = detect_source_type(&source, ref_spec.as_deref(), no_registry);

    if output_format == OutputFormat::Table {
        output::print_info(&format!(
            "Installing pack from: {} ({})",
            source, source_type
        ));
        output::print_info("Starting installation...");
        if skip_deps {
            output::print_info("⚠ Dependency validation will be skipped");
        }
    }

    let request = InstallPackRequest {
        source: source.clone(),
        ref_spec,
        force,
        skip_tests: skip_tests || skip_deps, // Skip tests implies skip deps
        skip_deps,
        no_registry,
    };

    // Note: Progress reporting will be added when API supports streaming
    // For now, we show a simple message during the potentially long operation
    let response: PackInstallResponse = client.post("/packs/install", &request).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&response, output_format)?;
        }
        OutputFormat::Table => {
            println!(); // Add spacing after progress messages
            output::print_success(&format!(
                "✓ Pack '{}' installed successfully",
                response.pack.pack_ref
            ));
            output::print_info(&format!("  Version: {}", response.pack.version));
            output::print_info(&format!("  ID: {}", response.pack.id));

            if response.tests_skipped {
                output::print_info("  ⚠ Tests were skipped");
            } else if let Some(test_result) = &response.test_result {
                if let Some(status) = test_result.get("status").and_then(|s| s.as_str()) {
                    if status == "passed" {
                        output::print_success("  ✓ All tests passed");
                    } else if status == "failed" {
                        output::print_error("  ✗ Some tests failed");
                    }
                    if let Some(summary) = test_result.get("summary") {
                        if let (Some(passed), Some(total)) = (
                            summary.get("passed").and_then(|p| p.as_u64()),
                            summary.get("total").and_then(|t| t.as_u64()),
                        ) {
                            output::print_info(&format!("  Tests: {}/{} passed", passed, total));
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

async fn handle_uninstall(
    profile: &Option<String>,
    pack_ref: String,
    yes: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    // Confirm deletion unless --yes is provided
    if !yes && matches!(output_format, OutputFormat::Table) {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Are you sure you want to uninstall pack '{}'?",
                pack_ref
            ))
            .default(false)
            .interact()?;

        if !confirm {
            output::print_info("Uninstall cancelled");
            return Ok(());
        }
    }

    let path = format!("/packs/{}", pack_ref);
    client.delete_no_response(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            let msg = serde_json::json!({"message": "Pack uninstalled successfully"});
            output::print_output(&msg, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!("Pack '{}' uninstalled successfully", pack_ref));
        }
    }

    Ok(())
}

async fn handle_upload(
    profile: &Option<String>,
    path: String,
    force: bool,
    skip_tests: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- CLI pack commands intentionally operate on operator-supplied local paths.
    let pack_dir = Path::new(&path);

    // Validate the directory exists and contains pack.yaml
    if !pack_dir.exists() {
        anyhow::bail!("Path does not exist: {}", path);
    }
    if !pack_dir.is_dir() {
        anyhow::bail!("Path is not a directory: {}", path);
    }
    let pack_yaml_path = pack_dir.join("pack.yaml");
    if !pack_yaml_path.exists() {
        anyhow::bail!("No pack.yaml found in: {}", path);
    }

    // Read pack ref from pack.yaml so we can display it
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Reading local pack metadata from the user-selected pack directory is expected CLI behavior.
    let pack_yaml_content =
        std::fs::read_to_string(&pack_yaml_path).context("Failed to read pack.yaml")?; // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- This CLI intentionally reads pack.yaml from the local directory selected by the operator.
    let pack_yaml: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&pack_yaml_content).context("Failed to parse pack.yaml")?;
    let pack_ref = pack_yaml
        .get("ref")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    if output_format == OutputFormat::Table {
        output::print_info(&format!("Uploading pack '{}' from: {}", pack_ref, path));
        output::print_info("Creating archive...");
    }

    // Build an in-memory tar.gz of the pack directory
    let tar_gz_bytes = {
        let buf = Vec::new();
        let enc = GzEncoder::new(buf, Compression::default());
        let mut tar = tar::Builder::new(enc);

        // Walk the directory and add files to the archive
        // We strip the leading path so the archive root is the pack directory contents
        let abs_pack_dir = pack_dir
            .canonicalize()
            .context("Failed to resolve pack directory path")?;

        append_dir_to_tar(&mut tar, &abs_pack_dir)?;

        let encoder = tar.into_inner().context("Failed to finalise tar archive")?;
        encoder.finish().context("Failed to flush gzip stream")?
    };

    let archive_size_kb = tar_gz_bytes.len() / 1024;

    if output_format == OutputFormat::Table {
        output::print_info(&format!(
            "Archive ready ({} KB), uploading...",
            archive_size_kb
        ));
    }

    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let mut extra_fields = Vec::new();
    if force {
        extra_fields.push(("force", "true".to_string()));
    }
    if skip_tests {
        extra_fields.push(("skip_tests", "true".to_string()));
    }

    let archive_name = format!("{}.tar.gz", pack_ref);
    let response: UploadPackResponse = client
        .multipart_post(
            "/packs/upload",
            "pack",
            tar_gz_bytes,
            &archive_name,
            "application/gzip",
            extra_fields,
        )
        .await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&response, output_format)?;
        }
        OutputFormat::Table => {
            println!();
            output::print_success(&format!(
                "✓ Pack '{}' uploaded and registered successfully",
                response.pack.pack_ref
            ));
            output::print_info(&format!("  Version: {}", response.pack.version));
            output::print_info(&format!("  ID: {}", response.pack.id));

            if response.tests_skipped {
                output::print_info("  ⚠ Tests were skipped");
            } else if let Some(test_result) = &response.test_result {
                if let Some(status) = test_result.get("status").and_then(|s| s.as_str()) {
                    if status == "passed" {
                        output::print_success("  ✓ All tests passed");
                    } else if status == "failed" {
                        output::print_error("  ✗ Some tests failed");
                    }
                }
            }
        }
    }

    Ok(())
}

/// Recursively append a directory's contents to a tar archive, honoring any
/// `.gitignore` / `.ignore` files inside the pack directory plus the user's
/// global gitignore. Hidden VCS metadata (`.git/`) and the directory's own
/// `.gitignore` files are also skipped from the archive.
///
/// Files are stored with paths relative to `base`.
fn append_dir_to_tar<W: std::io::Write>(tar: &mut tar::Builder<W>, base: &Path) -> Result<()> {
    let walker = ignore::WalkBuilder::new(base)
        // Honor .gitignore and .ignore files inside the pack directory plus the
        // user's global gitignore. This is the only behavior change from the
        // prior naive walk — dotfiles, symlinks, etc. are still treated the
        // same way they were before.
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .hidden(false)
        .parents(false)
        .require_git(false)
        .build();

    for result in walker {
        let entry = match result {
            Ok(entry) => entry,
            Err(err) => {
                anyhow::bail!("Failed to walk pack directory: {}", err);
            }
        };

        let entry_path = entry.path();

        // Skip the root directory itself
        if entry_path == base {
            continue;
        }

        let file_type = match entry.file_type() {
            Some(ft) => ft,
            None => continue,
        };

        if !file_type.is_file() {
            // Directories don't need explicit entries; tar's `append_path_with_name`
            // creates parent dirs implicitly. Symlinks are intentionally skipped.
            continue;
        }

        let relative_path = entry_path
            .strip_prefix(base)
            .context("Failed to compute relative path")?;

        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Pack upload intentionally archives regular files discovered beneath the canonical local directory selected by the CLI operator.
        tar.append_path_with_name(entry_path, relative_path)
            .with_context(|| format!("Failed to add {} to archive", entry_path.display()))?;
    }
    Ok(())
}

async fn handle_register(
    profile: &Option<String>,
    path: String,
    force: bool,
    skip_tests: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    // Warn if the path looks like a local filesystem path that the API server
    // probably can't see (i.e. not a known container mount point).
    let looks_local = !path.starts_with("/opt/attune/")
        && !path.starts_with("/app/")
        && !path.starts_with("/packs");
    if looks_local {
        if output_format == OutputFormat::Table {
            output::print_info(&format!("Registering pack from: {}", path));
            eprintln!(
                "⚠  Warning: '{}' looks like a local path. If the API is running in \
                 Docker it may not be able to access this path.\n   \
                 Use `attune pack upload {}` instead to upload the pack directly.",
                path, path
            );
        }
    } else if output_format == OutputFormat::Table {
        output::print_info(&format!("Registering pack from: {}", path));
    }

    let request = RegisterPackRequest {
        path: path.clone(),
        force,
        skip_tests,
    };

    let response: PackInstallResponse = client.post("/packs/register", &request).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&response, output_format)?;
        }
        OutputFormat::Table => {
            println!(); // Add spacing
            output::print_success(&format!(
                "✓ Pack '{}' registered successfully",
                response.pack.pack_ref
            ));
            output::print_info(&format!("  Version: {}", response.pack.version));
            output::print_info(&format!("  ID: {}", response.pack.id));

            if response.tests_skipped {
                output::print_info("  ⚠ Tests were skipped");
            } else if let Some(test_result) = &response.test_result {
                if let Some(status) = test_result.get("status").and_then(|s| s.as_str()) {
                    if status == "passed" {
                        output::print_success("  ✓ All tests passed");
                    } else if status == "failed" {
                        output::print_error("  ✗ Some tests failed");
                    }
                    if let Some(summary) = test_result.get("summary") {
                        if let (Some(passed), Some(total)) = (
                            summary.get("passed").and_then(|p| p.as_u64()),
                            summary.get("total").and_then(|t| t.as_u64()),
                        ) {
                            output::print_info(&format!("  Tests: {}/{} passed", passed, total));
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

async fn handle_test(
    pack: String,
    verbose: bool,
    detailed: bool,
    output_format: OutputFormat,
) -> Result<()> {
    use attune_common::test_executor::{TestConfig, TestExecutor};
    use std::path::{Path, PathBuf};

    // Determine if pack is a path or a pack name
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Pack test targets are local CLI inputs, not remote request paths.
    let pack_path = Path::new(&pack);
    let (pack_dir, pack_ref, pack_version) = if pack_path.exists() && pack_path.is_dir() {
        // Local pack directory
        output::print_info(&format!("Testing pack from local directory: {}", pack));

        // Load pack.yaml to get ref and version
        let pack_yaml_path = pack_path.join("pack.yaml");
        if !pack_yaml_path.exists() {
            anyhow::bail!("pack.yaml not found in directory: {}", pack);
        }

        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- This reads pack.yaml from a local directory explicitly selected by the CLI operator.
        let pack_yaml_content = std::fs::read_to_string(&pack_yaml_path)?;
        let pack_yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&pack_yaml_content)?;

        let ref_val = pack_yaml
            .get("ref")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("'ref' field not found in pack.yaml"))?;
        let version_val = pack_yaml
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        (
            pack_path.to_path_buf(),
            ref_val.to_string(),
            version_val.to_string(),
        )
    } else {
        // Installed pack - look in standard location
        let packs_dir = PathBuf::from("./packs");
        let pack_dir = packs_dir.join(&pack);

        if !pack_dir.exists() {
            anyhow::bail!(
                "Pack '{}' not found. Provide a pack name or path to a pack directory.",
                pack
            );
        }

        // Load pack.yaml
        let pack_yaml_path = pack_dir.join("pack.yaml");
        if !pack_yaml_path.exists() {
            anyhow::bail!("pack.yaml not found for pack: {}", pack);
        }

        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Installed pack tests intentionally read local metadata from the workspace packs directory.
        let pack_yaml_content = std::fs::read_to_string(&pack_yaml_path)?;
        let pack_yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&pack_yaml_content)?;

        let version_val = pack_yaml
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        (pack_dir, pack.clone(), version_val.to_string())
    };

    // Load pack.yaml and extract test configuration
    let pack_yaml_path = pack_dir.join("pack.yaml");
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Test configuration is loaded from the validated local pack directory.
    let pack_yaml_content = std::fs::read_to_string(&pack_yaml_path)?;
    let pack_yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&pack_yaml_content)?;

    let testing_config = pack_yaml
        .get("testing")
        .ok_or_else(|| anyhow::anyhow!("No 'testing' configuration found in pack.yaml"))?;

    let test_config: TestConfig = serde_yaml_ng::from_value(testing_config.clone())?;

    if !test_config.enabled {
        output::print_warning("Testing is disabled for this pack");
        return Ok(());
    }

    // Create test executor
    let pack_base_dir = pack_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid pack directory"))?
        .to_path_buf();

    let executor = TestExecutor::new(pack_base_dir);

    // Print test start message
    if output_format == OutputFormat::Table {
        println!();
        output::print_section(&format!("🧪 Testing Pack: {} v{}", pack_ref, pack_version));
        println!();
    }

    // Execute tests
    let result = executor
        .execute_pack_tests(&pack_ref, &pack_version, &test_config)
        .await?;

    // Display results
    match output_format {
        OutputFormat::Json => {
            output::print_output(&result, OutputFormat::Json)?;
        }
        OutputFormat::Yaml => {
            output::print_output(&result, OutputFormat::Yaml)?;
        }
        OutputFormat::Table => {
            // Print summary
            println!("Test Results:");
            println!("─────────────────────────────────────────────");
            println!("  Total Tests:  {}", result.total_tests);
            println!("  ✓ Passed:     {}", result.passed);
            println!("  ✗ Failed:     {}", result.failed);
            println!("  ○ Skipped:    {}", result.skipped);
            println!("  Pass Rate:    {:.1}%", result.pass_rate * 100.0);
            println!("  Duration:     {}ms", result.duration_ms);
            println!("─────────────────────────────────────────────");
            println!();

            // Print suite results
            if detailed || verbose {
                for suite in &result.test_suites {
                    println!("Test Suite: {} ({})", suite.name, suite.runner_type);
                    println!(
                        "  Total: {}, Passed: {}, Failed: {}, Skipped: {}",
                        suite.total, suite.passed, suite.failed, suite.skipped
                    );
                    println!("  Duration: {}ms", suite.duration_ms);

                    if verbose {
                        for test_case in &suite.test_cases {
                            let status_icon = match test_case.status {
                                attune_common::models::pack_test::TestStatus::Passed => "✓",
                                attune_common::models::pack_test::TestStatus::Failed => "✗",
                                attune_common::models::pack_test::TestStatus::Skipped => "○",
                                attune_common::models::pack_test::TestStatus::Error => "⚠",
                            };
                            println!(
                                "    {} {} ({}ms)",
                                status_icon, test_case.name, test_case.duration_ms
                            );

                            if let Some(error) = &test_case.error_message {
                                println!("      Error: {}", error);
                            }

                            if detailed {
                                if let Some(stdout) = &test_case.stdout {
                                    if !stdout.is_empty() {
                                        println!("      Stdout:");
                                        for line in stdout.lines().take(10) {
                                            println!("        {}", line);
                                        }
                                    }
                                }
                                if let Some(stderr) = &test_case.stderr {
                                    if !stderr.is_empty() {
                                        println!("      Stderr:");
                                        for line in stderr.lines().take(10) {
                                            println!("        {}", line);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    println!();
                }
            }

            // Final status
            if result.failed > 0 {
                output::print_error(&format!(
                    "❌ Tests failed: {}/{}",
                    result.failed, result.total_tests
                ));
            } else {
                output::print_success(&format!(
                    "✅ All tests passed: {}/{}",
                    result.passed, result.total_tests
                ));
            }
        }
    }

    if result.failed > 0 {
        anyhow::bail!(
            "Pack tests failed: {}/{}",
            result.failed,
            result.total_tests
        );
    }

    Ok(())
}

async fn handle_registries(output_format: OutputFormat) -> Result<()> {
    // Load Attune configuration to get registry settings
    let config = attune_common::config::Config::load()?;

    if !config.pack_registry.enabled {
        output::print_warning("Pack registry system is disabled in configuration");
        return Ok(());
    }

    let registries = config.pack_registry.indices;

    if registries.is_empty() {
        output::print_warning("No registries configured");
        return Ok(());
    }

    match output_format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&registries)?);
        }
        OutputFormat::Yaml => {
            println!("{}", serde_yaml_ng::to_string(&registries)?);
        }
        OutputFormat::Table => {
            use comfy_table::{presets::UTF8_FULL, Cell, Color, Table};

            let mut table = Table::new();
            table.load_preset(UTF8_FULL);
            table.set_header(vec![
                Cell::new("Priority").fg(Color::Green),
                Cell::new("Name").fg(Color::Green),
                Cell::new("URL").fg(Color::Green),
                Cell::new("Status").fg(Color::Green),
            ]);

            for registry in registries {
                let status = if registry.enabled {
                    Cell::new("✓ Enabled").fg(Color::Green)
                } else {
                    Cell::new("✗ Disabled").fg(Color::Red)
                };

                let name = registry.name.unwrap_or_else(|| "-".to_string());

                table.add_row(vec![
                    Cell::new(registry.priority.to_string()),
                    Cell::new(name),
                    Cell::new(registry.url),
                    status,
                ]);
            }

            println!("{table}");
        }
    }

    Ok(())
}

async fn handle_index_api(
    profile: &Option<String>,
    command: PackIndexApiCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    match command {
        PackIndexApiCommands::List => {
            let indices: Vec<PackRegistryIndexResponse> = client.get("/pack-indices").await?;
            print_pack_indices(&indices, output_format)?;
        }
        PackIndexApiCommands::Add {
            url,
            name,
            position,
            disabled,
        } => {
            let request = CreatePackRegistryIndexRequest {
                name,
                url,
                position,
                enabled: !disabled,
                headers: serde_json::json!({}),
            };
            let index: PackRegistryIndexResponse = client.post("/pack-indices", &request).await?;
            match output_format {
                OutputFormat::Json | OutputFormat::Yaml => {
                    output::print_output(&index, output_format)?
                }
                OutputFormat::Table => {
                    output::print_success(&format!("Added pack index {}", index.id));
                    print_pack_indices(&[index], output_format)?;
                }
            }
        }
        PackIndexApiCommands::Update {
            id,
            url,
            name,
            clear_name,
            position,
            enabled,
        } => {
            let request = UpdatePackRegistryIndexRequest {
                name: if clear_name {
                    Some(None)
                } else {
                    name.map(Some)
                },
                url,
                position,
                enabled,
            };
            let index: PackRegistryIndexResponse =
                client.put(&format!("/pack-indices/{id}"), &request).await?;
            match output_format {
                OutputFormat::Json | OutputFormat::Yaml => {
                    output::print_output(&index, output_format)?
                }
                OutputFormat::Table => {
                    output::print_success(&format!("Updated pack index {id}"));
                    print_pack_indices(&[index], output_format)?;
                }
            }
        }
        PackIndexApiCommands::Delete { id } => {
            client
                .delete_no_response(&format!("/pack-indices/{id}"))
                .await?;
            if output_format == OutputFormat::Table {
                output::print_success(&format!("Deleted pack index {id}"));
            }
        }
        PackIndexApiCommands::Browse { query } => {
            let path = if let Some(query) = query {
                format!("/pack-indices/packs?q={}", urlencoding::encode(&query))
            } else {
                "/pack-indices/packs".to_string()
            };
            let packs: Vec<IndexedPackResponse> = client.get(&path).await?;
            print_indexed_packs(&packs, output_format)?;
        }
        PackIndexApiCommands::Show { pack_ref } => {
            let pack: IndexedPackResponse = client
                .get(&format!(
                    "/pack-indices/packs/{}",
                    urlencoding::encode(&pack_ref)
                ))
                .await?;
            match output_format {
                OutputFormat::Json | OutputFormat::Yaml => {
                    output::print_output(&pack, output_format)?
                }
                OutputFormat::Table => print_indexed_pack_detail(&pack),
            }
        }
    }

    Ok(())
}

fn print_pack_indices(
    indices: &[PackRegistryIndexResponse],
    output_format: OutputFormat,
) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&indices.to_vec(), output_format)?
        }
        OutputFormat::Table => {
            use comfy_table::{presets::UTF8_FULL, Cell, Color, Table};
            let mut table = Table::new();
            table.load_preset(UTF8_FULL);
            table.set_header(vec![
                Cell::new("ID").fg(Color::Green),
                Cell::new("Position").fg(Color::Green),
                Cell::new("Name").fg(Color::Green),
                Cell::new("URL").fg(Color::Green),
                Cell::new("Enabled").fg(Color::Green),
            ]);
            for index in indices {
                table.add_row(vec![
                    Cell::new(index.id),
                    Cell::new(index.position),
                    Cell::new(index.name.as_deref().unwrap_or("-")),
                    Cell::new(&index.url),
                    Cell::new(if index.enabled { "yes" } else { "no" }),
                ]);
            }
            println!("{table}");
        }
    }
    Ok(())
}

fn print_indexed_packs(packs: &[IndexedPackResponse], output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&packs.to_vec(), output_format)?
        }
        OutputFormat::Table => {
            use comfy_table::{presets::UTF8_FULL, Cell, Color, Table};
            let mut table = Table::new();
            table.load_preset(UTF8_FULL);
            table.set_header(vec![
                Cell::new("Ref").fg(Color::Green),
                Cell::new("Version").fg(Color::Green),
                Cell::new("Label").fg(Color::Green),
                Cell::new("Install").fg(Color::Green),
                Cell::new("Index").fg(Color::Green),
            ]);
            for indexed in packs {
                let pack = &indexed.pack;
                let methods = pack
                    .get("install_sources")
                    .and_then(|sources| sources.as_array())
                    .map(|sources| {
                        sources
                            .iter()
                            .filter_map(|source| source.get("type").and_then(|v| v.as_str()))
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                table.add_row(vec![
                    Cell::new(pack.get("ref").and_then(|v| v.as_str()).unwrap_or("-")),
                    Cell::new(pack.get("version").and_then(|v| v.as_str()).unwrap_or("-")),
                    Cell::new(pack.get("label").and_then(|v| v.as_str()).unwrap_or("-")),
                    Cell::new(methods),
                    Cell::new(
                        indexed
                            .registry
                            .name
                            .as_deref()
                            .unwrap_or(indexed.registry.url.as_str()),
                    ),
                ]);
            }
            println!("{table}");
            output::print_success(&format!("Found {} indexed pack(s)", packs.len()));
        }
    }
    Ok(())
}

fn print_indexed_pack_detail(indexed: &IndexedPackResponse) {
    let pack = &indexed.pack;
    output::print_section(
        pack.get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("Indexed Pack"),
    );
    output::print_key_value_table(vec![
        (
            "Ref",
            pack.get("ref")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Version",
            pack.get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Description",
            pack.get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Use case",
            pack.get("use_case")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Index",
            indexed
                .registry
                .name
                .as_deref()
                .unwrap_or(indexed.registry.url.as_str())
                .to_string(),
        ),
    ]);
    if let Some(contents) = pack.get("contents").and_then(|v| v.as_object()) {
        output::print_section("Contents");
        for key in ["actions", "sensors", "triggers", "rules", "workflows"] {
            let count = contents
                .get(key)
                .and_then(|value| value.as_array())
                .map(|items| items.len())
                .unwrap_or(0);
            println!("  {key}: {count}");
        }
    }
}

async fn handle_search(
    _profile: &Option<String>,
    keyword: String,
    registry_name: Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    // Load Attune configuration to get registry settings
    let config = attune_common::config::Config::load()?;

    if !config.pack_registry.enabled {
        output::print_error("Pack registry system is disabled in configuration");
        std::process::exit(1);
    }

    // Create registry client
    let client = attune_common::pack_registry::RegistryClient::new(config.pack_registry)?;

    // Search for packs
    let results = if let Some(reg_name) = registry_name {
        // Search specific registry
        output::print_info(&format!(
            "Searching registry '{}' for '{}'...",
            reg_name, keyword
        ));

        // Find all registries with this name and search them
        let mut all_results = Vec::new();
        for registry in client.get_registries() {
            if registry.name.as_deref() == Some(&reg_name) {
                match client.fetch_index(&registry).await {
                    Ok(index) => {
                        let keyword_lower = keyword.to_lowercase();
                        for pack in index.packs {
                            let matches = pack.pack_ref.to_lowercase().contains(&keyword_lower)
                                || pack.label.to_lowercase().contains(&keyword_lower)
                                || pack.description.to_lowercase().contains(&keyword_lower)
                                || pack
                                    .keywords
                                    .iter()
                                    .any(|k| k.to_lowercase().contains(&keyword_lower));

                            if matches {
                                all_results.push((pack, registry.url.clone()));
                            }
                        }
                    }
                    Err(e) => {
                        output::print_error(&format!("Failed to fetch registry: {}", e));
                        std::process::exit(1);
                    }
                }
            }
        }
        all_results
    } else {
        // Search all registries
        output::print_info(&format!("Searching all registries for '{}'...", keyword));
        client.search_packs(&keyword).await?
    };

    if results.is_empty() {
        output::print_warning(&format!("No packs found matching '{}'", keyword));
        return Ok(());
    }

    match output_format {
        OutputFormat::Json => {
            let json_results: Vec<_> = results
                .iter()
                .map(|(pack, registry_url)| {
                    serde_json::json!({
                        "ref": pack.pack_ref,
                        "label": pack.label,
                        "version": pack.version,
                        "description": pack.description,
                        "author": pack.author,
                        "keywords": pack.keywords,
                        "registry": registry_url,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json_results)?);
        }
        OutputFormat::Yaml => {
            let yaml_results: Vec<_> = results
                .iter()
                .map(|(pack, registry_url)| {
                    serde_json::json!({
                        "ref": pack.pack_ref,
                        "label": pack.label,
                        "version": pack.version,
                        "description": pack.description,
                        "author": pack.author,
                        "keywords": pack.keywords,
                        "registry": registry_url,
                    })
                })
                .collect();
            println!("{}", serde_yaml_ng::to_string(&yaml_results)?);
        }
        OutputFormat::Table => {
            use comfy_table::{presets::UTF8_FULL, Cell, Color, Table};

            let mut table = Table::new();
            table.load_preset(UTF8_FULL);
            table.set_header(vec![
                Cell::new("Ref").fg(Color::Green),
                Cell::new("Version").fg(Color::Green),
                Cell::new("Description").fg(Color::Green),
                Cell::new("Author").fg(Color::Green),
            ]);

            for (pack, _) in results.iter() {
                table.add_row(vec![
                    Cell::new(&pack.pack_ref),
                    Cell::new(&pack.version),
                    Cell::new(&pack.description),
                    Cell::new(&pack.author),
                ]);
            }

            println!("{table}");
            output::print_success(&format!("Found {} pack(s)", results.len()));
        }
    }

    Ok(())
}

/// Detect the source type from the provided source string
fn detect_source_type(source: &str, ref_spec: Option<&str>, no_registry: bool) -> &'static str {
    // If no_registry flag is set, skip registry detection
    if no_registry {
        if source.starts_with("http://") || source.starts_with("https://") {
            if source.ends_with(".git") {
                return "git repository";
            } else if source.ends_with(".zip")
                || source.ends_with(".tar.gz")
                || source.ends_with(".tgz")
            {
                return "archive URL";
            }
            return "URL";
        } else if std::path::Path::new(source).exists() {
            if std::path::Path::new(source).is_file() {
                return "local archive";
            }
            return "local directory";
        }
        return "unknown source";
    }

    // Check if it's a URL
    if source.starts_with("http://") || source.starts_with("https://") {
        if source.ends_with(".git") || ref_spec.is_some() {
            return "git repository";
        }
        return "archive URL";
    }

    // Check if it's a local path
    if std::path::Path::new(source).exists() {
        if std::path::Path::new(source).is_file() {
            return "local archive";
        }
        return "local directory";
    }

    // Check if it looks like a git SSH URL
    if source.starts_with("git@") || source.contains("git://") {
        return "git repository";
    }

    // Otherwise assume it's a registry reference
    "registry reference"
}

async fn handle_checksum(path: String, json: bool, output_format: OutputFormat) -> Result<()> {
    use attune_common::pack_registry::{calculate_directory_checksum, calculate_file_checksum};

    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Checksum generation intentionally accepts arbitrary local paths from the CLI operator.
    let path_obj = Path::new(&path);

    if !path_obj.exists() {
        output::print_error(&format!("Path does not exist: {}", path));
        std::process::exit(1);
    }

    // Only print info message in table format
    if output_format == OutputFormat::Table {
        output::print_info(&format!("Calculating checksum for '{}'...", path));
    }

    let checksum = if path_obj.is_dir() {
        calculate_directory_checksum(path_obj)?
    } else if path_obj.is_file() {
        calculate_file_checksum(path_obj)?
    } else {
        output::print_error(&format!("Invalid path type: {}", path));
        std::process::exit(1);
    };

    if json {
        // Output in registry index format
        let install_source = if path_obj.is_file()
            && (path.ends_with(".zip") || path.ends_with(".tar.gz") || path.ends_with(".tgz"))
        {
            serde_json::json!({
                "type": "archive",
                "url": "https://example.com/path/to/pack.zip",
                "checksum": format!("sha256:{}", checksum)
            })
        } else {
            serde_json::json!({
                "type": "git",
                "url": "https://github.com/example/pack",
                "ref": "v1.0.0",
                "checksum": format!("sha256:{}", checksum)
            })
        };

        match output_format {
            OutputFormat::Json => {
                println!("{}", serde_json::to_string_pretty(&install_source)?);
            }
            OutputFormat::Yaml => {
                println!("{}", serde_yaml_ng::to_string(&install_source)?);
            }
            OutputFormat::Table => {
                println!("{}", serde_json::to_string_pretty(&install_source)?);
            }
        }

        // Only print note in table format
        if output_format == OutputFormat::Table {
            output::print_info("\nNote: Update the URL and ref fields with actual values");
        }
    } else {
        // Simple output
        match output_format {
            OutputFormat::Json => {
                let result = serde_json::json!({
                    "path": path,
                    "checksum": format!("sha256:{}", checksum)
                });
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
            OutputFormat::Yaml => {
                let result = serde_json::json!({
                    "path": path,
                    "checksum": format!("sha256:{}", checksum)
                });
                println!("{}", serde_yaml_ng::to_string(&result)?);
            }
            OutputFormat::Table => {
                println!("\nChecksum for: {}", path);
                println!("Algorithm: SHA256");
                println!("Hash: {}", checksum);
                println!("\nFormatted: sha256:{}", checksum);
                output::print_success("✓ Checksum calculated successfully");
            }
        }
    }

    Ok(())
}

async fn handle_index_entry(
    _profile: &Option<String>,
    path: String,
    git_url: Option<String>,
    git_ref: Option<String>,
    archive_url: Option<String>,
    _format: String,
    output_format: OutputFormat,
) -> Result<()> {
    use attune_common::pack_registry::calculate_directory_checksum;

    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Index-entry generation intentionally inspects a local pack directory chosen by the CLI operator.
    let path_obj = Path::new(&path);

    if !path_obj.exists() {
        output::print_error(&format!("Path does not exist: {}", path));
        std::process::exit(1);
    }

    if !path_obj.is_dir() {
        output::print_error(&format!("Path is not a directory: {}", path));
        std::process::exit(1);
    }

    // Look for pack.yaml
    let pack_yaml_path = path_obj.join("pack.yaml");
    if !pack_yaml_path.exists() {
        output::print_error(&format!("pack.yaml not found in: {}", path));
        std::process::exit(1);
    }

    // Only print info message in table format
    if output_format == OutputFormat::Table {
        output::print_info("Parsing pack.yaml...");
    }

    // Read and parse pack.yaml
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Reading local pack metadata for index generation is expected CLI behavior.
    let pack_yaml_content = std::fs::read_to_string(&pack_yaml_path)?;
    let pack_yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&pack_yaml_content)?;

    // Extract metadata
    let pack_ref = pack_yaml["ref"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing 'ref' field in pack.yaml"))?;
    let label = pack_yaml["label"].as_str().unwrap_or(pack_ref);
    let description = pack_yaml["description"].as_str().unwrap_or("");
    let version = pack_yaml["version"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing 'version' field in pack.yaml"))?;
    let author = pack_yaml["author"].as_str().unwrap_or("Unknown");
    let email = pack_yaml["email"].as_str();
    let homepage = pack_yaml["homepage"].as_str();
    let repository = pack_yaml["repository"].as_str();
    let license = pack_yaml["license"].as_str().unwrap_or("UNLICENSED");

    // Extract keywords
    let keywords: Vec<String> = pack_yaml["keywords"]
        .as_sequence()
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // Extract runtime dependencies
    let runtime_deps: Vec<String> = pack_yaml["runtime_deps"]
        .as_sequence()
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // Only print info message in table format
    if output_format == OutputFormat::Table {
        output::print_info("Calculating checksum...");
    }
    let checksum = calculate_directory_checksum(path_obj)?;

    // Build install sources
    let mut install_sources = Vec::new();

    if let Some(ref git) = git_url {
        let default_ref = format!("v{}", version);
        let ref_value = git_ref.as_deref().unwrap_or(&default_ref);
        let git_source = serde_json::json!({
            "type": "git",
            "url": git,
            "ref": ref_value,
            "checksum": format!("sha256:{}", checksum)
        });
        install_sources.push(git_source);
    }

    if let Some(ref archive) = archive_url {
        let archive_source = serde_json::json!({
            "type": "archive",
            "url": archive,
            "checksum": format!("sha256:{}", checksum)
        });
        install_sources.push(archive_source);
    }

    // If no sources provided, generate templates
    if install_sources.is_empty() {
        output::print_warning("No git-url or archive-url provided. Generating templates...");
        install_sources.push(serde_json::json!({
            "type": "git",
            "url": format!("https://github.com/your-org/{}", pack_ref),
            "ref": format!("v{}", version),
            "checksum": format!("sha256:{}", checksum)
        }));
    }

    // Count components
    let actions_count = pack_yaml["actions"]
        .as_mapping()
        .map(|m| m.len())
        .unwrap_or(0);
    let sensors_count = pack_yaml["sensors"]
        .as_mapping()
        .map(|m| m.len())
        .unwrap_or(0);
    let triggers_count = pack_yaml["triggers"]
        .as_mapping()
        .map(|m| m.len())
        .unwrap_or(0);

    // Build index entry
    let mut index_entry = serde_json::json!({
        "ref": pack_ref,
        "label": label,
        "description": description,
        "version": version,
        "author": author,
        "license": license,
        "keywords": keywords,
        "runtime_deps": runtime_deps,
        "install_sources": install_sources,
        "contents": {
            "actions": actions_count,
            "sensors": sensors_count,
            "triggers": triggers_count,
            "rules": 0,
            "workflows": 0
        }
    });

    // Add optional fields
    if let Some(e) = email {
        index_entry["email"] = serde_json::Value::String(e.to_string());
    }
    if let Some(h) = homepage {
        index_entry["homepage"] = serde_json::Value::String(h.to_string());
    }
    if let Some(r) = repository {
        index_entry["repository"] = serde_json::Value::String(r.to_string());
    }

    // Output
    match output_format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&index_entry)?);
        }
        OutputFormat::Yaml => {
            println!("{}", serde_yaml_ng::to_string(&index_entry)?);
        }
        OutputFormat::Table => {
            println!("\n{}", serde_json::to_string_pretty(&index_entry)?);
        }
    }

    // Only print success message in table format
    if output_format == OutputFormat::Table {
        output::print_success("✓ Index entry generated successfully");

        if git_url.is_none() && archive_url.is_none() {
            output::print_info(
                "\nNote: Update the install source URLs before adding to your registry index",
            );
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_update(
    profile: &Option<String>,
    pack_ref: String,
    label: Option<String>,
    description: Option<String>,
    version: Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    // Check that at least one field is provided
    if label.is_none() && description.is_none() && version.is_none() {
        anyhow::bail!("At least one field must be provided to update");
    }

    #[derive(Serialize)]
    #[serde(tag = "op", content = "value", rename_all = "snake_case")]
    enum PackDescriptionPatch {
        Set(String),
    }

    #[derive(Serialize)]
    struct UpdatePackRequest {
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<PackDescriptionPatch>,
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    }

    let request = UpdatePackRequest {
        label,
        description: description.map(PackDescriptionPatch::Set),
        version,
    };

    let path = format!("/packs/{}", pack_ref);
    let pack: PackDetail = client.put(&path, &request).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&pack, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!("Pack '{}' updated successfully", pack.pack_ref));
            output::print_key_value_table(vec![
                ("Ref", pack.pack_ref.clone()),
                ("Label", pack.label.clone()),
                ("Version", pack.version.clone()),
                ("Updated", output::format_timestamp(&pack.updated)),
            ]);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_label_from_ref_underscores() {
        assert_eq!(label_from_ref("my_cool_pack"), "My Cool Pack");
    }

    #[test]
    fn test_label_from_ref_hyphens() {
        assert_eq!(label_from_ref("my-cool-pack"), "My Cool Pack");
    }

    #[test]
    fn test_label_from_ref_dots() {
        assert_eq!(label_from_ref("my.cool.pack"), "My Cool Pack");
    }

    #[test]
    fn test_label_from_ref_mixed_separators() {
        assert_eq!(label_from_ref("my_cool-pack.v2"), "My Cool Pack V2");
    }

    #[test]
    fn test_label_from_ref_single_word() {
        assert_eq!(label_from_ref("slack"), "Slack");
    }

    #[test]
    fn test_label_from_ref_already_capitalized() {
        assert_eq!(label_from_ref("AWS"), "AWS");
    }

    #[test]
    fn test_label_from_ref_empty() {
        assert_eq!(label_from_ref(""), "");
    }

    #[test]
    fn test_label_from_ref_consecutive_separators() {
        assert_eq!(label_from_ref("my__pack"), "My Pack");
    }
}
