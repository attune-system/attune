use serde_json::json;
use std::path::PathBuf;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Test fixture for CLI integration tests
pub struct TestFixture {
    pub mock_server: MockServer,
    pub config_dir: TempDir,
    pub config_path: PathBuf,
}

impl TestFixture {
    /// Create a new test fixture with a mock API server
    pub async fn new() -> Self {
        let mock_server = MockServer::start().await;
        let config_dir = TempDir::new().expect("Failed to create temp dir");

        // Create attune subdirectory to match actual config path structure
        let attune_dir = config_dir.path().join("attune");
        std::fs::create_dir_all(&attune_dir).expect("Failed to create attune config dir");
        let config_path = attune_dir.join("config.yaml");

        Self {
            mock_server,
            config_dir,
            config_path,
        }
    }

    /// Get the mock server URI
    pub fn server_url(&self) -> String {
        self.mock_server.uri()
    }

    /// Get the config directory path
    pub fn config_dir_path(&self) -> &std::path::Path {
        self.config_dir.path()
    }

    /// Write a test config file with the mock server URL
    pub fn write_config(&self, content: &str) {
        std::fs::write(&self.config_path, content).expect("Failed to write config");
    }

    /// Write a default config with the mock server
    pub fn write_default_config(&self) {
        let config = format!(
            r#"
current_profile: default
default_output_format: table
profiles:
  default:
    api_url: {}
    description: Test server
"#,
            self.server_url()
        );
        self.write_config(&config);
    }

    /// Write a config with authentication tokens
    pub fn write_authenticated_config(&self, access_token: &str, refresh_token: &str) {
        let config = format!(
            r#"
current_profile: default
default_output_format: table
profiles:
  default:
    api_url: {}
    auth_token: {}
    refresh_token: {}
    description: Test server
"#,
            self.server_url(),
            access_token,
            refresh_token
        );
        self.write_config(&config);
    }

    /// Write a config with multiple profiles
    #[allow(dead_code)]
    pub fn write_multi_profile_config(&self) {
        let config = format!(
            r#"
current_profile: default
default_output_format: table
profiles:
  default:
    api_url: {}
    description: Default test server
  staging:
    api_url: https://staging.example.com
    description: Staging environment
  production:
    api_url: https://api.example.com
    description: Production environment
    output_format: json
"#,
            self.server_url()
        );
        self.write_config(&config);
    }
}

/// Mock a successful login response
#[allow(dead_code)]
pub async fn mock_login_success(server: &MockServer, access_token: &str, refresh_token: &str) {
    Mock::given(method("POST"))
        .and(path("/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "access_token": access_token,
                "refresh_token": refresh_token,
                "expires_in": 3600
            }
        })))
        .mount(server)
        .await;
}

/// Mock a failed login response
#[allow(dead_code)]
pub async fn mock_login_failure(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/auth/login"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "Invalid credentials"
        })))
        .mount(server)
        .await;
}

/// Mock a whoami response
#[allow(dead_code)]
pub async fn mock_whoami_success(server: &MockServer, username: &str, display_name: &str) {
    Mock::given(method("GET"))
        .and(path("/auth/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 1,
                "login": username,
                "display_name": display_name
            }
        })))
        .mount(server)
        .await;
}

/// Mock an unauthorized response
#[allow(dead_code)]
pub async fn mock_unauthorized(server: &MockServer, path_pattern: &str) {
    Mock::given(method("GET"))
        .and(path(path_pattern))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "Unauthorized"
        })))
        .mount(server)
        .await;
}

/// Mock a pack list response
#[allow(dead_code)]
pub async fn mock_pack_list(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/packs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "ref": "core",
                    "label": "Core Pack",
                    "description": "Core pack",
                    "version": "1.0.0",
                    "author": "Attune",
                    "enabled": true,
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                },
                {
                    "id": 2,
                    "ref": "linux",
                    "label": "Linux Pack",
                    "description": "Linux automation pack",
                    "version": "1.0.0",
                    "author": "Attune",
                    "enabled": true,
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
                "has_next": false,
                "has_previous": false,
                "total_items": 2,
                "total_pages": 1
            }
        })))
        .mount(server)
        .await;
}

/// Mock a pack get response
#[allow(dead_code)]
pub async fn mock_pack_get(server: &MockServer, pack_ref: &str) {
    let path_pattern = format!("/api/v1/packs/{}", pack_ref);
    // Capitalize first letter for label
    let label = pack_ref
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if i == 0 {
                c.to_uppercase().next().unwrap()
            } else {
                c
            }
        })
        .collect::<String>();
    Mock::given(method("GET"))
        .and(path(path_pattern.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 1,
                "ref": pack_ref,
                "label": format!("{} Pack", label),
                "description": format!("{} pack", pack_ref),
                "version": "1.0.0",
                "author": "Attune",
                "enabled": true,
                "created": "2024-01-01T00:00:00Z",
                "updated": "2024-01-01T00:00:00Z"
            }
        })))
        .mount(server)
        .await;
}

