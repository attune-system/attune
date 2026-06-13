//! Proactive Runtime Environment Setup
//!
//! This module provides functions for setting up runtime environments (Python
//! virtualenvs, Node.js node_modules, etc.) proactively — either at worker
//! startup (scanning all registered packs) or in response to a `pack.registered`
//! MQ event.
//!
//! The goal is to ensure environments are ready *before* the first execution,
//! eliminating the first-run penalty and potential permission errors that occur
//! when setup is deferred to execution time.
//!
//! ## Version-Aware Environments
//!
//! When runtime versions are registered (e.g., Python 3.11, 3.12, 3.13), this
//! module creates per-version environments at:
//!   `{runtime_envs_dir}/{pack_ref}/{runtime_name}-{version}`
//!
//! For example: `/opt/attune/runtime_envs/my_pack/python-3.12`
//!
//! This ensures that different versions maintain isolated environments with
//! their own interpreter binaries and installed dependencies. A base (unversioned)
//! environment is also created for backward compatibility with actions that don't
//! declare a version constraint.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use sqlx::PgPool;
use tokio::sync::oneshot;
use tokio::time::{sleep, Duration, MissedTickBehavior};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use attune_common::metadata_cache::{repositories::CachedMetadataRepository, MetadataCache};
use attune_common::models::{Runtime, RuntimeVersion, Worker};
use attune_common::mq::PackRegisteredPayload;
use attune_common::pack_environment::{
    PackEnvironmentManager, PackEnvironmentStatus, UpsertCoordinatedEnvironmentInput,
};
use attune_common::repositories::pack::PackRepository;
use attune_common::repositories::{FindById, FindByRef, List, WorkerRepository};
use attune_common::runtime_detection::{normalize_runtime_name, runtime_aliases_match_filter};
use attune_common::version_matching::matches_constraint;

// Re-export the utility that the API also uses so callers can reach it from
// either crate without adding a direct common dependency for this one function.
pub use attune_common::pack_environment::collect_runtime_names_for_pack;

use crate::runtime::process::ProcessRuntime;

#[derive(Debug, Clone, Default)]
struct RuntimeRequirementProfile {
    any_version: bool,
    constraints: Vec<String>,
}

/// Result of setting up environments for a single pack.
#[derive(Debug)]
pub struct PackEnvSetupResult {
    pub pack_ref: String,
    pub environments_created: Vec<String>,
    pub environments_skipped: Vec<String>,
    pub errors: Vec<String>,
}

/// Result of the full startup scan across all packs.
#[derive(Debug)]
pub struct StartupScanResult {
    pub packs_scanned: usize,
    pub environments_created: usize,
    pub environments_skipped: usize,
    pub errors: Vec<String>,
}

const INSTALL_CLAIM_LEASE_SECONDS: i64 = 300;
const INSTALL_CLAIM_RENEW_SECONDS: u64 = 60;
const INSTALL_CLAIM_ATTEMPTS: usize = 3;
const INSTALL_JITTER_MIN_MS: u64 = 100;
const INSTALL_JITTER_SPAN_MS: u64 = 400;

/// Scan all registered packs and create missing runtime environments.
///
/// This is called at worker startup, before the worker begins consuming
/// execution messages. It ensures that environments for all known packs
/// are ready to go.
///
/// # Arguments
/// * `db_pool` - Database connection pool
/// * `runtime_filter` - Optional list of runtime names this worker supports
///   (from `ATTUNE_WORKER_RUNTIMES`). If `None`, all runtimes are considered.
/// * `packs_base_dir` - Base directory where pack files are stored
/// * `runtime_envs_dir` - Base directory for isolated runtime environments
pub async fn scan_and_setup_all_environments(
    db_pool: &PgPool,
    metadata_cache: &MetadataCache,
    worker_id: i64,
    runtime_filter: Option<&[String]>,
    packs_base_dir: &Path,
    runtime_envs_dir: &Path,
) -> StartupScanResult {
    info!("Starting runtime environment scan for all registered packs");

    let mut result = StartupScanResult {
        packs_scanned: 0,
        environments_created: 0,
        environments_skipped: 0,
        errors: Vec::new(),
    };

    // Load all runtimes from DB, indexed by ID for quick lookup
    let cached = CachedMetadataRepository::new(db_pool, metadata_cache);
    let runtimes = match cached.list_runtimes().await {
        Ok(rts) => rts,
        Err(e) => {
            let msg = format!("Failed to load runtimes from database: {}", e);
            error!("{}", msg);
            result.errors.push(msg);
            return result;
        }
    };

    let runtime_map: HashMap<i64, _> = runtimes.into_iter().map(|r| (r.id, r)).collect();
    let worker = load_worker_for_env_setup(db_pool, worker_id).await;

    // Load all packs
    let packs = match PackRepository::list(db_pool).await {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("Failed to load packs from database: {}", e);
            error!("{}", msg);
            result.errors.push(msg);
            return result;
        }
    };

    // Load all runtime versions, indexed by runtime ID
    let version_map: HashMap<i64, Vec<RuntimeVersion>> = match cached.list_runtime_versions().await
    {
        Ok(versions) => {
            let mut map: HashMap<i64, Vec<RuntimeVersion>> = HashMap::new();
            for v in versions {
                map.entry(v.runtime).or_default().push(v);
            }
            map
        }
        Err(e) => {
            warn!(
                "Failed to load runtime versions from database: {}. \
                     Version-specific environments will not be created.",
                e
            );
            HashMap::new()
        }
    };

    info!("Found {} registered pack(s) to scan", packs.len());

    for pack in &packs {
        result.packs_scanned += 1;

        let pack_result = setup_environments_for_pack(
            db_pool,
            metadata_cache,
            worker.as_ref(),
            &pack.r#ref,
            pack.id,
            runtime_filter,
            packs_base_dir,
            runtime_envs_dir,
            &runtime_map,
            &version_map,
        )
        .await;

        result.environments_created += pack_result.environments_created.len();
        result.environments_skipped += pack_result.environments_skipped.len();
        result.errors.extend(pack_result.errors);
    }

    info!(
        "Environment scan complete: {} pack(s) scanned, {} environment(s) created, \
         {} skipped, {} error(s)",
        result.packs_scanned,
        result.environments_created,
        result.environments_skipped,
        result.errors.len(),
    );

    result
}

