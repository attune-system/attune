//! Optional Valkey/Redis-backed cache for relatively static metadata rows.
//!
//! PostgreSQL remains authoritative. This module deliberately exposes small,
//! best-effort primitives that callers can use for read-through and
//! update-after-commit behavior without coupling repositories to Redis APIs.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::time::timeout;
use tracing::{debug, warn};
use utoipa::ToSchema;

use crate::config::MetadataCacheConfig;
use crate::{Error, Result};

pub mod repositories;
pub mod sync;

const CACHE_VERSION: &str = "v1";
const LOCAL_CACHE_TTL: Duration = Duration::from_secs(2);

/// Metadata entity families supported by the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetadataEntity {
    Action,
    Rule,
    Trigger,
    Sensor,
    WorkQueue,
    WorkflowDefinition,
    Policy,
    PermissionSet,
    Runtime,
    RuntimeVersion,
}

impl MetadataEntity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Rule => "rule",
            Self::Trigger => "trigger",
            Self::Sensor => "sensor",
            Self::WorkQueue => "work_queue",
            Self::WorkflowDefinition => "workflow_definition",
            Self::Policy => "policy",
            Self::PermissionSet => "permission_set",
            Self::Runtime => "runtime",
            Self::RuntimeVersion => "runtime_version",
        }
    }

    pub fn enabled_by(self, config: &MetadataCacheConfig) -> bool {
        match self {
            Self::Action => config.entities.actions,
            Self::Rule => config.entities.rules,
            Self::Trigger => config.entities.triggers,
            Self::Sensor => config.entities.sensors,
            Self::WorkQueue => config.entities.work_queues,
            Self::WorkflowDefinition => config.entities.workflow_definitions,
            Self::Policy => config.entities.policies,
            Self::PermissionSet => config.entities.permission_sets,
            Self::Runtime => config.entities.runtimes,
            Self::RuntimeVersion => config.entities.runtime_versions,
        }
    }
}

impl fmt::Display for MetadataEntity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone)]
pub struct MetadataCache {
    inner: Arc<MetadataCacheInner>,
}

struct MetadataCacheInner {
    managers: Vec<ConnectionManager>,
    next_manager: AtomicUsize,
    key_prefix: String,
    ttl: Option<u64>,
    operation_timeout: Duration,
    config: MetadataCacheConfig,
    local_json: RwLock<HashMap<String, LocalCacheEntry<String>>>,
    local_indexes: RwLock<HashMap<String, LocalCacheEntry<Vec<String>>>>,
    stats: MetadataCacheStatsCounters,
}

#[derive(Clone)]
struct LocalCacheEntry<T> {
    value: T,
    expires_at: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MetadataCacheStatsSnapshot {
    pub l2_enabled: bool,
    pub local_ttl_seconds: u64,
    pub local_json_entries: usize,
    pub local_index_entries: usize,
    pub l1_json_hits: u64,
    pub l1_json_misses: u64,
    pub l1_index_hits: u64,
    pub l1_index_misses: u64,
    pub l2_json_hits: u64,
    pub l2_json_misses: u64,
    pub l2_index_hits: u64,
    pub l2_index_misses: u64,
    pub local_only_fallbacks: u64,
    pub writes: u64,
    pub evictions: u64,
    pub errors: u64,
}

#[derive(Default)]
struct MetadataCacheStatsCounters {
    l1_json_hits: AtomicU64,
    l1_json_misses: AtomicU64,
    l1_index_hits: AtomicU64,
    l1_index_misses: AtomicU64,
    l2_json_hits: AtomicU64,
    l2_json_misses: AtomicU64,
    l2_index_hits: AtomicU64,
    l2_index_misses: AtomicU64,
    local_only_fallbacks: AtomicU64,
    writes: AtomicU64,
    evictions: AtomicU64,
    errors: AtomicU64,
}

impl MetadataCache {
    pub fn disabled() -> Self {
        Self::local_only(MetadataCacheConfig::default())
    }

