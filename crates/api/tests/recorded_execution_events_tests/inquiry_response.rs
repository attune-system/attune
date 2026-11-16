//! Recorded inquiry responses exercise the real direct submission route.
use super::*;
use attune_common::{
    models::{Inquiry, InquiryResponseOption, InquiryResponseOptionStyle},
    repositories::inquiry::{CreateInquiryInput, InquiryRepository},
};

async fn inquiry(fixture: &Fixture, parent: Option<Id>) -> Result<Inquiry> {
    let source =
        ExecutionRepository::find_by_id(&fixture.ctx.pool, fixture.credential.execution_id)
            .await?
            .ok_or("source missing")?;
    inquiry_with_snapshot(
        fixture,
        parent,
        source
            .executable_snapshot
            .as_ref()
            .ok_or("source snapshot missing")?,
    )
    .await
}

async fn inquiry_with_snapshot(
    fixture: &Fixture,
    parent: Option<Id>,
    snapshot: &attune_common::models::ExecutionExecutableSnapshot,
) -> Result<Inquiry> {
    let mut tx = fixture.ctx.pool.begin().await?;
    let creator = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            parent,
            executor: Some(fixture.credential.sponsor_identity_id),
            ..Default::default()
        },
        snapshot,
    )
    .await?;
    let inquiry = InquiryRepository::create(
        &mut *tx,
        CreateInquiryInput {
            created_by_execution: creator.id,
            prompt: "Approve?".into(),
            response_schema: None,
            response_options: vec![InquiryResponseOption {
                r#ref: "approve".into(),
                label: "Approve".into(),
                style: InquiryResponseOptionStyle::Positive,
                response: json!({"approved":true}),
            }],
            assigned_to: Some(fixture.credential.sponsor_identity_id),
            status: attune_common::models::InquiryStatus::Pending,
            response: None,
            timeout_at: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(inquiry)
}

#[tokio::test]
async fn recorded_response_requires_selected_authority_and_preserves_unrelated_automation(
) -> Result<()> {
    for permitted in [false, true] {
        let grants = if permitted {
            vec![json!({"resource":"inquiries","actions":["respond"]})]
        } else {
            vec![]
        };
        let fixture = fixture_with_additional_grants(false, false, &[], &[], &grants).await?;
        for body in [
            json!({"response":{"approved":true}}),
            json!({"option_ref":"approve"}),
        ] {
            let target = inquiry(&fixture, None).await?;
            let response = fixture
                .ctx
                .post(
                    &format!("/api/v1/inquiries/{}/respond", target.id),
                    body,
                    Some(&fixture.token),
                )
                .await?;
            assert_eq!(
                response.status(),
                if permitted {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            let stored = InquiryRepository::find_by_id(&fixture.ctx.pool, target.id)
                .await?
                .unwrap();
            assert_eq!(stored.response.is_some(), permitted);
            assert_eq!(stored.responded_by.is_some(), permitted);
        }
        // Even explicitly admitted authority cannot answer its own tree's inquiry.
        let target = inquiry(&fixture, Some(fixture.credential.execution_id)).await?;
        let response = fixture
            .ctx
            .post(
                &format!("/api/v1/inquiries/{}/respond", target.id),
                json!({"response":{"approved":true}}),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(InquiryRepository::find_by_id(&fixture.ctx.pool, target.id)
            .await?
            .unwrap()
            .response
            .is_none());
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn recorded_response_rejects_revoked_admission_without_effect() -> Result<()> {
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[json!({"resource":"inquiries","actions":["respond"]})],
    )
    .await?;
    let target = inquiry(&fixture, None).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_admission(&mut tx, fixture.credential.admission_id).await?;
    tx.commit().await?;
    let response = fixture
        .ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", target.id),
            json!({"option_ref":"approve"}),
            Some(&fixture.token),
        )
        .await?;
    assert!(matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    let stored = InquiryRepository::find_by_id(&fixture.ctx.pool, target.id)
        .await?
        .unwrap();
    assert!(
        stored.response.is_none() && stored.responded_by.is_none() && stored.responded_at.is_none()
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

async fn no_effects(fixture: &Fixture, id: Id) -> Result<()> {
    use attune_common::{
        audit::{AuditEventFilters, AuditOutcome, AuditRepository},
        repositories::execution_secret_value::ExecutionSecretValueRepository,
    };
    let row = InquiryRepository::find_by_id(&fixture.ctx.pool, id)
        .await?
        .unwrap();
    assert!(row.response.is_none() && row.responded_by.is_none() && row.responded_at.is_none());
    assert!(ExecutionSecretValueRepository::find_stored_by_entity(
        &fixture.ctx.pool,
        attune_common::secret_values::ENTITY_INQUIRY_RESPONSE,
        id
    )
    .await?
    .is_empty());
    fixture.ctx.flush_audit().await?;
    assert!(AuditRepository::search(
        &fixture.ctx.pool,
        &AuditEventFilters {
            resource_type: Some("inquiry".into()),
            resource_id: Some(id),
            outcome: Some(AuditOutcome::Success),
            ..Default::default()
        }
    )
    .await?
    .is_empty());
    Ok(())
}

#[tokio::test]
async fn recorded_respond_grant_does_not_cover_a_foreign_inquiry() -> Result<()> {
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[json!({"resource":"inquiries","actions":["respond"]})],
    )
    .await?;
    let pack = create_test_pack(&fixture.ctx.pool, "foreign_inquiry").await?;
    let action = create_test_action(
        &fixture.ctx.pool,
        pack.id,
        &pack.r#ref,
        "foreign_inquiry.ask",
    )
    .await?;
    activate_test_pack_release_with_projections(
        &fixture.ctx.pool,
        &pack,
        &PackProjectionIds {
            actions: vec![action.id],
            ..Default::default()
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let snapshot =
        ExecutableSnapshotRepository::resolve_for_action_ref(&mut *tx, &action.r#ref).await?;
    tx.commit().await?;
    let target = inquiry_with_snapshot(&fixture, None, &snapshot).await?;
    let response = fixture
        .ctx
        .post(
            &format!("/api/v1/inquiries/{}/respond", target.id),
            json!({"option_ref":"approve"}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    no_effects(&fixture, target.id).await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_response_approval_expiry_during_target_wait_cannot_commit() -> Result<()> {
    // Warm the migration template before starting the actual expiry contract.
    let warmup = TestContext::new().await?;
    warmup.cleanup().await?;
    let fixture = fixture_with_approval_lifetime(
        false,
        false,
        &[],
        &[],
        &[json!({"resource":"inquiries","actions":["respond"]})],
        Some(chrono::Duration::seconds(30)),
    )
    .await?;
    let target = inquiry(&fixture, None).await?;
    let mut read = fixture.ctx.pool.acquire().await?;
    let approval =
        RuntimeAuthorityRepository::find_approval(&mut read, fixture.signing_approval_id)
            .await?
            .ok_or("approval missing")?;
    drop(read);
    let expires = approval.expires_at.ok_or("expiry missing")?;
    let mut blocker = fixture.ctx.pool.begin().await?;
    InquiryRepository::find_by_id_for_update(&mut blocker, target.id)
        .await?
        .ok_or("target missing")?;
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let path = format!("/api/v1/inquiries/{}/respond", target.id);
    let release = async {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(10));
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)))").bind(blocker_pid).fetch_one(&fixture.ctx.pool).await?;
                if waiting { return Ok::<_, Box<dyn std::error::Error>>(()); }
                interval.tick().await;
            }
        }).await??;
        assert!(
            expires > chrono::Utc::now(),
            "source did not reach target before approval expired"
        );
        // Real elapsed time is the contract: wait until the recorded deadline,
        // then make the previously blocked target available to the response.
        let remaining = (expires - chrono::Utc::now()).to_std()?;
        tokio::time::sleep(remaining + std::time::Duration::from_millis(25)).await;
        blocker.commit().await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let (response, released) = tokio::join!(
        fixture
            .ctx
            .post(&path, json!({"option_ref":"approve"}), Some(&fixture.token)),
        release
    );
    released?;
    assert!(matches!(
        response?.status(),
        StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED
    ));
    no_effects(&fixture, target.id).await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}
