use anyhow::Result;
use attune_common::device_auth::{
    DeviceAuthorizationResponse, DevicePollResponse, DeviceWaitReason,
};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Subcommand)]
pub enum AuthCommands {
    /// Log in to Attune API
    Login {
        /// Username or email
        #[arg(short, long)]
        username: String,

        /// Password (will prompt if not provided)
        #[arg(long)]
        password: Option<String>,

        /// API URL to log in to (saved into the profile for future use)
        #[arg(long)]
        url: Option<String>,

        /// Save credentials into a named profile (creates it if it doesn't exist)
        #[arg(long)]
        save_profile: Option<String>,
    },
    /// Log in using the OIDC Device Authorization Grant, without a local listener.
    SsoLogin {
        /// API URL to log in to (saved into the profile for future use)
        #[arg(long)]
        url: Option<String>,

        /// Save credentials into a named profile (creates it if it doesn't exist)
        #[arg(long)]
        save_profile: Option<String>,

        /// Maximum wait in seconds. Defaults to the provider's device-code lifetime.
        #[arg(long)]
        timeout: Option<u64>,

        /// Print the login URL instead of opening a browser (useful for headless environments)
        #[arg(long)]
        no_browser: bool,
    },
    /// Log in with a revokable integration token
    TokenLogin {
        /// Integration token (will prompt if not provided)
        #[arg(long)]
        token: Option<String>,

        /// API URL to log in to (saved into the profile for future use)
        #[arg(long)]
        url: Option<String>,

        /// Save credentials into a named profile (creates it if it doesn't exist)
        #[arg(long)]
        save_profile: Option<String>,
    },
    /// Manage revokable integration tokens
    Token {
        #[command(subcommand)]
        command: IntegrationTokenCommands,
    },
    /// Log out and clear authentication tokens
    Logout,
    /// Show current authentication status
    Whoami,
    /// Refresh authentication token
    Refresh,
}

