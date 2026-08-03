use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

const MAX_MULTI_LOOKUP_IDS: usize = 1_000;
const MAX_SCAN_PAGE_SIZE: u32 = 1_000;
const DEFAULT_UPLOAD_CHUNK_RECORDS: usize = 1_000;
const MAX_UPLOAD_CHUNK_RECORDS: usize = 10_000;
const DEFAULT_UPLOAD_CHUNK_BYTES: usize = 4 * 1024 * 1024;
const MAX_NDJSON_LINE_BYTES: usize = 1_048_576;

static REFRESH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheOutput {
    Standard,
    Ndjson,
}

#[derive(Subcommand)]
pub enum CacheCommands {
    /// Manage owner-scoped cache namespaces
    Namespace {
        #[command(subcommand)]
        command: CacheNamespaceCommands,
    },
    /// Read published cache entries
    Entry {
        #[command(subcommand)]
        command: CacheEntryCommands,
    },
    /// Inspect immutable cache generations
    Generation {
        #[command(subcommand)]
        command: CacheGenerationCommands,
    },
    /// Build and publish a cache generation
    Refresh {
        #[command(subcommand)]
        command: CacheRefreshCommands,
    },
}

#[derive(Subcommand)]
pub enum CacheNamespaceCommands {
    /// List namespaces for one explicit owner scope
    List {
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        /// Case-insensitive namespace substring
        #[arg(long)]
        namespace: Option<String>,
        /// Active-generation freshness filter
        #[arg(long, value_enum)]
        freshness: Option<CacheNamespaceFreshness>,
        /// Maximum namespaces in this page
        #[arg(long, value_parser = parse_metadata_page_size)]
        limit: Option<u32>,
        /// Opaque cursor returned by an earlier list request
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Create a namespace. Owner and namespace cannot later be changed.
    Create {
        /// Namespace name
        namespace: String,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        #[command(flatten)]
        policy: NamespacePolicyArgs,
    },
    /// Show namespace metadata without reading cache entries
    Show {
        /// Namespace name
        namespace: String,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
    },
    /// Update mutable namespace policy fields
    Update {
        /// Namespace name
        namespace: String,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        #[command(flatten)]
        policy: NamespacePolicyArgs,
    },
    /// Delete a namespace and make its generations unavailable
    Delete {
        /// Namespace name
        namespace: String,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        /// Skip the interactive confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum CacheEntryCommands {
    /// Retrieve one entry from the active generation
    Get {
        /// Namespace name
        namespace: String,
        /// Opaque external record ID
        external_id: String,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        /// Read this retained generation instead of the active generation
        #[arg(long)]
        generation: Option<i64>,
    },
    /// Retrieve a bounded set of entries without putting IDs in a URL
    GetMany {
        /// Namespace name
        namespace: String,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        /// External ID to retrieve (may be repeated)
        #[arg(long = "external-id")]
        external_ids: Vec<String>,
        /// Newline-delimited external IDs, or '-' to read stdin
        #[arg(long = "external-id-file")]
        external_id_file: Option<String>,
        /// Read this retained generation instead of the active generation
        #[arg(long)]
        generation: Option<i64>,
    },
    /// Scan one immutable-generation page by default
    Scan {
        /// Namespace name
        namespace: String,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        /// Read this retained generation instead of resolving the active one
        #[arg(long)]
        generation: Option<i64>,
        /// Opaque cursor returned by an earlier scan
        #[arg(long)]
        cursor: Option<String>,
        /// Maximum entries in a single page (at most 1000)
        #[arg(long, default_value_t = 100, value_parser = parse_scan_page_size)]
        page_size: u32,
        /// Include full entry values in table output
        #[arg(long)]
        include_values: bool,
        /// Traverse every page; requires --output ndjson
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
pub enum CacheGenerationCommands {
    /// List generations for a namespace
    List {
        /// Namespace name
        namespace: String,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        /// Maximum generations in this page
        #[arg(long, value_parser = parse_metadata_page_size)]
        limit: Option<u32>,
        /// Opaque cursor returned by an earlier list request
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Show one generation and its validation state
    Show {
        /// Namespace name
        namespace: String,
        /// Generation ID
        generation_id: i64,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
    },
}

#[derive(Subcommand)]
pub enum CacheRefreshCommands {
    /// Begin an idempotent staging generation
    Begin {
        /// Namespace name
        namespace: String,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        /// Stable producer-generated ID used to retry this begin request
        #[arg(long)]
        client_refresh_id: Option<String>,
        /// Number of chunks expected before sealing
        #[arg(long)]
        expected_chunk_count: i32,
        /// Optional expected total record count
        #[arg(long)]
        expected_count: Option<i64>,
        /// Optional expected total encoded byte count
        #[arg(long)]
        expected_bytes: Option<i64>,
        /// Upstream source revision
        #[arg(long)]
        source_revision: Option<String>,
        /// Active generation observed before this refresh began
        #[arg(
            long,
            conflicts_with = "expect_empty",
            required_unless_present = "expect_empty"
        )]
        expected_active: Option<i64>,
        /// Assert that this is the namespace's first publication
        #[arg(long, conflicts_with = "expected_active")]
        expect_empty: bool,
    },
    /// Upload one bounded newline-delimited JSON chunk
    Upload {
        /// Namespace name
        namespace: String,
        /// Staging generation ID
        generation_id: i64,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        /// Zero-based chunk index
        #[arg(long)]
        chunk_index: i32,
        /// NDJSON file containing at most one bounded chunk, or '-' for stdin
        #[arg(long)]
        file: String,
    },
    /// Validate a fully uploaded staging generation
    Seal {
        /// Namespace name
        namespace: String,
        /// Staging generation ID
        generation_id: i64,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        /// Chunk count declared when the generation was begun
        #[arg(long)]
        expected_chunk_count: i32,
        /// Record count declared when the generation was begun
        #[arg(long)]
        expected_count: Option<i64>,
        /// Encoded byte count declared when the generation was begun
        #[arg(long)]
        expected_bytes: Option<i64>,
    },
    /// Atomically publish a ready generation
    Promote {
        /// Namespace name
        namespace: String,
        /// Ready generation ID
        generation_id: i64,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        /// Active generation observed before this refresh began
        #[arg(
            long,
            conflicts_with = "expect_empty",
            required_unless_present = "expect_empty"
        )]
        expected_active: Option<i64>,
        /// Assert that this is the namespace's first publication
        #[arg(long, conflicts_with = "expected_active")]
        expect_empty: bool,
    },
    /// Abandon a staging generation
    Abort {
        /// Namespace name
        namespace: String,
        /// Staging generation ID
        generation_id: i64,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        /// Skip the interactive confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Stream an NDJSON input through begin, upload, seal, and promote
    Apply {
        /// Namespace name
        namespace: String,
        #[command(flatten)]
        owner: OwnerSelectorArgs,
        /// NDJSON input path, or '-' for stdin
        #[arg(long)]
        input: String,
        /// Stable producer-generated ID used to retry begin and chunk uploads
        #[arg(long)]
        client_refresh_id: Option<String>,
        /// Number of records in each upload request (at most 10000)
        #[arg(long, default_value_t = DEFAULT_UPLOAD_CHUNK_RECORDS, value_parser = parse_chunk_records)]
        chunk_records: usize,
        /// Required only when --input - is used because stdin cannot be replayed
        #[arg(long)]
        expected_chunk_count: Option<i32>,
        /// Upstream source revision
        #[arg(long)]
        source_revision: Option<String>,
        /// Active generation observed before this refresh began
        #[arg(
            long,
            conflicts_with = "expect_empty",
            required_unless_present = "expect_empty"
        )]
        expected_active: Option<i64>,
        /// Assert that this is the namespace's first publication
        #[arg(long, conflicts_with = "expected_active")]
        expect_empty: bool,
    },
}