/// Set up environments for a single pack, triggered by a `pack.registered` MQ event.
///
/// This is called when the worker receives a `PackRegistered` message. It only
/// sets up environments for the runtimes listed in the event payload (intersection
/// with this worker's supported runtimes).
pub async fn setup_environments_for_registered_pack(
    db_pool: &PgPool,
    metadata_cache: &MetadataCache,
    worker_id: i64,
    event: &PackRegisteredPayload,
    runtime_filter: Option<&[String]>,
    packs_base_dir: &Path,
    runtime_envs_dir: &Path,
) -> PackEnvSetupResult {
    info!(
        "Setting up environments for newly registered pack '{}' (version {})",
        event.pack_ref, event.version
    );

    let mut pack_result = PackEnvSetupResult {
        pack_ref: event.pack_ref.clone(),
        environments_created: Vec::new(),
        environments_skipped: Vec::new(),
        errors: Vec::new(),
    };
    let worker = load_worker_for_env_setup(db_pool, worker_id).await;

    let pack_dir = packs_base_dir.join(&event.pack_ref);
    if !pack_dir.exists() {
        let msg = format!(
            "Pack directory does not exist: {}. Skipping environment setup.",
            pack_dir.display()
        );
        warn!("{}", msg);
        pack_result.errors.push(msg);
        return pack_result;
    }

    let pack = match PackRepository::find_by_ref(db_pool, &event.pack_ref).await {
        Ok(Some(pack)) => pack,
        Ok(None) => {
            let msg = format!(
                "Pack '{}' not found in database during environment setup",
                event.pack_ref
            );
            warn!("{}", msg);
            pack_result.errors.push(msg);
            return pack_result;
        }
        Err(e) => {
            let msg = format!(
                "Failed to load pack '{}' during environment setup: {}",
                event.pack_ref, e
            );
            warn!("{}", msg);
            pack_result.errors.push(msg);
            return pack_result;
        }
    };

    let cached = CachedMetadataRepository::new(db_pool, metadata_cache);
    let runtimes = match cached.list_runtimes().await {
        Ok(rts) => rts,
        Err(e) => {
            let msg = format!("Failed to load runtimes from database: {}", e);
            error!("{}", msg);
            pack_result.errors.push(msg);
            return pack_result;
        }
    };
    let runtime_map: HashMap<i64, _> = runtimes.into_iter().map(|r| (r.id, r)).collect();

    let version_map: HashMap<i64, Vec<RuntimeVersion>> = match cached.list_runtime_versions().await
    {
        Ok(versions) => {
            let mut map: HashMap<i64, Vec<RuntimeVersion>> = HashMap::new();
            for v in versions {
                map.entry(v.runtime).or_default().push(v);
            }
            map
        }
        Err(e) => {
            warn!(
                    "Failed to load runtime versions from database: {}. Version-specific environments will not be created.",
                    e
                );
            HashMap::new()
        }
    };

    setup_environments_for_pack(
        db_pool,
        metadata_cache,
        worker.as_ref(),
        &pack.r#ref,
        pack.id,
        runtime_filter,
        packs_base_dir,
        runtime_envs_dir,
        &runtime_map,
        &version_map,
    )
    .await
}

