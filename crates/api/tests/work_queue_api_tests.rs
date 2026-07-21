use axum::http::StatusCode;
use helpers::{create_test_pack, Result, TestContext};
use serde_json::json;

use attune_common::{
    models::{
        ActionReferenceVisibility, ExecutionStatus, WorkQueueBatchMode, WorkQueueDispatchStatus,
        WorkQueueUpdateStrategy,
    },
    repositories::{
        action::{ActionRepository, CreateActionInput},
        execution::{CreateExecutionInput, ExecutionRepository},
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        work_queue::{
            CreateWorkQueueDispatchInput, CreateWorkQueueInput, CreateWorkQueueItemInput,
            UpdateWorkQueueItemInput, WorkQueueDispatchRepository, WorkQueueItemRepository,
            WorkQueueRepository,
        },
        Create, FindById, FindByRef, Update,
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

async fn register_scoped_user(
    ctx: &TestContext,
    login: &str,
    grants: serde_json::Value,
) -> Result<String> {
    let response = ctx
        .post(
            "/auth/register",
            json!({
                "login": login,
                "password": "TestPassword123!",
                "display_name": format!("Queue Visibility User {}", login),
            }),
            None,
        )
        .await?;

    assert!(
        response.status() == StatusCode::OK || response.status() == StatusCode::CREATED,
        "expected 200/201 from /auth/register, got {}",
        response.status()
    );
    let body: serde_json::Value = response.json().await?;
    let token = body["data"]["access_token"]
        .as_str()
        .expect("missing access token")
        .to_string();

    let identity = IdentityRepository::find_by_login(&ctx.pool, login)
        .await?
        .expect("registered identity should exist");
    let permset = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("test.queue_visibility_{}", uuid::Uuid::new_v4().simple()),
            pack: None,
            pack_ref: None,
            label: Some("Queue visibility test grants".to_string()),
            description: None,
            grants,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: permset.id,
        },
    )
    .await?;

    Ok(token)
}

