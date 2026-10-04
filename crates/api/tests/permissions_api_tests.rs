use axum::http::StatusCode;
use helpers::*;
use serde_json::json;

use attune_common::repositories::FindById;
use attune_common::repositories::{
    cache::{
        CacheNamespacePolicy, CacheNamespaceRepository, CacheOwnerScope, CreateCacheNamespaceInput,
    },
    IdentityRepository,
};

mod helpers;

#[tokio::test]
async fn test_identity_crud_and_permission_assignment_flow() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context")
        .with_admin_auth()
        .await
        .expect("Failed to create admin-authenticated test user");

    let create_identity_response = ctx
        .post(
            "/api/v1/identities",
            json!({
                "login": "managed_user",
                "display_name": "Managed User",
                "password": "ManagedPass123!",
                "attributes": {
                    "department": "platform"
                }
            }),
            ctx.token(),
        )
        .await
        .expect("Failed to create identity");

    assert_eq!(create_identity_response.status(), StatusCode::CREATED);

    let created_identity: serde_json::Value = create_identity_response
        .json()
        .await
        .expect("Failed to parse identity create response");
    let identity_id = created_identity["data"]["id"]
        .as_i64()
        .expect("Missing identity id");

    let list_identities_response = ctx
        .get("/api/v1/identities", ctx.token())
        .await
        .expect("Failed to list identities");
    assert_eq!(list_identities_response.status(), StatusCode::OK);

    let identities_body: serde_json::Value = list_identities_response
        .json()
        .await
        .expect("Failed to parse identities response");
    assert!(identities_body["items"]
        .as_array()
        .expect("Expected items array")
        .iter()
        .any(|item| item["login"] == "managed_user"));

    let update_identity_response = ctx
        .put(
            &format!("/api/v1/identities/{}", identity_id),
            json!({
                "display_name": "Managed User Updated",
                "attributes": {
                    "department": "security"
                }
            }),
            ctx.token(),
        )
        .await
        .expect("Failed to update identity");
    assert_eq!(update_identity_response.status(), StatusCode::OK);

    let get_identity_response = ctx
        .get(&format!("/api/v1/identities/{}", identity_id), ctx.token())
        .await
        .expect("Failed to get identity");
    assert_eq!(get_identity_response.status(), StatusCode::OK);

    let identity_body: serde_json::Value = get_identity_response
        .json()
        .await
        .expect("Failed to parse get identity response");
    assert_eq!(
        identity_body["data"]["display_name"],
        "Managed User Updated"
    );
    assert_eq!(
        identity_body["data"]["attributes"]["department"],
        "security"
    );

    let permission_sets_response = ctx
        .get("/api/v1/permissions/sets", ctx.token())
        .await
        .expect("Failed to list permission sets");
    assert_eq!(permission_sets_response.status(), StatusCode::OK);

    let assignment_response = ctx
        .post(
            "/api/v1/permissions/assignments",
            json!({
                "identity_id": identity_id,
                "permission_set_ref": "core.admin"
            }),
            ctx.token(),
        )
        .await
        .expect("Failed to create permission assignment");
    assert_eq!(assignment_response.status(), StatusCode::CREATED);

    let assignment_body: serde_json::Value = assignment_response
        .json()
        .await
        .expect("Failed to parse permission assignment response");
    let assignment_id = assignment_body["data"]["id"]
        .as_i64()
        .expect("Missing assignment id");
    assert_eq!(assignment_body["data"]["permission_set_ref"], "core.admin");

    let list_assignments_response = ctx
        .get(
            &format!("/api/v1/identities/{}/permissions", identity_id),
            ctx.token(),
        )
        .await
        .expect("Failed to list identity permissions");
    assert_eq!(list_assignments_response.status(), StatusCode::OK);

    let assignments_body: serde_json::Value = list_assignments_response
        .json()
        .await
        .expect("Failed to parse identity permissions response");
    assert!(assignments_body
        .as_array()
        .expect("Expected array response")
        .iter()
        .any(|item| item["permission_set_ref"] == "core.admin"));

    let delete_assignment_response = ctx
        .delete(
            &format!("/api/v1/permissions/assignments/{}", assignment_id),
            ctx.token(),
        )
        .await
        .expect("Failed to delete assignment");
    assert_eq!(delete_assignment_response.status(), StatusCode::OK);

    let delete_identity_response = ctx
        .delete(&format!("/api/v1/identities/{}", identity_id), ctx.token())
        .await
        .expect("Failed to delete identity");
    assert_eq!(delete_identity_response.status(), StatusCode::OK);

    let missing_identity_response = ctx
        .get(&format!("/api/v1/identities/{}", identity_id), ctx.token())
        .await
        .expect("Failed to fetch deleted identity");
    assert_eq!(missing_identity_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_identity_delete_tombstones_owned_caches_before_deletion() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context")
        .with_admin_auth()
        .await
        .expect("Failed to create admin-authenticated test user");

    let create_response = ctx
        .post(
            "/api/v1/identities",
            json!({
                "login": "cache_owner_delete",
                "display_name": "Cache Owner",
                "password": "ManagedPass123!"
            }),
            ctx.token(),
        )
        .await
        .expect("Failed to create identity");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let body: serde_json::Value = create_response.json().await.expect("identity response");
    let identity_id = body["data"]["id"].as_i64().expect("identity id");

    let namespace = CacheNamespaceRepository::create_api(
        &ctx.pool,
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::identity(identity_id),
            namespace: format!("identity_delete_{identity_id}"),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await
    .expect("create identity-owned cache");

    let pending_response = ctx
        .delete(&format!("/api/v1/identities/{identity_id}"), ctx.token())
        .await
        .expect("delete identity with owned cache");
    assert_eq!(pending_response.status(), StatusCode::CONFLICT);
    let pending_body: serde_json::Value = pending_response.json().await.expect("pending response");
    assert!(pending_body["error"]
        .as_str()
        .is_some_and(|message| message.contains("pending retention cleanup")));

    assert!(IdentityRepository::find_by_id(&ctx.pool, identity_id)
        .await
        .expect("find identity")
        .is_some());
    let tombstoned = CacheNamespaceRepository::find_by_id(&ctx.pool, namespace.id)
        .await
        .expect("find cache namespace")
        .expect("cache namespace remains for retention");
    assert!(tombstoned.tombstoned_at.is_some());

    assert!(
        CacheNamespaceRepository::delete_tombstoned_if_empty(&ctx.pool, namespace.id)
            .await
            .expect("drain empty namespace")
    );

    let delete_response = ctx
        .delete(&format!("/api/v1/identities/{identity_id}"), ctx.token())
        .await
        .expect("retry identity delete");
    assert_eq!(delete_response.status(), StatusCode::OK);
    assert!(IdentityRepository::find_by_id(&ctx.pool, identity_id)
        .await
        .expect("find deleted identity")
        .is_none());
}

#[tokio::test]
async fn test_plain_authenticated_user_cannot_manage_identities() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context")
        .with_auth()
        .await
        .expect("Failed to authenticate plain test user");

    let response = ctx
        .get("/api/v1/identities", ctx.token())
        .await
        .expect("Failed to call identities endpoint");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_permission_cli_filtered_inspection_and_assignment_contract() {
    use attune_common::repositories::{
        identity::{CreatePermissionSetInput, PermissionSetRepository},
        Create,
    };

    let ctx = TestContext::new()
        .await
        .unwrap()
        .with_admin_auth()
        .await
        .unwrap();
    let pack = create_test_pack(&ctx.pool, "deploy").await.unwrap();
    let set = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "deploy.operator".to_string(), pack: Some(pack.id), pack_ref: Some("deploy".to_string()),
        label: Some("Operator".to_string()), description: None,
        grants: json!([{"resource": "actions", "actions": ["read"], "constraints": {"refs": ["deploy.release"]}}]),
    }).await.unwrap();
    let response = ctx
        .post(
            "/api/v1/identities",
            json!({"login": "alice+ops@example.com"}),
            ctx.token(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: serde_json::Value = response.json().await.unwrap();
    let identity_id = body["data"]["id"].as_i64().unwrap();

    let direct_request =
        json!({"identity_id": identity_id, "permission_set_ref": "deploy.operator"});
    assert_eq!(
        ctx.post(
            "/api/v1/permissions/assignments",
            direct_request.clone(),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::CREATED
    );
    assert_eq!(
        ctx.post(
            "/api/v1/permissions/assignments",
            direct_request,
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::CONFLICT
    );
    let role_path = format!("/api/v1/permissions/sets/{}/roles", set.id);
    assert_eq!(
        ctx.post(&role_path, json!({"role": "ops & deploy"}), ctx.token())
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    assert_eq!(
        ctx.post(&role_path, json!({"role": "ops & deploy"}), ctx.token())
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let membership_path = format!("/api/v1/identities/{identity_id}/roles");
    assert_eq!(
        ctx.post(
            &membership_path,
            json!({"role": "ops & deploy"}),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::CREATED
    );
    assert_eq!(
        ctx.post(
            &membership_path,
            json!({"role": "ops & deploy"}),
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::CONFLICT
    );

    let response = ctx
        .get(
            "/api/v1/identities?login=alice%2Bops%40example.com",
            ctx.token(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["pagination"]["total_items"], 1);
    assert_eq!(body["items"][0]["roles"], json!(["ops & deploy"]));
    let response = ctx
        .get(
            "/api/v1/permissions/sets/by-ref/deploy.operator",
            ctx.token(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["data"]["management_origin"], "pack");
    assert_eq!(body["data"]["roles"][0]["role"], "ops & deploy");

    for (query, expected_type) in [
        ("identity_login=alice%2Bops%40example.com", "identity"),
        ("role=ops%20%26%20deploy", "role"),
    ] {
        let response = ctx
            .get(
                &format!(
                    "/api/v1/permissions/assignments?{query}&permission_set_ref=deploy.operator"
                ),
                ctx.token(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["pagination"]["total_items"], 1);
        assert_eq!(body["items"][0]["target"]["type"], expected_type);
        assert_eq!(body["items"][0]["permission_set_ref"], "deploy.operator");
    }
    for page in [1, 2] {
        let response = ctx.get(&format!("/api/v1/permissions/assignments?permission_set_ref=deploy.operator&page_size=1&page={page}"), ctx.token()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["pagination"]["total_items"], 2);
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(
            body["items"][0]["target"]["type"],
            if page == 1 { "identity" } else { "role" }
        );
    }
    assert_eq!(
        ctx.get(
            "/api/v1/permissions/assignments?identity_id=1&role=ops",
            ctx.token()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::BAD_REQUEST
    );
    for endpoint in ["/api/v1/identities", "/api/v1/permissions/assignments"] {
        for query in ["page_size=200", "page_size=0", "page=0"] {
            assert_eq!(
                ctx.get(&format!("{endpoint}?{query}"), ctx.token())
                    .await
                    .unwrap()
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
    }
    let mut tx = ctx.pool.begin().await.unwrap();
    attune_common::repositories::component_lifecycle::ComponentLifecycleRepository::reconcile_omissions(
        &mut tx, pack.id, &Default::default(),
    ).await.unwrap();
    tx.commit().await.unwrap();
    let response = ctx
        .get(&format!("/api/v1/identities/{identity_id}"), ctx.token())
        .await
        .unwrap();
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body["data"]["direct_permissions"][0]["permission_set_ref"],
        "deploy.operator"
    );
    let response = ctx
        .get(
            &format!("/api/v1/identities/{identity_id}/permissions"),
            ctx.token(),
        )
        .await
        .unwrap();
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body[0]["permission_set_ref"], "deploy.operator");
    let response = ctx
        .get("/api/v1/permissions/sets?pack_ref=deploy", ctx.token())
        .await
        .unwrap();
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body, json!([]));
    let response = ctx
        .get(
            "/api/v1/permissions/sets?pack_ref=deploy&include_retired=true",
            ctx.token(),
        )
        .await
        .unwrap();
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body[0]["retired_at"].is_string());
    ctx.cleanup().await.unwrap();
}

#[tokio::test]
async fn test_permission_update_dry_run_validates_without_persisting() {
    use attune_common::repositories::{
        identity::{CreatePermissionSetInput, PermissionSetRepository},
        Create,
    };
    let ctx = TestContext::new()
        .await
        .unwrap()
        .with_admin_auth()
        .await
        .unwrap();
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "deploy.operator".to_string(),
            pack: None,
            pack_ref: None,
            label: Some("Operator".to_string()),
            description: Some("Original description".to_string()),
            grants: json!([{"resource": "actions", "actions": ["read"]}]),
        },
    )
    .await
    .unwrap();
    let path = format!("/api/v1/permissions/sets/{}", set.id);
    let request = json!({"label": "Updated", "grants": [{"resource": "actions", "actions": ["execute"], "constraints": {"refs": ["deploy.release"]}}]});
    let response = ctx
        .put(
            &format!("{path}?dry_run=true"),
            request.clone(),
            ctx.token(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["data"]["label"], "Updated");
    assert_eq!(body["data"]["description"], "Original description");
    let unchanged = PermissionSetRepository::find_by_id(&ctx.pool, set.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.grants, set.grants);
    assert_eq!(unchanged.updated, set.updated);
    let response = ctx
        .put(
            &format!("{path}?dry_run=true"),
            json!({"grants": [{"resource": "actions", "actions": ["decrypt"]}]}),
            ctx.token(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = ctx.put(&format!("{path}?dry_run=true"), json!({"grants": [{"resource": "actions", "actions": ["execute"], "constraints": {"ref": ["deploy.release"]}}]}), ctx.token()).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("unknown field"));
    let response = ctx.put(&path, request.clone(), ctx.token()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let updated = PermissionSetRepository::find_by_id(&ctx.pool, set.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.grants, request["grants"]);
    assert_eq!(updated.label.as_deref(), Some("Updated"));
    ctx.cleanup().await.unwrap();
}

#[tokio::test]
async fn test_permission_inspection_requires_auth_and_read_authority() {
    let mut ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();
    let token = ctx.token.take().unwrap();
    for path in [
        "/api/v1/permissions/assignments",
        "/api/v1/permissions/sets/by-ref/core.admin",
    ] {
        assert_eq!(
            ctx.get(path, None).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            ctx.get(path, Some(&token)).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    ctx.cleanup().await.unwrap();
}

#[tokio::test]
async fn test_platform_permission_definitions_are_inspectable_but_not_editable() {
    use attune_api::authz::AuthorizationService;
    use attune_common::repositories::{
        identity::{
            CreatePermissionAssignmentInput, PermissionAssignmentRepository,
            PermissionSetRepository,
        },
        platform_catalog::PlatformCatalogRepository,
        Create, FindByRef,
    };
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();
    PlatformCatalogRepository::reconcile(&ctx.pool)
        .await
        .unwrap();
    let admin = PermissionSetRepository::find_by_ref(&ctx.pool, "core.admin")
        .await
        .unwrap()
        .unwrap();
    let identity_id = ctx.user.as_ref().unwrap().id;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity_id,
            permset: admin.id,
        },
    )
    .await
    .unwrap();
    AuthorizationService::invalidate_identity_authz_cache(identity_id).await;
    let response = ctx
        .get("/api/v1/permissions/sets/by-ref/core.admin", ctx.token())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["data"]["management_origin"], "platform");
    for suffix in ["", "?dry_run=true"] {
        let response = ctx
            .put(
                &format!("/api/v1/permissions/sets/{}{suffix}", admin.id),
                json!({"grants": []}),
                ctx.token(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["error"].as_str().unwrap().contains("platform-managed"));
    }
    let unchanged = PermissionSetRepository::find_by_id(&ctx.pool, admin.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.grants, admin.grants);
    ctx.cleanup().await.unwrap();
}
