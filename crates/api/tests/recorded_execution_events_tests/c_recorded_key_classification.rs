//! Original Key classification and surviving concrete owner are independent.
use super::*;
use attune_common::{
    models::OwnerType,
    repositories::{
        iam_ownership::VerifiedKeyOrigins,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        key::{CreateKeyInput, KeyRepository, UpdateKeyInput},
    },
    secret_provenance::{KeyOrigin, SecretProvenance},
    secret_values::{redaction_marker, ENTITY_EXECUTION_RESULT},
};

async fn bounded_body(response: helpers::TestResponse) -> Result<(StatusCode, Value)> {
    let status = response.status();
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        axum::body::to_bytes(response.into_response().into_body(), 64 * 1024),
    )
    .await??;
    let body = serde_json::from_slice(&body)?;
    Ok((status, body))
}

async fn reader(
    fixture: &Fixture,
    key: &KeyOrigin,
    key_actions: &[&str],
    destination_decrypt: bool,
) -> Result<(String, i64)> {
    let login = format!("c_key_reader_{}", uuid::Uuid::new_v4().simple());
    let registered=fixture.ctx.post("/auth/register",json!({"login":login,"password":"TestPassword123!","display_name":"C historical reader"}),None).await?;
    assert!(registered.status().is_success());
    let registration: Value = registered.json().await?;
    let token = registration["data"]["access_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let identity = IdentityRepository::find_by_login(&fixture.ctx.pool, &login)
        .await?
        .unwrap();
    let execution_actions = if destination_decrypt {
        vec!["read", "decrypt"]
    } else {
        vec!["read"]
    };
    let set=PermissionSetRepository::create(&fixture.ctx.pool,CreatePermissionSetInput {
        r#ref:format!("recorded_emitter.{login}"),pack:Some(fixture.pack_id),pack_ref:Some("recorded_emitter".into()),label:None,description:None,
        grants:json!([
            {"resource":"keys","actions":key_actions,"scope":"pack","constraints":{"ids":[key.key_id],"refs":[key.key_ref]}}
        ]),
    }).await?;
    let assignment = PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: set.id,
        },
    )
    .await?;
    let destination = PermissionSetRepository::create(&fixture.ctx.pool, CreatePermissionSetInput {
        r#ref:format!("fixture.{login}_destination"),pack:None,pack_ref:None,label:None,description:None,
        grants:json!([{"resource":"executions","actions":execution_actions,"constraints":{"ids":[fixture.credential.execution_id]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: destination.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    Ok((token, assignment.id))
}

fn denied_or_redacted(status: StatusCode, body: &Value, pointer: &str) {
    assert!(
        status == StatusCode::FORBIDDEN
            || status == StatusCode::OK
                && body.pointer(pointer) != Some(&json!("historical-classification-value")),
        "missing source rights must deny or redact, not fail internally or disclose"
    );
}

fn expect_ok(status: StatusCode, body: &Value) {
    if status != StatusCode::OK {
        panic!("historical Key reader expected HTTP 200, got {status}; bounded error body={body}");
    }
}

#[tokio::test]
async fn original_encrypted_key_still_requires_decrypt_after_current_metadata_becomes_plain(
) -> Result<()> {
    classification_case(true, true, true).await
}

#[tokio::test]
async fn original_encrypted_key_toggle_does_not_supply_recorded_reader_decrypt() -> Result<()> {
    classification_case(true, true, false).await
}

#[tokio::test]
async fn original_plain_key_does_not_acquire_source_decrypt_after_current_metadata_becomes_encrypted(
) -> Result<()> {
    classification_case(false, true, false).await
}

#[tokio::test]
async fn original_key_toggle_does_not_supply_recorded_reader_read() -> Result<()> {
    classification_case(true, false, true).await
}

async fn classification_case(
    original_encrypted: bool,
    recorded_read: bool,
    recorded_decrypt: bool,
) -> Result<()> {
    let mut actions = Vec::new();
    if recorded_read {
        actions.push("read");
    }
    if recorded_decrypt {
        actions.push("decrypt");
    }
    let extra = [
        json!({"resource":"executions","actions":["read","decrypt"],"constraints":{"pack_refs":["recorded_emitter"]}}),
    ];
    let fixture = if original_encrypted {
        // F's actual pre-admission Key source and immutable accepted-input seal.
        // Do not rewrite its provenance or ciphertext after admission.
        fixture_with_original_input(
            false,
            false,
            &actions,
            &[],
            &extra,
            None,
            Some((
                "c_original_classification",
                "historical-classification-value",
            )),
        )
        .await?
    } else {
        fixture_with_additional_grants(false, false, &actions, &[], &extra).await?
    };
    let execution = fixture.credential.execution_id;
    let encryption = fixture
        .ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .unwrap();
    let (entity, path, origin) = if original_encrypted {
        let key_ref = attune_common::key_ref::canonical_key_ref(
            OwnerType::Pack,
            Some("recorded_emitter"),
            "c_original_classification",
        )?;
        let key = KeyRepository::find_by_ref(&fixture.ctx.pool, &key_ref)
            .await?
            .unwrap();
        (
            ENTITY_EXECUTION_CONFIG,
            "/input",
            attune_common::key_access::key_origin(&key),
        )
    } else {
        // A real stored output field, encrypted independently of its plain Key
        // source. Its provenance is fixed before the source metadata edit.
        let key = KeyRepository::create(
            &fixture.ctx.pool,
            CreateKeyInput {
                local_ref: "c_original_classification".into(),
                owner_type: OwnerType::Pack,
                owner_identity: None,
                owner_pack: Some(fixture.pack_id),
                owner_pack_ref: Some("recorded_emitter".into()),
                owner_action: None,
                owner_action_ref: None,
                owner_sensor: None,
                owner_sensor_ref: None,
                name: "Original plain source".into(),
                encrypted: false,
                encryption_key_hash: None,
                value: json!("historical-classification-value"),
            },
        )
        .await?;
        let origin = attune_common::key_access::key_origin(&key);
        let fields = prepare_secret_values(
            vec![SecretValueInput {
                json_path: "/token".into(),
                value: json!("historical-classification-value"),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&SecretProvenance::key(
                    origin.clone(),
                ))?),
            }],
            encryption,
        )?;
        let mut tx = fixture.ctx.pool.begin().await?;
        ExecutionRepository::update(
            &mut *tx,
            execution,
            UpdateExecutionInput {
                result: Some(json!({"token":redaction_marker()})),
                ..Default::default()
            },
        )
        .await?;
        ExecutionSecretValueRepository::upsert_many_with_conn(
            &mut tx,
            ENTITY_EXECUTION_RESULT,
            execution,
            &fields,
        )
        .await?;
        tx.commit().await?;
        (ENTITY_EXECUTION_RESULT, "/token", origin)
    };
    assert_eq!(origin.encrypted, original_encrypted);
    let before =
        ExecutionSecretValueRepository::find_stored_by_entity(&fixture.ctx.pool, entity, execution)
            .await?;
    let read_path = format!("/api/v1/executions/{execution}?include_secret_values=true");
    let (full, full_assignment) = reader(&fixture, &origin, &["read", "decrypt"], true).await?;
    let (read_only, _) = reader(&fixture, &origin, &["read"], true).await?;
    let (no_read, _) = reader(&fixture, &origin, &["decrypt"], true).await?;
    let (no_destination_decrypt, _) =
        reader(&fixture, &origin, &["read", "decrypt"], false).await?;
    let changed = if original_encrypted {
        json!("wrong-current-plain-value")
    } else {
        attune_common::crypto::encrypt_json(&json!("wrong-current-encrypted-value"), encryption)?
    };
    KeyRepository::update(
        &fixture.ctx.pool,
        origin.key_id,
        UpdateKeyInput {
            encrypted: Some(!original_encrypted),
            value: Some(changed),
            encryption_key_hash: Some(attune_common::crypto::hash_encryption_key(encryption)),
            ..Default::default()
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let mut ownership = VerifiedKeyOrigins::default();
    let target = ownership
        .retain_recorded(&mut tx, fixture.requester, &origin)
        .await?
        .unwrap();
    assert_eq!(ownership.managing_pack(&origin), Some(fixture.pack_id));
    let mut opposite = origin.clone();
    opposite.encrypted = !opposite.encrypted;
    assert_eq!(ownership.managing_pack(&opposite),None,"the projected proof is keyed by the complete ORIGINAL tuple, not ID or either encryption state");
    assert_eq!(target.context().encrypted, Some(original_encrypted));
    assert!(!target.accepts_new_effects());
    for mutation in 0..4 {
        let mut wrong = origin.clone();
        match mutation {
            0 => wrong.key_ref = "recorded_emitter.keys.wrong".into(),
            1 => wrong.owner_ref = Some("other_owner".into()),
            2 => wrong.owner_type = OwnerType::System,
            _ => wrong.owner_identity = Some(fixture.requester),
        }
        assert!(ownership
            .retain_recorded(&mut tx, fixture.requester, &wrong)
            .await?
            .is_none());
    }
    tx.commit().await?;
    let pointer = if entity == ENTITY_EXECUTION_CONFIG {
        format!("/data/config{path}")
    } else {
        format!("/data/result{path}")
    };
    let (status, body) = bounded_body(fixture.ctx.get(&read_path, Some(&full)).await?).await?;
    expect_ok(status, &body);
    assert_eq!(
        body.pointer(&pointer),
        Some(&json!("historical-classification-value"))
    );
    let (status, body) = bounded_body(fixture.ctx.get(&read_path, Some(&read_only)).await?).await?;
    if original_encrypted {
        denied_or_redacted(status, &body, &pointer);
    } else {
        expect_ok(status, &body);
        assert_eq!(
            body.pointer(&pointer),
            Some(&json!("historical-classification-value"))
        );
    }
    let (status, body) = bounded_body(fixture.ctx.get(&read_path, Some(&no_read)).await?).await?;
    denied_or_redacted(status, &body, &pointer);
    let (status, body) = bounded_body(
        fixture
            .ctx
            .get(&read_path, Some(&no_destination_decrypt))
            .await?,
    )
    .await?;
    denied_or_redacted(status, &body, &pointer);
    let (status, body) =
        bounded_body(fixture.ctx.get(&read_path, Some(&fixture.token)).await?).await?;
    if !recorded_read || original_encrypted && !recorded_decrypt {
        assert_eq!(status, StatusCode::FORBIDDEN);
    } else {
        expect_ok(status, &body);
        assert_eq!(
            body.pointer(&pointer),
            Some(&json!("historical-classification-value"))
        );
    }
    let after =
        ExecutionSecretValueRepository::find_stored_by_entity(&fixture.ctx.pool, entity, execution)
            .await?;
    assert_eq!(
        before, after,
        "historical ciphertext, Key classification and provenance must not be rebound"
    );
    if original_encrypted && recorded_read && recorded_decrypt {
        let mut tx = fixture.ctx.pool.begin().await?;
        RuntimeAuthorityRepository::revoke_admission(&mut tx, fixture.credential.admission_id)
            .await?;
        tx.commit().await?;
        let (status, body) = bounded_body(fixture.ctx.get(&read_path, Some(&full)).await?).await?;
        denied_or_redacted(status, &body, &pointer);
        let (status, _) =
            bounded_body(fixture.ctx.get(&read_path, Some(&fixture.token)).await?).await?;
        assert!(status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED);
        fixture.ctx.cleanup().await?;
        return Ok(());
    }
    PermissionAssignmentRepository::delete(&fixture.ctx.pool, full_assignment).await?;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let (status, body) = bounded_body(fixture.ctx.get(&read_path, Some(&full)).await?).await?;
    denied_or_redacted(status, &body, &pointer);
    let (deletion_reader, _) = reader(&fixture, &origin, &["read", "decrypt"], true).await?;
    let (status, body) =
        bounded_body(fixture.ctx.get(&read_path, Some(&deletion_reader)).await?).await?;
    expect_ok(status, &body);
    assert_eq!(
        body.pointer(&pointer),
        Some(&json!("historical-classification-value"))
    );
    KeyRepository::delete(&fixture.ctx.pool, origin.key_id).await?;
    // Reusing the canonical ref must not recreate the deleted recorded ID.
    let replacement = KeyRepository::create(
        &fixture.ctx.pool,
        CreateKeyInput {
            local_ref: "c_original_classification".into(),
            owner_type: OwnerType::Pack,
            owner_identity: None,
            owner_pack: Some(fixture.pack_id),
            owner_pack_ref: Some("recorded_emitter".into()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Replacement".into(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!("wrong-replacement"),
        },
    )
    .await?;
    assert_ne!(replacement.id, origin.key_id);
    let mut tx = fixture.ctx.pool.begin().await?;
    assert!(ownership
        .retain_recorded(&mut tx, fixture.requester, &origin)
        .await?
        .is_none());
    assert_eq!(
        ownership.managing_pack(&origin),
        None,
        "failed recheck removes an earlier verified scope"
    );
    tx.rollback().await?;
    for token in [&full, &deletion_reader, &read_only, &fixture.token] {
        let (status, body) = bounded_body(fixture.ctx.get(&read_path, Some(token)).await?).await?;
        denied_or_redacted(status, &body, &pointer);
    }
    fixture.ctx.cleanup().await?;
    Ok(())
}
