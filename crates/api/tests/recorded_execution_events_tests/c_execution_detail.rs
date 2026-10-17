//! Uses the centrally sealed accepted-input fixture, not a post-seal rewrite.
use super::*;
use attune_common::{
    repositories::runtime_read::{RuntimeReadRepository, RuntimeReadTarget},
    secret_provenance::{SecretOrigin, SecretProvenance},
};

#[tokio::test]
async fn recorded_detail_restores_actual_first_receipt_and_commits_complete_capture() -> Result<()>
{
    let fixture = fixture_with_additional_grants(false, false, &[], &[], &[
        json!({"resource":"executions","actions":["read","decrypt"],"constraints":{"pack_refs":["recorded_emitter"]}}),
    ]).await?;
    let execution = fixture.credential.execution_id;
    let before = ExecutionSecretValueRepository::find_stored_by_entity(
        &fixture.ctx.pool,
        ENTITY_EXECUTION_CONFIG,
        execution,
    )
    .await?;
    let input = before
        .iter()
        .find(|field| field.json_path == "/input")
        .unwrap();
    let provenance: SecretProvenance = serde_json::from_str(input.source_ref.as_deref().unwrap())?;
    assert_eq!(provenance.origins.len(), 1);
    let receipt = provenance.origins[0].clone();
    assert!(attune_common::repositories::immutable_input_origin::is_input_receipt(&receipt));

    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let response = fixture
        .ctx
        .get(
            &format!("/api/v1/executions/{execution}?include_secret_values=true"),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "recorded detail must consume the configured-key receipt context"
    );
    assert_eq!(body["data"]["config"]["input"], "private input");
    let after = ExecutionSecretValueRepository::find_stored_by_entity(
        &fixture.ctx.pool,
        ENTITY_EXECUTION_CONFIG,
        execution,
    )
    .await?;
    assert_eq!(
        before, after,
        "a GET must not rebind source ciphertext or provenance"
    );

    let mut tx = fixture.ctx.pool.begin().await?;
    let references =
        RuntimeReadRepository::references_for_executions(&mut *tx, &[execution]).await?;
    let mut found = false;
    for reference in references.into_values().flatten() {
        let SecretOrigin::Entity { entity_id, .. } = reference else {
            unreachable!()
        };
        let capture = RuntimeReadRepository::find(&mut *tx, entity_id).await?;
        if !matches!(capture.target, RuntimeReadTarget::Execution { execution: target } if target == execution)
        {
            continue;
        }
        capture.validate()?;
        let SecretOrigin::Entity {
            entity_type,
            entity_id,
            path,
        } = &receipt
        else {
            unreachable!()
        };
        assert!(capture
            .parents
            .iter()
            .any(|parent| &parent.entity_type == entity_type
                && parent.entity_id == *entity_id
                && &parent.path == path));
        assert!(capture
            .parents
            .iter()
            .any(|parent| parent.entity_type == ENTITY_EXECUTION_CONFIG
                && parent.entity_id == execution
                && parent.path == "/input"
                && parent.provenance == provenance));
        found = true;
    }
    assert!(
        found,
        "plaintext must follow a committed, reloadable complete RuntimeRead capture"
    );
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn configured_receipt_key_does_not_supply_missing_current_reader_decrypt() -> Result<()> {
    let fixture = fixture_with_additional_grants(false, false, &[], &[], &[
        json!({"resource":"executions","actions":["read"],"constraints":{"pack_refs":["recorded_emitter"]}}),
    ]).await?;
    let response = fixture
        .ctx
        .get(
            &format!(
                "/api/v1/executions/{}?include_secret_values=true",
                fixture.credential.execution_id
            ),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body = response.text().await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!body.contains("private input"));
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_detail_never_falls_back_from_a_signed_wrong_attempt() -> Result<()> {
    let fixture = fixture_with_additional_grants(false, false, &[], &[], &[
        json!({"resource":"executions","actions":["read","decrypt"],"constraints":{"pack_refs":["recorded_emitter"]}}),
    ]).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let admission =
        RuntimeAuthorityRepository::execution_admission(&mut tx, fixture.credential.execution_id)
            .await?
            .unwrap();
    tx.rollback().await?;
    let wrong = RuntimeCredentialBinding {
        execution_id: fixture.credential.execution_id,
        admission_id: fixture.credential.admission_id,
        sponsor_identity_id: fixture.credential.sponsor_identity_id,
        action_ref: fixture.credential.action_ref.clone(),
        attempt: uuid::Uuid::from_u128(fixture.credential.attempt.as_u128() ^ 1),
    };
    let token = generate_admitted_execution_token(
        &wrong,
        &admission.authority.selected_refs,
        &fixture.ctx.state.jwt_config,
        Some(360),
    )?;
    let response = fixture
        .ctx
        .get(
            &format!(
                "/api/v1/executions/{}?include_secret_values=true",
                fixture.credential.execution_id
            ),
            Some(&token),
        )
        .await?;
    let status = response.status();
    let body = response.text().await?;
    assert!(matches!(
        status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    assert!(!body.contains("private input"));
    fixture.ctx.cleanup().await?;
    Ok(())
}
