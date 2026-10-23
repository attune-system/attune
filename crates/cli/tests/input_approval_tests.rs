//! Declared-input consent contract tests use only local HTTP mocks and pure parsing.
use attune_cli::{
    cli::{Cli, Commands},
    client::{ApiClient, ApiRequestError},
    commands::{input_approval::InputApprovalCommands, pack::PackCommands},
    input_approval::{self, InputApprovalRequest},
};
use clap::Parser;
use serde_json::{json, Value};
use wiremock::{
    matchers::{body_json, header, method, path},
    Mock, MockServer, ResponseTemplate,
};

fn policy_request() -> Value {
    json!({"policy_revision":17,"invocation_binding":{"kind":"workflow_task","workflow_ref":"example.flow","task_path":"/tasks/send","action_ref":"example.send"},
        "declaration_revision":"retained-revision","source_path":"/tasks/send/input/message","expression":"{{ keystore['foreign.token'] }}",
        "grants":[{"resource":"keys","actions":["read","decrypt"],"scope":"external","constraints":{"refs":["foreign.token"]}}],"expires_at":"2099-01-01T00:00:00Z"})
}

fn parse(body: &Value) -> anyhow::Result<InputApprovalRequest> {
    InputApprovalRequest::parse(&serde_json::to_vec(body).unwrap())
}

fn mock_client(api: &MockServer) -> ApiClient {
    let mut client =
        ApiClient::from_config(&attune_cli::config::CliConfig::default(), &Some(api.uri()));
    client.set_tokens("mock-access-token".into(), "mock-refresh-token".into());
    client
}

#[test]
fn policy_and_renderer_targets_roundtrip_without_sdk_selections_or_proof() {
    let body = policy_request();
    assert_eq!(serde_json::to_value(parse(&body).unwrap()).unwrap(), body);
    let mut renderer = body.clone();
    renderer.as_object_mut().unwrap().remove("policy_revision");
    renderer
        .as_object_mut()
        .unwrap()
        .remove("invocation_binding");
    renderer["renderer_execution"] = json!(23);
    assert_eq!(
        serde_json::to_value(parse(&renderer).unwrap()).unwrap(),
        renderer
    );
    for (key, value) in [
        ("permission_set_refs", json!(["standard"])),
        ("purpose", json!("value")),
        ("proof", json!({"capability":"SECRET_CANARY"})),
        ("issuer_identity_id", json!(2)),
    ] {
        let mut invalid = body.clone();
        invalid[key] = value;
        let error = parse(&invalid).err().unwrap().to_string();
        assert!(!error.contains("SECRET_CANARY"));
    }
    for change in [
        json!({}),
        json!({"renderer_execution":23}),
        json!({"policy_revision":0,"invocation_binding":{"kind":"action","action_ref":"example.send"}}),
    ] {
        let mut invalid = body.clone();
        invalid.as_object_mut().unwrap().remove("policy_revision");
        invalid
            .as_object_mut()
            .unwrap()
            .remove("invocation_binding");
        invalid
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        // renderer-only is valid; mixed targets and incomplete policy targets are not.
        if invalid.get("renderer_execution").is_some() {
            invalid["policy_revision"] = json!(17);
        }
        assert!(parse(&invalid).is_err());
    }
}

#[test]
fn fixed_scope_rejects_structural_ids_sdk_effects_relative_scope_and_expiry() {
    for grant in [
        json!({"resource":"keys","actions":["read"],"scope":"pack"}),
        json!({"resource":"keys","actions":["read"],"constraints":{"managing_pack_id":42}}),
        json!({"resource":"keys","actions":["read"],"constraints":{"ids":[42]}}),
        json!({"resource":"keys","actions":["read"],"constraints":{"owner":"self"}}),
        json!({"resource":"keys","actions":["read"],"constraints":{"attributes":{"team":"ops"}}}),
        json!({"resource":"events","actions":["create"]}),
        json!({"resource":"keys","actions":["update"]}),
    ] {
        let mut body = policy_request();
        body["grants"] = json!([grant]);
        assert!(parse(&body).is_err());
    }
    let mut body = policy_request();
    body["expires_at"] = json!("2000-01-01T00:00:00Z");
    assert!(parse(&body).is_err());
    body["expires_at"] = json!(null);
    assert!(parse(&body).is_ok());
    body["grants"] = json!([]);
    assert!(parse(&body).is_err());
}

