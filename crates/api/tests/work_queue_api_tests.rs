use axum::http::StatusCode;
use helpers::{create_test_pack, TestContext};
use serde_json::json;

use attune_common::{
    models::{WorkQueueBatchMode, WorkQueueUpdateStrategy},
    repositories::{
        action::{ActionRepository, CreateActionInput},
        work_queue::{CreateWorkQueueInput, WorkQueueRepository},
        Create,
    },
};

mod helpers;

async fn create_pack_with_action(
    ctx: &TestContext,
    pack_ref: &str,
    action_ref: &str,
) -> (
    attune_common::models::Pack,
    attune_common::models::action::Action,
) {
    let pack = create_test_pack(&ctx.pool, pack_ref)
        .await
        .expect("create test pack");
    let action = ActionRepository::create(
        &ctx.pool,
        CreateActionInput {
            r#ref: action_ref.to_string(),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: format!("Action {}", action_ref),
            description: Some("Queue dispatch action".to_string()),
            entrypoint: "main.py".to_string(),
            runtime: None,
            enabled: true,
            runtime_version_constraint: None,
            required_worker_runtimes: serde_json::json!({}),
            worker_selector: serde_json::json!({}),
            worker_tolerations: serde_json::json!([]),
            worker_affinity: serde_json::json!({}),
            param_schema: None,
            out_schema: None,
            is_adhoc: false,
            accesses_mcp: false,
            default_execution_permission_set_refs: Vec::new(),
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            timeout_seconds: None,
        },
    )
    .await
    .expect("create test action");

    (pack, action)
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn queue_api_supports_merge_patch_enqueue_and_pending_item_lifecycle() {
    let ctx = TestContext::new()
        .await
        .expect("test context")
        .with_auth()
        .await
        .expect("auth context");
    let token = ctx.token.as_deref();

    let (_pack, action) = create_pack_with_action(
        &ctx,
        &format!("queue_api_pack_{}", uuid::Uuid::new_v4().simple()),
        &format!("queue_api_pack.dispatch_{}", uuid::Uuid::new_v4().simple()),
    )
    .await;

    let queue_ref = format!("adhoc.queue_{}", uuid::Uuid::new_v4().simple());
    let create = ctx
        .post(
            "/api/v1/queues",
            json!({
                "ref": queue_ref,
                "label": "API Queue",
                "dispatch_action_ref": action.r#ref,
                "accepting_new_items": true,
                "allow_pending_update": true,
                "update_strategy": "merge_patch",
                "batch_mode": "batch",
                "item_schema": {
                    "customer": { "type": "string", "required": true },
                    "flags": { "type": "object" }
                },
                "config": {
                    "ack_contract": { "version": 2 }
                }
            }),
            token,
        )
        .await
        .expect("create queue");
    assert_eq!(create.status(), StatusCode::CREATED);

    let first_enqueue = ctx
        .post(
            &format!("/api/v1/queues/{}/items", queue_ref),
            json!({
                "item_key": "order-123",
                "priority": 9,
                "payload": {
                    "customer": "alice",
                    "flags": { "first": true }
                },
                "metadata": {
                    "attempt": 1
                }
            }),
            token,
        )
        .await
        .expect("enqueue first item");
    assert_eq!(first_enqueue.status(), StatusCode::CREATED);
    let first_body: serde_json::Value = first_enqueue.json().await.expect("enqueue body");
    let item_id = first_body["data"]["id"].as_i64().expect("queue item id");
    assert_eq!(first_body["data"]["enqueue_source"], "api");

    let merged_enqueue = ctx
        .post(
            &format!("/api/v1/queues/{}/items", queue_ref),
            json!({
                "item_key": "order-123",
                "payload": {
                    "flags": { "first": false, "second": true },
                    "status": "retrying"
                },
                "metadata": {
                    "worker": "api-test"
                }
            }),
            token,
        )
        .await
        .expect("enqueue merge patch item");
    assert_eq!(merged_enqueue.status(), StatusCode::OK);
    let merged_body: serde_json::Value = merged_enqueue.json().await.expect("merge body");
    assert_eq!(merged_body["data"]["id"].as_i64(), Some(item_id));
    assert_eq!(merged_body["data"]["priority"].as_i64(), Some(9));
    assert_eq!(merged_body["data"]["payload"]["customer"], "alice");
    assert_eq!(merged_body["data"]["payload"]["flags"]["first"], false);
    assert_eq!(merged_body["data"]["payload"]["flags"]["second"], true);
    assert_eq!(merged_body["data"]["payload"]["status"], "retrying");
    assert_eq!(merged_body["data"]["metadata"]["attempt"], 1);
    assert_eq!(merged_body["data"]["metadata"]["worker"], "api-test");
    assert_eq!(merged_body["data"]["enqueue_source"], "api");

    let update = ctx
        .put(
            &format!("/api/v1/queues/{}/items/{}", queue_ref, item_id),
            json!({
                "priority": 12,
                "payload": {
                    "customer": "bob"
                },
                "metadata": {
                    "manual": true
                }
            }),
            token,
        )
        .await
        .expect("update queue item");
    assert_eq!(update.status(), StatusCode::OK);
    let update_body: serde_json::Value = update.json().await.expect("update body");
    assert_eq!(update_body["data"]["priority"], 12);
    assert_eq!(update_body["data"]["payload"]["customer"], "bob");
    assert_eq!(update_body["data"]["metadata"]["manual"], true);

    let list = ctx
        .get(
            &format!(
                "/api/v1/queues/{}/items?statuses=queued&statuses=retry",
                queue_ref
            ),
            token,
        )
        .await
        .expect("list queue items");
    assert_eq!(list.status(), StatusCode::OK);
    let list_body: serde_json::Value = list.json().await.expect("list body");
    assert_eq!(list_body["pagination"]["total_items"].as_u64(), Some(1));
    assert_eq!(list_body["data"][0]["id"].as_i64(), Some(item_id));

    let list_comma_separated = ctx
        .get(
            &format!("/api/v1/queues/{}/items?statuses=queued,retry", queue_ref),
            token,
        )
        .await
        .expect("list queue items with comma separated statuses");
    assert_eq!(list_comma_separated.status(), StatusCode::OK);

    let delete = ctx
        .delete(
            &format!("/api/v1/queues/{}/items/{}", queue_ref, item_id),
            token,
        )
        .await
        .expect("delete queue item");
    assert_eq!(delete.status(), StatusCode::OK);

    let get_queue = ctx
        .get(&format!("/api/v1/queues/{}", queue_ref), token)
        .await
        .expect("get queue");
    assert_eq!(get_queue.status(), StatusCode::OK);
    let queue_body: serde_json::Value = get_queue.json().await.expect("queue body");
    assert_eq!(queue_body["data"]["batch_mode"], "batch");
    assert_eq!(
        queue_body["data"]["item_schema"]["customer"]["type"],
        "string"
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn queue_api_blocks_pack_managed_queue_mutations_but_lists_pack_queues() {
    let ctx = TestContext::new()
        .await
        .expect("test context")
        .with_auth()
        .await
        .expect("auth context");
    let token = ctx.token.as_deref();

    let (pack, action) = create_pack_with_action(
        &ctx,
        &format!("queue_pack_{}", uuid::Uuid::new_v4().simple()),
        &format!("queue_pack.dispatch_{}", uuid::Uuid::new_v4().simple()),
    )
    .await;

    let queue = WorkQueueRepository::create(
        &ctx.pool,
        CreateWorkQueueInput {
            r#ref: format!("{}.ops", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            is_adhoc: false,
            label: "Pack Queue".to_string(),
            description: Some("Pack-managed queue".to_string()),
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(action.id),
            dispatch_action_ref: action.r#ref.clone(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: WorkQueueUpdateStrategy::Replace,
            batch_mode: WorkQueueBatchMode::Single,
            item_schema: json!({
                "item": { "type": "object", "required": true }
            }),
            action_params: json!({
                "item": "{{ item }}"
            }),
            permission_set_refs: None,
            config: json!({}),
        },
    )
    .await
    .expect("create pack queue");

    let list = ctx
        .get(&format!("/api/v1/packs/{}/queues", pack.r#ref), token)
        .await
        .expect("list pack queues");
    assert_eq!(list.status(), StatusCode::OK);
    let list_body: serde_json::Value = list.json().await.expect("list body");
    assert!(list_body["data"]
        .as_array()
        .expect("queue list")
        .iter()
        .any(|row| row["ref"] == queue.r#ref));

    let update = ctx
        .put(
            &format!("/api/v1/queues/{}", queue.r#ref),
            json!({
                "label": "Should fail"
            }),
            token,
        )
        .await
        .expect("update pack queue");
    assert_eq!(update.status(), StatusCode::FORBIDDEN);

    let toggle_processing = ctx
        .put(
            &format!("/api/v1/queues/{}", queue.r#ref),
            json!({
                "enabled": false,
                "accepting_new_items": false
            }),
            token,
        )
        .await
        .expect("toggle pack queue operational flags");
    assert_eq!(toggle_processing.status(), StatusCode::OK);

    let toggle_body: serde_json::Value = toggle_processing.json().await.expect("toggle body");
    assert_eq!(toggle_body["data"]["enabled"], false);
    assert_eq!(toggle_body["data"]["accepting_new_items"], false);

    let delete = ctx
        .delete(&format!("/api/v1/queues/{}", queue.r#ref), token)
        .await
        .expect("delete pack queue");
    assert_eq!(delete.status(), StatusCode::FORBIDDEN);
}
