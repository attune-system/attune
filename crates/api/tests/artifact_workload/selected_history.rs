//! Small, real-route foundations for the later opt-in retained-history profile.
//! These are correctness tests, not a large-scale or latency certification.
use super::retained_fixture::{RetainedHistory, RunSelectors, VersionSelector};
use super::*;
use attune_common::repositories::{
    execution::ExecutionRepository,
    file_evidence_retention::FileEvidenceRetentionRepository,
    runtime_target::{RuntimeTarget, RuntimeTargetRepository},
    schema_contract::{BindingOwner, SchemaContractRepository},
    FindById,
};

async fn assert_fixture_integrity(fixture: &RetainedHistory, expected: i64) -> Result<()> {
    let ctx = fixture.context();
    let selected = fixture.selected();
    let seed = ExecutionRepository::find_by_id(&ctx.pool, selected.execution)
        .await?
        .ok_or("selected execution missing")?;
    let binding = SchemaContractRepository::find_with_executor(
        &ctx.pool,
        BindingOwner::Execution,
        selected.execution,
    )
    .await?
    .ok_or("selected binding missing")?;
    binding.validate()?;
    // Owned-fixture integrity queries are deliberately outside request metrics.
    // Do not materialize the complete ID/binding set in the application.
    let (executions, bound, exact_snapshot): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*), count(b.entity_id), count(*) FILTER (WHERE e.executable_snapshot=$2) \
         FROM execution e LEFT JOIN runtime_schema_binding b \
         ON b.entity_kind='execution' AND b.entity_id=e.id WHERE e.action_ref=$1",
    )
    .bind(&seed.action_ref)
    .bind(serde_json::to_value(
        seed.executable_snapshot
            .as_ref()
            .ok_or("snapshot missing")?,
    )?)
    .fetch_one(&ctx.pool)
    .await?;
    assert_eq!(
        (executions, bound, exact_snapshot),
        (expected, expected, expected)
    );
    let mismatches: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_schema_binding b JOIN execution e \
         ON b.entity_kind='execution' AND b.entity_id=e.id CROSS JOIN runtime_schema_binding original \
         WHERE e.action_ref=$1 AND original.entity_kind='execution' AND original.entity_id=$2 \
         AND (b.contract,b.binding_digest) IS DISTINCT FROM (original.contract,original.binding_digest)",
    ).bind(&seed.action_ref).bind(selected.execution).fetch_one(&ctx.pool).await?;
    assert_eq!(mismatches, 0);
    let bindings: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_schema_binding WHERE entity_kind='execution'",
    )
    .fetch_one(&ctx.pool)
    .await?;
    assert_eq!(
        bindings, expected,
        "failed batches must not leave orphan bindings"
    );
    let versions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM artifact_version WHERE artifact=ANY($1)")
            .bind(selected.versions.map(|version| version.artifact).to_vec())
            .fetch_one(&ctx.pool)
            .await?;
    assert_eq!(versions, expected * 2);
    let unpaired: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM execution e WHERE e.action_ref=$1 AND \
         (SELECT count(*) FROM artifact_version v WHERE v.execution=e.id AND v.artifact=ANY($2))<>2",
    ).bind(&seed.action_ref).bind(selected.versions.map(|version| version.artifact).to_vec())
        .fetch_one(&ctx.pool).await?;
    assert_eq!(unpaired, 0);
    let admissions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_authority_admission a JOIN execution e \
         ON a.owner_kind='execution' AND a.owner_id=e.id WHERE e.action_ref=$1",
    )
    .bind(&seed.action_ref)
    .fetch_one(&ctx.pool)
    .await?;
    assert_eq!(
        admissions, 0,
        "delegated seed and historical read targets must not acquire recorded authority"
    );
    let (delegated, selected_seed, historical_authority): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE s.mode='delegated' AND s.explicit), \
         count(*) FILTER (WHERE e.id=$2 AND s.mode='delegated' AND s.explicit), \
         count(*) FILTER (WHERE e.id<>$2 AND (s.explicit OR s.admission IS NOT NULL \
         OR EXISTS(SELECT 1 FROM delegated_execution_attempt a WHERE a.execution=e.id))) \
         FROM execution e JOIN execution_authority_selection s ON s.execution=e.id \
         WHERE e.action_ref=$1",
    )
    .bind(&seed.action_ref)
    .bind(selected.execution)
    .fetch_one(&ctx.pool)
    .await?;
    assert_eq!(
        (delegated, selected_seed, historical_authority),
        (1, 1, 0),
        "only the API-accepted seed may retain executable delegated authority"
    );
    let mut tx = ctx.pool.begin().await?;
    for version in selected.versions {
        let target = RuntimeTargetRepository::resolve(
            &mut tx,
            RuntimeTarget::Artifact(version.artifact),
            seed.executor.ok_or("executor missing")?,
        )
        .await?
        .ok_or("captured artifact owner missing")?;
        assert_eq!(target.owner_id(), seed.action);
        assert_eq!(
            target.pack_id(),
            seed.executable_snapshot
                .as_ref()
                .map(|snapshot| snapshot.executable.action.pack)
        );
    }
    tx.rollback().await?;
    assert_eq!(fixture.retained(), expected as usize);
    Ok(())
}

