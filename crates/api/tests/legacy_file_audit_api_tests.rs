//! Real authenticated maintenance routes. No production provider/stop claim is
//! synthesized; the unsupported backend path must persist actionable quarantine.
mod helpers;
use attune_api::authz::AuthorizationService;
use attune_common::repositories::{
    identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    },
    Create, Delete,
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

async fn grant(ctx: &TestContext, manage: bool) -> Result<i64> {
    let identity = ctx.user.as_ref().unwrap().id;
    let mut grants = vec![json!({"resource":"retention","actions":["read","update"]})];
    if manage {
        grants.push(json!({"resource":"permissions","actions":["manage"]}));
    }
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("test.legacy_operator_{}", Uuid::new_v4().simple()),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!(grants),
        },
    )
    .await?;
    let assignment = PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await?;
    AuthorizationService::invalidate_identity_authz_cache(identity).await;
    AuthorizationService::invalidate_permission_set_caches().await;
    Ok(assignment.id)
}

#[tokio::test]
async fn operator_api_exposes_manual_quarantine_without_releasing_unknown_native_operations(
) -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let result: Result<()> = async {
        let endpoint = "/api/v1/retention-config/file-audits";
        // TestContext::get(None) falls back to the context's login token.
        // Send no Authorization header at all to exercise real unauthenticated access.
        let anonymous = ctx.app.clone().oneshot(axum::http::Request::builder()
            .uri(format!("{endpoint}/capabilities"))
            .body(axum::body::Body::empty())?).await?;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        grant(&ctx, false).await?;
        assert_eq!(ctx.get(&format!("{endpoint}/capabilities"), ctx.token()).await?.status(), StatusCode::FORBIDDEN);
        let management_assignment = grant(&ctx, true).await?;
        let response = ctx.get(&format!("{endpoint}/capabilities"), ctx.token()).await?;
        assert_eq!(response.status(), StatusCode::OK);
        let capabilities: Value = response.json().await?;
        assert_eq!(capabilities["data"]["management_transport"], "manual_required");
        // Explicitly owned rows, not a broad historical fixture sweep.
        sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,metadata_gone,evidence_state) VALUES(98101,98102,98103,true,'quarantined')").execute(&ctx.pool).await?;
        sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) VALUES(98101,'execution',98104)").execute(&ctx.pool).await?;
        sqlx::query("INSERT INTO file_evidence_put(attempt,owner,protocol) VALUES($1,98101,'legacy_unknown')").bind(Uuid::new_v4()).execute(&ctx.pool).await?;
        let candidates = ctx.get(&format!("{endpoint}/candidates"), ctx.token()).await?;
        assert_eq!(candidates.status(), StatusCode::OK);
        let candidates: Value = candidates.json().await?;
        assert!(candidates["data"].as_array().unwrap().iter().any(|row| row["version"] == 98101));
        assert_eq!(ctx.post(endpoint, json!({"versions":[98101],"writer_stopped":true}), ctx.token()).await?.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let requested = ctx.post(endpoint, json!({"versions":[98101]}), ctx.token()).await?;
        assert_eq!(requested.status(), StatusCode::CREATED);
        let requested: Value = requested.json().await?;
        assert_eq!(requested["data"]["state"], "manual_required");
        assert!(requested["data"]["diagnostic"].as_str().unwrap().contains("export/isolation"));
        let audit = requested["data"]["id"].as_str().unwrap();
        let quarantine = ctx.post(&format!("{endpoint}/{audit}/quarantine"), json!({}), ctx.token()).await?;
        assert_eq!(quarantine.status(), StatusCode::OK);
        let receipts = ctx.get(&format!("{endpoint}/{audit}/receipts"), ctx.token()).await?;
        assert_eq!(receipts.status(), StatusCode::OK);
        let receipts: Value = receipts.json().await?;
        assert!(receipts["data"].as_array().unwrap().iter().any(|row| row["kind"] == "operator_decision" && row["proof"]["native_settlement_certified"] == false));
        let stepped = ctx.post(&format!("{endpoint}/{audit}/step"), json!({}), ctx.token()).await?;
        assert_eq!(stepped.status(), StatusCode::OK);
        let status = ctx.get(&format!("{endpoint}/{audit}"), ctx.token()).await?;
        assert_eq!(status.status(), StatusCode::OK);
        let state: Value = status.json().await?;
        assert_eq!(state["data"]["state"], "manual_required");
        let held: bool = sqlx::query_scalar("SELECT file_evidence_pinned('execution',98104) AND EXISTS(SELECT 1 FROM file_evidence_put WHERE owner=98101)").fetch_one(&ctx.pool).await?;
        assert!(held);
        let readmit = ctx.post("/api/v1/retention-config/file-evidence/98101/readmit", json!({}), ctx.token()).await?;
        assert_eq!(readmit.status(), StatusCode::OK);
        let readmit: Value = readmit.json().await?;
        assert_eq!(readmit["data"]["readmitted"], false, "readmission must not reopen a metadata-gone owner or settle its writer");
        let evidence = ctx.get("/api/v1/retention-config/file-evidence/98101", ctx.token()).await?;
        assert_eq!(evidence.status(), StatusCode::OK);
        let evidence: Value = evidence.json().await?;
        assert_eq!(evidence["data"]["unresolved_operations"], 1);
        // Keep the authenticated identity active and remove only this owned
        // management assignment. Do not invalidate caches here: the actual
        // operator gate must reject stale cached grants using fresh IAM.
        assert!(PermissionAssignmentRepository::delete(&ctx.pool, management_assignment).await?);
        let active: bool = sqlx::query_scalar("SELECT NOT frozen FROM identity WHERE id=$1").bind(ctx.user.as_ref().unwrap().id).fetch_one(&ctx.pool).await?;
        assert!(active);
        let revoked = ctx.post(&format!("{endpoint}/{audit}/step"), json!({}), ctx.token()).await?;
        let status = revoked.status();
        let body = revoked.text().await?;
        assert_eq!(status, StatusCode::FORBIDDEN, "active operator's removed IAM must be freshly checked: {body}");
        let held: bool = sqlx::query_scalar("SELECT file_evidence_pinned('execution',98104) AND EXISTS(SELECT 1 FROM file_evidence_put WHERE owner=98101)").fetch_one(&ctx.pool).await?;
        assert!(held, "denied management cannot advance settlement or release custody");
        Ok(())
    }.await;
    ctx.cleanup().await?;
    result
}