#[derive(Debug, Clone, Args)]
pub struct OwnerSelectorArgs {
    /// Owner type: system, identity, pack, action, or sensor
    #[arg(long, value_enum)]
    owner_type: CacheOwnerType,
    /// Pack reference (required with --owner-type pack)
    #[arg(long)]
    owner_pack_ref: Option<String>,
    /// Action reference (required with --owner-type action)
    #[arg(long)]
    owner_action_ref: Option<String>,
    /// Sensor reference (required with --owner-type sensor)
    #[arg(long)]
    owner_sensor_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
enum CacheOwnerType {
    System,
    Identity,
    Pack,
    Action,
    Sensor,
}

#[derive(Debug, Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheNamespaceFreshness {
    Fresh,
    Stale,
    Unpopulated,
}

impl CacheNamespaceFreshness {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Unpopulated => "unpopulated",
        }
    }
}

impl CacheOwnerType {
    fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Identity => "identity",
            Self::Pack => "pack",
            Self::Action => "action",
            Self::Sensor => "sensor",
        }
    }
}

#[derive(Debug, Clone, Args, Default)]
pub struct NamespacePolicyArgs {
    /// Desired freshness interval in seconds
    #[arg(long)]
    freshness_target_seconds: Option<i64>,
    /// Maximum records allowed in one generation
    #[arg(long)]
    max_records_per_generation: Option<i64>,
    /// Maximum bytes allowed in one generation
    #[arg(long)]
    max_generation_bytes: Option<i64>,
    /// Maximum retained bytes across generations
    #[arg(long)]
    max_retained_bytes: Option<i64>,
    /// Number of retained generations
    #[arg(long)]
    max_retained_generations: Option<i32>,
    /// Maximum concurrent staging generations
    #[arg(long)]
    max_staging_generations: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
struct OwnerSelectorRequest {
    owner_type: CacheOwnerType,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_ref: Option<String>,
}

impl OwnerSelectorArgs {
    fn request(&self) -> Result<OwnerSelectorRequest> {
        let set_count = [
            self.owner_pack_ref.is_some(),
            self.owner_action_ref.is_some(),
            self.owner_sensor_ref.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();

        let valid = match self.owner_type {
            CacheOwnerType::System => set_count == 0,
            CacheOwnerType::Identity => set_count == 0,
            CacheOwnerType::Pack => self.owner_pack_ref.is_some() && set_count == 1,
            CacheOwnerType::Action => self.owner_action_ref.is_some() && set_count == 1,
            CacheOwnerType::Sensor => self.owner_sensor_ref.is_some() && set_count == 1,
        };

        if !valid {
            let required = match self.owner_type {
                CacheOwnerType::System => "no owner reference",
                CacheOwnerType::Identity => "no owner reference (the current identity is used)",
                CacheOwnerType::Pack => "--owner-pack-ref",
                CacheOwnerType::Action => "--owner-action-ref",
                CacheOwnerType::Sensor => "--owner-sensor-ref",
            };
            anyhow::bail!(
                "--owner-type {} requires exactly {}",
                self.owner_type.as_str(),
                required
            );
        }

        Ok(OwnerSelectorRequest {
            owner_type: self.owner_type,
            owner_ref: match self.owner_type {
                CacheOwnerType::System | CacheOwnerType::Identity => None,
                CacheOwnerType::Pack => self.owner_pack_ref.clone(),
                CacheOwnerType::Action => self.owner_action_ref.clone(),
                CacheOwnerType::Sensor => self.owner_sensor_ref.clone(),
            },
        })
    }

    fn query(&self) -> Result<String> {
        let request = self.request()?;
        let mut query = format!("owner_type={}", request.owner_type.as_str());
        if let Some(owner_ref) = request.owner_ref {
            query.push_str("&owner_ref=");
            query.push_str(&urlencoding::encode(&owner_ref));
        }
        Ok(query)
    }
}

#[derive(Debug, Serialize)]
struct CreateNamespaceRequest {
    #[serde(flatten)]
    owner: OwnerSelectorRequest,
    namespace: String,
    #[serde(flatten)]
    policy: NamespacePolicyRequest,
}

#[derive(Debug, Serialize, Default)]
struct NamespacePolicyRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    freshness_target_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_records_per_generation: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_generation_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_retained_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_retained_generations: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_staging_generations: Option<i32>,
}

impl NamespacePolicyArgs {
    fn request(&self) -> NamespacePolicyRequest {
        NamespacePolicyRequest {
            freshness_target_seconds: self.freshness_target_seconds,
            max_records_per_generation: self.max_records_per_generation,
            max_generation_bytes: self.max_generation_bytes,
            max_retained_bytes: self.max_retained_bytes,
            max_retained_generations: self.max_retained_generations,
            max_staging_generations: self.max_staging_generations,
        }
    }