#[derive(Subcommand)]
pub enum IntegrationTokenCommands {
    /// Create an integration token for an identity
    Create {
        /// Identity ID that the token authenticates as
        #[arg(long)]
        identity_id: i64,

        /// Human-readable label for the token
        #[arg(long)]
        label: String,

        /// Optional token description
        #[arg(long)]
        description: Option<String>,

        /// Optional RFC3339 expiration timestamp
        #[arg(long)]
        expires_at: Option<String>,
    },
    /// List integration tokens for an identity
    List {
        /// Identity ID
        #[arg(long)]
        identity_id: i64,
    },
    /// Revoke an integration token
    Revoke {
        /// Identity ID that owns the token
        #[arg(long)]
        identity_id: i64,

        /// Integration token ID
        token_id: i64,

        /// Optional revocation reason
        #[arg(long)]
        reason: Option<String>,
    },
    /// Delete an integration token metadata record
    Delete {
        /// Identity ID that owns the token
        #[arg(long)]
        identity_id: i64,

        /// Integration token ID
        token_id: i64,

        /// Skip confirmation
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct LoginRequest {
    login: String,
    password: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TokenLoginRequest {
    token: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct LoginResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Identity {
    id: i64,
    login: String,
    display_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CreateIntegrationTokenRequest {
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RevokeIntegrationTokenRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IntegrationToken {
    id: i64,
    identity_id: i64,
    label: String,
    description: Option<String>,
    token_prefix: String,
    token_suffix: String,
    expires_at: Option<String>,
    last_used_at: Option<String>,
    revoked_at: Option<String>,
    active: bool,
    created: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CreateIntegrationTokenResponse {
    token: String,
    integration_token: IntegrationToken,
}

#[derive(Debug, Serialize, Deserialize)]
struct SuccessResponse {
    message: String,
}

pub async fn handle_auth_command(
    profile: &Option<String>,
    command: AuthCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        AuthCommands::SsoLogin {
            url,
            save_profile,
            timeout,
            no_browser,
        } => {
            let effective_api_url = url.or_else(|| api_url.clone());
            handle_sso_login(
                save_profile.as_ref().or(profile.as_ref()),
                &effective_api_url,
                timeout,
                no_browser,
                output_format,
            )
            .await
        }
        AuthCommands::Login {
            username,
            password,
            url,
            save_profile,
        } => {
            // --url is a convenient alias for --api-url at login time
            let effective_api_url = url.or_else(|| api_url.clone());
            handle_login(
                username,
                password,
                save_profile.as_ref().or(profile.as_ref()),
                &effective_api_url,
                output_format,
            )
            .await
        }
        AuthCommands::TokenLogin {
            token,
            url,
            save_profile,
        } => {
            let effective_api_url = url.or_else(|| api_url.clone());
            handle_token_login(
                token,
                save_profile.as_ref().or(profile.as_ref()),
                &effective_api_url,
                output_format,
            )
            .await
        }
        AuthCommands::Token { command } => {
            handle_integration_token_command(profile, command, api_url, output_format).await
        }
        AuthCommands::Logout => handle_logout(profile, output_format).await,
        AuthCommands::Whoami => handle_whoami(profile, api_url, output_format).await,
        AuthCommands::Refresh => handle_refresh(profile, api_url, output_format).await,
    }
}

async fn handle_sso_login(
    profile: Option<&String>,
    api_url: &Option<String>,
    timeout: Option<u64>,
    no_browser: bool,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load()?;
    let target_profile_name = profile
        .cloned()
        .unwrap_or_else(|| config.current_profile.clone());

    let base_api_url = api_url.clone().unwrap_or_else(|| {
        config
            .profiles
            .get(&target_profile_name)
            .map(|profile| profile.api_url.clone())
            .unwrap_or_else(|| config.effective_api_url(&None))
    });

    validate_sso_uri(&base_api_url)?;
    if timeout == Some(0) {
        anyhow::bail!("--timeout must be greater than zero");
    }
    let client = ApiClient::from_config_with_timeout(
        &config,
        &Some(base_api_url.clone()),
        // A successful poll can perform four sequential ten-second provider requests.
        Duration::from_secs(60),
    );
    let authorization: DeviceAuthorizationResponse = client
        .post_anonymous("/auth/oidc/device/start", &serde_json::json!({}))
        .await?;
    validate_sso_uri(&authorization.verification_uri)?;
    if let Some(uri) = &authorization.verification_uri_complete {
        validate_sso_uri(uri)?;
    }
    if authorization.user_code.is_empty() || authorization.user_code.chars().any(char::is_control) {
        anyhow::bail!("Authentication server returned an invalid user code");
    }
    eprintln!(
        "Go to {} and enter code {}.",
        authorization.verification_uri, authorization.user_code
    );
    let browser_uri = authorization
        .verification_uri_complete
        .as_deref()
        .unwrap_or(&authorization.verification_uri);
    if !no_browser {
        if let Err(error) = open_browser(browser_uri) {
            eprintln!(
                "Could not open the browser: {error}. Use the URL and code above on any device."
            );
        }
    }
    eprintln!("Waiting for approval. Press Ctrl+C to cancel.");
    let tokens = tokio::select! {
        result = poll_device_login(&client, authorization, timeout) => result?,
        _ = tokio::signal::ctrl_c() => anyhow::bail!("SSO device login cancelled"),
    };

    // Persist tokens.
    let mut config = CliConfig::load()?;
    let p = config
        .profiles
        .entry(target_profile_name.clone())
        .or_insert_with(|| crate::config::Profile {
            api_url: base_api_url.clone(),
            auth_token: None,
            refresh_token: None,
            output_format: None,
            description: None,
            auth_method: None,
            username: None,
        });
    p.api_url = base_api_url;
    p.auth_token = Some(tokens.access_token.clone());
    p.refresh_token = Some(tokens.refresh_token.clone());
    p.auth_method = Some("sso".to_string());
    p.username = None;
    config.save()?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(
                &serde_json::json!({
                    "access_token": tokens.access_token,
                    "refresh_token": tokens.refresh_token,
                    "expires_in": tokens.expires_in,
                }),
                output_format,
            )?;
        }
        OutputFormat::Table => {
            output::print_success("SSO login successful");
            output::print_info(&format!("Token expires in {} seconds", tokens.expires_in));
            if target_profile_name != config.current_profile {
                output::print_info(&format!(
                    "Credentials saved to profile '{target_profile_name}'"
                ));
            }
        }
    }

    Ok(())
}

fn validate_sso_uri(uri: &str) -> Result<()> {
    let url = url::Url::parse(uri)?;
    let loopback = match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || !(url.scheme() == "https" || url.scheme() == "http" && loopback)
    {
        anyhow::bail!("SSO device login URLs must use HTTPS; HTTP is allowed only for loopback development servers");
    }
    Ok(())
}

async fn poll_device_login(
    client: &ApiClient,
    authorization: DeviceAuthorizationResponse,
    timeout: Option<u64>,
) -> Result<LoginResponse> {
    if authorization.expires_in == 0
        || authorization.expires_in > 86400
        || authorization.interval == 0
        || authorization.interval > 86400
    {
        anyhow::bail!("Authentication server returned invalid device polling limits");
    }
    let deadline = tokio::time::Instant::now()
        + Duration::from_secs(
            timeout
                .unwrap_or(authorization.expires_in)
                .min(authorization.expires_in),
        );
    let mut code = authorization.device_code;
    let mut interval = authorization.interval;
    loop {
        tokio::time::timeout_at(deadline, tokio::time::sleep(Duration::from_secs(interval)))
            .await
            .map_err(|_| anyhow::anyhow!("SSO device login timed out; start a new login"))?;
        let response = tokio::time::timeout_at(
            deadline,
            client.post_anonymous::<DevicePollResponse<LoginResponse>, _>(
                "/auth/oidc/device/poll",
                &serde_json::json!({"device_code":code}),
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("SSO device login timed out; start a new login"))?;
        match response {
            Ok(DevicePollResponse::Authorized { tokens }) => {
                if tokens.access_token.is_empty() || tokens.refresh_token.is_empty() {
                    anyhow::bail!("Authentication server returned empty credentials");
                }
                return Ok(tokens);
            }
            Ok(DevicePollResponse::Waiting {
                reason,
                device_code,
                interval: requested,
            }) => {
                let minimum = match reason {
                    DeviceWaitReason::SlowDown => interval.saturating_add(5),
                    DeviceWaitReason::ProviderTimeout => interval.saturating_mul(2),
                    DeviceWaitReason::AuthorizationPending => interval,
                };
                interval = requested.max(minimum).min(86400);
                code = device_code;
            }
            Ok(DevicePollResponse::AccessDenied) => {
                anyhow::bail!("SSO device authorization was denied")
            }
            Ok(DevicePollResponse::Expired) => {
                anyhow::bail!("SSO device code expired; start a new login")
            }
            Err(error)
                if error.chain().any(|cause| {
                    cause
                        .downcast_ref::<reqwest::Error>()
                        .is_some_and(reqwest::Error::is_timeout)
                }) =>
            {
                interval = interval.saturating_mul(2).min(86400);
            }
            Err(error) => return Err(error),
        }
    }
}

/// Open a URL in the system default browser.
fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe")
            .arg(url)
            .spawn()?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        anyhow::bail!("Automatic browser opening is not supported on this platform");
    }
    Ok(())
}

async fn handle_login(
    username: String,
    password: Option<String>,
    profile: Option<&String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let mut config = CliConfig::load()?;
    let target_profile_name = profile
        .cloned()
        .unwrap_or_else(|| config.current_profile.clone());

    if !config.profiles.contains_key(&target_profile_name) {
        let url = api_url
            .clone()
            .unwrap_or_else(|| "http://localhost:8080".to_string());
        use crate::config::Profile;
        config.set_profile(
            target_profile_name.clone(),
            Profile {
                api_url: url,
                auth_token: None,
                refresh_token: None,
                output_format: None,
                description: None,
                auth_method: None,
                username: None,
            },
        )?;
    } else if let Some(url) = api_url {
        if let Some(p) = config.profiles.get_mut(&target_profile_name) {
            p.api_url = url.clone();
        }
        config.save()?;
    }

    let mut login_config = CliConfig::load()?;
    login_config.current_profile = target_profile_name.clone();

    let password = match password {
        Some(p) => p,
        None => dialoguer::Password::new()
            .with_prompt("Password")
            .interact()?,
    };

    let mut client = ApiClient::from_config(&login_config, api_url);

    // Auto-detect: query /auth/settings to determine whether to use local or LDAP login.
    let login_path = match client.get::<serde_json::Value>("/auth/settings").await {
        Ok(settings) => {
            let data = settings.get("data").unwrap_or(&settings);
            let local_enabled = data
                .get("local_password_enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let ldap_enabled = data
                .get("ldap_enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !local_enabled && ldap_enabled {
                "/auth/ldap/login"
            } else {
                "/auth/login"
            }
        }
        // If settings endpoint is unreachable, default to local login.
        Err(_) => "/auth/login",
    };

    let auth_method = if login_path == "/auth/ldap/login" {
        "ldap"
    } else {
        "direct"
    };

    let login_req = LoginRequest {
        login: username.clone(),
        password,
    };

    let response: LoginResponse = client.post(login_path, &login_req).await?;

    let mut config = CliConfig::load()?;
    if let Some(p) = config.profiles.get_mut(&target_profile_name) {
        p.auth_token = Some(response.access_token.clone());
        p.refresh_token = Some(response.refresh_token.clone());
        p.auth_method = Some(auth_method.to_string());
        p.username = Some(username);
        config.save()?;
    } else {
        config.set_auth(
            response.access_token.clone(),
            response.refresh_token.clone(),
        )?;
    }

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&response, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success("Successfully logged in");
            output::print_info(&format!("Token expires in {} seconds", response.expires_in));
            if target_profile_name != config.current_profile {
                output::print_info(&format!(
                    "Credentials saved to profile '{}'",
                    target_profile_name
                ));
            }
        }
    }

    Ok(())
}

async fn handle_token_login(
    token: Option<String>,
    profile: Option<&String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let mut config = CliConfig::load()?;
    let target_profile_name = profile
        .cloned()
        .unwrap_or_else(|| config.current_profile.clone());

    if !config.profiles.contains_key(&target_profile_name) {
        let url = api_url
            .clone()
            .unwrap_or_else(|| "http://localhost:8080".to_string());
        use crate::config::Profile;
        config.set_profile(
            target_profile_name.clone(),
            Profile {
                api_url: url,
                auth_token: None,
                refresh_token: None,
                output_format: None,
                description: None,
                auth_method: None,
                username: None,
            },
        )?;
    } else if let Some(url) = api_url {
        if let Some(p) = config.profiles.get_mut(&target_profile_name) {
            p.api_url = url.clone();
        }
        config.save()?;
    }

    let mut login_config = CliConfig::load()?;
    login_config.current_profile = target_profile_name.clone();

    let token = match token {
        Some(token) => token,
        None => dialoguer::Password::new()
            .with_prompt("Integration token")
            .interact()?,
    };

    let mut client = ApiClient::from_config(&login_config, api_url);
    let response: LoginResponse = client
        .post("/auth/token-login", &TokenLoginRequest { token })
        .await?;

    let mut config = CliConfig::load()?;
    if let Some(p) = config.profiles.get_mut(&target_profile_name) {
        p.auth_token = Some(response.access_token.clone());
        p.refresh_token = Some(response.refresh_token.clone());
        p.auth_method = Some("token".to_string());
        p.username = None;
        config.save()?;
    } else {
        config.set_auth(
            response.access_token.clone(),
            response.refresh_token.clone(),
        )?;
    }

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&response, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success("Successfully logged in with integration token");
            output::print_info(&format!("Token expires in {} seconds", response.expires_in));
            if target_profile_name != config.current_profile {
                output::print_info(&format!(
                    "Credentials saved to profile '{}'",
                    target_profile_name
                ));
            }
        }
    }

