//! Exercise policy loading and policy enforcement through the real stdio binary.
use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};

fn command(root: &std::path::Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("attune-mcp"));
    for name in [
        "ATTUNE_PROFILE",
        "ATTUNE_API_URL",
        "ATTUNE_AUTH_TOKEN",
        "ATTUNE_API_TOKEN",
        "ATTUNE_REFRESH_TOKEN",
        "ATTUNE_LOGIN",
        "ATTUNE_PASSWORD",
        "ATTUNE_MCP_TRANSPORT",
        "ATTUNE_MCP_ACCESS_MODE",
        "ATTUNE_MCP_ALLOWED_TOOLS",
        "ATTUNE_MCP_POLICY_CONFIG",
        "ATTUNE_MCP_ACCESS_POLICY",
        "ATTUNE_MCP_PACKS_CHECK_ROOTS",
    ] {
        cmd.env_remove(name);
    }
    cmd.env("XDG_CONFIG_HOME", root);
    cmd
}

#[test]
fn stdio_access_policies_load_default_and_environment_selection() {
    let root = tempfile::tempdir().unwrap();
    let policy = root.path().join("policies.yaml");
    std::fs::write(&policy, "default_policy: observer\npolicies:\n  observer:\n    access_mode: read-only\n    allowed_tools: [events_get, actions_execute]\n  disabled:\n    access_mode: full\n    allowed_tools: []\n").unwrap();
    let input = [
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"actions_execute","arguments":{"ref":"core.echo"}}}),
    ].into_iter().map(|r| format!("{r}\n")).collect::<String>();
    for disabled in [false, true] {
        let mut cmd = command(root.path());
        cmd.env("ATTUNE_MCP_POLICY_CONFIG", &policy);
        if disabled {
            cmd.env("ATTUNE_MCP_ACCESS_POLICY", "disabled");
        }
        let output = cmd
            .write_stdin(input.clone())
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let replies = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(replies.len(), 2);
        let tools = replies[0]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), if disabled { 0 } else { 1 });
        if !disabled {
            assert_eq!(tools[0]["name"], "events_get");
        }
        assert_eq!(replies[1]["result"]["isError"], true);
        assert!(replies[1]
            .to_string()
            .contains("disabled by MCP access policy"));
    }
}

#[test]
fn invalid_access_policy_fails_startup_before_attempting_login() {
    let root = tempfile::tempdir().unwrap();
    let policy = root.path().join("policies.yaml");
    std::fs::write(
        &policy,
        "default_policy: observer\npolicies:\n  observer:\n    access_mode: read-only\n",
    )
    .unwrap();
    command(root.path())
        .args([
            "--policy-config",
            policy.to_str().unwrap(),
            "--access-policy",
            "missing",
        ])
        .env("ATTUNE_LOGIN", "unused")
        .env("ATTUNE_PASSWORD", "unused")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Unknown MCP access policy 'missing'",
        ));
}

#[test]
fn user_policies_are_discovered_locally_and_read_only_needs_no_file() {
    let root = tempfile::tempdir().unwrap();
    let input = format!(
        "{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})
    );
    // The default starts with no policy file and does not install one.
    let output = command(root.path())
        .write_stdin(input.clone())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let reply: Value = serde_json::from_slice(&output).unwrap();
    let tools = reply["result"]["tools"].as_array().unwrap();
    assert!(!tools.is_empty());
    assert!(tools
        .iter()
        .all(|tool| tool["annotations"]["readOnlyHint"] == true));
    let policy_path = root.path().join("attune/mcp-policies.yaml");
    assert!(!policy_path.exists());
    command(root.path())
        .args(["--access-policy", "read-only"])
        .write_stdin(input.clone())
        .assert()
        .success();
    // Custom policies belong only to this user, with no explicit file flag.
    std::fs::write(&policy_path, "default_policy: my-operator\npolicies:\n  my-operator:\n    access_mode: full\n    allowed_tools: [actions_execute]\n").unwrap();
    let output = command(root.path())
        .write_stdin(input.clone())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let reply: Value = serde_json::from_slice(&output).unwrap();
    let tools = reply["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "actions_execute");
    // Another user's config directory has only the built-in read-only policy.
    let other_user = tempfile::tempdir().unwrap();
    command(other_user.path())
        .args(["--access-policy", "my-operator"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unknown MCP access policy"));
    // An empty per-user file mapping still falls back to read-only.
    std::fs::write(policy_path, "policies: {}\n").unwrap();
    let output = command(root.path())
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let reply: Value = serde_json::from_slice(&output).unwrap();
    assert!(reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .all(|tool| tool["annotations"]["readOnlyHint"] == true));
}

#[test]
fn builtin_full_access_needs_no_file_and_enables_the_complete_stdio_catalog() {
    let root = tempfile::tempdir().unwrap();
    let input = format!(
        "{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})
    );
    let output = command(root.path())
        .args(["--access-policy", "full-access"])
        .write_stdin(input.clone())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let full: Value = serde_json::from_slice(&output).unwrap();
    let full_tools = full["result"]["tools"].as_array().unwrap();
    for name in [
        "actions_execute",
        "executions_cancel",
        "packs_update_config",
        "permission_sets_import",
        "cache_refresh_promote",
        "packs_check",
    ] {
        assert!(full_tools.iter().any(|tool| tool["name"] == name), "{name}");
    }
    assert!(!root.path().join("attune/mcp-policies.yaml").exists());
    let output = command(root.path())
        .env("ATTUNE_MCP_ACCESS_POLICY", "full-access")
        .write_stdin(input.clone())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(serde_json::from_slice::<Value>(&output).unwrap(), full);
    let output = command(root.path())
        .args(["--access-mode", "full"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let safe: Value = serde_json::from_slice(&output).unwrap();
    assert!(safe["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .all(|tool| tool["annotations"]["readOnlyHint"] == true));
}

#[test]
fn builtin_stdio_read_only_catalog_exactly_matches_full_access_read_only_annotations() {
    use std::collections::BTreeMap;

    let root = tempfile::tempdir().unwrap();
    let request = format!(
        "{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})
    );
    let list_tools = |policy: &str| {
        let output = command(root.path())
            .args(["--access-policy", policy])
            .write_stdin(request.clone())
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let response: Value = serde_json::from_slice(&output).unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();
        let mut by_name = BTreeMap::new();
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            let read_only = tool["annotations"]["readOnlyHint"]
                .as_bool()
                .unwrap_or_else(|| panic!("{name} is missing a boolean readOnlyHint"));
            assert!(
                by_name.insert(name.to_owned(), read_only).is_none(),
                "duplicate tool: {name}"
            );
        }
        by_name
    };
    let full_tools = list_tools("full-access");
    assert!(!full_tools.is_empty());
    assert!(full_tools.values().any(|read_only| !read_only));
    let expected = full_tools
        .into_iter()
        .filter(|(_, read_only)| *read_only)
        .collect::<BTreeMap<_, _>>();
    assert!(!expected.is_empty());
    assert_eq!(
        list_tools("read-only"),
        expected,
        "read-only must include every published read tool and exclude every modifying tool"
    );
}