    fn has_values(&self) -> bool {
        self.freshness_target_seconds.is_some()
            || self.max_records_per_generation.is_some()
            || self.max_generation_bytes.is_some()
            || self.max_retained_bytes.is_some()
            || self.max_retained_generations.is_some()
            || self.max_staging_generations.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheNamespaceResponse {
    id: i64,
    owner_type: String,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    owner_ref: Option<String>,
    #[serde(default)]
    owner_identity: Option<i64>,
    #[serde(default)]
    owner_pack_ref: Option<String>,
    #[serde(default)]
    owner_action_ref: Option<String>,
    #[serde(default)]
    owner_sensor_ref: Option<String>,
    namespace: String,
    #[serde(default, alias = "active_generation_id")]
    active_generation: Option<i64>,
    #[serde(default)]
    freshness_target_seconds: Option<i64>,
    #[serde(default)]
    max_records_per_generation: Option<i64>,
    #[serde(default)]
    max_generation_bytes: Option<i64>,
    #[serde(default)]
    max_retained_bytes: Option<i64>,
    #[serde(default)]
    max_retained_generations: Option<i32>,
    #[serde(default)]
    max_staging_generations: Option<i32>,
    #[serde(default)]
    stale: Option<bool>,
    #[serde(default)]
    refresh_health: Option<String>,
    #[serde(default)]
    record_count: Option<i64>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    updated: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheGenerationResponse {
    #[serde(rename = "generation_id")]
    id: i64,
    #[serde(rename = "namespace_id")]
    namespace: i64,
    #[serde(rename = "status")]
    state: String,
    client_refresh_id: String,
    expected_active_generation_id: Option<i64>,
    expected_chunk_count: i32,
    #[serde(rename = "expected_record_count")]
    expected_count: Option<i64>,
    #[serde(rename = "expected_size_bytes")]
    expected_bytes: Option<i64>,
    record_count: i64,
    size_bytes: i64,
    checksum_algorithm: Option<String>,
    checksum: Option<String>,
    source_revision: Option<String>,
    created_by: Option<i64>,
    created: String,
    sealed: Option<String>,
    activated: Option<String>,
    retired: Option<String>,
    readable_until: Option<String>,
    failed: Option<String>,
    failure_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntryResponse {
    external_id: String,
    value: JsonValue,
    source_updated_at: Option<String>,
    source_checksum: Option<String>,
    size_bytes: i64,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CacheList<T> {
    Items {
        items: Vec<T>,
        #[serde(default)]
        next_cursor: Option<String>,
    },
    Namespaces {
        namespaces: Vec<T>,
        #[serde(default)]
        next_cursor: Option<String>,
    },
    Generations {
        generations: Vec<T>,
        #[serde(default)]
        next_cursor: Option<String>,
    },
    Entries {
        entries: Vec<T>,
        #[serde(default)]
        next_cursor: Option<String>,
    },
    Direct(Vec<T>),
}

impl<T> CacheList<T> {
    fn into_parts(self) -> (Vec<T>, Option<String>) {
        match self {
            Self::Items { items, next_cursor } => (items, next_cursor),
            Self::Namespaces {
                namespaces,
                next_cursor,
            } => (namespaces, next_cursor),
            Self::Generations {
                generations,
                next_cursor,
            } => (generations, next_cursor),
            Self::Entries {
                entries,
                next_cursor,
            } => (entries, next_cursor),
            Self::Direct(items) => (items, None),
        }
    }
}

#[derive(Debug, Serialize)]
struct SealRefreshRequest {
    expected_chunk_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_record_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_size_bytes: Option<i64>,
}

#[derive(Debug, Serialize)]
struct EntryLookupRequest {
    external_ids: Vec<String>,
    generation_id: Option<i64>,
    require_fresh: bool,
}

#[derive(Debug, Serialize)]
struct EntryPointLookupRequest {
    external_id: String,
    generation_id: Option<i64>,
    require_fresh: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct EntryPointLookupResponse {
    generation_id: i64,
    item: Option<CacheEntryResponse>,
    stale: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct EntryLookupResponse {
    generation_id: i64,
    items: Vec<CacheEntryResponse>,
    missing_external_ids: Vec<String>,
    stale: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct CacheScanResponse {
    generation_id: i64,
    items: Vec<CacheEntryResponse>,
    next_cursor: Option<String>,
    cursor_expires_at: Option<String>,
    record_count: Option<i64>,
    stale: bool,
}

#[derive(Debug, Serialize)]
struct BeginRefreshRequest {
    client_refresh_id: String,
    expected_active_generation_id: Option<i64>,
    expected_chunk_count: i32,
    #[serde(
        rename = "expected_record_count",
        skip_serializing_if = "Option::is_none"
    )]
    expected_count: Option<i64>,
    #[serde(
        rename = "expected_size_bytes",
        skip_serializing_if = "Option::is_none"
    )]
    expected_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_revision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UploadEntry {
    external_id: String,
    value: JsonValue,
    #[serde(default)]
    source_updated_at: Option<String>,
    #[serde(default)]
    source_checksum: Option<String>,
}

#[derive(Debug, Serialize)]
struct UploadChunkRequest {
    entries: Vec<UploadEntry>,
}

#[derive(Debug, Serialize)]
struct PromoteRequest {
    expected_active_generation_id: Option<i64>,
}

pub async fn handle_cache_command(
    profile: &Option<String>,
    command: CacheCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
    cache_output: CacheOutput,
) -> Result<()> {
    match command {
        CacheCommands::Namespace { command } => {
            reject_ndjson(cache_output)?;
            handle_namespace(profile, command, api_url, output_format).await
        }
        CacheCommands::Entry { command } => {
            handle_entry(profile, command, api_url, output_format, cache_output).await
        }
        CacheCommands::Generation { command } => {
            reject_ndjson(cache_output)?;
            handle_generation(profile, command, api_url, output_format).await
        }
        CacheCommands::Refresh { command } => {
            reject_ndjson(cache_output)?;
            handle_refresh(profile, command, api_url, output_format).await
        }
    }
}

fn reject_ndjson(cache_output: CacheOutput) -> Result<()> {
    if matches!(cache_output, CacheOutput::Ndjson) {
        anyhow::bail!("--output ndjson is only supported by 'attune cache entry scan --all'");
    }
    Ok(())
}

async fn handle_namespace(
    profile: &Option<String>,
    command: CacheNamespaceCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let mut client = configured_client(profile, api_url)?;
    match command {
        CacheNamespaceCommands::List {
            owner,
            namespace,
            freshness,
            limit,
            cursor,
        } => {
            let mut path = format!("/cache/namespaces?{}", owner.query()?);
            if let Some(namespace) = namespace {
                path.push_str("&namespace=");
                path.push_str(&urlencoding::encode(&namespace));
            }
            if let Some(freshness) = freshness {
                path.push_str("&freshness=");
                path.push_str(freshness.as_str());
            }
            if let Some(limit) = limit {
                path.push_str(&format!("&limit={limit}"));
            }
            if let Some(cursor) = cursor {
                path.push_str("&cursor=");
                path.push_str(&urlencoding::encode(&cursor));
            }
            let namespaces: CacheList<CacheNamespaceResponse> = client.cache_get(&path).await?;
            let (namespaces, next_cursor) = namespaces.into_parts();
            print_namespaces(namespaces, next_cursor, output_format)
        }
        CacheNamespaceCommands::Create {
            namespace,
            owner,
            policy,
        } => {
            let request = CreateNamespaceRequest {
                owner: owner.request()?,
                namespace,
                policy: policy.request(),
            };
            let created: CacheNamespaceResponse =
                client.cache_post("/cache/namespaces", &request).await?;
            print_namespace(&created, output_format, Some("created"))
        }
        CacheNamespaceCommands::Show { namespace, owner } => {
            let path = namespace_query_path(&namespace, &owner)?;
            let namespace: CacheNamespaceResponse = client.cache_get(&path).await?;
            print_namespace(&namespace, output_format, None)
        }
        CacheNamespaceCommands::Update {
            namespace,
            owner,
            policy,
        } => {
            if !policy.has_values() {
                anyhow::bail!("Provide at least one mutable policy field to update");
            }
            let path = namespace_base_path(&namespace);
            let request = scoped_payload(&owner, &policy.request())?;
            let namespace: CacheNamespaceResponse = client.cache_put(&path, &request).await?;
            print_namespace(&namespace, output_format, Some("updated"))
        }
        CacheNamespaceCommands::Delete {
            namespace,
            owner,
            yes,
        } => {
            confirm(
                yes,
                output_format,
                &format!("Delete cache namespace '{}'?", namespace),
            )?;
            let path = namespace_query_path(&namespace, &owner)?;
            client.cache_delete(&path).await?;
            print_delete_confirmation("Cache namespace", &namespace, output_format)
        }
    }
}

async fn handle_entry(
    profile: &Option<String>,
    command: CacheEntryCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
    cache_output: CacheOutput,
) -> Result<()> {
    let mut client = configured_client(profile, api_url)?;
    match command {
        CacheEntryCommands::Get {
            namespace,
            external_id,
            owner,
            generation,
        } => {
            reject_ndjson(cache_output)?;
            let path = format!("{}/entries/lookup", namespace_base_path(&namespace));
            let request = scoped_payload(
                &owner,
                &EntryPointLookupRequest {
                    external_id,
                    generation_id: generation,
                    require_fresh: false,
                },
            )?;
            let response: EntryPointLookupResponse = client.cache_post(&path, &request).await?;
            print_entry_lookup_point(&response, output_format)
        }
        CacheEntryCommands::GetMany {
            namespace,
            owner,
            external_ids,
            external_id_file,
            generation,
        } => {
            reject_ndjson(cache_output)?;
            let ids = collect_external_ids(external_ids, external_id_file)?;
            let path = format!("{}/entries/lookup-many", namespace_base_path(&namespace));
            let request = scoped_payload(
                &owner,
                &EntryLookupRequest {
                    external_ids: ids,
                    generation_id: generation,
                    require_fresh: false,
                },
            )?;
            let response: EntryLookupResponse = client.cache_post(&path, &request).await?;
            print_entry_lookup(&response, output_format)
        }
        CacheEntryCommands::Scan {
            namespace,
            owner,
            generation,
            cursor,
            page_size,
            include_values,
            all,
        } => {
            if matches!(cache_output, CacheOutput::Ndjson) && !all {
                anyhow::bail!("--output ndjson requires '--all' for cache entry scans");
            }
            if all && !matches!(cache_output, CacheOutput::Ndjson) {
                anyhow::bail!("'cache entry scan --all' requires '--output ndjson'");
            }

            if all {
                stream_all_entries(
                    &mut client,
                    &namespace,
                    &owner,
                    generation,
                    cursor,
                    page_size,
                    include_values,
                )
                .await
            } else {
                let page = fetch_scan_page(
                    &mut client,
                    &namespace,
                    &owner,
                    generation,
                    cursor,
                    page_size,
                    include_values,
                )
                .await?;
                print_scan_page(&page, output_format, include_values)
            }
        }
    }
}

async fn handle_generation(
    profile: &Option<String>,
    command: CacheGenerationCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let mut client = configured_client(profile, api_url)?;
    match command {
        CacheGenerationCommands::List {
            namespace,
            owner,
            limit,
            cursor,
        } => {
            let mut path = format!(
                "{}/generations?{}",
                namespace_base_path(&namespace),
                owner.query()?
            );
            if let Some(limit) = limit {
                path.push_str(&format!("&limit={limit}"));
            }
            if let Some(cursor) = cursor {
                path.push_str("&cursor=");
                path.push_str(&urlencoding::encode(&cursor));
            }
            let generations: CacheList<CacheGenerationResponse> = client.cache_get(&path).await?;
            let (generations, next_cursor) = generations.into_parts();
            print_generations(generations, next_cursor, output_format)
        }
        CacheGenerationCommands::Show {
            namespace,
            generation_id,
            owner,
        } => {
            let path = format!(
                "{}/generations/{generation_id}?{}",
                namespace_base_path(&namespace),
                owner.query()?
            );
            let generation: CacheGenerationResponse = client.cache_get(&path).await?;
            print_generation(&generation, output_format, None)
        }
    }
}

async fn handle_refresh(
    profile: &Option<String>,
    command: CacheRefreshCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let mut client = configured_client(profile, api_url)?;
    match command {
        CacheRefreshCommands::Begin {
            namespace,
            owner,
            client_refresh_id,
            expected_chunk_count,
            expected_count,
            expected_bytes,
            source_revision,
            expected_active,
            expect_empty,
        } => {
            validate_expected_chunk_count(expected_chunk_count)?;
            let generation = begin_refresh(
                &mut client,
                &namespace,
                &owner,
                BeginRefreshRequest {
                    client_refresh_id: client_refresh_id.unwrap_or_else(new_refresh_id),
                    expected_active_generation_id: expected_active_for_promotion(
                        expected_active,
                        expect_empty,
                    )?,
                    expected_chunk_count,
                    expected_count,
                    expected_bytes,
                    source_revision,
                },
            )
            .await?;
            print_generation(
                &generation,
                output_format,
                Some("staging generation created"),
            )
        }
        CacheRefreshCommands::Upload {
            namespace,
            generation_id,
            owner,
            chunk_index,
            file,
        } => {
            if chunk_index < 0 {
                anyhow::bail!("--chunk-index must be nonnegative");
            }
            let batch = read_one_chunk(&file, DEFAULT_UPLOAD_CHUNK_RECORDS)?;
            if batch.records.is_empty() {
                anyhow::bail!("Upload input contains no records");
            }
            let response = upload_chunk(
                &mut client,
                &namespace,
                &owner,
                generation_id,
                chunk_index,
                batch.records,
            )
            .await?;
            print_value(&response, output_format)
        }
        CacheRefreshCommands::Seal {
            namespace,
            generation_id,
            owner,
            expected_chunk_count,
            expected_count,
            expected_bytes,
        } => {
            validate_expected_chunk_count(expected_chunk_count)?;
            let generation = seal_refresh(
                &mut client,
                &namespace,
                &owner,
                generation_id,
                expected_chunk_count,
                expected_count,
                expected_bytes,
            )
            .await?;
            print_generation(&generation, output_format, Some("generation sealed"))
        }
        CacheRefreshCommands::Promote {
            namespace,
            generation_id,
            owner,
            expected_active,
            expect_empty,
        } => {
            let generation = promote_refresh(
                &mut client,
                &namespace,
                &owner,
                generation_id,
                expected_active_for_promotion(expected_active, expect_empty)?,
            )
            .await?;
            print_generation(&generation, output_format, Some("generation promoted"))
        }
        CacheRefreshCommands::Abort {
            namespace,
            generation_id,
            owner,
            yes,
        } => {
            confirm(
                yes,
                output_format,
                &format!("Abort cache generation {generation_id}?"),
            )?;
            let path = format!(
                "{}/generations/{generation_id}",
                namespace_base_path(&namespace)
            );
            let path = format!("{path}/abandon");
            let request = scoped_payload(&owner, &json!({}))?;
            let _: JsonValue = client.cache_post(&path, &request).await?;
            print_delete_confirmation(
                "Cache generation",
                &generation_id.to_string(),
                output_format,
            )
        }
        CacheRefreshCommands::Apply {
            namespace,
            owner,
            input,
            client_refresh_id,
            chunk_records,
            expected_chunk_count,
            source_revision,
            expected_active,
            expect_empty,
        } => {
            let expected_active = expected_active_for_promotion(expected_active, expect_empty)?;
            let plan = if input == "-" {
                let expected_chunk_count = expected_chunk_count.context(
                    "--expected-chunk-count is required with --input - because stdin cannot be replayed",
                )?;
                validate_expected_chunk_count(expected_chunk_count)?;
                UploadPlan {
                    expected_chunk_count,
                    expected_count: None,
                }
            } else {
                let plan = plan_upload(&input, chunk_records)?;
                if let Some(expected) = expected_chunk_count {
                    if expected != plan.expected_chunk_count {
                        anyhow::bail!(
                            "--expected-chunk-count ({expected}) does not match input ({})",
                            plan.expected_chunk_count
                        );
                    }
                }
                plan
            };

            let generation = begin_refresh(
                &mut client,
                &namespace,
                &owner,
                BeginRefreshRequest {
                    client_refresh_id: client_refresh_id.unwrap_or_else(new_refresh_id),
                    expected_active_generation_id: expected_active,
                    expected_chunk_count: plan.expected_chunk_count,
                    expected_count: plan.expected_count,
                    expected_bytes: None,
                    source_revision,
                },
            )
            .await?;

            let uploaded = upload_all_chunks(
                &mut client,
                &namespace,
                &owner,
                generation.id,
                &input,
                chunk_records,
            )
            .await?;
            if uploaded != plan.expected_chunk_count {
                anyhow::bail!(
                    "Input changed while refresh was applying: expected {} chunks, uploaded {uploaded}",
                    plan.expected_chunk_count
                );
            }

            let sealed = seal_refresh(
                &mut client,
                &namespace,
                &owner,
                generation.id,
                plan.expected_chunk_count,
                plan.expected_count,
                None,
            )
            .await?;
            let promoted =
                promote_refresh(&mut client, &namespace, &owner, sealed.id, expected_active)
                    .await?;
            print_generation(
                &promoted,
                output_format,
                Some("refresh applied and promoted"),
            )
        }
    }
}

fn configured_client(profile: &Option<String>, api_url: &Option<String>) -> Result<ApiClient> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    Ok(ApiClient::from_config(&config, api_url))
}

fn namespace_base_path(namespace: &str) -> String {
    format!("/cache/namespaces/{}", urlencoding::encode(namespace))
}

fn namespace_query_path(namespace: &str, owner: &OwnerSelectorArgs) -> Result<String> {
    Ok(format!(
        "{}?{}",
        namespace_base_path(namespace),
        owner.query()?
    ))
}

fn fetch_query_path(
    namespace: &str,
    owner: &OwnerSelectorArgs,
    generation: Option<i64>,
    cursor: Option<&str>,
    page_size: u32,
    include_values: bool,
) -> Result<String> {
    let mut path = format!(
        "{}/entries?{}",
        namespace_base_path(namespace),
        owner.query()?
    );
    if let Some(generation) = generation {
        path.push_str(&format!("&generation={generation}"));
    }
    if let Some(cursor) = cursor {
        path.push_str(&format!("&cursor={}", urlencoding::encode(cursor)));
    }
    path.push_str(&format!("&limit={page_size}"));
    let _ = include_values;
    Ok(path)
}

async fn fetch_scan_page(
    client: &mut ApiClient,
    namespace: &str,
    owner: &OwnerSelectorArgs,
    generation: Option<i64>,
    cursor: Option<String>,
    page_size: u32,
    include_values: bool,
) -> Result<CacheScanResponse> {
    let path = fetch_query_path(
        namespace,
        owner,
        generation,
        cursor.as_deref(),
        page_size,
        include_values,
    )?;
    client.cache_get(&path).await
}

async fn stream_all_entries(
    client: &mut ApiClient,
    namespace: &str,
    owner: &OwnerSelectorArgs,
    mut generation: Option<i64>,
    mut cursor: Option<String>,
    page_size: u32,
    include_values: bool,
) -> Result<()> {
    let mut stdout = io::stdout().lock();
    loop {
        let page = fetch_scan_page(
            client,
            namespace,
            owner,
            generation,
            cursor,
            page_size,
            include_values,
        )
        .await?;

        let pinned_generation = page.generation_id;
        if let Some(expected) = generation {
            if expected != pinned_generation {
                anyhow::bail!(
                    "Cache scan response changed pinned generation from {expected} to {pinned_generation}"
                );
            }
        }
        generation = Some(pinned_generation);

        for entry in &page.items {
            serde_json::to_writer(&mut stdout, entry)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }

        eprintln!(
            "{}",
            serde_json::to_string(&scan_metadata(pinned_generation, &page))?
        );

        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(())
}

async fn begin_refresh(
    client: &mut ApiClient,
    namespace: &str,
    owner: &OwnerSelectorArgs,
    request: BeginRefreshRequest,
) -> Result<CacheGenerationResponse> {
    let path = format!("{}/generations", namespace_base_path(namespace));
    let request = scoped_payload(owner, &request)?;
    client.cache_post(&path, &request).await
}

async fn upload_chunk(
    client: &mut ApiClient,
    namespace: &str,
    owner: &OwnerSelectorArgs,
    generation_id: i64,
    chunk_index: i32,
    entries: Vec<UploadEntry>,
) -> Result<JsonValue> {
    let path = format!(
        "{}/generations/{generation_id}/chunks/{chunk_index}",
        namespace_base_path(namespace)
    );
    let request = scoped_payload(owner, &UploadChunkRequest { entries })?;
    client.cache_put(&path, &request).await
}

async fn seal_refresh(
    client: &mut ApiClient,
    namespace: &str,
    owner: &OwnerSelectorArgs,
    generation_id: i64,
    expected_chunk_count: i32,
    expected_count: Option<i64>,
    expected_bytes: Option<i64>,
) -> Result<CacheGenerationResponse> {
    let path = format!(
        "{}/generations/{generation_id}/seal",
        namespace_base_path(namespace)
    );
    let request = scoped_payload(
        owner,
        &SealRefreshRequest {
            expected_chunk_count,
            expected_record_count: expected_count,
            expected_size_bytes: expected_bytes,
        },
    )?;
    client.cache_post(&path, &request).await
}

async fn promote_refresh(
    client: &mut ApiClient,
    namespace: &str,
    owner: &OwnerSelectorArgs,
    generation_id: i64,
    expected_active_generation: Option<i64>,
) -> Result<CacheGenerationResponse> {
    let path = format!(
        "{}/generations/{generation_id}/promote",
        namespace_base_path(namespace)
    );
    // A promotion is one optimistic request. In particular, do not retry a
    // conflict or turn it into a force promotion.
    let request = scoped_payload(
        owner,
        &PromoteRequest {
            expected_active_generation_id: expected_active_generation,
        },
    )?;
    client.cache_post(&path, &request).await
}

fn scoped_payload<T: Serialize>(owner: &OwnerSelectorArgs, payload: &T) -> Result<JsonValue> {
    let mut payload = serde_json::to_value(payload)?;
    let object = payload
        .as_object_mut()
        .context("Cache request payload must be a JSON object")?;
    let owner = owner.request()?;
    object.insert(
        "owner_type".to_string(),
        JsonValue::String(owner.owner_type.as_str().to_string()),
    );
    if let Some(owner_ref) = owner.owner_ref {
        object.insert("owner_ref".to_string(), JsonValue::String(owner_ref));
    }
    Ok(payload)
}

fn scan_metadata(generation: i64, page: &CacheScanResponse) -> JsonValue {
    json!({
        "generation": generation,
        "cursor_expires_at": page.cursor_expires_at,
        "next_cursor": page.next_cursor,
        "record_count": page.record_count,
        "stale": page.stale,
    })
}

fn expected_active_for_promotion(
    expected_active: Option<i64>,
    expect_empty: bool,
) -> Result<Option<i64>> {
    match (expected_active, expect_empty) {
        (Some(id), false) => Ok(Some(id)),
        (None, true) => Ok(None),
        _ => anyhow::bail!("Specify exactly one of --expected-active or --expect-empty"),
    }
}

#[derive(Debug)]
struct RecordBatch {
    records: Vec<UploadEntry>,
}

#[derive(Debug)]
struct UploadPlan {
    expected_chunk_count: i32,
    expected_count: Option<i64>,
}

fn read_one_chunk(path: &str, max_records: usize) -> Result<RecordBatch> {
    with_reader(path, |reader| {
        let batch = read_record_batch(reader, max_records)?;
        let mut line = Vec::new();
        while let Some(next) = read_bounded_ndjson_line(reader, &mut line)? {
            if !trim_line_ending(next).is_empty() {
                anyhow::bail!(
                    "Upload input exceeds one bounded chunk; split it into at most {max_records} records per upload"
                );
            }
        }
        Ok(batch)
    })
}

fn plan_upload(path: &str, max_records: usize) -> Result<UploadPlan> {
    with_reader(path, |reader| {
        let mut records = 0_i64;
        let mut chunks = 0_i32;
        loop {
            let batch = read_record_batch(reader, max_records)?;
            if batch.records.is_empty() {
                break;
            }
            records = records
                .checked_add(batch.records.len() as i64)
                .context("Input has too many records")?;
            chunks = chunks.checked_add(1).context("Input has too many chunks")?;
        }
        Ok(UploadPlan {
            expected_chunk_count: chunks,
            expected_count: Some(records),
        })
    })
}

async fn upload_all_chunks(
    client: &mut ApiClient,
    namespace: &str,
    owner: &OwnerSelectorArgs,
    generation_id: i64,
    path: &str,
    max_records: usize,
) -> Result<i32> {
    let mut reader = open_reader(path)?;
    let mut chunk_index = 0_i32;
    loop {
        let batch = read_record_batch(&mut *reader, max_records)?;
        if batch.records.is_empty() {
            break;
        }
        upload_chunk(
            client,
            namespace,
            owner,
            generation_id,
            chunk_index,
            batch.records,
        )
        .await?;
        chunk_index = chunk_index
            .checked_add(1)
            .context("Too many input chunks")?;
    }
    Ok(chunk_index)
}

fn with_reader<T>(path: &str, callback: impl FnOnce(&mut dyn BufRead) -> Result<T>) -> Result<T> {
    let mut reader = open_reader(path)?;
    callback(&mut *reader)
}

fn open_reader(path: &str) -> Result<Box<dyn BufRead>> {
    if path == "-" {
        Ok(Box::new(BufReader::new(io::stdin())))
    } else {
        let file = File::open(path).with_context(|| format!("Failed to open '{path}'"))?;
        Ok(Box::new(BufReader::new(file)))
    }
}

fn read_record_batch(reader: &mut dyn BufRead, max_records: usize) -> Result<RecordBatch> {
    let mut records = Vec::with_capacity(max_records.min(DEFAULT_UPLOAD_CHUNK_RECORDS));
    let mut input_bytes = 0usize;
    let mut bytes = Vec::new();

    while records.len() < max_records {
        let Some(line) = read_bounded_ndjson_line(reader, &mut bytes)? else {
            break;
        };
        let line = trim_line_ending(line);
        if line.is_empty() {
            continue;
        }
        let line_bytes = line.len();
        if input_bytes
            .checked_add(line_bytes)
            .filter(|bytes| *bytes <= DEFAULT_UPLOAD_CHUNK_BYTES)
            .is_none()
        {
            anyhow::bail!(
                "Upload chunk exceeds the {} byte bound; lower --chunk-records",
                DEFAULT_UPLOAD_CHUNK_BYTES
            );
        }
        let entry: UploadEntry = serde_json::from_slice(line)
            .context("Each upload record must be a JSON object with external_id and value")?;
        validate_upload_entry(&entry)?;
        input_bytes += line_bytes;
        records.push(entry);
    }

    Ok(RecordBatch { records })
}

fn read_bounded_ndjson_line<'a>(
    reader: &mut dyn BufRead,
    buffer: &'a mut Vec<u8>,
) -> Result<Option<&'a [u8]>> {
    buffer.clear();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if buffer.is_empty() {
                Ok(None)
            } else {
                Ok(Some(buffer.as_slice()))
            };
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |position| position + 1);
        if buffer
            .len()
            .checked_add(take)
            .filter(|size| *size <= MAX_NDJSON_LINE_BYTES)
            .is_none()
        {
            anyhow::bail!("NDJSON input line exceeds {MAX_NDJSON_LINE_BYTES} bytes");
        }
        buffer.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            return Ok(Some(buffer.as_slice()));
        }
    }
}

fn trim_line_ending(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn validate_upload_entry(entry: &UploadEntry) -> Result<()> {
    if entry.external_id.is_empty() {
        anyhow::bail!("Upload records require a non-empty external_id");
    }
    Ok(())
}

fn collect_external_ids(
    mut external_ids: Vec<String>,
    external_id_file: Option<String>,
) -> Result<Vec<String>> {
    if let Some(path) = external_id_file {
        with_reader(&path, |reader| {
            let mut line = Vec::new();
            while let Some(next) = read_bounded_ndjson_line(reader, &mut line)? {
                let value = std::str::from_utf8(trim_line_ending(next))
                    .context("External ID input must be UTF-8")?
                    .trim();
                if !value.is_empty() {
                    external_ids.push(value.to_string());
                    if external_ids.len() > MAX_MULTI_LOOKUP_IDS {
                        anyhow::bail!(
                            "entry get-many accepts at most {MAX_MULTI_LOOKUP_IDS} external IDs"
                        );
                    }
                }
            }
            Ok(())
        })?;
    }

    if external_ids.is_empty() {
        anyhow::bail!("Provide at least one --external-id or --external-id-file");
    }
    if external_ids.len() > MAX_MULTI_LOOKUP_IDS {
        anyhow::bail!("entry get-many accepts at most {MAX_MULTI_LOOKUP_IDS} external IDs");
    }
    if external_ids.iter().any(|id| id.is_empty()) {
        anyhow::bail!("External IDs must be non-empty");
    }
    Ok(external_ids)
}

fn validate_expected_chunk_count(expected_chunk_count: i32) -> Result<()> {
    if expected_chunk_count < 0 {
        anyhow::bail!("--expected-chunk-count must be nonnegative");
    }
    Ok(())
}

fn parse_scan_page_size(value: &str) -> std::result::Result<u32, String> {
    let value = value
        .parse::<u32>()
        .map_err(|_| "page size must be an integer".to_string())?;
    if value == 0 || value > MAX_SCAN_PAGE_SIZE {
        return Err(format!(
            "page size must be between 1 and {MAX_SCAN_PAGE_SIZE}"
        ));
    }
    Ok(value)
}

fn parse_metadata_page_size(value: &str) -> std::result::Result<u32, String> {
    let value = value
        .parse::<u32>()
        .map_err(|_| "limit must be an integer".to_string())?;
    if value == 0 || value > 500 {
        return Err("limit must be between 1 and 500".to_string());
    }
    Ok(value)
}

fn parse_chunk_records(value: &str) -> std::result::Result<usize, String> {
    let value = value
        .parse::<usize>()
        .map_err(|_| "chunk records must be an integer".to_string())?;
    if value == 0 || value > MAX_UPLOAD_CHUNK_RECORDS {
        return Err(format!(
            "chunk records must be between 1 and {MAX_UPLOAD_CHUNK_RECORDS}"
        ));
    }
    Ok(value)
}

fn new_refresh_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let sequence = REFRESH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("cli-{millis}-{}-{sequence}", std::process::id())
}

fn confirm(yes: bool, output_format: OutputFormat, prompt: &str) -> Result<()> {
    if yes || !matches!(output_format, OutputFormat::Table) {
        return Ok(());
    }
    if !dialoguer::Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .interact()?
    {
        anyhow::bail!("Operation cancelled");
    }
    Ok(())
}

fn print_delete_confirmation(kind: &str, value: &str, output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(
            &json!({"message": format!("{kind} '{value}' deleted")}),
            output_format,
        ),
        OutputFormat::Table => {
            output::print_success(&format!("{kind} '{value}' deleted"));
            Ok(())
        }
    }
}

