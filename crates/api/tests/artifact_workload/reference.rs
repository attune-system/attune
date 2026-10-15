//! Controlled batching comparison, not a reconstruction of a historical build.
//! Only this test executable mounts the serial route. Both readers use the live
//! repository visibility predicate and runtime-log disclosure implementation.
use super::source_origins::save_report;
use super::*;
use attune_api::{
    auth::middleware::{AuthenticatedUser, RequireAuth},
    dto::{
        artifact::{ArtifactSummary, ExecutionArtifactQueryParams},
        common::PaginatedResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};
use attune_common::repositories::{identity::IdentityRepository, FindById};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};

async fn read_context(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
) -> ApiResult<ArtifactReadContext> {
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid fixture identity".into()))?;
    let identity = IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("Fixture identity disappeared".into()))?;
    Ok(ArtifactReadContext {
        identity_id,
        identity_attributes: identity
            .attributes
            .as_object()
            .map(|attributes| attributes.clone().into_iter().collect())
            .unwrap_or_default(),
        grants: state.authorization_service().effective_grants(user).await?,
    })
}

async fn serial_reference(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(execution): Path<i64>,
    Query(query): Query<ExecutionArtifactQueryParams>,
) -> ApiResult<Json<PaginatedResponse<ArtifactSummary>>> {
    if user.claims.token_type != attune_api::auth::jwt::TokenType::Access {
        return Err(ApiError::Forbidden(
            "The controlled reference fixture requires an access identity".into(),
        ));
    }
    let pagination = query.pagination().map_err(ApiError::BadRequest)?;
    let mut eligible = Vec::new();
    for artifact in ArtifactRepository::find_by_execution(&state.db, execution).await? {
        if query.r#type.is_some_and(|kind| kind != artifact.r#type) {
            continue;
        }
        // Deliberately reproduce per-artifact identity/readability and per-linked
        // execution materialization, without replacing authorization predicates.
        let context = read_context(&state, &user).await?;
        if !ArtifactRepository::is_readable(&state.db, artifact.id, &context).await? {
            continue;
        }
        let mut allowed = true;
        if artifact.classification == ArtifactClassification::RuntimeLog {
            for id in ArtifactRepository::linked_execution_ids(&state.db, artifact.id).await? {
                let Some(execution) = ExecutionRepository::find_by_id(&state.db, id).await? else {
                    allowed = false;
                    break;
                };
                match attune_api::secret_disclosure::authorize_runtime_log(
                    &state, &user, &execution,
                )
                .await
                {
                    Ok(()) => {}
                    Err(ApiError::Forbidden(_) | ApiError::NotFound(_)) => {
                        allowed = false;
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        if allowed {
            eligible.push(artifact);
        }
    }
    eligible.sort_by(|a, b| b.created.cmp(&a.created).then(b.id.cmp(&a.id)));
    let total = eligible.len() as u64;
    let items = eligible
        .into_iter()
        .skip(pagination.offset() as usize)
        .take(pagination.limit() as usize)
        .map(ArtifactSummary::from)
        .collect();
    Ok(Json(PaginatedResponse::new(items, &pagination, total)))
}

fn mount(ctx: &mut TestContext) {
    // Both paths have the same test router and RequireAuth extraction. The
    // optimized path is the actual production handler, not a benchmark rewrite.
    let routes = Router::new()
        .route("/benchmark/serial/{execution}", get(serial_reference))
        .route(
            "/benchmark/batched/{execution}",
            get(attune_api::routes::artifacts::list_artifacts_by_execution),
        )
        .with_state(ctx.state.clone());
    ctx.app = ctx.app.clone().merge(routes);
}

#[tokio::test]
async fn serial_reference_matches_filtered_pages_and_missing_origin_denials() -> Result<()> {
    let mut ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let execution = execution(&ctx, &action_ref).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    let progress = artifact(
        &ctx,
        "artifact_perf.run.progress",
        ArtifactClassification::General,
        Some(json!({"step":1})),
    )
    .await?;
    link(&ctx, log, execution).await?;
    link(&ctx, progress, execution).await?;
    mount(&mut ctx);
    for denied in [false, true] {
        if denied {
            secret_origins(&ctx, execution, vec![execution_origin(i64::MAX)]).await?;
        }
        for query in ["", "?per_page=1", "?per_page=1&page=2", "?type=progress"] {
            let (_, serial) =
                measure(&ctx, &format!("/benchmark/serial/{execution}{query}")).await?;
            let (sample, batched) =
                measure(&ctx, &format!("/benchmark/batched/{execution}{query}")).await?;
            assert_eq!(sample.status, 200);
            assert_eq!(
                serial, batched,
                "same scope and pagination must produce identical bodies"
            );
            assert_eq!(
                batched["pagination"]["total_items"],
                if query.contains("type=") || denied {
                    1
                } else {
                    2
                }
            );
        }
    }
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "explicit paired serial-reference/batched p95 and statement-count acceptance"]
async fn paired_serial_reference_latency_and_query_gate() -> Result<()> {
    let mut ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let stdout = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    let stderr = artifact(
        &ctx,
        "artifact_perf.run.stderr.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    let mut latest = 0;
    for _ in 0..100 {
        latest = execution(&ctx, &action_ref).await?;
        link(&ctx, stdout, latest).await?;
        link(&ctx, stderr, latest).await?;
    }
    mount(&mut ctx);
    let paths = [
        format!("/benchmark/serial/{latest}"),
        format!("/benchmark/batched/{latest}"),
    ];
    let mut samples = [Vec::new(), Vec::new()];
    let mut cold = Vec::new();
    for path in &paths {
        AuthorizationService::invalidate_identity_authz_cache(
            ctx.user.as_ref().ok_or("fixture identity missing")?.id,
        )
        .await;
        AuthorizationService::invalidate_permission_set_caches().await;
        let (sample, body) = measure(&ctx, path).await?;
        assert_eq!(sample.status, 200);
        assert_eq!(body["pagination"]["total_items"], 2);
        cold.push(sample);
    }
    for round in 0..20 {
        let mut bodies = [Value::Null, Value::Null];
        for index in [round % 2, (round + 1) % 2] {
            let (sample, body) = measure(&ctx, &paths[index]).await?;
            assert_eq!(sample.status, 200, "{body}");
            assert_eq!(body["items"].as_array().ok_or("items missing")?.len(), 2);
            samples[index].push(sample);
            bodies[index] = body;
        }
        assert_eq!(bodies[0], bodies[1]);
    }
    save_report(
        "paired-serial-reference",
        &json!({
            "fixture":"two-non-sensitive-runtime-logs-100-retained-executions",
            "permissions":"fixture.artifact_reader", "concurrency":1,
            "cache_state":"warm-after-separate-auth-cache-invalidation-samples", "completed_requests":20,
            "query_reduction_target":0.90,"p95_reduction_target":0.70,
            "comparison_kind":"controlled-serial-reference-not-historical-revision",
            "first_after_auth_cache_invalidation":cold,
            "baseline":samples[0],"treatment":samples[1],
            "identical_response_bodies":true,
            "limitations":["in-process Axum handlers, not browser/network TTFB","no historical-build speedup claim",
                "same repository visibility and runtime-log disclosure predicates; serial reads only in this test executable"]
        }),
    )?;
    ctx.cleanup().await?;
    Ok(())
}
