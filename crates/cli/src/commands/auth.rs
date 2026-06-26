use anyhow::Result;
use axum::{extract::State, response::Html, routing::post, Form, Router};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};
use urlencoding;

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
    /// Log in using SSO (OIDC) — opens a browser window for authentication
    SsoLogin {
        /// API URL to log in to (saved into the profile for future use)
        #[arg(long)]
        url: Option<String>,

        /// Save credentials into a named profile (creates it if it doesn't exist)
        #[arg(long)]
        save_profile: Option<String>,

        /// Local port for the OAuth callback server (default: random available port)
        #[arg(long)]
        port: Option<u16>,

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
            port,
            no_browser,
        } => {
            let effective_api_url = url.or_else(|| api_url.clone());
            handle_sso_login(
                save_profile.as_ref().or(profile.as_ref()),
                &effective_api_url,
                port,
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
    port: Option<u16>,
    no_browser: bool,
    output_format: OutputFormat,
) -> Result<()> {
    let mut config = CliConfig::load()?;
    let target_profile_name = profile
        .cloned()
        .unwrap_or_else(|| config.current_profile.clone());

    // Resolve / create the target profile so we know the base API URL.
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

    let config = CliConfig::load()?;
    let base_api_url = api_url.clone().unwrap_or_else(|| {
        config
            .profiles
            .get(&target_profile_name)
            .map(|profile| profile.api_url.clone())
            .unwrap_or_else(|| config.effective_api_url(&None))
    });

    // Bind the local callback server to a random (or explicit) port.
    let listener = {
        let addr = format!(
            "127.0.0.1:{}",
            port.unwrap_or(0) // 0 → OS picks a free port
        );
        tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to bind local callback server: {e}"))?
    };
    let local_port = listener.local_addr()?.port();
    let callback_uri = format!("http://localhost:{local_port}/callback");

    // Channel: the callback route sends the received tokens back to this task.
    let (tx, rx) = oneshot::channel::<SsoCallbackTokens>();
    let tx = Arc::new(Mutex::new(Some(tx)));

    // Build a minimal Axum router for the local callback server.
    // Accepts POST from the API's auto-submitting form (tokens in body, not URL).
    let app = Router::new()
        .route("/callback", post(sso_callback_handler))
        .with_state(tx.clone());

    let server = axum::serve(listener, app);
    // Wrap in a cancellable future so we can shut the server down after receiving tokens.
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server_handle = tokio::spawn(async move {
        let _ = server
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    // Build the login URL pointing at the API's OIDC entry point.
    let login_url = format!(
        "{}/auth/oidc/login?cli_redirect_uri={}",
        base_api_url.trim_end_matches('/'),
        urlencoding::encode(&callback_uri),
    );

    if no_browser {
        output::print_info("Open the following URL in your browser to complete SSO login:");
        println!("{login_url}");
    } else {
        output::print_info("Opening browser for SSO login...");
        if let Err(e) = open_browser(&login_url) {
            output::print_warning(&format!(
                "Could not open browser automatically: {e}\nOpen this URL manually: {login_url}"
            ));
        }
    }
    output::print_info("Waiting for authentication (press Ctrl+C to cancel)...");

    // Wait for the callback with a 5-minute timeout.
    let tokens = tokio::time::timeout(std::time::Duration::from_secs(300), rx)
        .await
        .map_err(|_| anyhow::anyhow!("SSO login timed out after 5 minutes"))?
        .map_err(|_| anyhow::anyhow!("SSO callback server shut down unexpectedly"))?;

    // Shut down the local server gracefully (allow response to be sent to browser).
    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), server_handle).await;

    // Persist tokens.
    let mut config = CliConfig::load()?;
    if let Some(p) = config.profiles.get_mut(&target_profile_name) {
        p.auth_token = Some(tokens.access_token.clone());
        p.refresh_token = Some(tokens.refresh_token.clone());
        p.auth_method = Some("sso".to_string());
        p.username = None;
        config.save()?;
    } else {
        config.set_auth(tokens.access_token.clone(), tokens.refresh_token.clone())?;
    }

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

#[derive(Debug, Clone, Deserialize)]
struct SsoCallbackTokens {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    expires_in: i64,
}

type SsoCallbackState = Arc<Mutex<Option<oneshot::Sender<SsoCallbackTokens>>>>;

async fn sso_callback_handler(
    State(tx): State<SsoCallbackState>,
    Form(params): Form<SsoCallbackTokens>,
) -> Html<String> {
    // Forward tokens to the waiting handle_sso_login call.
    if let Some(sender) = tx.lock().await.take() {
        let _ = sender.send(params);
    }

    Html(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Attune SSO Login</title>
  <style>
    body { font-family: system-ui, sans-serif; max-width: 480px; margin: 80px auto; text-align: center; color: #222; }
    h1 { color: #16a34a; }
    p  { color: #555; }
  </style>
</head>
<body>
  <h1>Login successful!</h1>
  <p>You are now authenticated with Attune. You can close this tab.</p>
  <script>setTimeout(() => window.close(), 2000);</script>
</body>
</html>"#
        .to_string(),
    )
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
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
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
                        None,  // port: random
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