fn print_namespaces(
    namespaces: Vec<CacheNamespaceResponse>,
    next_cursor: Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(
            &json!({"namespaces": namespaces, "next_cursor": next_cursor}),
            output_format,
        ),
        OutputFormat::Table => {
            if namespaces.is_empty() {
                output::print_info("No cache namespaces found");
                return Ok(());
            }
            let mut table = output::create_table();
            output::add_header(
                &mut table,
                vec![
                    "Namespace",
                    "Owner",
                    "Active Gen",
                    "Records",
                    "Bytes",
                    "Stale",
                ],
            );
            for namespace in namespaces {
                let owner = namespace_owner_display(&namespace);
                table.add_row(vec![
                    namespace.namespace,
                    owner,
                    optional_display(namespace.active_generation),
                    optional_display(namespace.record_count),
                    optional_display(namespace.size_bytes),
                    namespace
                        .stale
                        .map(output::format_bool)
                        .unwrap_or_else(|| "-".to_string()),
                ]);
            }
            println!("{table}");
            if let Some(cursor) = next_cursor {
                output::print_info(&format!("Next cursor: {cursor}"));
            }
            Ok(())
        }
    }
}

fn print_namespace(
    namespace: &CacheNamespaceResponse,
    output_format: OutputFormat,
    success: Option<&str>,
) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(namespace, output_format),
        OutputFormat::Table => {
            if let Some(success) = success {
                output::print_success(&format!(
                    "Cache namespace '{}' {success}",
                    namespace.namespace
                ));
            }
            output::print_key_value_table(vec![
                ("Namespace", namespace.namespace.clone()),
                ("ID", namespace.id.to_string()),
                ("Owner", namespace_owner_display(namespace)),
                (
                    "Active Generation",
                    optional_display(namespace.active_generation),
                ),
                ("Records", optional_display(namespace.record_count)),
                ("Bytes", optional_display(namespace.size_bytes)),
                (
                    "Freshness Target",
                    optional_display(namespace.freshness_target_seconds),
                ),
                (
                    "Stale",
                    namespace
                        .stale
                        .map(output::format_bool)
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "Refresh Health",
                    namespace
                        .refresh_health
                        .clone()
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "Max Records / Generation",
                    optional_display(namespace.max_records_per_generation),
                ),
                (
                    "Max Generation Bytes",
                    optional_display(namespace.max_generation_bytes),
                ),
                (
                    "Max Retained Bytes",
                    optional_display(namespace.max_retained_bytes),
                ),
                (
                    "Max Retained Generations",
                    optional_display(namespace.max_retained_generations),
                ),
                (
                    "Max Staging Generations",
                    optional_display(namespace.max_staging_generations),
                ),
                (
                    "Created",
                    namespace
                        .created
                        .as_deref()
                        .map(output::format_timestamp)
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "Updated",
                    namespace
                        .updated
                        .as_deref()
                        .map(output::format_timestamp)
                        .unwrap_or_else(|| "-".into()),
                ),
            ]);
            Ok(())
        }
    }
}