    pub async fn from_config(config: &MetadataCacheConfig) -> Result<Self> {
        if !config.enabled {
            return Ok(Self::local_only(config.clone()));
        }

        let url = config
            .url
            .as_deref()
            .ok_or_else(|| Error::configuration("metadata_cache.url is required"))?;
        let client = redis::Client::open(url)
            .map_err(|e| Error::configuration(format!("invalid metadata_cache.url: {e}")))?;
        let connect_timeout = Duration::from_secs(config.connect_timeout_seconds);
        let connection_count = normalize_max_connections(config.max_connections);
        let mut managers = Vec::with_capacity(connection_count);
        for index in 0..connection_count {
            let manager = timeout(connect_timeout, client.get_connection_manager())
                .await
                .map_err(|_| Error::timeout("timed out connecting to metadata cache"))?
                .map_err(|e| {
                    Error::external_service(format!(
                        "metadata cache connection {}/{} failed: {e}",
                        index + 1,
                        connection_count
                    ))
                })?;
            managers.push(manager);
        }

        Ok(Self {
            inner: Arc::new(MetadataCacheInner {
                managers,
                next_manager: AtomicUsize::new(0),
                key_prefix: normalize_prefix(&config.key_prefix),
                ttl: config.default_ttl_seconds,
                operation_timeout: Duration::from_millis(config.operation_timeout_ms),
                config: config.clone(),
                local_json: RwLock::new(HashMap::new()),
                local_indexes: RwLock::new(HashMap::new()),
                stats: MetadataCacheStatsCounters::default(),
            }),
        })
    }

    fn local_only(config: MetadataCacheConfig) -> Self {
        Self {
            inner: Arc::new(MetadataCacheInner {
                managers: Vec::new(),
                next_manager: AtomicUsize::new(0),
                key_prefix: normalize_prefix(&config.key_prefix),
                ttl: config.default_ttl_seconds,
                operation_timeout: Duration::from_millis(config.operation_timeout_ms),
                config,
                local_json: RwLock::new(HashMap::new()),
                local_indexes: RwLock::new(HashMap::new()),
                stats: MetadataCacheStatsCounters::default(),
            }),
        }
    }

    pub async fn best_effort_from_config(config: &MetadataCacheConfig) -> Self {
        match Self::from_config(config).await {
            Ok(cache) => cache,
            Err(e) => {
                warn!("Metadata cache L2 disabled after initialization failure: {e}");
                Self::local_only(config.clone())
            }
        }
    }

    pub fn is_enabled(&self) -> bool {
        !self.inner.managers.is_empty()
    }

    pub fn entity_enabled(&self, entity: MetadataEntity) -> bool {
        entity.enabled_by(&self.inner.config)
    }

    pub fn key_for_id(&self, entity: MetadataEntity, id: i64) -> Option<String> {
        Some(self.inner.key_for_id(entity, id))
    }

    pub fn key_for_ref(&self, entity: MetadataEntity, ref_str: &str) -> Option<String> {
        Some(self.inner.key_for_ref(entity, ref_str))
    }

    pub fn index_key(&self, entity: MetadataEntity, index: &str) -> Option<String> {
        Some(self.inner.index_key(entity, index))
    }

    pub fn empty_index_key(&self, entity: MetadataEntity, index: &str) -> Option<String> {
        Some(self.inner.empty_index_key(entity, index))
    }

    pub async fn stats_snapshot(&self) -> MetadataCacheStatsSnapshot {
        let inner = self.inner.as_ref();
        MetadataCacheStatsSnapshot {
            l2_enabled: self.is_enabled(),
            local_ttl_seconds: LOCAL_CACHE_TTL.as_secs(),
            local_json_entries: inner.local_json.read().await.len(),
            local_index_entries: inner.local_indexes.read().await.len(),
            l1_json_hits: inner.stats.l1_json_hits.load(Ordering::Relaxed),
            l1_json_misses: inner.stats.l1_json_misses.load(Ordering::Relaxed),
            l1_index_hits: inner.stats.l1_index_hits.load(Ordering::Relaxed),
            l1_index_misses: inner.stats.l1_index_misses.load(Ordering::Relaxed),
            l2_json_hits: inner.stats.l2_json_hits.load(Ordering::Relaxed),
            l2_json_misses: inner.stats.l2_json_misses.load(Ordering::Relaxed),
            l2_index_hits: inner.stats.l2_index_hits.load(Ordering::Relaxed),
            l2_index_misses: inner.stats.l2_index_misses.load(Ordering::Relaxed),
            local_only_fallbacks: inner.stats.local_only_fallbacks.load(Ordering::Relaxed),
            writes: inner.stats.writes.load(Ordering::Relaxed),
            evictions: inner.stats.evictions.load(Ordering::Relaxed),
            errors: inner.stats.errors.load(Ordering::Relaxed),
        }
    }