#[test]
fn cli_is_noninteractive_and_exposes_stdin_dry_run_and_explicit_revocation() {
    let cli = Cli::try_parse_from([
        "attune",
        "pack",
        "input-approval",
        "issue",
        "example",
        "--file",
        "-",
        "--dry-run",
    ])
    .unwrap();
    assert!(
        matches!(cli.command, Commands::Pack { command: PackCommands::InputApproval { command: InputApprovalCommands::Issue { file, dry_run:true, .. } } } if file == "-")
    );
    assert!(Cli::try_parse_from(["attune", "pack", "input-approval", "issue", "example"]).is_err());
    for verb in ["targets", "list", "show", "issue", "revoke"] {
        let error = Cli::try_parse_from(["attune", "pack", "input-approval", verb, "--help"])
            .err()
            .unwrap();
        assert!(error.to_string().contains("Examples:"));
    }
    assert!(Cli::try_parse_from([
        "attune",
        "pack",
        "input-approval",
        "replace",
        "example",
        "42"
    ])
    .is_err());
}

#[tokio::test]
async fn discovery_transport_returns_typed_public_metadata_without_creating_execution() {
    let api = MockServer::start().await;
    let request = policy_request();
    let target = json!({"policy_revision":17,"invocation_binding":request["invocation_binding"],"action_id":3,"action_ref":"example.send","pack_id":4,"pack_ref":"example","release_id":5,"metadata_revision":null,"declaration_revision":request["declaration_revision"],"source_path":request["source_path"],"expression":request["expression"]});
    Mock::given(method("GET"))
        .and(path("/api/v1/packs/example/input-approval-targets"))
        .and(wiremock::matchers::query_param("policy_revision", "17"))
        .and(wiremock::matchers::query_param("page", "2"))
        .and(wiremock::matchers::query_param("page_size", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"items":[target.clone()],"pagination":{"page":2,"total_items":2}}),
        ))
        .expect(1)
        .mount(&api)
        .await;
    let mut client = mock_client(&api);
    let discovered = input_approval::discover(&mut client, "example", Some(17), 2, 1)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&discovered.items[0]).unwrap(), target);
    assert!(
        input_approval::discover(&mut client, "example", Some(0), 1, 1)
            .await
            .is_err()
    );
    assert!(
        input_approval::discover(&mut client, "example", None, 1, 101)
            .await
            .is_err()
    );
    assert_eq!(api.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn transport_issues_before_execution_and_uses_shared_get_list_revoke_routes() {
    let api = MockServer::start().await;
    let body = policy_request();
    let record = json!({"id":42,"active":true,"grants":body["grants"],"approval_digest":"opaque-digest","expires_at":body["expires_at"]});
    for (verb, endpoint, response) in [
        (
            "POST",
            "/api/v1/packs/example/external-input-approvals",
            json!({"data":record}),
        ),
        (
            "GET",
            "/api/v1/packs/example/external-approvals/42",
            json!({"data":record}),
        ),
        (
            "GET",
            "/api/v1/packs/example/external-approvals",
            json!({"items":[record],"pagination":{"total_pages":1}}),
        ),
        (
            "POST",
            "/api/v1/packs/example/external-approvals/42/revoke",
            json!({"data":{"id":42,"active":false,"revoked_at":"2026-10-15T00:00:00Z"}}),
        ),
    ] {
        let mock = Mock::given(method(verb))
            .and(path(endpoint))
            .and(header("authorization", "Bearer mock-access-token"));
        let mock = if endpoint.ends_with("external-input-approvals") {
            mock.and(body_json(body.clone()))
        } else {
            mock
        };
        mock.respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&api)
            .await;
    }
    let mut client = mock_client(&api);
    assert_eq!(
        input_approval::issue(&mut client, "example", &parse(&body).unwrap())
            .await
            .unwrap(),
        record
    );
    assert_eq!(
        input_approval::get(&mut client, "example", 42)
            .await
            .unwrap(),
        record
    );
    let listed = input_approval::list(&mut client, "example").await.unwrap();
    assert_eq!(listed, vec![record]);
    assert!(input_approval::revoke(&mut client, "example", 42, false)
        .await
        .is_err());
    assert_eq!(
        input_approval::revoke(&mut client, "example", 42, true)
            .await
            .unwrap()["active"],
        false
    );
    assert_eq!(api.received_requests().await.unwrap().len(), 4);
}

