//! Process boundaries verify CLI/MCP share the same persisted credential contract.
#![allow(deprecated)]
use serde_json::json;
use std::{
    io::Write,
    process::{Command, Stdio},
};
use wiremock::{
    matchers::{header, method, path as request_path},
    Mock, MockServer, ResponseTemplate,
};

fn command(bin: &str, home: &std::path::Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin(bin));
    cmd.env("XDG_CONFIG_HOME", home)
        .env_remove("ATTUNE_API_TOKEN")
        .env_remove("ATTUNE_AUTH_TOKEN")
        .env_remove("ATTUNE_REFRESH_TOKEN")
        .env_remove("ATTUNE_EXECUTION_TOKEN")
        .env_remove("ATTUNE_LOGIN")
        .env_remove("ATTUNE_PASSWORD")
        .env_remove("ATTUNE_PROFILE")
        .env_remove("ATTUNE_API_URL");
    cmd
}
fn run(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    let output = command("attune", home).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
fn setup(home: &std::path::Path, url: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(home.join("attune")).unwrap();
    let path = home.join("attune/config.yaml");
    std::fs::write(
        &path,
        format!("profile: default\nprofiles:\n  default:\n    api_url: {url}\n"),
    )
    .unwrap();
    path
}
async fn login_mock(server: &MockServer) {
    Mock::given(method("GET"))
        .and(request_path("/auth/settings"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data":{"local_password_enabled":true}})),
        )
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(request_path("/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{
            "access_token":"private-access", "refresh_token":"private-refresh", "expires_in":3600
        }})))
        .mount(server)
        .await;
}