    pub async fn get_json<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        let inner = self.inner.as_ref();

        if let Some(json) = inner.get_local_json(key).await {
            inner.stats.l1_json_hits.fetch_add(1, Ordering::Relaxed);
            return serde_json::from_str(&json).map(Some).map_err(Into::into);
        }
        inner.stats.l1_json_misses.fetch_add(1, Ordering::Relaxed);

        let Some(mut manager) = inner.connection() else {
            inner
                .stats
                .local_only_fallbacks
                .fetch_add(1, Ordering::Relaxed);
            return Ok(None);
        };
        let value: Option<String> = timeout(inner.operation_timeout, manager.get(key))
            .await
            .map_err(|_| Error::timeout(format!("metadata cache get timed out for key {key}")))?
            .map_err(|e| Error::external_service(format!("metadata cache get failed: {e}")))?;

        match value {
            Some(json) => {
                inner.stats.l2_json_hits.fetch_add(1, Ordering::Relaxed);
                inner.put_local_json(key, json.clone()).await;
                serde_json::from_str(&json).map(Some).map_err(Into::into)
            }
            None => {
                inner.stats.l2_json_misses.fetch_add(1, Ordering::Relaxed);
                Ok(None)
            }
        }
    }

    pub async fn get_json_many<T, I, S>(&self, keys: I) -> Result<Vec<Option<T>>>
    where
        T: DeserializeOwned,
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let inner = self.inner.as_ref();
        let keys: Vec<String> = keys
            .into_iter()
            .map(|key| key.as_ref().to_string())
            .filter(|key| !key.is_empty())
            .collect();
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let mut output: Vec<Option<T>> = Vec::with_capacity(keys.len());
        let mut missing_positions = Vec::new();
        let mut missing_keys = Vec::new();
        for (position, key) in keys.iter().enumerate() {
            if let Some(json) = inner.get_local_json(key).await {
                inner.stats.l1_json_hits.fetch_add(1, Ordering::Relaxed);
                output.push(Some(serde_json::from_str(&json)?));
            } else {
                inner.stats.l1_json_misses.fetch_add(1, Ordering::Relaxed);
                output.push(None);
                missing_positions.push(position);
                missing_keys.push(key.clone());
            }
        }
        if missing_keys.is_empty() {
            return Ok(output);
        }

        let Some(mut manager) = inner.connection() else {
            inner
                .stats
                .local_only_fallbacks
                .fetch_add(missing_keys.len() as u64, Ordering::Relaxed);
            return Ok(output);
        };
        let values: Vec<Option<String>> = timeout(
            inner.operation_timeout,
            redis::cmd("MGET")
                .arg(&missing_keys)
                .query_async(&mut manager),
        )
        .await
        .map_err(|_| Error::timeout("metadata cache multi-get timed out"))?
        .map_err(|e| Error::external_service(format!("metadata cache multi-get failed: {e}")))?;