async fn assert_selected_page(ctx: &TestContext, selected: RunSelectors) -> Result<()> {
    let (sample, body) = measure(
        ctx,
        &format!(
            "/api/v1/executions/{}/artifacts?per_page=20",
            selected.execution
        ),
    )
    .await?;
    assert_eq!(sample.status, 200, "{body}");
    assert_eq!(body["items"].as_array().ok_or("items missing")?.len(), 2);
    assert_eq!(body["has_more"], false);
    assert!(body["next_cursor"].is_null());
    for item in body["items"].as_array().ok_or("items missing")? {
        let expected = selected
            .versions
            .iter()
            .find(|version| item["id"] == version.artifact)
            .ok_or("unexpected artifact")?;
        assert_eq!(item["selected_version"]["id"], expected.id);
        assert_eq!(item["selected_version"]["version"], expected.version);
        assert_eq!(item["selected_version"]["execution"], selected.execution);
    }
    assert_eq!(sample.queries.artifact_count_reads, 0);
    assert_eq!(sample.queries.artifact_linked_history_reads, 0);
    assert_eq!(sample.queries.payload_projection_reads, 0);
    assert_eq!(sample.queries.ciphertext_reads, 0);
    for category in [
        "execution_secret_value",
        "runtime_schema_binding",
        "execution_key_read_origin",
    ] {
        assert_eq!(
            sample
                .queries
                .statement_categories
                .get(category)
                .copied()
                .unwrap_or(0),
            0
        );
    }
    Ok(())
}

async fn assert_download(
    ctx: &TestContext,
    version: VersionSelector,
    execution: i64,
    marker: &str,
) -> Result<()> {
    let response = ctx.get(&version.download_path(), ctx.token()).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_response().into_body(), 4096).await?;
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes)?,
        json!({"execution":execution,"marker":marker})
    );
    Ok(())
}

#[tokio::test]
async fn retained_read_targets_keep_real_schema_and_owner_across_two_bounded_batches() -> Result<()>
{
    let mut fixture = RetainedHistory::new(3).await?;
    let selected = fixture.selected();
    fixture.grow_to(7).await?; // One accepted selected run plus two batches of three.
    assert_fixture_integrity(&fixture, 7).await?;
    assert_selected_page(fixture.context(), selected).await?;
    assert_download(
        fixture.context(),
        selected.versions[0],
        selected.execution,
        "selected",
    )
    .await?;
    let ctx = fixture.context();
    let (history_sample, history) = measure(
        ctx,
        &format!(
            "/api/v1/artifacts/{}/versions?per_page=20",
            selected.versions[0].artifact
        ),
    )
    .await?;
    assert_eq!(history_sample.status, 200, "{history}");
    assert_eq!(
        history["items"]
            .as_array()
            .ok_or("history items missing")?
            .len(),
        7
    );
    assert_eq!(history_sample.queries.artifact_count_reads, 0);
    assert_eq!(history_sample.queries.artifact_linked_history_reads, 0);
    assert_eq!(history_sample.queries.payload_projection_reads, 0);
    assert_eq!(history_sample.queries.ciphertext_reads, 0);
    // Cross the actual selected-content guard for a batched historical target,
    // not only the original API-accepted template. This version has no body.
    let historical = &history["items"][0];
    assert_ne!(historical["execution"], selected.execution);
    let response = ctx
        .get(
            &format!(
                "/api/v1/artifacts/{}/versions/{}?version_id={}",
                selected.versions[0].artifact,
                historical["version"]
                    .as_i64()
                    .ok_or("historical version missing")?,
                historical["id"]
                    .as_i64()
                    .ok_or("historical immutable ID missing")?
            ),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["execution"], historical["execution"]);
    assert!(
        fixture.grow_to(6).await.is_err(),
        "growth must not discard retained rows"
    );
    assert!(
        fixture.grow_to(604_801).await.is_err(),
        "large runs still have an explicit ceiling"
    );
    assert_fixture_integrity(&fixture, 7).await?;
    fixture.cleanup().await
}