/// Internal helper: set up environments for a single pack during the startup scan.
///
/// Discovers which runtimes the pack's actions use, filters by this worker's
/// capabilities, and creates any missing environments. Also creates per-version
/// environments for runtimes that have registered versions.
#[allow(clippy::too_many_arguments)]
async fn setup_environments_for_pack(
    db_pool: &PgPool,
    metadata_cache: &MetadataCache,
    worker: Option<&Worker>,
    pack_ref: &str,
    pack_id: i64,
    runtime_filter: Option<&[String]>,
    packs_base_dir: &Path,
    runtime_envs_dir: &Path,
    runtime_map: &HashMap<i64, attune_common::models::Runtime>,
    version_map: &HashMap<i64, Vec<RuntimeVersion>>,
) -> PackEnvSetupResult {
    let mut pack_result = PackEnvSetupResult {
        pack_ref: pack_ref.to_string(),
        environments_created: Vec::new(),
        environments_skipped: Vec::new(),
        errors: Vec::new(),
    };

    let pack_dir = packs_base_dir.join(pack_ref);
    if !pack_dir.exists() {
        debug!(
            "Pack directory '{}' does not exist on disk, skipping",
            pack_dir.display()
        );
        return pack_result;
    }

    // Get all actions for this pack
    let actions = match CachedMetadataRepository::new(db_pool, metadata_cache)
        .find_actions_by_pack(pack_id)
        .await
    {
        Ok(a) => a,
        Err(e) => {
            let msg = format!("Failed to load actions for pack '{}': {}", pack_ref, e);
            warn!("{}", msg);
            pack_result.errors.push(msg);
            return pack_result;
        }
    };

    let runtime_requirements =
        collect_runtime_requirements(db_pool, metadata_cache, &actions).await;
    let seen_runtime_ids: HashSet<i64> = runtime_requirements.keys().copied().collect();
    let env_manager =
        PackEnvironmentManager::with_base_path(db_pool.clone(), runtime_envs_dir.to_path_buf());

    if seen_runtime_ids.is_empty() {
        debug!("Pack '{}' has no actions with runtimes, skipping", pack_ref);
        return pack_result;
    }

    for runtime_id in seen_runtime_ids {
        let rt = match runtime_map.get(&runtime_id) {
            Some(r) => r,
            None => {
                // Try fetching from DB directly (might be a newly added runtime)
                match CachedMetadataRepository::new(db_pool, metadata_cache)
                    .find_runtime_by_id(runtime_id)
                    .await
                {
                    Ok(Some(r)) => {
                        // Can't insert into the borrowed map, so just use it inline
                        let rt_name = r.name.to_lowercase();
                        process_runtime_for_pack(
                            &env_manager,
                            worker,
                            &r,
                            &rt_name,
                            pack_id,
                            pack_ref,
                            runtime_filter,
                            &pack_dir,
                            packs_base_dir,
                            runtime_envs_dir,
                            &mut pack_result,
                        )
                        .await;
                        // Also set up version-specific environments
                        let versions: Vec<attune_common::models::RuntimeVersion> =
                            CachedMetadataRepository::new(db_pool, metadata_cache)
                                .find_runtime_versions_by_runtime(runtime_id)
                                .await
                                .unwrap_or_default();
                        setup_version_environments_from_list(
                            &versions,
                            &env_manager,
                            &r,
                            &rt_name,
                            pack_id,
                            pack_ref,
                            worker,
                            runtime_requirements.get(&runtime_id),
                            &pack_dir,
                            packs_base_dir,
                            runtime_envs_dir,
                            &mut pack_result,
                        )
                        .await;
                        continue;
                    }
                    Ok(None) => {
                        debug!("Runtime ID {} not found in database, skipping", runtime_id);
                        continue;
                    }
                    Err(e) => {
                        warn!("Failed to load runtime {}: {}", runtime_id, e);
                        continue;
                    }
                }
            }
        };

        let rt_name = rt.name.to_lowercase();
        process_runtime_for_pack(
            &env_manager,
            worker,
            rt,
            &rt_name,
            pack_id,
            pack_ref,
            runtime_filter,
            &pack_dir,
            packs_base_dir,
            runtime_envs_dir,
            &mut pack_result,
        )
        .await;

        // Set up per-version environments for available versions of this runtime
        if let Some(versions) = version_map.get(&runtime_id) {
            setup_version_environments_from_list(
                versions,
                &env_manager,
                rt,
                &rt_name,
                pack_id,
                pack_ref,
                worker,
                runtime_requirements.get(&runtime_id),
                &pack_dir,
                packs_base_dir,
                runtime_envs_dir,
                &mut pack_result,
            )
            .await;
        }
    }

    if !pack_result.environments_created.is_empty() {
        info!(
            "Pack '{}': created environments for {:?}",
            pack_ref, pack_result.environments_created,
        );
    }

    pack_result
}

/// Process a single runtime for a pack: check filters, check if env exists, create if needed.
#[allow(clippy::too_many_arguments)]
async fn process_runtime_for_pack(
    env_manager: &PackEnvironmentManager,
    worker: Option<&Worker>,
    rt: &attune_common::models::Runtime,
    rt_name: &str,
    pack_id: i64,
    pack_ref: &str,
    runtime_filter: Option<&[String]>,
    pack_dir: &Path,
    packs_base_dir: &Path,
    runtime_envs_dir: &Path,
    pack_result: &mut PackEnvSetupResult,
) {
    let exec_config = rt.parsed_execution_config();

    // Check if this runtime actually needs an environment
    if exec_config.environment.is_none() && !exec_config.has_dependencies(pack_dir) {
        debug!(
            "Runtime '{}' has no environment config, skipping for pack '{}'",
            rt_name, pack_ref,
        );
        pack_result.environments_skipped.push(rt_name.to_string());
        return;
    }

    let env_dir = runtime_envs_dir.join(pack_ref).join(rt_name);
    let manifest_checksum = dependency_manifest_checksum(&exec_config, pack_dir).await;
    let runtime_supported = runtime_filter
        .map(|filter| runtime_aliases_match_filter(&rt.aliases, filter))
        .unwrap_or(true);

    // Create a temporary ProcessRuntime to perform the setup
    let process_runtime = ProcessRuntime::new(
        rt_name.to_string(),
        exec_config,
        packs_base_dir.to_path_buf(),
        runtime_envs_dir.to_path_buf(),
    );

    match coordinate_environment_setup(
        env_manager,
        worker,
        &process_runtime,
        pack_id,
        pack_ref,
        rt.id,
        &rt.r#ref,
        None,
        rt_name,
        &env_dir,
        manifest_checksum.as_deref(),
        runtime_supported,
        pack_dir,
    )
    .await
    {
        Ok(CoordinateSetupOutcome::Installed) => {
            pack_result.environments_created.push(rt_name.to_string());
        }
        Ok(CoordinateSetupOutcome::Skipped) => {
            pack_result.environments_skipped.push(rt_name.to_string());
        }
        Err(e) => {
            let msg = format!(
                "Failed to set up '{}' environment for pack '{}': {}",
                rt_name, pack_ref, e,
            );
            warn!("{}", msg);
            pack_result.errors.push(msg);
        }
    }
}

