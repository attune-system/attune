//! Pack installer module for downloading and extracting packs from various sources
//!
//! This module provides functionality for:
//! - Cloning git repositories
//! - Downloading and extracting archives (zip, tar.gz)
//! - Copying local directories
//! - Verifying checksums
//! - Resolving registry references to install sources
//! - Progress reporting during installation

use super::{
    calculate_directory_checksum, extract_archive as extract_archive_safely, Checksum,
    InstallSource, OutboundUrlPolicy, PackIndexEntry, RegistryClient, SafeExtractionLimits,
    ValidatedUrl,
};
use crate::config::PackRegistryConfig;
use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use std::{ffi::OsString, net::IpAddr};
use tokio::fs;
use tokio::process::Command;
use url::Url;

/// Progress callback type
pub type ProgressCallback = Arc<dyn Fn(ProgressEvent) + Send + Sync>;

/// Progress event during pack installation
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// Started a new step
    StepStarted { step: String, message: String },
    /// Step completed
    StepCompleted { step: String, message: String },
    /// Download progress
    Downloading {
        url: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    /// Extraction progress
    Extracting { file: String },
    /// Verification progress
    Verifying { message: String },
    /// Warning message
    Warning { message: String },
    /// Info message
    Info { message: String },
}

/// Pack installer for handling various installation sources
pub struct PackInstaller {
    /// Temporary directory for downloads
    temp_dir: PathBuf,

    /// Registry client for resolving pack references
    registry_client: Option<RegistryClient>,

    /// Whether to verify checksums
    verify_checksums: bool,

    outbound_policy: OutboundUrlPolicy,

    archive_max_bytes: u64,

    install_timeout: Duration,

    /// Progress callback (optional)
    progress_callback: Option<ProgressCallback>,
}

/// Information about an installed pack
#[derive(Debug, Clone)]
pub struct InstalledPack {
    /// Path to the pack directory
    pub path: PathBuf,

    /// Installation source
    pub source: PackSource,

    /// Checksum (if available and verified)
    pub checksum: Option<String>,

    /// Whether the supplied checksum was compared with the installed content.
    pub checksum_verified: bool,
}

/// Pack installation source type
#[derive(Debug, Clone)]
pub enum PackSource {
    /// Git repository
    Git {
        url: String,
        git_ref: Option<String>,
    },

    /// Archive URL (zip, tar.gz, tgz)
    Archive { url: String },

    /// Local directory
    LocalDirectory { path: PathBuf },

    /// Local archive file
    LocalArchive { path: PathBuf },

    /// Registry reference
    Registry {
        pack_ref: String,
        version: Option<String>,
    },
}

impl PackInstaller {
    /// Create a new pack installer
    pub async fn new(
        temp_base_dir: impl AsRef<Path>,
        registry_config: Option<PackRegistryConfig>,
    ) -> Result<Self> {
        let temp_dir = temp_base_dir.as_ref().join("pack-installs");
        fs::create_dir_all(&temp_dir)
            .await
            .map_err(|e| Error::internal(format!("Failed to create temp directory: {}", e)))?;

        let (
            registry_client,
            verify_checksums,
            outbound_policy,
            archive_max_bytes,
            install_timeout,
        ) = if let Some(config) = registry_config {
            let verify_checksums = config.verify_checksums;
            let outbound_policy = OutboundUrlPolicy::from_config(&config)?;
            let archive_max_bytes = config.archive_max_bytes;
            let install_timeout = Duration::from_secs(config.timeout);
            (
                Some(RegistryClient::new(config)?),
                verify_checksums,
                outbound_policy,
                archive_max_bytes,
                install_timeout,
            )
        } else {
            let config = PackRegistryConfig::default();
            (
                None,
                false,
                OutboundUrlPolicy::from_config(&config)?,
                config.archive_max_bytes,
                Duration::from_secs(config.timeout),
            )
        };

        Ok(Self {
            temp_dir,
            registry_client,
            verify_checksums,
            outbound_policy,
            archive_max_bytes,
            install_timeout,
            progress_callback: None,
        })
    }

