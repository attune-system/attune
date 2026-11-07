use crate::credentials::{self, storage, Envelope, Tokens};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::output::OutputFormat;

/// CLI configuration stored in user's home directory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    /// Current active profile name
    #[serde(
        default = "default_profile_name",
        rename = "profile",
        alias = "current_profile"
    )]
    pub current_profile: String,
    /// Named profiles (like SSH hosts)
    #[serde(default)]
    pub profiles: HashMap<String, Profile>,
    /// Output format (table, json, yaml)
    #[serde(
        default = "default_format",
        rename = "format",
        alias = "default_output_format"
    )]
    pub format: String,
    #[serde(skip)]
    snapshot: RefCell<Option<Snapshot>>,
    #[serde(skip)]
    source_path: Option<PathBuf>,
    #[serde(skip)]
    pub ephemeral_auth: bool,
    #[serde(skip)]
    preserve_selection: bool,
}

#[derive(Clone)]
struct Snapshot(serde_json::Value);
impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}
fn encryption_default() -> bool {
    true
}

fn default_profile_name() -> String {
    "default".to_string()
}

fn default_format() -> String {
    "table".to_string()
}

/// A named profile for connecting to an Attune server
#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default = "encryption_default")]
    pub credential_encryption: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<Envelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_endpoint: Option<String>,
    /// API endpoint URL
    pub api_url: String,
    /// Authentication token
    #[serde(skip_serializing)]
    pub auth_token: Option<String>,
    /// Refresh token
    #[serde(skip_serializing)]
    pub refresh_token: Option<String>,
    /// Output format override for this profile (deprecated — ignored, kept for deserialization compat)
    #[serde(skip_serializing)]
    #[allow(dead_code)]
    pub output_format: Option<String>,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Authentication method used for the last login ("direct", "sso", "ldap", "token")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,
    /// Username/login from the last direct or LDAP login
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Profile")
            .field("credential_encryption", &self.credential_encryption)
            .field("tokens", &"[redacted]")
            .finish_non_exhaustive()
    }
}
impl Profile {
    pub fn clear_credentials(&mut self) {
        self.auth_token = None;
        self.refresh_token = None;
        self.credentials = None;
        self.credentials_endpoint = None;
    }
    pub fn has_credentials(&self) -> bool {
        self.credentials.is_some() || self.auth_token.is_some() || self.refresh_token.is_some()
    }
    pub fn redacted(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("Profile serialization");
        value.as_object_mut().unwrap().remove("credentials");
        value["auth_token"] = serde_json::json!(if self.has_credentials() {
            "***"
        } else {
            "(not set)"
        });
        value["refresh_token"] = serde_json::json!(if self.has_credentials() {
            "***"
        } else {
            "(not set)"
        });
        value
    }
}

impl Default for CliConfig {
    fn default() -> Self {
        let mut profiles = HashMap::new();
        profiles.insert(
            "default".to_string(),
            Profile {
                credential_encryption: true,
                credentials: None,
                credentials_endpoint: None,
                api_url: "http://localhost:8080".to_string(),
                auth_token: None,
                refresh_token: None,
                output_format: None,
                description: Some("Default local server".to_string()),
                auth_method: None,
                username: None,
            },
        );

        Self {
            current_profile: "default".to_string(),
            profiles,
            format: default_format(),
            snapshot: RefCell::new(None),
            source_path: None,
            ephemeral_auth: false,
            preserve_selection: false,
        }
    }
}

impl CliConfig {
    /// Get the configuration file path
    pub fn config_path() -> Result<PathBuf> {
        // Respect XDG_CONFIG_HOME environment variable (for tests and user overrides)
        let config_dir = if let Ok(xdg_config) = env::var("XDG_CONFIG_HOME") {
            PathBuf::from(xdg_config)
        } else {
            dirs::config_dir().context("Failed to determine config directory")?
        };

        let attune_config_dir = config_dir.join("attune");
        fs::create_dir_all(&attune_config_dir).context("Failed to create config directory")?;

        Ok(attune_config_dir.join("config.yaml"))
    }

    /// Load metadata without unlocking credentials. This keeps login, logout,
    /// profile management and completion usable after identifier changes.
    pub fn load() -> Result<Self> {
        Self::load_at(&Self::config_path()?)
    }

