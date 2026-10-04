mod common;

use assert_cmd::Command;
use common::TestFixture;
use serde_json::{json, Value};
use wiremock::{
    matchers::{body_json, header, method, path, query_param},
    Mock, ResponseTemplate,
};

const SET_ID: i64 = 9_007_199_254_740_993;

fn command(fixture: &TestFixture) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_attune"));
    command
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .env_remove("ATTUNE_PROFILE")
        .env_remove("ATTUNE_API_TOKEN")
        .env_remove("ATTUNE_AUTH_TOKEN")
        .env_remove("ATTUNE_REFRESH_TOKEN")
        .args(["--api-url", &fixture.server_url()]);
    command
}

async fn fixture() -> TestFixture {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("permissions_test_token", "refresh_token");
    fixture
}

fn permission_set() -> Value {
    json!({
        "id": SET_ID, "ref": "deploy.operator", "pack_ref": "deploy",
        "label": "Deployment operator", "description": "Run deployment actions",
        "grants": [{"resource": "actions", "actions": ["read", "execute"],
                    "constraints": {"refs": ["deploy.release"]}}],
        "retired_at": null, "management_origin": "pack", "roles": []
    })
}

fn binding() -> Value {
    json!({"id": 72, "permission_set_id": SET_ID, "permission_set_ref": "deploy.operator",
           "target": {"type": "role", "role": "deployment-operators"}, "created": "2026-10-03T00:00:00Z"})
}

fn paginated(items: Vec<Value>) -> Value {
    json!({"items": items, "pagination": {"has_next": false}})
}

async fn mock_set(fixture: &TestFixture, value: Value) {
    Mock::given(method("GET"))
        .and(path("/api/v1/permissions/sets/by-ref/deploy.operator"))
        .and(header("authorization", "Bearer permissions_test_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": value})))
        .mount(&fixture.mock_server)
        .await;
}

async fn mock_bindings(fixture: &TestFixture, items: Vec<Value>) {
    Mock::given(method("GET"))
        .and(path("/api/v1/permissions/assignments"))
        .and(query_param("permission_set_ref", "deploy.operator"))
        .and(query_param("role", "deployment-operators"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paginated(items)))
        .mount(&fixture.mock_server)
        .await;
}