    /// Set progress callback
    pub fn with_progress_callback(mut self, callback: ProgressCallback) -> Self {
        self.progress_callback = Some(callback);
        self
    }

    /// Report progress event
    fn report_progress(&self, event: ProgressEvent) {
        if let Some(ref callback) = self.progress_callback {
            callback(event);
        }
    }

    /// Install a pack from the given source
    pub async fn install(&self, source: PackSource) -> Result<InstalledPack> {
        match source {
            PackSource::Git { url, git_ref } => {
                self.install_from_git(&url, git_ref.as_deref()).await
            }
            PackSource::Archive { url } => self.install_from_archive_url(&url, None).await,
            PackSource::LocalDirectory { path } => self.install_from_local_directory(&path).await,
            PackSource::LocalArchive { path } => self.install_from_local_archive(&path).await,
            PackSource::Registry { pack_ref, version } => {
                self.install_from_registry(&pack_ref, version.as_deref())
                    .await
            }
        }
    }

    /// Install from git repository
    async fn install_from_git(&self, url: &str, git_ref: Option<&str>) -> Result<InstalledPack> {
        if git_ref.is_some_and(|git_ref| git_ref.starts_with('-')) {
            return Err(Error::validation("Git refs must not start with '-'"));
        }
        let validated = self.validate_git_source(url).await?;
        tracing::info!("Installing pack from git: {} (ref: {:?})", url, git_ref);

        self.report_progress(ProgressEvent::StepStarted {
            step: "clone".to_string(),
            message: format!("Cloning git repository: {}", url),
        });

        // Create unique temp directory for this installation
        let install_dir = self.create_temp_dir().await?;

        // Clone the repository
        let mut clone_cmd = Command::new("git");
        clone_cmd
            .args(git_clone_arguments(&validated, &install_dir, git_ref)?)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_COUNT", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_SSL_NO_VERIFY")
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("ALL_PROXY")
            .env_remove("http_proxy")
            .env_remove("https_proxy")
            .env_remove("all_proxy");

