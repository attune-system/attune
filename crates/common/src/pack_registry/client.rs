//! Registry client for fetching and parsing pack indices
//!
//! This module provides functionality for:
//! - Fetching approved HTTP(S) index files
//! - Caching indices with TTL-based expiration
//! - Searching packs across multiple registries
//! - Handling authenticated registries

use super::{OutboundUrlPolicy, PackIndex, PackIndexEntry};
use crate::config::{PackRegistryConfig, RegistryIndexConfig};
use crate::error::{Error, Result};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

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
                    tracing::warn!("Failed to fetch registry {}: {}", registry.url, e);
                    continue;
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
    use std::time::Duration;

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
    async fn bounded_reader_rejects_oversized_chunked_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nabcdef\r\n0\r\n\r\n").await.unwrap();
        });
        let response = reqwest::Client::new()
            .get(format!("http://{}", address))
            .send()
            .await
            .unwrap();
        assert!(read_bounded_response(response, 5).await.is_err());
    }
}
