use axum::http::StatusCode;
use helpers::*;
use serde_json::json;

mod helpers;

#[tokio::test]
#[ignore = "integration test — requires database"]
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
#[ignore = "integration test — requires database"]
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
