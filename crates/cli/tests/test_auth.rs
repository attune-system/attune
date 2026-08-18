//! Integration tests for CLI authentication commands

#![allow(deprecated)]

use assert_cmd::Command;
use attune_cli::config::CliConfig;
use predicates::prelude::*;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command as TokioCommand};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use url::Url;

mod common;
use common::*;

struct SsoLoginProcess {
    child: Child,
    stdout_reader: JoinHandle<()>,
    stderr_reader: JoinHandle<std::io::Result<Vec<u8>>>,
}

async fn join_reader<T>(mut reader: JoinHandle<T>) -> Option<T> {
    match tokio::time::timeout(Duration::from_secs(2), &mut reader).await {
        Ok(Ok(output)) => Some(output),
        Ok(Err(_)) => None,
        Err(_) => {
            reader.abort();
            let _ = reader.await;
            None
        }
    }
}

async fn terminate_sso_process(process: &mut SsoLoginProcess) {
    let _ = process.child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), process.child.wait()).await;
}

async fn clean_up_sso_process(mut process: SsoLoginProcess) {
    terminate_sso_process(&mut process).await;
    let _ = join_reader(process.stdout_reader).await;
    let _ = join_reader(process.stderr_reader).await;
}

async fn spawn_sso_login_and_read_url(
    fixture: &TestFixture,
    args: &[&str],
) -> anyhow::Result<(SsoLoginProcess, Url)> {
    let mut child = TokioCommand::new(assert_cmd::cargo::cargo_bin("attune"))
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture CLI stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture CLI stderr"))?;
    let mut lines = BufReader::new(stdout).lines();
    let (url_tx, url_rx) = oneshot::channel();

    let stdout_reader = tokio::spawn(async move {
        let mut url_tx = Some(url_tx);
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                if let Some(sender) = url_tx.take() {
                    let _ = sender.send(trimmed.to_string());
                }
            }
        }
    });
    let stderr_reader = tokio::spawn(async move {
        let mut output = Vec::new();
        stderr.read_to_end(&mut output).await?;
        Ok(output)
    });
    let process = SsoLoginProcess {
        child,
        stdout_reader,
        stderr_reader,
    };

    let login_url = match tokio::time::timeout(Duration::from_secs(10), url_rx).await {
        Ok(Ok(login_url)) => login_url,
        Ok(Err(_)) => {
            clean_up_sso_process(process).await;
            return Err(anyhow::anyhow!(
                "CLI exited before printing an SSO login URL"
            ));
        }
        Err(error) => {
            clean_up_sso_process(process).await;
            return Err(error.into());
        }
    };
    let login_url = match Url::parse(&login_url) {
        Ok(login_url) => login_url,
        Err(error) => {
            clean_up_sso_process(process).await;
            return Err(error.into());
        }
    };

    Ok((process, login_url))
}

fn cli_redirect_uri(login_url: &Url) -> String {
    login_url
        .query_pairs()
        .find_map(|(key, value)| {
            if key == "cli_redirect_uri" {
                Some(value.into_owned())
            } else {
                None
            }
        })
        .expect("SSO login URL should include cli_redirect_uri")
}

async fn post_sso_callback(callback_uri: &str, access_token: &str, refresh_token: &str) {
    reqwest::Client::new()
        .post(callback_uri)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(format!(
            "access_token={}&refresh_token={}&expires_in=3600",
            urlencoding::encode(access_token),
            urlencoding::encode(refresh_token)
        ))
        .send()
        .await
        .expect("Failed to POST SSO callback")
        .error_for_status()
        .expect("SSO callback returned an error");
}

async fn wait_for_sso_child(mut process: SsoLoginProcess) {
    let status = match tokio::time::timeout(Duration::from_secs(10), process.child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            terminate_sso_process(&mut process).await;
            let _ = join_reader(process.stdout_reader).await;
            let _ = join_reader(process.stderr_reader).await;
            panic!("Failed to wait for SSO CLI process: {error}");
        }
        Err(_) => {
            terminate_sso_process(&mut process).await;
            let _ = join_reader(process.stdout_reader).await;
            let _ = join_reader(process.stderr_reader).await;
            panic!("SSO CLI process did not exit");
        }
    };
    let _ = join_reader(process.stdout_reader).await;
    let stderr = join_reader(process.stderr_reader)
        .await
        .and_then(Result::ok)
        .unwrap_or_default();

    assert!(
        status.success(),
        "SSO CLI failed with stderr:\n{}",
        String::from_utf8_lossy(&stderr)
    );
}

fn load_test_config(fixture: &TestFixture) -> CliConfig {
    let config_content =
        std::fs::read_to_string(&fixture.config_path).expect("Failed to read config");
    serde_yaml_ng::from_str(&config_content).expect("Failed to parse CLI config")
}

#[tokio::test]
async fn test_login_success() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    // Mock successful login
    mock_login_success(
        &fixture.mock_server,
        "test_access_token",
        "test_refresh_token",
    )
    .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("auth")
        .arg("login")
        .arg("--username")
        .arg("testuser")
        .arg("--password")
        .arg("testpass");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Successfully logged in"));

    // Verify tokens were saved to config
    let config_content =
        std::fs::read_to_string(&fixture.config_path).expect("Failed to read config");
    assert!(config_content.contains("test_access_token"));
    assert!(config_content.contains("test_refresh_token"));
}

