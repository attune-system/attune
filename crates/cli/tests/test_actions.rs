//! Integration tests for CLI action commands
#![allow(deprecated)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use wiremock::{
    matchers::{method, path, query_param},
    Mock, ResponseTemplate,
};

mod common;
use common::*;

#[test]
fn test_bash_completion_script_includes_dynamic_entrypoint() {
    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.args(["completion", "bash"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("attune __complete"));
}

#[test]
fn test_fish_completion_script_includes_dynamic_entrypoint() {
    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.args(["completion", "fish"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("__attune_dynamic_complete"))
        .stdout(predicate::str::contains("attune __complete"))
        .stdout(predicate::str::contains(
            "complete -c attune -n '__attune_no_path_context' -f",
        ));
}

#[test]
fn test_zsh_completion_script_includes_dynamic_entrypoint() {
    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.args(["completion", "zsh"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("#compdef attune"))
        .stdout(predicate::str::contains("attune __complete"))
        .stdout(predicate::str::contains("_files"));
}

#[test]
fn test_powershell_completion_script_registers_dynamic_entrypoint() {
    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.args(["completion", "powershell"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains(
            "Register-ArgumentCompleter -Native -CommandName attune",
        ))
        .stdout(predicate::str::contains("attune __complete"))
        .stdout(predicate::str::contains("CompletionResult]::new"));
}

#[test]
fn test_completion_install_uses_xdg_paths_and_prints_zsh_setup() {
    let home = tempfile::tempdir().unwrap();
    let data_home = tempfile::tempdir().unwrap();
    let config_home = tempfile::tempdir().unwrap();

    let mut bash = Command::cargo_bin("attune").unwrap();
    bash.args(["completion", "install", "bash"])
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", data_home.path())
        .env("XDG_CONFIG_HOME", config_home.path());
    bash.assert().success();
    assert!(
        fs::read_to_string(data_home.path().join("bash-completion/completions/attune"))
            .unwrap()
            .contains("attune __complete")
    );

    let mut fish = Command::cargo_bin("attune").unwrap();
    fish.args(["completion", "install", "fish"])
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", data_home.path())
        .env("XDG_CONFIG_HOME", config_home.path());
    fish.assert().success();
    assert!(
        fs::read_to_string(config_home.path().join("fish/completions/attune.fish"))
            .unwrap()
            .contains("attune __complete")
    );

    let mut zsh = Command::cargo_bin("attune").unwrap();
    zsh.args(["completion", "install", "zsh"])
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", data_home.path())
        .env("XDG_CONFIG_HOME", config_home.path());
    zsh.assert()
        .success()
        .stdout(predicate::str::contains(
            "fpath=(~/.zsh/completions $fpath)",
        ))
        .stdout(predicate::str::contains(
            "autoload -Uz compinit && compinit",
        ));
    assert!(
        fs::read_to_string(home.path().join(".zsh/completions/_attune"))
            .unwrap()
            .contains("attune __complete")
    );
}

#[test]
fn test_completion_install_overwrites_regular_file() {
    let home = tempfile::tempdir().unwrap();
    let data_home = tempfile::tempdir().unwrap();
    let target = data_home.path().join("bash-completion/completions/attune");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "stale completion").unwrap();

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.args(["completion", "install", "bash"])
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", data_home.path());
    cmd.assert().success();
    assert!(fs::read_to_string(target)
        .unwrap()
        .contains("attune __complete"));
}

#[cfg(unix)]
#[test]
fn test_completion_install_rejects_symlink_target() {
    use std::os::unix::fs::symlink;

    let home = tempfile::tempdir().unwrap();
    let data_home = tempfile::tempdir().unwrap();
    let target = data_home.path().join("bash-completion/completions/attune");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    let destination = data_home.path().join("outside");
    symlink(&destination, &target).unwrap();

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.args(["completion", "install", "bash"])
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", data_home.path());
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("refusing to overwrite symlink"));
    assert!(!destination.exists());
}

#[test]
fn test_completion_suggests_execution_options() {
    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.args(["__complete", "--", "run", "core.echo", "--w"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--watch"))
        .stdout(predicate::str::contains("--worker-selector"));
}

#[tokio::test]
async fn test_completion_suggests_actions_and_schema_parameters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    Mock::given(method("GET"))
        .and(path("/api/v1/packs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "ref": "core" }],
            "total": 1,
            "page": 1,
            "page_size": 100
        })))
        .mount(&fixture.mock_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/actions/search"))
        .and(query_param("q", "core.e"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "ref": "core.echo" }],
            "total": 1,
            "page": 1,
            "page_size": 100
        })))
        .mount(&fixture.mock_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/actions/core.echo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "param_schema": {
                    "message": { "type": "string" },
                    "style": { "type": "string", "enum": ["plain", "json"] }
                }
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut pack_cmd = Command::cargo_bin("attune").unwrap();
    pack_cmd
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .args(["__complete", "--", "run"]);
    pack_cmd
        .assert()
        .success()
        .stdout(predicate::str::contains("core."));

    let mut action_cmd = Command::cargo_bin("attune").unwrap();
    action_cmd
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .args(["__complete", "--", "run", "core.e"]);
    action_cmd
        .assert()
        .success()
        .stdout(predicate::str::contains("core.echo"));

    let mut parameter_cmd = Command::cargo_bin("attune").unwrap();
    parameter_cmd
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .args(["__complete", "--", "run", "core.echo", "--param", ""]);
    parameter_cmd
        .assert()
        .success()
        .stdout(predicate::str::contains("message="))
        .stdout(predicate::str::contains("style="));

    let mut fish_parameter_cmd = Command::cargo_bin("attune").unwrap();
    fish_parameter_cmd
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .args(["__complete", "--", "run", "core.echo", "--param"]);
    fish_parameter_cmd
        .assert()
        .success()
        .stdout(predicate::str::contains("message="))
        .stdout(predicate::str::contains("style="));

    let mut enum_cmd = Command::cargo_bin("attune").unwrap();
    enum_cmd
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .args(["__complete", "--", "run", "core.echo", "--param", "style=j"]);
    enum_cmd
        .assert()
        .success()
        .stdout(predicate::str::contains("style=json"));
}

