use anyhow::Result;
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::path::Path;

use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Subcommand)]
pub enum ArtifactCommands {
    /// List artifacts with optional filters
    List {
        /// Filter by owner scope type (system, identity, pack, action, sensor)
        #[arg(long)]
        scope: Option<String>,

        /// Filter by owner identifier
        #[arg(long)]
        owner: Option<String>,

        /// Filter by artifact type (file_binary, file_datatable, file_image, file_text, other, progress, url)
        #[arg(long, name = "type")]
        artifact_type: Option<String>,

        /// Filter by visibility (public, private)
        #[arg(long)]
        visibility: Option<String>,

        /// Filter by execution ID
        #[arg(long)]
        execution: Option<i64>,

        /// Search by name (case-insensitive substring match)
        #[arg(long)]
        name: Option<String>,

        /// Page number
        #[arg(long, default_value = "1")]
        page: u32,

        /// Items per page
        #[arg(long, default_value = "50")]
        per_page: u32,
    },
    /// Show details of a specific artifact
    Show {
        /// Artifact ID or ref
        artifact: String,
    },
    /// Create a new artifact
    Create {
        /// Artifact reference (unique identifier, e.g. "mypack.build_log")
        #[arg(long)]
        r#ref: String,

        /// Owner scope type (system, identity, pack, action, sensor)
        #[arg(long, default_value = "action")]
        scope: String,

        /// Owner identifier (ref string of the owning entity)
        #[arg(long)]
        owner: String,

        /// Artifact type (file_binary, file_datatable, file_image, file_text, other, progress, url)
        #[arg(long, name = "type", default_value = "file_text")]
        artifact_type: String,

        /// Visibility (public, private)
        #[arg(long)]
        visibility: Option<String>,

        /// Retention policy (versions, days, hours, minutes)
        #[arg(long, default_value = "versions")]
        retention_policy: Option<String>,

        /// Retention limit
        #[arg(long, default_value = "5")]
        retention_limit: Option<i32>,

        /// Human-readable name
        #[arg(long)]
        name: Option<String>,

        /// Description
        #[arg(long)]
        description: Option<String>,

        /// MIME content type
        #[arg(long)]
        content_type: Option<String>,
    },
    /// Delete an artifact
    Delete {
        /// Artifact ID
        id: i64,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Upload a file as a new version of an artifact
    Upload {
        /// Artifact ID
        id: i64,

        /// Path to the file to upload
        file: String,

        /// MIME content type override (auto-detected if omitted)
        #[arg(long)]
        content_type: Option<String>,

        /// Creator identity string
        #[arg(long)]
        created_by: Option<String>,

        /// JSON metadata to attach to the version
        #[arg(long)]
        meta: Option<String>,
    },
    /// Download the latest version of an artifact (or a specific version)
    #[command(disable_version_flag = true)]
    Download {
        /// Artifact ID
        id: i64,

        /// Specific version number to download (latest if omitted)
        #[arg(short = 'V', long = "version")]
        version: Option<i32>,

        /// Output file path (defaults to auto-derived filename or stdout)
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Manage artifact versions
    #[command(subcommand)]
    Version(VersionCommands),
}

#[derive(Subcommand)]
pub enum VersionCommands {
    /// List versions of an artifact
    List {
        /// Artifact ID
        artifact_id: i64,
    },
    /// Show details of a specific version
    Show {
        /// Artifact ID
        artifact_id: i64,

        /// Version number
        version: i32,
    },
    /// Upload a file as a new version
    Upload {
        /// Artifact ID
        artifact_id: i64,

        /// Path to the file to upload
        file: String,

        /// MIME content type override
        #[arg(long)]
        content_type: Option<String>,

        /// Creator identity string
        #[arg(long)]
        created_by: Option<String>,

        /// JSON metadata to attach to the version
        #[arg(long)]
        meta: Option<String>,
    },
    /// Create a JSON content version
    CreateJson {
        /// Artifact ID
        artifact_id: i64,

        /// JSON content (as a string)
        content: String,

        /// MIME content type (defaults to application/json)
        #[arg(long)]
        content_type: Option<String>,

        /// Creator identity string
        #[arg(long)]
        created_by: Option<String>,

        /// JSON metadata to attach to the version
        #[arg(long)]
        meta: Option<String>,
    },
    /// Download a specific version
    #[command(disable_version_flag = true)]
    Download {
        /// Artifact ID
        artifact_id: i64,

        /// Version number
        version: i32,

        /// Output file path (defaults to auto-derived filename or stdout)
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Delete a specific version
    #[command(disable_version_flag = true)]
    Delete {
        /// Artifact ID
        artifact_id: i64,

        /// Version number
        version: i32,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

// ── Response / request types used for (de)serialization against the API ────

#[derive(Debug, Serialize, Deserialize)]
struct ArtifactResponse {
    id: i64,
    #[serde(rename = "ref")]
    artifact_ref: String,
    scope: String,
    owner: String,
    r#type: String,
    visibility: String,
    retention_policy: String,
    retention_limit: i32,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default)]
    data: Option<JsonValue>,
    created: String,
    updated: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ArtifactSummary {
    id: i64,
    #[serde(rename = "ref")]
    artifact_ref: String,
    r#type: String,
    visibility: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    size_bytes: Option<i64>,
    scope: String,
    owner: String,
    created: String,
    updated: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct VersionResponse {
    id: i64,
    artifact: i64,
    version: i32,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default)]
    content_json: Option<JsonValue>,
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    meta: Option<JsonValue>,
    #[serde(default)]
    created_by: Option<String>,
    created: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct VersionSummary {
    id: i64,
    version: i32,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    created_by: Option<String>,
    created: String,
}

#[derive(Debug, Serialize)]
struct CreateArtifactBody {
    r#ref: String,
    scope: String,
    owner: String,
    r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retention_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retention_limit: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateVersionJsonBody {
    content: JsonValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    meta: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_by: Option<String>,
}

// ── Command dispatch ───────────────────────────────────────────────────────

pub async fn handle_artifact_command(
    profile: &Option<String>,
    command: ArtifactCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        ArtifactCommands::List {
            scope,
            owner,
            artifact_type,
            visibility,
            execution,
            name,
            page,
            per_page,
        } => {
            handle_list(
                profile,
                scope,
                owner,
                artifact_type,
                visibility,
                execution,
                name,
                page,
                per_page,
                api_url,
                output_format,
            )
            .await
        }
        ArtifactCommands::Show { artifact } => {
            handle_show(profile, artifact, api_url, output_format).await
        }
        ArtifactCommands::Create {
            r#ref,
            scope,
            owner,
            artifact_type,
            visibility,
            retention_policy,
            retention_limit,
            name,
            description,
            content_type,
        } => {
            handle_create(
                profile,
                r#ref,
                scope,
                owner,
                artifact_type,
                visibility,
                retention_policy,
                retention_limit,
                name,
                description,
                content_type,
                api_url,
                output_format,
            )
            .await
        }
        ArtifactCommands::Delete { id, yes } => {
            handle_delete(profile, id, yes, api_url, output_format).await
        }
        ArtifactCommands::Upload {
            id,
            file,
            content_type,
            created_by,
            meta,
        } => {
            handle_upload(
                profile,
                id,
                file,
                content_type,
                created_by,
                meta,
                api_url,
                output_format,
            )
            .await
        }
        ArtifactCommands::Download {
            id,
            version,
            output,
        } => handle_download(profile, id, version, output, api_url, output_format).await,
        ArtifactCommands::Version(version_cmd) => {
            handle_version_command(profile, version_cmd, api_url, output_format).await
        }
    }
}

async fn handle_version_command(
    profile: &Option<String>,
    command: VersionCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        VersionCommands::List { artifact_id } => {
            handle_version_list(profile, artifact_id, api_url, output_format).await
        }
        VersionCommands::Show {
            artifact_id,
            version,
        } => handle_version_show(profile, artifact_id, version, api_url, output_format).await,
        VersionCommands::Upload {
            artifact_id,
            file,
            content_type,
            created_by,
            meta,
        } => {
            handle_upload(
                profile,
                artifact_id,
                file,
                content_type,
                created_by,
                meta,
                api_url,
                output_format,
            )
            .await
        }
        VersionCommands::CreateJson {
            artifact_id,
            content,
            content_type,
            created_by,
            meta,
        } => {
            handle_version_create_json(
                profile,
                artifact_id,
                content,
                content_type,
                created_by,
                meta,
                api_url,
                output_format,
            )
            .await
        }
        VersionCommands::Download {
            artifact_id,
            version,
            output,
        } => {
            handle_download(
                profile,
                artifact_id,
                Some(version),
                output,
                api_url,
                output_format,
            )
            .await
        }
        VersionCommands::Delete {
            artifact_id,
            version,
            yes,
        } => {
            handle_version_delete(profile, artifact_id, version, yes, api_url, output_format).await
        }
    }
}

// ── Handlers ───────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn handle_list(
    profile: &Option<String>,
    scope: Option<String>,
    owner: Option<String>,
    artifact_type: Option<String>,
    visibility: Option<String>,
    execution: Option<i64>,
    name: Option<String>,
    page: u32,
    per_page: u32,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let mut query_params = vec![format!("page={}", page), format!("per_page={}", per_page)];

    if let Some(s) = scope {
        query_params.push(format!("scope={}", s));
    }
    if let Some(o) = owner {
        query_params.push(format!("owner={}", urlencoding::encode(&o)));
    }
    if let Some(t) = artifact_type {
        query_params.push(format!("type={}", t));
    }
    if let Some(v) = visibility {
        query_params.push(format!("visibility={}", v));
    }
    if let Some(e) = execution {
        query_params.push(format!("execution={}", e));
    }
    if let Some(n) = name {
        query_params.push(format!("name={}", urlencoding::encode(&n)));
    }

    let path = format!("/artifacts?{}", query_params.join("&"));
    let artifacts: Vec<ArtifactSummary> = client.get_paginated(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&artifacts, output_format)?;
        }
        OutputFormat::Table => {
            if artifacts.is_empty() {
                output::print_info("No artifacts found");
            } else {
                let mut table = output::create_table();
                output::add_header(
                    &mut table,
                    vec!["Ref", "Name", "Type", "Visibility", "Size", "Created", "ID"],
                );

                for artifact in &artifacts {
                    table.add_row(vec![
                        artifact.artifact_ref.clone(),
                        artifact.name.clone().unwrap_or_else(|| "-".to_string()),
                        artifact.r#type.clone(),
                        artifact.visibility.clone(),
                        format_size(artifact.size_bytes),
                        output::format_timestamp(&artifact.created),
                        artifact.id.to_string(),
                    ]);
                }

                println!("{}", table);
                output::print_info(&format!("{} artifact(s)", artifacts.len()));
            }
        }
    }