#[tokio::test]
async fn test_login_failure() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    // Mock failed login
    mock_login_failure(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("auth")
        .arg("login")
        .arg("--username")
        .arg("baduser")
        .arg("--password")
        .arg("badpass");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Error"));
}

#[tokio::test]
async fn test_sso_login_no_browser_saves_tokens() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    let (child, login_url) = spawn_sso_login_and_read_url(
        &fixture,
        &[
            "--api-url",
            &fixture.server_url(),
            "auth",
            "sso-login",
            "--no-browser",
        ],
    )
    .await
    .unwrap();

    let callback_uri = cli_redirect_uri(&login_url);
    post_sso_callback(&callback_uri, "sso_access_token", "sso_refresh_token").await;
    wait_for_sso_child(child).await;

    assert_eq!(
        format!(
            "{}://{}{}",
            login_url.scheme(),
            login_url.host_str().unwrap(),
            login_url
                .port()
                .map(|port| format!(":{port}"))
                .unwrap_or_default()
        ),
        fixture.server_url()
    );
    assert_eq!(login_url.path(), "/auth/oidc/login");
    assert!(callback_uri.starts_with("http://localhost:"));
    assert!(callback_uri.ends_with("/callback"));

    let config = load_test_config(&fixture);
    let profile = config.profiles.get("default").unwrap();
    assert_eq!(profile.auth_token.as_deref(), Some("sso_access_token"));
    assert_eq!(profile.refresh_token.as_deref(), Some("sso_refresh_token"));
    assert_eq!(profile.auth_method.as_deref(), Some("sso"));
    assert!(profile.username.is_none());
}

#[tokio::test]
async fn test_sso_login_uses_selected_profile_url() {
    let fixture = TestFixture::new().await;
    fixture.write_config(&format!(
        r#"
current_profile: default
default_output_format: table
profiles:
  default:
    api_url: http://127.0.0.1:9
    description: Default profile should not be used
  staging:
    api_url: {}
    description: Staging test server
"#,
        fixture.server_url()
    ));

    let (child, login_url) = spawn_sso_login_and_read_url(
        &fixture,
        &["--profile", "staging", "auth", "sso-login", "--no-browser"],
    )
    .await
    .unwrap();

    let callback_uri = cli_redirect_uri(&login_url);
    post_sso_callback(
        &callback_uri,
        "staging_sso_access_token",
        "staging_sso_refresh_token",
    )
    .await;
    wait_for_sso_child(child).await;

    assert!(login_url
        .as_str()
        .starts_with(&format!("{}/auth/oidc/login?", fixture.server_url())));
    assert!(!login_url.as_str().starts_with("http://127.0.0.1:9/"));

    let config = load_test_config(&fixture);
    let staging = config.profiles.get("staging").unwrap();
    let default = config.profiles.get("default").unwrap();
    assert_eq!(
        staging.auth_token.as_deref(),
        Some("staging_sso_access_token")
    );
    assert_eq!(
        staging.refresh_token.as_deref(),
        Some("staging_sso_refresh_token")
    );
    assert_eq!(staging.auth_method.as_deref(), Some("sso"));
    assert!(default.auth_token.is_none());
    assert!(default.refresh_token.is_none());
}

#[tokio::test]
async fn test_whoami_authenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock whoami endpoint
    mock_whoami_success(&fixture.mock_server, "testuser", "Test User").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("auth")
        .arg("whoami");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("testuser"))
        .stdout(predicate::str::contains("Test User"))
        .stdout(predicate::str::contains("API Host"))
        .stdout(predicate::str::contains(fixture.server_url()));
}

#[tokio::test]
async fn test_whoami_unauthenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    // Mock unauthorized response
    mock_unauthorized(&fixture.mock_server, "/auth/me").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("auth")
        .arg("whoami");

    cmd.assert().failure();
}

#[tokio::test]
async fn test_logout() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Verify tokens exist before logout
    let config_before =
        std::fs::read_to_string(&fixture.config_path).expect("Failed to read config");
    assert!(config_before.contains("valid_token"));

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("auth")
        .arg("logout");

    cmd.assert().success().stdout(
        predicate::str::contains("logged out")
            .or(predicate::str::contains("Successfully logged out")),
    );

    // Verify tokens were removed from config
    let config_after =
        std::fs::read_to_string(&fixture.config_path).expect("Failed to read config");
    assert!(!config_after.contains("valid_token"));
}

#[tokio::test]
async fn test_login_with_profile_override() {
    let fixture = TestFixture::new().await;
    fixture.write_multi_profile_config();

    // Mock successful login
    mock_login_success(&fixture.mock_server, "staging_token", "staging_refresh").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--profile")
        .arg("default")
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("auth")
        .arg("login")
        .arg("--username")
        .arg("testuser")
        .arg("--password")
        .arg("testpass");

    cmd.assert().success();
}

#[tokio::test]
async fn test_login_missing_username() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .arg("auth")
        .arg("login")
        .arg("--password")
        .arg("testpass");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("required"));
}

#[tokio::test]
async fn test_whoami_json_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock whoami endpoint
    mock_whoami_success(&fixture.mock_server, "testuser", "Test User").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--json")
        .arg("auth")
        .arg("whoami");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains(r#""login":"#))
        .stdout(predicate::str::contains("testuser"));
}

#[tokio::test]
async fn test_whoami_yaml_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock whoami endpoint
    mock_whoami_success(&fixture.mock_server, "testuser", "Test User").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--yaml")
        .arg("auth")
        .arg("whoami");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("login:"))
        .stdout(predicate::str::contains("testuser"));
}