    pub(crate) fn load_at(path: &Path) -> Result<Self> {
        let _lock = storage::lock(path)?;
        if !path.exists() {
            let config = Self {
                source_path: Some(path.to_owned()),
                ..Self::default()
            };
            let value = config.disk_value()?;
            storage::write(path, serde_yaml_ng::to_string(&value)?.as_bytes())?;
            *config.snapshot.borrow_mut() = Some(Snapshot(value));
            return Ok(config);
        }
        storage::private(path)?;
        let (value, migrated) = Self::read_disk(path)?;
        if migrated {
            storage::write(path, serde_yaml_ng::to_string(&value)?.as_bytes())?;
            eprintln!("Removed legacy plaintext Attune credentials; log in again.");
        }
        let mut config: Self = serde_json::from_value(value.clone())
            .map_err(|_| anyhow::anyhow!("Invalid CLI configuration"))?;
        config.source_path = Some(path.to_owned());
        *config.snapshot.borrow_mut() = Some(Snapshot(value));
        Ok(config)
    }

    fn read_disk(path: &Path) -> Result<(serde_json::Value, bool)> {
        storage::reject_link(path)?;
        let content = fs::read_to_string(path).context("Failed to read config file")?;
        // Never include parser diagnostics: they may contain token-bearing input.
        let mut value: serde_json::Value = serde_yaml_ng::from_str(&content)
            .map_err(|_| anyhow::anyhow!("Invalid CLI configuration"))?;
        let mut migrated = false;
        if let Some(profiles) = value.get_mut("profiles").and_then(|v| v.as_object_mut()) {
            for profile in profiles.values_mut() {
                if profile
                    .get("credential_encryption")
                    .and_then(|v| v.as_bool())
                    != Some(false)
                {
                    for field in ["auth_token", "refresh_token"] {
                        if profile
                            .as_object_mut()
                            .context("Invalid profile")?
                            .remove(field)
                            .is_some()
                        {
                            migrated = true;
                        }
                    }
                }
            }
        }
        let config: Self = serde_json::from_value(value)
            .map_err(|_| anyhow::anyhow!("Invalid CLI configuration"))?;
        // Normalize aliases/defaults before capturing a comparison snapshot.
        let mut normalized = serde_json::to_value(&config)?;
        for (name, profile) in &config.profiles {
            if !profile.credential_encryption {
                if profile.credentials.is_some() {
                    anyhow::bail!("Encrypted credentials require credential_encryption: true; change the setting with attune config set");
                }
                normalized["profiles"][name]["auth_token"] = serde_json::json!(profile.auth_token);
                normalized["profiles"][name]["refresh_token"] =
                    serde_json::json!(profile.refresh_token);
            }
        }
        Ok((normalized, migrated))
    }

    /// Completion reads do not mutate or migrate the configuration.
    pub fn load_existing() -> Result<Option<Self>> {
        let config_dir = env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .ok()
            .or_else(dirs::config_dir)
            .context("Cannot determine config directory")?;
        let path = config_dir.join("attune/config.yaml");
        if !path.exists() {
            return Ok(None);
        }
        let (value, _) = Self::read_disk(&path)?;
        let mut config: Self = serde_json::from_value(value.clone())?;
        config.source_path = Some(path);
        *config.snapshot.borrow_mut() = Some(Snapshot(value));
        Ok(Some(config))
    }

    pub fn load_existing_with_profile(profile_name: Option<&str>) -> Result<Option<Self>> {
        let Some(mut config) = Self::load_existing()? else {
            return Ok(None);
        };
        if let Some(name) = profile_name {
            if !config.profiles.contains_key(name) {
                anyhow::bail!("Profile '{}' does not exist", name);
            }
            config.current_profile = name.to_string();
            config.preserve_selection = true;
        }
        Ok(Some(config))
    }

    pub(crate) fn is_persisted(&self) -> bool {
        self.source_path.is_some()
    }
    pub(crate) fn select_profile(&mut self, name: &str) -> Result<()> {
        if !self.profiles.contains_key(name) {
            anyhow::bail!("Profile does not exist");
        }
        self.current_profile = name.to_owned();
        self.preserve_selection = true;
        Ok(())
    }

    pub(crate) fn path(&self) -> Result<PathBuf> {
        self.source_path
            .clone()
            .map(Ok)
            .unwrap_or_else(Self::config_path)
    }

