//! Actual reschedule router cases. Broker cases require an explicitly run-owned
//! disposable RabbitMQ vhost, never the developer/service broker.
use super::*;
use attune_common::{
    models::{Execution, OwnerType},
    mq::{Connection, MessageQueueConfig, Publisher, PublisherConfig},
    repositories::{
        declared_input_materialization::DeclaredInputMaterializationRepository as Inputs,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        key::{CreateKeyInput, KeyRepository, UpdateKeyInput},
        recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
    },
    secret_provenance::SecretProvenance,
    secret_values::{
        prepare_secret_values, redaction_marker, SecretValueInput, ENTITY_EXECUTION_CONFIG,
    },
};

struct Target {
    execution: Execution,
    key: i64,
    admission: Option<i64>,
    attempt: Option<uuid::Uuid>,
}

async fn target(fixture: &Fixture, sealed: bool) -> Result<Target> {
    target_with_mode(fixture, sealed, true).await
}

async fn target_with_mode(fixture: &Fixture, sealed: bool, recorded: bool) -> Result<Target> {
    let mut tx = fixture.ctx.pool.begin().await?;
    let policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap();
    // A separate policy at target birth, not a child of the management caller.
    // Even an unselected set can retain policy-level approval dependencies.
    let mut target_policy = policy.policy.0;
    for invocation in &mut target_policy.invocations {
        invocation.permission_sets.clear();
    }
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &target_policy).await?;
    let selected = policy.policy.select(
        policy.id,
        &policy.policy.invocations[0].binding,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: fixture.requester,
                login: fixture.ctx.user.as_ref().unwrap().login.clone(),
            },
        },
    )?;
    let snapshot = attune_common::repositories::managed_executable::ManagedExecutableRepository::resolve_invocation(
        &mut tx, &policy.policy, &selected.binding,
    ).await?;
    let encryption = fixture
        .ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .unwrap();
    let original = KeyRepository::create_encrypted(
        &mut tx,
        CreateKeyInput {
            local_ref: "requested_original".into(),
            owner_type: OwnerType::Pack,
            owner_identity: None,
            owner_pack: Some(fixture.pack_id),
            owner_pack_ref: Some("recorded_emitter".into()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Original Requested input".into(),
            encrypted: true,
            encryption_key_hash: None,
            value: json!("original private input"),
        },
        encryption,
    )
    .await?;
    let input_set = PermissionSetRepository::create(&mut *tx, CreatePermissionSetInput {
        r#ref: "fixture.requested_original_input".into(), pack: None, pack_ref: None, label: None, description: None,
        grants: json!([
            {"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[original.r#ref]}},
            {"resource":"executions","actions":["cancel"]}
        ]),
    }).await?;
    PermissionAssignmentRepository::create(
        &mut *tx,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: input_set.id,
        },
    )
    .await?;
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Requested,
            executor: Some(fixture.requester),
            permission_set_refs: selected.selected_refs.clone(),
            config: Some(json!({"input": redaction_marker()})),
            ..Default::default()
        },
        &snapshot,
    )
    .await?;
    let schema = SchemaContractRepository::materialize_action_pair(&mut tx, &snapshot).await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &schema)
        .await?;
    let admission = if recorded {
        Some(
            RuntimeAuthorityRepository::record_admission(
                &mut tx,
                RuntimeOwner::Execution(execution.id),
                &selected,
            )
            .await?
            .id,
        )
    } else {
        attune_common::repositories::delegated_execution::DelegatedExecutionRepository::select(
            &mut tx,
            execution.id,
            None,
        )
        .await?;
        None
    };
    Contributors::capture_human(&mut tx, "execution", execution.id, fixture.requester).await?;
    let fields = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/input".into(),
            value: json!("original private input"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&SecretProvenance::key(
                attune_common::key_access::key_origin(&original),
            ))?),
        }],
        encryption,
    )?;
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        ENTITY_EXECUTION_CONFIG,
        execution.id,
        &fields,
    )
    .await?;
    if sealed && recorded {
        Inputs::seal_execution_inputs(&mut tx, execution.id, encryption).await?;
    }
    tx.commit().await?;
    Ok(Target {
        execution,
        key: original.id,
        admission,
        attempt: recorded.then_some(selected.attempt),
    })
}

async fn broker(fixture: &Fixture) -> Result<Connection> {
    let url = std::env::var("ATTUNE_REQUESTED_PUBLICATION_MQ_URL").map_err(|_| {
        "Set ATTUNE_REQUESTED_PUBLICATION_MQ_URL to this run's disposable RabbitMQ vhost"
    })?;
    let connection = Connection::connect(&url).await?;
    connection
        .setup_common_infrastructure(&MessageQueueConfig::default())
        .await?;
    let publisher = Publisher::new(
        &connection,
        PublisherConfig {
            confirm_publish: true,
            timeout_secs: 3,
            exchange: "attune.executions".into(),
        },
    )
    .await?;
    fixture.ctx.state.set_publisher(Arc::new(publisher)).await;
    Ok(connection)
}

