//! Integration tests for CLI authentication commands

#![allow(deprecated)]

use assert_cmd::Command;
use attune_cli::config::CliConfig;
use predicates::prelude::*;
use serde_json::json;
use wiremock::{
    matchers::{body_json, method, path},
    Mock, ResponseTemplate,
};

mod common;
use common::*;

async fn mock_device_login(
    fixture: &TestFixture,
    access: &str,
    refresh: &str,
    poll_delay: std::time::Duration,
) {
    Mock::given(method("POST"))
        .and(path("/auth/oidc/device/start"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{
            "device_code":"opaque-device-session", "user_code":"ABCD-1234",
            "verification_uri":"https://idp.example.com/device", "expires_in":30, "interval":1
        }})))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/auth/oidc/device/poll"))
        .and(body_json(json!({"device_code":"opaque-device-session"})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(poll_delay)
                .set_body_json(json!({"data":{"status":"authorized", "tokens":{
                    "access_token":access, "refresh_token":refresh, "expires_in":3600
                }}})),
        )
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
}

fn load_test_config(fixture: &TestFixture) -> CliConfig {
    let config_content =
        std::fs::read_to_string(&fixture.config_path).expect("Failed to read config");
    serde_yaml_ng::from_str(&config_content).expect("Failed to parse CLI config")
}

#[tokio::test]
async fn device_denial_leaves_existing_credentials_and_profile_url_untouched() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("existing-access", "existing-refresh");
    let before = std::fs::read_to_string(&fixture.config_path).unwrap();
    Mock::given(method("POST")).and(path("/auth/oidc/device/start"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{
            "device_code":"secret-session", "user_code":"ABCD-1234", "verification_uri":"https://idp.example.com/device", "expires_in":30, "interval":1
        }}))).expect(1).mount(&fixture.mock_server).await;
    Mock::given(method("POST"))
        .and(path("/auth/oidc/device/poll"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"status":"access_denied"}})),
        )
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    Command::cargo_bin("attune")
        .unwrap()
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .args([
            "--api-url",
            &fixture.server_url(),
            "auth",
            "sso-login",
            "--no-browser",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("authorization was denied"));
    assert_eq!(
        std::fs::read_to_string(&fixture.config_path).unwrap(),
        before
    );
    for request in fixture.mock_server.received_requests().await.unwrap() {
        assert!(!request.headers.contains_key("authorization"));
        assert_ne!(request.url.path(), "/auth/refresh");
    }
}

#[tokio::test]
async fn device_login_timeout_is_bounded_and_does_not_save_tokens() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();
    let before = std::fs::read_to_string(&fixture.config_path).unwrap();
    Mock::given(method("POST")).and(path("/auth/oidc/device/start"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{
            "device_code":"secret-session", "user_code":"ABCD-1234", "verification_uri":"https://idp.example.com/device", "expires_in":30, "interval":5
        }}))).expect(1).mount(&fixture.mock_server).await;
    Command::cargo_bin("attune")
        .unwrap()
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .args([
            "--api-url",
            &fixture.server_url(),
            "auth",
            "sso-login",
            "--no-browser",
            "--timeout",
            "1",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("timed out"));
    assert_eq!(
        std::fs::read_to_string(&fixture.config_path).unwrap(),
        before
    );
    assert_eq!(
        fixture.mock_server.received_requests().await.unwrap().len(),
        1
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn device_login_opens_no_listening_socket() {
    use tokio::io::AsyncBufReadExt;
    let fixture = TestFixture::new().await;
    fixture.write_default_config();
    Mock::given(method("POST")).and(path("/auth/oidc/device/start"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{
            "device_code":"secret-session", "user_code":"ABCD-1234", "verification_uri":"https://idp.example.com/device", "expires_in":30, "interval":5
        }}))).mount(&fixture.mock_server).await;
    let mut child = tokio::process::Command::new(assert_cmd::cargo::cargo_bin("attune"))
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .args([
            "--api-url",
            &fixture.server_url(),
            "auth",
            "sso-login",
            "--no-browser",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let ready = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(line) = lines.next_line().await.unwrap() {
            if line.contains("Waiting for approval") {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    let owns_listener = if ready {
        let fds = std::fs::read_dir(format!("/proc/{pid}/fd"))
            .unwrap()
            .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        ["tcp", "tcp6"].iter().any(|protocol| {
            std::fs::read_to_string(format!("/proc/{pid}/net/{protocol}"))
                .unwrap()
                .lines()
                .skip(1)
                .any(|line| {
                    let fields = line.split_whitespace().collect::<Vec<_>>();
                    fields.get(3) == Some(&"0A")
                        && fields
                            .get(9)
                            .is_some_and(|inode| fds.contains(&format!("socket:[{inode}]")))
                })
        })
    } else {
        false
    };
    child.start_kill().unwrap();
    child.wait().await.unwrap();
    assert!(ready, "CLI never reached device polling");
    assert!(
        !owns_listener,
        "SSO device login opened an inbound listening socket"
    );
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

    mock_device_login(
        &fixture,
        "sso_access_token",
        "sso_refresh_token",
        std::time::Duration::ZERO,
    )
    .await;
    Command::cargo_bin("attune")
        .unwrap()
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .args([
            "--api-url",
            &fixture.server_url(),
            "auth",
            "sso-login",
            "--no-browser",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("ABCD-1234"))
        .stderr(predicate::str::contains("opaque-device-session").not());

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

    mock_device_login(
        &fixture,
        "staging_sso_access_token",
        "staging_sso_refresh_token",
        std::time::Duration::ZERO,
    )
    .await;
    Command::cargo_bin("attune")
        .unwrap()
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .args(["--profile", "staging", "auth", "sso-login", "--no-browser"])
        .assert()
        .success();

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
    assert_eq!(config.current_profile, "default");
    for request in fixture.mock_server.received_requests().await.unwrap() {
        assert!(!request.headers.contains_key("authorization"));
    }
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
async fn device_login_waits_for_slow_approved_poll_without_redeeming_twice() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();
    mock_device_login(
        &fixture,
        "slow-access",
        "slow-refresh",
        std::time::Duration::from_secs(16),
    )
    .await;
    Command::cargo_bin("attune")
        .unwrap()
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .args([
            "--api-url",
            &fixture.server_url(),
            "auth",
            "sso-login",
            "--no-browser",
        ])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success();
    let config = load_test_config(&fixture);
    assert_eq!(
        config.profiles["default"].auth_token.as_deref(),
        Some("slow-access")
    );
    let polls = fixture
        .mock_server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == "/auth/oidc/device/poll")
        .count();
    assert_eq!(polls, 1);
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