    fn disk_value(&self) -> Result<serde_json::Value> {
        let mut value = serde_json::to_value(self)?;
        for (name, profile) in &self.profiles {
            if profile.auth_token.is_some() || profile.refresh_token.is_some() {
                let url = credentials::endpoint(&profile.api_url)?;
                if !profile.credential_encryption
                    && profile
                        .credentials_endpoint
                        .as_ref()
                        .is_some_and(|stored| stored != &url)
                {
                    anyhow::bail!(
                        "Stored credentials belong to a different endpoint; log in again"
                    );
                }
                value["profiles"][name]["credentials_endpoint"] = serde_json::json!(url);
                if profile.credential_encryption {
                    let tokens = Tokens {
                        access_token: profile.auth_token.clone(),
                        refresh_token: profile.refresh_token.clone(),
                    };
                    value["profiles"][name]["credentials"] =
                        serde_json::to_value(match &profile.credentials {
                            Some(existing) => existing.replace(&tokens, name, &profile.api_url)?,
                            None => Envelope::seal(&tokens, name, &profile.api_url)?,
                        })?;
                } else {
                    value["profiles"][name]["auth_token"] = serde_json::json!(profile.auth_token);
                    value["profiles"][name]["refresh_token"] =
                        serde_json::json!(profile.refresh_token);
                }
            } else if !profile.credential_encryption {
                value["profiles"][name]["auth_token"] = serde_json::Value::Null;
                value["profiles"][name]["refresh_token"] = serde_json::Value::Null;
            }
        }
        Ok(value)
    }

    /// Merge independent profile edits under a cross-process lock. Reject stale
    /// edits to the same profile instead of overwriting rotated credentials.
    pub fn save(&self) -> Result<()> {
        if self.ephemeral_auth {
            anyhow::bail!("Runtime credentials cannot be persisted");
        }
        let path = self.path()?;
        let _lock = storage::lock(&path)?;
        if self.snapshot.borrow().is_some() && !path.exists() {
            anyhow::bail!("Configuration was removed in another process; retry the operation");
        }
        let desired = self.disk_value()?;
        let mut latest = if path.exists() {
            Self::read_disk(&path)?.0
        } else {
            serde_json::to_value(Self::default())?
        };
        if let Some(Snapshot(base)) = self.snapshot.borrow().as_ref() {
            let before = base["profiles"].as_object().context("Invalid profiles")?;
            let after = desired["profiles"]
                .as_object()
                .context("Invalid profiles")?;
            let current = latest["profiles"]
                .as_object_mut()
                .context("Invalid profiles")?;
            let names: std::collections::HashSet<_> = before.keys().chain(after.keys()).collect();
            for name in names {
                if before.get(name) != after.get(name) {
                    if before.get(name) != current.get(name) {
                        anyhow::bail!("Profile changed in another process; retry the operation");
                    }
                    match after.get(name) {
                        Some(v) => {
                            current.insert(name.clone(), v.clone());
                        }
                        None => {
                            current.remove(name);
                        }
                    }
                }
            }
            for key in ["profile", "format"] {
                if key == "profile" && self.preserve_selection {
                    continue;
                }
                if desired[key] != base[key] {
                    if latest[key] != base[key] {
                        anyhow::bail!("Configuration changed in another process; retry");
                    }
                    latest[key] = desired[key].clone();
                }
            }
        } else {
            latest = desired.clone();
        }
        storage::write(&path, serde_yaml_ng::to_string(&latest)?.as_bytes())?;
        // Track this object's serialized state, not concurrent edits merged
        // into the file. The object still holds its original values for those
        // fields; treating the merged file as its baseline would turn stale
        // values into apparent edits on its next save (including old tokens).
        *self.snapshot.borrow_mut() = Some(Snapshot(desired));
        Ok(())
    }

    pub fn tokens(&self) -> Result<Tokens> {
        let profile = self.current_profile()?;
        if let Some(envelope) = &profile.credentials {
            if profile.auth_token.is_none() && profile.refresh_token.is_none() {
                return envelope.open(&self.current_profile, &profile.api_url);
            }
        }
        if !self.ephemeral_auth
            && !profile.credential_encryption
            && profile.has_credentials()
            && profile.credentials_endpoint.as_deref()
                != Some(credentials::endpoint(&profile.api_url)?.as_str())
        {
            anyhow::bail!("Stored credentials belong to a different endpoint; log in again");
        }
        Ok(Tokens {
            access_token: profile.auth_token.clone(),
            refresh_token: profile.refresh_token.clone(),
        })
    }