async fn marks(fixture: &Fixture, target: i64) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM execution_reschedule_state WHERE execution_id=$1")
            .bind(target)
            .fetch_one(&fixture.ctx.pool)
            .await?,
    )
}

#[tokio::test]
#[ignore = "requires run-owned PostgreSQL and disposable RabbitMQ vhost via ATTUNE_REQUESTED_PUBLICATION_MQ_URL"]
async fn delegated_reschedule_preserves_inputs_and_recorded_readiness_preserves_admission(
) -> Result<()> {
    for recorded in [false, true] {
        let fixture = fixture_with_additional_grants(
            false,
            false,
            &[],
            &[],
            &[json!({"resource":"executions","actions":["cancel"]})],
        )
        .await?;
        let target = target_with_mode(&fixture, true, recorded).await?;
        let connection = broker(&fixture).await?;
        let before = ExecutionSecretValueRepository::find_stored_by_entity(
            &fixture.ctx.pool,
            ENTITY_EXECUTION_CONFIG,
            target.execution.id,
        )
        .await?;
        let encryption = fixture
            .ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .unwrap();
        KeyRepository::update(
            &fixture.ctx.pool,
            target.key,
            UpdateKeyInput {
                value: Some(attune_common::crypto::keystore::encrypt_json(
                    &json!("changed current key"),
                    encryption,
                    target.key,
                )?),
                ..Default::default()
            },
        )
        .await?;
        let token = if recorded {
            fixture.token.as_str()
        } else {
            fixture.ctx.token().unwrap()
        };
        let response = fixture
            .ctx
            .post(
                &format!("/api/v1/executions/{}/reschedule", target.execution.id),
                json!({}),
                Some(token),
            )
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        let mut tx = fixture.ctx.pool.begin().await?;
        let after =
            RuntimeAuthorityRepository::execution_admission(&mut tx, target.execution.id).await?;
        tx.rollback().await?;
        let fields = ExecutionSecretValueRepository::find_stored_by_entity(
            &fixture.ctx.pool,
            ENTITY_EXECUTION_CONFIG,
            target.execution.id,
        )
        .await?;
        let count = marks(&fixture, target.execution.id).await?;
        *fixture.ctx.state.publisher.write().await = None;
        connection.close().await?;
        fixture.ctx.cleanup().await?;
        assert_eq!(
            status,
            if recorded {
                StatusCode::NOT_IMPLEMENTED
            } else {
                StatusCode::OK
            },
            "{body}"
        );
        assert_eq!(after.as_ref().map(|a| a.id), target.admission);
        assert_eq!(after.as_ref().map(|a| a.authority.attempt), target.attempt);
        assert_eq!(fields, before);
        assert_eq!(count, if recorded { 0 } else { 1 });
        assert!(!body.to_string().contains("original private input"));
    }
    Ok(())
}

