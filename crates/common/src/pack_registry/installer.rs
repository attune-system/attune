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
    calculate_directory_checksum, calculate_file_checksum,
    extract_archive as extract_archive_safely, Checksum, InstallSource, OutboundUrlPolicy,
    PackIndexEntry, RegistryClient, SafeExtractionLimits, ValidatedUrl,
};
use crate::config::PackRegistryConfig;
use crate::error::{Error, Result};
use serde::Serialize;
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

    /// Whether direct remote sources without registry integrity metadata are allowed.
    allow_unverified_direct_remote_installs: bool,

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

    /// Declared or calculated checksum, when available.
    pub checksum: Option<String>,

    /// The bytes or content represented by `checksum`.
    pub checksum_subject: Option<ChecksumSubject>,

    /// Whether the supplied checksum was verified against its subject. Git
    /// checksums cover installed directory content; archive checksums cover the
    /// downloaded archive bytes.
    pub checksum_verified: bool,

    /// Registry identity selected before downloading the pack, when applicable.
    pub registry_identity: Option<RegistryPackIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChecksumSubject {
    ArchiveBytes,
    DirectoryContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryPackIdentity {
    pub pack_ref: String,
    pub version: String,
    pub registry_url: String,
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
            allow_unverified_direct_remote_installs,
            outbound_policy,
            archive_max_bytes,
            install_timeout,
        ) = if let Some(config) = registry_config {
            let verify_checksums = config.verify_checksums;
            let allow_unverified_direct_remote_installs =
                config.allow_unverified_direct_remote_installs;
            let outbound_policy = OutboundUrlPolicy::from_config(&config)?;
            let archive_max_bytes = config.archive_max_bytes;
            let install_timeout = Duration::from_secs(config.timeout);
            (
                Some(RegistryClient::new(config)?),
                verify_checksums,
                allow_unverified_direct_remote_installs,
                outbound_policy,
                archive_max_bytes,
                install_timeout,
            )
        } else {
            let mut config = PackRegistryConfig::default();
            config.approved_public_hosts.clear();
            config.approved_private_hosts.clear();
            config.approved_private_cidrs.clear();
            config.allow_http = false;
            (
                None,
                false,
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
            allow_unverified_direct_remote_installs,
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
        if matches!(&source, PackSource::Git { .. } | PackSource::Archive { .. })
            && !self.allow_unverified_direct_remote_installs
        {
            return Err(Error::validation(
                "Direct remote Git and archive pack installs are unverified and disabled; use a registry reference or set pack_registry.allow_unverified_direct_remote_installs to true",
            ));
        }

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
        let result = async {
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
                .env_remove("all_proxy")
                .kill_on_drop(true);

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
                    .env("GIT_CONFIG_COUNT", "0")
                    .kill_on_drop(true);
                let checkout_output =
                    tokio::time::timeout(self.install_timeout, checkout_cmd.output())
                        .await
                        .map_err(|_| Error::internal("Git checkout timed out"))?
                        .map_err(|e| {
                            Error::internal(format!("Failed to execute git checkout: {}", e))
                        })?;

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
                checksum_subject: None,
                checksum_verified: false,
                registry_identity: None,
            })
        }
        .await;

        if result.is_err() {
            self.cleanup_temp_install_after_error(&install_dir).await;
        }
        result
    }

    /// Install from archive URL
    async fn install_from_archive_url(
        &self,
        url: &str,
        expected_checksum: Option<&str>,
    ) -> Result<InstalledPack> {
        let url = crate::pack_registry::validate_remote_pack_url(url)?;
        tracing::info!("Installing pack from archive: {}", url);

        // Download the archive
        let archive_path = self.download_archive(url.as_str()).await?;
        let result = async {
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
            let pack_dir = match self.find_pack_directory(&extract_dir).await {
                Ok(pack_dir) => pack_dir,
                Err(error) => {
                    self.cleanup_temp_install_after_error(&extract_dir).await;
                    return Err(error);
                }
            };

            Ok(InstalledPack {
                path: pack_dir,
                source: PackSource::Archive { url: url.into() },
                checksum: expected_checksum.map(str::to_owned),
                checksum_subject: expected_checksum.map(|_| ChecksumSubject::ArchiveBytes),
                checksum_verified,
                registry_identity: None,
            })
        }
        .await;

        if let Err(error) = fs::remove_file(&archive_path).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    "Failed to remove temporary archive {}: {}",
                    archive_path.display(),
                    error
                );
            }
        }
        result
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
        let result = async {
            self.copy_directory(source_path, &install_dir).await?;
            let pack_dir = self.find_pack_directory(&install_dir).await?;

            Ok(InstalledPack {
                path: pack_dir,
                source: PackSource::LocalDirectory {
                    path: source_path.to_path_buf(),
                },
                checksum: None,
                checksum_subject: None,
                checksum_verified: false,
                registry_identity: None,
            })
        }
        .await;

        if result.is_err() {
            self.cleanup_temp_install_after_error(&install_dir).await;
        }
        result
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
        let pack_dir = match self.find_pack_directory(&extract_dir).await {
            Ok(pack_dir) => pack_dir,
            Err(error) => {
                self.cleanup_temp_install_after_error(&extract_dir).await;
                return Err(error);
            }
        };

        Ok(InstalledPack {
            path: pack_dir,
            source: PackSource::LocalArchive {
                path: archive_path.to_path_buf(),
            },
            checksum: None,
            checksum_subject: None,
            checksum_verified: false,
            registry_identity: None,
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
        let (pack_entry, registry_url) = registry_client
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
        self.install_resolved_registry_pack(pack_entry, registry_url)
            .await
    }

    /// Install a registry entry that was already fetched and validated by the caller.
    pub async fn install_resolved_registry_pack(
        &self,
        pack_entry: PackIndexEntry,
        registry_url: String,
    ) -> Result<InstalledPack> {
        let install_source = self.select_install_source(&pack_entry)?;

        let fallback_archive = pack_entry
            .install_sources
            .iter()
            .find(|source| matches!(source, InstallSource::Archive { .. }))
            .cloned();
        let mut installed = match self.install_registry_source(install_source.clone()).await {
            Ok(installed) => installed,
            Err(git_error) if matches!(install_source, InstallSource::Git { .. }) => {
                let Some(archive) = fallback_archive else {
                    return Err(git_error);
                };
                tracing::warn!("Git pack source failed; trying verified archive fallback");
                match self.install_registry_source(archive).await {
                    Ok(installed) => installed,
                    Err(archive_error) => {
                        return Err(Error::internal(format!(
                        "Git source failed: {git_error}; archive fallback failed: {archive_error}"
                    )))
                    }
                }
            }
            Err(error) => return Err(error),
        };
        installed.registry_identity = Some(RegistryPackIdentity {
            pack_ref: pack_entry.pack_ref,
            version: pack_entry.version,
            registry_url,
        });
        Ok(installed)
    }

    async fn install_registry_source(
        &self,
        install_source: InstallSource,
    ) -> Result<InstalledPack> {
        match install_source {
            InstallSource::Git {
                url,
                git_ref,
                checksum,
            } => {
                let installed = self.install_from_git(&url, git_ref.as_deref()).await?;
                self.verify_registry_git_install(installed, &checksum).await
            }
            InstallSource::Archive { url, checksum } => {
                self.install_from_archive_url(&url, Some(&checksum)).await
            }
        }
    }

    async fn verify_registry_git_install(
        &self,
        mut installed: InstalledPack,
        checksum: &str,
    ) -> Result<InstalledPack> {
        installed.checksum = Some(checksum.to_string());
        installed.checksum_subject = Some(ChecksumSubject::DirectoryContent);
        if !self.verify_checksums {
            return Ok(installed);
        }

        match verify_git_content_checksum(&installed.path, checksum) {
            Ok(calculated) => {
                installed.checksum = Some(calculated);
                installed.checksum_verified = true;
                Ok(installed)
            }
            Err(error) => {
                self.cleanup_temp_install_after_error(&installed.path).await;
                Err(error)
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
        let write_result = async {
            let mut downloaded = 0_u64;
            let mut body = response.bytes_stream();
            while let Some(chunk) = body.next().await {
                let chunk = chunk
                    .map_err(|e| Error::internal(format!("Failed to read archive bytes: {}", e)))?;
                downloaded = downloaded.saturating_add(chunk.len() as u64);
                if downloaded > self.archive_max_bytes {
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
            Ok(())
        }
        .await;
        drop(output);
        if let Err(error) = write_result {
            let _ = fs::remove_file(&archive_path).await;
            return Err(error);
        }

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
        let result = match tokio::task::spawn_blocking(move || {
            extract_archive_safely(&archive_path, &destination, limits)
        })
        .await
        {
            Ok(result) => result,
            Err(error) => {
                self.cleanup_temp_install_after_error(&extract_dir).await;
                return Err(Error::internal(format!(
                    "Archive extraction task failed: {error}"
                )));
            }
        };
        if let Err(error) = result {
            self.cleanup_temp_install_after_error(&extract_dir).await;
            return Err(error);
        }

        Ok(extract_dir)
    }

    /// Verify archive checksum
    async fn verify_archive_checksum(&self, archive_path: &Path, checksum_str: &str) -> Result<()> {
        let checksum = Checksum::parse_registry_sha256(checksum_str)
            .map_err(|e| Error::validation(format!("Invalid checksum: {}", e)))?;

        let path = archive_path.to_path_buf();
        let computed = tokio::task::spawn_blocking(move || calculate_file_checksum(path))
            .await
            .map_err(|error| Error::internal(format!("Archive checksum task failed: {error}")))??;

        if computed != checksum.hash {
            return Err(Error::validation(format!(
                "Checksum mismatch: expected {}, got {}",
                checksum.hash, computed
            )));
        }

        tracing::info!("Checksum verified: {}", checksum_str);
        Ok(())
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

    fn temp_install_root(&self, pack_path: &Path) -> Option<PathBuf> {
        let relative = pack_path.strip_prefix(&self.temp_dir).ok()?;
        let install_name = relative.components().next()?.as_os_str();
        Some(self.temp_dir.join(install_name))
    }

    async fn cleanup_temp_install_after_error(&self, pack_path: &Path) {
        let Some(install_root) = self.temp_install_root(pack_path) else {
            return;
        };
        if let Err(error) = fs::remove_dir_all(&install_root).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    "Failed to clean up temporary Git install {}: {}",
                    install_root.display(),
                    error
                );
            }
        }
    }

    /// Clean up temporary directory
    pub async fn cleanup(&self, pack_path: &Path) -> Result<()> {
        if let Some(install_root) = self.temp_install_root(pack_path) {
            fs::remove_dir_all(&install_root)
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
    let expected = Checksum::parse_registry_sha256(expected)
        .map_err(|e| Error::validation(format!("Invalid Git content checksum: {}", e)))?;
    let calculated = calculate_directory_checksum(path)?;
    if calculated != expected.hash {
        return Err(Error::validation(format!(
            "Git content checksum mismatch: expected {}, got {}",
            expected.hash, calculated
        )));
    }
    Ok(format!("sha256:{calculated}"))
}

fn archive_filename_from_url(url: &Url) -> String {
    let segments: Vec<_> = url
        .path_segments()
        .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
        .unwrap_or_default();
    let raw_name = segments.last().copied().unwrap_or("archive.bin");

    let suffix = match segments.as_slice() {
        [.., "tar.gz", revision]
            if revision.len() == 40 && revision.chars().all(|ch| ch.is_ascii_hexdigit()) =>
        {
            ".tar.gz"
        }
        _ => "",
    };

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
        format!("{filename}{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pack_archive() -> Vec<u8> {
        use flate2::{write::GzEncoder, Compression};
        use std::io::Cursor;

        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let content = b"ref: registry-test\nname: Registry Test\nversion: 1.2.3\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, "registry-test/pack.yaml", Cursor::new(content))
            .unwrap();
        archive.into_inner().unwrap().finish().unwrap()
    }

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

    #[tokio::test]
    async fn registry_git_failure_uses_verified_archive_and_preserves_identity() {
        use tokio::io::AsyncWriteExt;

        let archive = test_pack_archive();
        let checksum_file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(checksum_file.path(), &archive).unwrap();
        let checksum = crate::pack_registry::calculate_file_checksum(checksum_file.path()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let archive_for_server = archive.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/gzip\r\nContent-Disposition: attachment; filename=not-an-archive.txt\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                archive_for_server.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&archive_for_server).await.unwrap();
        });

        let temp_dir = tempfile::tempdir().unwrap();
        let config = PackRegistryConfig {
            indices: Vec::new(),
            approved_public_hosts: Vec::new(),
            approved_private_hosts: vec!["127.0.0.1".to_string()],
            allow_http: true,
            ..Default::default()
        };
        let installer = PackInstaller::new(temp_dir.path(), Some(config))
            .await
            .unwrap();
        let pack_entry = PackIndexEntry {
            pack_ref: "registry-test".to_string(),
            label: "Registry Test".to_string(),
            description: "test".to_string(),
            use_case: None,
            version: "1.2.3".to_string(),
            author: "Test".to_string(),
            email: None,
            homepage: None,
            repository: None,
            license: "MIT".to_string(),
            keywords: Vec::new(),
            runtime_deps: Vec::new(),
            install_sources: vec![
                InstallSource::Git {
                    url: "https://127.0.0.1:1/unavailable.git".to_string(),
                    git_ref: Some("deadbeef".to_string()),
                    checksum: format!("sha256:{}", "0".repeat(64)),
                },
                InstallSource::Archive {
                    url: format!(
                        "http://{address}/attune-packs/registry-test/tar.gz/{}",
                        "a".repeat(40)
                    ),
                    checksum: format!("sha256:{checksum}"),
                },
            ],
            contents: Default::default(),
            dependencies: None,
            meta: None,
        };
        let installed = installer
            .install_resolved_registry_pack(
                pack_entry,
                "https://registry.example.com/index.json".to_string(),
            )
            .await
            .unwrap();
        server.await.unwrap();

        assert!(installed.checksum_verified);
        assert_eq!(
            installed.checksum.as_deref(),
            Some(format!("sha256:{checksum}").as_str())
        );
        assert_eq!(
            installed.registry_identity,
            Some(RegistryPackIdentity {
                pack_ref: "registry-test".to_string(),
                version: "1.2.3".to_string(),
                registry_url: "https://registry.example.com/index.json".to_string(),
            })
        );
        assert!(installed.path.join("pack.yaml").is_file());
        let temp_entries: Vec<_> = std::fs::read_dir(temp_dir.path().join("pack-installs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(temp_entries.len(), 1);
        assert!(installed.path.starts_with(&temp_entries[0]));
    }

    #[test]
    fn test_archive_filename_from_url_sanitizes_path_segments() {
        let url = Url::parse("https://example.com/releases/../../pack.zip?token=x").unwrap();
        assert_eq!(archive_filename_from_url(&url), "pack.zip");
    }

    #[test]
    fn archive_filename_from_extensionless_codeload_url_uses_tar_gz_suffix() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let url = Url::parse(&format!(
            "https://codeload.github.com/attune-packs/demo/tar.gz/{sha}"
        ))
        .unwrap();

        assert_eq!(archive_filename_from_url(&url), format!("{sha}.tar.gz"));
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
    async fn default_installer_denies_hosts_approved_by_registry_defaults() {
        assert!(PackRegistryConfig::default()
            .approved_public_hosts
            .iter()
            .any(|host| host == "github.com"));
        let temp_dir = tempfile::tempdir().unwrap();
        let installer = PackInstaller::new(temp_dir.path(), None).await.unwrap();

        let error = match installer
            .validate_git_source("https://github.com/attune-system/attune.git")
            .await
        {
            Ok(_) => panic!("default installer unexpectedly allowed github.com"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("not explicitly approved"));
    }

    #[tokio::test]
    async fn default_installer_preserves_local_directory_installs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source_dir = temp_dir.path().join("source");
        std::fs::create_dir(&source_dir).unwrap();
        std::fs::write(source_dir.join("pack.yaml"), "ref: local-test\n").unwrap();
        let installer = PackInstaller::new(temp_dir.path(), None).await.unwrap();

        let installed = installer
            .install(PackSource::LocalDirectory {
                path: source_dir.clone(),
            })
            .await
            .unwrap();

        assert!(installed.path.join("pack.yaml").is_file());
        assert!(matches!(
            installed.source,
            PackSource::LocalDirectory { path } if path == source_dir
        ));
    }

    #[tokio::test]
    async fn direct_remote_sources_require_explicit_opt_in() {
        let temp_dir = tempfile::tempdir().unwrap();
        let installer = PackInstaller::new(temp_dir.path(), Some(PackRegistryConfig::default()))
            .await
            .unwrap();

        for source in [
            PackSource::Git {
                url: "https://github.com/attacker/pack.git".to_string(),
                git_ref: None,
            },
            PackSource::Archive {
                url: "https://codeload.github.com/attacker/pack/tar.gz/main".to_string(),
            },
        ] {
            let error = installer.install(source).await.unwrap_err();
            assert!(error
                .to_string()
                .contains("allow_unverified_direct_remote_installs"));
        }
    }

    #[tokio::test]
    async fn direct_remote_opt_in_reaches_source_validation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = PackRegistryConfig {
            allow_unverified_direct_remote_installs: true,
            ..Default::default()
        };
        let installer = PackInstaller::new(temp_dir.path(), Some(config))
            .await
            .unwrap();

        let error = installer
            .install(PackSource::Git {
                url: "https://github.com/attacker/pack.git".to_string(),
                git_ref: Some("--config".to_string()),
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("must not start with '-'"));
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
            format!("sha256:{calculated}")
        );
        assert!(verify_git_content_checksum(directory.path(), "sha256:deadbeef").is_err());
    }

    #[tokio::test]
    async fn failed_git_clone_removes_its_temp_directory() {
        let temp = tempfile::tempdir().unwrap();
        let config = PackRegistryConfig {
            indices: Vec::new(),
            approved_public_hosts: Vec::new(),
            approved_private_hosts: vec!["127.0.0.1".to_string()],
            timeout: 5,
            ..Default::default()
        };
        let installer = PackInstaller::new(temp.path(), Some(config)).await.unwrap();

        let error = installer
            .install_from_git("https://127.0.0.1:1/unavailable.git", Some("deadbeef"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("Git clone"));
        assert_eq!(
            std::fs::read_dir(temp.path().join("pack-installs"))
                .unwrap()
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn failed_git_checksum_removes_install_and_success_is_canonical() {
        let temp = tempfile::tempdir().unwrap();
        let installer = PackInstaller::new(temp.path(), Some(PackRegistryConfig::default()))
            .await
            .unwrap();

        let failed_root = installer.create_temp_dir().await.unwrap();
        std::fs::write(failed_root.join("pack.yaml"), "ref: failed\n").unwrap();
        let failed = InstalledPack {
            path: failed_root.clone(),
            source: PackSource::Git {
                url: "https://example.com/failed.git".to_string(),
                git_ref: Some("deadbeef".to_string()),
            },
            checksum: None,
            checksum_subject: None,
            checksum_verified: false,
            registry_identity: None,
        };
        let error = installer
            .verify_registry_git_install(failed, &format!("sha256:{}", "0".repeat(64)))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"));
        assert!(!failed_root.exists());

        let successful_root = installer.create_temp_dir().await.unwrap();
        std::fs::write(successful_root.join("pack.yaml"), "ref: successful\n").unwrap();
        let digest = calculate_directory_checksum(&successful_root).unwrap();
        let successful = InstalledPack {
            path: successful_root,
            source: PackSource::Git {
                url: "https://example.com/successful.git".to_string(),
                git_ref: Some("deadbeef".to_string()),
            },
            checksum: None,
            checksum_subject: None,
            checksum_verified: false,
            registry_identity: None,
        };
        let installed = installer
            .verify_registry_git_install(successful, &format!("sha256:{digest}"))
            .await
            .unwrap();
        assert!(installed.checksum_verified);
        assert_eq!(
            installed.checksum.as_deref(),
            Some(format!("sha256:{digest}").as_str())
        );
        assert_eq!(
            installed.checksum_subject,
            Some(ChecksumSubject::DirectoryContent)
        );
    }

    #[tokio::test]
    async fn failed_local_directory_install_removes_partial_tree() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("not-a-pack.txt"), "copied before validation").unwrap();
        let installer = PackInstaller::new(temp.path(), None).await.unwrap();

        let error = installer
            .install_from_local_directory(&source)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("pack.yaml not found"));
        assert_eq!(
            std::fs::read_dir(temp.path().join("pack-installs"))
                .unwrap()
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejected_local_directory_entry_removes_files_already_copied() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("a-pack.yaml"), "partial").unwrap();
        symlink(source.join("a-pack.yaml"), source.join("z-link")).unwrap();
        let installer = PackInstaller::new(temp.path(), None).await.unwrap();

        assert!(installer
            .install_from_local_directory(&source)
            .await
            .is_err());
        assert_eq!(
            std::fs::read_dir(temp.path().join("pack-installs"))
                .unwrap()
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn unverified_archive_preserves_declared_archive_checksum() {
        use tokio::io::AsyncWriteExt;

        let archive = test_pack_archive();
        let checksum_file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(checksum_file.path(), &archive).unwrap();
        let checksum = format!(
            "sha256:{}",
            calculate_file_checksum(checksum_file.path()).unwrap()
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                archive.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&archive).await.unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let config = PackRegistryConfig {
            verify_checksums: false,
            approved_public_hosts: Vec::new(),
            approved_private_hosts: vec!["127.0.0.1".to_string()],
            allow_http: true,
            ..Default::default()
        };
        let installer = PackInstaller::new(temp.path(), Some(config)).await.unwrap();

        let installed = installer
            .install_from_archive_url(&format!("http://{address}/pack.tar.gz"), Some(&checksum))
            .await
            .unwrap();
        server.await.unwrap();

        assert_eq!(installed.checksum.as_deref(), Some(checksum.as_str()));
        assert_eq!(
            installed.checksum_subject,
            Some(ChecksumSubject::ArchiveBytes)
        );
        assert!(!installed.checksum_verified);
    }

    #[tokio::test]
    async fn archive_verification_accepts_only_exact_sha256() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("archive.tar.gz");
        std::fs::write(&archive, b"archive bytes").unwrap();
        let digest = calculate_file_checksum(&archive).unwrap();
        let installer = PackInstaller::new(temp.path(), Some(PackRegistryConfig::default()))
            .await
            .unwrap();

        installer
            .verify_archive_checksum(&archive, &format!("sha256:{digest}"))
            .await
            .unwrap();
        assert!(installer
            .verify_archive_checksum(&archive, &format!("md5:{}", "0".repeat(32)))
            .await
            .is_err());
        assert!(installer
            .verify_archive_checksum(&archive, &format!("sha256:{}", "0".repeat(64)))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn archive_failures_remove_downloads_and_extraction_directories() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\ninvalid",
                )
                .await
                .unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let config = PackRegistryConfig {
            indices: Vec::new(),
            approved_public_hosts: Vec::new(),
            approved_private_hosts: vec!["127.0.0.1".to_string()],
            allow_http: true,
            ..Default::default()
        };
        let installer = PackInstaller::new(temp.path(), Some(config)).await.unwrap();

        let error = installer
            .install_from_archive_url(
                &format!("http://{address}/pack.tar.gz"),
                Some(&format!("sha256:{}", "0".repeat(64))),
            )
            .await
            .unwrap_err();
        server.await.unwrap();
        assert!(error.to_string().contains("Checksum mismatch"));
        assert_eq!(
            std::fs::read_dir(temp.path().join("pack-installs"))
                .unwrap()
                .count(),
            0
        );

        let invalid_archive = temp.path().join("invalid.tar.gz");
        std::fs::write(&invalid_archive, b"invalid").unwrap();
        assert!(installer.extract_archive(&invalid_archive).await.is_err());
        assert_eq!(
            std::fs::read_dir(temp.path().join("pack-installs"))
                .unwrap()
                .count(),
            0
        );
    }
}