    Ok(())
}

async fn handle_show(
    profile: &Option<String>,
    artifact: String,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    // Try to parse as i64 (ID), otherwise treat as ref
    let path = if let Ok(id) = artifact.parse::<i64>() {
        format!("/artifacts/{}", id)
    } else {
        format!("/artifacts/ref/{}", urlencoding::encode(&artifact))
    };

    let artifact_resp: ArtifactResponse = client.get(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&artifact_resp, output_format)?;
        }
        OutputFormat::Table => {
            output::print_section(&format!("Artifact: {}", artifact_resp.artifact_ref));

            let mut pairs = vec![
                ("Reference", artifact_resp.artifact_ref.clone()),
                ("ID", artifact_resp.id.to_string()),
                (
                    "Name",
                    artifact_resp
                        .name
                        .clone()
                        .unwrap_or_else(|| "-".to_string()),
                ),
                ("Type", artifact_resp.r#type.clone()),
                ("Visibility", artifact_resp.visibility.clone()),
                ("Scope", artifact_resp.scope.clone()),
                ("Owner", artifact_resp.owner.clone()),
                (
                    "Retention",
                    format!(
                        "{} (limit: {})",
                        artifact_resp.retention_policy, artifact_resp.retention_limit
                    ),
                ),
                (
                    "Content Type",
                    artifact_resp
                        .content_type
                        .clone()
                        .unwrap_or_else(|| "-".to_string()),
                ),
                ("Size", format_size(artifact_resp.size_bytes)),
            ];

            if let Some(ref desc) = artifact_resp.description {
                pairs.push(("Description", desc.clone()));
            }

            if let Some(ref data) = artifact_resp.data {
                let data_str =
                    serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string());
                pairs.push(("Data", output::truncate(&data_str, 200)));
            }

            pairs.push(("Created", output::format_timestamp(&artifact_resp.created)));
            pairs.push(("Updated", output::format_timestamp(&artifact_resp.updated)));

            output::print_key_value_table(pairs);
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_create(
    profile: &Option<String>,
    artifact_ref: String,
    scope: String,
    owner: String,
    artifact_type: String,
    visibility: Option<String>,
    retention_policy: Option<String>,
    retention_limit: Option<i32>,
    name: Option<String>,
    description: Option<String>,
    content_type: Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let request = CreateArtifactBody {
        r#ref: artifact_ref,
        scope,
        owner,
        r#type: artifact_type,
        visibility,
        retention_policy,
        retention_limit,
        name,
        description,
        content_type,
    };

    let artifact: ArtifactResponse = client.post("/artifacts", &request).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&artifact, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!(
                "Artifact '{}' created successfully",
                artifact.artifact_ref
            ));
            output::print_key_value_table(vec![
                ("Reference", artifact.artifact_ref.clone()),
                ("ID", artifact.id.to_string()),
                (
                    "Name",
                    artifact.name.clone().unwrap_or_else(|| "-".to_string()),
                ),
                ("Type", artifact.r#type.clone()),
                ("Visibility", artifact.visibility.clone()),
                ("Scope", artifact.scope.clone()),
                ("Owner", artifact.owner.clone()),
                ("Created", output::format_timestamp(&artifact.created)),
            ]);
        }
    }

    Ok(())
}

