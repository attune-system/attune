use super::*;
use wiremock::{
    matchers::{header, method, path},
    Mock, MockServer, ResponseTemplate,
};

// Inert fixture credentials, accepted only by this test-owned local API mock.
async fn start(
    api: &MockServer,
    policy: ToolPolicy,
    origins: Vec<String>,
) -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let server = McpServer::new(
        ApiClient::for_caller(&api.uri(), None).unwrap(),
        "http-test".into(),
        None,
        PacksCheckAccess::Disabled,
    );
    let mut server = server;
    server.tool_policy = policy;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = http_router(server, vec![addr.to_string()], origins).unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    (format!("http://{addr}/mcp"), stop, task)
}

async fn caller(api: &MockServer, token: &str, status: u16) {
    Mock::given(method("GET"))
        .and(path("/auth/me"))
        .and(header("authorization", format!("Bearer {token}")))
        .respond_with(ResponseTemplate::new(status).set_body_json(json!({"data":{"id":42}})))
        .mount(api)
        .await;
}

fn call(name: &str) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":{"id":42}}})
}

#[tokio::test]
async fn concurrent_callers_keep_authority_and_invalid_callers_never_dispatch_or_login() {
    let api = MockServer::start().await;
    caller(&api, "fixture-a", 200).await;
    caller(&api, "fixture-b", 200).await;
    caller(&api, "fixture-expired", 401).await;
    for (token, status, body) in [
        ("fixture-a", 200, json!({"data":{"id":42,"caller":"a"}})),
        (
            "fixture-b",
            403,
            json!({"error":{"code":"FORBIDDEN","message":"Denied"}}),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path("/api/v1/events/42"))
            .and(header("authorization", format!("Bearer {token}")))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .expect(1)
            .mount(&api)
            .await;
    }
    let (url, stop, task) = start(&api, ToolPolicy::ReadOnly, vec![]).await;
    let client = reqwest::Client::new();
    let request = call("events_get");
    let (a, b, expired) = tokio::join!(
        client
            .post(&url)
            .bearer_auth("fixture-a")
            .json(&request)
            .send(),
        client
            .post(&url)
            .bearer_auth("fixture-b")
            .json(&request)
            .send(),
        client
            .post(&url)
            .bearer_auth("fixture-expired")
            .json(&request)
            .send()
    );
    let a: Value = a.unwrap().json().await.unwrap();
    let b: Value = b.unwrap().json().await.unwrap();
    assert_eq!(a["result"]["structuredContent"]["caller"], "a");
    assert_eq!(b["result"]["isError"], true);
    assert_eq!(expired.unwrap().status(), StatusCode::UNAUTHORIZED);
    // Anonymous and malformed credentials never even reach the API.
    let before = api.received_requests().await.unwrap().len();
    for auth in [None, Some("Basic fixture-a"), Some("Bearer ")] {
        let mut req = client.post(&url).json(&request);
        if let Some(auth) = auth {
            req = req.header("authorization", auth);
        }
        assert_eq!(req.send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(api.received_requests().await.unwrap().len(), before);
    stop.send(()).unwrap();
    task.await.unwrap();
    let requests = api.received_requests().await.unwrap();
    assert_eq!(requests.len(), 5); // 3 identity checks + 2 authorized dispatch attempts
    assert!(requests
        .iter()
        .all(|r| r.url.path() == "/auth/me" || r.url.path() == "/api/v1/events/42"));
}

#[tokio::test]
async fn host_origin_and_default_policy_reject_before_tools() {
    let api = MockServer::start().await;
    caller(&api, "fixture-a", 200).await;
    let (url, stop, task) = start(
        &api,
        ToolPolicy::ReadOnly,
        vec!["https://console.example.invalid".into()],
    )
    .await;
    let client = reqwest::Client::new();
    for (name, value) in [
        ("host", "attacker.invalid"),
        ("origin", "https://attacker.invalid"),
        ("origin", "null"),
    ] {
        assert_eq!(
            client
                .post(&url)
                .header(name, value)
                .bearer_auth("fixture-a")
                .json(&call("events_get"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert!(api.received_requests().await.unwrap().is_empty());
    for name in [
        "actions_execute",
        "keys_set",
        "executions_export",
        "packs_check",
    ] {
        let body: Value = client
            .post(&url)
            .header("origin", "https://console.example.invalid")
            .bearer_auth("fixture-a")
            .json(&call(name))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(body["result"]["isError"], true, "{name}: {body}");
    }
    assert!(api
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|r| r.url.path() == "/auth/me"));
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn caller_client_rejects_redirects_and_has_no_refresh_fallback() {
    let api = MockServer::start().await;
    let other = MockServer::start().await;
    Mock::given(path("/auth/me"))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("location", format!("{}/auth/me", other.uri())),
        )
        .mount(&api)
        .await;
    let (url, stop, task) = start(&api, ToolPolicy::ReadOnly, vec![]).await;
    assert_eq!(
        reqwest::Client::new()
            .post(url)
            .bearer_auth("fixture-a")
            .json(&call("events_get"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    assert!(other.received_requests().await.unwrap().is_empty());
    assert_eq!(api.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn http_startup_rejects_server_credentials_and_requires_fixed_origin() {
    for option in [
        "--auth-token",
        "--execution-token",
        "--refresh-token",
        "--login",
        "--password",
        "--profile",
    ] {
        let cli = Cli::try_parse_from([
            "attune-mcp",
            "--transport",
            "http",
            "--api-url",
            "http://127.0.0.1:1",
            option,
            "inert-fixture",
        ])
        .unwrap();
        let error = build_server(&cli)
            .await
            .err()
            .expect("startup credential must be rejected");
        assert!(error.to_string().contains("per-request bearer"));
        assert!(!error.to_string().contains("inert-fixture"));
    }
    for url in [
        "file:///tmp/api",
        "http://user:password@localhost",
        "http://localhost/path",
        "http://localhost?token=fixture",
    ] {
        assert!(ApiClient::for_caller(url, None).is_err());
    }
    let cli = Cli::try_parse_from(["attune-mcp", "--transport", "http"]).unwrap();
    assert!(build_server(&cli).await.is_err());
}