    pub fn set_encryption(&mut self, enabled: bool) -> Result<()> {
        if self.current_profile()?.credential_encryption == enabled {
            return Ok(());
        }
        let tokens = self.tokens()?;
        let profile = self.current_profile_mut()?;
        profile.credential_encryption = enabled;
        profile.credentials = None;
        profile.auth_token = tokens.access_token;
        profile.refresh_token = tokens.refresh_token;
        self.save()
    }

    /// Get the current active profile
    pub fn current_profile(&self) -> Result<&Profile> {
        self.profiles
            .get(&self.current_profile)
            .context(format!("Profile '{}' not found", self.current_profile))
    }

    /// Get a mutable reference to the current profile
    pub fn current_profile_mut(&mut self) -> Result<&mut Profile> {
        let profile_name = self.current_profile.clone();
        self.profiles
            .get_mut(&profile_name)
            .context(format!("Profile '{}' not found", profile_name))
    }

    /// Get a profile by name
    pub fn get_profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }

    /// Switch to a different profile
    pub fn switch_profile(&mut self, name: String) -> Result<()> {
        if !self.profiles.contains_key(&name) {
            anyhow::bail!("Profile '{}' does not exist", name);
        }
        self.current_profile = name;
        self.preserve_selection = false;
        self.save()
    }

    /// Add or update a profile
    pub fn set_profile(&mut self, name: String, profile: Profile) -> Result<()> {
        self.profiles.insert(name, profile);
        self.save()
    }

    /// Remove a profile
    pub fn remove_profile(&mut self, name: &str) -> Result<()> {
        if self.current_profile == name {
            anyhow::bail!("Cannot remove active profile");
        }
        if name == "default" {
            anyhow::bail!("Cannot remove the default profile");
        }
        self.profiles.remove(name);
        self.save()
    }

    /// List all profile names
    pub fn list_profiles(&self) -> Vec<String> {
        let mut names: Vec<String> = self.profiles.keys().cloned().collect();
        names.sort();
        names
    }

    /// Set the API URL for the current profile
    ///
    /// Part of configuration management API - used by `attune config set api-url` command
    #[allow(dead_code)]
    pub fn set_api_url(&mut self, url: String) -> Result<()> {
        let profile = self.current_profile_mut()?;
        profile.clear_credentials();
        profile.api_url = url;
        self.save()
    }

    /// Set authentication tokens for the current profile
    pub fn set_auth(&mut self, access_token: String, refresh_token: String) -> Result<()> {
        let profile = self.current_profile_mut()?;
        profile.auth_token = Some(access_token);
        profile.refresh_token = Some(refresh_token);
        self.save()
    }

    /// Clear authentication tokens for the current profile
    pub fn clear_auth(&mut self) -> Result<()> {
        let profile = self.current_profile_mut()?;
        profile.clear_credentials();
        self.save()
    }

    /// Resolve the effective output format.
    ///
    /// Priority (highest to lowest):
    /// 1. Explicit CLI flag (`--json`, `--yaml`, `--output`)
    /// 2. Config `format` field
    ///
    /// The `cli_flag` parameter should be `None` when the user did not pass an
    /// explicit flag (i.e. clap returned the default value `table` *without*
    /// the user typing it). Callers should pass `Some(format)` only when the
    /// user actually supplied the flag.
    pub fn effective_format(&self, cli_override: Option<OutputFormat>) -> OutputFormat {
        if let Some(fmt) = cli_override {
            return fmt;
        }

        // Fall back to config value
        match self.format.to_lowercase().as_str() {
            "json" => OutputFormat::Json,
            "yaml" => OutputFormat::Yaml,
            _ => OutputFormat::Table,
        }
    }

    /// Set a configuration value by key
    pub fn set_value(&mut self, key: &str, value: String) -> Result<()> {
        match key {
            "credential_encryption" => {
                return self.set_encryption(value.parse().context("Expected true or false")?)
            }
            "api_url" => {
                let profile = self.current_profile_mut()?;
                profile.clear_credentials();
                profile.api_url = value;
            }
            "format" | "output_format" | "default_output_format" => {
                // Validate the value
                match value.to_lowercase().as_str() {
                    "table" | "json" | "yaml" => {}
                    _ => anyhow::bail!(
                        "Invalid format '{}'. Must be one of: table, json, yaml",
                        value
                    ),
                }
                self.format = value.to_lowercase();
            }
            "profile" | "current_profile" => {
                self.switch_profile(value)?;
                return Ok(());
            }
            _ => anyhow::bail!("Unknown config key: {}", key),
        }
        self.save()
    }

    /// Get a configuration value by key
    pub fn get_value(&self, key: &str) -> Result<String> {
        match key {
            "credential_encryption" => {
                Ok(self.current_profile()?.credential_encryption.to_string())
            }
            "api_url" => {
                let profile = self.current_profile()?;
                Ok(profile.api_url.clone())
            }
            "format" | "output_format" | "default_output_format" => Ok(self.format.clone()),
            "profile" | "current_profile" => Ok(self.current_profile.clone()),
            "auth_token" => {
                let profile = self.current_profile()?;
                Ok((if profile.has_credentials() {
                    "***"
                } else {
                    "(not set)"
                })
                .to_string())
            }
            "refresh_token" => {
                let profile = self.current_profile()?;
                Ok((if profile.has_credentials() {
                    "***"
                } else {
                    "(not set)"
                })
                .to_string())
            }
            _ => anyhow::bail!("Unknown config key: {}", key),
        }
    }

    /// List all configuration keys and values for current profile
    pub fn list_all(&self) -> Vec<(String, String)> {
        let profile = match self.current_profile() {
            Ok(p) => p,
            Err(_) => return vec![],
        };

        vec![
            ("profile".to_string(), self.current_profile.clone()),
            ("format".to_string(), self.format.clone()),
            ("api_url".to_string(), profile.api_url.clone()),
            (
                "credential_encryption".to_string(),
                profile.credential_encryption.to_string(),
            ),
            (
                "auth_token".to_string(),
                (if profile.has_credentials() {
                    "***"
                } else {
                    "(not set)"
                })
                .to_string(),
            ),
            (
                "refresh_token".to_string(),
                (if profile.has_credentials() {
                    "***"
                } else {
                    "(not set)"
                })
                .to_string(),
            ),
        ]
    }

    /// Load configuration with optional profile override (without saving)
    ///
    /// Used by `--profile` flag to temporarily use a different profile
    pub fn load_with_profile(profile_name: Option<&str>) -> Result<Self> {
        let mut config = Self::load()?;

        if let Some(name) = profile_name {
            // Temporarily switch profile without saving
            if !config.profiles.contains_key(name) {
                anyhow::bail!("Profile '{}' does not exist", name);
            }
            config.current_profile = name.to_string();
            config.preserve_selection = true;
        }

        Ok(config)
    }

    /// Get the effective API URL (from override, current profile, or default)
    pub fn effective_api_url(&self, override_url: &Option<String>) -> String {
        if let Some(url) = override_url {
            return url.clone();
        }

        if let Ok(profile) = self.current_profile() {
            profile.api_url.clone()
        } else {
            "http://localhost:8080".to_string()
        }
    }

    /// Get API URL for current profile (without override)
    #[allow(unused)]
    pub fn api_url(&self) -> Result<String> {
        let profile = self.current_profile()?;
        Ok(profile.api_url.clone())
    }

    /// Get auth token for current profile
    pub fn auth_token(&self) -> Result<Option<String>> {
        Ok(self.tokens()?.access_token)
    }

    /// Get refresh token for current profile
    pub fn refresh_token(&self) -> Result<Option<String>> {
        Ok(self.tokens()?.refresh_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use tempfile::TempDir;

    static CONFIG_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct TestConfigHomeGuard {
        _env_lock: MutexGuard<'static, ()>,
        _temp_dir: TempDir,
        previous_xdg_config_home: Option<String>,
    }

    impl TestConfigHomeGuard {
        fn new() -> Self {
            let env_lock = CONFIG_ENV_LOCK
                .get_or_init(|| Mutex::new(()))
                .lock()
                .expect("Failed to lock config test environment");
            let temp_dir = TempDir::new().expect("Failed to create temp config dir");
            let previous_xdg_config_home = env::var("XDG_CONFIG_HOME").ok();
            unsafe {
                env::set_var("XDG_CONFIG_HOME", temp_dir.path());
            }

            Self {
                _env_lock: env_lock,
                _temp_dir: temp_dir,
                previous_xdg_config_home,
            }
        }
    }

    impl Drop for TestConfigHomeGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.previous_xdg_config_home {
                    Some(value) => env::set_var("XDG_CONFIG_HOME", value),
                    None => env::remove_var("XDG_CONFIG_HOME"),
                }
            }
        }
    }

    #[test]
    fn test_default_config() {
        let config = CliConfig::default();
        assert_eq!(config.current_profile, "default");
        assert_eq!(config.format, "table");
        assert!(config.profiles.contains_key("default"));

        let profile = config.current_profile().unwrap();
        assert_eq!(profile.api_url, "http://localhost:8080");
        assert!(profile.auth_token.is_none());
        assert!(profile.refresh_token.is_none());
    }

    #[test]
    fn test_effective_api_url() {
        let config = CliConfig::default();

        // No override
        assert_eq!(config.effective_api_url(&None), "http://localhost:8080");

        // With override
        let override_url = Some("http://example.com".to_string());
        assert_eq!(
            config.effective_api_url(&override_url),
            "http://example.com"
        );
    }

    #[test]
    fn test_effective_format_defaults_to_config() {
        let config = CliConfig {
            format: "json".to_string(),
            ..Default::default()
        };

        // No CLI override → uses config
        assert_eq!(config.effective_format(None), OutputFormat::Json);
    }

    #[test]
    fn test_effective_format_cli_overrides_config() {
        let config = CliConfig {
            format: "json".to_string(),
            ..Default::default()
        };

        // CLI override wins
        assert_eq!(
            config.effective_format(Some(OutputFormat::Yaml)),
            OutputFormat::Yaml
        );
    }

    #[test]
    fn test_effective_format_default_table() {
        let config = CliConfig::default();
        assert_eq!(config.effective_format(None), OutputFormat::Table);
    }

    #[test]
    fn test_profile_management() {
        let _guard = TestConfigHomeGuard::new();
        let mut config = CliConfig::default();

        // Add a new profile
        let staging_profile = Profile {
            credential_encryption: true,
            credentials: None,
            credentials_endpoint: None,
            api_url: "https://staging.example.com".to_string(),
            auth_token: None,
            refresh_token: None,
            output_format: None,
            description: Some("Staging environment".to_string()),
            auth_method: None,
            username: None,
        };
        config
            .set_profile("staging".to_string(), staging_profile)
            .unwrap();

        // List profiles
        let profiles = config.list_profiles();
        assert!(profiles.contains(&"default".to_string()));
        assert!(profiles.contains(&"staging".to_string()));

        // Switch to staging
        config.switch_profile("staging".to_string()).unwrap();
        assert_eq!(config.current_profile, "staging");

        let profile = config.current_profile().unwrap();
        assert_eq!(profile.api_url, "https://staging.example.com");
    }

    #[test]
    fn test_cannot_remove_default_profile() {
        let _guard = TestConfigHomeGuard::new();
        let mut config = CliConfig::default();
        let result = config.remove_profile("default");
        assert!(result.is_err());
    }

    #[test]
    fn test_cannot_remove_active_profile() {
        let _guard = TestConfigHomeGuard::new();
        let mut config = CliConfig::default();

        let test_profile = Profile {
            credential_encryption: true,
            credentials: None,
            credentials_endpoint: None,
            api_url: "http://test.com".to_string(),
            auth_token: None,
            refresh_token: None,
            output_format: None,
            description: None,
            auth_method: None,
            username: None,
        };
        config
            .set_profile("test".to_string(), test_profile)
            .unwrap();
        config.switch_profile("test".to_string()).unwrap();

        let result = config.remove_profile("test");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_set_value() {
        let _guard = TestConfigHomeGuard::new();
        let mut config = CliConfig::default();

        assert_eq!(
            config.get_value("api_url").unwrap(),
            "http://localhost:8080"
        );
        assert_eq!(config.get_value("format").unwrap(), "table");

        // Set API URL for current profile
        config
            .set_value("api_url", "http://test.com".to_string())
            .unwrap();
        assert_eq!(config.get_value("api_url").unwrap(), "http://test.com");

        // Set format
        config.set_value("format", "json".to_string()).unwrap();
        assert_eq!(config.get_value("format").unwrap(), "json");
    }

    #[test]
    fn test_set_value_validates_format() {
        let _guard = TestConfigHomeGuard::new();
        let mut config = CliConfig::default();

        // Valid values
        assert!(config.set_value("format", "table".to_string()).is_ok());
        assert!(config.set_value("format", "json".to_string()).is_ok());
        assert!(config.set_value("format", "yaml".to_string()).is_ok());
        assert!(config.set_value("format", "JSON".to_string()).is_ok()); // case-insensitive

        // Invalid value
        assert!(config.set_value("format", "xml".to_string()).is_err());
    }

    #[test]
    fn test_backward_compat_aliases() {
        let _guard = TestConfigHomeGuard::new();
        let mut config = CliConfig::default();

        // Old key names should still work for get/set
        assert!(config
            .set_value("output_format", "json".to_string())
            .is_ok());
        assert_eq!(config.get_value("output_format").unwrap(), "json");
        assert_eq!(config.get_value("format").unwrap(), "json");

        assert!(config
            .set_value("default_output_format", "yaml".to_string())
            .is_ok());
        assert_eq!(config.get_value("default_output_format").unwrap(), "yaml");
        assert_eq!(config.get_value("format").unwrap(), "yaml");
    }

    #[test]
    fn test_deserialize_legacy_default_output_format() {
        let yaml = r#"
profile: default
default_output_format: json
profiles:
  default:
    api_url: http://localhost:8080
"#;
        let config: CliConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(config.format, "json");
    }
}

