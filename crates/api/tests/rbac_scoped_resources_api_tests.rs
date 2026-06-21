use axum::http::StatusCode;
use helpers::*;
use serde_json::json;

use attune_common::{
    auth::jwt::STANDARD_EXECUTION_ACCESS_REF,
    models::{
        enums::{ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType},
        ActionReferenceVisibility, WorkQueueBatchMode, WorkQueueItemStatus,
        WorkQueueUpdateStrategy,
    },
    repositories::{
        action::{ActionRepository, CreateActionInput},
        artifact::{ArtifactRepository, CreateArtifactInput},
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        key::{CreateKeyInput, KeyRepository},
        work_queue::{
            CreateWorkQueueInput, UpdateWorkQueueItemInput, WorkQueueItemRepository,
            WorkQueueRepository,
        },
        Create, FindByRef, Update,
    },
};

mod helpers;

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
                "display_name": format!("Scoped User {}", login),
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
            r#ref: format!("test.scoped_{}", uuid::Uuid::new_v4().simple()),
            pack: None,
            pack_ref: None,
            label: Some("Scoped Test Permission Set".to_string()),
            description: Some("Scoped test grants".to_string()),
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

    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;

    Ok(token)
}

async fn create_pack_with_action(
    ctx: &TestContext,
    pack_ref: &str,
    action_ref: &str,
) -> Result<(
    attune_common::models::Pack,
    attune_common::models::action::Action,
)> {
    let pack = create_test_pack(&ctx.pool, pack_ref).await?;
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
    .await?;

    Ok((pack, action))
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_scoped_key_permissions_enforce_owner_refs() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let token = register_scoped_user(
        &ctx,
        &format!("scoped_keys_{}", uuid::Uuid::new_v4().simple()),
        json!([
            {
                "resource": "keys",
                "actions": ["read"],
                "constraints": {
                    "owner_types": ["pack"],
                    "owner_refs": ["python_example"]
                }
            }
        ]),
    )
    .await
    .expect("Failed to register scoped user");

    let allowed_pack = create_test_pack(&ctx.pool, "python_example")
        .await
        .expect("Failed to create allowed pack");
    let blocked_pack = create_test_pack(&ctx.pool, "other_pack")
        .await
        .expect("Failed to create blocked pack");

    KeyRepository::create(
        &ctx.pool,
        CreateKeyInput {
            r#ref: format!("python_example_key_{}", uuid::Uuid::new_v4().simple()),
            owner_type: OwnerType::Pack,
            owner: None,
            owner_identity: None,
            owner_pack: Some(allowed_pack.id),
            owner_pack_ref: Some("python_example".to_string()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Python Example Key".to_string(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!("allowed"),
        },
    )
    .await
    .expect("Failed to create scoped key");

    let blocked_key = KeyRepository::create(
        &ctx.pool,
        CreateKeyInput {
            r#ref: format!("other_pack_key_{}", uuid::Uuid::new_v4().simple()),
            owner_type: OwnerType::Pack,
            owner: None,
            owner_identity: None,
            owner_pack: Some(blocked_pack.id),
            owner_pack_ref: Some("other_pack".to_string()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Other Pack Key".to_string(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!("blocked"),
        },
    )
    .await
    .expect("Failed to create blocked key");

    let allowed_list = ctx
        .get("/api/v1/keys", Some(&token))
        .await
        .expect("Failed to list keys");
    assert_eq!(allowed_list.status(), StatusCode::OK);
    let allowed_body: serde_json::Value = allowed_list.json().await.expect("Invalid key list");
    assert_eq!(
        allowed_body["items"]
            .as_array()
            .expect("expected items list")
            .len(),
        1
    );
    assert_eq!(allowed_body["items"][0]["owner_type"], "pack");

    let blocked_get = ctx
        .get(&format!("/api/v1/keys/{}", blocked_key.r#ref), Some(&token))
        .await
        .expect("Failed to fetch blocked key");
    assert_eq!(blocked_get.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_scoped_artifact_permissions_enforce_owner_refs() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let token = register_scoped_user(
        &ctx,
        &format!("scoped_artifacts_{}", uuid::Uuid::new_v4().simple()),
        json!([
            {
                "resource": "artifacts",
                "actions": ["read", "create"],
                "constraints": {
                    "owner_types": ["pack"],
                    "owner_refs": ["python_example"]
                }
            }
        ]),
    )
    .await
    .expect("Failed to register scoped user");

    let allowed_artifact = ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: format!("python_example.allowed_{}", uuid::Uuid::new_v4().simple()),
            scope: OwnerType::Pack,
            owner: "python_example".to_string(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 5,
            name: Some("Allowed Artifact".to_string()),
            description: None,
            content_type: Some("text/plain".to_string()),
            data: None,
        },
    )
    .await
    .expect("Failed to create allowed artifact");

    let blocked_artifact = ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: format!("other_pack.blocked_{}", uuid::Uuid::new_v4().simple()),
            scope: OwnerType::Pack,
            owner: "other_pack".to_string(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 5,
            name: Some("Blocked Artifact".to_string()),
            description: None,
            content_type: Some("text/plain".to_string()),
            data: None,
        },
    )
    .await
    .expect("Failed to create blocked artifact");

    let allowed_get = ctx
        .get(
            &format!("/api/v1/artifacts/{}", allowed_artifact.id),
            Some(&token),
        )
        .await
        .expect("Failed to fetch allowed artifact");
    assert_eq!(allowed_get.status(), StatusCode::OK);

    let blocked_get = ctx
        .get(
            &format!("/api/v1/artifacts/{}", blocked_artifact.id),
            Some(&token),
        )
        .await
        .expect("Failed to fetch blocked artifact");
    assert_eq!(blocked_get.status(), StatusCode::NOT_FOUND);

    let create_allowed = ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref": format!("python_example.created_{}", uuid::Uuid::new_v4().simple()),
                "scope": "pack",
                "owner": "python_example",
                "type": "file_text",
                "name": "Created Artifact"
            }),
            Some(&token),
        )
        .await
        .expect("Failed to create allowed artifact");
    assert_eq!(create_allowed.status(), StatusCode::CREATED);

    let create_blocked = ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref": format!("other_pack.created_{}", uuid::Uuid::new_v4().simple()),
                "scope": "pack",
                "owner": "other_pack",
                "type": "file_text",
                "name": "Blocked Artifact"
            }),
            Some(&token),
        )
        .await
        .expect("Failed to create blocked artifact");
    assert_eq!(create_blocked.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_queue_admin_like_crud_and_pending_item_guards() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let (_pack, action) = create_pack_with_action(
        &ctx,
        &format!("queue_admin_pack_{}", uuid::Uuid::new_v4().simple()),
        &format!(
            "queue_admin_pack.dispatch_{}",
            uuid::Uuid::new_v4().simple()
        ),
    )
    .await
    .expect("Failed to create queue fixture");

    let token = register_scoped_user(
        &ctx,
        &format!("queue_admin_{}", uuid::Uuid::new_v4().simple()),
        json!([
            {
                "resource": "queues",
                "actions": ["read", "create", "update", "delete"]
            }
        ]),
    )
    .await
    .expect("Failed to register queue admin user");

    let queue_ref = format!("adhoc_queue_{}", uuid::Uuid::new_v4().simple());
    let create_queue = ctx
        .post(
            "/api/v1/queues",
            json!({
                "ref": queue_ref,
                "label": "Adhoc Queue",
                "dispatch_action_ref": action.r#ref,
                "allow_pending_update": true,
                "update_strategy": "merge_patch",
                "batch_mode": "single"
            }),
            Some(&token),
        )
        .await
        .expect("Failed to create adhoc queue");
    assert_eq!(create_queue.status(), StatusCode::CREATED);

    let enqueue = ctx
        .post(
            &format!("/api/v1/queues/{}/items", queue_ref),
            json!({
                "item_key": "order-123",
                "payload": {"state": "queued"},
                "metadata": {"source": "api"}
            }),
            Some(&token),
        )
        .await
        .expect("Failed to enqueue queue item");
    assert_eq!(enqueue.status(), StatusCode::CREATED);
    let enqueue_body: serde_json::Value = enqueue.json().await.expect("Invalid enqueue response");
    let item_id = enqueue_body["data"]["id"]
        .as_i64()
        .expect("Missing queue item id");

    let enqueue_merge = ctx
        .post(
            &format!("/api/v1/queues/{}/items", queue_ref),
            json!({
                "item_key": "order-123",
                "payload": {"extra": true},
                "metadata": {"attempt": 2}
            }),
            Some(&token),
        )
        .await
        .expect("Failed to merge queue item");
    assert_eq!(enqueue_merge.status(), StatusCode::OK);

    let update_item = ctx
        .put(
            &format!("/api/v1/queues/{}/items/{}", queue_ref, item_id),
            json!({
                "priority": 9,
                "metadata": {"attempt": 3}
            }),
            Some(&token),
        )
        .await
        .expect("Failed to update pending queue item");
    assert_eq!(update_item.status(), StatusCode::OK);

    let list_items = ctx
        .get(&format!("/api/v1/queues/{}/items", queue_ref), Some(&token))
        .await
        .expect("Failed to list queue items");
    assert_eq!(list_items.status(), StatusCode::OK);
    let list_body: serde_json::Value = list_items.json().await.expect("Invalid queue item list");
    assert_eq!(
        list_body["items"]
            .as_array()
            .expect("Expected items array")
            .len(),
        1
    );
    assert_eq!(list_body["items"][0]["payload"]["state"], "queued");
    assert_eq!(list_body["items"][0]["payload"]["extra"], true);
    assert_eq!(list_body["items"][0]["priority"], 9);

    let queue = WorkQueueRepository::find_by_ref(&ctx.pool, &queue_ref)
        .await
        .expect("Failed to load queue")
        .expect("Queue should exist");
    let item = WorkQueueItemRepository::find_by_queue_and_id(&ctx.pool, queue.id, item_id)
        .await
        .expect("Failed to load queue item")
        .expect("Queue item should exist");
    WorkQueueItemRepository::update(
        &ctx.pool,
        item.id,
        UpdateWorkQueueItemInput {
            status: Some(WorkQueueItemStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .expect("Failed to force queue item to completed");

    let blocked_update = ctx
        .put(
            &format!("/api/v1/queues/{}/items/{}", queue_ref, item_id),
            json!({"priority": 5}),
            Some(&token),
        )
        .await
        .expect("Failed to call blocked queue item update");
    assert_eq!(blocked_update.status(), StatusCode::CONFLICT);

    let blocked_delete = ctx
        .delete(
            &format!("/api/v1/queues/{}/items/{}", queue_ref, item_id),
            Some(&token),
        )
        .await
        .expect("Failed to call blocked queue item delete");
    assert_eq!(blocked_delete.status(), StatusCode::CONFLICT);

    let delete_queue = ctx
        .delete(&format!("/api/v1/queues/{}", queue_ref), Some(&token))
        .await
        .expect("Failed to delete adhoc queue");
    assert_eq!(delete_queue.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_scoped_queue_permissions_cover_definitions_and_items() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let (allowed_pack, allowed_action) =
        create_pack_with_action(&ctx, "python_example", "python_example.dispatch_queue")
            .await
            .expect("Failed to create allowed queue fixture");
    let (blocked_pack, blocked_action) =
        create_pack_with_action(&ctx, "other_pack", "other_pack.dispatch_queue")
            .await
            .expect("Failed to create blocked queue fixture");

    let blocked_queue = WorkQueueRepository::create(
        &ctx.pool,
        CreateWorkQueueInput {
            r#ref: "other_pack.blocked_queue".to_string(),
            pack: Some(blocked_pack.id),
            pack_ref: Some(blocked_pack.r#ref.clone()),
            is_adhoc: false,
            label: "Blocked Queue".to_string(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(blocked_action.id),
            dispatch_action_ref: blocked_action.r#ref.clone(),
            default_priority: 0,
            allow_pending_update: true,
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
    .expect("Failed to create blocked queue");

    let token = register_scoped_user(
        &ctx,
        &format!("queue_scoped_{}", uuid::Uuid::new_v4().simple()),
        json!([
            {
                "resource": "queues",
                "actions": ["read", "create", "update", "delete"],
                "constraints": {
                    "pack_refs": [allowed_pack.r#ref]
                }
            }
        ]),
    )
    .await
    .expect("Failed to register queue scoped user");

    let create_allowed = ctx
        .post(
            "/api/v1/queues",
            json!({
                "ref": "python_example.scoped_queue",
                "pack_ref": allowed_pack.r#ref,
                "label": "Scoped Queue",
                "dispatch_action_ref": allowed_action.r#ref,
                "allow_pending_update": true,
                "update_strategy": "replace"
            }),
            Some(&token),
        )
        .await
        .expect("Failed to create allowed scoped queue");
    assert_eq!(create_allowed.status(), StatusCode::CREATED);

    let create_blocked = ctx
        .post(
            "/api/v1/queues",
            json!({
                "ref": "other_pack.denied_queue",
                "pack_ref": blocked_pack.r#ref,
                "label": "Denied Queue",
                "dispatch_action_ref": blocked_action.r#ref
            }),
            Some(&token),
        )
        .await
        .expect("Failed to create blocked scoped queue");
    assert_eq!(create_blocked.status(), StatusCode::FORBIDDEN);

    let list_allowed = ctx
        .get(
            &format!("/api/v1/packs/{}/queues", allowed_pack.r#ref),
            Some(&token),
        )
        .await
        .expect("Failed to list allowed pack queues");
    assert_eq!(list_allowed.status(), StatusCode::OK);
    let list_allowed_body: serde_json::Value = list_allowed
        .json()
        .await
        .expect("Invalid allowed queue list");
    assert!(list_allowed_body["items"]
        .as_array()
        .expect("Expected queue items")
        .iter()
        .any(|queue| queue["ref"] == "python_example.scoped_queue"));

    let get_allowed = ctx
        .get("/api/v1/queues/python_example.scoped_queue", Some(&token))
        .await
        .expect("Failed to get allowed queue");
    assert_eq!(get_allowed.status(), StatusCode::OK);

    let get_blocked = ctx
        .get(
            &format!("/api/v1/queues/{}", blocked_queue.r#ref),
            Some(&token),
        )
        .await
        .expect("Failed to get blocked queue");
    assert_eq!(get_blocked.status(), StatusCode::NOT_FOUND);

    let enqueue_allowed = ctx
        .post(
            "/api/v1/queues/python_example.scoped_queue/items",
            json!({
                "item_key": "job-1",
                "payload": {"hello": "world"}
            }),
            Some(&token),
        )
        .await
        .expect("Failed to enqueue allowed queue item");
    assert_eq!(enqueue_allowed.status(), StatusCode::CREATED);
    let enqueue_allowed_body: serde_json::Value = enqueue_allowed
        .json()
        .await
        .expect("Invalid allowed enqueue body");
    let item_id = enqueue_allowed_body["data"]["id"]
        .as_i64()
        .expect("Missing item id");

    let update_allowed = ctx
        .put(
            &format!(
                "/api/v1/queues/python_example.scoped_queue/items/{}",
                item_id
            ),
            json!({"priority": 11}),
            Some(&token),
        )
        .await
        .expect("Failed to update allowed queue item");
    assert_eq!(update_allowed.status(), StatusCode::OK);

    let delete_allowed = ctx
        .delete(
            &format!(
                "/api/v1/queues/python_example.scoped_queue/items/{}",
                item_id
            ),
            Some(&token),
        )
        .await
        .expect("Failed to delete allowed queue item");
    assert_eq!(delete_allowed.status(), StatusCode::OK);

    let enqueue_blocked = ctx
        .post(
            &format!("/api/v1/queues/{}/items", blocked_queue.r#ref),
            json!({"payload": {"blocked": true}}),
            Some(&token),
        )
        .await
        .expect("Failed to enqueue blocked queue item");
    assert_eq!(enqueue_blocked.status(), StatusCode::FORBIDDEN);
}

// ============================================================================
// Artifact visibility × scope authorization tests
// ============================================================================

mod artifact_authz_tests {
    use super::*;
    use attune_common::auth::jwt::{
        generate_execution_token,
        generate_execution_token_with_permission_sets_and_standard_access, JwtConfig,
    };

    fn jwt_config() -> JwtConfig {
        JwtConfig {
            secret: "test-secret-for-testing-only-not-secure".to_string(),
            access_token_expiration: 300,
            refresh_token_expiration: 3600,
        }
    }

    async fn create_artifact_row(
        ctx: &TestContext,
        ref_str: &str,
        scope: OwnerType,
        owner: &str,
        visibility: ArtifactVisibility,
    ) -> attune_common::models::Artifact {
        ArtifactRepository::create(
            &ctx.pool,
            CreateArtifactInput {
                r#ref: ref_str.to_string(),
                scope,
                owner: owner.to_string(),
                r#type: ArtifactType::FileText,
                visibility,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 5,
                name: Some("test artifact".to_string()),
                description: None,
                content_type: Some("text/plain".to_string()),
                data: None,
            },
        )
        .await
        .expect("create artifact")
    }

    /// Public artifacts are readable by any authenticated user with `artifacts:read`.
    #[tokio::test]
    #[ignore = "integration test — requires database"]
    async fn public_artifact_readable_by_any_user_with_artifacts_read() {
        let ctx = TestContext::new().await.expect("test ctx");
        let token = register_scoped_user(
            &ctx,
            &format!("public_reader_{}", uuid::Uuid::new_v4().simple()),
            json!([
                { "resource": "artifacts", "actions": ["read"] }
            ]),
        )
        .await
        .expect("register user");

        let art = create_artifact_row(
            &ctx,
            &format!("some_pack.public_{}", uuid::Uuid::new_v4().simple()),
            OwnerType::Pack,
            "some_pack",
            ArtifactVisibility::Public,
        )
        .await;

        let resp = ctx
            .get(&format!("/api/v1/artifacts/{}", art.id), Some(&token))
            .await
            .expect("fetch");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Private + scope=identity: only the owning identity may read.
    #[tokio::test]
    #[ignore = "integration test — requires database"]
    async fn private_identity_scoped_artifact_owner_can_read_other_cannot() {
        let ctx = TestContext::new().await.expect("test ctx");

        // Register two users, both with broad artifacts:read.
        let owner_login = format!("owner_{}", uuid::Uuid::new_v4().simple());
        let owner_token = register_scoped_user(
            &ctx,
            &owner_login,
            json!([{ "resource": "artifacts", "actions": ["read"] }]),
        )
        .await
        .expect("register owner");
        let owner_identity = IdentityRepository::find_by_login(&ctx.pool, &owner_login)
            .await
            .expect("lookup")
            .expect("owner identity");

        let other_token = register_scoped_user(
            &ctx,
            &format!("other_{}", uuid::Uuid::new_v4().simple()),
            json!([{ "resource": "artifacts", "actions": ["read"] }]),
        )
        .await
        .expect("register other");

        let art = create_artifact_row(
            &ctx,
            &format!("identity_artifact_{}", uuid::Uuid::new_v4().simple()),
            OwnerType::Identity,
            &owner_identity.id.to_string(),
            ArtifactVisibility::Private,
        )
        .await;

        let owner_resp = ctx
            .get(&format!("/api/v1/artifacts/{}", art.id), Some(&owner_token))
            .await
            .expect("owner fetch");
        assert_eq!(owner_resp.status(), StatusCode::OK);

        let other_resp = ctx
            .get(&format!("/api/v1/artifacts/{}", art.id), Some(&other_token))
            .await
            .expect("other fetch");
        assert_eq!(other_resp.status(), StatusCode::NOT_FOUND);
    }

    /// Private + scope=action: derive pack from `<pack>.<action>`, require packs:read.
    #[tokio::test]
    #[ignore = "integration test — requires database"]
    async fn private_action_scoped_artifact_uses_derived_pack_for_authz() {
        let ctx = TestContext::new().await.expect("test ctx");

        // User has packs:read on `python_example`, but no artifacts:* grant.
        let token = register_scoped_user(
            &ctx,
            &format!("pack_reader_{}", uuid::Uuid::new_v4().simple()),
            json!([
                {
                    "resource": "packs",
                    "actions": ["read"],
                    "constraints": { "pack_refs": ["python_example"] }
                }
            ]),
        )
        .await
        .expect("register user");

        let art_allowed = create_artifact_row(
            &ctx,
            &format!(
                "python_example.deploy_log_{}",
                uuid::Uuid::new_v4().simple()
            ),
            OwnerType::Action,
            "python_example.deploy",
            ArtifactVisibility::Private,
        )
        .await;
        let art_blocked = create_artifact_row(
            &ctx,
            &format!("other_pack.deploy_log_{}", uuid::Uuid::new_v4().simple()),
            OwnerType::Action,
            "other_pack.deploy",
            ArtifactVisibility::Private,
        )
        .await;

        let ok = ctx
            .get(
                &format!("/api/v1/artifacts/{}", art_allowed.id),
                Some(&token),
            )
            .await
            .expect("fetch allowed");
        assert_eq!(ok.status(), StatusCode::OK);

        let denied = ctx
            .get(
                &format!("/api/v1/artifacts/{}", art_blocked.id),
                Some(&token),
            )
            .await
            .expect("fetch blocked");
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    }

    /// Private + scope=sensor: same pack-derivation rule as scope=action.
    #[tokio::test]
    #[ignore = "integration test — requires database"]
    async fn private_sensor_scoped_artifact_uses_derived_pack_for_authz() {
        let ctx = TestContext::new().await.expect("test ctx");
        let token = register_scoped_user(
            &ctx,
            &format!("sensor_reader_{}", uuid::Uuid::new_v4().simple()),
            json!([
                {
                    "resource": "packs",
                    "actions": ["read"],
                    "constraints": { "pack_refs": ["sensor_pack"] }
                }
            ]),
        )
        .await
        .expect("register user");

        let allowed = create_artifact_row(
            &ctx,
            &format!("sensor_pack.heartbeat_{}", uuid::Uuid::new_v4().simple()),
            OwnerType::Sensor,
            "sensor_pack.heartbeat",
            ArtifactVisibility::Private,
        )
        .await;
        let blocked = create_artifact_row(
            &ctx,
            &format!("foreign.heartbeat_{}", uuid::Uuid::new_v4().simple()),
            OwnerType::Sensor,
            "foreign.heartbeat",
            ArtifactVisibility::Private,
        )
        .await;

        let ok = ctx
            .get(&format!("/api/v1/artifacts/{}", allowed.id), Some(&token))
            .await
            .expect("ok");
        assert_eq!(ok.status(), StatusCode::OK);
        let denied = ctx
            .get(&format!("/api/v1/artifacts/{}", blocked.id), Some(&token))
            .await
            .expect("denied");
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    }

    /// List endpoint hides private artifacts the user cannot access.
    #[tokio::test]
    #[ignore = "integration test — requires database"]
    async fn list_endpoint_filters_private_artifacts_user_cannot_read() {
        let ctx = TestContext::new().await.expect("test ctx");
        let token = register_scoped_user(
            &ctx,
            &format!("listing_user_{}", uuid::Uuid::new_v4().simple()),
            json!([
                { "resource": "artifacts", "actions": ["read"] },
                {
                    "resource": "packs",
                    "actions": ["read"],
                    "constraints": { "pack_refs": ["mine"] }
                }
            ]),
        )
        .await
        .expect("register user");

        // Public artifact in some pack — should be visible.
        let public_art = create_artifact_row(
            &ctx,
            &format!("mine.public_{}", uuid::Uuid::new_v4().simple()),
            OwnerType::Pack,
            "mine",
            ArtifactVisibility::Public,
        )
        .await;
        // Private artifact in user's pack — visible via packs:read.
        let private_mine = create_artifact_row(
            &ctx,
            &format!("mine.private_{}", uuid::Uuid::new_v4().simple()),
            OwnerType::Pack,
            "mine",
            ArtifactVisibility::Private,
        )
        .await;
        // Private artifact in foreign pack — must be hidden.
        let private_foreign = create_artifact_row(
            &ctx,
            &format!("yours.private_{}", uuid::Uuid::new_v4().simple()),
            OwnerType::Pack,
            "yours",
            ArtifactVisibility::Private,
        )
        .await;

        let resp = ctx
            .get("/api/v1/artifacts?per_page=100", Some(&token))
            .await
            .expect("list");
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = resp.json().await.expect("json");
        let ids: Vec<i64> = body["items"]
            .as_array()
            .expect("items array")
            .iter()
            .map(|v| v["id"].as_i64().expect("id"))
            .collect();
        assert!(ids.contains(&public_art.id));
        assert!(ids.contains(&private_mine.id));
        assert!(!ids.contains(&private_foreign.id));
    }

    /// Execution token from pack X cannot mutate artifact owned by pack Y.
    #[tokio::test]
    #[ignore = "integration test — requires database"]
    async fn execution_token_cannot_cross_pack_mutate_artifact() {
        let ctx = TestContext::new().await.expect("test ctx");

        // Register an identity to embed in the execution token.
        let login = format!("exec_user_{}", uuid::Uuid::new_v4().simple());
        let _access_token = register_scoped_user(
            &ctx,
            &login,
            json!([{ "resource": "artifacts", "actions": ["read", "update", "create"] }]),
        )
        .await
        .expect("register user");
        let identity = IdentityRepository::find_by_login(&ctx.pool, &login)
            .await
            .expect("lookup")
            .expect("identity");

        // Mint an execution token whose action_ref lives in pack `pack_x`.
        let exec_token = generate_execution_token_with_permission_sets_and_standard_access(
            identity.id,
            424242, // execution_id, not validated by route
            "pack_x.deploy",
            &jwt_config(),
            Some(300),
            &[STANDARD_EXECUTION_ACCESS_REF.to_string()],
            &["pack_x.deploy".to_string()],
        )
        .expect("mint exec token");

        // Create a private artifact owned by a *different* pack.
        let art = create_artifact_row(
            &ctx,
            &format!("pack_y.build_log_{}", uuid::Uuid::new_v4().simple()),
            OwnerType::Pack,
            "pack_y",
            ArtifactVisibility::Private,
        )
        .await;

        // Cross-pack progress append must be refused.
        let resp = ctx
            .post(
                &format!("/api/v1/artifacts/{}/progress", art.id),
                json!({ "entry": { "msg": "hi" } }),
                Some(&exec_token),
            )
            .await
            .expect("append");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // Cross-pack create must be refused too.
        let create_resp = ctx
            .post(
                "/api/v1/artifacts",
                json!({
                    "ref": format!("pack_y.created_{}", uuid::Uuid::new_v4().simple()),
                    "scope": "pack",
                    "owner": "pack_y",
                    "type": "file_text",
                    "name": "x"
                }),
                Some(&exec_token),
            )
            .await
            .expect("create cross-pack");
        assert_eq!(create_resp.status(), StatusCode::FORBIDDEN);

        // Same-pack create succeeds.
        let same_pack_resp = ctx
            .post(
                "/api/v1/artifacts",
                json!({
                    "ref": format!("pack_x.created_{}", uuid::Uuid::new_v4().simple()),
                    "scope": "pack",
                    "owner": "pack_x",
                    "type": "file_text",
                    "name": "x"
                }),
                Some(&exec_token),
            )
            .await
            .expect("create same-pack");
        assert_eq!(same_pack_resp.status(), StatusCode::CREATED);
    }

    /// An action-scoped artifact whose owner ref lacks a `.` separator
    /// (e.g. `"action"` rather than `"<pack>.<action>"`) is malformed and
    /// must not be silently treated as having pack `"action"`. The cross-pack
    /// guard refuses rather than letting an execution token mutate it via a
    /// fake derived pack.
    #[tokio::test]
    #[ignore = "integration test — requires database"]
    async fn dotless_action_owner_is_treated_as_malformed_and_refused() {
        let ctx = TestContext::new().await.expect("test ctx");

        // Register an identity for the execution token.
        let login = format!("dotless_user_{}", uuid::Uuid::new_v4().simple());
        let _access = register_scoped_user(
            &ctx,
            &login,
            json!([{ "resource": "artifacts", "actions": ["read", "update", "create"] }]),
        )
        .await
        .expect("register user");
        let identity = IdentityRepository::find_by_login(&ctx.pool, &login)
            .await
            .expect("lookup")
            .expect("identity");

        // Mint a normal execution token in pack `pack_x`.
        let exec_token = generate_execution_token(
            identity.id,
            12345,
            "pack_x.deploy",
            &jwt_config(),
            Some(300),
        )
        .expect("mint exec token");

        // Create a private action-scoped artifact with a malformed (dotless)
        // owner ref. `derive_pack_ref` must return `None` for this owner;
        // the cross-pack guard must refuse the mutation.
        let art = create_artifact_row(
            &ctx,
            &format!("malformed_owner_{}", uuid::Uuid::new_v4().simple()),
            OwnerType::Action,
            "action",
            ArtifactVisibility::Private,
        )
        .await;

        let resp = ctx
            .post(
                &format!("/api/v1/artifacts/{}/progress", art.id),
                json!({ "entry": { "msg": "hi" } }),
                Some(&exec_token),
            )
            .await
            .expect("append");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    /// An execution token with an empty `action_ref` cannot derive a token
    /// pack; cross-pack writes against pack-derivable artifacts must be
    /// refused with 403, not silently allowed.
    #[tokio::test]
    #[ignore = "integration test — requires database"]
    async fn execution_token_with_empty_action_ref_is_refused() {
        let ctx = TestContext::new().await.expect("test ctx");

        let login = format!("empty_ref_user_{}", uuid::Uuid::new_v4().simple());
        let _access = register_scoped_user(
            &ctx,
            &login,
            json!([{ "resource": "artifacts", "actions": ["read", "update", "create"] }]),
        )
        .await
        .expect("register user");
        let identity = IdentityRepository::find_by_login(&ctx.pool, &login)
            .await
            .expect("lookup")
            .expect("identity");

        // Malformed: action_ref is the empty string.
        let exec_token = generate_execution_token(identity.id, 99999, "", &jwt_config(), Some(300))
            .expect("mint exec token");

        // Pack-scoped artifact in some pack.
        let art = create_artifact_row(
            &ctx,
            &format!("pack_z.log_{}", uuid::Uuid::new_v4().simple()),
            OwnerType::Pack,
            "pack_z",
            ArtifactVisibility::Private,
        )
        .await;

        let resp = ctx
            .post(
                &format!("/api/v1/artifacts/{}/progress", art.id),
                json!({ "entry": { "msg": "hi" } }),
                Some(&exec_token),
            )
            .await
            .expect("append");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // Same for create against a pack-scoped target.
        let create_resp = ctx
            .post(
                "/api/v1/artifacts",
                json!({
                    "ref": format!("pack_z.created_{}", uuid::Uuid::new_v4().simple()),
                    "scope": "pack",
                    "owner": "pack_z",
                    "type": "file_text",
                    "name": "x"
                }),
                Some(&exec_token),
            )
            .await
            .expect("create");
        assert_eq!(create_resp.status(), StatusCode::FORBIDDEN);
    }
}