    Ok(())
}

async fn handle_logout(profile: &Option<String>, output_format: OutputFormat) -> Result<()> {
    let mut config = CliConfig::load_with_profile(profile.as_deref())?;
    config.clear_auth()?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            let msg = serde_json::json!({"message": "Successfully logged out"});
            output::print_output(&msg, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success("Successfully logged out");
        }
    }

    Ok(())
}

async fn handle_integration_token_command(
    profile: &Option<String>,
    command: IntegrationTokenCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    match command {
        IntegrationTokenCommands::Create {
            identity_id,
            label,
            description,
            expires_at,
        } => {
            let response: CreateIntegrationTokenResponse = client
                .post(
                    &format!("/identities/{identity_id}/integration-tokens"),
                    &CreateIntegrationTokenRequest {
                        label,
                        description,
                        expires_at,
                    },
                )
                .await?;

            match output_format {
                OutputFormat::Json | OutputFormat::Yaml => {
                    output::print_output(&response, output_format)?;
                }
                OutputFormat::Table => {
                    output::print_success("Integration token created");
                    output::print_warning(
                        "Copy this token now. It will not be shown again after this response.",
                    );
                    output::print_key_value_table(vec![
                        ("Token", response.token),
                        ("ID", response.integration_token.id.to_string()),
                        (
                            "Identity ID",
                            response.integration_token.identity_id.to_string(),
                        ),
                        ("Label", response.integration_token.label),
                        (
                            "Expires",
                            response
                                .integration_token
                                .expires_at
                                .unwrap_or_else(|| "never".to_string()),
                        ),
                    ]);
                }
            }
        }
        IntegrationTokenCommands::List { identity_id } => {
            let tokens: Vec<IntegrationToken> = client
                .get(&format!("/identities/{identity_id}/integration-tokens"))
                .await?;

            match output_format {
                OutputFormat::Json | OutputFormat::Yaml => {
                    output::print_output(&tokens, output_format)?;
                }
                OutputFormat::Table => {
                    let mut table = output::create_table();
                    output::add_header(
                        &mut table,
                        vec!["ID", "Label", "Token", "Active", "Expires", "Last Used"],
                    );
                    for token in tokens {
                        table.add_row(vec![
                            token.id.to_string(),
                            token.label,
                            format!("{}...{}", token.token_prefix, token.token_suffix),
                            token.active.to_string(),
                            token.expires_at.unwrap_or_else(|| "never".to_string()),
                            token.last_used_at.unwrap_or_else(|| "-".to_string()),
                        ]);
                    }
                    println!("{}", table);
                }
            }
        }
        IntegrationTokenCommands::Revoke {
            identity_id,
            token_id,
            reason,
        } => {
            let token: IntegrationToken = client
                .post(
                    &format!("/identities/{identity_id}/integration-tokens/{token_id}/revoke"),
                    &RevokeIntegrationTokenRequest { reason },
                )
                .await?;

            match output_format {
                OutputFormat::Json | OutputFormat::Yaml => {
                    output::print_output(&token, output_format)?;
                }
                OutputFormat::Table => {
                    output::print_success("Integration token revoked");
                    output::print_key_value_table(vec![
                        ("ID", token.id.to_string()),
                        ("Label", token.label),
                        (
                            "Revoked At",
                            token.revoked_at.unwrap_or_else(|| "-".to_string()),
                        ),
                    ]);
                }
            }
        }
        IntegrationTokenCommands::Delete {
            identity_id,
            token_id,
            yes,
        } => {
            if !yes
                && !dialoguer::Confirm::new()
                    .with_prompt(format!(
                        "Delete integration token metadata record {token_id}?"
                    ))
                    .default(false)
                    .interact()?
            {
                output::print_info("Delete cancelled");
                return Ok(());
            }

            let response: SuccessResponse = client
                .delete(&format!(
                    "/identities/{identity_id}/integration-tokens/{token_id}"
                ))
                .await?;

            match output_format {
                OutputFormat::Json | OutputFormat::Yaml => {
                    output::print_output(&response, output_format)?;
                }
                OutputFormat::Table => output::print_success(&response.message),
            }
        }
    }

    Ok(())
}