        let output = tokio::time::timeout(self.install_timeout, clone_cmd.output())
            .await
            .map_err(|_| Error::internal("Git clone timed out"))?
            .map_err(|e| Error::internal(format!("Failed to execute git clone: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::internal(format!("Git clone failed: {}", stderr)));
        }

        // Checkout specific ref if provided
        if let Some(ref_spec) = git_ref {
            let mut checkout_cmd = Command::new("git");
            checkout_cmd
                .arg("-C")
                .arg(&install_dir)
                .arg("checkout")
                .arg(ref_spec)
                .arg("--")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_COUNT", "0");
            let checkout_output = tokio::time::timeout(self.install_timeout, checkout_cmd.output())
                .await
                .map_err(|_| Error::internal("Git checkout timed out"))?
                .map_err(|e| Error::internal(format!("Failed to execute git checkout: {}", e)))?;

            if !checkout_output.status.success() {
                let stderr = String::from_utf8_lossy(&checkout_output.stderr);
                return Err(Error::internal(format!("Git checkout failed: {}", stderr)));
            }
        }

        // Repository metadata is not installed pack content and makes content hashes unstable.
        fs::remove_dir_all(install_dir.join(".git"))
            .await
            .map_err(|e| Error::internal(format!("Failed to remove Git metadata: {}", e)))?;

        // Find pack.yaml (could be at root or in pack/ subdirectory)
        let pack_dir = self.find_pack_directory(&install_dir).await?;

        Ok(InstalledPack {
            path: pack_dir,
            source: PackSource::Git {
                url: validated.url.to_string(),
                git_ref: git_ref.map(String::from),
            },
            checksum: None,
            checksum_verified: false,
        })
    }

    /// Install from archive URL
    async fn install_from_archive_url(
        &self,
        url: &str,
        expected_checksum: Option<&str>,
    ) -> Result<InstalledPack> {
        tracing::info!("Installing pack from archive: {}", url);

        // Download the archive
        let archive_path = self.download_archive(url).await?;

        // Verify checksum if provided
        let mut checksum_verified = false;
        if let Some(checksum_str) = expected_checksum {
            if self.verify_checksums {
                self.verify_archive_checksum(&archive_path, checksum_str)
                    .await?;
                checksum_verified = true;
            }
        }

        // Extract the archive
        let extract_dir = self.extract_archive(&archive_path).await?;

        // Find pack.yaml
        let pack_dir = self.find_pack_directory(&extract_dir).await?;

        // Clean up archive file
        let _ = fs::remove_file(&archive_path).await;

        Ok(InstalledPack {
            path: pack_dir,
            source: PackSource::Archive {
                url: url.to_string(),
            },
            checksum: checksum_verified.then(|| expected_checksum.unwrap().to_string()),
            checksum_verified,
        })
    }

    /// Install from local directory
    async fn install_from_local_directory(&self, source_path: &Path) -> Result<InstalledPack> {
        tracing::info!("Installing pack from local directory: {:?}", source_path);

        // Verify source exists and is a directory
        if !source_path.exists() {
            return Err(Error::not_found(
                "directory",
                "path",
                source_path.display().to_string(),
            ));
        }

        if !source_path.is_dir() {
            return Err(Error::validation(format!(
                "Path is not a directory: {}",
                source_path.display()
            )));
        }

        // Create temp directory
        let install_dir = self.create_temp_dir().await?;

        // Copy directory contents
        self.copy_directory(source_path, &install_dir).await?;

        // Find pack.yaml
        let pack_dir = self.find_pack_directory(&install_dir).await?;

        Ok(InstalledPack {
            path: pack_dir,
            source: PackSource::LocalDirectory {
                path: source_path.to_path_buf(),
            },
            checksum: None,
            checksum_verified: false,
        })
    }

    /// Install from local archive file
    async fn install_from_local_archive(&self, archive_path: &Path) -> Result<InstalledPack> {
        tracing::info!("Installing pack from local archive: {:?}", archive_path);

        // Verify file exists
        if !archive_path.exists() {
            return Err(Error::not_found(
                "file",
                "path",
                archive_path.display().to_string(),
            ));
        }

        let metadata = std::fs::symlink_metadata(archive_path)
            .map_err(|e| Error::io(format!("Failed to inspect archive: {e}")))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(Error::validation(format!(
                "Path is not a file: {}",
                archive_path.display()
            )));
        }

        // Extract the archive
        let extract_dir = self.extract_archive(archive_path).await?;

        // Find pack.yaml
        let pack_dir = self.find_pack_directory(&extract_dir).await?;

        Ok(InstalledPack {
            path: pack_dir,
            source: PackSource::LocalArchive {
                path: archive_path.to_path_buf(),
            },
            checksum: None,
            checksum_verified: false,
        })
    }

    /// Install from registry reference
    async fn install_from_registry(
        &self,
        pack_ref: &str,
        version: Option<&str>,
    ) -> Result<InstalledPack> {
        tracing::info!(
            "Installing pack from registry: {} (version: {:?})",
            pack_ref,
            version
        );

        let registry_client = self
            .registry_client
            .as_ref()
            .ok_or_else(|| Error::configuration("Registry client not configured"))?;

        // Search for the pack
        let (pack_entry, _registry_url) = registry_client
            .search_pack(pack_ref)
            .await?
            .ok_or_else(|| Error::not_found("pack", "ref", pack_ref))?;

        // Validate version if specified
        if let Some(requested_version) = version {
            if requested_version != "latest" && pack_entry.version != requested_version {
                return Err(Error::validation(format!(
                    "Pack {} version {} not found (available: {})",
                    pack_ref, requested_version, pack_entry.version
                )));
            }
        }

        // Get the preferred install source (try git first, then archive)
        let install_source = self.select_install_source(&pack_entry)?;

        // Install from the selected source
        match install_source {
            InstallSource::Git {
                url,
                git_ref,
                checksum,
            } => {
                let mut installed = self.install_from_git(&url, git_ref.as_deref()).await?;
                if self.verify_checksums {
                    let calculated = verify_git_content_checksum(&installed.path, &checksum)?;
                    installed.checksum = Some(calculated);
                    installed.checksum_verified = true;
                }
                Ok(installed)
            }
            InstallSource::Archive { url, checksum } => {
                self.install_from_archive_url(&url, Some(&checksum)).await
            }
        }
    }

