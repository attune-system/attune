//! Access identities hit the protected production detail handler. Receipt
//! purpose authority must not replace current root/child read and decrypt grants.
#[path = "../../common/tests/support/empty_root_reader_fixture.rs"]
#[allow(dead_code)]
mod source;

use crate::{
    auth::{
        jwt::{Claims, TokenType},
        middleware::{AuthenticatedUser, RequireAuth},
    },
    dto::execution::ExecutionDetailQueryParams,
    routes::executions::get_execution,
    state::AppState,
};
use attune_common::{
    config::{BlobStorageConfig, Config},
    repositories::{
        execution::ExecutionRepository,
        file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        identity::PermissionAssignmentRepository, Delete,
    },
};
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
};
use serde_json::{json, Value};
use std::sync::Arc;

const KEY: &str = "0123456789abcdef0123456789abcdef";

#[tokio::test]
async fn empty_root_human_direct_requires_current_read_and_decrypt_and_rechecks_revocation() {
    run(false).await;
}
#[tokio::test]
async fn empty_root_human_real_child_requires_both_scopes_after_child_collection() {
    run(true).await;
}

fn user(identity: i64, login: String) -> AuthenticatedUser {
    let now = chrono::Utc::now().timestamp();
    AuthenticatedUser {
        claims: Claims {
            sub: identity.to_string(),
            login,
            iat: now,
            exp: now + 600,
            token_type: TokenType::Access,
            scope: None,
            metadata: None,
        },
    }
}
async fn detail(
    state: &Arc<AppState>,
    reader: &AuthenticatedUser,
    execution: i64,
    reveal: bool,
) -> (axum::http::StatusCode, Value) {
    let response = match get_execution(
        State(state.clone()),
        RequireAuth(reader.clone()),
        Path(execution),
        Query(ExecutionDetailQueryParams {
            include_secret_values: reveal,
        }),
    )
    .await
    {
        Ok(response) => response.into_response(),
        Err(error) => error.into_response(),
    };
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn run(successor: bool) {
    use source::{add_reader_grant, completed_empty_root, human_reader};
    let (db, root, _, child) = completed_empty_root(successor).await;
    let directory = tempfile::tempdir().unwrap();
    let config_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.test.yaml");
    let mut config = Config::load_from_file(config_path.to_str().unwrap()).unwrap();
    config.security.encryption_key = Some(KEY.into());
    config.storage = BlobStorageConfig::Filesystem {
        root: directory.path().join("objects"),
    };
    let state = Arc::new(AppState::new(db.pool().clone(), config));
    let (id, login, _) = human_reader(&db, std::slice::from_ref(&root.action_ref), &["read"]).await;
    let read_only = user(id, login);
    assert_eq!(
        detail(&state, &read_only, root.id, true).await.0,
        axum::http::StatusCode::FORBIDDEN
    );
    let (id, login, _) = human_reader(
        &db,
        std::slice::from_ref(&root.action_ref),
        &["read", "decrypt"],
    )
    .await;
    let root_only = user(id, login);
    let mut refs = vec![root.action_ref.clone()];
    if let Some(child) = &child {
        refs.push(child.action_ref.clone());
        let (id, login, _) = human_reader(
            &db,
            std::slice::from_ref(&child.action_ref),
            &["read", "decrypt"],
        )
        .await;
        assert_eq!(
            detail(&state, &user(id, login), root.id, true).await.0,
            axum::http::StatusCode::FORBIDDEN
        );
        let mut gc = db.begin().await.unwrap();
        Evidence::lock_reclamation(&mut gc).await.unwrap();
        assert!(ExecutionRepository::delete(&mut *gc, child.id)
            .await
            .unwrap());
        gc.commit().await.unwrap();
        let (id, login, _) = human_reader(
            &db,
            std::slice::from_ref(&root.action_ref),
            &["read", "decrypt"],
        )
        .await;
        add_reader_grant(&db, id, std::slice::from_ref(&child.action_ref), &["read"]).await;
        let (status, body) = detail(&state, &user(id, login), root.id, true).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(
            body["data"]["result"],
            attune_common::secret_values::redaction_marker(),
            "source Read without source Decrypt must remain redacted"
        );
    }
    let (status, body) = detail(&state, &root_only, root.id, true).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(
        body["data"]["result"],
        if successor {
            attune_common::secret_values::redaction_marker()
        } else {
            json!({"empty":[],"succeeded":true})
        }
    );
    if let Some(child) = &child {
        let root_reader_id = root_only.claims.sub.parse::<i64>().unwrap();
        let child_assignment = add_reader_grant(
            &db,
            root_reader_id,
            std::slice::from_ref(&child.action_ref),
            &["read", "decrypt"],
        )
        .await;
        crate::authz::AuthorizationService::invalidate_identity_authz_cache(root_reader_id).await;
        let (status, body) = detail(&state, &root_only, root.id, true).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(body["data"]["result"], json!({"empty":[],"succeeded":true}));
        assert!(
            PermissionAssignmentRepository::delete(&*db, child_assignment)
                .await
                .unwrap()
        );
        crate::authz::AuthorizationService::invalidate_identity_authz_cache(root_reader_id).await;
        let (status, body) = detail(&state, &root_only, root.id, true).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(
            body["data"]["result"],
            attune_common::secret_values::redaction_marker(),
            "revoking only the collected child's grant must redact the result without revoking root access"
        );
        assert_eq!(
            detail(&state, &root_only, root.id, false).await.0,
            axum::http::StatusCode::OK
        );
    }
    let (id, login, assignment) = human_reader(&db, &refs, &["read", "decrypt"]).await;
    let full = user(id, login);
    let (status, body) = detail(&state, &full, root.id, true).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["data"]["result"], json!({"empty":[],"succeeded":true}));
    assert!(PermissionAssignmentRepository::delete(&*db, assignment)
        .await
        .unwrap());
    // Match the normal permission-unassignment endpoint's cache invalidation.
    crate::authz::AuthorizationService::invalidate_identity_authz_cache(id).await;
    assert_eq!(
        detail(&state, &full, root.id, true).await.0,
        axum::http::StatusCode::FORBIDDEN
    );
    drop(state);
    db.cleanup().await.unwrap();
    directory.close().unwrap();
}