fn print_generations(
    generations: Vec<CacheGenerationResponse>,
    next_cursor: Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(
            &json!({"generations": generations, "next_cursor": next_cursor}),
            output_format,
        ),
        OutputFormat::Table => {
            if generations.is_empty() {
                output::print_info("No cache generations found");
                return Ok(());
            }
            let mut table = output::create_table();
            output::add_header(
                &mut table,
                vec![
                    "ID",
                    "State",
                    "Records",
                    "Bytes",
                    "Source Revision",
                    "Created",
                ],
            );
            for generation in generations {
                table.add_row(vec![
                    generation.id.to_string(),
                    generation.state,
                    generation.record_count.to_string(),
                    generation.size_bytes.to_string(),
                    generation
                        .source_revision
                        .unwrap_or_else(|| "-".to_string()),
                    output::format_timestamp(&generation.created),
                ]);
            }
            println!("{table}");
            if let Some(cursor) = next_cursor {
                output::print_info(&format!("Next cursor: {cursor}"));
            }
            Ok(())
        }
    }
}

fn print_generation(
    generation: &CacheGenerationResponse,
    output_format: OutputFormat,
    success: Option<&str>,
) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(generation, output_format),
        OutputFormat::Table => {
            if let Some(success) = success {
                output::print_success(success);
            }
            output::print_key_value_table(vec![
                ("Generation", generation.id.to_string()),
                ("Namespace", generation.namespace.to_string()),
                ("State", generation.state.clone()),
                ("Client Refresh ID", generation.client_refresh_id.clone()),
                (
                    "Expected Chunks",
                    generation.expected_chunk_count.to_string(),
                ),
                (
                    "Expected Active",
                    optional_display(generation.expected_active_generation_id),
                ),
                (
                    "Expected Records",
                    optional_display(generation.expected_count),
                ),
                (
                    "Expected Bytes",
                    optional_display(generation.expected_bytes),
                ),
                ("Records", generation.record_count.to_string()),
                ("Bytes", generation.size_bytes.to_string()),
                (
                    "Source Revision",
                    generation
                        .source_revision
                        .clone()
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "Sealed",
                    generation
                        .sealed
                        .as_deref()
                        .map(output::format_timestamp)
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "Activated",
                    generation
                        .activated
                        .as_deref()
                        .map(output::format_timestamp)
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "Readable Until",
                    generation
                        .readable_until
                        .as_deref()
                        .map(output::format_timestamp)
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "Failure",
                    generation
                        .failure_reason
                        .clone()
                        .unwrap_or_else(|| "-".into()),
                ),
            ]);
            Ok(())
        }
    }
}