async fn create_queue_with_visibility(
    ctx: &TestContext,
    pack_ref: &str,
    queue_name: &str,
    visibility: ActionReferenceVisibility,
    allowed_pack_refs: Vec<String>,
) -> attune_common::models::work_queue::WorkQueue {
    let (pack, action) = create_pack_with_action(
        ctx,
        pack_ref,
        &format!("{}.dispatch_{}", pack_ref, uuid::Uuid::new_v4().simple()),
    )
    .await;

    WorkQueueRepository::create(
        &ctx.pool,
        CreateWorkQueueInput {
            r#ref: format!("{}.{}", pack.r#ref, queue_name),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            is_adhoc: false,
            label: format!("Queue {}", queue_name),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(action.id),
            dispatch_action_ref: action.r#ref,
            default_priority: 0,
            allow_pending_update: true,
            update_strategy: WorkQueueUpdateStrategy::Replace,
            batch_mode: WorkQueueBatchMode::Single,
            item_schema: json!({}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            config: json!({}),
            reference_visibility: visibility,
            reference_allowed_pack_refs: allowed_pack_refs,
        },
    )
    .await
    .expect("create queue")
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn queue_api_bulk_enqueues_items() {
    let ctx = TestContext::new()
        .await
        .expect("test context")
        .with_auth()
        .await
        .expect("auth context");
    let token = ctx.token.as_deref();

    let pack_ref = format!("bulk_enqueue_pack_{}", uuid::Uuid::new_v4().simple());
    let action_ref = format!("{}.dispatch_{}", pack_ref, uuid::Uuid::new_v4().simple());
    let (_pack, action) = create_pack_with_action(&ctx, &pack_ref, &action_ref).await;

    let queue_ref = format!("adhoc.bulk_enqueue_{}", uuid::Uuid::new_v4().simple());
    let create = ctx
        .post(
            "/api/v1/queues",
            json!({
                "ref": queue_ref,
                "label": "Bulk Enqueue Queue",
                "dispatch_action_ref": action.r#ref,
                "item_schema": {
                    "customer": { "type": "string", "required": true }
                }
            }),
            token,
        )
        .await
        .expect("create queue");
    assert_eq!(create.status(), StatusCode::CREATED);

    let response = ctx
        .post(
            &format!("/api/v1/queues/{}/items/bulk", queue_ref),
            json!({
                "items": [
                    {
                        "item_key": "bulk-1",
                        "priority": 5,
                        "payload": { "customer": "alice" }
                    },
                    {
                        "item_key": "bulk-2",
                        "payload": { "customer": "bob" }
                    }
                ]
            }),
            token,
        )
        .await
        .expect("bulk enqueue queue items");
    assert_eq!(response.status(), StatusCode::CREATED);

    let body: serde_json::Value = response.json().await.expect("bulk enqueue body");
    assert_eq!(body["data"]["created_count"].as_u64(), Some(2));
    assert_eq!(body["data"]["updated_count"].as_u64(), Some(0));
    assert_eq!(body["data"]["items"].as_array().map(Vec::len), Some(2));

    let rejected = ctx
        .post(
            &format!("/api/v1/queues/{}/items/bulk", queue_ref),
            json!({
                "items": [
                    {
                        "item_key": "rolled-back-item",
                        "payload": { "customer": "carol" }
                    },
                    {
                        "item_key": "invalid-trace-tag",
                        "payload": { "customer": "dave" },
                        "trace_tag": "\n"
                    }
                ]
            }),
            token,
        )
        .await
        .expect("reject invalid bulk enqueue");
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);

    let queue = WorkQueueRepository::find_by_ref(&ctx.pool, &queue_ref)
        .await
        .expect("find queue")
        .expect("queue exists");
    let rolled_back_items =
        WorkQueueItemRepository::find_pending_by_item_key(&ctx.pool, queue.id, "rolled-back-item")
            .await
            .expect("find rolled-back item");
    assert!(rolled_back_items.is_empty());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn queue_reference_visibility_filters_discovery_by_referencing_pack() {
    let ctx = TestContext::new()
        .await
        .expect("test context")
        .with_auth()
        .await
        .expect("auth context");

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let owner_pack = format!("queue_owner_{}", suffix);
    let allowed_pack = format!("queue_allowed_{}", suffix);
    let other_pack = format!("queue_other_{}", suffix);

    let public_queue = create_queue_with_visibility(
        &ctx,
        &format!("{}_public", owner_pack),
        "public_queue",
        ActionReferenceVisibility::Public,
        Vec::new(),
    )
    .await;
    let private_queue = create_queue_with_visibility(
        &ctx,
        &owner_pack,
        "private_queue",
        ActionReferenceVisibility::Private,
        Vec::new(),
    )
    .await;
    let restricted_queue = create_queue_with_visibility(
        &ctx,
        &format!("{}_restricted", owner_pack),
        "restricted_queue",
        ActionReferenceVisibility::Restricted,
        vec![allowed_pack.clone()],
    )
    .await;

    let token = register_scoped_user(
        &ctx,
        &format!("queue_reader_{}", suffix),
        json!([
            {
                "resource": "queues",
                "actions": ["read"]
            }
        ]),
    )
    .await
    .expect("scoped reader");

    let list_default = ctx
        .get("/api/v1/queues", Some(&token))
        .await
        .expect("list queues");
    assert_eq!(list_default.status(), StatusCode::OK);
    let list_default_body: serde_json::Value = list_default.json().await.expect("list body");
    let default_refs: Vec<&str> = list_default_body["data"]
        .as_array()
        .expect("queue array")
        .iter()
        .filter_map(|queue| queue["ref"].as_str())
        .collect();
    assert!(default_refs.contains(&public_queue.r#ref.as_str()));
    assert!(!default_refs.contains(&private_queue.r#ref.as_str()));
    assert!(!default_refs.contains(&restricted_queue.r#ref.as_str()));

    let list_allowed = ctx
        .get(
            &format!("/api/v1/queues?referencing_pack_ref={}", allowed_pack),
            Some(&token),
        )
        .await
        .expect("list allowed queues");
    assert_eq!(list_allowed.status(), StatusCode::OK);
    let list_allowed_body: serde_json::Value = list_allowed.json().await.expect("list body");
    let allowed_refs: Vec<&str> = list_allowed_body["data"]
        .as_array()
        .expect("queue array")
        .iter()
        .filter_map(|queue| queue["ref"].as_str())
        .collect();
    assert!(allowed_refs.contains(&public_queue.r#ref.as_str()));
    assert!(!allowed_refs.contains(&private_queue.r#ref.as_str()));
    assert!(allowed_refs.contains(&restricted_queue.r#ref.as_str()));

    let list_other = ctx
        .get(
            &format!("/api/v1/queues?referencing_pack_ref={}", other_pack),
            Some(&token),
        )
        .await
        .expect("list other queues");
    assert_eq!(list_other.status(), StatusCode::OK);
    let list_other_body: serde_json::Value = list_other.json().await.expect("list body");
    let other_refs: Vec<&str> = list_other_body["data"]
        .as_array()
        .expect("queue array")
        .iter()
        .filter_map(|queue| queue["ref"].as_str())
        .collect();
    assert!(other_refs.contains(&public_queue.r#ref.as_str()));
    assert!(!other_refs.contains(&private_queue.r#ref.as_str()));
    assert!(!other_refs.contains(&restricted_queue.r#ref.as_str()));

    let get_restricted_allowed = ctx
        .get(
            &format!(
                "/api/v1/queues/{}?referencing_pack_ref={}",
                restricted_queue.r#ref, allowed_pack
            ),
            Some(&token),
        )
        .await
        .expect("get restricted queue");
    assert_eq!(get_restricted_allowed.status(), StatusCode::OK);

    let get_restricted_other = ctx
        .get(
            &format!(
                "/api/v1/queues/{}?referencing_pack_ref={}",
                restricted_queue.r#ref, other_pack
            ),
            Some(&token),
        )
        .await
        .expect("get restricted queue as other pack");
    assert_eq!(get_restricted_other.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn private_queue_item_submission_requires_constrained_item_grant() {
    let ctx = TestContext::new()
        .await
        .expect("test context")
        .with_auth()
        .await
        .expect("auth context");

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let public_queue = create_queue_with_visibility(
        &ctx,
        &format!("queue_item_public_{}", suffix),
        "inbox",
        ActionReferenceVisibility::Public,
        Vec::new(),
    )
    .await;
    let private_queue = create_queue_with_visibility(
        &ctx,
        &format!("queue_item_private_{}", suffix),
        "inbox",
        ActionReferenceVisibility::Private,
        Vec::new(),
    )
    .await;

    let broad_item_token = register_scoped_user(
        &ctx,
        &format!("queue_item_broad_{}", suffix),
        json!([
            {
                "resource": "queue_items",
                "actions": ["create"]
            }
        ]),
    )
    .await
    .expect("broad queue item user");

    let enqueue_public = ctx
        .post(
            &format!("/api/v1/queues/{}/items", public_queue.r#ref),
            json!({ "payload": { "id": 1 } }),
            Some(&broad_item_token),
        )
        .await
        .expect("enqueue public queue");
    assert_eq!(enqueue_public.status(), StatusCode::CREATED);

    let enqueue_private_broad = ctx
        .post(
            &format!("/api/v1/queues/{}/items", private_queue.r#ref),
            json!({ "payload": { "id": 2 } }),
            Some(&broad_item_token),
        )
        .await
        .expect("enqueue private queue with broad grant");
    assert_eq!(enqueue_private_broad.status(), StatusCode::FORBIDDEN);

    let constrained_item_token = register_scoped_user(
        &ctx,
        &format!("queue_item_constrained_{}", suffix),
        json!([
            {
                "resource": "queue_items",
                "actions": ["create"],
                "constraints": {
                    "refs": [private_queue.r#ref]
                }
            }
        ]),
    )
    .await
    .expect("constrained queue item user");

    let enqueue_private_constrained = ctx
        .post(
            &format!("/api/v1/queues/{}/items", private_queue.r#ref),
            json!({ "payload": { "id": 3 } }),
            Some(&constrained_item_token),
        )
        .await
        .expect("enqueue private queue with constrained grant");
    assert_eq!(enqueue_private_constrained.status(), StatusCode::CREATED);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn api_created_pack_owned_queue_is_api_managed() {
    let ctx = TestContext::new()
        .await
        .expect("test context")
        .with_auth()
        .await
        .expect("auth context");
    let token = ctx.token.as_deref();

    let pack_ref = format!("queue_api_owned_{}", uuid::Uuid::new_v4().simple());
    let action_ref = format!("{}.dispatch_{}", pack_ref, uuid::Uuid::new_v4().simple());
    let (pack, action) = create_pack_with_action(&ctx, &pack_ref, &action_ref).await;
    let queue_ref = format!("{}.api_queue_{}", pack.r#ref, uuid::Uuid::new_v4().simple());

    let create = ctx
        .post(
            "/api/v1/queues",
            json!({
                "ref": queue_ref,
                "pack_ref": pack.r#ref,
                "label": "API-managed Queue",
                "dispatch_action_ref": action.r#ref
            }),
            token,
        )
        .await
        .expect("create queue");
    assert_eq!(create.status(), StatusCode::CREATED);
    let create_body: serde_json::Value = create.json().await.expect("create body");
    assert_eq!(create_body["data"]["is_adhoc"], true);
    assert_eq!(create_body["data"]["pack_ref"], pack.r#ref);

    let api_managed = ctx
        .get("/api/v1/queues?is_adhoc=true", token)
        .await
        .expect("list API-managed queues");
    assert_eq!(api_managed.status(), StatusCode::OK);
    let api_managed_body: serde_json::Value = api_managed
        .json()
        .await
        .expect("API-managed queue list body");
    assert!(api_managed_body["data"]
        .as_array()
        .expect("queue list")
        .iter()
        .any(|queue| queue["ref"] == queue_ref));

    let update = ctx
        .put(
            &format!("/api/v1/queues/{}", queue_ref),
            json!({ "label": "Updated API-managed Queue" }),
            token,
        )
        .await
        .expect("update queue");
    assert_eq!(update.status(), StatusCode::OK);
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
async fn queue_api_supports_jsonpath_preview_and_bulk_operations() {
    let ctx = TestContext::new()
        .await
        .expect("test context")
        .with_auth()
        .await
        .expect("auth context");
    let token = ctx.token.as_deref();

    let pack_ref = format!("queue_selector_pack_{}", uuid::Uuid::new_v4().simple());
    let action_ref = format!("{}.dispatch_{}", pack_ref, uuid::Uuid::new_v4().simple());
    let (_pack, action) = create_pack_with_action(&ctx, &pack_ref, &action_ref).await;

    let queue_ref = format!("adhoc.selector_{}", uuid::Uuid::new_v4().simple());
    let create = ctx
        .post(
            "/api/v1/queues",
            json!({
                "ref": queue_ref,
                "label": "Selector Queue",
                "dispatch_action_ref": action.r#ref,
                "item_schema": {
                    "customer": { "type": "string", "required": true },
                    "flags": { "type": "object" }
                }
            }),
            token,
        )
        .await
        .expect("create queue");
    assert_eq!(create.status(), StatusCode::CREATED);

    let alice_one = ctx
        .post(
            &format!("/api/v1/queues/{}/items", queue_ref),
            json!({
                "item_key": "alice-1",
                "priority": 5,
                "payload": { "customer": "alice", "flags": { "source": "first" } }
            }),
            token,
        )
        .await
        .expect("enqueue alice one");
    assert_eq!(alice_one.status(), StatusCode::CREATED);
    let alice_one_body: serde_json::Value = alice_one.json().await.expect("alice one body");
    let alice_one_id = alice_one_body["data"]["id"].as_i64().expect("alice one id");

    let alice_two = ctx
        .post(
            &format!("/api/v1/queues/{}/items", queue_ref),
            json!({
                "item_key": "alice-2",
                "priority": 1,
                "payload": { "customer": "alice", "flags": { "source": "second" } }
            }),
            token,
        )
        .await
        .expect("enqueue alice two");
    assert_eq!(alice_two.status(), StatusCode::CREATED);
    let alice_two_body: serde_json::Value = alice_two.json().await.expect("alice two body");
    let alice_two_id = alice_two_body["data"]["id"].as_i64().expect("alice two id");

    let bob = ctx
        .post(
            &format!("/api/v1/queues/{}/items", queue_ref),
            json!({
                "item_key": "bob-1",
                "priority": 10,
                "payload": { "customer": "bob" }
            }),
            token,
        )
        .await
        .expect("enqueue bob");
    assert_eq!(bob.status(), StatusCode::CREATED);
    let bob_body: serde_json::Value = bob.json().await.expect("bob body");
    let bob_id = bob_body["data"]["id"].as_i64().expect("bob id");

    let bob_item = WorkQueueItemRepository::find_by_id(&ctx.pool, bob_id)
        .await
        .expect("find bob")
        .expect("bob item");
    let completed_bob = WorkQueueItemRepository::update(
        &ctx.pool,
        bob_item.id,
        UpdateWorkQueueItemInput {
            status: Some(attune_common::models::WorkQueueItemStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .expect("complete bob");
    assert_eq!(
        completed_bob.status,
        attune_common::models::WorkQueueItemStatus::Completed
    );

    let preview = ctx
        .post(
            &format!("/api/v1/queues/{}/items/query/preview", queue_ref),
            json!({
                "selector": {
                    "path": "$.payload.customer ? (@ == $target)",
                    "vars": { "target": "alice" }
                },
                "limit": 100
            }),
            token,
        )
        .await
        .expect("preview selector");
    assert_eq!(preview.status(), StatusCode::OK);
    let preview_body: serde_json::Value = preview.json().await.expect("preview body");
    assert_eq!(preview_body["data"]["matched_count"].as_u64(), Some(2));
    assert_eq!(preview_body["data"]["preview_count"].as_u64(), Some(2));

    let patch = ctx
        .post(
            &format!("/api/v1/queues/{}/items/query/apply", queue_ref),
            json!({
                "selector": {
                    "path": "$.payload.customer ? (@ == $target)",
                    "vars": { "target": "alice" }
                },
                "operation": "patch_payload",
                "payload_patch": {
                    "flags": { "bulk": true }
                },
                "preview_limit": 100
            }),
            token,
        )
        .await
        .expect("patch selected items");
    assert_eq!(patch.status(), StatusCode::OK);
    let patch_body: serde_json::Value = patch.json().await.expect("patch body");
    assert_eq!(patch_body["data"]["matched_count"].as_u64(), Some(2));
    assert_eq!(patch_body["data"]["affected_count"].as_u64(), Some(2));

    for item_id in [alice_one_id, alice_two_id] {
        let item = WorkQueueItemRepository::find_by_id(&ctx.pool, item_id)
            .await
            .expect("find patched item")
            .expect("patched item");
        assert_eq!(item.payload["customer"], "alice");
        assert_eq!(item.payload["flags"]["bulk"], true);
    }

    let reprioritize = ctx
        .post(
            &format!("/api/v1/queues/{}/items/query/apply", queue_ref),
            json!({
                "selector": {
                    "path": "$.payload.flags.bulk ? (@ == true)",
                    "vars": {}
                },
                "operation": "reprioritize",
                "priority": 42,
                "preview_limit": 100
            }),
            token,
        )
        .await
        .expect("reprioritize selected items");
    assert_eq!(reprioritize.status(), StatusCode::OK);
    let reprioritize_body: serde_json::Value =
        reprioritize.json().await.expect("reprioritize body");
    assert_eq!(
        reprioritize_body["data"]["affected_count"].as_u64(),
        Some(2)
    );

    let cancel = ctx
        .post(
            &format!("/api/v1/queues/{}/items/query/apply", queue_ref),
            json!({
                "selector": {
                    "path": "$.payload.flags.bulk ? (@ == true)",
                    "vars": {}
                },
                "operation": "cancel",
                "preview_limit": 100
            }),
            token,
        )
        .await
        .expect("cancel selected items");
    assert_eq!(cancel.status(), StatusCode::OK);
    let cancel_body: serde_json::Value = cancel.json().await.expect("cancel body");
    assert_eq!(cancel_body["data"]["affected_count"].as_u64(), Some(2));

    for item_id in [alice_one_id, alice_two_id] {
        let item = WorkQueueItemRepository::find_by_id(&ctx.pool, item_id)
            .await
            .expect("find cancelled item")
            .expect("cancelled item");
        assert_eq!(item.priority, 42);
        assert_eq!(
            item.status,
            attune_common::models::WorkQueueItemStatus::Cancelled
        );
    }

    let invalid_selector = ctx
        .post(
            &format!("/api/v1/queues/{}/items/query/preview", queue_ref),
            json!({
                "selector": {
                    "path": "$.payload[",
                    "vars": {}
                }
            }),
            token,
        )
        .await
        .expect("invalid selector");
    assert_eq!(invalid_selector.status(), StatusCode::BAD_REQUEST);
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
            trace_tag_template: None,
            permission_set_refs: None,
            config: json!({}),
            reference_visibility: ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: Vec::new(),
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

async fn create_trace_queue_item(
    ctx: &TestContext,
    queue: &attune_common::models::work_queue::WorkQueue,
    payload: serde_json::Value,
) -> attune_common::models::WorkQueueItem {
    use attune_common::repositories::Create;
    WorkQueueItemRepository::create(
        &ctx.pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: None,
            priority: 0,
            status: attune_common::models::WorkQueueItemStatus::Queued,
            payload,
            metadata: json!({}),
            trace_tag: None,
            enqueue_source: "api".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .expect("create trace queue item")
}

fn trace_queue_item_ids(body: &serde_json::Value) -> Vec<i64> {
    body["data"]["queue_items"]
        .as_array()
        .expect("queue_items array")
        .iter()
        .map(|item| item["id"].as_i64().expect("queue item id"))
        .collect()
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn trace_report_enforces_per_queue_item_visibility() {
    let ctx = TestContext::new()
        .await
        .expect("test context")
        .with_auth()
        .await
        .expect("auth context");

    let suffix = uuid::Uuid::new_v4().simple().to_string();

    let public_queue = create_queue_with_visibility(
        &ctx,
        &format!("trace_pub_{}", suffix),
        "inbox",
        ActionReferenceVisibility::Public,
        Vec::new(),
    )
    .await;
    let private_queue = create_queue_with_visibility(
        &ctx,
        &format!("trace_priv_{}", suffix),
        "inbox",
        ActionReferenceVisibility::Private,
        Vec::new(),
    )
    .await;

    let public_item = create_trace_queue_item(&ctx, &public_queue, json!({ "id": 1 })).await;
    let private_item = create_trace_queue_item(&ctx, &private_queue, json!({ "id": 2 })).await;

    // Viewer has the reads required to load a trace report but no per-queue
    // management access, so private-queue items must be filtered out.
    let viewer_token = register_scoped_user(
        &ctx,
        &format!("trace_viewer_{}", suffix),
        json!([
            { "resource": "executions", "actions": ["read"] },
            { "resource": "enforcements", "actions": ["read"] },
            { "resource": "events", "actions": ["read"] },
            { "resource": "queue_items", "actions": ["read"] }
        ]),
    )
    .await
    .expect("viewer user");

    let public_trace = format!("{}.{}", public_queue.r#ref, public_item.id);
    let public_response = ctx
        .get(
            &format!("/api/v1/traces/{}", public_trace),
            Some(&viewer_token),
        )
        .await
        .expect("public trace report");
    assert_eq!(public_response.status(), StatusCode::OK);
    let public_body: serde_json::Value = public_response.json().await.expect("public body");
    assert_eq!(
        trace_queue_item_ids(&public_body),
        vec![public_item.id],
        "public queue item should appear in its origin trace report"
    );

    let private_trace = format!("{}.{}", private_queue.r#ref, private_item.id);
    let private_response = ctx
        .get(
            &format!("/api/v1/traces/{}", private_trace),
            Some(&viewer_token),
        )
        .await
        .expect("private trace report");
    assert_eq!(private_response.status(), StatusCode::OK);
    let private_body: serde_json::Value = private_response.json().await.expect("private body");
    assert!(
        trace_queue_item_ids(&private_body).is_empty(),
        "private queue item must be excluded from trace report for caller without queue visibility"
    );

    // A caller with queue management access on the private queue can see the item.
    let manager_token = register_scoped_user(
        &ctx,
        &format!("trace_manager_{}", suffix),
        json!([
            { "resource": "executions", "actions": ["read"] },
            { "resource": "enforcements", "actions": ["read"] },
            { "resource": "events", "actions": ["read"] },
            { "resource": "queue_items", "actions": ["read"] },
            { "resource": "queues", "actions": ["read", "update"] }
        ]),
    )
    .await
    .expect("manager user");

    let manager_response = ctx
        .get(
            &format!("/api/v1/traces/{}", private_trace),
            Some(&manager_token),
        )
        .await
        .expect("manager private trace report");
    assert_eq!(manager_response.status(), StatusCode::OK);
    let manager_body: serde_json::Value = manager_response.json().await.expect("manager body");
    assert_eq!(
        trace_queue_item_ids(&manager_body),
        vec![private_item.id],
        "queue manager should see the private queue item in the trace report"
    );
}

async fn create_trace_execution(
    ctx: &TestContext,
    action_ref: &str,
    trace_tag: &str,
) -> attune_common::models::Execution {
    ExecutionRepository::create(
        &ctx.pool,
        CreateExecutionInput {
            action: None,
            action_ref: action_ref.to_string(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Completed,
            result: None,
            timeout_seconds: None,
            trace_tag: Some(trace_tag.to_string()),
            workflow_task: None,
        },
    )
    .await
    .expect("create trace execution")
}

async fn create_trace_dispatch(
    ctx: &TestContext,
    queue: &attune_common::models::work_queue::WorkQueue,
    execution_id: i64,
) -> attune_common::models::work_queue::WorkQueueDispatch {
    WorkQueueDispatchRepository::create(
        &ctx.pool,
        CreateWorkQueueDispatchInput {
            id: None,
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            execution: execution_id,
            status: WorkQueueDispatchStatus::Dispatched,
            leased_item_count: 1,
        },
    )
    .await
    .expect("create trace dispatch")
}

fn trace_dispatch_ids(body: &serde_json::Value) -> Vec<i64> {
    body["data"]["queue_dispatches"]
        .as_array()
        .expect("queue_dispatches array")
        .iter()
        .map(|dispatch| dispatch["id"].as_i64().expect("dispatch id"))
        .collect()
}

fn trace_origins(body: &serde_json::Value) -> Vec<String> {
    body["data"]["origins"]
        .as_array()
        .expect("origins array")
        .iter()
        .map(|origin| origin.as_str().expect("origin string").to_string())
        .collect()
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn trace_report_enforces_per_queue_dispatch_visibility() {
    let ctx = TestContext::new()
        .await
        .expect("test context")
        .with_auth()
        .await
        .expect("auth context");

    let suffix = uuid::Uuid::new_v4().simple().to_string();

    let private_queue = create_queue_with_visibility(
        &ctx,
        &format!("trace_disp_priv_{}", suffix),
        "inbox",
        ActionReferenceVisibility::Private,
        Vec::new(),
    )
    .await;

    // One execution carries the trace tag and is the source for the dispatch.
    let trace_tag = format!("manual.exec.{}", suffix);
    let execution =
        create_trace_execution(&ctx, &private_queue.dispatch_action_ref, &trace_tag).await;
    let dispatch = create_trace_dispatch(&ctx, &private_queue, execution.id).await;

    // Viewer can load a trace report but lacks per-queue management access, so
    // the private-queue dispatch must be filtered out and the work-queue origin
    // must not be reported.
    let viewer_token = register_scoped_user(
        &ctx,
        &format!("trace_disp_viewer_{}", suffix),
        json!([
            { "resource": "executions", "actions": ["read"] },
            { "resource": "enforcements", "actions": ["read"] },
            { "resource": "events", "actions": ["read"] },
            { "resource": "queue_items", "actions": ["read"] }
        ]),
    )
    .await
    .expect("viewer user");

    let viewer_response = ctx
        .get(
            &format!("/api/v1/traces/{}", trace_tag),
            Some(&viewer_token),
        )
        .await
        .expect("viewer trace report");
    assert_eq!(viewer_response.status(), StatusCode::OK);
    let viewer_body: serde_json::Value = viewer_response.json().await.expect("viewer body");
    assert!(
        trace_dispatch_ids(&viewer_body).is_empty(),
        "private queue dispatch must be excluded for caller without queue visibility"
    );
    let viewer_origins = trace_origins(&viewer_body);
    assert!(
        !viewer_origins.contains(&"work_queue_item".to_string()),
        "work_queue_item origin must not be reported when all queue entities are filtered out"
    );
    assert!(
        viewer_origins.contains(&"manual_execution".to_string()),
        "execution with no visible queue entities should report manual_execution origin"
    );

    // A caller with queue management access on the private queue sees the dispatch.
    let manager_token = register_scoped_user(
        &ctx,
        &format!("trace_disp_manager_{}", suffix),
        json!([
            { "resource": "executions", "actions": ["read"] },
            { "resource": "enforcements", "actions": ["read"] },
            { "resource": "events", "actions": ["read"] },
            { "resource": "queue_items", "actions": ["read"] },
            { "resource": "queues", "actions": ["read", "update"] }
        ]),
    )
    .await
    .expect("manager user");

    let manager_response = ctx
        .get(
            &format!("/api/v1/traces/{}", trace_tag),
            Some(&manager_token),
        )
        .await
        .expect("manager trace report");
    assert_eq!(manager_response.status(), StatusCode::OK);
    let manager_body: serde_json::Value = manager_response.json().await.expect("manager body");
    assert_eq!(
        trace_dispatch_ids(&manager_body),
        vec![dispatch.id],
        "queue manager should see the private queue dispatch in the trace report"
    );
    assert!(
        trace_origins(&manager_body).contains(&"work_queue_item".to_string()),
        "work_queue_item origin should be reported when a queue dispatch is visible"
    );
}