async fn handle_delete(
    profile: &Option<String>,
    id: i64,
    yes: bool,
    api_url: &Option<String>,
    _output_format: OutputFormat,
) -> Result<()> {
    if !yes {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Delete artifact with ID {}? This cannot be undone",
                id
            ))
            .default(false)
            .interact()?;

        if !confirm {
            output::print_info("Deletion cancelled");
            return Ok(());
        }
    }

    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/artifacts/{}", id);
    client.delete_no_response(&path).await?;

    output::print_success(&format!("Artifact {} deleted successfully", id));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_upload(
    profile: &Option<String>,
    id: i64,
    file: String,
    content_type: Option<String>,
    created_by: Option<String>,
    meta: Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- CLI users explicitly choose a local file to upload; this is not a server-side path sink.
    let file_path = Path::new(&file);
    if !file_path.exists() {
        anyhow::bail!("File not found: {}", file);
    }
    if !file_path.is_file() {
        anyhow::bail!("Not a file: {}", file);
    }

    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- The validated CLI-selected upload path is intentionally read and sent to the API.
    let file_bytes = tokio::fs::read(file_path).await?;
    let file_name = file_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "upload".to_string());

    let mime = content_type
        .clone()
        .unwrap_or_else(|| guess_mime_type(&file_name));

    let mut extra_fields: Vec<(&str, String)> = Vec::new();
    if let Some(ref ct) = content_type {
        extra_fields.push(("content_type", ct.clone()));
    }
    if let Some(ref cb) = created_by {
        extra_fields.push(("created_by", cb.clone()));
    }
    if let Some(ref m) = meta {
        // Validate it's valid JSON
        serde_json::from_str::<JsonValue>(m)
            .map_err(|e| anyhow::anyhow!("Invalid meta JSON: {}", e))?;
        extra_fields.push(("meta", m.clone()));
    }

    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    if output_format == OutputFormat::Table {
        output::print_info(&format!(
            "Uploading '{}' ({}) to artifact {}...",
            file_name,
            format_bytes(file_bytes.len() as u64),
            id,
        ));
    }

    let api_path = format!("/artifacts/{}/versions/upload", id);
    let version: VersionResponse = client
        .multipart_post(
            &api_path,
            "file",
            file_bytes,
            &file_name,
            &mime,
            extra_fields,
        )
        .await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&version, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!(
                "Version {} uploaded successfully",
                version.version
            ));
            output::print_key_value_table(vec![
                ("Version ID", version.id.to_string()),
                ("Version Number", version.version.to_string()),
                ("Artifact ID", version.artifact.to_string()),
                (
                    "Content Type",
                    version
                        .content_type
                        .clone()
                        .unwrap_or_else(|| "-".to_string()),
                ),
                ("Size", format_size(version.size_bytes)),
                (
                    "Created By",
                    version
                        .created_by
                        .clone()
                        .unwrap_or_else(|| "-".to_string()),
                ),
                ("Created", output::format_timestamp(&version.created)),
            ]);
        }
    }

    Ok(())
}