fn print_entry(
    entry: &CacheEntryResponse,
    output_format: OutputFormat,
    include_value: bool,
) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(entry, output_format),
        OutputFormat::Table => {
            let mut pairs = vec![
                ("External ID", entry.external_id.clone()),
                (
                    "Source Updated",
                    entry
                        .source_updated_at
                        .as_deref()
                        .map(output::format_timestamp)
                        .unwrap_or_else(|| "-".into()),
                ),
                ("Bytes", entry.size_bytes.to_string()),
            ];
            if include_value {
                pairs.push((
                    "Value",
                    serde_json::to_string_pretty(&entry.value)
                        .unwrap_or_else(|_| entry.value.to_string()),
                ));
            }
            output::print_key_value_table(pairs);
            Ok(())
        }
    }
}

fn print_entry_lookup_point(
    response: &EntryPointLookupResponse,
    output_format: OutputFormat,
) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(response, output_format),
        OutputFormat::Table => match &response.item {
            Some(entry) => {
                output::print_info(&format!("Generation {}", response.generation_id));
                print_entry(entry, output_format, true)
            }
            None => {
                output::print_info(&format!(
                    "No cache entry found in generation {}",
                    response.generation_id
                ));
                Ok(())
            }
        },
    }
}

fn print_entry_lookup(response: &EntryLookupResponse, output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(response, output_format),
        OutputFormat::Table => {
            if response.items.is_empty() {
                output::print_info("No cache entries found");
            } else {
                print_entry_table(&response.items, false);
            }
            if !response.missing_external_ids.is_empty() {
                output::print_warning(&format!(
                    "{} requested external ID(s) were not found",
                    response.missing_external_ids.len()
                ));
            }
            Ok(())
        }
    }
}