        for (position, value) in missing_positions.into_iter().zip(values) {
            if let Some(json) = value {
                inner.stats.l2_json_hits.fetch_add(1, Ordering::Relaxed);
                inner.put_local_json(&keys[position], json.clone()).await;
                output[position] = Some(serde_json::from_str(&json)?);
            } else {
                inner.stats.l2_json_misses.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(output)
    }

    pub async fn set_json<T>(&self, key: &str, value: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        let inner = self.inner.as_ref();

        let payload = serde_json::to_string(value)?;
        let Some(mut manager) = inner.connection() else {
            inner.stats.writes.fetch_add(1, Ordering::Relaxed);
            inner.put_local_json(key, payload).await;
            return Ok(());
        };
        if let Some(ttl) = inner.ttl {
            timeout(
                inner.operation_timeout,
                manager.set_ex::<_, _, ()>(key, &payload, ttl),
            )
            .await
            .map_err(|_| Error::timeout(format!("metadata cache set timed out for key {key}")))?
            .map_err(|e| Error::external_service(format!("metadata cache set failed: {e}")))?;
        } else {
            timeout(
                inner.operation_timeout,
                manager.set::<_, _, ()>(key, &payload),
            )
            .await
            .map_err(|_| Error::timeout(format!("metadata cache set timed out for key {key}")))?
            .map_err(|e| Error::external_service(format!("metadata cache set failed: {e}")))?;
        }

        inner.stats.writes.fetch_add(1, Ordering::Relaxed);
        inner.put_local_json(key, payload).await;
        Ok(())
    }

    pub async fn set_json_for_keys<T, I, S>(&self, keys: I, value: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let inner = self.inner.as_ref();
        let keys: Vec<String> = keys
            .into_iter()
            .map(|key| key.as_ref().to_string())
            .filter(|key| !key.is_empty())
            .collect();
        if keys.is_empty() {
            return Ok(());
        }

        let payload = serde_json::to_string(value)?;
        let Some(mut manager) = inner.connection() else {
            inner
                .stats
                .writes
                .fetch_add(keys.len() as u64, Ordering::Relaxed);
            for key in &keys {
                inner.put_local_json(key, payload.clone()).await;
            }
            return Ok(());
        };
        let mut pipe = redis::pipe();
        for key in &keys {
            if let Some(ttl) = inner.ttl {
                pipe.cmd("SETEX").arg(key).arg(ttl).arg(&payload);
            } else {
                pipe.cmd("SET").arg(key).arg(&payload);
            }
        }

        timeout(
            inner.operation_timeout,
            pipe.query_async::<()>(&mut manager),
        )
        .await
        .map_err(|_| Error::timeout("metadata cache pipelined set timed out"))?
        .map_err(|e| {
            Error::external_service(format!("metadata cache pipelined set failed: {e}"))
        })?;
        for key in &keys {
            inner.stats.writes.fetch_add(1, Ordering::Relaxed);
            inner.put_local_json(key, payload.clone()).await;
        }
        Ok(())
    }

    pub async fn delete_keys<I, S>(&self, keys: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let inner = self.inner.as_ref();
        let keys: Vec<String> = keys
            .into_iter()
            .map(|key| key.as_ref().to_string())
            .filter(|key| !key.is_empty())
            .collect();
        if keys.is_empty() {
            return Ok(());
        }

        let Some(mut manager) = inner.connection() else {
            inner
                .stats
                .evictions
                .fetch_add(keys.len() as u64, Ordering::Relaxed);
            inner.delete_local_keys(&keys).await;
            return Ok(());
        };
        timeout(
            inner.operation_timeout,
            redis::cmd("DEL").arg(&keys).query_async::<()>(&mut manager),
        )
        .await
        .map_err(|_| Error::timeout("metadata cache delete timed out"))?
        .map_err(|e| Error::external_service(format!("metadata cache delete failed: {e}")))?;
        inner
            .stats
            .evictions
            .fetch_add(keys.len() as u64, Ordering::Relaxed);
        inner.delete_local_keys(&keys).await;
        Ok(())
    }

    pub async fn add_index_members<I, S>(&self, index_key: &str, members: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let inner = self.inner.as_ref();
        let members: Vec<String> = members
            .into_iter()
            .map(|member| member.as_ref().to_string())
            .filter(|member| !member.is_empty())
            .collect();
        if members.is_empty() {
            return Ok(());
        }

        let Some(mut manager) = inner.connection() else {
            inner.stats.writes.fetch_add(1, Ordering::Relaxed);
            inner.put_local_index(index_key, members).await;
            if let Some(empty_key) = self.empty_index_key_from_index_key(index_key) {
                inner.delete_local_keys(&[empty_key]).await;
            }
            return Ok(());
        };

        let mut pipe = redis::pipe();
        pipe.cmd("DEL").arg(index_key);
        pipe.cmd("SADD").arg(index_key).arg(&members);
        if let Some(empty_key) = self.empty_index_key_from_index_key(index_key) {
            pipe.cmd("DEL").arg(&empty_key);
        }
        if let Some(ttl) = inner.ttl {
            pipe.cmd("EXPIRE").arg(index_key).arg(ttl);
        }
        timeout(
            inner.operation_timeout,
            pipe.query_async::<()>(&mut manager),
        )
        .await
        .map_err(|_| {
            Error::timeout(format!(
                "metadata cache index replace timed out for {index_key}"
            ))
        })?
        .map_err(|e| {
            Error::external_service(format!("metadata cache index replace failed: {e}"))
        })?;

        inner.stats.writes.fetch_add(1, Ordering::Relaxed);
        inner.put_local_index(index_key, members).await;
        Ok(())
    }

    pub async fn add_member_to_indexes<I, S>(&self, index_keys: I, member: &str) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let inner = self.inner.as_ref();
        if member.is_empty() {
            return Ok(());
        }
        let index_keys: Vec<String> = index_keys
            .into_iter()
            .map(|key| key.as_ref().to_string())
            .filter(|key| !key.is_empty())
            .collect();
        if index_keys.is_empty() {
            return Ok(());
        }

        let Some(mut manager) = inner.connection() else {
            inner
                .stats
                .evictions
                .fetch_add(index_keys.len() as u64, Ordering::Relaxed);
            inner.delete_local_indexes(&index_keys).await;
            return Ok(());
        };

        let mut pipe = redis::pipe();
        let mut empty_keys = Vec::new();
        for index_key in &index_keys {
            pipe.cmd("SADD").arg(index_key).arg(member);
            if let Some(empty_key) = self.empty_index_key_from_index_key(index_key) {
                pipe.cmd("DEL").arg(&empty_key);
                empty_keys.push(empty_key);
            }
            if let Some(ttl) = inner.ttl {
                pipe.cmd("EXPIRE").arg(index_key).arg(ttl);
            }
        }

        timeout(
            inner.operation_timeout,
            pipe.query_async::<()>(&mut manager),
        )
        .await
        .map_err(|_| Error::timeout("metadata cache pipelined index add timed out"))?
        .map_err(|e| {
            Error::external_service(format!("metadata cache pipelined index add failed: {e}"))
        })?;
        inner
            .stats
            .writes
            .fetch_add(index_keys.len() as u64, Ordering::Relaxed);
        inner.delete_local_indexes(&index_keys).await;
        inner.delete_local_keys(&empty_keys).await;
        Ok(())
    }

