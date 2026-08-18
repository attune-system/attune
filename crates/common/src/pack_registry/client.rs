//! Registry client for fetching and parsing pack indices
//!
//! This module provides functionality for:
//! - Fetching approved HTTP(S) index files
//! - Caching indices with TTL-based expiration
//! - Searching packs across multiple registries
//! - Handling authenticated registries

use super::{
    validate_remote_pack_url, Checksum, InstallSource, OutboundUrlPolicy, PackIndex, PackIndexEntry,
};
use crate::config::{PackRegistryConfig, RegistryIndexConfig};
use crate::error::{Error, Result};
use crate::schema::RefValidator;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

const SUPPORTED_INDEX_VERSION: &str = "1.0";

/// Cached registry index with expiration
#[derive(Clone)]
struct CachedIndex {
    /// The parsed index
    index: PackIndex,

    /// When this cache entry was created
    cached_at: SystemTime,

    /// TTL in seconds
    ttl: u64,
}

impl CachedIndex {
    /// Check if this cache entry is expired
    fn is_expired(&self) -> bool {
        match SystemTime::now().duration_since(self.cached_at) {
            Ok(duration) => duration.as_secs() > self.ttl,
            Err(_) => true, // If time went backwards, consider expired
        }
    }
}

/// Registry client for fetching and managing pack indices
pub struct RegistryClient {
    /// Configuration
    config: PackRegistryConfig,

    outbound_policy: OutboundUrlPolicy,

    /// Cache of fetched indices (URL -> CachedIndex)
    cache: Arc<RwLock<HashMap<String, CachedIndex>>>,
}