/// Set up per-version environments for a runtime, given a list of registered versions.
#[allow(clippy::too_many_arguments)]
async fn setup_version_environments_from_list(
    versions: &[RuntimeVersion],
    env_manager: &PackEnvironmentManager,
    runtime: &Runtime,
    rt_name: &str,
    pack_id: i64,
    pack_ref: &str,
    worker: Option<&Worker>,
    requirements: Option<&RuntimeRequirementProfile>,
    pack_dir: &Path,
    packs_base_dir: &Path,
    runtime_envs_dir: &Path,
    pack_result: &mut PackEnvSetupResult,
) {
    let qualifying_versions: Vec<RuntimeVersion> = versions
        .iter()
        .filter(|version| version_qualifies_for_pack(version, requirements))
        .cloned()
        .collect();
    if qualifying_versions.is_empty() {
        return;
    }

    let advertised_versions = worker
        .map(|worker| worker_runtime_versions_for_runtime(worker, runtime))
        .unwrap_or_default();

    for version in &qualifying_versions {
        let version_exec_config = version.parsed_execution_config();

        if version_exec_config.environment.is_none()
            && !version_exec_config.has_dependencies(pack_dir)
        {
            debug!(
                "Version '{}' {} has no environment config, skipping for pack '{}'",
                version.runtime_ref, version.version, pack_ref,
            );
            continue;
        }

        let version_env_suffix = format!("{}-{}", rt_name, version.version);
        let version_env_dir = runtime_envs_dir.join(pack_ref).join(&version_env_suffix);
        let manifest_checksum = dependency_manifest_checksum(&version_exec_config, pack_dir).await;
        let worker_can_claim = !advertised_versions.is_empty()
            && version_matches_worker(version, &advertised_versions);

        let version_runtime = ProcessRuntime::new(
            rt_name.to_string(),
            version_exec_config,
            packs_base_dir.to_path_buf(),
            runtime_envs_dir.to_path_buf(),
        );

        match coordinate_environment_setup(
            env_manager,
            worker,
            &version_runtime,
            pack_id,
            pack_ref,
            runtime.id,
            &runtime.r#ref,
            Some(version),
            &version_env_suffix,
            &version_env_dir,
            manifest_checksum.as_deref(),
            worker_can_claim,
            pack_dir,
        )
        .await
        {
            Ok(CoordinateSetupOutcome::Installed) => {
                info!(
                    "Version environment '{}' ready for pack '{}'",
                    version_env_suffix, pack_ref,
                );
                pack_result.environments_created.push(version_env_suffix);
            }
            Ok(CoordinateSetupOutcome::Skipped) => {
                pack_result
                    .environments_skipped
                    .push(version_env_suffix.to_string());
            }
            Err(e) => {
                let msg = format!(
                    "Failed to set up version environment '{}' for pack '{}': {}",
                    version_env_suffix, pack_ref, e,
                );
                warn!("{}", msg);
                pack_result.errors.push(msg);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoordinateSetupOutcome {
    Installed,
    Skipped,
}

#[allow(clippy::too_many_arguments)]
async fn coordinate_environment_setup(
    env_manager: &PackEnvironmentManager,
    worker: Option<&Worker>,
    process_runtime: &ProcessRuntime,
    pack_id: i64,
    pack_ref: &str,
    runtime_id: i64,
    runtime_ref: &str,
    runtime_version: Option<&RuntimeVersion>,
    target_name: &str,
    env_dir: &Path,
    manifest_checksum: Option<&str>,
    worker_can_claim: bool,
    pack_dir: &Path,
) -> std::result::Result<CoordinateSetupOutcome, String> {
    let coordinated = env_manager
        .upsert_coordinated_environment(UpsertCoordinatedEnvironmentInput {
            pack_id,
            pack_ref,
            runtime_id,
            runtime_ref,
            runtime_version,
            env_path: env_dir,
            manifest_checksum,
        })
        .await
        .map_err(|e| e.to_string())?;

    let target_label = coordinated_target_label(target_name, runtime_version);
    if target_is_ready(
        &coordinated,
        manifest_checksum,
        process_runtime,
        pack_dir,
        env_dir,
    ) {
        return Ok(CoordinateSetupOutcome::Skipped);
    }

    if !worker_can_claim {
        debug!(
            "Worker cannot claim coordinated environment '{}' for pack '{}'",
            target_label, pack_ref
        );
        return Ok(CoordinateSetupOutcome::Skipped);
    }

    let Some(worker) = worker else {
        return Ok(CoordinateSetupOutcome::Skipped);
    };

    for attempt in 0..INSTALL_CLAIM_ATTEMPTS {
        wait_for_claim_jitter(&target_label, attempt).await;

        if let Some(record) = env_manager
            .get_coordinated_environment(&coordinated.env_key)
            .await
            .map_err(|e| e.to_string())?
        {
            if target_is_ready(
                &record,
                manifest_checksum,
                process_runtime,
                pack_dir,
                env_dir,
            ) {
                return Ok(CoordinateSetupOutcome::Skipped);
            }

            if target_needs_reinstall(process_runtime, pack_dir, env_dir)
                && record.status == PackEnvironmentStatus::Ready
            {
                env_manager
                    .mark_coordinated_environment_outdated(&record.env_key)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }

        let Some(claimed) = env_manager
            .claim_coordinated_environment(
                &coordinated.env_key,
                worker.id,
                INSTALL_CLAIM_LEASE_SECONDS,
            )
            .await
            .map_err(|e| e.to_string())?
        else {
            continue;
        };

        let renewer = spawn_claim_renewer(
            env_manager.clone(),
            claimed.env_key.clone(),
            worker.id,
            target_label.clone(),
        );

        let install_result = process_runtime
            .setup_pack_environment(pack_dir, env_dir)
            .await;
        let _ = renewer.send(());

        match install_result {
            Ok(()) => {
                env_manager
                    .mark_coordinated_environment_ready(
                        &claimed.env_key,
                        worker.id,
                        manifest_checksum,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                return Ok(CoordinateSetupOutcome::Installed);
            }
            Err(e) => {
                let error_message = e.to_string();
                env_manager
                    .mark_coordinated_environment_failed(
                        &claimed.env_key,
                        worker.id,
                        &error_message,
                    )
                    .await
                    .map_err(|mark_err| mark_err.to_string())?;

                if attempt + 1 == INSTALL_CLAIM_ATTEMPTS {
                    return Err(format!(
                        "Failed to set up coordinated environment '{}' for pack '{}': {}",
                        target_label, pack_ref, error_message
                    ));
                }
            }
        }
    }

    Ok(CoordinateSetupOutcome::Skipped)
}

fn target_is_ready(
    record: &attune_common::pack_environment::CoordinatedPackEnvironment,
    manifest_checksum: Option<&str>,
    process_runtime: &ProcessRuntime,
    pack_dir: &Path,
    env_dir: &Path,
) -> bool {
    record.status == PackEnvironmentStatus::Ready
        && record.manifest_checksum.as_deref() == manifest_checksum
        && !target_needs_reinstall(process_runtime, pack_dir, env_dir)
}

fn target_needs_reinstall(
    process_runtime: &ProcessRuntime,
    pack_dir: &Path,
    env_dir: &Path,
) -> bool {
    let config = process_runtime.config();
    if config.environment.is_none() {
        return false;
    }

    if !env_dir.exists() {
        return true;
    }

    if let Some(env_cfg) = &config.environment {
        if let Some(interpreter_template) = &env_cfg.interpreter_path {
            let mut vars = HashMap::new();
            vars.insert("env_dir", env_dir.to_string_lossy().to_string());
            vars.insert("pack_dir", pack_dir.to_string_lossy().to_string());
            let resolved = attune_common::models::runtime::RuntimeExecutionConfig::resolve_template(
                interpreter_template,
                &vars,
            );
            if !std::path::Path::new(&resolved).exists() {
                return true;
            }
        }
    }

    false
}

async fn dependency_manifest_checksum(
    exec_config: &attune_common::models::runtime::RuntimeExecutionConfig,
    pack_dir: &Path,
) -> Option<String> {
    use sha2::{Digest, Sha256};

    let manifest_file = exec_config.dependencies.as_ref()?.manifest_file.as_str();
    let manifest_path = pack_dir.join(manifest_file);
    let data = tokio::fs::read(manifest_path).await.ok()?;
    Some(
        Sha256::digest(&data)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn coordinated_target_label(target_name: &str, runtime_version: Option<&RuntimeVersion>) -> String {
    runtime_version
        .map(|version| format!("{target_name} ({})", version.version))
        .unwrap_or_else(|| target_name.to_string())
}

async fn wait_for_claim_jitter(target_label: &str, attempt: usize) {
    let delay_ms = jitter_delay_ms();
    debug!(
        "Waiting {}ms before claim attempt {} for coordinated environment '{}'",
        delay_ms,
        attempt + 1,
        target_label,
    );
    sleep(Duration::from_millis(delay_ms)).await;
}

fn jitter_delay_ms() -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&Uuid::new_v4().into_bytes()[..8]);
    INSTALL_JITTER_MIN_MS + (u64::from_le_bytes(bytes) % INSTALL_JITTER_SPAN_MS)
}

fn spawn_claim_renewer(
    env_manager: PackEnvironmentManager,
    env_key: String,
    worker_id: i64,
    target_label: String,
) -> oneshot::Sender<()> {
    let (tx, mut rx) = oneshot::channel();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(INSTALL_CLAIM_RENEW_SECONDS));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        interval.tick().await;

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match env_manager
                        .renew_coordinated_environment_claim(&env_key, worker_id, INSTALL_CLAIM_LEASE_SECONDS)
                        .await
                    {
                        Ok(true) => {}
                        Ok(false) => {
                            debug!(
                                "Claim renewer for '{}' stopped because the install claim is no longer owned",
                                target_label
                            );
                            break;
                        }
                        Err(e) => {
                            warn!(
                                "Failed to renew coordinated install claim for '{}': {}",
                                target_label, e
                            );
                        }
                    }
                }
                _ = &mut rx => break,
            }
        }
    });

    tx
}