async fn run_json(fixture: &TestFixture, args: &[&str]) -> Value {
    let mut command = command(fixture);
    command.args(args).arg("--json");
    let output = tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn permission_targets_and_identity_role_id_syntax_are_unambiguous() {
    use attune_cli::cli::Cli;
    use clap::Parser;
    for args in [
        vec!["attune", "permission", "assign", "core.executor"],
        vec![
            "attune",
            "permission",
            "assign",
            "core.executor",
            "--identity",
            "alice",
            "--role",
            "ops",
        ],
        vec!["attune", "identity", "show"],
        vec!["attune", "identity", "show", "alice", "--identity-id", "42"],
        vec![
            "attune",
            "permission",
            "assignment",
            "list",
            "--identity-id",
            "42",
            "--role",
            "ops",
        ],
        vec!["attune", "identity", "list", "--per-page", "0"],
    ] {
        assert!(Cli::try_parse_from(&args).is_err(), "{args:?}");
    }
    for args in [
        vec!["attune", "identity", "role", "add", "alice", "ops"],
        vec![
            "attune",
            "identity",
            "role",
            "add",
            "--identity-id",
            "42",
            "ops",
        ],
        vec![
            "attune",
            "identity",
            "role",
            "remove",
            "--identity-id",
            "42",
            "ops",
            "--dry-run",
        ],
    ] {
        assert!(Cli::try_parse_from(&args).is_ok(), "{args:?}");
    }
}

#[tokio::test]
async fn export_is_authorable_yaml_and_update_replaces_grants_from_stdin() {
    let fixture = fixture().await;
    let original = permission_set();
    mock_set(&fixture, original.clone()).await;
    let mut export = command(&fixture);
    export.args(["permission", "set", "export", "deploy.operator"]);
    let output = tokio::task::spawn_blocking(move || export.output().unwrap())
        .await
        .unwrap();
    assert!(output.status.success());
    let mut definition: Value = serde_yaml_ng::from_slice(&output.stdout).unwrap();
    assert_eq!(definition["ref"], "deploy.operator");
    assert_eq!(definition["grants"], original["grants"]);
    for field in ["id", "roles", "pack_ref", "management_origin", "retired_at"] {
        assert!(definition.get(field).is_none(), "Export included {field}");
    }
    definition["grants"] = json!([{ "resource": "actions", "actions": ["read"], "constraints": {"pack_refs": ["deploy"]}}]);
    let mut updated = original.clone();
    updated["grants"] = definition["grants"].clone();
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/permissions/sets/{SET_ID}")))
        .and(query_param("dry_run", "true"))
        .and(body_json(json!({"label": definition["label"], "description": definition["description"], "grants": definition["grants"]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": updated})))
        .expect(1).mount(&fixture.mock_server).await;
    let mut update = command(&fixture);
    update
        .args([
            "permission",
            "set",
            "update",
            "deploy.operator",
            "--file",
            "-",
            "--dry-run",
            "--json",
        ])
        .write_stdin(serde_yaml_ng::to_string(&definition).unwrap());
    let output = tokio::task::spawn_blocking(move || update.output().unwrap())
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["changed"], false);
    assert_eq!(result["would_change"], true);
    assert_eq!(result["before"]["grants"], original["grants"]);
    assert_eq!(result["after"]["grants"], definition["grants"]);
}

#[tokio::test]
async fn set_list_reads_the_bare_array_and_encodes_pack_filter() {
    let fixture = fixture().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/permissions/sets"))
        .and(query_param("pack_ref", "deploy & ops"))
        .and(query_param("include_retired", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(vec![permission_set()]))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    let output = run_json(
        &fixture,
        &[
            "permission",
            "set",
            "list",
            "--pack",
            "deploy & ops",
            "--include-retired",
        ],
    )
    .await;
    assert_eq!(output[0]["id"], SET_ID);
    assert_eq!(output[0]["management_origin"], "pack");
}

#[tokio::test]
async fn assignments_list_preserves_target_types_and_encodes_filters() {
    let fixture = fixture().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/permissions/assignments"))
        .and(query_param("role", "ops & deploy"))
        .and(query_param("permission_set_ref", "deploy.operator"))
        .and(query_param("page", "2"))
        .and(query_param("page_size", "25"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paginated(vec![binding()])))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    let output = run_json(
        &fixture,
        &[
            "permission",
            "assignment",
            "list",
            "--role",
            "ops & deploy",
            "--set",
            "deploy.operator",
            "--page",
            "2",
            "--per-page",
            "25",
        ],
    )
    .await;
    assert_eq!(output["items"][0]["target"]["type"], "role");
}

#[tokio::test]
async fn role_assign_and_revoke_use_distinct_mapping_endpoints() {
    let fixture = fixture().await;
    mock_set(&fixture, permission_set()).await;
    mock_bindings(&fixture, vec![]).await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/permissions/sets/{SET_ID}/roles")))
        .and(body_json(json!({"role": "deployment-operators"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"data": {"id": 72}})))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    let result = run_json(
        &fixture,
        &[
            "permission",
            "assign",
            "deploy.operator",
            "--role",
            "deployment-operators",
        ],
    )
    .await;
    assert_eq!(result["changed"], true);
    let revoke_fixture = TestFixture::new().await;
    revoke_fixture.write_authenticated_config("permissions_test_token", "refresh_token");
    mock_set(&revoke_fixture, permission_set()).await;
    mock_bindings(&revoke_fixture, vec![binding()]).await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/permissions/sets/roles/72"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&revoke_fixture.mock_server)
        .await;
    let result = run_json(
        &revoke_fixture,
        &[
            "permission",
            "revoke",
            "deploy.operator",
            "--role",
            "deployment-operators",
        ],
    )
    .await;
    assert_eq!(result["changed"], true);
}

#[tokio::test]
async fn assignment_noops_and_dry_runs_do_not_send_mutations() {
    let fixture = fixture().await;
    mock_set(&fixture, permission_set()).await;
    mock_bindings(&fixture, vec![binding()]).await;
    let result = run_json(
        &fixture,
        &[
            "permission",
            "assign",
            "deploy.operator",
            "--role",
            "deployment-operators",
        ],
    )
    .await;
    assert_eq!(result["changed"], false);
    let result = run_json(
        &fixture,
        &[
            "permission",
            "revoke",
            "deploy.operator",
            "--role",
            "deployment-operators",
            "--dry-run",
        ],
    )
    .await;
    assert_eq!(result["changed"], false);
    assert_eq!(result["would_change"], true);
    let empty_fixture = TestFixture::new().await;
    empty_fixture.write_authenticated_config("permissions_test_token", "refresh_token");
    mock_set(&empty_fixture, permission_set()).await;
    mock_bindings(&empty_fixture, vec![]).await;
    let result = run_json(
        &empty_fixture,
        &[
            "permission",
            "revoke",
            "deploy.operator",
            "--role",
            "deployment-operators",
        ],
    )
    .await;
    assert_eq!(result["changed"], false);
    let result = run_json(
        &empty_fixture,
        &[
            "permission",
            "assign",
            "deploy.operator",
            "--role",
            "deployment-operators",
            "--dry-run",
        ],
    )
    .await;
    assert_eq!(result["changed"], false);
    assert_eq!(result["would_change"], true);
    assert!(empty_fixture
        .mock_server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|request| request.method == "GET"));
    assert!(fixture
        .mock_server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|request| request.method == "GET"));
}

#[tokio::test]
async fn assignment_conflict_is_a_noop_only_if_the_binding_exists() {
    let fixture = fixture().await;
    mock_set(&fixture, permission_set()).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/permissions/assignments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paginated(vec![])))
        .up_to_n_times(1)
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/permissions/sets/{SET_ID}/roles")))
        .respond_with(
            ResponseTemplate::new(409).set_body_json(json!({"error": "Assignment already exists"})),
        )
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    // Lower priority than the first, one-shot preflight response.
    Mock::given(method("GET"))
        .and(path("/api/v1/permissions/assignments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paginated(vec![binding()])))
        .with_priority(6)
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    let result = run_json(
        &fixture,
        &[
            "permission",
            "assign",
            "deploy.operator",
            "--role",
            "deployment-operators",
        ],
    )
    .await;
    assert_eq!(result["changed"], false);
}

#[tokio::test]
async fn forbidden_assignment_is_not_reported_as_success() {
    let fixture = fixture().await;
    mock_set(&fixture, permission_set()).await;
    mock_bindings(&fixture, vec![]).await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/permissions/sets/{SET_ID}/roles")))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(json!({"error": "Insufficient permissions: permissions:manage"})),
        )
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    let output = command(&fixture)
        .args([
            "permission",
            "assign",
            "deploy.operator",
            "--role",
            "deployment-operators",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("permissions:manage"));
}

#[tokio::test]
async fn platform_sets_and_mismatched_definitions_are_rejected_before_put() {
    let fixture = fixture().await;
    let definition = fixture.config_dir_path().join("definition.yaml");
    std::fs::write(&definition, "ref: deploy.operator\ngrants: []\n").unwrap();
    let mut set = permission_set();
    set["management_origin"] = json!("platform");
    mock_set(&fixture, set).await;
    let output = command(&fixture)
        .args(["permission", "set", "update", "deploy.operator", "--file"])
        .arg(&definition)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("platform-managed"));
    std::fs::write(&definition, "ref: deploy.other\ngrants: []\n").unwrap();
    let output = command(&fixture)
        .args(["permission", "set", "update", "deploy.operator", "--file"])
        .arg(&definition)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not match target"));
    assert!(fixture
        .mock_server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|request| request.method == "GET"));
}

#[tokio::test]
async fn reserved_standard_ref_cannot_be_assigned() {
    let fixture = fixture().await;
    let output = command(&fixture)
        .args(["permission", "assign", "standard", "--role", "ops"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("reserved for execution tokens"));
    assert!(fixture
        .mock_server
        .received_requests()
        .await
        .unwrap()
        .is_empty());
}

async fn mock_identity(fixture: &TestFixture, managed: bool) {
    Mock::given(method("GET"))
        .and(path("/api/v1/identities/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {
            "id": 42, "login": "alice+ops@example.com", "display_name": "Alice", "frozen": false,
            "attributes": {}, "direct_permissions": [], "roles": [{"id": 81, "identity_id": 42,
            "role": "ops", "source": if managed { "oidc" } else { "manual" }, "managed": managed,
            "created": "2026-10-03T00:00:00Z", "updated": "2026-10-03T00:00:00Z"}]
        }})))
        .mount(&fixture.mock_server)
        .await;
}