async fn handle_download(
    profile: &Option<String>,
    id: i64,
    version: Option<i32>,
    output_path: Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = match version {
        Some(v) => format!("/artifacts/{}/versions/{}/download", id, v),
        None => format!("/artifacts/{}/download", id),
    };

    let (bytes, content_type, server_filename) = client.download_bytes(&path).await?;

    // Determine output destination
    let dest = if let Some(ref out) = output_path {
        out.clone()
    } else if let Some(ref sf) = server_filename {
        sf.clone()
    } else {
        // Build a default filename
        let ext = extension_from_content_type(&content_type);
        match version {
            Some(v) => format!("artifact_{}_v{}{}", id, v, ext),
            None => format!("artifact_{}_latest{}", id, ext),
        }
    };

    // If output is "-", write to stdout
    if dest == "-" {
        use std::io::Write;
        std::io::stdout().write_all(&bytes)?;
    } else {
        tokio::fs::write(&dest, &bytes).await?;
        if output_format == OutputFormat::Table {
            output::print_success(&format!(
                "Downloaded {} to '{}' ({})",
                match version {
                    Some(v) => format!("version {}", v),
                    None => "latest version".to_string(),
                },
                dest,
                format_bytes(bytes.len() as u64),
            ));
        }
    }

    Ok(())
}