    pub async fn remove_index_members<I, S>(&self, index_key: &str, members: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let inner = self.inner.as_ref();
        let members: Vec<String> = members
            .into_iter()
            .map(|member| member.as_ref().to_string())
            .filter(|member| !member.is_empty())
            .collect();
        if members.is_empty() {
            return Ok(());
        }

        let Some(mut manager) = inner.connection() else {
            inner.stats.evictions.fetch_add(1, Ordering::Relaxed);
            inner.delete_local_index(index_key).await;
            return Ok(());
        };
        timeout(
            inner.operation_timeout,
            redis::cmd("SREM")
                .arg(index_key)
                .arg(&members)
                .query_async::<()>(&mut manager),
        )
        .await
        .map_err(|_| {
            Error::timeout(format!(
                "metadata cache index remove timed out for {index_key}"
            ))
        })?
        .map_err(|e| Error::external_service(format!("metadata cache index remove failed: {e}")))?;

        inner.stats.evictions.fetch_add(1, Ordering::Relaxed);
        inner.delete_local_index(index_key).await;
        Ok(())
    }

    pub async fn remove_member_from_indexes<I, S>(&self, index_keys: I, member: &str) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let inner = self.inner.as_ref();
        if member.is_empty() {
            return Ok(());
        }
        let index_keys: Vec<String> = index_keys
            .into_iter()
            .map(|key| key.as_ref().to_string())
            .filter(|key| !key.is_empty())
            .collect();
        if index_keys.is_empty() {
            return Ok(());
        }

        let Some(mut manager) = inner.connection() else {
            inner
                .stats
                .evictions
                .fetch_add(index_keys.len() as u64, Ordering::Relaxed);
            inner.delete_local_indexes(&index_keys).await;
            return Ok(());
        };

        let mut pipe = redis::pipe();
        for index_key in &index_keys {
            pipe.cmd("SREM").arg(index_key).arg(member);
        }

