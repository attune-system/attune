//! Shared-IAM rejection exercised with live, current service credentials.
use super::*;
use attune_api::{
    auth::middleware::AuthenticatedUser, authz::AuthorizationCheck, middleware::ApiError,
};
use attune_common::{
    auth::jwt::{validate_token, TokenType},
    rbac::{Action, AuthorizationContext, Resource},
};

pub async fn assert_service_iam_rejected(
    ctx: &TestContext,
    tokens: &[&str],
    access: &str,
) -> Result<()> {
    let authz = ctx.state.authorization_service();
    let access_user = AuthenticatedUser {
        claims: validate_token(access, &ctx.state.jwt_config)?,
    };
    let snapshot = authz
        .load_snapshot(&access_user)
        .await?
        .ok_or("missing access snapshot")?;
    let check = || AuthorizationCheck {
        resource: Resource::Identities,
        action: Action::Read,
        context: AuthorizationContext::new(snapshot.identity_id),
    };
    assert!(matches!(
        authz.authorize_with_snapshot(&access_user, None, check()),
        Err(ApiError::Forbidden(_))
    ));
    // Malformed recorded authority must not fall back to human IAM, including
    // when the consumer supplies a human snapshot or selects no named grants.
    let mut recorded = access_user.clone();
    recorded.claims.token_type = TokenType::Execution;
    recorded.claims.metadata = Some(json!({"authority_source":"pack"}));
    assert!(matches!(
        authz.load_snapshot(&recorded).await,
        Err(ApiError::Unauthorized(_))
    ));
    assert!(matches!(
        authz.authorize_with_snapshot(&recorded, Some(&snapshot), check()),
        Err(ApiError::Unauthorized(_))
    ));
    assert!(matches!(
        authz.can_delegate_permission_sets(&recorded, &[]).await,
        Err(ApiError::Unauthorized(_))
    ));
    let mut other = access_user.clone();
    other.claims.sub = (snapshot.identity_id + 1).to_string();
    assert!(matches!(
        authz.authorize_with_snapshot(&other, Some(&snapshot), check()),
        Err(ApiError::Forbidden(_))
    ));
    for token in tokens {
        let mut user = AuthenticatedUser {
            claims: validate_token(token, &ctx.state.jwt_config)?,
        };
        // Refresh is rejected by middleware; exercise the shared service directly too.
        for kind in [user.claims.token_type.clone(), TokenType::Refresh] {
            user.claims.token_type = kind;
            assert!(matches!(
                authz.authorize(&user, check()).await,
                Err(ApiError::Forbidden(_))
            ));
            assert!(matches!(
                authz.load_snapshot(&user).await,
                Err(ApiError::Forbidden(_))
            ));
            assert!(matches!(
                authz.effective_grants(&user).await,
                Err(ApiError::Forbidden(_))
            ));
            assert!(matches!(
                authz.resolved_grants(&user).await,
                Err(ApiError::Forbidden(_))
            ));
            for supplied in [None, Some(&snapshot)] {
                assert!(matches!(
                    authz.authorize_with_snapshot(&user, supplied, check()),
                    Err(ApiError::Forbidden(_))
                ));
                for refs in [vec![], vec!["standard".into()], vec!["core.admin".into()]] {
                    assert!(matches!(
                        authz.can_delegate_permission_sets(&user, &refs).await,
                        Err(ApiError::Forbidden(_))
                    ));
                    assert!(matches!(
                        authz
                            .can_delegate_permission_sets_with_snapshot(&user, supplied, &refs)
                            .await,
                        Err(ApiError::Forbidden(_))
                    ));
                    let mut tx = ctx.pool.begin().await?;
                    assert!(matches!(
                        authz
                            .can_delegate_permission_sets_on(&mut tx, &user, &refs)
                            .await,
                        Err(ApiError::Forbidden(_))
                    ));
                    assert!(matches!(
                        authz.authorize_access_fresh(&mut tx, &user, check()).await,
                        Err(ApiError::Forbidden(_))
                    ));
                    tx.rollback().await?;
                }
            }
        }
        // All public IAM handlers must reject a credential that middleware accepts.
        for path in [
            "/api/v1/identities",
            "/api/v1/identities/1",
            "/api/v1/identities/1/permissions",
            "/api/v1/identities/1/integration-tokens",
            "/api/v1/permissions/sets",
            "/api/v1/permissions/sets/by-ref/core.admin",
            "/api/v1/permissions/assignments",
        ] {
            assert_eq!(
                ctx.get(path, Some(token)).await?.status(),
                StatusCode::FORBIDDEN,
                "GET {path}"
            );
        }
        for (path, body) in [
            (
                "/api/v1/identities",
                json!({"login":"service-injected-identity"}),
            ),
            ("/api/v1/identities/1/roles", json!({"role":"admin"})),
            ("/api/v1/identities/1/freeze", json!({})),
            ("/api/v1/identities/1/unfreeze", json!({})),
            (
                "/api/v1/identities/1/integration-tokens",
                json!({"label":"service-injected-token"}),
            ),
            (
                "/api/v1/identities/1/integration-tokens/1/revoke",
                json!({}),
            ),
            (
                "/api/v1/permissions/sets",
                json!({"ref":"service.injected","grants":[]}),
            ),
            ("/api/v1/permissions/sets/1/roles", json!({"role":"admin"})),
            (
                "/api/v1/permissions/assignments",
                json!({"identity_id":1,"permission_set_ref":"core.admin"}),
            ),
        ] {
            assert_eq!(
                ctx.post(path, body, Some(token)).await?.status(),
                StatusCode::FORBIDDEN,
                "POST {path}"
            );
        }
        for path in ["/api/v1/identities/1", "/api/v1/permissions/sets/1"] {
            assert_eq!(
                ctx.put(path, json!({"grants":[]}), Some(token))
                    .await?
                    .status(),
                StatusCode::FORBIDDEN,
                "PUT {path}"
            );
        }
        for path in [
            "/api/v1/identities/1",
            "/api/v1/identities/roles/1",
            "/api/v1/identities/1/integration-tokens/1",
            "/api/v1/permissions/sets/roles/1",
            "/api/v1/permissions/assignments/1",
        ] {
            assert_eq!(
                ctx.delete(path, Some(token)).await?.status(),
                StatusCode::FORBIDDEN,
                "DELETE {path}"
            );
        }
    }
    Ok(())
}