async fn load_worker_for_env_setup(db_pool: &PgPool, worker_id: i64) -> Option<Worker> {
    match WorkerRepository::find_by_id(db_pool, worker_id).await {
        Ok(Some(worker)) => Some(worker),
        Ok(None) => {
            warn!(
                "Worker {} not found during environment setup; skipping version-specific filtering",
                worker_id
            );
            None
        }
        Err(e) => {
            warn!(
                "Failed to load worker {} during environment setup: {}. Skipping version-specific filtering",
                worker_id, e
            );
            None
        }
    }
}

#[cfg(test)]
fn filter_versions_for_worker(
    versions: &[RuntimeVersion],
    runtime: &Runtime,
    worker: Option<&Worker>,
    requirements: Option<&RuntimeRequirementProfile>,
) -> Vec<RuntimeVersion> {
    let Some(worker) = worker else {
        return Vec::new();
    };

    let advertised_versions = worker_runtime_versions_for_runtime(worker, runtime);
    if advertised_versions.is_empty() {
        return Vec::new();
    }

    versions
        .iter()
        .filter(|version| {
            version_matches_worker(version, &advertised_versions)
                && version_qualifies_for_pack(version, requirements)
        })
        .cloned()
        .collect()
}

async fn collect_runtime_requirements(
    db_pool: &PgPool,
    metadata_cache: &MetadataCache,
    actions: &[attune_common::models::action::Action],
) -> HashMap<i64, RuntimeRequirementProfile> {
    let mut requirements = HashMap::new();

    for action in actions {
        if let Some(runtime_id) = action.runtime {
            let profile = requirements
                .entry(runtime_id)
                .or_insert_with(RuntimeRequirementProfile::default);

            match action
                .runtime_version_constraint
                .as_deref()
                .map(str::trim)
                .filter(|constraint| !constraint.is_empty())
            {
                Some(constraint) => {
                    if !profile
                        .constraints
                        .iter()
                        .any(|existing| existing == constraint)
                    {
                        profile.constraints.push(constraint.to_string());
                    }
                }
                None => profile.any_version = true,
            }
        }

        for (required_runtime, constraint) in action.required_worker_runtime_constraints() {
            if let Some(runtime_id) =
                resolve_required_runtime_id(db_pool, metadata_cache, &required_runtime).await
            {
                let profile = requirements
                    .entry(runtime_id)
                    .or_insert_with(RuntimeRequirementProfile::default);
                if constraint.trim() == "*" {
                    profile.any_version = true;
                } else if !profile
                    .constraints
                    .iter()
                    .any(|existing| existing == &constraint)
                {
                    profile.constraints.push(constraint);
                }
            } else {
                warn!(
                    "Failed to resolve required worker runtime '{}' while collecting versioned pack environment requirements",
                    required_runtime
                );
            }
        }
    }

    requirements
}