// ── Version subcommand handlers ────────────────────────────────────────────

async fn handle_version_list(
    profile: &Option<String>,
    artifact_id: i64,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/artifacts/{}/versions", artifact_id);
    let versions: Vec<VersionSummary> = client.get(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&versions, output_format)?;
        }
        OutputFormat::Table => {
            if versions.is_empty() {
                output::print_info(&format!("No versions found for artifact {}", artifact_id));
            } else {
                let mut table = output::create_table();
                output::add_header(
                    &mut table,
                    vec![
                        "ID",
                        "Version",
                        "Content Type",
                        "Size",
                        "File Path",
                        "Created By",
                        "Created",
                    ],
                );

                for ver in &versions {
                    table.add_row(vec![
                        ver.id.to_string(),
                        format!("v{}", ver.version),
                        ver.content_type.clone().unwrap_or_else(|| "-".to_string()),
                        format_size(ver.size_bytes),
                        ver.file_path.clone().unwrap_or_else(|| "(db)".to_string()),
                        ver.created_by.clone().unwrap_or_else(|| "-".to_string()),
                        output::format_timestamp(&ver.created),
                    ]);
                }

                println!("{}", table);
                output::print_info(&format!(
                    "{} version(s) for artifact {}",
                    versions.len(),
                    artifact_id,
                ));
            }
        }
    }

    Ok(())
}

async fn handle_version_show(
    profile: &Option<String>,
    artifact_id: i64,
    version: i32,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/artifacts/{}/versions/{}", artifact_id, version);
    let ver: VersionResponse = client.get(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&ver, output_format)?;
        }
        OutputFormat::Table => {
            output::print_section(&format!(
                "Version {} of Artifact {}",
                ver.version, artifact_id
            ));

            let mut pairs = vec![
                ("Version ID", ver.id.to_string()),
                ("Version Number", format!("v{}", ver.version)),
                ("Artifact ID", ver.artifact.to_string()),
                (
                    "Content Type",
                    ver.content_type.clone().unwrap_or_else(|| "-".to_string()),
                ),
                ("Size", format_size(ver.size_bytes)),
            ];

            if let Some(ref fp) = ver.file_path {
                pairs.push(("File Path", fp.clone()));
            } else {
                pairs.push(("Storage", "Database".to_string()));
            }

            if let Some(ref cj) = ver.content_json {
                let json_str = serde_json::to_string_pretty(cj).unwrap_or_else(|_| cj.to_string());
                pairs.push(("JSON Content", output::truncate(&json_str, 300)));
            }

            if let Some(ref meta) = ver.meta {
                let meta_str =
                    serde_json::to_string_pretty(meta).unwrap_or_else(|_| meta.to_string());
                pairs.push(("Metadata", output::truncate(&meta_str, 200)));
            }

            pairs.push((
                "Created By",
                ver.created_by.clone().unwrap_or_else(|| "-".to_string()),
            ));
            pairs.push(("Created", output::format_timestamp(&ver.created)));

            output::print_key_value_table(pairs);
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_version_create_json(
    profile: &Option<String>,
    artifact_id: i64,
    content: String,
    content_type: Option<String>,
    created_by: Option<String>,
    meta: Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let content_json: JsonValue = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Invalid JSON content: {}", e))?;

    let meta_json: Option<JsonValue> = meta
        .map(|m| serde_json::from_str(&m).map_err(|e| anyhow::anyhow!("Invalid meta JSON: {}", e)))
        .transpose()?;

    let body = CreateVersionJsonBody {
        content: content_json,
        content_type,
        meta: meta_json,
        created_by,
    };

    let path = format!("/artifacts/{}/versions", artifact_id);
    let version: VersionResponse = client.post(&path, &body).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&version, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!(
                "JSON version {} created successfully",
                version.version
            ));
            output::print_key_value_table(vec![
                ("Version ID", version.id.to_string()),
                ("Version Number", format!("v{}", version.version)),
                ("Artifact ID", version.artifact.to_string()),
                (
                    "Content Type",
                    version
                        .content_type
                        .clone()
                        .unwrap_or_else(|| "application/json".to_string()),
                ),
                ("Size", format_size(version.size_bytes)),
                ("Created", output::format_timestamp(&version.created)),
            ]);
        }
    }

    Ok(())
}