#[tokio::test]
async fn test_completion_uses_explicit_profile() {
    let fixture = TestFixture::new().await;
    fixture.write_config(&format!(
        r#"
profile: default
format: table
profiles:
  default:
    api_url: http://127.0.0.1:1
  my-custom-profile:
    api_url: {}
    auth_token: valid_token
    refresh_token: refresh_token
"#,
        fixture.server_url()
    ));

    Mock::given(method("GET"))
        .and(path("/api/v1/packs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "ref": "custom" }],
            "total": 1,
            "page": 1,
            "page_size": 100
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .args(["__complete", "--", "--profile", "my-custom-profile", "run"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("custom."));
}

#[tokio::test]
async fn test_completion_uses_profile_from_environment() {
    let fixture = TestFixture::new().await;
    fixture.write_config(&format!(
        r#"
profile: default
format: table
profiles:
  default:
    api_url: http://127.0.0.1:1
  staging:
    api_url: {}
    auth_token: valid_token
"#,
        fixture.server_url()
    ));
    Mock::given(method("GET"))
        .and(path("/api/v1/packs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "ref": "staging" }], "total": 1, "page": 1, "page_size": 100
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .env("ATTUNE_PROFILE", "staging")
        .args(["__complete", "--", "run"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("staging."));
}

#[test]
fn test_completion_does_not_create_default_config() {
    let config_dir = tempfile::TempDir::new().unwrap();
    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", config_dir.path())
        .env("HOME", config_dir.path())
        .args(["__complete", "--", "run"]);
    cmd.assert().success().stdout(predicate::str::is_empty());
    assert!(!config_dir.path().join("attune/config.yaml").exists());
}

#[tokio::test]
async fn test_completion_suggests_registered_profiles() {
    let fixture = TestFixture::new().await;
    fixture.write_config(
        r#"
profile: default
format: table
profiles:
  default:
    api_url: http://localhost:8080
  my-custom-profile:
    api_url: https://custom.example.com
  staging:
    api_url: https://staging.example.com
"#,
    );

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .args(["__complete", "--", "--profile", "my-"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("my-custom-profile"))
        .stdout(predicate::str::contains("staging").not());
}

#[tokio::test]
async fn test_action_list_authenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action list endpoint
    mock_action_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("core.echo"))
        .stdout(predicate::str::contains("Echo a message"));
}

#[tokio::test]
async fn test_action_list_unauthenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    // Mock unauthorized response
    mock_unauthorized(&fixture.mock_server, "/api/v1/actions").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("list");

    cmd.assert().failure();
}

#[tokio::test]
async fn test_action_list_json_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action list endpoint
    mock_action_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--json")
        .arg("action")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains(r#""ref""#))
        .stdout(predicate::str::contains(r#"core.echo"#));
}

#[tokio::test]
async fn test_action_list_yaml_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action list endpoint
    mock_action_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--yaml")
        .arg("action")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("core.echo"))
        .stdout(predicate::str::contains("Echo a message"));
}

#[tokio::test]
async fn test_action_get_by_ref() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action get endpoint
    Mock::given(method("GET"))
        .and(path("/api/v1/actions/core.echo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 1,
                "ref": "core.echo",
                "pack": 1,
                "pack_ref": "core",
                "label": "Echo Action",
                "description": "Echo a message",
                "entrypoint": "echo.py",
                "runtime": null,
                "param_schema": {
                    "message": {
                        "type": "string",
                        "description": "Message to echo",
                        "required": true
                    }
                },
                "out_schema": null,
                "created": "2024-01-01T00:00:00Z",
                "updated": "2024-01-01T00:00:00Z"
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("show")
        .arg("core.echo");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("core.echo"))
        .stdout(predicate::str::contains("Echo a message"));
}

#[tokio::test]
async fn test_action_get_not_found() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock 404 response
    mock_not_found(&fixture.mock_server, "/api/v1/actions/nonexistent.action").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("show")
        .arg("nonexistent.action");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Error"));
}

#[tokio::test]
async fn test_action_execute_with_parameters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 42).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.echo")
        .arg("--param")
        .arg("message=Hello World");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("42").or(predicate::str::contains("scheduled")));
}