        timeout(
            inner.operation_timeout,
            pipe.query_async::<()>(&mut manager),
        )
        .await
        .map_err(|_| Error::timeout("metadata cache pipelined index remove timed out"))?
        .map_err(|e| {
            Error::external_service(format!("metadata cache pipelined index remove failed: {e}"))
        })?;
        inner
            .stats
            .evictions
            .fetch_add(index_keys.len() as u64, Ordering::Relaxed);
        inner.delete_local_indexes(&index_keys).await;
        Ok(())
    }

    pub async fn get_index_members(&self, index_key: &str) -> Result<Vec<String>> {
        let inner = self.inner.as_ref();

        if let Some(members) = inner.get_local_index(index_key).await {
            inner.stats.l1_index_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(members);
        }
        inner.stats.l1_index_misses.fetch_add(1, Ordering::Relaxed);

        let Some(mut manager) = inner.connection() else {
            inner
                .stats
                .local_only_fallbacks
                .fetch_add(1, Ordering::Relaxed);
            return Ok(Vec::new());
        };
        let members = timeout(
            inner.operation_timeout,
            redis::cmd("SMEMBERS")
                .arg(index_key)
                .query_async::<Vec<String>>(&mut manager),
        )
        .await
        .map_err(|_| {
            Error::timeout(format!(
                "metadata cache index read timed out for {index_key}"
            ))
        })?
        .map_err(|e| Error::external_service(format!("metadata cache index read failed: {e}")))?;
        if members.is_empty() {
            inner.stats.l2_index_misses.fetch_add(1, Ordering::Relaxed);
        } else {
            inner.stats.l2_index_hits.fetch_add(1, Ordering::Relaxed);
        }
        inner.put_local_index(index_key, members.clone()).await;
        Ok(members)
    }

    pub async fn get_index_members_many<I, S>(&self, index_keys: I) -> Result<Vec<Vec<String>>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let inner = self.inner.as_ref();
        let index_keys: Vec<String> = index_keys
            .into_iter()
            .map(|key| key.as_ref().to_string())
            .filter(|key| !key.is_empty())
            .collect();
        if index_keys.is_empty() {
            return Ok(Vec::new());
        }

        let mut output: Vec<Vec<String>> = vec![Vec::new(); index_keys.len()];
        let mut missing_positions = Vec::new();
        let mut missing_keys = Vec::new();
        for (position, index_key) in index_keys.iter().enumerate() {
            if let Some(members) = inner.get_local_index(index_key).await {
                inner.stats.l1_index_hits.fetch_add(1, Ordering::Relaxed);
                output[position] = members;
            } else {
                inner.stats.l1_index_misses.fetch_add(1, Ordering::Relaxed);
                missing_positions.push(position);
                missing_keys.push(index_key.clone());
            }
        }
        if missing_keys.is_empty() {
            return Ok(output);
        }

        let Some(mut manager) = inner.connection() else {
            inner
                .stats
                .local_only_fallbacks
                .fetch_add(missing_keys.len() as u64, Ordering::Relaxed);
            return Ok(output);
        };

        let mut pipe = redis::pipe();
        for index_key in &missing_keys {
            pipe.cmd("SMEMBERS").arg(index_key);
        }

        let member_sets = timeout(
            inner.operation_timeout,
            pipe.query_async::<Vec<Vec<String>>>(&mut manager),
        )
        .await
        .map_err(|_| Error::timeout("metadata cache pipelined index read timed out"))?
        .map_err(|e| {
            Error::external_service(format!("metadata cache pipelined index read failed: {e}"))
        })?;
        for (position, members) in missing_positions.into_iter().zip(member_sets) {
            if members.is_empty() {
                inner.stats.l2_index_misses.fetch_add(1, Ordering::Relaxed);
            } else {
                inner.stats.l2_index_hits.fetch_add(1, Ordering::Relaxed);
            }
            inner
                .put_local_index(&index_keys[position], members.clone())
                .await;
            output[position] = members;
        }
        Ok(output)
    }

    pub async fn mark_index_empty(&self, empty_index_key: &str) -> Result<()> {
        self.set_json(empty_index_key, &true).await
    }

    pub async fn is_index_marked_empty(&self, empty_index_key: &str) -> Result<bool> {
        self.get_json::<bool>(empty_index_key)
            .await
            .map(|value| value.unwrap_or(false))
    }

    fn empty_index_key_from_index_key(&self, index_key: &str) -> Option<String> {
        Some(format!("{index_key}:empty"))
    }

    pub async fn health_check(&self) -> Result<()> {
        let inner = self.inner.as_ref();
        let Some(mut manager) = inner.connection() else {
            return Ok(());
        };
        let response: String = timeout(
            inner.operation_timeout,
            redis::cmd("PING").query_async(&mut manager),
        )
        .await
        .map_err(|_| Error::timeout("metadata cache ping timed out"))?
        .map_err(|e| Error::external_service(format!("metadata cache ping failed: {e}")))?;
        if response == "PONG" {
            Ok(())
        } else {
            Err(Error::external_service(format!(
                "metadata cache ping returned unexpected response: {response}"
            )))
        }
    }

    pub fn log_best_effort_error(&self, operation: &str, err: &Error) {
        self.inner.stats.errors.fetch_add(1, Ordering::Relaxed);
        if self.is_enabled() {
            warn!(operation, error = %err, "Metadata cache operation failed; PostgreSQL remains authoritative");
        } else {
            debug!(
                operation,
                "Metadata cache operation skipped because cache is disabled"
            );
        }
    }
}