#[cfg(test)]
mod credential_tests {
    use super::*;

    #[test]
    fn legacy_tokens_are_removed_without_unlocking_or_losing_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        fs::write(&path, "profile: default\nprofiles:\n  default:\n    api_url: https://example.com\n    auth_token: old-access\n    refresh_token: old-refresh\n    description: keep\n").unwrap();
        let config = CliConfig::load_at(&path).unwrap();
        assert_eq!(
            config.current_profile().unwrap().description.as_deref(),
            Some("keep")
        );
        assert_eq!(config.tokens().unwrap(), Tokens::default());
        let disk = fs::read_to_string(&path).unwrap();
        assert!(!disk.contains("old-access"));
        assert!(!disk.contains("old-refresh"));
        assert!(config.current_profile().unwrap().credential_encryption);
    }

    #[test]
    fn encrypted_roundtrip_optout_and_unreadable_profile_management() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let mut config = CliConfig::load_at(&path).unwrap();
        config
            .set_auth("access-secret".into(), "refresh-secret".into())
            .unwrap();
        let disk = fs::read_to_string(&path).unwrap();
        assert!(!disk.contains("access-secret"));
        assert!(!disk.contains("refresh-secret"));
        let mut loaded = CliConfig::load_at(&path).unwrap();
        let tokens = loaded.tokens().unwrap();
        assert_eq!(tokens.access_token.as_deref(), Some("access-secret"));
        assert!(!format!("{loaded:?}").contains("access-secret"));
        assert!(!loaded
            .current_profile()
            .unwrap()
            .redacted()
            .to_string()
            .contains("access-secret"));
        let before = loaded
            .current_profile()
            .unwrap()
            .credentials
            .clone()
            .unwrap();
        loaded
            .set_auth("new-access".into(), "new-refresh".into())
            .unwrap();
        let mut loaded = CliConfig::load_at(&path).unwrap();
        let after = loaded
            .current_profile()
            .unwrap()
            .credentials
            .clone()
            .unwrap();
        assert_eq!(before.profile_id, after.profile_id);
        assert_eq!(before.salt, after.salt);
        assert_eq!(before.sources, after.sources);
        assert_ne!(before.nonce, after.nonce);
        loaded.set_encryption(false).unwrap();
        let disk = fs::read_to_string(&path).unwrap();
        assert!(disk.contains("new-access"));
        let mut loaded = CliConfig::load_at(&path).unwrap();
        assert_eq!(loaded.auth_token().unwrap().as_deref(), Some("new-access"));
        assert!(!format!("{loaded:?}").contains("new-access"));
        loaded.set_encryption(true).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains("new-access"));
        let mut loaded = CliConfig::load_at(&path).unwrap();
        loaded
            .current_profile_mut()
            .unwrap()
            .credentials
            .as_mut()
            .unwrap()
            .ciphertext = "invalid".into();
        loaded.save().unwrap();
        let mut loaded = CliConfig::load_at(&path).unwrap();
        assert!(loaded.tokens().is_err());
        assert!(loaded.set_encryption(false).is_err());
        loaded.clear_auth().unwrap(); // recovery doesn't require the old key
        loaded.set_encryption(false).unwrap();
        assert!(!loaded.current_profile().unwrap().has_credentials());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn independent_edits_merge_and_stale_profile_edits_fail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let mut initial = CliConfig::load_at(&path).unwrap();
        let profile = initial.current_profile().unwrap().clone();
        initial.set_profile("other".into(), profile).unwrap();
        let mut a = CliConfig::load_at(&path).unwrap();
        let mut b = CliConfig::load_at(&path).unwrap();
        a.current_profile_mut().unwrap().description = Some("from-a".into());
        a.save().unwrap();
        b.select_profile("other").unwrap();
        b.current_profile_mut().unwrap().description = Some("from-b".into());
        b.save().unwrap();
        let all = CliConfig::load_at(&path).unwrap();
        assert_eq!(
            all.profiles["default"].description.as_deref(),
            Some("from-a")
        );
        assert_eq!(all.profiles["other"].description.as_deref(), Some("from-b"));
        assert_eq!(all.current_profile, "default");
        let mut stale = CliConfig::load_at(&path).unwrap();
        let mut fresh = CliConfig::load_at(&path).unwrap();
        fresh.clear_auth().unwrap();
        fresh.current_profile_mut().unwrap().description = Some("changed".into());
        fresh.save().unwrap();
        stale.current_profile_mut().unwrap().description = Some("stale".into());
        assert!(stale.save().is_err());
    }

    #[test]
    fn repeated_saves_preserve_concurrent_logout_and_metadata_edits() {
        // Exercise both representations: stale plaintext must not be written
        // back, and an old encrypted envelope must not be restored either.
        for encrypted in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.yaml");
            let mut initial = CliConfig::load_at(&path).unwrap();
            initial.set_encryption(encrypted).unwrap();
            initial
                .set_auth("access-secret".into(), "refresh-secret".into())
                .unwrap();
            initial
                .set_profile(
                    "other".into(),
                    CliConfig::default().current_profile().unwrap().clone(),
                )
                .unwrap();
            let mut stale = CliConfig::load_at(&path).unwrap();
            let mut fresh = CliConfig::load_at(&path).unwrap();
            fresh.clear_auth().unwrap();
            fresh.current_profile_mut().unwrap().description = Some("concurrent edit".into());
            fresh.format = "json".into();
            fresh.save().unwrap();
            stale.select_profile("other").unwrap();
            stale.current_profile_mut().unwrap().description = Some("first edit".into());
            stale.save().unwrap();
            stale.current_profile_mut().unwrap().description = Some("second edit".into());
            stale.save().unwrap();
            // A third no-op save must also leave concurrent state untouched.
            stale.save().unwrap();
            let loaded = CliConfig::load_at(&path).unwrap();
            assert!(!loaded.profiles["default"].has_credentials());
            assert_eq!(
                loaded.profiles["default"].description.as_deref(),
                Some("concurrent edit")
            );
            assert_eq!(
                loaded.profiles["other"].description.as_deref(),
                Some("second edit")
            );
            assert_eq!(loaded.format, "json");
        }
    }

    #[test]
    fn saving_an_existing_snapshot_cannot_recreate_a_deleted_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let mut config = CliConfig::load_at(&path).unwrap();
        fs::remove_file(&path).unwrap();
        config.current_profile_mut().unwrap().description = Some("stale edit".into());
        assert!(config.save().is_err());
        assert!(!path.exists());
    }

    #[test]
    fn runtime_overrides_cannot_be_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let mut config = CliConfig::load_at(&path).unwrap();
        let before = fs::read(&path).unwrap();
        config.ephemeral_auth = true;
        config.current_profile_mut().unwrap().auth_token = Some("runtime-secret".into());
        assert!(config.save().is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn plaintext_credentials_are_also_bound_to_the_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let mut config = CliConfig::load_at(&path).unwrap();
        config.set_encryption(false).unwrap();
        config.set_auth("access".into(), "refresh".into()).unwrap();
        let mut config = CliConfig::load_at(&path).unwrap();
        config.current_profile_mut().unwrap().api_url = "https://other.example".into();
        assert!(config.tokens().is_err());
    }
}