#[tokio::test]
async fn test_action_execute_multiple_parameters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 100).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("linux.run_command")
        .arg("--param")
        .arg("cmd=ls -la")
        .arg("--param")
        .arg("timeout=30");

    cmd.assert().success();
}

#[tokio::test]
async fn test_action_execute_with_json_parameters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 101).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.webhook")
        .arg("--params-json")
        .arg(r#"{"url": "https://example.com", "method": "POST"}"#);

    cmd.assert().success();
}

#[tokio::test]
async fn test_action_execute_without_parameters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 200).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.no_params_action");

    cmd.assert().success();
}

#[tokio::test]
async fn test_action_execute_json_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 150).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--json")
        .arg("action")
        .arg("execute")
        .arg("core.echo")
        .arg("--param")
        .arg("message=test");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("150"))
        .stdout(predicate::str::contains("scheduled"));
}

#[tokio::test]
async fn test_action_execute_wait_for_completion() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 250).await;

    // Mock execution polling - first running, then succeeded
    Mock::given(method("GET"))
        .and(path("/api/v1/executions/250"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 250,
                "action": 1,
                "action_ref": "core.echo",
                "config": {"message": "test"},
                "parent": null,
                "enforcement": null,
                "executor": null,
                "status": "succeeded",
                "result": {"output": "test"},
                "created": "2024-01-01T00:00:00Z",
                "updated": "2024-01-01T00:00:00Z"
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.echo")
        .arg("--param")
        .arg("message=test")
        .arg("--watch");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("succeeded"));
}

#[tokio::test]
#[ignore = "Profile switching needs more investigation - CLI integration issue"]
async fn test_action_execute_with_profile() {
    let fixture = TestFixture::new().await;

    // Create multi-profile config
    let config = format!(
        r#"
current_profile: default
default_output_format: table
profiles:
  default:
    api_url: {}
    auth_token: default_token
    refresh_token: default_refresh
  production:
    api_url: {}
    auth_token: prod_token
    refresh_token: prod_refresh
"#,
        fixture.server_url(),
        fixture.server_url()
    );
    fixture.write_config(&config);

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 300).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--profile")
        .arg("production")
        .arg("action")
        .arg("execute")
        .arg("core.echo")
        .arg("--param")
        .arg("message=prod_test");

    cmd.assert().success();
}

#[tokio::test]
async fn test_action_execute_invalid_param_format() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.echo")
        .arg("--param")
        .arg("invalid_format_no_equals");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Error").or(predicate::str::contains("=")));
}

#[tokio::test]
async fn test_action_execute_invalid_json_parameters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.echo")
        .arg("--params-json")
        .arg(r#"{"invalid json"#);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Error").or(predicate::str::contains("JSON")));
}

#[tokio::test]
async fn test_action_list_by_pack() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action list for a specific pack
    Mock::given(method("GET"))
        .and(path("/api/v1/packs/core/actions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "ref": "core.echo",
                    "pack_ref": "core",
                    "label": "Echo Action",
                    "description": "Echo a message",
                    "entrypoint": "echo.py",
                    "runtime": null,
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
                "total_items": 1,
                "total_pages": 1,
                "has_previous": false,
                "has_next": false
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("list")
        .arg("--pack")
        .arg("core");

    cmd.assert().success();
}

#[tokio::test]
async fn test_action_execute_async_flag() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 400).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.long_running");
    // Note: default behavior is async (no --watch), so no --async flag needed

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("scheduled").or(predicate::str::contains("400")));
}

#[tokio::test]
async fn test_action_list_empty_result() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock empty action list
    Mock::given(method("GET"))
        .and(path("/api/v1/actions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "pagination": {
                "page": 1,
                "page_size": 50,
                "total_items": 0,
                "total_pages": 0,
                "has_previous": false,
                "has_next": false
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("list");

    cmd.assert().success();
}

#[tokio::test]
async fn test_action_get_shows_parameters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action get with detailed parameters
    Mock::given(method("GET"))
        .and(path("/api/v1/actions/core.complex"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 5,
                "ref": "core.complex",
                "pack": 1,
                "pack_ref": "core",
                "label": "Complex Action",
                "description": "Complex action with multiple params",
                "entrypoint": "complex.py",
                "runtime": null,
                "param_schema": {
                    "required_string": {
                        "type": "string",
                        "description": "A required string parameter",
                        "required": true
                    },
                    "optional_number": {
                        "type": "integer",
                        "description": "An optional number",
                        "required": false,
                        "default": 42
                    },
                    "boolean_flag": {
                        "type": "boolean",
                        "description": "A boolean flag",
                        "required": false,
                        "default": false
                    }
                },
                "out_schema": null,
                "created": "2024-01-01T00:00:00Z",
                "updated": "2024-01-01T00:00:00Z"
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("show")
        .arg("core.complex");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("required_string"))
        .stdout(predicate::str::contains("optional_number"));
}