impl MetadataCacheInner {
    fn connection(&self) -> Option<ConnectionManager> {
        if self.managers.is_empty() {
            return None;
        }
        let index = self.next_manager.fetch_add(1, Ordering::Relaxed) % self.managers.len();
        Some(self.managers[index].clone())
    }

    fn key_for_id(&self, entity: MetadataEntity, id: i64) -> String {
        format!(
            "{}:{CACHE_VERSION}:{}:id:{id}",
            self.key_prefix,
            entity.as_str()
        )
    }

    fn key_for_ref(&self, entity: MetadataEntity, ref_str: &str) -> String {
        format!(
            "{}:{CACHE_VERSION}:{}:ref:{}",
            self.key_prefix,
            entity.as_str(),
            escape_key_segment(ref_str)
        )
    }

    fn index_key(&self, entity: MetadataEntity, index: &str) -> String {
        format!(
            "{}:{CACHE_VERSION}:{}:index:{}",
            self.key_prefix,
            entity.as_str(),
            escape_key_segment(index)
        )
    }

    fn empty_index_key(&self, entity: MetadataEntity, index: &str) -> String {
        format!("{}:empty", self.index_key(entity, index))
    }

    async fn get_local_json(&self, key: &str) -> Option<String> {
        let now = Instant::now();
        let cache = self.local_json.read().await;
        cache
            .get(key)
            .filter(|entry| entry.expires_at > now)
            .map(|entry| entry.value.clone())
    }

    async fn put_local_json(&self, key: &str, value: String) {
        self.local_json.write().await.insert(
            key.to_string(),
            LocalCacheEntry {
                value,
                expires_at: Instant::now() + LOCAL_CACHE_TTL,
            },
        );
    }

    async fn delete_local_keys(&self, keys: &[String]) {
        let mut json = self.local_json.write().await;
        let mut indexes = self.local_indexes.write().await;
        for key in keys {
            json.remove(key);
            indexes.remove(key);
        }
    }

    async fn get_local_index(&self, key: &str) -> Option<Vec<String>> {
        let now = Instant::now();
        let cache = self.local_indexes.read().await;
        cache
            .get(key)
            .filter(|entry| entry.expires_at > now)
            .map(|entry| entry.value.clone())
    }

    async fn put_local_index(&self, key: &str, value: Vec<String>) {
        self.local_indexes.write().await.insert(
            key.to_string(),
            LocalCacheEntry {
                value,
                expires_at: Instant::now() + LOCAL_CACHE_TTL,
            },
        );
    }

    async fn delete_local_index(&self, key: &str) {
        self.local_indexes.write().await.remove(key);
    }

    async fn delete_local_indexes(&self, keys: &[String]) {
        let mut indexes = self.local_indexes.write().await;
        for key in keys {
            indexes.remove(key);
        }
    }
}

fn normalize_prefix(prefix: &str) -> String {
    prefix.trim().trim_matches(':').to_string()
}

