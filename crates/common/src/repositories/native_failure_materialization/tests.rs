use super::*;
use serde_json::json;

#[test]
fn generated_failure_preserves_existing_shape_and_literal_task_names() {
    let name = "{{ keystore['foreign.never_parse'] }}".to_owned();
    assert_eq!(
        failure_metadata(&["ordinary".into(), name.clone()]),
        json!({
            "error":format!("Workflow failed: 2 task(s) failed: ordinary, {name}"), "succeeded":false,
        })
    );
}

#[test]
fn normal_failure_scope_does_not_claim_cache_wait_iteration_or_successors() {
    let normal = json!({"tasks":[{"name":"run","action":"pack.run","next":[{"when":"{{ failed() }}","publish":[{"note":"literal"}],"do":[]}]}]});
    assert!(ordinary_terminal_graph(&normal).unwrap());
    for field in ["with_items", "iterate_cache", "wait_for"] {
        let mut outside = normal.clone();
        outside["tasks"][0][field] = json!({"source":"not this owner"});
        assert!(!ordinary_terminal_graph(&outside).unwrap());
    }
    let mut successor = normal;
    successor["tasks"][0]["next"][0]["do"] = json!(["next"]);
    assert!(!ordinary_terminal_graph(&successor).unwrap());
}

#[test]
fn failure_scope_does_not_mistake_payload_names_for_graph_controls() {
    let graph = json!({"tasks":[{"name":"run","action":"pack.run","input":{"wait_for":"literal","iterate_cache":"literal"}}]});
    assert!(ordinary_terminal_graph(&graph).unwrap());
}