fn print_scan_page(
    page: &CacheScanResponse,
    output_format: OutputFormat,
    include_values: bool,
) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(page, output_format),
        OutputFormat::Table => {
            print_entry_table(&page.items, include_values);
            output::print_key_value_table(vec![
                ("Pinned Generation", page.generation_id.to_string()),
                (
                    "Cursor Expires",
                    page.cursor_expires_at
                        .as_deref()
                        .map(output::format_timestamp)
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "Next Cursor",
                    page.next_cursor.clone().unwrap_or_else(|| "-".into()),
                ),
                ("Record Count", optional_display(page.record_count)),
                ("Stale", output::format_bool(page.stale)),
            ]);
            Ok(())
        }
    }
}

fn print_entry_table(entries: &[CacheEntryResponse], include_values: bool) {
    if entries.is_empty() {
        output::print_info("No cache entries found");
        return;
    }
    let mut table = output::create_table();
    let mut headers = vec!["External ID", "Source Updated", "Bytes", "Value"];
    if include_values {
        headers.push("Full Value");
    }
    output::add_header(&mut table, headers);
    for entry in entries {
        let indicator = match &entry.value {
            JsonValue::Object(value) => format!("object ({} fields)", value.len()),
            JsonValue::Array(value) => format!("array ({} items)", value.len()),
            JsonValue::String(_) => "string".to_string(),
            JsonValue::Number(_) => "number".to_string(),
            JsonValue::Bool(_) => "boolean".to_string(),
            JsonValue::Null => "null".to_string(),
        };
        let mut row = vec![
            entry.external_id.clone(),
            entry
                .source_updated_at
                .as_deref()
                .map(output::format_timestamp)
                .unwrap_or_else(|| "-".into()),
            entry.size_bytes.to_string(),
            indicator,
        ];
        if include_values {
            row.push(
                serde_json::to_string(&entry.value).unwrap_or_else(|_| entry.value.to_string()),
            );
        }
        table.add_row(row);
    }
    println!("{table}");
}

fn print_value(value: &JsonValue, output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(value, output_format),
        OutputFormat::Table => {
            output::print_success("Cache chunk uploaded");
            output::print_key_value_table(vec![(
                "Response",
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
            )]);
            Ok(())
        }
    }
}

fn namespace_owner_display(namespace: &CacheNamespaceResponse) -> String {
    let reference = namespace
        .owner_ref
        .clone()
        .or_else(|| match namespace.owner_type.as_str() {
            "identity" => namespace.owner_identity.map(|id| id.to_string()),
            "pack" => namespace.owner_pack_ref.clone(),
            "action" => namespace.owner_action_ref.clone(),
            "sensor" => namespace.owner_sensor_ref.clone(),
            _ => namespace.owner.clone(),
        });
    reference
        .map(|reference| format!("{}: {reference}", namespace.owner_type))
        .unwrap_or_else(|| namespace.owner_type.clone())
}