impl RegistryClient {
    /// Create a new registry client
    pub fn new(config: PackRegistryConfig) -> Result<Self> {
        let outbound_policy = OutboundUrlPolicy::from_config(&config)?;
        for registry in &config.indices {
            validate_remote_pack_url(&registry.url)?;
            validate_registry_headers(&registry.headers)?;
        }

        Ok(Self {
            config,
            outbound_policy,
            cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Get all enabled registries sorted by priority (lower number = higher priority)
    pub fn get_registries(&self) -> Vec<RegistryIndexConfig> {
        self.get_registries_including_disabled(false)
    }

    /// Get registries in priority order, optionally retaining disabled entries.
    pub fn get_registries_including_disabled(
        &self,
        include_disabled: bool,
    ) -> Vec<RegistryIndexConfig> {
        let mut registries: Vec<_> = self
            .config
            .indices
            .iter()
            .filter(|r| include_disabled || r.enabled)
            .cloned()
            .collect();

        // Sort by priority (ascending)
        registries.sort_by_key(|r| r.priority);

        registries
    }

    /// Fetch a pack index from a registry
    pub async fn fetch_index(&self, registry: &RegistryIndexConfig) -> Result<PackIndex> {
        validate_remote_pack_url(&registry.url)?;
        // Check cache first if caching is enabled
        if self.config.cache_enabled {
            if let Some(cached) = self.get_cached_index(&registry.url) {
                if !cached.is_expired() {
                    tracing::debug!("Using cached index for registry: {}", registry.url);
                    return Ok(cached.index);
                }
            }
        }

        // Fetch fresh index
        tracing::info!("Fetching index from registry: {}", registry.url);
        let index = self.fetch_index_from_url(registry).await?;

        // Cache the result
        if self.config.cache_enabled {
            self.cache_index(&registry.url, index.clone());
        }

        Ok(index)
    }

    /// Fetch index from URL (bypassing cache)
    async fn fetch_index_from_url(&self, registry: &RegistryIndexConfig) -> Result<PackIndex> {
        let validated = self.outbound_policy.validate(&registry.url).await?;

        // Build HTTP request
        let mut request = validated.client.get(validated.url);

        // Add custom headers
        for (key, value) in &registry.headers {
            request = request.header(key, value);
        }

        // Send request
        let response = request
            .send()
            .await
            .map_err(|e| Error::internal(format!("Failed to fetch registry index: {}", e)))?;

        // Check status
        if !response.status().is_success() {
            return Err(Error::internal(format!(
                "Registry returned error status {}: {}",
                response.status(),
                registry.url
            )));
        }

        if response
            .content_length()
            .is_some_and(|length| length > self.config.index_max_bytes)
        {
            return Err(Error::validation(
                "Registry index exceeds configured size limit",
            ));
        }
        let bytes = read_bounded_response(response, self.config.index_max_bytes).await?;
        let index: PackIndex = serde_json::from_slice(&bytes)
            .map_err(|e| Error::internal(format!("Failed to parse registry index: {}", e)))?;
        validate_pack_index(&index)?;

        Ok(index)
    }

    /// Get cached index if available
    fn get_cached_index(&self, url: &str) -> Option<CachedIndex> {
        let cache = self.cache.read().ok()?;
        cache.get(url).cloned()
    }

    /// Cache an index
    fn cache_index(&self, url: &str, index: PackIndex) {
        let cached = CachedIndex {
            index,
            cached_at: SystemTime::now(),
            ttl: self.config.cache_ttl,
        };

        if let Ok(mut cache) = self.cache.write() {
            cache.insert(url.to_string(), cached);
        }
    }

    /// Clear the cache
    pub fn clear_cache(&self) {
        if let Ok(mut cache) = self.cache.write() {
            cache.clear();
        }
    }

    /// Search for a pack by reference across all registries
    pub async fn search_pack(&self, pack_ref: &str) -> Result<Option<(PackIndexEntry, String)>> {
        let registries = self.get_registries();

        for registry in registries {
            match self.fetch_index(&registry).await {
                Ok(index) => {
                    if let Some(pack) = index.packs.iter().find(|p| p.pack_ref == pack_ref) {
                        return Ok(Some((pack.clone(), registry.url.clone())));
                    }
                }
                Err(e) => {
                    return Err(Error::internal(format!(
                        "Failed to resolve pack from registry '{}': {}",
                        registry.url, e
                    )));
                }
            }
        }

        Ok(None)
    }

    /// Search for packs by keyword across all registries
    pub async fn search_packs(&self, keyword: &str) -> Result<Vec<(PackIndexEntry, String)>> {
        let registries = self.get_registries();
        let mut results = Vec::new();
        let keyword_lower = keyword.to_lowercase();

        for registry in registries {
            match self.fetch_index(&registry).await {
                Ok(index) => {
                    for pack in index.packs {
                        // Search in ref, label, description, and keywords
                        let matches = pack.pack_ref.to_lowercase().contains(&keyword_lower)
                            || pack.label.to_lowercase().contains(&keyword_lower)
                            || pack.description.to_lowercase().contains(&keyword_lower)
                            || pack
                                .keywords
                                .iter()
                                .any(|k| k.to_lowercase().contains(&keyword_lower));

                        if matches {
                            results.push((pack, registry.url.clone()));
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch registry {}: {}", registry.url, e);
                    continue;
                }
            }
        }

        Ok(results)
    }

    /// Get pack from specific registry
    pub async fn get_pack_from_registry(
        &self,
        pack_ref: &str,
        registry_name: &str,
    ) -> Result<Option<PackIndexEntry>> {
        // Find registry by name
        let registry = self
            .config
            .indices
            .iter()
            .find(|r| r.name.as_deref() == Some(registry_name))
            .ok_or_else(|| Error::not_found("registry", "name", registry_name))?;

        let index = self.fetch_index(registry).await?;

        Ok(index.packs.into_iter().find(|p| p.pack_ref == pack_ref))
    }
}

fn validate_pack_index(index: &PackIndex) -> Result<()> {
    if index.registry_name.trim().is_empty() {
        return Err(Error::validation("Registry name must be nonempty"));
    }
    let registry_url = validate_remote_pack_url(&index.registry_url)?;
    if registry_url.scheme() != "https" {
        return Err(Error::validation("Registry URL must use HTTPS"));
    }
    if index.version != SUPPORTED_INDEX_VERSION {
        return Err(Error::validation(format!(
            "Unsupported registry index version '{}'; expected {}",
            index.version, SUPPORTED_INDEX_VERSION
        )));
    }
    chrono::DateTime::parse_from_rfc3339(&index.last_updated)
        .map_err(|_| Error::validation("Registry last_updated must be an RFC 3339 timestamp"))?;

    let mut pack_refs = HashSet::with_capacity(index.packs.len());
    let mut previous_pack_ref: Option<&str> = None;
    for pack in &index.packs {
        RefValidator::validate_pack_ref(&pack.pack_ref)?;
        if !pack_refs.insert(pack.pack_ref.as_str()) {
            return Err(Error::validation(format!(
                "Registry contains duplicate pack ref '{}'",
                pack.pack_ref
            )));
        }
        if previous_pack_ref.is_some_and(|previous| previous >= pack.pack_ref.as_str()) {
            return Err(Error::validation(
                "Registry pack refs must be unique and sorted",
            ));
        }
        previous_pack_ref = Some(&pack.pack_ref);
        semver::Version::parse(&pack.version).map_err(|_| {
            Error::validation(format!(
                "Registry pack '{}' has an invalid semantic version",
                pack.pack_ref
            ))
        })?;
        for (field, value) in [
            ("label", pack.label.as_str()),
            ("author", pack.author.as_str()),
            ("license", pack.license.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(Error::validation(format!(
                    "Registry pack '{}' has an empty {}",
                    pack.pack_ref, field
                )));
            }
        }
        if let Some(homepage) = &pack.homepage {
            url::Url::parse(homepage).map_err(|_| {
                Error::validation(format!(
                    "Registry pack '{}' has an invalid homepage URL",
                    pack.pack_ref
                ))
            })?;
        }
        if let Some(repository) = &pack.repository {
            let repository = url::Url::parse(repository).map_err(|_| {
                Error::validation(format!(
                    "Registry pack '{}' has an invalid repository URL",
                    pack.pack_ref
                ))
            })?;
            if repository.scheme() != "https" {
                return Err(Error::validation(format!(
                    "Registry repository URL for '{}' must use HTTPS",
                    pack.pack_ref
                )));
            }
        }
        validate_unique_values(&pack.pack_ref, "keywords", &pack.keywords)?;
        validate_unique_values(&pack.pack_ref, "runtime_deps", &pack.runtime_deps)?;
        if let Some(meta) = &pack.meta {
            validate_unique_values(
                &pack.pack_ref,
                "meta.tested_attune_versions",
                &meta.tested_attune_versions,
            )?;
        }
        if pack.install_sources.is_empty() {
            return Err(Error::validation(format!(
                "Registry pack '{}' has no install sources",
                pack.pack_ref
            )));
        }

        for source in &pack.install_sources {
            if source.url().trim() != source.url() {
                return Err(Error::validation(format!(
                    "Registry source URL for '{}' contains surrounding whitespace",
                    pack.pack_ref
                )));
            }
            let url = validate_remote_pack_url(source.url())?;
            if url.scheme() != "https" {
                return Err(Error::validation(format!(
                    "Registry source URLs for '{}' must use HTTPS",
                    pack.pack_ref
                )));
            }
            if let InstallSource::Git { git_ref, .. } = source {
                if git_ref.as_deref().is_none_or(|git_ref| {
                    git_ref.trim().is_empty()
                        || git_ref.trim() != git_ref
                        || git_ref.starts_with('-')
                        || git_ref.chars().any(char::is_control)
                }) {
                    return Err(Error::validation(format!(
                        "Registry Git source for '{}' has an invalid ref",
                        pack.pack_ref
                    )));
                }
            }
            Checksum::parse_registry_sha256(source.checksum()).map_err(|error| {
                Error::validation(format!(
                    "Invalid registry checksum for '{}': {}",
                    pack.pack_ref, error
                ))
            })?;
        }

        for (component_type, components) in [
            ("actions", &pack.contents.actions),
            ("sensors", &pack.contents.sensors),
            ("triggers", &pack.contents.triggers),
            ("rules", &pack.contents.rules),
            ("workflows", &pack.contents.workflows),
        ] {
            let mut names = HashSet::with_capacity(components.len());
            for component in components {
                if component.name.trim().is_empty() || !names.insert(component.name.as_str()) {
                    return Err(Error::validation(format!(
                        "Registry pack '{}' has invalid or duplicate {} component names",
                        pack.pack_ref, component_type
                    )));
                }
            }
        }
    }

    Ok(())
}

fn validate_unique_values(pack_ref: &str, field: &str, values: &[String]) -> Result<()> {
    let mut unique = HashSet::with_capacity(values.len());
    if values.iter().any(|value| !unique.insert(value.as_str())) {
        return Err(Error::validation(format!(
            "Registry pack '{}' has duplicate {} values",
            pack_ref, field
        )));
    }
    Ok(())
}

/// Validate user-managed registry headers before they reach reqwest.
pub fn validate_registry_headers(headers: &HashMap<String, String>) -> Result<()> {
    const MANAGED_HEADERS: &[&str] = &[
        "host",
        "content-length",
        "transfer-encoding",
        "connection",
        "keep-alive",
        "te",
        "trailer",
        "upgrade",
        "proxy-authenticate",
        "proxy-authorization",
        "forwarded",
        "via",
        "x-forwarded",
        "x-real-ip",
    ];

    for (name, value) in headers {
        let normalized = name.to_ascii_lowercase();
        if MANAGED_HEADERS.contains(&normalized.as_str())
            || normalized.starts_with("proxy-")
            || normalized.starts_with("x-forwarded-")
        {
            return Err(Error::validation(format!(
                "Registry header '{}' is managed by the HTTP client and is not allowed",
                name
            )));
        }
        reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| Error::validation(format!("Invalid registry header name '{}'", name)))?;
        reqwest::header::HeaderValue::from_str(value).map_err(|_| {
            Error::validation(format!("Invalid value for registry header '{}'", name))
        })?;
    }
    Ok(())
}

async fn read_bounded_response(response: reqwest::Response, limit: u64) -> Result<Vec<u8>> {
    use futures::StreamExt;

    let mut body = response.bytes_stream();
    let mut output = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk =
            chunk.map_err(|e| Error::internal(format!("Failed to read registry index: {}", e)))?;
        let next_len = output.len().saturating_add(chunk.len());
        if next_len as u64 > limit {
            return Err(Error::validation(
                "Registry index exceeds configured size limit",
            ));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RegistryIndexConfig;
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn valid_index_json() -> Value {
        serde_json::json!({
            "registry_name": "Test",
            "registry_url": "https://registry.example.com",
            "version": "1.0",
            "last_updated": "2026-01-01T00:00:00Z",
            "packs": [{
                "ref": "example",
                "label": "Example",
                "description": "test",
                "version": "1.0.0",
                "author": "Test",
                "license": "MIT",
                "keywords": [],
                "runtime_deps": [],
                "install_sources": [{
                    "type": "git",
                    "url": "https://github.com/example/pack.git",
                    "ref": "0123456789abcdef0123456789abcdef01234567",
                    "checksum": format!("sha256:{}", "a".repeat(64))
                }],
                "contents": {
                    "actions": [],
                    "sensors": [],
                    "triggers": [],
                    "rules": [],
                    "workflows": []
                }
            }]
        })
    }

    fn deserialize_and_validate(value: Value) -> Result<()> {
        let index: PackIndex = serde_json::from_value(value)
            .map_err(|error| Error::validation(format!("Invalid registry index: {error}")))?;
        validate_pack_index(&index)
    }

    #[test]
    fn registry_contract_rejects_weak_and_malformed_checksums() {
        let invalid = [
            format!("md5:{}", "0".repeat(32)),
            format!("sha1:{}", "0".repeat(40)),
            format!("sha512:{}", "0".repeat(128)),
            format!("sha256:{}", "0".repeat(63)),
            format!("sha256:{}", "0".repeat(65)),
            format!("sha256:{}", "A".repeat(64)),
            format!("sha256:{}g", "0".repeat(63)),
        ];

        for checksum in invalid {
            let mut index = valid_index_json();
            index["packs"][0]["install_sources"][0]["checksum"] = checksum.clone().into();
            assert!(
                deserialize_and_validate(index).is_err(),
                "accepted {checksum}"
            );
        }
    }

    #[test]
    fn registry_contract_rejects_duplicate_or_empty_pack_refs() {
        let mut duplicate = valid_index_json();
        let duplicate_pack = duplicate["packs"][0].clone();
        duplicate["packs"]
            .as_array_mut()
            .unwrap()
            .push(duplicate_pack);
        assert!(deserialize_and_validate(duplicate).is_err());

        let mut empty = valid_index_json();
        empty["packs"][0]["ref"] = "  ".into();
        assert!(deserialize_and_validate(empty).is_err());

        let mut invalid = valid_index_json();
        invalid["packs"][0]["ref"] = "../example".into();
        assert!(deserialize_and_validate(invalid).is_err());
    }

    #[test]
    fn registry_contract_requires_git_ref_and_install_source() {
        let mut missing_ref = valid_index_json();
        missing_ref["packs"][0]["install_sources"][0]
            .as_object_mut()
            .unwrap()
            .remove("ref");
        assert!(deserialize_and_validate(missing_ref).is_err());

        let mut empty_ref = valid_index_json();
        empty_ref["packs"][0]["install_sources"][0]["ref"] = " ".into();
        assert!(deserialize_and_validate(empty_ref).is_err());

        let mut option_ref = valid_index_json();
        option_ref["packs"][0]["install_sources"][0]["ref"] = "--config".into();
        assert!(deserialize_and_validate(option_ref).is_err());

        let mut whitespace_ref = valid_index_json();
        whitespace_ref["packs"][0]["install_sources"][0]["ref"] = " main ".into();
        assert!(deserialize_and_validate(whitespace_ref).is_err());

        let mut immutable_ref = valid_index_json();
        immutable_ref["packs"][0]["install_sources"][0]["ref"] =
            "0123456789abcdef0123456789abcdef01234567".into();
        assert!(deserialize_and_validate(immutable_ref).is_ok());

        let mut no_sources = valid_index_json();
        no_sources["packs"][0]["install_sources"] = serde_json::json!([]);
        assert!(deserialize_and_validate(no_sources).is_err());
    }

    #[test]
    fn registry_contract_rejects_unsupported_index_version() {
        let mut index = valid_index_json();
        index["version"] = "1.0.0".into();
        assert!(deserialize_and_validate(index).is_err());
    }

    #[test]
    fn registry_contract_requires_semver_pack_versions_and_rfc3339_timestamps() {
        let mut invalid_version = valid_index_json();
        invalid_version["packs"][0]["version"] = "latest".into();
        assert!(deserialize_and_validate(invalid_version).is_err());

        let mut invalid_timestamp = valid_index_json();
        invalid_timestamp["last_updated"] = "yesterday".into();
        assert!(deserialize_and_validate(invalid_timestamp).is_err());
    }

    #[test]
    fn registry_contract_requires_clean_https_source_urls() {
        for url in [
            "http://example.com/pack.git",
            "https://example.com/pack.git?ref=main",
            " https://example.com/pack.git",
        ] {
            let mut index = valid_index_json();
            index["packs"][0]["install_sources"][0]["url"] = url.into();
            assert!(deserialize_and_validate(index).is_err(), "accepted {url}");
        }
    }

    #[test]
    fn registry_runtime_enforces_producer_semantic_constraints() {
        for field in ["label", "author", "license"] {
            let mut index = valid_index_json();
            index["packs"][0][field] = " ".into();
            assert!(
                deserialize_and_validate(index).is_err(),
                "accepted empty {field}"
            );
        }

        for field in ["keywords", "runtime_deps"] {
            let mut index = valid_index_json();
            index["packs"][0][field] = serde_json::json!(["duplicate", "duplicate"]);
            assert!(
                deserialize_and_validate(index).is_err(),
                "accepted duplicate {field}"
            );
        }

        let mut duplicate_tested_versions = valid_index_json();
        duplicate_tested_versions["packs"][0]["meta"] = serde_json::json!({
            "tested_attune_versions": ["0.3.0", "0.3.0"]
        });
        assert!(deserialize_and_validate(duplicate_tested_versions).is_err());

        let mut invalid_repository = valid_index_json();
        invalid_repository["packs"][0]["repository"] = "http://example.com/pack".into();
        assert!(deserialize_and_validate(invalid_repository).is_err());

        let mut duplicate_component = valid_index_json();
        duplicate_component["packs"][0]["contents"]["actions"] = serde_json::json!([
            {"name": "run", "description": "first"},
            {"name": "run", "description": "second"}
        ]);
        assert!(deserialize_and_validate(duplicate_component).is_err());

        let mut unsorted = valid_index_json();
        let mut earlier = unsorted["packs"][0].clone();
        earlier["ref"] = "another".into();
        unsorted["packs"].as_array_mut().unwrap().push(earlier);
        assert!(deserialize_and_validate(unsorted).is_err());

        assert!(deserialize_and_validate(valid_index_json()).is_ok());
    }

    #[test]
    fn test_cached_index_expiration() {
        let index = PackIndex {
            registry_name: "Test".to_string(),
            registry_url: "https://example.com".to_string(),
            version: "1.0".to_string(),
            last_updated: "2024-01-20T12:00:00Z".to_string(),
            packs: vec![],
        };

        let cached = CachedIndex {
            index,
            cached_at: SystemTime::now(),
            ttl: 3600,
        };

        assert!(!cached.is_expired());

        // Test with expired cache
        let cached_old = CachedIndex {
            index: cached.index.clone(),
            cached_at: SystemTime::now() - Duration::from_secs(7200),
            ttl: 3600,
        };

        assert!(cached_old.is_expired());
    }

    #[test]
    fn test_get_registries_sorted() {
        let config = PackRegistryConfig {
            enabled: true,
            indices: vec![
                RegistryIndexConfig {
                    url: "https://registry3.example.com".to_string(),
                    priority: 3,
                    enabled: true,
                    name: Some("Registry 3".to_string()),
                    headers: HashMap::new(),
                },
                RegistryIndexConfig {
                    url: "https://registry1.example.com".to_string(),
                    priority: 1,
                    enabled: true,
                    name: Some("Registry 1".to_string()),
                    headers: HashMap::new(),
                },
                RegistryIndexConfig {
                    url: "https://registry2.example.com".to_string(),
                    priority: 2,
                    enabled: true,
                    name: Some("Registry 2".to_string()),
                    headers: HashMap::new(),
                },
                RegistryIndexConfig {
                    url: "https://disabled.example.com".to_string(),
                    priority: 0,
                    enabled: false,
                    name: Some("Disabled".to_string()),
                    headers: HashMap::new(),
                },
            ],
            cache_ttl: 3600,
            cache_enabled: true,
            timeout: 120,
            verify_checksums: true,
            allow_unverified_direct_remote_installs: false,
            approved_public_hosts: vec![
                "registry1.example.com".into(),
                "registry2.example.com".into(),
                "registry3.example.com".into(),
            ],
            approved_private_hosts: Vec::new(),
            approved_private_cidrs: Vec::new(),
            allow_http: false,
            connect_timeout: 10,
            index_max_bytes: 1024,
            archive_max_bytes: 1024,
        };

        let client = RegistryClient::new(config).unwrap();
        let registries = client.get_registries();

        assert_eq!(registries.len(), 3); // Disabled one excluded
        assert_eq!(registries[0].priority, 1);
        assert_eq!(registries[1].priority, 2);
        assert_eq!(registries[2].priority, 3);

        let registries = client.get_registries_including_disabled(true);
        assert_eq!(registries.len(), 4);
        assert_eq!(registries[0].name.as_deref(), Some("Disabled"));
    }

    #[test]
    fn registry_client_rejects_query_bearing_static_index() {
        let config = PackRegistryConfig {
            indices: vec![RegistryIndexConfig {
                url: "https://registry.example.com/index.json?token=secret".to_string(),
                priority: 0,
                enabled: true,
                name: None,
                headers: HashMap::new(),
            }],
            approved_public_hosts: vec!["registry.example.com".to_string()],
            ..Default::default()
        };

        let error = match RegistryClient::new(config) {
            Ok(_) => panic!("query-bearing static index was accepted"),
            Err(error) => error,
        };
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn rejects_routing_and_hop_by_hop_headers() {
        for name in [
            "Host",
            "Content-Length",
            "Transfer-Encoding",
            "Connection",
            "Keep-Alive",
            "TE",
            "Trailer",
            "Upgrade",
            "Proxy-Authorization",
            "Forwarded",
            "X-Forwarded-Host",
            "X-Real-IP",
            "Via",
        ] {
            let headers = HashMap::from([(name.to_string(), "value".to_string())]);
            assert!(validate_registry_headers(&headers).is_err(), "{}", name);
        }
        assert!(validate_registry_headers(&HashMap::from([
            ("Authorization".to_string(), "Bearer secret".to_string()),
            ("X-Api-Key".to_string(), "secret".to_string()),
        ]))
        .is_ok());
    }

    #[tokio::test]
    async fn pack_resolution_fails_closed_when_higher_priority_index_is_invalid() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let server_requests = requests.clone();
        let server = tokio::spawn(async move {
            loop {
                let accepted =
                    tokio::time::timeout(Duration::from_millis(250), listener.accept()).await;
                let Ok(Ok((mut stream, _))) = accepted else {
                    break;
                };
                server_requests.fetch_add(1, Ordering::SeqCst);
                let mut request = [0_u8; 1024];
                let read = stream.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..read]);
                let packs = if request.starts_with("GET /higher ") {
                    serde_json::json!([{
                        "ref": "example",
                        "label": "Example",
                        "description": "test",
                        "version": "1.0.0",
                        "author": "Test",
                        "license": "MIT",
                        "runtime_deps": [],
                        "install_sources": [{
                            "type": "archive",
                            "url": "https://example.com/pack.tar.gz?token=secret",
                            "checksum": format!("sha256:{}", "0".repeat(64))
                        }],
                        "contents": {}
                    }])
                } else {
                    serde_json::json!([])
                };
                let body = serde_json::json!({
                    "registry_name": "Test",
                    "registry_url": format!("http://{address}"),
                    "version": "1.0",
                    "last_updated": "2026-01-01T00:00:00Z",
                    "packs": packs
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let registry = |path: &str, priority| RegistryIndexConfig {
            url: format!("http://{address}/{path}"),
            priority,
            enabled: true,
            name: None,
            headers: HashMap::new(),
        };
        let config = PackRegistryConfig {
            indices: vec![registry("higher", 0), registry("lower", 1)],
            approved_public_hosts: Vec::new(),
            approved_private_hosts: vec!["127.0.0.1".to_string()],
            allow_http: true,
            ..Default::default()
        };
        let client = RegistryClient::new(config).unwrap();

        assert!(client.search_pack("example").await.is_err());
        server.await.unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn bounded_reader_rejects_oversized_chunked_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut server_task = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nabcdef\r\n0\r\n\r\n").await.unwrap();
        });
        let response = reqwest::Client::new()
            .get(format!("http://{}", address))
            .send()
            .await
            .unwrap();
        let rejected = read_bounded_response(response, 5).await.is_err();

        match tokio::time::timeout(Duration::from_secs(1), &mut server_task).await {
            Ok(result) => result.unwrap(),
            Err(_) => {
                server_task.abort();
                let _ = server_task.await;
                panic!("mock chunked server did not stop within the timeout");
            }
        }

        assert!(rejected);
    }
}