fn normalize_max_connections(max_connections: u32) -> usize {
    max_connections.max(1) as usize
}

fn escape_key_segment(segment: &str) -> String {
    segment.replace(':', "%3A")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn disabled_cache_reports_local_only_l1() {
        let cache = MetadataCache::disabled();
        assert!(!cache.is_enabled());
        assert!(cache.entity_enabled(MetadataEntity::Action));
        assert_eq!(
            cache.key_for_id(MetadataEntity::Action, 1).as_deref(),
            Some("attune:v1:action:id:1")
        );
    }

    #[test]
    fn entity_names_are_stable() {
        assert_eq!(MetadataEntity::Action.as_str(), "action");
        assert_eq!(
            MetadataEntity::WorkflowDefinition.as_str(),
            "workflow_definition"
        );
        assert_eq!(MetadataEntity::PermissionSet.as_str(), "permission_set");
    }

    #[test]
    fn prefix_normalization_removes_colons() {
        assert_eq!(normalize_prefix(":attune:test:"), "attune:test");
    }

    #[test]
    fn max_connections_normalization_uses_at_least_one_connection() {
        assert_eq!(normalize_max_connections(0), 1);
        assert_eq!(normalize_max_connections(4), 4);
    }

    #[test]
    fn key_segments_escape_colons() {
        assert_eq!(escape_key_segment("core:echo"), "core%3Aecho");
    }

    #[tokio::test]
    async fn stats_track_local_only_json_reads_writes_and_evictions() {
        let cache = MetadataCache::disabled();
        let key = cache.key_for_id(MetadataEntity::Action, 42).unwrap();

        let missing: Option<serde_json::Value> = cache.get_json(&key).await.unwrap();
        assert!(missing.is_none());

        cache
            .set_json(&key, &json!({"ref": "core.echo"}))
            .await
            .unwrap();
        let cached: Option<serde_json::Value> = cache.get_json(&key).await.unwrap();
        assert_eq!(cached, Some(json!({"ref": "core.echo"})));

        cache.delete_keys(vec![key]).await.unwrap();

        let stats = cache.stats_snapshot().await;
        assert!(!stats.l2_enabled);
        assert_eq!(stats.l1_json_hits, 1);
        assert_eq!(stats.l1_json_misses, 1);
        assert_eq!(stats.local_only_fallbacks, 1);
        assert_eq!(stats.writes, 1);
        assert_eq!(stats.evictions, 1);
        assert_eq!(stats.local_json_entries, 0);
    }

    #[tokio::test]
    async fn stats_track_l1_index_hits_and_misses() {
        let cache = MetadataCache::disabled();
        let index_key = cache
            .index_key(MetadataEntity::Action, "by_ref:core.echo")
            .unwrap();

        let missing = cache.get_index_members(&index_key).await.unwrap();
        assert!(missing.is_empty());

        cache
            .inner
            .put_local_index(&index_key, vec!["attune:v1:action:id:1".to_string()])
            .await;
        let members = cache.get_index_members(&index_key).await.unwrap();
        assert_eq!(members, vec!["attune:v1:action:id:1".to_string()]);

        let stats = cache.stats_snapshot().await;
        assert_eq!(stats.l1_index_hits, 1);
        assert_eq!(stats.l1_index_misses, 1);
        assert_eq!(stats.local_only_fallbacks, 1);
        assert_eq!(stats.local_index_entries, 1);
    }

    #[tokio::test]
    async fn local_only_full_index_writes_warm_l1() {
        let cache = MetadataCache::disabled();
        let index_key = cache
            .index_key(MetadataEntity::Action, "pack:core")
            .unwrap();

        cache
            .add_index_members(&index_key, ["core.echo", "core.http_request"])
            .await
            .unwrap();
        let members = cache.get_index_members(&index_key).await.unwrap();

        assert_eq!(members, vec!["core.echo", "core.http_request"]);
        let stats = cache.stats_snapshot().await;
        assert_eq!(stats.writes, 1);
        assert_eq!(stats.evictions, 0);
        assert_eq!(stats.l1_index_hits, 1);
        assert_eq!(stats.l1_index_misses, 0);
        assert_eq!(stats.local_only_fallbacks, 0);
        assert_eq!(stats.local_index_entries, 1);
    }
}