async fn resolve_required_runtime_id(
    db_pool: &PgPool,
    metadata_cache: &MetadataCache,
    requirement: &str,
) -> Option<i64> {
    let normalized_requirement = normalize_runtime_name(requirement);
    let cached = CachedMetadataRepository::new(db_pool, metadata_cache);

    match cached.find_runtime_by_alias(&normalized_requirement).await {
        Ok(Some(runtime)) => return Some(runtime.id),
        Ok(None) => {}
        Err(e) => {
            warn!(
                "Failed to resolve required worker runtime '{}' by alias during env setup: {}",
                requirement, e
            );
            return None;
        }
    }

    match cached.find_runtime_by_name(&normalized_requirement).await {
        Ok(Some(runtime)) => return Some(runtime.id),
        Ok(None) => {}
        Err(e) => {
            warn!(
                "Failed to resolve required worker runtime '{}' by name during env setup: {}",
                requirement, e
            );
            return None;
        }
    }

    match cached.find_runtime_by_ref(requirement).await {
        Ok(Some(runtime)) => Some(runtime.id),
        Ok(None) => None,
        Err(e) => {
            warn!(
                "Failed to resolve required worker runtime '{}' by ref during env setup: {}",
                requirement, e
            );
            None
        }
    }
}

fn version_qualifies_for_pack(
    version: &RuntimeVersion,
    requirements: Option<&RuntimeRequirementProfile>,
) -> bool {
    let Some(requirements) = requirements else {
        return false;
    };

    if requirements.any_version || requirements.constraints.is_empty() {
        return true;
    }

    requirements
        .constraints
        .iter()
        .any(|constraint| matches_constraint(&version.version, constraint).unwrap_or(false))
}

fn worker_runtime_versions_for_runtime(worker: &Worker, runtime: &Runtime) -> Vec<String> {
    let mut versions = Vec::new();
    let candidate_runtime_names: Vec<String> = runtime
        .aliases
        .iter()
        .map(|alias| normalize_runtime_name(alias))
        .chain(std::iter::once(normalize_runtime_name(&runtime.name)))
        .collect();

    let Some(capabilities) = worker
        .capabilities
        .as_ref()
        .and_then(|value| value.as_object())
    else {
        return versions;
    };

    if let Some(runtime_versions) = capabilities.get("runtime_versions") {
        if let Some(runtime_versions_obj) = runtime_versions.as_object() {
            for runtime_name in &candidate_runtime_names {
                if let Some(version_values) = runtime_versions_obj.get(runtime_name) {
                    if let Some(version_array) = version_values.as_array() {
                        versions.extend(
                            version_array
                                .iter()
                                .filter_map(|value| value.as_str().map(ToOwned::to_owned)),
                        );
                    }
                }
            }
        }
    }

    if versions.is_empty() {
        if let Some(detected_interpreters) = capabilities.get("detected_interpreters") {
            if let Some(interpreters) = detected_interpreters.as_array() {
                for interpreter in interpreters {
                    let Some(name) = interpreter.get("name").and_then(|value| value.as_str())
                    else {
                        continue;
                    };

                    if !candidate_runtime_names
                        .iter()
                        .any(|candidate| candidate == &normalize_runtime_name(name))
                    {
                        continue;
                    }

                    if let Some(version) =
                        interpreter.get("version").and_then(|value| value.as_str())
                    {
                        versions.push(version.to_string());
                    }
                }
            }
        }
    }

    versions.sort();
    versions.dedup();
    versions
}