fn optional_display<T: ToString>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: CacheCommands,
    }

    #[test]
    fn owner_selector_requires_the_matching_typed_reference() {
        let owner = OwnerSelectorArgs {
            owner_type: CacheOwnerType::Pack,
            owner_pack_ref: None,
            owner_action_ref: Some("core.echo".into()),
            owner_sensor_ref: None,
        };
        assert!(owner.request().is_err());
    }

    #[test]
    fn owner_selector_accepts_explicit_system_only() {
        let owner = OwnerSelectorArgs {
            owner_type: CacheOwnerType::System,
            owner_pack_ref: None,
            owner_action_ref: None,
            owner_sensor_ref: None,
        };
        assert_eq!(owner.query().unwrap(), "owner_type=system");
    }

    #[test]
    fn scan_page_size_is_bounded() {
        assert_eq!(parse_scan_page_size("1000").unwrap(), 1000);
        assert!(parse_scan_page_size("1001").is_err());
        assert!(parse_scan_page_size("0").is_err());
    }

    #[test]
    fn zero_chunks_are_allowed_for_authoritative_empty_snapshots() {
        assert!(validate_expected_chunk_count(0).is_ok());
        assert!(validate_expected_chunk_count(-1).is_err());
    }

    #[test]
    fn parser_accepts_zero_chunk_begin_and_seal_contracts() {
        assert!(TestCli::try_parse_from([
            "attune",
            "refresh",
            "begin",
            "users",
            "--owner-type",
            "system",
            "--expected-chunk-count",
            "0",
            "--expect-empty",
        ])
        .is_ok());
        assert!(TestCli::try_parse_from([
            "attune",
            "refresh",
            "seal",
            "users",
            "42",
            "--owner-type",
            "system",
            "--expected-chunk-count",
            "0",
            "--expected-count",
            "0",
            "--expected-bytes",
            "0",
        ])
        .is_ok());
    }

    #[test]
    fn parser_accepts_metadata_list_continuation_arguments() {
        assert!(TestCli::try_parse_from([
            "attune",
            "namespace",
            "list",
            "--owner-type",
            "pack",
            "--owner-pack-ref",
            "salesforce",
            "--namespace",
            "active users",
            "--freshness",
            "fresh",
            "--limit",
            "25",
            "--cursor",
            "next/page",
        ])
        .is_ok());
        assert!(TestCli::try_parse_from([
            "attune",
            "generation",
            "list",
            "users",
            "--owner-type",
            "system",
            "--limit",
            "10",
            "--cursor",
            "next/page",
        ])
        .is_ok());
        assert!(parse_metadata_page_size("501").is_err());
    }

    #[test]
    fn metadata_continuations_do_not_inject_a_default_limit() {
        let namespace = TestCli::try_parse_from([
            "attune",
            "namespace",
            "list",
            "--owner-type",
            "system",
            "--cursor",
            "next/page",
        ])
        .expect("namespace continuation should parse");
        match namespace.command {
            CacheCommands::Namespace {
                command: CacheNamespaceCommands::List { limit, cursor, .. },
            } => {
                assert_eq!(limit, None);
                assert_eq!(cursor.as_deref(), Some("next/page"));
            }
            _ => panic!("unexpected cache command"),
        }

        let generation = TestCli::try_parse_from([
            "attune",
            "generation",
            "list",
            "users",
            "--owner-type",
            "system",
            "--cursor",
            "next/page",
        ])
        .expect("generation continuation should parse");
        match generation.command {
            CacheCommands::Generation {
                command: CacheGenerationCommands::List { limit, cursor, .. },
            } => {
                assert_eq!(limit, None);
                assert_eq!(cursor.as_deref(), Some("next/page"));
            }
            _ => panic!("unexpected cache command"),
        }
    }

    #[test]
    fn parser_accepts_repeated_ids_with_a_typed_owner() {
        let parsed = TestCli::try_parse_from([
            "attune",
            "entry",
            "get-many",
            "users",
            "--owner-type",
            "pack",
            "--owner-pack-ref",
            "salesforce",
            "--external-id",
            "first",
            "--external-id",
            "second",
        ])
        .unwrap();

        match parsed.command {
            CacheCommands::Entry {
                command:
                    CacheEntryCommands::GetMany {
                        external_ids,
                        owner,
                        ..
                    },
            } => {
                assert_eq!(external_ids, ["first", "second"]);
                assert_eq!(
                    owner.query().unwrap(),
                    "owner_type=pack&owner_ref=salesforce"
                );
            }

            _ => panic!("unexpected cache command"),
        }
    }

    #[test]
    fn namespace_operation_urls_append_scope_after_the_full_path() {
        let owner = OwnerSelectorArgs {
            owner_type: CacheOwnerType::Pack,
            owner_pack_ref: Some("salesforce".into()),
            owner_action_ref: None,
            owner_sensor_ref: None,
        };
        assert_eq!(
            fetch_query_path("users", &owner, Some(42), Some("cursor"), 100, false).unwrap(),
            "/cache/namespaces/users/entries?owner_type=pack&owner_ref=salesforce&generation=42&cursor=cursor&limit=100"
        );
        assert_eq!(namespace_base_path("users"), "/cache/namespaces/users");
    }

    #[test]
    fn identity_scope_uses_the_authenticated_identity_without_an_owner_ref() {
        let owner = OwnerSelectorArgs {
            owner_type: CacheOwnerType::Identity,
            owner_pack_ref: None,
            owner_action_ref: None,
            owner_sensor_ref: None,
        };
        assert_eq!(owner.query().unwrap(), "owner_type=identity");
        assert!(owner.request().unwrap().owner_ref.is_none());
    }

    #[test]
    fn parser_requires_an_optimistic_promotion_selection() {
        assert!(TestCli::try_parse_from([
            "attune",
            "refresh",
            "promote",
            "users",
            "42",
            "--owner-type",
            "system",
        ])
        .is_err());
    }

    #[test]
    fn bounded_ndjson_reader_rejects_oversized_lines() {
        let oversized = vec![b'x'; MAX_NDJSON_LINE_BYTES + 1];
        let mut reader = BufReader::new(oversized.as_slice());
        let mut line = Vec::new();
        assert!(read_bounded_ndjson_line(&mut reader, &mut line).is_err());
    }

    #[test]
    fn record_batches_are_bounded_without_materializing_all_input() {
        let input = br#"{"external_id":"one","value":{"n":1}}
{"external_id":"two","value":{"n":2}}
{"external_id":"three","value":{"n":3}}
"#;
        let mut reader = BufReader::new(input.as_slice());
        let first = read_record_batch(&mut reader, 2).unwrap();
        assert_eq!(first.records.len(), 2);
        let second = read_record_batch(&mut reader, 2).unwrap();
        assert_eq!(second.records.len(), 1);
    }

    #[test]
    fn record_batches_skip_lf_and_crlf_blank_lines() {
        let input = b"\n{\"external_id\":\"one\",\"value\":{}}\r\n\r\n";
        let mut reader = BufReader::new(input.as_slice());
        let batch = read_record_batch(&mut reader, 2).unwrap();
        assert_eq!(batch.records.len(), 1);
        assert_eq!(batch.records[0].external_id, "one");
    }

    #[test]
    fn ndjson_output_is_one_record_per_line() {
        let entry = CacheEntryResponse {
            external_id: "record-1".into(),
            value: json!({"ok": true}),
            source_updated_at: None,
            source_checksum: None,
            size_bytes: 11,
        };
        let mut output = Vec::new();
        serde_json::to_writer(&mut output, &entry).unwrap();
        output.push(b'\n');
        assert_eq!(output.iter().filter(|byte| **byte == b'\n').count(), 1);
        let decoded: CacheEntryResponse = serde_json::from_slice(&output).unwrap();
        assert_eq!(decoded.external_id, "record-1");
    }

    #[test]
    fn streaming_metadata_carries_the_resume_cursor_separately() {
        let page = CacheScanResponse {
            generation_id: 7,
            items: Vec::new(),
            next_cursor: Some("opaque-cursor".into()),
            cursor_expires_at: Some("2026-07-22T00:00:00Z".into()),
            record_count: Some(200_000),
            stale: false,
        };

        assert_eq!(
            scan_metadata(7, &page),
            json!({
                "generation": 7,
                "cursor_expires_at": "2026-07-22T00:00:00Z",
                "next_cursor": "opaque-cursor",
                "record_count": 200_000,
                "stale": false,
            })
        );
    }
}