async fn handle_whoami(
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;

    if config.auth_token().ok().flatten().is_none() {
        anyhow::bail!("Not logged in. Use 'attune auth login' to authenticate.");
    }

    let mut client = ApiClient::from_config(&config, api_url);

    let identity: Identity = client.get("/auth/me").await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&identity, output_format)?;
        }
        OutputFormat::Table => {
            output::print_section("Current Identity");
            output::print_key_value_table(vec![
                ("Login", identity.login),
                (
                    "Display Name",
                    identity.display_name.unwrap_or_else(|| "-".to_string()),
                ),
                ("API Host", client.base_url().to_string()),
                ("ID", identity.id.to_string()),
            ]);
        }
    }

    Ok(())
}

async fn handle_refresh(
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;

    // Check if we have a refresh token
    let refresh_token = config
        .refresh_token()
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("No refresh token found. Please log in again."))?;

    let mut client = ApiClient::from_config(&config, api_url);

    #[derive(Serialize)]
    struct RefreshRequest {
        refresh_token: String,
    }

    // Attempt the refresh
    let refresh_result: Result<LoginResponse> = client
        .post("/auth/refresh", &RefreshRequest { refresh_token })
        .await;

    match refresh_result {
        Ok(response) => {
            // Save new tokens to config
            let mut config = CliConfig::load()?;
            config.set_auth(
                response.access_token.clone(),
                response.refresh_token.clone(),
            )?;

            match output_format {
                OutputFormat::Json | OutputFormat::Yaml => {
                    output::print_output(&response, output_format)?;
                }
                OutputFormat::Table => {
                    output::print_success("Token refreshed successfully");
                    output::print_info(&format!(
                        "New token expires in {} seconds",
                        response.expires_in
                    ));
                }
            }

            Ok(())
        }
        Err(_) => {
            // Refresh failed (likely expired) — re-initiate authentication
            let current_profile = config.current_profile()?;
            let auth_method = current_profile.auth_method.clone();
            let username = current_profile.username.clone();

            match auth_method.as_deref() {
                Some("sso") => {
                    output::print_warning(
                        "Session expired. Re-initiating SSO login in your browser...",
                    );
                    handle_sso_login(
                        profile.as_ref(),
                        api_url,
                        None,  // use the device-code lifetime
                        false, // no_browser: open browser
                        output_format,
                    )
                    .await
                }
                Some("direct") | Some("ldap") => {
                    let login_username = match &username {
                        Some(u) => u.clone(),
                        None => {
                            anyhow::bail!(
                                "Session expired and no stored username. Please log in again with: attune auth login -u <username>"
                            );
                        }
                    };
                    output::print_warning(&format!(
                        "Session expired. Re-authenticating as '{login_username}'..."
                    ));
                    handle_login(
                        login_username,
                        None, // prompt for password
                        profile.as_ref(),
                        api_url,
                        output_format,
                    )
                    .await
                }
                Some("token") => {
                    anyhow::bail!(
                        "Session expired. Integration token sessions cannot be refreshed automatically.\n\
                         Please log in again with: attune auth token-login"
                    );
                }
                _ => {
                    // Unknown or no stored auth method — fall back to generic error
                    anyhow::bail!(
                        "Session expired and could not be refreshed. Please log in again."
                    );
                }
            }
        }
    }
}