/// Mock an action list response
#[allow(dead_code)]
pub async fn mock_action_list(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/actions"))
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
        .mount(server)
        .await;
}

/// Mock an action execution response
#[allow(dead_code)]
pub async fn mock_action_execute(server: &MockServer, execution_id: i64) {
    Mock::given(method("POST"))
        .and(path("/api/v1/executions/execute"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "data": {
                "id": execution_id,
                "action": 1,
                "action_ref": "core.echo",
                "config": {},
                "parent": null,
                "enforcement": null,
                "executor": null,
                "status": "scheduled",
                "result": null,
                "created": "2024-01-01T00:00:00Z",
                "updated": "2024-01-01T00:00:00Z"
            }
        })))
        .mount(server)
        .await;
}

/// Mock an execution get response
#[allow(dead_code)]
pub async fn mock_execution_get(server: &MockServer, execution_id: i64, status: &str) {
    let path_pattern = format!("/api/v1/executions/{}", execution_id);
    Mock::given(method("GET"))
        .and(path(path_pattern.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": execution_id,
                "action": 1,
                "action_ref": "core.echo",
                "config": {"message": "Hello"},
                "parent": null,
                "enforcement": null,
                "executor": null,
                "status": status,
                "result": {"output": "Hello"},
                "created": "2024-01-01T00:00:00Z",
                "updated": "2024-01-01T00:00:00Z"
            }
        })))
        .mount(server)
        .await;
}

/// Mock an execution list response with filters
#[allow(dead_code)]
pub async fn mock_execution_list(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/executions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "action_ref": "core.echo",
                    "status": "succeeded",
                    "parent": null,
                    "enforcement": null,
                    "result": {"output": "Hello"},
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                },
                {
                    "id": 2,
                    "action_ref": "core.echo",
                    "status": "failed",
                    "parent": null,
                    "enforcement": null,
                    "result": {"error": "Command failed"},
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
                "has_previous": false,
                "has_next": false
            }
        })))
        .mount(server)
        .await;
}

/// Mock a rule list response
#[allow(dead_code)]
pub async fn mock_rule_list(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/rules"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "ref": "core.on_webhook",
                    "pack": 1,
                    "pack_ref": "core",
                    "label": "On Webhook",
                    "description": "Handle webhook events",
                    "trigger": 1,
                    "trigger_ref": "core.webhook",
                    "action": 1,
                    "action_ref": "core.echo",
                    "enabled": true,
                    "conditions": {},
                    "action_params": {},
                    "trigger_params": {},
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
                "has_next": false,
                "has_previous": false,
                "total_items": 1,
                "total_pages": 1
            }
        })))
        .mount(server)
        .await;
}

/// Mock a trigger list response
#[allow(dead_code)]
pub async fn mock_trigger_list(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/triggers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "ref": "core.webhook",
                    "pack": 1,
                    "pack_ref": "core",
                    "label": "Webhook Trigger",
                    "description": "Webhook trigger",
                    "enabled": true,
                    "param_schema": {},
                    "out_schema": {},
                    "webhook_enabled": false,
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
                "has_next": false,
                "has_previous": false,
                "total_items": 1,
                "total_pages": 1
            }
        })))
        .mount(server)
        .await;
}

/// Mock a sensor list response
#[allow(dead_code)]
pub async fn mock_sensor_list(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/sensors"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "ref": "core.webhook_sensor",
                    "pack": 1,
                    "pack_ref": "core",
                    "label": "Webhook Sensor",
                    "description": "Webhook sensor",
                    "enabled": true,
                    "trigger_types": ["core.webhook"],
                    "entry_point": "webhook_sensor.py",
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
                "has_next": false,
                "has_previous": false,
                "total_items": 1,
                "total_pages": 1
            }
        })))
        .mount(server)
        .await;
}

/// Mock a 404 not found response
#[allow(dead_code)]
pub async fn mock_not_found(server: &MockServer, path_pattern: &str) {
    Mock::given(method("GET"))
        .and(path(path_pattern))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": "Not found"
        })))
        .mount(server)
        .await;
}

/// Mock a successful pack create response (POST /api/v1/packs)
#[allow(dead_code)]
pub async fn mock_pack_create(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/v1/packs"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "data": {
                "id": 42,
                "ref": "my_pack",
                "label": "My Pack",
                "description": "A test pack",
                "version": "0.1.0",
                "author": null,
                "enabled": true,
                "tags": ["test"],
                "created": "2024-01-01T00:00:00Z",
                "updated": "2024-01-01T00:00:00Z"
            }
        })))
        .mount(server)
        .await;
}

/// Mock a 409 conflict response for pack create
#[allow(dead_code)]
pub async fn mock_pack_create_conflict(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/v1/packs"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error": "Pack with ref 'my_pack' already exists"
        })))
        .mount(server)
        .await;
}