    /// Select the best install source from a pack entry
    fn select_install_source(&self, pack_entry: &PackIndexEntry) -> Result<InstallSource> {
        if pack_entry.install_sources.is_empty() {
            return Err(Error::validation(format!(
                "Pack {} has no install sources",
                pack_entry.pack_ref
            )));
        }

        // Prefer git sources for development
        for source in &pack_entry.install_sources {
            if matches!(source, InstallSource::Git { .. }) {
                return Ok(source.clone());
            }
        }

        // Fall back to first archive source
        for source in &pack_entry.install_sources {
            if matches!(source, InstallSource::Archive { .. }) {
                return Ok(source.clone());
            }
        }

        // Return first source if no preference matched
        Ok(pack_entry.install_sources[0].clone())
    }

    /// Download an archive from a URL
    async fn download_archive(&self, url: &str) -> Result<PathBuf> {
        let validated = self.outbound_policy.validate(url).await?;
        let parsed_url = validated.url.clone();
        // The policy structurally validates the URL, rejects special addresses, disables
        // redirects/proxies, and pins the checked DNS answers into this reqwest client.
        let response = validated
            .client
            .get(validated.url)
            .send()
            .await
            .map_err(|e| Error::internal(format!("Failed to download archive: {}", e)))?;

        if !response.status().is_success() {
            return Err(Error::internal(format!(
                "Failed to download archive: HTTP {}",
                response.status()
            )));
        }

        // Determine filename from URL
        let filename = archive_filename_from_url(&parsed_url);

        let archive_path = self
            .temp_dir
            .join(format!("{}-{filename}", uuid::Uuid::new_v4()));

        if response
            .content_length()
            .is_some_and(|length| length > self.archive_max_bytes)
        {
            return Err(Error::validation(
                "Pack archive exceeds configured size limit",
            ));
        }

        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&archive_path)
            .await
            .map_err(|e| Error::internal(format!("Failed to create archive: {}", e)))?;
        let mut downloaded = 0_u64;
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk
                .map_err(|e| Error::internal(format!("Failed to read archive bytes: {}", e)))?;
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            if downloaded > self.archive_max_bytes {
                let _ = fs::remove_file(&archive_path).await;
                return Err(Error::validation(
                    "Pack archive exceeds configured size limit",
                ));
            }
            output
                .write_all(&chunk)
                .await
                .map_err(|e| Error::internal(format!("Failed to write archive: {}", e)))?;
        }
        output
            .flush()
            .await
            .map_err(|e| Error::internal(format!("Failed to flush archive: {}", e)))?;

        Ok(archive_path)
    }

    async fn validate_git_source(&self, raw_url: &str) -> Result<ValidatedUrl> {
        let validated = self.outbound_policy.validate(raw_url).await?;
        if validated.url.scheme() != "https" {
            return Err(Error::validation("Git pack URLs must use HTTPS"));
        }
        Ok(validated)
    }

    /// Extract an archive (zip or tar.gz)
    async fn extract_archive(&self, archive_path: &Path) -> Result<PathBuf> {
        let extract_dir = self.create_temp_dir().await?;

        let archive_path = archive_path.to_path_buf();
        let destination = extract_dir.clone();
        let limits = SafeExtractionLimits {
            max_total_bytes: self.archive_max_bytes,
            ..Default::default()
        };
        tokio::task::spawn_blocking(move || {
            extract_archive_safely(&archive_path, &destination, limits)
        })
        .await
        .map_err(|error| Error::internal(format!("Archive extraction task failed: {error}")))??;

        Ok(extract_dir)
    }

    /// Verify archive checksum
    async fn verify_archive_checksum(&self, archive_path: &Path, checksum_str: &str) -> Result<()> {
        let checksum = Checksum::parse(checksum_str)
            .map_err(|e| Error::validation(format!("Invalid checksum: {}", e)))?;

        let computed = self
            .compute_checksum(archive_path, &checksum.algorithm)
            .await?;

        if computed != checksum.hash {
            return Err(Error::validation(format!(
                "Checksum mismatch: expected {}, got {}",
                checksum.hash, computed
            )));
        }

        tracing::info!("Checksum verified: {}", checksum_str);
        Ok(())
    }

    /// Compute checksum of a file
    async fn compute_checksum(&self, path: &Path, algorithm: &str) -> Result<String> {
        let command = match algorithm {
            "sha256" => "sha256sum",
            "sha512" => "sha512sum",
            "sha1" => "sha1sum",
            "md5" => "md5sum",
            _ => {
                return Err(Error::validation(format!(
                    "Unsupported hash algorithm: {}",
                    algorithm
                )));
            }
        };

        let output = Command::new(command)
            .arg(path)
            .output()
            .await
            .map_err(|e| Error::internal(format!("Failed to compute checksum: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::internal(format!(
                "Checksum computation failed: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let hash = stdout
            .split_whitespace()
            .next()
            .ok_or_else(|| Error::internal("Failed to parse checksum output"))?;

        Ok(hash.to_lowercase())
    }

    /// Find pack directory (pack.yaml location)
    async fn find_pack_directory(&self, base_dir: &Path) -> Result<PathBuf> {
        // Check if pack.yaml exists at root
        let root_pack_yaml = base_dir.join("pack.yaml");
        if root_pack_yaml.exists() {
            return Ok(base_dir.to_path_buf());
        }

        // Check in pack/ subdirectory
        let pack_subdir = base_dir.join("pack");
        let pack_subdir_yaml = pack_subdir.join("pack.yaml");
        if pack_subdir_yaml.exists() {
            return Ok(pack_subdir);
        }

        // Check in first subdirectory (common for GitHub archives)
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Archive inspection is limited to the temporary extraction directory created by this installer.
        let mut entries = fs::read_dir(base_dir)
            .await
            .map_err(|e| Error::internal(format!("Failed to read directory: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| Error::internal(format!("Failed to read directory entry: {}", e)))?
        {
            let path = entry.path();
            if path.is_dir() {
                let subdir_pack_yaml = path.join("pack.yaml");
                if subdir_pack_yaml.exists() {
                    return Ok(path);
                }
            }
        }

        Err(Error::validation(format!(
            "pack.yaml not found in {}",
            base_dir.display()
        )))
    }

    /// Copy directory recursively
    #[async_recursion::async_recursion]
    async fn copy_directory(&self, src: &Path, dst: &Path) -> Result<()> {
        use tokio::fs;

        // Create destination directory if it doesn't exist
        fs::create_dir_all(dst).await.map_err(|e| {
            Error::internal(format!("Failed to create destination directory: {}", e))
        })?;

        // Read source directory
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Directory copy operates on installer-managed local paths, not request-derived paths.
        let mut entries = fs::read_dir(src)
            .await
            .map_err(|e| Error::internal(format!("Failed to read source directory: {}", e)))?;

        // Copy each entry
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| Error::internal(format!("Failed to read directory entry: {}", e)))?
        {
            let path = entry.path();
            let file_name = entry.file_name();
            let dest_path = dst.join(&file_name);

            let metadata = fs::symlink_metadata(&path)
                .await
                .map_err(|e| Error::internal(format!("Failed to read entry metadata: {}", e)))?;

            if metadata.file_type().is_symlink() {
                return Err(Error::validation(format!(
                    "Pack directory contains symlink: {}",
                    path.display()
                )));
            } else if metadata.is_dir() {
                // Recursively copy subdirectory
                self.copy_directory(&path, &dest_path).await?;
            } else if metadata.is_file() {
                // Copy file
                fs::copy(&path, &dest_path)
                    .await
                    .map_err(|e| Error::internal(format!("Failed to copy file: {}", e)))?;
            } else {
                return Err(Error::validation(format!(
                    "Pack directory contains special file: {}",
                    path.display()
                )));
            }
        }

        Ok(())
    }

    /// Create a unique temporary directory
    async fn create_temp_dir(&self) -> Result<PathBuf> {
        let uuid = uuid::Uuid::new_v4();
        let dir = self.temp_dir.join(uuid.to_string());

        fs::create_dir_all(&dir)
            .await
            .map_err(|e| Error::internal(format!("Failed to create temp directory: {}", e)))?;

        Ok(dir)
    }

    /// Clean up temporary directory
    pub async fn cleanup(&self, pack_path: &Path) -> Result<()> {
        if pack_path.starts_with(&self.temp_dir) {
            fs::remove_dir_all(pack_path)
                .await
                .map_err(|e| Error::internal(format!("Failed to cleanup temp directory: {}", e)))?;
        }
        Ok(())
    }
}

fn git_clone_arguments(
    validated: &ValidatedUrl,
    install_dir: &Path,
    git_ref: Option<&str>,
) -> Result<Vec<OsString>> {
    let host = validated
        .url
        .host_str()
        .ok_or_else(|| Error::validation("Git URL is missing a host"))?;
    let port = validated
        .url
        .port_or_known_default()
        .ok_or_else(|| Error::validation("Git URL has no usable port"))?;
    let address = validated
        .addresses
        .first()
        .ok_or_else(|| Error::validation("Git URL resolved to no validated addresses"))?
        .ip();
    let address = match address {
        IpAddr::V4(address) => address.to_string(),
        IpAddr::V6(address) => format!("[{}]", address),
    };
    let resolve = format!("{}:{}:{}", host, port, address);

    let mut args = vec![
        "-c".into(),
        "http.followRedirects=false".into(),
        "-c".into(),
        "http.proxy=".into(),
        "-c".into(),
        "http.sslVerify=true".into(),
        "-c".into(),
        format!("http.curloptResolve={}", resolve).into(),
        "-c".into(),
        "protocol.allow=never".into(),
        "-c".into(),
        "protocol.https.allow=always".into(),
        "clone".into(),
    ];
    if git_ref.is_none() {
        args.extend([OsString::from("--depth"), OsString::from("1")]);
    }
    args.extend([
        OsString::from(validated.url.as_str()),
        install_dir.as_os_str().to_owned(),
    ]);
    Ok(args)
}

fn verify_git_content_checksum(path: &Path, expected: &str) -> Result<String> {
    let expected = Checksum::parse(expected)
        .map_err(|e| Error::validation(format!("Invalid Git content checksum: {}", e)))?;
    if expected.algorithm != "sha256" {
        return Err(Error::validation("Git content checksums must use sha256"));
    }
    let calculated = calculate_directory_checksum(path)?;
    if !calculated.eq_ignore_ascii_case(&expected.hash) {
        return Err(Error::validation(format!(
            "Git content checksum mismatch: expected {}, got {}",
            expected.hash, calculated
        )));
    }
    Ok(calculated)
}

fn archive_filename_from_url(url: &Url) -> String {
    let raw_name = url
        .path_segments()
        .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
        .unwrap_or("archive.bin");

    let sanitized: String = raw_name
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => ch,
            _ => '_',
        })
        .collect();

    let filename = sanitized.trim_matches('.');
    if filename.is_empty() {
        "archive.bin".to_string()
    } else {
        filename.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_checksum_parsing() {
        let checksum = Checksum::parse("sha256:abc123def456").unwrap();
        assert_eq!(checksum.algorithm, "sha256");
        assert_eq!(checksum.hash, "abc123def456");
    }

    #[tokio::test]
    async fn test_select_install_source_prefers_git() {
        let entry = PackIndexEntry {
            pack_ref: "test".to_string(),
            label: "Test".to_string(),
            description: "Test pack".to_string(),
            use_case: None,
            version: "1.0.0".to_string(),
            author: "Test".to_string(),
            email: None,
            homepage: None,
            repository: None,
            license: "MIT".to_string(),
            keywords: vec![],
            runtime_deps: vec![],
            install_sources: vec![
                InstallSource::Archive {
                    url: "https://example.com/archive.zip".to_string(),
                    checksum: "sha256:abc123".to_string(),
                },
                InstallSource::Git {
                    url: "https://github.com/example/pack".to_string(),
                    git_ref: Some("v1.0.0".to_string()),
                    checksum: "sha256:def456".to_string(),
                },
            ],
            contents: Default::default(),
            dependencies: None,
            meta: None,
        };

        let temp_dir = std::env::temp_dir().join("attune-test");
        let installer = PackInstaller::new(&temp_dir, None).await.unwrap();
        let source = installer.select_install_source(&entry).unwrap();

        assert!(matches!(source, InstallSource::Git { .. }));
    }

    #[test]
    fn test_archive_filename_from_url_sanitizes_path_segments() {
        let url = Url::parse("https://example.com/releases/../../pack.zip?token=x").unwrap();
        assert_eq!(archive_filename_from_url(&url), "pack.zip");
    }

    #[tokio::test]
    async fn test_validate_git_source_allows_explicit_private_https_host() {
        let temp_dir = std::env::temp_dir().join("attune-test");
        let config = PackRegistryConfig {
            approved_private_hosts: vec!["localhost".to_string()],
            ..Default::default()
        };
        let installer = PackInstaller::new(&temp_dir, Some(config)).await.unwrap();

        installer
            .validate_git_source("https://localhost:3000/example/repo.git")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_validate_git_source_rejects_non_https_transports() {
        let installer = PackInstaller::new(std::env::temp_dir().join("attune-test"), None)
            .await
            .unwrap();
        for source in [
            "git://example.com/repo.git",
            "git@example.com:repo.git",
            "file:///tmp/repo",
        ] {
            assert!(
                installer.validate_git_source(source).await.is_err(),
                "{}",
                source
            );
        }
    }

    #[tokio::test]
    async fn git_install_rejects_option_like_refs_before_network_access() {
        let installer = PackInstaller::new(std::env::temp_dir().join("attune-test"), None)
            .await
            .unwrap();
        assert!(installer
            .install_from_git("https://example.com/repo.git", Some("--config"))
            .await
            .unwrap_err()
            .to_string()
            .contains("must not start"));
    }

    #[test]
    fn git_clone_pins_one_address_and_hardens_libcurl() {
        let validated = ValidatedUrl {
            url: Url::parse("https://github.com/example/pack.git").unwrap(),
            client: reqwest::Client::new(),
            addresses: vec![
                "192.0.2.10:443".parse().unwrap(),
                "192.0.2.11:443".parse().unwrap(),
            ],
        };
        let args = git_clone_arguments(&validated, Path::new("/tmp/pack"), None).unwrap();
        let args: Vec<_> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(args.contains(&"http.curloptResolve=github.com:443:192.0.2.10".to_string()));
        assert!(!args.iter().any(|arg| arg.contains("192.0.2.11")));
        assert!(args.contains(&"http.followRedirects=false".to_string()));
        assert!(args.contains(&"http.proxy=".to_string()));
        assert!(args.contains(&"http.sslVerify=true".to_string()));
        assert!(args.contains(&"protocol.allow=never".to_string()));
        assert!(args.contains(&"protocol.https.allow=always".to_string()));
        assert_eq!(args.last().unwrap(), "/tmp/pack");
    }

    #[test]
    fn git_content_checksum_must_match_calculated_content() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("pack.yaml"), "ref: test\n").unwrap();
        let calculated = calculate_directory_checksum(directory.path()).unwrap();

        assert_eq!(
            verify_git_content_checksum(directory.path(), &format!("sha256:{}", calculated))
                .unwrap(),
            calculated
        );
        assert!(verify_git_content_checksum(directory.path(), "sha256:deadbeef").is_err());
    }
}