#[tokio::test]
async fn retained_batch_bounds_reject_invalid_configuration_before_setup() {
    assert!(RetainedHistory::new(0).await.is_err());
    assert!(RetainedHistory::new(1001).await.is_err());
}

#[tokio::test]
async fn retained_batch_failure_rolls_back_executions_bindings_and_both_log_links() -> Result<()> {
    let mut fixture = RetainedHistory::new(3).await?;
    fixture.grow_to(4).await?;
    let stderr = fixture.selected().versions[1].artifact;
    // Inject a real failure at the last write of the next batch. Existing
    // production triggers stay enabled; no fixture-owned proof rows are created.
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION retained_fixture_reject_stderr() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.artifact={stderr} THEN RAISE EXCEPTION 'owned retained batch failure'; END IF; RETURN NEW; END $$; \
         CREATE TRIGGER retained_fixture_reject_stderr BEFORE INSERT ON artifact_version \
         FOR EACH ROW EXECUTE FUNCTION retained_fixture_reject_stderr();"
    )).execute(&fixture.context().pool).await?;
    let failure = fixture
        .grow_to(7)
        .await
        .err()
        .ok_or("injected batch failure did not fire")?;
    assert!(
        failure.to_string().contains("owned retained batch failure"),
        "{failure}"
    );
    assert_fixture_integrity(&fixture, 4).await?;
    sqlx::raw_sql("DROP TRIGGER retained_fixture_reject_stderr ON artifact_version; DROP FUNCTION retained_fixture_reject_stderr();")
        .execute(&fixture.context().pool).await?;
    fixture.grow_to(7).await?;
    assert_fixture_integrity(&fixture, 7).await?;
    fixture.cleanup().await
}