#[tokio::test]
async fn denial_keeps_canonical_status_code_request_id_without_request_secrets() {
    let api = MockServer::start().await;
    Mock::given(method("POST")).and(path("/api/v1/packs/example/external-input-approvals"))
        .respond_with(ResponseTemplate::new(403).insert_header("x-request-id","input-denied-42")
            .set_body_json(json!({"code":"FORBIDDEN","error":"Input declaration authority is unavailable"}))).expect(1).mount(&api).await;
    let mut client = mock_client(&api);
    let error = input_approval::issue(&mut client, "example", &parse(&policy_request()).unwrap())
        .await
        .unwrap_err();
    let typed = error.downcast_ref::<ApiRequestError>().unwrap();
    assert_eq!(typed.status.as_u16(), 403);
    assert_eq!(typed.code.as_deref(), Some("FORBIDDEN"));
    assert_eq!(typed.request_id.as_deref(), Some("input-denied-42"));
    assert!(!error.to_string().contains("mock-access-token"));
    assert!(!error.to_string().contains("keystore"));
}

#[tokio::test]
async fn cli_stdin_uses_profile_auth_and_dry_run_never_sends_or_echoes_expression() {
    let api = MockServer::start().await;
    let config = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(config.path().join("attune")).unwrap();
    std::fs::write(config.path().join("attune/config.yaml"), format!(
        "current_profile: staging\ndefault_output_format: json\nprofiles:\n  staging:\n    api_url: {}\n    auth_token: mock-profile-access\n", api.uri()
    )).unwrap();
    let body = policy_request();
    Mock::given(method("POST"))
        .and(path("/api/v1/packs/example/external-input-approvals"))
        .and(header("authorization", "Bearer mock-profile-access"))
        .and(body_json(body.clone()))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"data":{"id":42,"active":true}})),
        )
        .expect(1)
        .mount(&api)
        .await;
    for dry_run in [true, false] {
        let root = config.path().to_path_buf();
        let stdin = serde_json::to_vec(&body).unwrap();
        let output = tokio::task::spawn_blocking(move || {
            let mut command = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("attune"));
            command
                .env("HOME", &root)
                .env("XDG_CONFIG_HOME", &root)
                .env_remove("ATTUNE_API_TOKEN")
                .env_remove("ATTUNE_AUTH_TOKEN")
                .env_remove("ATTUNE_REFRESH_TOKEN")
                .env_remove("ATTUNE_API_URL")
                .env_remove("ATTUNE_PROFILE")
                .args([
                    "--profile",
                    "staging",
                    "--output",
                    "json",
                    "pack",
                    "input-approval",
                    "issue",
                    "example",
                    "--file",
                    "-",
                ])
                .write_stdin(stdin)
                .timeout(std::time::Duration::from_secs(10));
            if dry_run {
                command.arg("--dry-run");
            }
            command.output().unwrap()
        })
        .await
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let record: Value = serde_json::from_slice(&output.stdout).unwrap();
        if dry_run {
            assert_eq!(record["authorization_checked"], false);
            assert_eq!(record["expression"], "[REDACTED]");
            assert_eq!(api.received_requests().await.unwrap().len(), 0);
        } else {
            assert_eq!(record["id"], 42);
        }
        assert!(!String::from_utf8_lossy(&output.stdout).contains("keystore"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("mock-profile-access"));
    }
}