#[tokio::test]
async fn login_lookup_is_exact_and_managed_role_removal_never_sends_delete() {
    let fixture = fixture().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/identities"))
        .and(query_param("login", "alice+ops@example.com"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(paginated(vec![json!({
                "id": 42, "login": "alice+ops@example.com", "display_name": "Alice",
                "frozen": false, "attributes": {}, "roles": ["ops"]
            })])),
        )
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    mock_identity(&fixture, true).await;
    let output = command(&fixture)
        .args([
            "identity",
            "role",
            "remove",
            "alice+ops@example.com",
            "ops",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("provider-managed"));
    assert!(fixture
        .mock_server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|request| request.method == "GET"));
}

#[tokio::test]
async fn manual_role_removal_and_freeze_dry_run_use_identity_id() {
    let fixture = fixture().await;
    mock_identity(&fixture, false).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/identities/42/roles"))
        .and(body_json(json!({"role": "deployment-operators"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"data": {
            "id": 82, "identity_id": 42, "role": "deployment-operators", "source": "manual", "managed": false,
            "created": "2026-10-03T00:00:00Z", "updated": "2026-10-03T00:00:00Z"
        }})))
        .expect(1).mount(&fixture.mock_server).await;
    let result = run_json(
        &fixture,
        &[
            "identity",
            "role",
            "add",
            "--identity-id",
            "42",
            "deployment-operators",
        ],
    )
    .await;
    assert_eq!(result["changed"], true);
    Mock::given(method("DELETE"))
        .and(path("/api/v1/identities/roles/81"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    let result = run_json(
        &fixture,
        &["identity", "role", "remove", "--identity-id", "42", "ops"],
    )
    .await;
    assert_eq!(result["changed"], true);
    let result = run_json(
        &fixture,
        &["identity", "freeze", "--identity-id", "42", "--dry-run"],
    )
    .await;
    assert_eq!(result["changed"], false);
    assert_eq!(result["would_change"], true);
}

#[tokio::test]
async fn direct_assignment_uses_identity_endpoint_and_preserves_role_membership() {
    let fixture = fixture().await;
    mock_set(&fixture, permission_set()).await;
    mock_identity(&fixture, false).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/permissions/assignments"))
        .and(query_param("identity_id", "42"))
        .and(query_param("permission_set_ref", "deploy.operator"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paginated(vec![])))
        .mount(&fixture.mock_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/permissions/assignments"))
        .and(body_json(
            json!({"identity_id": 42, "permission_set_ref": "deploy.operator"}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"data": {"id": 71}})))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    let result = run_json(
        &fixture,
        &[
            "permission",
            "assign",
            "deploy.operator",
            "--identity-id",
            "42",
        ],
    )
    .await;
    assert_eq!(result["changed"], true);

    let revoke_fixture = TestFixture::new().await;
    revoke_fixture.write_authenticated_config("permissions_test_token", "refresh_token");
    mock_set(&revoke_fixture, permission_set()).await;
    mock_identity(&revoke_fixture, false).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/permissions/assignments"))
        .and(query_param("identity_id", "42"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(paginated(vec![json!({
                "id": 71, "permission_set_id": SET_ID, "permission_set_ref": "deploy.operator",
                "target": {"type": "identity", "identity_id": 42, "login": "alice+ops@example.com"},
                "created": "2026-10-03T00:00:00Z"
            })])),
        )
        .mount(&revoke_fixture.mock_server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/permissions/assignments/71"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&revoke_fixture.mock_server)
        .await;
    let result = run_json(
        &revoke_fixture,
        &[
            "permission",
            "revoke",
            "deploy.operator",
            "--identity-id",
            "42",
        ],
    )
    .await;
    assert_eq!(result["changed"], true);
    let requests = revoke_fixture
        .mock_server
        .received_requests()
        .await
        .unwrap();
    assert!(requests
        .iter()
        .filter(|request| request.method == "DELETE")
        .all(|request| request.url.path() == "/api/v1/permissions/assignments/71"));
}

#[tokio::test]
async fn retired_assignment_retries_are_noops_but_new_assignments_fail() {
    let fixture = fixture().await;
    let mut retired = permission_set();
    retired["retired_at"] = json!("2026-10-03T00:00:00Z");
    mock_set(&fixture, retired.clone()).await;
    mock_bindings(&fixture, vec![binding()]).await;
    let result = run_json(
        &fixture,
        &[
            "permission",
            "assign",
            "deploy.operator",
            "--role",
            "deployment-operators",
        ],
    )
    .await;
    assert_eq!(result["changed"], false);
    let new_fixture = TestFixture::new().await;
    new_fixture.write_authenticated_config("permissions_test_token", "refresh_token");
    mock_set(&new_fixture, retired).await;
    mock_bindings(&new_fixture, vec![]).await;
    let output = command(&new_fixture)
        .args([
            "permission",
            "assign",
            "deploy.operator",
            "--role",
            "deployment-operators",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be newly assigned"));
    assert!(new_fixture
        .mock_server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|request| request.method == "GET"));
}

#[tokio::test]
async fn role_name_limits_apply_to_mutation_dry_runs() {
    let fixture = fixture().await;
    mock_set(&fixture, permission_set()).await;
    let too_long = "r".repeat(256);
    for args in [
        vec![
            "permission",
            "assign",
            "deploy.operator",
            "--role",
            &too_long,
            "--dry-run",
        ],
        vec![
            "identity",
            "role",
            "add",
            "--identity-id",
            "42",
            &too_long,
            "--dry-run",
        ],
    ] {
        let output = command(&fixture).args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("1 to 255 characters"));
    }
    assert!(fixture
        .mock_server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|request| request.method == "GET"));
}
