//! Stale UI pins must fail before the typed reader records an acquisition.
use super::*;
use attune_common::repositories::{
    artifact::ArtifactVersionRepository, runtime_read::RuntimeReadRepository,
};

#[tokio::test]
async fn stale_artifact_version_id_does_not_acquire_recreated_number() -> Result<()> {
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &["read", "decrypt"],
        &[],
        &[
            json!({"resource":"artifacts","actions":["read","decrypt","create","update"]}),
            json!({"resource":"executions","actions":["read","decrypt"]}),
        ],
    )
    .await?;
    let response = fixture.ctx.post("/api/v1/artifacts", json!({
        "ref":"recorded_emitter.selected_output", "scope":"pack", "owner":"recorded_emitter",
        "type":"other", "visibility":"public"
    }), Some(&fixture.token)).await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response.json().await?;
    let id = body["data"]["id"].as_i64().ok_or("artifact missing")?;
    let mut versions = Vec::new();
    for value in ["deleted", "replacement"] {
        let response = fixture
            .ctx
            .post(
                &format!("/api/v1/artifacts/{id}/versions"),
                json!({"content":{"value":value}}),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::CREATED);
        let body: Value = response.json().await?;
        let version = body["data"]["id"].as_i64().ok_or("version missing")?;
        assert_eq!(body["data"]["version"], 1);
        versions.push(version);
        if value == "deleted" {
            assert!(ArtifactVersionRepository::delete(&fixture.ctx.pool, version).await?);
        }
    }
    assert_ne!(versions[0], versions[1]);
    let executions = [fixture.credential.execution_id];
    let before = serde_json::to_value(
        RuntimeReadRepository::references_for_executions(&fixture.ctx.pool, &executions).await?,
    )?;
    for suffix in [
        "versions/1",
        "versions/latest",
        "versions/1/download",
        "download",
    ] {
        let response = fixture
            .ctx
            .get(
                &format!("/api/v1/artifacts/{id}/{suffix}?version_id={}", versions[0]),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{suffix}");
        let after = serde_json::to_value(
            RuntimeReadRepository::references_for_executions(&fixture.ctx.pool, &executions)
                .await?,
        )?;
        assert_eq!(
            before, after,
            "stale {suffix} pin captured an unread replacement version"
        );
    }
    // Positive control proves this is a permitted typed consumer, not rejection
    // by an unmarked route or unrelated credential/source denial.
    let response = fixture
        .ctx
        .get(
            &format!(
                "/api/v1/artifacts/{id}/versions/1?version_id={}",
                versions[1]
            ),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["content_json"]["value"], "replacement");
    let after = serde_json::to_value(
        RuntimeReadRepository::references_for_executions(&fixture.ctx.pool, &executions).await?,
    )?;
    assert_ne!(
        before, after,
        "positive control did not record its real acquisition"
    );
    fixture.ctx.cleanup().await
}