fn version_matches_worker(version: &RuntimeVersion, advertised_versions: &[String]) -> bool {
    advertised_versions.iter().any(|advertised_version| {
        advertised_version == &version.version
            || matches_constraint(advertised_version, &version.version).unwrap_or(false)
    })
}

/// Determine the runtime filter from the `ATTUNE_WORKER_RUNTIMES` environment variable.
///
/// Returns `None` if the variable is not set (meaning all runtimes are accepted).
pub fn runtime_filter_from_env() -> Option<Vec<String>> {
    std::env::var("ATTUNE_WORKER_RUNTIMES")
        .ok()
        .map(|val| parse_runtime_filter(&val))
}

/// Parse a comma-separated runtime filter string into a list of lowercase runtime names.
/// Empty entries are filtered out.
fn parse_runtime_filter(val: &str) -> Vec<String> {
    val.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_parse_runtime_filter_values() {
        let filter = parse_runtime_filter("shell,Python, Node");
        assert_eq!(filter, vec!["shell", "python", "node"]);
    }

    #[test]
    fn test_parse_runtime_filter_empty() {
        let filter = parse_runtime_filter("");
        assert!(filter.is_empty());
    }

    #[test]
    fn test_parse_runtime_filter_whitespace() {
        let filter = parse_runtime_filter("  shell , , python  ");
        assert_eq!(filter, vec!["shell", "python"]);
    }

    #[test]
    fn test_pack_env_setup_result_defaults() {
        let result = PackEnvSetupResult {
            pack_ref: "test".to_string(),
            environments_created: vec![],
            environments_skipped: vec![],
            errors: vec![],
        };
        assert_eq!(result.pack_ref, "test");
        assert!(result.environments_created.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_startup_scan_result_defaults() {
        let result = StartupScanResult {
            packs_scanned: 0,
            environments_created: 0,
            environments_skipped: 0,
            errors: vec![],
        };
        assert_eq!(result.packs_scanned, 0);
        assert_eq!(result.environments_created, 0);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_jitter_delay_ms_in_expected_range() {
        for _ in 0..32 {
            let delay = jitter_delay_ms();
            assert!(delay >= INSTALL_JITTER_MIN_MS);
            assert!(delay < INSTALL_JITTER_MIN_MS + INSTALL_JITTER_SPAN_MS);
        }
    }

    #[test]
    fn test_filter_versions_for_worker_uses_runtime_versions_capability() {
        let runtime = Runtime {
            id: 1,
            r#ref: "core.python".to_string(),
            pack: None,
            pack_ref: Some("core".to_string()),
            description: None,
            name: "Python".to_string(),
            aliases: vec!["python".to_string(), "python3".to_string()],
            distributions: serde_json::json!({}),
            installation: None,
            installers: serde_json::json!({}),
            execution_config: serde_json::json!({}),
            auto_detected: false,
            detection_config: serde_json::json!({}),
            created: Utc::now(),
            updated: Utc::now(),
        };
        let worker = Worker {
            id: 1,
            name: "worker-1".to_string(),
            worker_type: attune_common::models::WorkerType::Local,
            worker_role: attune_common::models::WorkerRole::Action,
            runtime: None,
            host: None,
            port: None,
            status: Some(attune_common::models::WorkerStatus::Active),
            capabilities: Some(serde_json::json!({
                "runtime_versions": {
                    "python": ["3.12.13", "3.11.9"]
                }
            })),
            meta: None,
            last_heartbeat: None,
            cordoned: false,
            cordon_reason: None,
            cordoned_by: None,
            cordoned_at: None,
            created: Utc::now(),
            updated: Utc::now(),
        };
        let versions = vec![
            RuntimeVersion {
                id: 1,
                runtime: 1,
                runtime_ref: "core.python".to_string(),
                version: "3.12".to_string(),
                version_major: Some(3),
                version_minor: Some(12),
                version_patch: None,
                execution_config: serde_json::json!({}),
                distributions: serde_json::json!({}),
                is_default: false,
                available: false,
                verified_at: None,
                meta: serde_json::json!({}),
                created: Utc::now(),
                updated: Utc::now(),
            },
            RuntimeVersion {
                id: 2,
                runtime: 1,
                runtime_ref: "core.python".to_string(),
                version: "3.13".to_string(),
                version_major: Some(3),
                version_minor: Some(13),
                version_patch: None,
                execution_config: serde_json::json!({}),
                distributions: serde_json::json!({}),
                is_default: false,
                available: true,
                verified_at: None,
                meta: serde_json::json!({}),
                created: Utc::now(),
                updated: Utc::now(),
            },
        ];

        let requirements = RuntimeRequirementProfile {
            any_version: false,
            constraints: vec![">=3.12,<3.13".to_string()],
        };
        let filtered =
            filter_versions_for_worker(&versions, &runtime, Some(&worker), Some(&requirements));

        assert_eq!(
            filtered
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            vec!["3.12"]
        );
    }

    #[test]
    fn test_filter_versions_for_worker_falls_back_to_detected_interpreters() {
        let runtime = Runtime {
            id: 1,
            r#ref: "core.python".to_string(),
            pack: None,
            pack_ref: Some("core".to_string()),
            description: None,
            name: "Python".to_string(),
            aliases: vec!["python".to_string(), "python3".to_string()],
            distributions: serde_json::json!({}),
            installation: None,
            installers: serde_json::json!({}),
            execution_config: serde_json::json!({}),
            auto_detected: false,
            detection_config: serde_json::json!({}),
            created: Utc::now(),
            updated: Utc::now(),
        };
        let worker = Worker {
            id: 1,
            name: "agent-1".to_string(),
            worker_type: attune_common::models::WorkerType::Local,
            worker_role: attune_common::models::WorkerRole::Action,
            runtime: None,
            host: None,
            port: None,
            status: Some(attune_common::models::WorkerStatus::Active),
            capabilities: Some(serde_json::json!({
                "detected_interpreters": [
                    { "name": "python", "version": "3.12.13", "path": "/usr/bin/python3" }
                ]
            })),
            meta: None,
            last_heartbeat: None,
            cordoned: false,
            cordon_reason: None,
            cordoned_by: None,
            cordoned_at: None,
            created: Utc::now(),
            updated: Utc::now(),
        };
        let versions = vec![RuntimeVersion {
            id: 1,
            runtime: 1,
            runtime_ref: "core.python".to_string(),
            version: "3.12".to_string(),
            version_major: Some(3),
            version_minor: Some(12),
            version_patch: None,
            execution_config: serde_json::json!({}),
            distributions: serde_json::json!({}),
            is_default: false,
            available: false,
            verified_at: None,
            meta: serde_json::json!({}),
            created: Utc::now(),
            updated: Utc::now(),
        }];

        let requirements = RuntimeRequirementProfile {
            any_version: false,
            constraints: vec![">=3.12,<3.13".to_string()],
        };
        let filtered =
            filter_versions_for_worker(&versions, &runtime, Some(&worker), Some(&requirements));

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].version, "3.12");
    }

    #[test]
    fn test_filter_versions_for_worker_returns_empty_without_worker_context() {
        let runtime = Runtime {
            id: 1,
            r#ref: "core.python".to_string(),
            pack: None,
            pack_ref: Some("core".to_string()),
            description: None,
            name: "Python".to_string(),
            aliases: vec!["python".to_string(), "python3".to_string()],
            distributions: serde_json::json!({}),
            installation: None,
            installers: serde_json::json!({}),
            execution_config: serde_json::json!({}),
            auto_detected: false,
            detection_config: serde_json::json!({}),
            created: Utc::now(),
            updated: Utc::now(),
        };
        let versions = vec![RuntimeVersion {
            id: 1,
            runtime: 1,
            runtime_ref: "core.python".to_string(),
            version: "3.12".to_string(),
            version_major: Some(3),
            version_minor: Some(12),
            version_patch: None,
            execution_config: serde_json::json!({}),
            distributions: serde_json::json!({}),
            is_default: false,
            available: true,
            verified_at: None,
            meta: serde_json::json!({}),
            created: Utc::now(),
            updated: Utc::now(),
        }];

        let requirements = RuntimeRequirementProfile {
            any_version: true,
            constraints: Vec::new(),
        };
        let filtered = filter_versions_for_worker(&versions, &runtime, None, Some(&requirements));

        assert!(filtered.is_empty());
    }

    #[test]
    fn test_filter_versions_for_worker_respects_pack_constraints() {
        let runtime = Runtime {
            id: 1,
            r#ref: "core.python".to_string(),
            pack: None,
            pack_ref: Some("core".to_string()),
            description: None,
            name: "Python".to_string(),
            aliases: vec!["python".to_string(), "python3".to_string()],
            distributions: serde_json::json!({}),
            installation: None,
            installers: serde_json::json!({}),
            execution_config: serde_json::json!({}),
            auto_detected: false,
            detection_config: serde_json::json!({}),
            created: Utc::now(),
            updated: Utc::now(),
        };
        let worker = Worker {
            id: 1,
            name: "worker-1".to_string(),
            worker_type: attune_common::models::WorkerType::Local,
            worker_role: attune_common::models::WorkerRole::Action,
            runtime: None,
            host: None,
            port: None,
            status: Some(attune_common::models::WorkerStatus::Active),
            capabilities: Some(serde_json::json!({
                "runtime_versions": {
                    "python": ["3.12.13", "3.13.2"]
                }
            })),
            meta: None,
            last_heartbeat: None,
            cordoned: false,
            cordon_reason: None,
            cordoned_by: None,
            cordoned_at: None,
            created: Utc::now(),
            updated: Utc::now(),
        };
        let versions = vec![
            RuntimeVersion {
                id: 1,
                runtime: 1,
                runtime_ref: "core.python".to_string(),
                version: "3.12".to_string(),
                version_major: Some(3),
                version_minor: Some(12),
                version_patch: None,
                execution_config: serde_json::json!({}),
                distributions: serde_json::json!({}),
                is_default: false,
                available: true,
                verified_at: None,
                meta: serde_json::json!({}),
                created: Utc::now(),
                updated: Utc::now(),
            },
            RuntimeVersion {
                id: 2,
                runtime: 1,
                runtime_ref: "core.python".to_string(),
                version: "3.13".to_string(),
                version_major: Some(3),
                version_minor: Some(13),
                version_patch: None,
                execution_config: serde_json::json!({}),
                distributions: serde_json::json!({}),
                is_default: false,
                available: true,
                verified_at: None,
                meta: serde_json::json!({}),
                created: Utc::now(),
                updated: Utc::now(),
            },
        ];
        let requirements = RuntimeRequirementProfile {
            any_version: false,
            constraints: vec!["~3.12".to_string()],
        };

        let filtered =
            filter_versions_for_worker(&versions, &runtime, Some(&worker), Some(&requirements));

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].version, "3.12");
    }
}