async fn handle_version_delete(
    profile: &Option<String>,
    artifact_id: i64,
    version: i32,
    yes: bool,
    api_url: &Option<String>,
    _output_format: OutputFormat,
) -> Result<()> {
    if !yes {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Delete version {} of artifact {}? This cannot be undone",
                version, artifact_id
            ))
            .default(false)
            .interact()?;

        if !confirm {
            output::print_info("Deletion cancelled");
            return Ok(());
        }
    }

    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/artifacts/{}/versions/{}", artifact_id, version);
    client.delete_no_response(&path).await?;

    output::print_success(&format!(
        "Version {} of artifact {} deleted successfully",
        version, artifact_id
    ));
    Ok(())
}

// ── Utility functions ──────────────────────────────────────────────────────

/// Format an optional byte count for display
fn format_size(size_bytes: Option<i64>) -> String {
    match size_bytes {
        Some(b) => format_bytes(b as u64),
        None => "-".to_string(),
    }
}

/// Format a byte count as a human-readable string
fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Guess MIME type from file extension
fn guess_mime_type(filename: &str) -> String {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "txt" | "log" => "text/plain",
        "json" => "application/json",
        "yaml" | "yml" => "application/x-yaml",
        "xml" => "application/xml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "tar" => "application/x-tar",
        "csv" => "text/csv",
        "py" => "text/x-python",
        "rs" => "text/x-rust",
        "sh" => "text/x-shellscript",
        "md" => "text/markdown",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Derive a file extension from a content type
fn extension_from_content_type(ct: &str) -> String {
    // Strip parameters (e.g. "; charset=utf-8")
    let base = ct.split(';').next().unwrap_or(ct).trim();

    match base {
        "text/plain" => ".txt",
        "application/json" => ".json",
        "application/x-yaml" | "text/yaml" => ".yaml",
        "application/xml" | "text/xml" => ".xml",
        "text/html" => ".html",
        "text/css" => ".css",
        "application/javascript" => ".js",
        "image/png" => ".png",
        "image/jpeg" => ".jpg",
        "image/gif" => ".gif",
        "image/svg+xml" => ".svg",
        "application/pdf" => ".pdf",
        "application/zip" => ".zip",
        "application/gzip" => ".gz",
        "text/csv" => ".csv",
        "text/markdown" => ".md",
        _ => ".bin",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1048576), "1.0 MB");
        assert_eq!(format_bytes(1073741824), "1.00 GB");
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(None), "-");
        assert_eq!(format_size(Some(1024)), "1.0 KB");
    }

    #[test]
    fn test_guess_mime_type() {
        assert_eq!(guess_mime_type("test.txt"), "text/plain");
        assert_eq!(guess_mime_type("data.json"), "application/json");
        assert_eq!(guess_mime_type("image.png"), "image/png");
        assert_eq!(guess_mime_type("archive.tar"), "application/x-tar");
        assert_eq!(guess_mime_type("noext"), "application/octet-stream");
    }

    #[test]
    fn test_extension_from_content_type() {
        assert_eq!(extension_from_content_type("text/plain"), ".txt");
        assert_eq!(
            extension_from_content_type("text/plain; charset=utf-8"),
            ".txt"
        );
        assert_eq!(extension_from_content_type("application/json"), ".json");
        assert_eq!(
            extension_from_content_type("application/octet-stream"),
            ".bin"
        );
    }
}