#[tokio::test]
async fn recorded_caller_without_cancel_cannot_reschedule_an_authentic_target() -> Result<()> {
    let fixture = fixture_with_additional_grants(false, false, &[], &[], &[]).await?;
    let target = target(&fixture, true).await?;
    let response = fixture
        .ctx
        .post(
            &format!("/api/v1/executions/{}/reschedule", target.execution.id),
            json!({}),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let count = marks(&fixture, target.execution.id).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    assert!(
        ExecutionRepository::cancel_recorded_tree(&mut tx, target.execution.id, &authority)
            .await
            .is_err()
    );
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(count, 0);
    Ok(())
}

#[tokio::test]
async fn requested_reschedule_still_requires_authenticated_route_access() -> Result<()> {
    use axum::{body::Body, http::Request};
    use tower::Service;
    let fixture = fixture_with_additional_grants(false, false, &[], &[], &[]).await?;
    let target = target(&fixture, true).await?;
    // TestContext::post(None) falls back to the fixture's default human token.
    // Send the real route request without any Authorization header instead.
    let mut app = fixture.ctx.app.clone();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/executions/{}/reschedule",
                    target.execution.id
                ))
                .header("content-type", "application/json")
                .body(Body::from("{}"))?,
        )
        .await?;
    let status = response.status();
    let count = marks(&fixture, target.execution.id).await?;
    fixture.ctx.cleanup().await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(count, 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires run-owned PostgreSQL and disposable RabbitMQ vhost via ATTUNE_REQUESTED_PUBLICATION_MQ_URL"]
async fn recorded_readiness_rejection_does_not_repair_missing_target_acceptance() -> Result<()> {
    for recorded in [false, true] {
        let fixture = fixture_with_additional_grants(
            false,
            false,
            &[],
            &[],
            &[json!({"resource":"executions","actions":["cancel"]})],
        )
        .await?;
        let target = target(&fixture, false).await?;
        let connection = broker(&fixture).await?;
        let token = if recorded {
            fixture.token.as_str()
        } else {
            fixture.ctx.token().unwrap()
        };
        let response = fixture
            .ctx
            .post(
                &format!("/api/v1/executions/{}/reschedule", target.execution.id),
                json!({}),
                Some(token),
            )
            .await?;
        let status = response.status();
        let count = marks(&fixture, target.execution.id).await?;
        let seals: i64 =
            sqlx::query_scalar("SELECT count(*) FROM runtime_accepted_input WHERE execution=$1")
                .bind(target.execution.id)
                .fetch_one(&fixture.ctx.pool)
                .await?;
        *fixture.ctx.state.publisher.write().await = None;
        connection.close().await?;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        // The public readiness rejection cannot repair the original proof.
        let mut tx = fixture.ctx.pool.begin().await?;
        let encryption = fixture
            .ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .unwrap();
        assert!(attune_common::repositories::requested_execution_publication::RequestedExecutionPublicationRepository::prepare(
            &mut tx, &target.execution, encryption,
        ).await.is_err());
        tx.rollback().await?;
        assert_eq!(count, 0);
        assert_eq!(seals, 0);
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires run-owned PostgreSQL and a separate empty RabbitMQ vhost via ATTUNE_REQUESTED_PUBLICATION_FAILURE_MQ_URL"]
async fn delegated_publish_error_rolls_back_and_recorded_readiness_creates_no_marks() -> Result<()>
{
    for recorded in [false, true] {
        let fixture = fixture_with_additional_grants(
            false,
            false,
            &[],
            &[],
            &[json!({"resource":"executions","actions":["cancel"]})],
        )
        .await?;
        let target = target_with_mode(&fixture, true, recorded).await?;
        // Envelope publication uses its message type's exchange, and closed
        // connections recover. This separate run-owned vhost deliberately has
        // no common topology, so the real execution exchange remains absent on
        // the first attempt and any channel-recovery attempt.
        let url = std::env::var("ATTUNE_REQUESTED_PUBLICATION_FAILURE_MQ_URL")
            .map_err(|_| "Set ATTUNE_REQUESTED_PUBLICATION_FAILURE_MQ_URL to a separate run-owned empty RabbitMQ vhost")?;
        let connection = Connection::connect(&url).await?;
        let publisher = Publisher::new(
            &connection,
            PublisherConfig {
                confirm_publish: true,
                timeout_secs: 3,
                exchange: "attune.executions".into(),
            },
        )
        .await?;
        fixture.ctx.state.set_publisher(Arc::new(publisher)).await;
        let token = if recorded {
            fixture.token.as_str()
        } else {
            fixture.ctx.token().unwrap()
        };
        let response = fixture
            .ctx
            .post(
                &format!("/api/v1/executions/{}/reschedule", target.execution.id),
                json!({}),
                Some(token),
            )
            .await?;
        let status = response.status();
        let count = marks(&fixture, target.execution.id).await?;
        *fixture.ctx.state.publisher.write().await = None;
        connection.close().await?;
        fixture.ctx.cleanup().await?;
        assert_eq!(status, if recorded {
            StatusCode::NOT_IMPLEMENTED
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        }, "delegated publication must reach the missing exchange; recorded sponsorship stays unavailable");
        assert_eq!(count, 0);
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires run-owned PostgreSQL and disposable RabbitMQ vhost via ATTUNE_REQUESTED_PUBLICATION_MQ_URL"]
async fn recorded_caller_expiry_after_real_mark_wait_rolls_back_even_when_target_remains_valid(
) -> Result<()> {
    use attune_common::repositories::{
        maintenance::MaintenanceRepository as Maintenance,
        requested_execution_publication::RequestedExecutionPublicationRepository as Requested,
    };
    let fixture = fixture_with_approval_lifetime(
        false,
        false,
        &[],
        &[],
        &[json!({"resource":"executions","actions":["cancel"]})],
        Some(chrono::Duration::seconds(15)),
    )
    .await?;
    // No selected target API refs. The caller's external signing approval is
    // deliberately absent from the target's independent original admission.
    let target = target(&fixture, true).await?;
    let connection = broker(&fixture).await?;
    Maintenance::mark_execution_reschedule_attempt(
        &fixture.ctx.pool,
        target.execution.id,
        "test",
        "original mark",
        10,
        0,
        true,
    )
    .await?
    .unwrap();
    let mut blocker = fixture.ctx.pool.begin().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    sqlx::query(
        "SELECT execution_id FROM execution_reschedule_state WHERE execution_id=$1 FOR UPDATE",
    )
    .bind(target.execution.id)
    .fetch_one(&mut *blocker)
    .await?;
    // The route must reject new recorded scheduling before any mark wait.
    let response = fixture
        .ctx
        .post(
            &format!("/api/v1/executions/{}/reschedule", target.execution.id),
            json!({}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let pool = fixture.ctx.pool.clone();
    let credential = fixture.credential.clone();
    let execution = target.execution.clone();
    let publisher = fixture.ctx.state.get_publisher().await.unwrap();
    let encryption = fixture
        .ctx
        .state
        .config
        .security
        .encryption_key
        .clone()
        .unwrap();
    // Consume authentic pre-admitted authority directly. This preserves the
    // historical caller-expiry/mark-wait fence without enabling sponsorship.
    let mut writer = tokio::spawn(async move {
        let mut tx = pool.begin().await.unwrap();
        let published: attune_common::Result<()> = async {
            let authority =
                RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &credential)
                    .await?;
            use attune_common::repositories::runtime_target::{
                RuntimeTarget, RuntimeTargetRepository,
            };
            let caller_target = RuntimeTargetRepository::resolve(
                &mut tx,
                RuntimeTarget::Execution(execution.id),
                authority.credential_binding().sponsor_identity_id,
            )
            .await?
            .unwrap();
            assert!(authority.allows_target(&caller_target, attune_common::rbac::Action::Cancel));
            let prepared = Requested::prepare(&mut tx, &execution, &encryption)
                .await?
                .unwrap();
            prepared.lock_target(&mut tx).await?;
            Maintenance::mark_execution_reschedule_attempt(
                &mut *tx,
                execution.id,
                "test",
                "historical recorded publication",
                10,
                0,
                true,
            )
            .await?
            .unwrap();
            authority.ensure_fresh_on(&mut tx).await?;
            let message = attune_common::mq::MessageEnvelope::new(
                attune_common::mq::MessageType::ExecutionRequested,
                attune_common::mq::ExecutionRequestedPayload {
                    execution_id: execution.id,
                    action_id: execution.action,
                    action_ref: execution.action_ref.clone(),
                    parent_id: execution.parent,
                    enforcement_id: execution.enforcement,
                    config: execution.config.clone(),
                    release_id: execution.pack_release,
                    release_digest: execution.pack_release_digest.clone(),
                },
            );
            prepared
                .publish_bounded(&mut tx, publisher.publish_envelope(&message))
                .await?;
            authority.ensure_fresh_on(&mut tx).await?;
            Ok(())
        }
        .await;
        tx.rollback().await.unwrap();
        published.err().map(|error| error.to_string())
    });
    let waited = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%execution_reschedule_state%')")
                .bind(pid).fetch_one(&fixture.ctx.pool).await.unwrap();
            if blocked { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.is_ok();
    let expired = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let expired: bool = sqlx::query_scalar(
                "SELECT clock_timestamp()>=expires_at FROM runtime_external_approval WHERE id=$1",
            )
            .bind(fixture.signing_approval_id)
            .fetch_one(&fixture.ctx.pool)
            .await
            .unwrap();
            if expired {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    blocker.commit().await?;
    let status = match tokio::time::timeout(std::time::Duration::from_secs(5), &mut writer).await {
        Ok(Ok(error)) => error,
        Ok(Err(_)) => None,
        Err(_) => {
            writer.abort();
            let _ = writer.await;
            None
        }
    };
    let count: i32 = sqlx::query_scalar(
        "SELECT attempt_count FROM execution_reschedule_state WHERE execution_id=$1",
    )
    .bind(target.execution.id)
    .fetch_one(&fixture.ctx.pool)
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let encryption = fixture
        .ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .unwrap();
    let target_valid = matches!(
        Requested::prepare(&mut tx, &target.execution, encryption).await,
        Ok(Some(_))
    );
    tx.rollback().await?;
    *fixture.ctx.state.publisher.write().await = None;
    connection.close().await?;
    fixture.ctx.cleanup().await?;
    assert!(waited && expired);
    assert!(
        status.as_deref().is_some_and(|error| error
            .contains("Validated external approval expired before effect or disclosure")),
        "caller expiry must be the publication failure: {status:?}"
    );
    assert_eq!(count, 1);
    assert!(
        target_valid,
        "caller expiry must not invalidate the independent target proof"
    );
    Ok(())
}