#[tokio::test]
async fn frozen_operator_access_token_is_rejected_before_maintenance_step() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let result: Result<()> = async {
        grant(&ctx, true).await?;
        sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,metadata_gone,evidence_state) VALUES(98201,98202,98203,true,'quarantined')").execute(&ctx.pool).await?;
        sqlx::query("INSERT INTO file_evidence_put(attempt,owner,protocol) VALUES($1,98201,'legacy_unknown')").bind(Uuid::new_v4()).execute(&ctx.pool).await?;
        let audit = ctx.post("/api/v1/retention-config/file-audits", json!({"versions":[98201]}), ctx.token()).await?;
        assert_eq!(audit.status(), StatusCode::CREATED);
        let audit: Value = audit.json().await?;
        let path = format!("/api/v1/retention-config/file-audits/{}/step", audit["data"]["id"].as_str().unwrap());
        assert_eq!(ctx.post(&path, json!({}), ctx.token()).await?.status(), StatusCode::OK);
        sqlx::query("UPDATE identity SET frozen=true WHERE id=$1").bind(ctx.user.as_ref().unwrap().id).execute(&ctx.pool).await?;
        let frozen = ctx.post(&path, json!({}), ctx.token()).await?;
        let status = frozen.status();
        let body = frozen.text().await?;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "frozen access-token subject must fail authentication before operator authorization: {body}");
        let unresolved: i64 = sqlx::query_scalar("SELECT count(*) FROM file_evidence_put WHERE owner=98201").fetch_one(&ctx.pool).await?;
        assert_eq!(unresolved, 1);
        Ok(())
    }.await;
    ctx.cleanup().await?;
    result
}