#[tokio::test]
async fn encrypted_login_cli_refresh_mcp_use_and_explicit_optout() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let path = setup(home.path(), &server.uri());
    login_mock(&server).await;
    let output = run(
        home.path(),
        &[
            "--json",
            "auth",
            "login",
            "--username",
            "alice",
            "--password",
            "test-password",
        ],
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-access"));
    let disk = std::fs::read_to_string(&path).unwrap();
    assert!(!disk.contains("private-access"));
    assert!(!disk.contains("private-refresh"));
    assert!(disk.contains("ciphertext:"));
    for args in [
        vec!["--json", "config", "show-profile", "default"],
        vec!["--yaml", "config", "show-profile", "default"],
        vec!["config", "list"],
    ] {
        let output = run(home.path(), &args);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-access"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("ciphertext"));
    }
    Mock::given(method("GET"))
        .and(request_path("/auth/me"))
        .and(header("authorization", "Bearer private-access"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data":{"id":1,"login":"alice","display_name":null}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    run(home.path(), &["auth", "whoami"]);
    Mock::given(method("POST"))
        .and(request_path("/auth/refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data":{"access_token":"rotated-access","refresh_token":"rotated-refresh"}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    run(home.path(), &["auth", "refresh"]);
    let rotated = std::fs::read_to_string(&path).unwrap();
    assert!(!rotated.contains("rotated-access"));
    // MCP startup unlocks the refreshed CLI credentials without prompting.
    let mut child = command("attune-mcp", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("protocolVersion"));
    run(
        home.path(),
        &[
            "--profile",
            "default",
            "config",
            "set",
            "credential_encryption",
            "false",
        ],
    );
    let disk = std::fs::read_to_string(&path).unwrap();
    assert!(disk.contains("rotated-access"));
    assert!(!disk.contains("ciphertext"));
    let output = run(home.path(), &["config", "profiles"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("ENCRYPTION DISABLED"));
    run(
        home.path(),
        &["config", "set", "credential_encryption", "true"],
    );
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .contains("rotated-access"));
}

#[tokio::test]
async fn migration_endpoint_rejection_and_recovery_from_unreadable_credentials() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let path = setup(home.path(), &server.uri());
    let legacy = format!("profile: default\nprofiles:\n  default:\n    api_url: {}\n    auth_token: legacy-access\n    refresh_token: legacy-refresh\n",server.uri());
    std::fs::write(&path, legacy).unwrap();
    run(home.path(), &["config", "list"]);
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .contains("legacy-access"));
    let output = command("attune", home.path())
        .args(["auth", "whoami"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    login_mock(&server).await;
    run(
        home.path(),
        &[
            "auth",
            "login",
            "--username",
            "alice",
            "--password",
            "test-password",
        ],
    );
    let other = MockServer::start().await;
    let output = command("attune", home.path())
        .args(["--api-url", &other.uri(), "auth", "whoami"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(other.received_requests().await.unwrap().is_empty());
    // Metadata remains usable after ciphertext corruption, and new login recovers.
    let mut value: serde_json::Value =
        serde_yaml_ng::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    value["profiles"]["default"]["credentials"]["ciphertext"] = json!("invalid");
    std::fs::write(&path, serde_yaml_ng::to_string(&value).unwrap()).unwrap();
    run(home.path(), &["config", "profiles"]);
    let output = command("attune", home.path())
        .args(["auth", "whoami"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be unlocked"));
    run(
        home.path(),
        &[
            "auth",
            "login",
            "--username",
            "alice",
            "--password",
            "test-password",
        ],
    );
    run(home.path(), &["auth", "logout"]);
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .contains("ciphertext"));
}

#[tokio::test]
async fn runtime_credentials_refresh_without_touching_saved_tokens() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let path = setup(home.path(), &server.uri());
    login_mock(&server).await;
    run(
        home.path(),
        &[
            "auth",
            "login",
            "--username",
            "alice",
            "--password",
            "test-password",
        ],
    );
    let before = std::fs::read(&path).unwrap();
    Mock::given(method("POST"))
        .and(request_path("/auth/refresh"))
        .and(wiremock::matchers::body_json(
            json!({"refresh_token":"runtime-refresh"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data":{"access_token":"runtime-next","refresh_token":"runtime-next-refresh"}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let output = command("attune", home.path())
        .env("ATTUNE_API_TOKEN", "runtime-access")
        .env("ATTUNE_REFRESH_TOKEN", "runtime-refresh")
        .args(["auth", "refresh"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[tokio::test]
async fn named_profile_plaintext_optout_does_not_change_the_active_profile() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let path = setup(home.path(), &server.uri());
    run(
        home.path(),
        &[
            "config",
            "add-profile",
            "staging",
            "--api-url",
            &server.uri(),
        ],
    );
    run(
        home.path(),
        &[
            "--profile",
            "staging",
            "config",
            "set",
            "credential_encryption",
            "false",
        ],
    );
    login_mock(&server).await;
    run(
        home.path(),
        &[
            "--profile",
            "staging",
            "auth",
            "login",
            "--username",
            "alice",
            "--password",
            "test-password",
        ],
    );
    let value: serde_json::Value =
        serde_yaml_ng::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(value["profile"], "default");
    assert_eq!(value["profiles"]["default"]["credential_encryption"], true);
    assert_eq!(value["profiles"]["staging"]["credential_encryption"], false);
    assert_eq!(value["profiles"]["staging"]["auth_token"], "private-access");
    let output = run(
        home.path(),
        &["--json", "config", "show-profile", "staging"],
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-access"));
}

#[derive(Clone, Copy)]
enum LoginFlow {
    Password,
    Ldap,
    Integration,
    Sso,
}

async fn assert_login_rejects_concurrent_profile_change(flow: LoginFlow, delete: bool) {
    let authenticated_server = MockServer::start().await;
    let other_server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let config_path = setup(home.path(), &other_server.uri());
    run(
        home.path(),
        &[
            "config",
            "add-profile",
            "staging",
            "--api-url",
            &authenticated_server.uri(),
        ],
    );
    Mock::given(method("GET"))
        .and(request_path("/auth/settings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{
            "local_password_enabled": !matches!(flow, LoginFlow::Ldap), "ldap_enabled": true
        }})))
        .mount(&authenticated_server)
        .await;
    if matches!(flow, LoginFlow::Sso) {
        Mock::given(method("POST"))
            .and(request_path("/auth/oidc/device/start"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{
                "device_code":"owned-device-session", "user_code":"ABCD-1234",
                "verification_uri":"https://idp.example.com/device", "expires_in":30, "interval":1
            }})))
            .mount(&authenticated_server)
            .await;
    }
    let response_path = match flow {
        LoginFlow::Password => "/auth/login",
        LoginFlow::Ldap => "/auth/ldap/login",
        LoginFlow::Integration => "/auth/token-login",
        LoginFlow::Sso => "/auth/oidc/device/poll",
    };
    let owned_home = home.path().to_owned();
    let other_url = other_server.uri();
    Mock::given(method("POST")).and(request_path(response_path))
        .respond_with(move |_: &wiremock::Request| {
            // The authenticated request has arrived, but its tokens have not
            // been returned. Complete the competing edit before releasing them.
            if delete {
                run(&owned_home, &["config", "remove-profile", "staging"]);
            } else {
                run(&owned_home, &["--profile", "staging", "config", "set", "api_url", &other_url]);
            }
            let tokens = json!({"access_token":"server-a-access", "refresh_token":"server-a-refresh", "expires_in":3600});
            let data = if matches!(flow, LoginFlow::Sso) { json!({"status":"authorized", "tokens":tokens}) } else { tokens };
            ResponseTemplate::new(200).set_body_json(json!({"data":data}))
        }).expect(1).mount(&authenticated_server).await;
    let args = match flow {
        LoginFlow::Password | LoginFlow::Ldap => vec![
            "--profile",
            "staging",
            "auth",
            "login",
            "--username",
            "alice",
            "--password",
            "test-password",
        ],
        LoginFlow::Integration => vec![
            "--profile",
            "staging",
            "auth",
            "token-login",
            "--token",
            "integration-secret",
        ],
        LoginFlow::Sso => vec!["--profile", "staging", "auth", "sso-login", "--no-browser"],
    };
    let output = command("attune", home.path()).args(args).output().unwrap();
    assert!(
        !output.status.success(),
        "stale login unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Profile changed in another process"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_yaml_ng::from_str(&std::fs::read_to_string(config_path).unwrap()).unwrap();
    assert_eq!(value["profile"], "default");
    assert!(value["profiles"]["default"].get("credentials").is_none());
    if delete {
        assert!(
            value["profiles"].get("staging").is_none(),
            "login recreated the deleted profile"
        );
    } else {
        assert_eq!(value["profiles"]["staging"]["api_url"], other_server.uri());
        assert!(value["profiles"]["staging"].get("credentials").is_none());
        let whoami = command("attune", home.path())
            .args(["--profile", "staging", "auth", "whoami"])
            .output()
            .unwrap();
        assert!(!whoami.status.success());
    }
    assert!(
        other_server.received_requests().await.unwrap().is_empty(),
        "server A's credentials reached another server"
    );
}

#[tokio::test]
async fn login_rejects_concurrent_endpoint_changes_for_all_login_methods() {
    for flow in [
        LoginFlow::Password,
        LoginFlow::Ldap,
        LoginFlow::Integration,
        LoginFlow::Sso,
    ] {
        assert_login_rejects_concurrent_profile_change(flow, false).await;
    }
}

#[tokio::test]
async fn login_rejects_concurrent_profile_deletion_for_all_login_methods() {
    for flow in [
        LoginFlow::Password,
        LoginFlow::Ldap,
        LoginFlow::Integration,
        LoginFlow::Sso,
    ] {
        assert_login_rejects_concurrent_profile_change(flow, true).await;
    }
}
