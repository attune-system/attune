use super::*;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn server(api: &MockServer) -> McpServer {
    let mut config = CliConfig::default();
    config.current_profile_mut().unwrap().api_url = api.uri();
    let mut server = McpServer::new(
        ApiClient::from_config(&config, &None).unwrap(),
        config.current_profile.clone(),
        None,
        PacksCheckAccess::Disabled,
    );
    server.tool_policy = ToolPolicy::FullAccess;
    server
}

async fn call(server: &mut McpServer, name: &str, args: Value) -> Value {
    server.handle_request(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}}))
        .await.unwrap().unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn export_returns_only_file_metadata_in_all_mcp_representations() {
    let api = MockServer::start().await;
    let execution = json!({"id":42,"config":{"token":"fixture-secret-input"},"result":{"stdout":"public sanitized projection","data":{"restricted":{"$attune_secret":true,"redacted":true}}},"revealed_output_channels":{"stdout":"fixture-secret-original"}});
    Mock::given(method("GET"))
        .and(path("/api/v1/executions/42"))
        .and(wiremock::matchers::query_param(
            "include_secret_values",
            "true",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":execution})))
        .expect(1)
        .mount(&api)
        .await;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("execution.json");
    let response = call(
        &mut server(&api),
        "executions_export",
        json!({"id":42,"output_file":target}),
    )
    .await;
    assert_eq!(response["result"]["isError"], false);
    let encoded = response.to_string();
    for secret in [
        "fixture-secret-input",
        "fixture-secret-original",
        "public sanitized projection",
    ] {
        assert!(!encoded.contains(secret));
    }
    assert_eq!(response["result"]["structuredContent"]["status"], "written");
    assert!(std::path::Path::new(
        response["result"]["structuredContent"]["output_file"]
            .as_str()
            .unwrap()
    )
    .is_absolute());
    assert!(response["result"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .all(|content| content["type"] == "text"));
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&target).unwrap()).unwrap(),
        execution
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn export_denial_never_creates_a_file_or_echoes_response_details() {
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403).set_body_json(
            json!({"error":"fixture-secret-error","details":{"value":"fixture-secret-body"}}),
        ))
        .expect(1)
        .mount(&api)
        .await;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("denied.json");
    let response = call(
        &mut server(&api),
        "executions_export",
        json!({"id":42,"output_file":target}),
    )
    .await;
    assert_eq!(response["result"]["isError"], true);
    assert!(!response.to_string().contains("fixture-secret"));
    assert!(!target.exists());
}

#[tokio::test]
async fn export_requires_path_and_never_clobbers_or_fetches_for_occupied_path() {
    let api = MockServer::start().await;
    let mut server = server(&api);
    let missing = call(&mut server, "executions_export", json!({"id":42})).await;
    assert_eq!(missing["result"]["isError"], true);
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("keep.json");
    std::fs::write(&target, b"keep").unwrap();
    let collision = call(
        &mut server,
        "executions_export",
        json!({"id":42,"output_file":target}),
    )
    .await;
    assert_eq!(collision["result"]["isError"], true);
    assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    assert!(api.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn inline_execution_get_never_requests_plaintext_even_with_extra_arguments() {
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/executions/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data":{"id":42,"result":{"data":{"$attune_secret":true,"redacted":true}}}}),
        ))
        .expect(1)
        .mount(&api)
        .await;
    let response = call(
        &mut server(&api),
        "executions_get",
        json!({"id":42,"include_secret_values":true}),
    )
    .await;
    assert_eq!(response["result"]["isError"], false);
    let requests = api.received_requests().await.unwrap();
    assert!(requests[0].url.query().is_none());
}

#[test]
fn export_is_a_local_write_and_is_not_in_the_builtin_read_only_policy() {
    let tools = tool_defs();
    let export = tools
        .iter()
        .find(|tool| tool.name == "executions_export")
        .unwrap();
    let get = tools
        .iter()
        .find(|tool| tool.name == "executions_get")
        .unwrap();
    assert!(!export.read_only);
    assert!(!ToolPolicy::ReadOnly.allows(export));
    assert!(ToolPolicy::ReadOnly.allows(get));
    assert!(ToolPolicy::Allowlist(vec!["executions_export".into()]).allows(export));
    let schema = executions_export_schema();
    assert_eq!(schema["required"], json!(["id", "output_file"]));
    assert_eq!(schema["additionalProperties"], false);
}

#[cfg(unix)]
#[tokio::test]
async fn export_does_not_reflect_reauthentication_failure_bodies() {
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/executions/42"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error":"Unauthorized"})))
        .expect(1)
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path("/auth/login"))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({"error":"fixture-secret-login-body"})),
        )
        .expect(1)
        .mount(&api)
        .await;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("execution.json");
    let mut server = server(&api);
    server.credentials = Some(LoginCredentials {
        api_url: api.uri(),
        login: "fixture-login".into(),
        password: "fixture-secret-password".into(),
    });
    let response = call(
        &mut server,
        "executions_export",
        json!({"id":42,"output_file":target}),
    )
    .await;
    assert_eq!(response["result"]["isError"], true);
    assert!(!response.to_string().contains("fixture-secret"));
    assert!(!target.exists());
}

#[cfg(not(unix))]
#[tokio::test]
async fn export_on_unsupported_platform_returns_safe_error_without_fetch_or_file() {
    let api = MockServer::start().await;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("execution.json");
    let response = call(
        &mut server(&api),
        "executions_export",
        json!({"id":42,"output_file":target}),
    )
    .await;
    assert_eq!(response["result"]["isError"], true);
    assert!(!target.exists());
    assert!(api.received_requests().await.unwrap().is_empty());
}
