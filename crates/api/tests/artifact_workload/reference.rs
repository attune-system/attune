//! The former paired reference authorized all historical executions. It cannot
//! certify the selected-version policy: differing disclosure work is not an
//! equal-workload latency comparison. Keep its explicit gate red, never passing
//! on renamed or omitted measurements, until a matched-policy reference exists.
use super::*;

#[tokio::test]
async fn metadata_pages_do_not_become_content_disclosure_when_an_origin_is_missing() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    for denied in [false, true] {
        if denied {
            secret_origins(&ctx, execution, vec![execution_origin(i64::MAX)]).await?;
        }
        for query in ["", "?per_page=1", "?type=progress"] {
            let (sample, body) = measure(
                &ctx,
                &format!("/api/v1/executions/{execution}/artifacts{query}"),
            )
            .await?;
            assert_eq!(sample.status, 200, "{body}");
            assert_eq!(
                body["items"].as_array().ok_or("items missing")?.len(),
                if query.is_empty() { 2 } else { 1 }
            );
            assert!(body.get("pagination").is_none());
            assert_eq!(sample.queries.artifact_count_reads, 0);
            assert_eq!(sample.queries.artifact_linked_history_reads, 0);
        }
        assert_safe_runtime_log_header(&ctx, log).await?;
        assert_eq!(
            ctx.get(&format!("/api/v1/artifacts/{log}/versions/1"), ctx.token())
                .await?
                .status(),
            if denied {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::OK
            }
        );
    }
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "former aggregate-disclosure reference is not an equivalent-policy benchmark; needs a matched selected-version reference"]
async fn paired_serial_reference_latency_and_query_gate() -> Result<()> {
    Err("Paired artifact acceptance is unverified: the former whole-history reference does not implement the approved selected-version policy".into())
}