#[tokio::test]
async fn selected_old_run_uses_its_highest_version_not_newer_sibling_or_mismatched_id() -> Result<()>
{
    let mut fixture = RetainedHistory::new(2).await?;
    fixture.grow_to(5).await?;
    let mut selected = fixture.selected();
    selected.versions[0] = fixture
        .append_version(selected.execution, 0, "selected-second")
        .await?;
    let newer = fixture.accept_probe("newer-sibling").await?;
    assert!(newer.versions[0].version > selected.versions[0].version);
    assert_selected_page(fixture.context(), selected).await?;
    assert_download(
        fixture.context(),
        selected.versions[0],
        selected.execution,
        "selected-second",
    )
    .await?;
    let mismatched = format!(
        "/api/v1/artifacts/{}/versions/{}/download?version_id={}",
        selected.versions[0].artifact, selected.versions[0].version, newer.versions[0].id
    );
    assert_eq!(
        fixture
            .context()
            .get(&mismatched, fixture.context().token())
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_download(
        fixture.context(),
        selected.versions[0],
        selected.execution,
        "selected-second",
    )
    .await?;
    fixture.cleanup().await
}

#[tokio::test]
async fn missing_selected_binding_denies_content_but_unrelated_poisoned_binding_does_not(
) -> Result<()> {
    let mut fixture = RetainedHistory::new(2).await?;
    fixture.grow_to(5).await?;
    let selected = fixture.selected();
    let unrelated = fixture.accept_probe("unrelated").await?;
    let ctx = fixture.context();
    // Remove the sibling's genuine binding under the actual evidence GC gate.
    // No UPDATE bypasses the immutable binding guard and no proof is forged.
    let mut tx = ctx.pool.begin().await?;
    FileEvidenceRetentionRepository::lock_reclamation(&mut tx).await?;
    sqlx::query(
        "DELETE FROM runtime_schema_binding WHERE entity_kind='execution' AND entity_id=$1",
    )
    .bind(unrelated.execution)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    assert_eq!(
        ctx.get(&unrelated.versions[0].download_path(), ctx.token())
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_selected_page(ctx, selected).await?;
    assert_download(ctx, selected.versions[0], selected.execution, "selected").await?;
    let mut tx = ctx.pool.begin().await?;
    FileEvidenceRetentionRepository::lock_reclamation(&mut tx).await?;
    sqlx::query(
        "DELETE FROM runtime_schema_binding WHERE entity_kind='execution' AND entity_id=$1",
    )
    .bind(selected.execution)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    assert_selected_page(ctx, selected).await?;
    assert_eq!(
        ctx.get(&selected.versions[0].download_path(), ctx.token())
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    fixture.cleanup().await
}

#[tokio::test]
async fn selected_content_requires_its_captured_artifact_owner_binding() -> Result<()> {
    let mut fixture = RetainedHistory::new(2).await?;
    fixture.grow_to(5).await?;
    let selected = fixture.selected();
    let ctx = fixture.context();
    assert_download(ctx, selected.versions[0], selected.execution, "selected").await?;
    let mut tx = ctx.pool.begin().await?;
    FileEvidenceRetentionRepository::lock_reclamation(&mut tx).await?;
    let removed = sqlx::query("DELETE FROM artifact_runtime_owner_binding WHERE artifact=$1")
        .bind(selected.versions[0].artifact)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    assert_eq!(removed, 1);
    tx.commit().await?;
    assert_selected_page(ctx, selected).await?;
    assert_eq!(
        ctx.get(&selected.versions[0].download_path(), ctx.token())
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_download(ctx, selected.versions[1], selected.execution, "selected").await?;
    fixture.cleanup().await
}

async fn wait_for_retained_batch_version_lock(
    observer: &mut sqlx::PgConnection,
    writer_pid: i32,
    artifact: i64,
) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            // Observe the exact two-key version lock, not merely a slow query
            // or an unrelated audit writer. This is an owned-fixture barrier.
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid \
                 WHERE a.datname=current_database() AND l.locktype='advisory' AND NOT l.granted \
                 AND l.classid=hashtext('artifact_version')::oid AND l.objid=hashtext($2::text)::oid \
                 AND l.objsubid=2 AND $1=ANY(pg_blocking_pids(a.pid)))",
            ).bind(writer_pid).bind(artifact.to_string()).fetch_one(&mut *observer).await?;
            if waiting { return Ok::<_, sqlx::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await??;
    Ok(())
}

#[tokio::test]
async fn concurrent_append_and_retained_growth_lock_versions_before_artifact_owners() -> Result<()>
{
    let mut fixture = RetainedHistory::new(2).await?;
    let mut selected = fixture.selected();
    let pool = fixture.context().pool.clone();
    let artifact = selected
        .versions
        .iter()
        .map(|version| version.artifact)
        .min()
        .ok_or("log missing")?;
    let mut writer = pool.begin().await?;
    FileEvidenceRetentionRepository::lock_capture(&mut writer).await?;
    sqlx::query(
        "SELECT set_config('lock_timeout','5s',true), set_config('statement_timeout','10s',true)",
    )
    .execute(&mut *writer)
    .await?;
    let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *writer)
        .await?;
    // Pause the REAL repository append between its version advisory admission
    // and the INSERT trigger's parent-row update. No parent row is locked yet.
    // There is no advisory-only repository helper; this fixture-only statement
    // exactly matches ArtifactVersionRepository::create's two-key lock identity.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('artifact_version'), hashtext($1::text))")
        .bind(artifact.to_string())
        .execute(&mut *writer)
        .await?;
    let mut observer = pool.acquire().await?;
    let append = async move {
        let attempt: Result<_> = async {
            wait_for_retained_batch_version_lock(&mut observer, writer_pid, artifact).await?;
            ArtifactVersionRepository::create(
                &mut *writer,
                CreateArtifactVersionInput {
                    artifact,
                    execution: Some(selected.execution),
                    content_type: Some("application/json".into()),
                    content: None,
                    content_json: Some(
                        json!({"execution":selected.execution,"marker":"concurrent"}),
                    ),
                    file_path: None,
                    meta: None,
                    created_by: None,
                },
            )
            .await
            .map_err(Into::into)
        }
        .await;
        match attempt {
            Ok(version) => {
                writer.commit().await?;
                Ok(version)
            }
            Err(error) => {
                writer.rollback().await?;
                Err(error)
            }
        }
    };
    // An old row-share -> advisory order creates a real cycle after the catalog
    // barrier. Both operations must complete; a deadlock victim is not success.
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        tokio::join!(fixture.grow_to(3), append)
    })
    .await;
    let verification: Result<()> = async {
        let (grown, appended) = outcome?;
        grown?;
        let appended = appended?;
        assert_eq!(fixture.retained(), 3);
        assert_eq!(appended.version, 2);
        let versions =
            ArtifactVersionRepository::scan_summaries(&pool, artifact, None, 20, None).await?;
        assert_eq!(
            versions.len(),
            4,
            "three retained runs plus the concurrent append"
        );
        assert_eq!(versions[2].id, appended.id);
        let concurrent_selector = VersionSelector {
            artifact,
            id: appended.id,
            version: appended.version,
        };
        *selected
            .versions
            .iter_mut()
            .find(|version| version.artifact == artifact)
            .ok_or("log selector missing")? = concurrent_selector;
        assert_selected_page(fixture.context(), selected).await?;
        assert_download(
            fixture.context(),
            concurrent_selector,
            selected.execution,
            "concurrent",
        )
        .await?;
        Ok(())
    }
    .await;
    let cleanup = fixture.cleanup().await;
    verification?;
    cleanup
}
