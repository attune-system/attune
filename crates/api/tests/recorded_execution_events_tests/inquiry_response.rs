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
    let mut tx = fixture.ctx.pool.begin().await?;
    let creator = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: source.action,
            action_ref: source.action_ref.clone(),
            parent,
            executor: Some(fixture.credential.sponsor_identity_id),
            ..Default::default()
        },
        source
            .executable_snapshot
            .as_ref()
            .ok_or("source snapshot missing")?,
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
