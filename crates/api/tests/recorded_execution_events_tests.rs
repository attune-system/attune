#[path = "recorded_execution_events_tests/c_execution_detail.rs"]
mod c_execution_detail;
#[path = "recorded_execution_events_tests/c_file_b_key_grants.rs"]
mod c_file_b_key_grants;
#[path = "recorded_execution_events_tests/c_file_sink_replay.rs"]
mod c_file_sink_replay;
#[path = "recorded_execution_events_tests/c_native_cache.rs"]
mod c_native_cache;
#[path = "recorded_execution_events_tests/c_native_contributors.rs"]
mod c_native_contributors;
#[path = "recorded_execution_events_tests/c_native_execution_wait.rs"]
mod c_native_execution_wait;
#[path = "recorded_execution_events_tests/c_native_human_transport.rs"]
mod c_native_human_transport;
#[path = "recorded_execution_events_tests/c_native_mixed.rs"]
mod c_native_mixed;
#[path = "recorded_execution_events_tests/c_native_related_wait.rs"]
mod c_native_related_wait;
#[path = "recorded_execution_events_tests/c_native_socket.rs"]
mod c_native_socket;
#[path = "recorded_execution_events_tests/c_native_transport.rs"]
mod c_native_transport;
#[path = "recorded_execution_events_tests/c_recorded_key_classification.rs"]
mod c_recorded_key_classification;
mod helpers;
#[cfg(unix)]
#[path = "recorded_execution_events_tests/http_handoff.rs"]
mod http_handoff;
#[path = "recorded_execution_events_tests/live_decoder_gate.rs"]
mod live_decoder_gate;
#[path = "recorded_execution_events_tests/live_streams.rs"]
mod live_streams;
#[path = "support/recorded_file_completion_cases.rs"]
mod recorded_file_completion_cases;
#[path = "recorded_consumers/queue_execution.rs"]
mod recorded_queue_execution_consumers_tests;
#[path = "recorded_execution_events_tests/worker_live_streams.rs"]
mod worker_live_streams;

use attune_common::{
    auth::jwt::generate_admitted_execution_token,
    models::{ExecutionStatus, Id},
    repositories::{
        component_lifecycle::PackProjectionIds,
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        execution_event_emitter::ExecutionEventEmitterRepository,
        execution_secret_value::ExecutionSecretValueRepository,
        identity::{IdentityRepository, UpdateIdentityInput},
        rule_authority::RuleAuthorityRepository,
        runtime_authority::{RuntimeAuthorityRepository, RuntimeCredentialBinding, RuntimeOwner},
        schema_contract::{BindingOwner, SchemaContractRepository},
        trigger::{CreateTriggerInput, TriggerRepository},
        Create, Delete, Update,
    },
    runtime_authority::{
        HumanAttribution, InvocationBinding, InvocationPolicy, PermissionSetDefinition,
        RequestAttribution, RuntimePolicy,
    },
    secret_values::{
        prepare_secret_values, redact_secret_path_sources, SecretPathSource, SecretSource,
        SecretValueInput, ENTITY_EVENT_PAYLOAD, ENTITY_EXECUTION_CONFIG,
    },
};
use helpers::{
    activate_test_pack_release_with_projections, create_test_action, create_test_pack, TestContext,
};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Fixture {
    ctx: TestContext,
    credential: RuntimeCredentialBinding,
    token: String,
    trigger_ref: String,
    foreign_trigger_ref: String,
    requester: Id,
    policy_id: Id,
    pack_id: Id,
    signing_approval_id: Id,
}

mod connected_recorded_notifier {
    use super::*;
    use attune_notifier::{
        service::Notification,
        subscriber_manager::SubscriberManager,
        websocket_server::{dispatch_notification, WebSocketServer},
    };
    use futures::{SinkExt, StreamExt};
    use std::{sync::Arc, time::Duration};
    use tokio_tungstenite::{
        tungstenite::{client::IntoClientRequest, Message},
        MaybeTlsStream, WebSocketStream,
    };

    pub(super) struct SocketHarness {
        pub(super) socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        pub(super) manager: Arc<SubscriberManager>,
        server: tokio::task::JoinHandle<()>,
    }
    impl Drop for SocketHarness {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    impl SocketHarness {
        async fn connect(fixture: &Fixture) -> Result<Self> {
            Self::connect_for(fixture, &fixture.token, fixture.credential.execution_id).await
        }
        pub(super) async fn connect_for(
            fixture: &Fixture,
            token: &str,
            execution: Id,
        ) -> Result<Self> {
            let manager = Arc::new(SubscriberManager::new());
            let (tx, _) = tokio::sync::broadcast::channel(16);
            let mut config = fixture.ctx.state.config.as_ref().clone();
            config.security.jwt_secret = Some(fixture.ctx.state.jwt_config.secret.clone());
            let service = WebSocketServer::new(
                config,
                tx,
                manager.clone(),
                tokio_util::sync::CancellationToken::new(),
                fixture.ctx.pool.clone(),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let router = service.router();
            let server = tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });
            let mut request = format!("ws://{address}/ws").into_client_request()?;
            request.headers_mut().insert(
                "sec-websocket-protocol",
                format!("attune.v1, attune.jwt.{token}").parse()?,
            );
            let (socket, _) = tokio_tungstenite::connect_async(request).await?;
            let mut harness = Self {
                socket,
                manager,
                server,
            };
            let welcome = tokio::time::timeout(Duration::from_secs(10), harness.socket.next())
                .await?
                .unwrap()?;
            assert!(welcome.into_text()?.contains("welcome"));
            harness
                .socket
                .send(Message::Text(
                    json!({"type":"subscribe","filter":format!("entity:execution:{execution}")})
                        .to_string()
                        .into(),
                ))
                .await?;
            tokio::time::timeout(Duration::from_secs(10), async {
                while harness.manager.subscription_count() != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            Ok(harness)
        }

        async fn baseline(&mut self, fixture: &Fixture) -> Result<Notification> {
            let notification = Notification {
                notification_type: "execution_status_changed".into(),
                entity_type: "execution".into(),
                entity_id: fixture.credential.execution_id,
                user_id: Some(fixture.requester),
                payload: json!({"result":"must-not-leak","status":"running"}),
                timestamp: chrono::Utc::now(),
            };
            dispatch_notification(&self.manager, &fixture.ctx.pool, notification.clone()).await;
            let frame = tokio::time::timeout(Duration::from_secs(10), self.socket.next())
                .await?
                .unwrap()?;
            let body: Value = serde_json::from_str(&frame.into_text()?)?;
            assert_eq!(body["type"], "notification");
            assert_eq!(body["payload"]["auth_mode"], "deferred");
            assert!(body["user_id"].is_null());
            assert!(!body.to_string().contains("must-not-leak"));
            Ok(notification)
        }

        pub(super) async fn expect_revoked_close(&mut self) -> Result<()> {
            let frame = tokio::time::timeout(Duration::from_secs(10), self.socket.next())
                .await?
                .unwrap()?;
            match frame {
                Message::Close(Some(close)) => assert_eq!(u16::from(close.code), 4401),
                _ => panic!("Revoked socket delivered a non-close frame"),
            }
            let _ = self.socket.close(None).await;
            Ok(())
        }
    }

    async fn socket_fixture() -> Result<Fixture> {
        fixture_with_additional_grants(
            false,
            false,
            &["read"],
            &[],
            &[json!({"resource":"executions","actions":["read"]})],
        )
        .await
    }

    #[tokio::test]
    async fn connected_socket_revalidates_queued_delivery_behind_database_revocation_barrier(
    ) -> Result<()> {
        let fixture = socket_fixture().await?;
        let mut socket = SocketHarness::connect(&fixture).await?;
        let mut notification = socket.baseline(&fixture).await?;
        let candidate = socket
            .manager
            .collect_delivery_candidates(&notification)
            .pop()
            .unwrap();
        let mut revoke = fixture.ctx.pool.begin().await?;
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *revoke)
            .await?;
        assert!(
            RuntimeAuthorityRepository::revoke_admission(
                &mut revoke,
                fixture.credential.admission_id
            )
            .await?
        );
        notification.payload = json!({"auth_mode":"deferred","queued_before_revocation":true});
        // Exercise the real socket dequeue path, not a fresh fanout decision.
        candidate.tx.send(notification)?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND $1 = ANY(pg_blocking_pids(pid)))")
                    .bind(blocker).fetch_one(&fixture.ctx.pool).await.unwrap();
                if waiting { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await?;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), socket.socket.next())
                .await
                .is_err()
        );
        revoke.commit().await?;
        socket.expect_revoked_close().await?;
        drop(socket);
        fixture.ctx.cleanup().await?;
        Ok(())
    }

    #[tokio::test]
    async fn connected_recorded_socket_fails_closed_when_its_database_pool_is_unavailable(
    ) -> Result<()> {
        let fixture = socket_fixture().await?;
        let mut socket = SocketHarness::connect(&fixture).await?;
        let notification = socket.baseline(&fixture).await?;
        let candidate = socket
            .manager
            .collect_delivery_candidates(&notification)
            .pop()
            .unwrap();
        fixture.ctx.pool.close().await;
        candidate.tx.send(notification)?;
        socket.expect_revoked_close().await?;
        drop(socket);
        fixture.ctx.cleanup().await?;
        Ok(())
    }

    #[tokio::test]
    async fn connected_socket_denies_approval_expiry_while_queued_target_is_locked() -> Result<()> {
        let fixture = fixture_with_approval_lifetime(
            false,
            false,
            &["read"],
            &[],
            &[
                json!({"resource":"executions","actions":["read"]}),
                json!({"resource":"artifacts","actions":["create","read"]}),
            ],
            Some(chrono::Duration::seconds(20)),
        )
        .await?;
        let created = fixture.ctx.post("/api/v1/artifacts", json!({"ref":"recorded_emitter.socket_expiry", "scope":"pack", "owner":"recorded_emitter", "type":"progress"}), Some(&fixture.token)).await?;
        assert_eq!(created.status(), StatusCode::CREATED);
        let artifact = created.json::<Value>().await?["data"]["id"]
            .as_i64()
            .unwrap();
        let mut socket = SocketHarness::connect(&fixture).await?;
        socket.baseline(&fixture).await?;
        socket
            .socket
            .send(Message::Text(
                json!({"type":"subscribe","filter":format!("entity:artifact:{artifact}")})
                    .to_string()
                    .into(),
            ))
            .await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while socket.manager.subscription_count() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let notification = Notification {
            notification_type: "artifact_updated".into(),
            entity_type: "artifact".into(),
            entity_id: artifact,
            user_id: Some(fixture.requester),
            payload: json!({"ref":"recorded_emitter.socket_expiry","private":"must-not-escape"}),
            timestamp: chrono::Utc::now(),
        };
        let candidate = socket
            .manager
            .collect_delivery_candidates(&notification)
            .pop()
            .unwrap();
        let mut blocker = fixture.ctx.pool.begin().await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        sqlx::query("SELECT id FROM artifact WHERE id=$1 FOR UPDATE")
            .bind(artifact)
            .fetch_one(&mut *blocker)
            .await?;
        candidate.tx.send(notification)?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))")
                    .bind(pid).fetch_one(&fixture.ctx.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await?;
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let expired: bool = sqlx::query_scalar("SELECT clock_timestamp() >= expires_at FROM runtime_external_approval WHERE id=$1")
                    .bind(fixture.signing_approval_id).fetch_one(&fixture.ctx.pool).await.unwrap();
                if expired { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await?;
        blocker.rollback().await?;
        socket.expect_revoked_close().await?;
        drop(socket);
        fixture.ctx.cleanup().await?;
        Ok(())
    }
}

async fn fixture(standard_only: bool) -> Result<Fixture> {
    fixture_with_workflow(standard_only, false).await
}

async fn fixture_with_workflow(standard_only: bool, workflow_context: bool) -> Result<Fixture> {
    fixture_with_key_actions(
        standard_only,
        workflow_context,
        &["read", "decrypt", "update", "delete"],
    )
    .await
}

async fn fixture_with_key_actions(
    standard_only: bool,
    workflow_context: bool,
    key_actions: &[&str],
) -> Result<Fixture> {
    fixture_with_capabilities(standard_only, workflow_context, key_actions, &[]).await
}

async fn create_signing_target(ctx: &TestContext) -> Result<()> {
    use attune_common::{
        models::OwnerType,
        repositories::key::{CreateKeyInput, KeyRepository},
    };
    let encryption = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let value = json!({
        "private_key_pem":include_str!("../../common/tests/fixtures/jwt_signing_test_private.pem"),
        "profiles":{"test":{"issuer":"approved-client","audience":"https://example.test",
            "subjects":["approved-subject"],"pack_refs":["recorded_emitter"],"max_ttl_seconds":120}}
    });
    KeyRepository::create_encrypted_in_pool(
        &ctx.pool,
        CreateKeyInput {
            local_ref: "recorded_signer".into(),
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Recorded signer".into(),
            encrypted: true,
            encryption_key_hash: None,
            value,
        },
        encryption,
    )
    .await?;
    Ok(())
}

async fn fixture_with_capabilities(
    standard_only: bool,
    workflow_context: bool,
    key_actions: &[&str],
    cache_actions: &[&str],
) -> Result<Fixture> {
    fixture_with_additional_grants(
        standard_only,
        workflow_context,
        key_actions,
        cache_actions,
        &[],
    )
    .await
}

async fn fixture_with_additional_grants(
    standard_only: bool,
    workflow_context: bool,
    key_actions: &[&str],
    cache_actions: &[&str],
    additional: &[Value],
) -> Result<Fixture> {
    fixture_with_approval_lifetime(
        standard_only,
        workflow_context,
        key_actions,
        cache_actions,
        additional,
        None,
    )
    .await
}

async fn fixture_with_approval_lifetime(
    standard_only: bool,
    workflow_context: bool,
    key_actions: &[&str],
    cache_actions: &[&str],
    additional: &[Value],
    lifetime: Option<chrono::Duration>,
) -> Result<Fixture> {
    fixture_with_approval_deadlines(
        standard_only,
        workflow_context,
        key_actions,
        cache_actions,
        additional,
        lifetime,
        None,
    )
    .await
}

async fn fixture_with_approval_expiry(
    standard_only: bool,
    workflow_context: bool,
    key_actions: &[&str],
    cache_actions: &[&str],
    additional: &[Value],
    approval_deadline: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<Fixture> {
    fixture_with_approval_deadlines(
        standard_only,
        workflow_context,
        key_actions,
        cache_actions,
        additional,
        None,
        approval_deadline,
    )
    .await
}

async fn fixture_with_approval_deadlines(
    standard_only: bool,
    workflow_context: bool,
    key_actions: &[&str],
    cache_actions: &[&str],
    additional: &[Value],
    lifetime: Option<chrono::Duration>,
    approval_deadline: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<Fixture> {
    fixture_with_original_input_deadlines(
        standard_only,
        workflow_context,
        key_actions,
        cache_actions,
        additional,
        lifetime,
        approval_deadline,
        None,
    )
    .await
}

async fn fixture_with_original_input(
    standard_only: bool,
    workflow_context: bool,
    key_actions: &[&str],
    cache_actions: &[&str],
    additional: &[Value],
    approval_deadline: Option<chrono::DateTime<chrono::Utc>>,
    original_key_input: Option<(&str, &str)>,
) -> Result<Fixture> {
    fixture_with_original_input_deadlines(
        standard_only,
        workflow_context,
        key_actions,
        cache_actions,
        additional,
        None,
        approval_deadline,
        original_key_input,
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "Exercise independent source deadlines and grants without hiding their combinations"
)]
async fn fixture_with_original_input_deadlines(
    standard_only: bool,
    workflow_context: bool,
    key_actions: &[&str],
    cache_actions: &[&str],
    additional: &[Value],
    lifetime: Option<chrono::Duration>,
    approval_deadline: Option<chrono::DateTime<chrono::Utc>>,
    original_key_input: Option<(&str, &str)>,
) -> Result<Fixture> {
    let ctx = TestContext::new().await?.with_auth().await?;
    create_signing_target(&ctx).await?;
    let requester = ctx.user.as_ref().unwrap().id;
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let issuer_set = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "fixture.recorded_signer_issuer".into(), pack: None, pack_ref: None,
        label: None, description: None,
        grants: json!([{"resource":"keys","actions":["use"],"constraints":{"refs":["system.recorded_signer"]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: requester,
            permset: issuer_set.id,
        },
    )
    .await?;
    let pack = create_test_pack(&ctx.pool, "recorded_emitter").await?;
    let other = create_test_pack(&ctx.pool, "foreign_events").await?;
    let action =
        create_test_action(&ctx.pool, pack.id, &pack.r#ref, "recorded_emitter.emit").await?;
    // Capture the actual secret field in the immutable executable/schema, not
    // an orphan secret row at a path absent from the execution's input shape.
    let action = attune_common::repositories::action::ActionRepository::update(
        &ctx.pool,
        action.id,
        attune_common::repositories::action::UpdateActionInput {
            param_schema: Some(json!({"input":{"type":"string","secret":true}})),
            ..Default::default()
        },
    )
    .await?;
    let workflow = if workflow_context {
        let workflow =
            create_test_action(&ctx.pool, pack.id, &pack.r#ref, "recorded_emitter.workflow")
                .await?;
        use attune_common::repositories::workflow::{
            CreateWorkflowDefinitionInput, WorkflowDefinitionRepository,
        };
        let definition = WorkflowDefinitionRepository::create(&ctx.pool, CreateWorkflowDefinitionInput {
            r#ref: workflow.r#ref.clone(), pack: pack.id, pack_ref: pack.r#ref.clone(),
            label: "Workflow".into(), description: None, version: "1.0.0".into(),
            param_schema: None, out_schema: None,
            definition: json!({"tasks":[{"name":"emit", "action":action.r#ref, "permission_set_refs":["standard"]}]}),
            tags: Vec::new(),
        }).await?;
        attune_common::repositories::action::ActionRepository::link_workflow_def(
            &ctx.pool,
            workflow.id,
            definition.id,
        )
        .await?;
        Some(workflow)
    } else {
        None
    };
    let mut trigger_refs = Vec::new();
    for (pack, reference) in [
        (&pack, "recorded_emitter.completed"),
        (&other, "foreign_events.completed"),
    ] {
        let trigger = TriggerRepository::create(
            &ctx.pool,
            CreateTriggerInput {
                r#ref: reference.into(),
                pack: Some(pack.id),
                pack_ref: Some(pack.r#ref.clone()),
                label: "Completed".into(),
                description: None,
                enabled: true,
                param_schema: Some(json!({})),
                event_config_schema: None,
                out_schema: Some(json!({"secret": {"type": "string", "secret": true}})),
                sensor: None,
                sensor_ref: None,
                is_adhoc: false,
                reference_visibility: Default::default(),
                reference_allowed_pack_refs: Vec::new(),
            },
        )
        .await?;
        trigger_refs.push(trigger.r#ref);
    }
    activate_test_pack_release_with_projections(
        &ctx.pool,
        &pack,
        &PackProjectionIds {
            actions: std::iter::once(action.id)
                .chain(workflow.as_ref().map(|workflow| workflow.id))
                .collect(),
            ..Default::default()
        },
    )
    .await?;
    let mut tx = ctx.pool.begin().await?;
    RuleAuthorityRepository::capture_deployment(&mut tx, pack.id).await?;
    let owner = RuntimeAuthorityRepository::find_owner(&mut tx, pack.id)
        .await?
        .unwrap();
    let snapshot =
        ExecutableSnapshotRepository::resolve_for_action_ref(&mut *tx, &action.r#ref).await?;
    let signing_grant = serde_json::from_value(json!({
        "resource": "keys", "actions": ["use"], "constraints": {"refs": ["system.recorded_signer"]}
    }))?;
    let signing_approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        pack.id,
        requester,
        vec![signing_grant],
        approval_deadline.or_else(|| lifetime.map(|lifetime| chrono::Utc::now() + lifetime)),
    )
    .await?;
    let mut definition = json!({
        "ref": "recorded_emitter.events", "grants": [
            {"resource": "events", "actions": ["create"]},
            {"resource": "keys", "actions": ["use"], "scope": "external",
                "constraints": {"refs": ["system.recorded_signer"]}}
        ]
    });
    if !key_actions.is_empty() {
        definition["grants"]
            .as_array_mut()
            .unwrap()
            .push(json!({"resource":"keys","actions":key_actions}));
    }
    if !cache_actions.is_empty() {
        definition["grants"]
            .as_array_mut()
            .unwrap()
            .push(json!({"resource":"caches","actions":cache_actions}));
    }
    definition["grants"]
        .as_array_mut()
        .unwrap()
        .extend(additional.iter().cloned());
    let definition: PermissionSetDefinition = serde_json::from_value(definition)?;
    let set = definition.compile(&owner, &[signing_approval.approval()?])?;
    let binding = match workflow {
        Some(workflow) => InvocationBinding::WorkflowTask {
            workflow_ref: workflow.r#ref,
            task_path: "/tasks/0".into(),
            action_ref: action.r#ref.clone(),
        },
        None => InvocationBinding::Action {
            action_ref: action.r#ref.clone(),
        },
    };
    let policy = RuntimePolicy {
        concrete_targets: BTreeMap::new(),
        owner,
        release_id: snapshot.release.id,
        release_digest: snapshot.release.digest.clone(),
        invocations: vec![InvocationPolicy {
            dispatch_template_digest: None,
            source_id: None,
            metadata_revision: None,
            foreign_workflow_policy: None,
            binding: binding.clone(),
            target_release_id: snapshot.release.id,
            target_release_digest: snapshot.release.digest.clone(),
            permission_sets: BTreeMap::from([(set.reference.clone(), set)]),
            standard_access: true,
        }],
    };
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy).await?;
    let refs = vec![if standard_only {
        "standard".into()
    } else {
        "recorded_emitter.events".into()
    }];
    let selected = policy.policy.select(
        policy.id,
        &binding,
        &refs,
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: requester,
                login: ctx.user.as_ref().unwrap().login.clone(),
            },
        },
    )?;
    let encryption = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let (input_value, input_provenance) = if let Some((local_ref, value)) = original_key_input {
        use attune_common::{
            models::OwnerType,
            repositories::key::{CreateKeyInput, KeyRepository},
        };
        let original = KeyRepository::create_encrypted(
            &mut tx,
            CreateKeyInput {
                local_ref: local_ref.into(),
                owner_type: OwnerType::Pack,
                owner_identity: None,
                owner_pack: Some(pack.id),
                owner_pack_ref: Some(pack.r#ref.clone()),
                owner_action: None,
                owner_action_ref: None,
                owner_sensor: None,
                owner_sensor_ref: None,
                name: "Original file input".into(),
                encrypted: true,
                encryption_key_hash: None,
                value: json!(value),
            },
            encryption,
        )
        .await?;
        // Actual original caller consent, captured before the accepted seal.
        // The runtime/API policy is not a substitute for this input grant.
        let input_set = PermissionSetRepository::create(&mut *tx,CreatePermissionSetInput {
            r#ref:"fixture.original_file_input".into(), pack:None,pack_ref:None,label:None,description:None,
            grants:json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[original.r#ref]}}]),
        }).await?;
        PermissionAssignmentRepository::create(
            &mut *tx,
            CreatePermissionAssignmentInput {
                identity: requester,
                permset: input_set.id,
            },
        )
        .await?;
        (
            value,
            attune_common::secret_provenance::SecretProvenance::key(
                attune_common::key_access::key_origin(&original),
            ),
        )
    } else {
        (
            "private input",
            attune_common::secret_provenance::SecretProvenance {
                origins: vec![attune_common::secret_provenance::SecretOrigin::Local {
                    source_kind: "parameter_schema".into(),
                    source_ref: Some("/input".into()),
                }],
                templates: vec![],
            },
        )
    };
    // This is an accepted caller-supplied parameter, not an orphan encrypted
    // row or permission borrowed from the execution's API/runtime approval.
    let (config, secret_inputs) = redact_secret_path_sources(
        json!({"input":input_value}),
        &[SecretPathSource {
            path: "/input".into(),
            source: SecretSource::Bound(input_provenance),
        }],
    );
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            executor: Some(requester),
            permission_set_refs: refs.clone(),
            status: ExecutionStatus::Running,
            config: Some(config),
            ..Default::default()
        },
        &snapshot,
    )
    .await?;
    let schema = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &schema)
        .await?;
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(execution.id),
        &selected,
    )
    .await?;
    let credential = RuntimeCredentialBinding {
        execution_id: execution.id,
        admission_id: admission.id,
        sponsor_identity_id: selected.owner.sponsor_identity_id,
        action_ref: action.r#ref,
        attempt: selected.attempt,
    };
    attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(
        &mut tx,
        "execution",
        execution.id,
        requester,
    )
    .await?;
    let key = encryption;
    let prepared = prepare_secret_values(secret_inputs, key)?;
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        ENTITY_EXECUTION_CONFIG,
        execution.id,
        &prepared,
    )
    .await?;
    // Seal only after the original schema/config/contributor/ciphertext exists,
    // and before committing admission. First delivery receipts retain the
    // original accepted consent independently of later runtime/API ceilings.
    // This helper invokes the seal once, then retains the actual first-delivery
    // receipt fences. Never write the pre-seal provenance buffer again.
    let input_receipts = attune_common::repositories::declared_input_materialization::DeclaredInputMaterializationRepository::seal_execution_inputs(
        &mut tx, execution.id, key,
    ).await?;
    for receipt in &input_receipts {
        receipt.ensure_fresh_on(&mut tx).await?;
    }
    tx.commit().await?;
    for receipt in &input_receipts {
        receipt.ensure_fresh()?;
    }
    let token =
        generate_admitted_execution_token(&credential, &refs, &ctx.state.jwt_config, Some(360))?;
    Ok(Fixture {
        ctx,
        credential,
        token,
        trigger_ref: trigger_refs[0].clone(),
        foreign_trigger_ref: trigger_refs[1].clone(),
        requester,
        policy_id: policy.id,
        pack_id: pack.id,
        signing_approval_id: signing_approval.id,
    })
}

#[tokio::test]
async fn recorded_artifact_secret_delivery_captures_runtime_read_provenance() -> Result<()> {
    use attune_common::{
        models::OwnerType,
        repositories::key::{CreateKeyInput, KeyRepository},
        secret_provenance::SecretProvenance,
    };
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
    let key = KeyRepository::create(
        &fixture.ctx.pool,
        CreateKeyInput {
            local_ref: "artifact_input".into(),
            owner_type: OwnerType::Pack,
            owner_identity: None,
            owner_pack: Some(fixture.pack_id),
            owner_pack_ref: Some("recorded_emitter".into()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Artifact input".into(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!("secret origin"),
        },
    )
    .await?;
    let provenance = SecretProvenance::key(attune_common::key_access::key_origin(&key));
    let prepared = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/input".into(),
            value: json!("secret origin"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&provenance)?),
        }],
        fixture
            .ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .unwrap(),
    )?;
    let mut tx = fixture.ctx.pool.begin().await?;
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        ENTITY_EXECUTION_CONFIG,
        fixture.credential.execution_id,
        &prepared,
    )
    .await?;
    tx.commit().await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let created = fixture
        .ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref":"recorded_emitter.secret_output", "scope":"pack", "owner":"recorded_emitter",
                "type":"other", "visibility":"public"
            }),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let body: Value = created.json().await?;
    assert_eq!(body["data"]["type"], "other");
    let id = body["data"]["id"].as_i64().unwrap();
    let version = fixture
        .ctx
        .post(
            &format!("/api/v1/artifacts/{id}/versions"),
            json!({"content":{"value":"output"}}),
            Some(&fixture.token),
        )
        .await?;
    let status = version.status();
    let published = version.json::<Value>().await?;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    assert_eq!(published["data"]["content_json"]["value"], "output");
    let readable = fixture
        .ctx
        .get(&format!("/api/v1/artifacts/{id}"), Some(&fixture.token))
        .await?;
    assert_eq!(readable.status(), StatusCode::OK);
    let version_read = fixture
        .ctx
        .get(
            &format!("/api/v1/artifacts/{id}/versions/1"),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(version_read.status(), StatusCode::OK);
    let body: Value = version_read.json().await?;
    assert_eq!(body["data"]["content_json"]["value"], "output");
    assert!(body["data"]["file_path"].is_null());
    assert!(body["data"]["object_key"].is_null());
    let captured = attune_common::repositories::runtime_read::RuntimeReadRepository::references_for_executions(
        &fixture.ctx.pool, &[fixture.credential.execution_id],
    ).await?;
    assert!(!captured[&fixture.credential.execution_id].is_empty());
    KeyRepository::delete(&fixture.ctx.pool, key.id).await?;
    let metadata = fixture
        .ctx
        .get(&format!("/api/v1/artifacts/{id}"), Some(&fixture.token))
        .await?;
    // This root has no data. A metadata read does not disclose its selected
    // version or inherit the version's Key authority.
    assert_eq!(metadata.status(), StatusCode::OK);
    let metadata: Value = metadata.json().await?;
    assert!(metadata["data"]["data"].is_null());
    assert!(metadata["data"].get("content_json").is_none());
    let denied = fixture
        .ctx
        .get(
            &format!("/api/v1/artifacts/{id}/versions/1"),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let page = fixture
        .ctx
        .get("/api/v1/artifacts", Some(&fixture.token))
        .await?;
    assert_eq!(page.status(), StatusCode::OK);
    let body: Value = page.json().await?;
    assert_eq!(body["pagination"]["total_items"], 0);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_artifact_standard_does_not_follow_recreated_action_owner() -> Result<()> {
    use attune_common::repositories::{action::ActionRepository, FindByRef};
    let fixture = fixture(true).await?;
    let action = ActionRepository::find_by_ref(&fixture.ctx.pool, "recorded_emitter.emit")
        .await?
        .unwrap();
    let original = fixture
        .ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref":"recorded_emitter.original_owner", "scope":"action",
                "owner":"recorded_emitter.emit", "type":"progress"
            }),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(original.status(), StatusCode::CREATED);
    let body: Value = original.json().await?;
    let original_id = body["data"]["id"].as_i64().unwrap();
    ActionRepository::delete(&fixture.ctx.pool, action.id).await?;
    let replacement = create_test_action(
        &fixture.ctx.pool,
        fixture.pack_id,
        "recorded_emitter",
        "recorded_emitter.emit",
    )
    .await?;
    use attune_common::repositories::{
        artifact::{ArtifactRepository, UpdateArtifactInput},
        runtime_target::{RuntimeTarget, RuntimeTargetRepository},
        Patch,
    };
    ArtifactRepository::update(
        &fixture.ctx.pool,
        original_id,
        UpdateArtifactInput {
            name: Some(Patch::Set("Label only".into())),
            ..Default::default()
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let target = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Artifact(original_id),
        fixture.credential.sponsor_identity_id,
    )
    .await?
    .unwrap();
    assert_eq!(target.owner_id(), Some(action.id));
    assert_ne!(target.owner_id(), Some(replacement.id));
    assert!(!target.accepts_new_effects());
    tx.commit().await?;
    // Exercise structural pack authority before a generic read taints this
    // process's later writes. Read-to-write closure is a separate contract.
    let denied = fixture
        .ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref":"recorded_emitter.reused", "scope":"action",
                "owner":"recorded_emitter.emit", "type":"progress"
            }),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let permitted = fixture
        .ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref":"recorded_emitter.pack_progress", "scope":"pack",
                "owner":"recorded_emitter", "type":"progress"
            }),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(permitted.status(), StatusCode::CREATED);
    let readable = fixture
        .ctx
        .get(
            &format!("/api/v1/artifacts/{original_id}"),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(readable.status(), StatusCode::OK);
    assert!(sqlx::query(
        "UPDATE artifact_runtime_owner_binding SET owner_id = $2 WHERE artifact = $1"
    )
    .bind(original_id)
    .bind(replacement.id)
    .execute(&fixture.ctx.pool)
    .await
    .is_err());
    // Unknown historical bindings cannot be reconstructed by a label-only edit.
    sqlx::query("DELETE FROM artifact_runtime_owner_binding WHERE artifact = $1")
        .bind(original_id)
        .execute(&fixture.ctx.pool)
        .await?;
    ArtifactRepository::update(
        &fixture.ctx.pool,
        original_id,
        UpdateArtifactInput {
            name: Some(Patch::Set("Still only a label".into())),
            ..Default::default()
        },
    )
    .await?;
    let denied = fixture
        .ctx
        .get(
            &format!("/api/v1/artifacts/{original_id}"),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    fixture.ctx.cleanup().await?;
    Ok(())
}

async fn sealed_decoder_fixture(fixture: &Fixture, frames: usize) -> Result<(Id, Vec<u8>)> {
    use attune_common::{
        models::{
            enums::LogStreamBackend, ArtifactClassification, ArtifactType, ArtifactVisibility,
            OwnerType, RetentionPolicyType,
        },
        repositories::{
            artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
            log_stream::LogStreamRepository,
        },
        runtime_log_completion::FrameTotals,
        runtime_read_log::{encode, ENCODING, FRAME_BYTES},
    };
    let mut tx = fixture.ctx.pool.begin().await?;
    let executable = ExecutableSnapshotRepository::resolve_for_action_ref(
        &mut *tx,
        &fixture.credential.action_ref,
    )
    .await?;
    let source = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(executable.executable.action.id),
            action_ref: fixture.credential.action_ref.clone(),
            config: Some(json!({})),
            status: ExecutionStatus::Completed,
            ..Default::default()
        },
        &executable,
    )
    .await?;
    let schema = SchemaContractRepository::materialize_action(&mut tx, &executable).await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, source.id, &schema).await?;
    tx.commit().await?;
    let artifact = ArtifactRepository::create(
        &fixture.ctx.pool,
        CreateArtifactInput {
            r#ref: "recorded_emitter.sealed_decoder".into(),
            scope: OwnerType::Pack,
            owner: "recorded_emitter".into(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Public,
            classification: ArtifactClassification::RuntimeLog,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 5,
            name: None,
            description: None,
            content_type: Some("text/plain".into()),
            data: None,
        },
    )
    .await?;
    let version = ArtifactVersionRepository::create_log_pending(
        &fixture.ctx.pool,
        artifact.id,
        &artifact.r#ref,
        LogStreamBackend::SharedFile,
        "text/plain".into(),
        Some(source.id),
        Some(json!({"log_encoding":ENCODING})),
        None,
    )
    .await?;
    let stream = LogStreamRepository::create_with_backend(
        &fixture.ctx.pool,
        version.id,
        LogStreamBackend::SharedFile,
        65536,
        100,
    )
    .await?;
    let key = fixture
        .ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .unwrap();
    let mut totals = FrameTotals::default();
    let mut ciphertext = Vec::new();
    let mut plaintext = Vec::new();
    for sequence in 0..frames {
        let bytes = vec![b'a' + (sequence % 26) as u8; FRAME_BYTES];
        let encoded = encode(key, source.id, version.id, sequence as u64, &bytes)?;
        totals.record(&encoded, bytes.len())?;
        ciphertext.extend_from_slice(&encoded);
        plaintext.extend_from_slice(&bytes);
    }
    let manifest = totals.manifest(source.id, version.id)?;
    let path = std::path::Path::new(&fixture.ctx.state.config.artifacts_dir)
        .join(version.file_path.as_ref().unwrap());
    tokio::fs::create_dir_all(path.parent().unwrap()).await?;
    tokio::fs::write(path, &ciphertext).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    // Reader-only fixture publication. F's authenticated producer/seal route has
    // separate tests; this seeds a real sealed shared file and exact proof.
    sqlx::query("UPDATE artifact_version SET body_state='ready',size_bytes=$2,sha256=$3,meta=$4 WHERE id=$1")
        .bind(version.id).bind(ciphertext.len() as i64).bind(&manifest.ciphertext_sha256)
        .bind(json!({"log_encoding":ENCODING,"encrypted_log_manifest":manifest.authenticate(key)?})).execute(&mut *tx).await?;
    LogStreamRepository::seal_shared_file(&mut tx, stream.id, ciphertext.len() as i64, false)
        .await?;
    tx.commit().await?;
    Ok((artifact.id, plaintext))
}

async fn recorded_decoder_fixture() -> Result<Fixture> {
    fixture_with_additional_grants(
        false,
        false,
        &["read"],
        &[],
        &[
            json!({"resource":"artifacts","actions":["read","decrypt"]}),
            json!({"resource":"executions","actions":["read","decrypt"]}),
        ],
    )
    .await
}

async fn decoder_route_response(
    fixture: &Fixture,
    artifact: Id,
    token: &str,
) -> Result<axum::response::Response> {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .uri(format!("/api/v1/artifacts/{artifact}/download"))
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::empty())?;
    Ok(fixture.ctx.app.clone().oneshot(request).await.unwrap())
}

#[tokio::test]
async fn recorded_large_sealed_log_route_streams_plaintext_above_the_old_eight_mib_cap(
) -> Result<()> {
    let fixture = recorded_decoder_fixture().await?;
    let (artifact, expected) = sealed_decoder_fixture(&fixture, 513).await?;
    let response = decoder_route_response(&fixture, artifact, &fixture.token).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("content-length"));
    let received = axum::body::to_bytes(response.into_body(), expected.len() + 1).await?;
    assert_eq!(received.as_ref(), expected.as_slice());
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_log_route_revalidates_already_decoded_next_chunk_at_database_revocation_barrier(
) -> Result<()> {
    use futures::TryStreamExt;
    let fixture = recorded_decoder_fixture().await?;
    let (artifact, _) = sealed_decoder_fixture(&fixture, 3).await?;
    let response = decoder_route_response(&fixture, artifact, &fixture.token).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert!(body.try_next().await?.is_some());
    let mut revoke = fixture.ctx.pool.begin().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *revoke)
        .await?;
    RuntimeAuthorityRepository::revoke_admission(&mut revoke, fixture.credential.admission_id)
        .await?;
    let next = tokio::spawn(async move { body.try_next().await });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&fixture.ctx.pool).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await?;
    revoke.commit().await?;
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(10), next)
            .await??
            .is_err()
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn human_encrypted_log_route_decodes_plaintext_and_stops_after_database_identity_freeze(
) -> Result<()> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    use futures::TryStreamExt;
    use tower::ServiceExt;
    let fixture = recorded_decoder_fixture().await?;
    let (artifact, expected) = sealed_decoder_fixture(&fixture, 3).await?;
    let set = PermissionSetRepository::create(&fixture.ctx.pool, CreatePermissionSetInput {
        r#ref:"fixture.human_log_decoder".into(), pack:None, pack_ref:None, label:None, description:None,
        grants:json!([
            {"resource":"artifacts","actions":["read","decrypt"],"constraints":{"refs":["recorded_emitter.sealed_decoder"]}},
            {"resource":"executions","actions":["read","decrypt"]}
        ]),
    }).await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(fixture.requester)
        .await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let human = fixture.ctx.token.as_deref().unwrap();
    let response = decoder_route_response(&fixture, artifact, human).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(response.into_body(), expected.len() + 1)
            .await?
            .as_ref(),
        expected.as_slice()
    );
    let response = fixture
        .ctx
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/v1/artifacts/{artifact}/versions/1/download"))
                .header("authorization", format!("Bearer {human}"))
                .body(axum::body::Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(response.into_body(), expected.len() + 1)
            .await?
            .as_ref(),
        expected.as_slice()
    );
    let response = decoder_route_response(&fixture, artifact, human).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert!(body.try_next().await?.is_some());
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    assert!(body.try_next().await.is_err());
    drop(body);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_encrypted_sse_valid_prefix_never_signals_done_after_hard_truncation() -> Result<()>
{
    use attune_common::repositories::artifact::ArtifactVersionRepository;
    use tower::ServiceExt;
    let fixture = recorded_decoder_fixture().await?;
    let (artifact, _) = sealed_decoder_fixture(&fixture, 3).await?;
    let version = ArtifactVersionRepository::find_latest(&fixture.ctx.pool, artifact)
        .await?
        .unwrap();
    let path = std::path::Path::new(&fixture.ctx.state.config.artifacts_dir)
        .join(version.file_path.unwrap());
    let file = tokio::fs::OpenOptions::new().write(true).open(path).await?;
    file.set_len(version.size_bytes.unwrap() as u64 - 1).await?;
    drop(file);
    let request = axum::http::Request::builder()
        .uri(format!("/api/v1/artifacts/{artifact}/stream"))
        .header("authorization", format!("Bearer {}", fixture.token))
        .body(axum::body::Body::empty())?;
    let response = fixture.ctx.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await?;
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("event: error"));
    assert!(!text.contains("event: done"));
    fixture.ctx.cleanup().await?;
    Ok(())
}

async fn artifact_route_after_approval_target_wait(
    lifetime: Option<chrono::Duration>,
    expire: bool,
) -> Result<()> {
    use tower::ServiceExt;
    let fixture = fixture_with_approval_lifetime(
        false,
        false,
        &["read"],
        &[],
        &[json!({"resource":"artifacts","actions":["create","read"]})],
        lifetime,
    )
    .await?;
    let created = fixture.ctx.post("/api/v1/artifacts", json!({"ref":"recorded_emitter.expiry_wait", "scope":"pack", "owner":"recorded_emitter", "type":"progress", "data":{"message":"must-not-escape-after-expiry"}}), Some(&fixture.token)).await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = created.json::<Value>().await?["data"]["id"]
        .as_i64()
        .unwrap();
    let mut blocker = fixture.ctx.pool.begin().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    sqlx::query("SELECT id FROM artifact WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *blocker)
        .await?;
    let app = fixture.ctx.app.clone();
    let request = axum::http::Request::builder()
        .uri(format!("/api/v1/artifacts/{id}"))
        .header("authorization", format!("Bearer {}", fixture.token))
        .body(axum::body::Body::empty())?;
    let response = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&fixture.ctx.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await?;
    if expire {
        tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                let expired: bool = sqlx::query_scalar("SELECT clock_timestamp() >= expires_at FROM runtime_external_approval WHERE id=$1")
                    .bind(fixture.signing_approval_id).fetch_one(&fixture.ctx.pool).await.unwrap();
                if expired { break; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await?;
    }
    blocker.rollback().await?;
    let response = tokio::time::timeout(std::time::Duration::from_secs(10), response).await??;
    assert_eq!(
        response.status(),
        if expire {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::OK
        }
    );
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024).await?;
    assert_eq!(
        String::from_utf8_lossy(&body).contains("must-not-escape-after-expiry"),
        !expire
    );
    let reads: i64 =
        sqlx::query_scalar("SELECT count(*) FROM execution_runtime_read_origin WHERE admission=$1")
            .bind(fixture.credential.admission_id)
            .fetch_one(&fixture.ctx.pool)
            .await?;
    assert_eq!(reads, i64::from(!expire));
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_artifact_route_denies_approval_expired_during_actual_target_wait() -> Result<()> {
    artifact_route_after_approval_target_wait(Some(chrono::Duration::seconds(20)), true).await
}

#[tokio::test]
async fn recorded_artifact_route_keeps_unexpired_approval_after_actual_target_wait() -> Result<()> {
    artifact_route_after_approval_target_wait(Some(chrono::Duration::hours(1)), false).await
}

#[tokio::test]
async fn recorded_artifact_route_keeps_no_expiry_approval_after_actual_target_wait() -> Result<()> {
    artifact_route_after_approval_target_wait(None, false).await
}

#[tokio::test]
async fn recorded_standard_reads_linked_root_metadata_but_not_its_protected_version() -> Result<()>
{
    let fixture = fixture(true).await?;
    let created = fixture
        .ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref":"recorded_emitter.metadata_only", "scope":"action",
                "owner":"recorded_emitter.emit", "type":"other", "visibility":"public"
            }),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = created.json::<Value>().await?;
    assert_eq!(created["data"]["type"], "other");
    let id = created["data"]["id"].as_i64().unwrap();
    let created = fixture
        .ctx
        .post(
            &format!("/api/v1/artifacts/{id}/versions"),
            json!({"content":{"message":"protected execution content"}}),
            Some(&fixture.token),
        )
        .await?;
    let status = created.status();
    let created = created.json::<Value>().await?;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(
        created["data"]["execution"],
        fixture.credential.execution_id
    );
    assert_eq!(
        created["data"]["content_json"]["message"],
        "protected execution content"
    );
    assert!(created["data"]["file_path"].is_null());
    // Standard contains no Execution read/decrypt or Artifact decrypt rights.
    // The link describes a version, not the metadata returned by this route.
    let root = fixture
        .ctx
        .get(&format!("/api/v1/artifacts/{id}"), Some(&fixture.token))
        .await?;
    assert_eq!(root.status(), StatusCode::OK);
    let root = root.json::<Value>().await?;
    assert!(root["data"]["data"].is_null());
    assert!(!root.to_string().contains("protected execution content"));
    let version = fixture
        .ctx
        .get(
            &format!("/api/v1/artifacts/{id}/versions/1"),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(version.status(), StatusCode::FORBIDDEN);
    assert!(!version
        .json::<Value>()
        .await?
        .to_string()
        .contains("protected execution content"));
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn generic_runtime_reads_are_transactional_idempotent_and_irreversibly_closed() -> Result<()>
{
    use attune_common::repositories::runtime_read::{
        RuntimeReadOrigin, RuntimeReadRepository as Reads, RuntimeReadTarget,
    };
    let fixture = fixture(false).await?;
    let origin = RuntimeReadOrigin {
        target: RuntimeReadTarget::Execution {
            execution: fixture.credential.execution_id,
        },
        provenance: vec![],
        parents: vec![],
    };
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    Reads::require_untainted(&mut tx, fixture.credential.admission_id).await?;
    Reads::record(&mut tx, &authority, origin.clone()).await?;
    tx.rollback().await?;
    assert!(Reads::references_for_executions(
        &fixture.ctx.pool,
        &[fixture.credential.execution_id]
    )
    .await?
    .is_empty());
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    let first = Reads::record(&mut tx, &authority, origin.clone()).await?;
    assert_eq!(
        first,
        Reads::record(&mut tx, &authority, origin.clone()).await?
    );
    assert!(
        Reads::require_untainted(&mut tx, fixture.credential.admission_id)
            .await
            .is_err()
    );
    tx.commit().await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    assert_eq!(
        Reads::close_execution(&mut tx, fixture.credential.execution_id).await?,
        vec![first]
    );
    tx.commit().await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    assert!(Reads::record(&mut tx, &authority, origin).await.is_err());
    assert!(Reads::lock_open(&mut tx, fixture.credential.admission_id)
        .await
        .is_err());
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn capture_admission_precedes_target_locks_and_a_waiting_sink_observes_the_read() -> Result<()>
{
    use attune_common::repositories::{
        artifact::ArtifactRepository,
        runtime_read::{
            RuntimeReadOrigin, RuntimeReadOwner, RuntimeReadRepository as Reads, RuntimeReadTarget,
        },
        runtime_target::{RuntimeTarget, RuntimeTargetRepository},
    };
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &["read"],
        &[],
        &[json!({"resource":"artifacts","actions":["read","decrypt","create","update"]})],
    )
    .await?;
    let created = fixture.ctx.post("/api/v1/artifacts", json!({"ref":"recorded_emitter.capture_order","scope":"pack","owner":"recorded_emitter","type":"progress"}), Some(&fixture.token)).await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = created.json::<Value>().await?["data"]["id"]
        .as_i64()
        .unwrap();
    let actor = attune_api::auth::middleware::AuthenticatedUser {
        claims: attune_common::auth::jwt::validate_token(
            &fixture.token,
            &fixture.ctx.state.jwt_config,
        )?,
    };
    let mut read = fixture.ctx.pool.begin().await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *read)
        .await?;
    let authority = actor.recorded_runtime_authority(&mut read).await?.unwrap();
    let pool = fixture.ctx.pool.clone();
    let effect_actor = actor.clone();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let effect = tokio::spawn(async move {
        let mut tx = pool.begin().await?;
        let _ = started.send(());
        let authority = effect_actor
            .recorded_runtime_authority(&mut tx)
            .await?
            .unwrap();
        ArtifactRepository::find_by_id_for_update(&mut tx, id)
            .await?
            .unwrap();
        let denied = Reads::require_untainted(&mut tx, authority.credential_binding().admission_id)
            .await
            .is_err();
        tx.rollback().await?;
        Ok::<_, attune_common::Error>(denied)
    });
    waiting.await?;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND $1 = ANY(pg_blocking_pids(pid)))")
                .bind(blocker).fetch_one(&fixture.ctx.pool).await.unwrap();
            if blocked { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await?;
    // The effect is already waiting at admission, so it cannot own a target row
    // while this reader still needs that target before recording its evidence.
    let target = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        RuntimeTargetRepository::resolve(
            &mut read,
            RuntimeTarget::Artifact(id),
            fixture.credential.sponsor_identity_id,
        ),
    )
    .await??
    .unwrap();
    Reads::record(
        &mut read,
        &authority,
        RuntimeReadOrigin {
            target: RuntimeReadTarget::Artifact {
                artifact: id,
                owner: RuntimeReadOwner::of(&target),
                executions: vec![],
            },
            provenance: vec![],
            parents: vec![],
        },
    )
    .await?;
    read.commit().await?;
    assert!(tokio::time::timeout(std::time::Duration::from_secs(10), effect).await???);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn generic_input_does_not_infer_purpose_from_uncaptured_execution_grants() -> Result<()> {
    use attune_common::{
        repositories::{
            execution_admission::ExecutionAdmissionRepository as Admission,
            identity::{
                CreatePermissionAssignmentInput, CreatePermissionSetInput,
                PermissionAssignmentRepository, PermissionSetRepository,
            },
            recorded_input_authority::RecordedInputAuthorityRepository as Inputs,
            runtime_read::{RuntimeReadOrigin, RuntimeReadRepository as Reads, RuntimeReadTarget},
        },
        secret_provenance::{SecretOrigin, SecretProvenance},
    };
    let fixture = fixture(false).await?;
    let execution = fixture.credential.execution_id;
    let mut capture = fixture.ctx.pool.begin().await?;
    Inputs::capture_human(&mut capture, "execution", execution, fixture.requester).await?;
    capture.commit().await?;
    let set = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.generic_input_purpose".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"executions","actions":["read","decrypt"]}]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: set.id,
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    let source = Reads::record(
        &mut tx,
        &authority,
        RuntimeReadOrigin {
            target: RuntimeReadTarget::Execution { execution },
            provenance: vec![SecretProvenance {
                origins: vec![SecretOrigin::Local {
                    source_kind: "parameter_schema".into(),
                    source_ref: None,
                }],
                templates: vec![],
            }],
            parents: vec![],
        },
    )
    .await?;
    // The execution grant was added after input capture. Current human IAM
    // cannot widen that immutable purpose receipt.
    assert!(matches!(
        Admission::input_runtime_origin_allowed(&mut tx, execution, &source).await,
        Ok(false) | Err(attune_common::Error::PermissionDenied(_))
    ));
    tx.commit().await?;
    IdentityRepository::delete(&fixture.ctx.pool, fixture.requester).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    assert!(matches!(
        Admission::input_runtime_origin_allowed(&mut tx, execution, &source).await,
        Ok(false) | Err(attune_common::Error::PermissionDenied(_))
    ));
    RuntimeAuthorityRepository::revoke_admission(&mut tx, fixture.credential.admission_id).await?;
    tx.commit().await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    assert!(
        Admission::input_runtime_origin_allowed(&mut tx, execution, &source)
            .await
            .is_err()
    );
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn qualified_event_origin_survives_partition_twins_but_never_rebinds_after_retention(
) -> Result<()> {
    use attune_common::{
        repositories::{
            event::{CreateEventInput, EventRepository},
            event_origin::{EventOriginRepository as Sources, EVENT_SOURCE_PAYLOAD},
            event_schema::EventSchemaRepository,
            native_maintenance::{
                partitions::{utc_day, PartitionRepository},
                ManagedTable,
            },
        },
        secret_provenance::{SecretOrigin, SecretProvenance},
    };
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &["read", "decrypt"],
        &[],
        &[
            json!({"resource":"events","actions":["read","decrypt"]}),
            json!({"resource":"executions","actions":["read","decrypt"]}),
        ],
    )
    .await?;
    let key = fixture
        .ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .unwrap();
    let event = EventSchemaRepository::create_internal(
        &fixture.ctx.pool,
        CreateEventInput {
            trigger: None,
            trigger_ref: fixture.trigger_ref.clone(),
            config: None,
            payload: Some(json!({"secret":"first-event-secret"})),
            trace_tag: None,
            source: None,
            source_ref: None,
            rule: None,
            rule_ref: None,
        },
        Some(key),
    )
    .await?
    .unwrap();
    let source = Sources::source_id(&fixture.ctx.pool, event.id, event.created).await?;
    let snapshot = Sources::find(&fixture.ctx.pool, source).await?;
    let twin_created = event.created + chrono::Duration::days(15);
    PartitionRepository::ensure_day(
        &fixture.ctx.pool,
        ManagedTable::Event,
        utc_day(twin_created),
        &Default::default(),
    )
    .await?;
    let sections = snapshot
        .binding
        .prepare(Some(json!({"secret":"second-event-secret"})), None)?;
    let secrets = prepare_secret_values(sections.payload_secrets, key)?;
    let mut tx = fixture.ctx.pool.begin().await?;
    sqlx::query(
        "INSERT INTO event(id,created,trigger,trigger_ref,payload) VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(event.id)
    .bind(twin_created)
    .bind(event.trigger)
    .bind(&event.trigger_ref)
    .bind(sections.payload)
    .execute(&mut *tx)
    .await?;
    let twin = EventRepository::find_by_identity(&mut *tx, event.id, twin_created)
        .await?
        .unwrap();
    let twin_source = Sources::capture(&mut tx, &twin, &snapshot.binding, &secrets, &[]).await?;
    assert_ne!(source, twin_source);
    tx.commit().await?;
    let user = attune_api::auth::middleware::AuthenticatedUser {
        claims: attune_common::auth::jwt::validate_token(
            &fixture.token,
            &fixture.ctx.state.jwt_config,
        )?,
    };
    for (kind, source_id, allowed) in [
        (ENTITY_EVENT_PAYLOAD, event.id, false),
        (EVENT_SOURCE_PAYLOAD, source, true),
    ] {
        let provenance = SecretProvenance {
            origins: vec![SecretOrigin::Entity {
                entity_type: kind.into(),
                entity_id: source_id,
                path: "/secret".into(),
            }],
            templates: vec![],
        };
        let prepared = prepare_secret_values(
            vec![SecretValueInput {
                json_path: "/input".into(),
                value: json!("copied"),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&provenance)?),
            }],
            key,
        )?;
        let mut tx = fixture.ctx.pool.begin().await?;
        ExecutionSecretValueRepository::replace_entity_with_conn(
            &mut tx,
            ENTITY_EXECUTION_CONFIG,
            fixture.credential.execution_id,
            &prepared,
        )
        .await?;
        tx.commit().await?;
        let mut tx = fixture.ctx.pool.begin().await?;
        let result = attune_api::recorded_consumers::authorize_execution_content(
            &mut tx,
            &user,
            fixture.credential.execution_id,
        )
        .await;
        assert_eq!(result.is_ok(), allowed);
        if allowed {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
    }
    sqlx::query("DELETE FROM event WHERE id = $1 AND created = $2")
        .bind(event.id)
        .bind(event.created)
        .execute(&fixture.ctx.pool)
        .await?;
    assert!(
        EventRepository::find_by_identity(&fixture.ctx.pool, event.id, twin_created)
            .await?
            .is_some()
    );
    let mut tx = fixture.ctx.pool.begin().await?;
    assert!(attune_api::recorded_consumers::authorize_execution_content(
        &mut tx,
        &user,
        fixture.credential.execution_id
    )
    .await
    .is_err());
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn human_generic_read_disclosure_rejects_bare_local_names_in_immutable_parents() -> Result<()>
{
    use attune_common::{
        repositories::{
            identity::{
                CreatePermissionAssignmentInput, CreatePermissionSetInput,
                PermissionAssignmentRepository, PermissionSetRepository,
            },
            runtime_read::{
                RuntimeReadOrigin, RuntimeReadParent, RuntimeReadRepository as Reads,
                RuntimeReadTarget,
            },
        },
        secret_provenance::{SecretOrigin, SecretProvenance},
        secret_values::ENTITY_EXECUTION_RESULT,
    };
    let fixture = fixture(false).await?;
    let set = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.generic_human_reader".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"executions","actions":["read","decrypt"]}]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: set.id,
        },
    )
    .await?;
    let token = attune_common::auth::jwt::generate_access_token(
        fixture.requester,
        &fixture.ctx.user.as_ref().unwrap().login,
        &fixture.ctx.state.jwt_config,
    )?;
    let user = attune_api::auth::middleware::AuthenticatedUser {
        claims: attune_common::auth::jwt::validate_token(&token, &fixture.ctx.state.jwt_config)?,
    };
    for local in ["pack_config", "queue_item", "trigger_schema"] {
        let parent = SecretProvenance {
            origins: vec![SecretOrigin::Local {
                source_kind: local.into(),
                source_ref: Some("display-name-is-not-proof".into()),
            }],
            templates: vec![],
        };
        let mut tx = fixture.ctx.pool.begin().await?;
        let authority =
            RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
                .await?;
        let read = Reads::record(
            &mut tx,
            &authority,
            RuntimeReadOrigin {
                target: RuntimeReadTarget::Execution {
                    execution: fixture.credential.execution_id,
                },
                provenance: vec![SecretProvenance {
                    origins: vec![SecretOrigin::Entity {
                        entity_type: ENTITY_EXECUTION_CONFIG.into(),
                        entity_id: fixture.credential.execution_id,
                        path: "/input".into(),
                    }],
                    templates: vec![],
                }],
                parents: vec![RuntimeReadParent {
                    entity_type: ENTITY_EXECUTION_CONFIG.into(),
                    entity_id: fixture.credential.execution_id,
                    path: "/input".into(),
                    provenance: parent,
                }],
            },
        )
        .await?;
        let origin = SecretProvenance {
            origins: vec![read],
            templates: vec![],
        };
        let prepared = prepare_secret_values(
            vec![SecretValueInput {
                json_path: "/data".into(),
                value: json!("private-copy"),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&origin)?),
            }],
            fixture
                .ctx
                .state
                .config
                .security
                .encryption_key
                .as_deref()
                .unwrap(),
        )?;
        ExecutionSecretValueRepository::replace_entity_with_conn(
            &mut tx,
            ENTITY_EXECUTION_RESULT,
            fixture.credential.execution_id,
            &prepared,
        )
        .await?;
        tx.commit().await?;
        let revealed = attune_api::secret_disclosure::reveal_authorized(
            &fixture.ctx.state,
            &user,
            Some(json!({"data":attune_common::secret_values::redaction_marker()})),
            ENTITY_EXECUTION_RESULT,
            fixture.credential.execution_id,
        )
        .await?;
        // Field-level denial keeps the marker. A successful response must not
        // be mistaken for permission to restore the encrypted value.
        assert_eq!(
            revealed,
            Some(json!({"data":attune_common::secret_values::redaction_marker()}))
        );
    }
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn generic_runtime_reads_propagate_into_whole_event_sections() -> Result<()> {
    use attune_common::repositories::runtime_read::{
        RuntimeReadOrigin, RuntimeReadRepository as Reads, RuntimeReadTarget, ENTITY_RUNTIME_READ,
    };
    let fixture = fixture(false).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    Reads::record(
        &mut tx,
        &authority,
        RuntimeReadOrigin {
            target: RuntimeReadTarget::Execution {
                execution: fixture.credential.execution_id,
            },
            provenance: vec![],
            parents: vec![],
        },
    )
    .await?;
    tx.commit().await?;
    let response = fixture
        .ctx
        .post(
            "/api/v1/events",
            json!({"trigger_ref":fixture.trigger_ref,"payload":{"secret":"runtime-read-copy"}}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response.json().await?;
    assert!(!body.to_string().contains("runtime-read-copy"));
    let event = body["data"]["id"].as_i64().unwrap();
    let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &fixture.ctx.pool,
        ENTITY_EVENT_PAYLOAD,
        event,
    )
    .await?;
    assert_eq!(secrets.len(), 1);
    assert_eq!(secrets[0].json_path, "");
    assert!(secrets[0]
        .source_ref
        .as_deref()
        .unwrap()
        .contains(ENTITY_RUNTIME_READ));
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn runtime_read_closure_waits_for_the_protected_read_transaction() -> Result<()> {
    use attune_common::repositories::runtime_read::{
        RuntimeReadOrigin, RuntimeReadRepository as Reads, RuntimeReadTarget,
    };
    let fixture = fixture(false).await?;
    let execution = fixture.credential.execution_id;
    let mut delivery = fixture.ctx.pool.begin().await?;
    let authority = RuntimeAuthorityRepository::validate_execution_credential(
        &mut delivery,
        &fixture.credential,
    )
    .await?;
    Reads::lock_open(&mut delivery, fixture.credential.admission_id).await?;
    let pool = fixture.ctx.pool.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let mut close = tokio::spawn(async move {
        let mut tx = pool.begin().await.unwrap();
        started_tx.send(()).unwrap();
        let origins = Reads::close_execution(&mut tx, execution).await.unwrap();
        tx.commit().await.unwrap();
        origins
    });
    started_rx.await?;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut close)
            .await
            .is_err()
    );
    let read = Reads::record(
        &mut delivery,
        &authority,
        RuntimeReadOrigin {
            target: RuntimeReadTarget::Execution { execution },
            provenance: vec![],
            parents: vec![],
        },
    )
    .await?;
    delivery.commit().await?;
    let closed = tokio::time::timeout(std::time::Duration::from_secs(10), close).await??;
    assert_eq!(closed, vec![read]);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn runtime_disclosure_never_rebases_a_captured_parent_to_its_wider_current_value(
) -> Result<()> {
    use attune_common::{
        models::OwnerType,
        repositories::runtime_read::{
            RuntimeReadOrigin, RuntimeReadParent, RuntimeReadRepository as Reads, RuntimeReadTarget,
        },
        secret_provenance::{KeyOrigin, SecretOrigin, SecretProvenance},
    };
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &["read", "decrypt"],
        &[],
        &[json!({"resource":"executions","actions":["read","decrypt"]})],
    )
    .await?;
    let execution = fixture.credential.execution_id;
    let wider = SecretProvenance {
        origins: vec![SecretOrigin::Local {
            source_kind: "parameter_schema".into(),
            source_ref: None,
        }],
        templates: vec![],
    };
    let prepared = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/input".into(),
            value: json!("replacement"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&wider)?),
        }],
        fixture
            .ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .unwrap(),
    )?;
    let mut tx = fixture.ctx.pool.begin().await?;
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut tx,
        ENTITY_EXECUTION_CONFIG,
        execution,
        &prepared,
    )
    .await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    let original = SecretProvenance::key(KeyOrigin {
        key_id: i64::MAX,
        key_ref: "system.removed_source".into(),
        owner_type: OwnerType::System,
        owner_identity: None,
        owner_ref: None,
        encrypted: true,
    });
    Reads::record(
        &mut tx,
        &authority,
        RuntimeReadOrigin {
            target: RuntimeReadTarget::Execution { execution },
            provenance: vec![SecretProvenance {
                origins: vec![SecretOrigin::Entity {
                    entity_type: ENTITY_EXECUTION_CONFIG.into(),
                    entity_id: execution,
                    path: "/input".into(),
                }],
                templates: vec![],
            }],
            parents: vec![RuntimeReadParent {
                entity_type: ENTITY_EXECUTION_CONFIG.into(),
                entity_id: execution,
                path: "/input".into(),
                provenance: original,
            }],
        },
    )
    .await?;
    tx.commit().await?;
    let user = attune_api::auth::middleware::AuthenticatedUser {
        claims: attune_common::auth::jwt::validate_token(
            &fixture.token,
            &fixture.ctx.state.jwt_config,
        )?,
    };
    let mut tx = fixture.ctx.pool.begin().await?;
    assert!(
        attune_api::recorded_consumers::authorize_execution_content(&mut tx, &user, execution)
            .await
            .is_err()
    );
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_artifact_progress_is_transactional_local_and_terminal_fenced() -> Result<()> {
    let fixture = fixture(true).await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let created = fixture.ctx.post("/api/v1/artifacts", json!({
        "ref": "recorded_emitter.progress", "scope": "action",
        "owner": "recorded_emitter.emit", "type": "progress", "data": ["previous private data"]
    }), Some(&fixture.token)).await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let body: Value = created.json().await?;
    let id = body["data"]["id"].as_i64().unwrap();
    let foreign = fixture
        .ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref": "recorded_emitter.progress", "scope": "pack",
                "owner": "foreign_events", "type": "progress"
            }),
            Some(&fixture.token),
        )
        .await?;
    // Authority is checked before uniqueness, even for an existing local ref.
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    let appended = fixture
        .ctx
        .post(
            &format!("/api/v1/artifacts/{id}/progress"),
            json!({"entry": {"message":"running"}}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(appended.status(), StatusCode::OK);
    let body: Value = appended.json().await?;
    assert!(
        body["data"]["data"].is_null(),
        "Mutation response must not disclose previous data"
    );
    let moved = fixture
        .ctx
        .put(
            &format!("/api/v1/artifacts/{id}"),
            json!({"scope":"pack", "owner":"foreign_events"}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(moved.status(), StatusCode::FORBIDDEN);
    // Keep mutable progress and generic JSON versions as separate artifacts.
    // File-body publication belongs to the encrypted upload fixtures.
    let generic = fixture
        .ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref":"recorded_emitter.progress_versions", "scope":"action",
                "owner":"recorded_emitter.emit", "type":"other", "visibility":"public",
                "data":["previous private data"]
            }),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(generic.status(), StatusCode::CREATED);
    let generic = generic.json::<Value>().await?;
    assert_eq!(generic["data"]["type"], "other");
    let generic_id = generic["data"]["id"].as_i64().unwrap();
    let forged = fixture
        .ctx
        .post(
            &format!("/api/v1/artifacts/{generic_id}/versions"),
            json!({"content":{"message":"new"}, "execution":fixture.credential.execution_id + 1}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(forged.status(), StatusCode::FORBIDDEN);
    let version = fixture
        .ctx
        .post(
            &format!("/api/v1/artifacts/{generic_id}/versions"),
            json!({"content":{"message":"new"}}),
            Some(&fixture.token),
        )
        .await?;
    let status = version.status();
    let body: Value = version.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["data"]["execution"], fixture.credential.execution_id);
    assert_eq!(body["data"]["content_json"]["message"], "new");
    let version = body["data"]["version"].as_i64().unwrap();
    // Linking to secret-bearing input invokes the stricter disclosure gate.
    let denied = fixture
        .ctx
        .get(
            &format!("/api/v1/artifacts/{generic_id}"),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    let deleted = fixture
        .ctx
        .delete(
            &format!("/api/v1/artifacts/{generic_id}/versions/{version}"),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(deleted.status(), StatusCode::OK);
    let readable = fixture
        .ctx
        .get(
            &format!("/api/v1/artifacts/{generic_id}"),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(readable.status(), StatusCode::OK);
    let body: Value = readable.json().await?;
    assert_eq!(body["data"]["data"][0], "previous private data");
    let page = fixture
        .ctx
        .get("/api/v1/artifacts?per_page=1", Some(&fixture.token))
        .await?;
    assert_eq!(page.status(), StatusCode::OK);
    let body: Value = page.json().await?;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["pagination"]["total_items"], 2);
    // A sink without origin encryption cannot launder a protected SDK read.
    let tainted_write = fixture
        .ctx
        .post(
            &format!("/api/v1/artifacts/{id}/progress"),
            json!({"entry":"copied private data"}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(tainted_write.status(), StatusCode::FORBIDDEN);
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    let denied = fixture
        .ctx
        .post(
            &format!("/api/v1/artifacts/{id}/progress"),
            json!({"entry":"after completion"}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let denied = fixture
        .ctx
        .get(&format!("/api/v1/artifacts/{id}"), Some(&fixture.token))
        .await?;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_cache_namespace_lifecycle_is_local_and_keeps_managed_metadata_gated() -> Result<()>
{
    use attune_common::{
        config::CacheAdmissionConfig,
        repositories::cache::{
            CacheNamespacePolicy, CacheNamespaceRepository, CacheOwnerScope,
            ManagedCacheNamespaceDefinition,
        },
    };
    let fixture = fixture_with_capabilities(
        false,
        false,
        &["read"],
        &["read", "create", "update", "delete"],
    )
    .await?;
    CacheNamespaceRepository::upsert_managed_definitions(
        &fixture.ctx.pool,
        fixture.pack_id,
        "recorded_emitter",
        &[ManagedCacheNamespaceDefinition {
            mode: Default::default(),
            definition_ref: "recorded_emitter.defined".into(),
            owner: CacheOwnerScope::pack(fixture.pack_id, Some("recorded_emitter".into())),
            namespace: "defined".into(),
            policy: CacheNamespacePolicy::default(),
        }],
        &CacheAdmissionConfig::default(),
    )
    .await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let request =
        json!({"owner_type":"pack", "owner_ref":"recorded_emitter", "namespace":"created"});
    let response = fixture
        .ctx
        .post(
            "/api/v1/cache/namespaces",
            request.clone(),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        fixture
            .ctx
            .post("/api/v1/cache/namespaces", request, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    for owner in ["foreign_events", "missing_owner"] {
        assert_eq!(
            fixture
                .ctx
                .post(
                    "/api/v1/cache/namespaces",
                    json!({"owner_type":"pack", "owner_ref":owner,
            "namespace":"created"}),
                    Some(&fixture.token)
                )
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let response = fixture
        .ctx
        .put(
            "/api/v1/cache/namespaces/created",
            json!({"owner_type":"pack", "owner_ref":"recorded_emitter",
        "freshness_target_seconds":30}),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["freshness_target_seconds"], 30);
    let path = "/api/v1/cache/namespaces/created?owner_type=pack&owner_ref=recorded_emitter";
    assert_eq!(
        fixture
            .ctx
            .delete(path, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::OK
    );
    let updated = fixture
        .ctx
        .put(
            "/api/v1/cache/namespaces/defined",
            json!({"owner_type":"pack",
        "owner_ref":"recorded_emitter", "freshness_target_seconds":30}),
            Some(&fixture.token),
        )
        .await?;
    let deleted = fixture
        .ctx
        .delete(
            "/api/v1/cache/namespaces/defined?owner_type=pack&owner_ref=recorded_emitter",
            Some(&fixture.token),
        )
        .await?;
    for response in [updated, deleted] {
        let status = response.status();
        let body: Value = response.json().await?;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["code"], "pack_managed_namespace");
    }
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_cache_writes_use_selected_rights_and_keep_execution_attribution() -> Result<()> {
    let fixture =
        fixture_with_capabilities(false, false, &["read"], &["read", "create", "update"]).await?;
    grant_cache_setup(&fixture).await?;
    for (owner, namespace, mode) in [
        ("recorded_emitter", "published", "snapshot"),
        ("foreign_events", "published", "snapshot"),
        ("recorded_emitter", "live_data", "live"),
    ] {
        assert_eq!(
            fixture
                .ctx
                .post(
                    "/api/v1/cache/namespaces",
                    json!({"owner_type":"pack", "owner_ref":owner,
            "namespace":namespace, "mode":mode}),
                    fixture.ctx.token()
                )
                .await?
                .status(),
            StatusCode::CREATED
        );
    }
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let generation = publish_cache_snapshot(&fixture, "published", &fixture.token).await?;
    let response = fixture.ctx.get(&format!("/api/v1/cache/namespaces/published/generations/{generation}?owner_type=pack&owner_ref=recorded_emitter"), Some(&fixture.token)).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["created_by_execution"], fixture.credential.execution_id,
        "{body}"
    );
    assert_eq!(fixture.ctx.post("/api/v1/cache/namespaces/published/generations", json!({"owner_type":"pack",
        "owner_ref":"foreign_events", "client_refresh_id":"denied", "expected_active_generation_id":null,
        "expected_chunk_count":0}), Some(&fixture.token)).await?.status(), StatusCode::FORBIDDEN);
    let applied = fixture.ctx.post("/api/v1/cache/namespaces/live_data/entries/apply", json!({
        "owner_type":"pack","owner_ref":"recorded_emitter","request_id":"recorded-write", "expected_revision":0,
        "entries":[{"external_id":"a","value":"recorded-live"}]
    }), Some(&fixture.token)).await?;
    let status = applied.status();
    let body: Value = applied.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["revision"], 1);
    let response = fixture
        .ctx
        .post(
            "/api/v1/cache/namespaces/live_data/entries/lookup",
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter","external_id":"a"
            }),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["item"]["value"], "recorded-live");
    fixture.ctx.flush_audit().await?;
    let audits = attune_common::audit::AuditRepository::search(
        &fixture.ctx.pool,
        &attune_common::audit::AuditEventFilters {
            event_type: Some("cache.generation.created".into()),
            actor_identity: Some(fixture.credential.sponsor_identity_id),
            outcome: Some(attune_common::audit::AuditOutcome::Success),
            limit: Some(10),
            ..Default::default()
        },
    )
    .await?;
    let audit = audits
        .first()
        .expect("Recorded cache refresh should be audited under its sponsor");
    let details = audit.details.as_ref().unwrap();
    assert_eq!(
        details["runtime_authority"]["admission_id"],
        fixture.credential.admission_id
    );
    assert_eq!(
        details["runtime_authority"]["execution_id"],
        fixture.credential.execution_id
    );
    assert_eq!(
        details["runtime_authority"]["sponsor_identity_id"],
        fixture.credential.sponsor_identity_id
    );
    assert!(!details.to_string().contains("recorded-live"));
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        fixture
            .ctx
            .post(
                &format!("/api/v1/cache/namespaces/published/generations/{generation}/abandon"),
                json!({"owner_type":"pack","owner_ref":"recorded_emitter"}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

async fn grant_cache_setup(fixture: &Fixture) -> Result<()> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let writer = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.cache_setup".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"caches","actions":["read","create","update","delete"]}]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: writer.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(fixture.requester)
        .await;
    Ok(())
}

async fn publish_cache_snapshot(fixture: &Fixture, namespace: &str, token: &str) -> Result<Id> {
    let generations = format!("/api/v1/cache/namespaces/{namespace}/generations");
    let response = fixture
        .ctx
        .post(
            &generations,
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter", "client_refresh_id":"seed",
                "expected_active_generation_id":null,"expected_chunk_count":1
            }),
            Some(token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let generation = body["data"]["generation_id"].as_i64().unwrap();
    let response = fixture
        .ctx
        .put(
            &format!("{generations}/{generation}/chunks/0"),
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter", "entries":[
                    {"external_id":"a","value":"first"}, {"external_id":"b","value":"second"}]
            }),
            Some(token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    for (operation, fields) in [
        ("seal", json!({"expected_chunk_count":1})),
        ("promote", json!({"expected_active_generation_id":null})),
    ] {
        let mut request = json!({"owner_type":"pack","owner_ref":"recorded_emitter"});
        request
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let response = fixture
            .ctx
            .post(
                &format!("{generations}/{generation}/{operation}"),
                request,
                Some(token),
            )
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    Ok(generation)
}

#[tokio::test]
async fn recorded_cache_clone_requires_read_as_well_as_refresh_rights() -> Result<()> {
    use attune_common::repositories::cache::{
        CacheGenerationRepository, CacheNamespaceRepository, CacheOwnerScope,
    };
    let fixture = fixture_with_capabilities(false, false, &["read"], &["create", "update"]).await?;
    grant_cache_setup(&fixture).await?;
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/cache/namespaces",
                json!({"owner_type":"pack", "owner_ref":"recorded_emitter",
        "namespace":"published"}),
                fixture.ctx.token()
            )
            .await?
            .status(),
        StatusCode::CREATED
    );
    let base = publish_cache_snapshot(&fixture, "published", fixture.ctx.token().unwrap()).await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let response = fixture.ctx.post("/api/v1/cache/namespaces/published/generations", json!({"owner_type":"pack",
        "owner_ref":"recorded_emitter", "client_refresh_id":"clone-denied", "expected_active_generation_id":base,
        "source_generation_id":base, "expected_chunk_count":0}), Some(&fixture.token)).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let scope = CacheOwnerScope::pack(fixture.pack_id, Some("recorded_emitter".into()));
    let namespace = CacheNamespaceRepository::resolve(&fixture.ctx.pool, &scope, "published")
        .await?
        .unwrap();
    assert_eq!(
        CacheGenerationRepository::list_for_namespace(&fixture.ctx.pool, namespace.id, 10)
            .await?
            .len(),
        1
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_standard_cache_reads_are_owner_bound_live_and_independent_of_requester_activity(
) -> Result<()> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let fixture = fixture(true).await?;
    let writer = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.cache_setup".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"caches","actions":["read","create","update","delete"]}]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: writer.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(fixture.requester)
        .await;
    for owner in ["recorded_emitter", "foreign_events"] {
        let created = fixture
            .ctx
            .post(
                "/api/v1/cache/namespaces",
                json!({"owner_type":"pack", "owner_ref":owner, "namespace":"users", "mode":"live"}),
                fixture.ctx.token(),
            )
            .await?;
        let status = created.status();
        let body: Value = created.json().await?;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let applied = fixture.ctx.post("/api/v1/cache/namespaces/users/entries/apply", json!({
            "owner_type":"pack", "owner_ref":owner, "request_id":"seed", "expected_revision":0,
            "entries":[{"external_id":"a","value":{"owner":owner}}]
        }), fixture.ctx.token()).await?;
        let status = applied.status();
        let body: Value = applied.json().await?;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let created = fixture
        .ctx
        .post(
            "/api/v1/cache/namespaces",
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter","namespace":"snapshot"
            }),
            fixture.ctx.token(),
        )
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let generations = "/api/v1/cache/namespaces/snapshot/generations";
    let response = fixture
        .ctx
        .post(
            generations,
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter", "client_refresh_id":"seed",
                "expected_active_generation_id":null,"expected_chunk_count":1
            }),
            fixture.ctx.token(),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let generation = body["data"]["generation_id"].as_i64().unwrap();
    assert_eq!(
        fixture
            .ctx
            .put(
                &format!("{generations}/{generation}/chunks/0"),
                json!({
                    "owner_type":"pack","owner_ref":"recorded_emitter", "entries":[
                        {"external_id":"a","value":"first"}, {"external_id":"b","value":"second"}]
                }),
                fixture.ctx.token()
            )
            .await?
            .status(),
        StatusCode::OK
    );
    for (operation, fields) in [
        ("seal", json!({"expected_chunk_count":1})),
        ("promote", json!({"expected_active_generation_id":null})),
    ] {
        let mut request = json!({"owner_type":"pack","owner_ref":"recorded_emitter"});
        request
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        assert_eq!(
            fixture
                .ctx
                .post(
                    &format!("{generations}/{generation}/{operation}"),
                    request,
                    fixture.ctx.token()
                )
                .await?
                .status(),
            StatusCode::OK
        );
    }
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let local = "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref=recorded_emitter";
    assert_eq!(
        fixture.ctx.get(local, Some(&fixture.token)).await?.status(),
        StatusCode::OK
    );
    let response = fixture
        .ctx
        .post(
            "/api/v1/cache/namespaces/users/entries/lookup",
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter","external_id":"a"
            }),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["revision"], 1);
    assert_eq!(body["data"]["item"]["value"]["owner"], "recorded_emitter");
    let response = fixture
        .ctx
        .post(
            "/api/v1/cache/namespaces/users/entries/lookup-many",
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter","external_ids":["a","absent"]
            }),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(body["data"]["missing_external_ids"], json!(["absent"]));
    assert_eq!(
        fixture
            .ctx
            .get(
                "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref=foreign_events",
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = fixture
        .ctx
        .get("/api/v1/cache/namespaces?limit=1", Some(&fixture.token))
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["namespaces"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        body["data"]["namespaces"][0]["owner_ref"],
        "recorded_emitter"
    );
    let first_namespace = body["data"]["namespaces"][0]["namespace"].clone();
    let metadata_cursor = body["data"]["next_cursor"].as_str().unwrap();
    let metadata_continuation =
        format!("/api/v1/cache/namespaces?limit=1&cursor={metadata_cursor}");
    let response = fixture
        .ctx
        .get(&metadata_continuation, Some(&fixture.token))
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["namespaces"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        body["data"]["namespaces"][0]["owner_ref"],
        "recorded_emitter"
    );
    assert_ne!(body["data"]["namespaces"][0]["namespace"], first_namespace);
    assert!(body["data"]["next_cursor"].is_null());
    let response = fixture
        .ctx
        .get(
            "/api/v1/cache/namespaces?owner_type=pack&owner_ref=foreign_events",
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["namespaces"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/cache/namespaces/users/entries/apply",
                json!({"owner_type":"pack", "owner_ref":"recorded_emitter",
                    "request_id":"denied", "expected_revision":1, "entries":[{"external_id":"x","value":false}]}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    for path in [
        format!("{generations}?owner_type=pack&owner_ref=recorded_emitter"),
        format!("{generations}/{generation}?owner_type=pack&owner_ref=recorded_emitter"),
    ] {
        assert_eq!(
            fixture.ctx.get(&path, Some(&fixture.token)).await?.status(),
            StatusCode::OK
        );
    }
    let scan = "/api/v1/cache/namespaces/snapshot/entries?owner_type=pack&owner_ref=recorded_emitter&limit=1";
    let response = fixture.ctx.get(scan, Some(&fixture.token)).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["items"][0]["external_id"], "a");
    let cursor = body["data"]["next_cursor"].as_str().unwrap();
    let continuation = format!("{scan}&generation={generation}&cursor={cursor}");
    let response = fixture.ctx.get(&continuation, Some(&fixture.token)).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["items"][0]["external_id"], "b");
    let mut transaction = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_policy(&mut transaction, fixture.policy_id).await?;
    transaction.commit().await?;
    assert_eq!(
        fixture
            .ctx
            .get(&metadata_continuation, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .ctx
            .get(&continuation, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        fixture.ctx.get(local, Some(&fixture.token)).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_named_selection_without_cache_read_cannot_use_local_namespaces() -> Result<()> {
    use attune_common::repositories::cache::{
        CacheNamespacePolicy, CacheNamespaceRepository, CacheOwnerScope, CreateCacheNamespaceInput,
    };
    let fixture = fixture(false).await?;
    CacheNamespaceRepository::create(
        &fixture.ctx.pool,
        CreateCacheNamespaceInput {
            mode: Default::default(),
            owner: CacheOwnerScope::pack(fixture.pack_id, Some("recorded_emitter".into())),
            namespace: "users".into(),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await?;
    assert_eq!(
        fixture
            .ctx
            .get(
                "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref=recorded_emitter",
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_credentials_can_read_public_schema_metadata_but_terminal_tokens_cannot(
) -> Result<()> {
    let fixture = fixture(true).await?;
    for path in [
        "/api/v1/schema-types",
        "/api/v1/packs/recorded_emitter/schema-types",
    ] {
        assert_eq!(
            fixture.ctx.get(path, Some(&fixture.token)).await?.status(),
            StatusCode::OK
        );
    }
    let request = json!({"context":"flat_document", "schema":{"message":{"type":"string"}}});
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/schema-types/preview",
                request.clone(),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::OK
    );
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        fixture
            .ctx
            .get("/api/v1/schema-types", Some(&fixture.token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/schema-types/preview",
                request,
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_key_creation_checks_real_ownership_before_uniqueness_and_survives_requester_freeze(
) -> Result<()> {
    let fixture = fixture_with_key_actions(false, false, &["create"]).await?;
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        FindByRef,
    };
    let action = ActionRepository::find_by_ref(&fixture.ctx.pool, "recorded_emitter.emit")
        .await?
        .unwrap();
    ActionRepository::update(
        &fixture.ctx.pool,
        action.id,
        UpdateActionInput {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    for (owner_type, owner_field, owner_ref) in [
        ("pack", "owner_pack_ref", "recorded_emitter"),
        ("action", "owner_action_ref", "recorded_emitter.emit"),
    ] {
        let mut request = json!({"local_ref":"created", "owner_type":owner_type,
            "name":"Created by admitted runtime", "encrypted":true, "value":"caller supplied"});
        request[owner_field] = json!(owner_ref);
        let response = fixture
            .ctx
            .post("/api/v1/keys", request.clone(), Some(&fixture.token))
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["data"]["value"], "caller supplied");
        assert_eq!(
            fixture
                .ctx
                .post("/api/v1/keys", request, Some(&fixture.token))
                .await?
                .status(),
            StatusCode::CONFLICT
        );
    }
    for owner in ["foreign_events", "missing_owner"] {
        assert_eq!(
            fixture
                .ctx
                .post(
                    "/api/v1/keys",
                    json!({"local_ref":"created","owner_type":"pack", "owner_pack_ref":owner,
            "name":"Denied", "value":"not written"}),
                    Some(&fixture.token)
                )
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/keys",
                json!({"local_ref":"created","owner_type":"system",
        "name":"Denied", "value":"not written"}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(fixture.ctx.post("/api/v1/keys", json!({"local_ref":"after_completion","owner_type":"pack", "owner_pack_ref":"recorded_emitter",
        "name":"Denied", "value":"not written"}), Some(&fixture.token)).await?.status(), StatusCode::UNAUTHORIZED);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_key_mutations_require_selected_local_rights_and_preserve_material_disclosure_checks(
) -> Result<()> {
    use attune_common::{
        models::OwnerType,
        repositories::key::{CreateKeyInput, KeyRepository},
    };
    let fixture = fixture_with_key_actions(false, false, &["update", "delete"]).await?;
    let encryption = fixture
        .ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .unwrap();
    let input = |local: &str, owner_type| CreateKeyInput {
        local_ref: local.into(),
        owner_type,
        owner_identity: None,
        owner_pack: (owner_type == OwnerType::Pack).then_some(fixture.pack_id),
        owner_pack_ref: (owner_type == OwnerType::Pack).then(|| "recorded_emitter".into()),
        owner_action: None,
        owner_action_ref: None,
        owner_sensor: None,
        owner_sensor_ref: None,
        name: "Mutation fixture".into(),
        encrypted: true,
        encryption_key_hash: None,
        value: json!("protected original"),
    };
    let local = KeyRepository::create_encrypted_in_pool(
        &fixture.ctx.pool,
        input("mutable", OwnerType::Pack),
        encryption,
    )
    .await?;
    let foreign = KeyRepository::create_encrypted_in_pool(
        &fixture.ctx.pool,
        input("not_mutable", OwnerType::System),
        encryption,
    )
    .await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let path = format!("/api/v1/keys/{}", local.r#ref);
    let foreign_path = format!("/api/v1/keys/{}", foreign.r#ref);
    let response = fixture
        .ctx
        .put(&path, json!({"name":"New label"}), Some(&fixture.token))
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["value"].is_null(), "{body}");
    assert!(!body.to_string().contains("protected original"));
    assert_eq!(
        fixture
            .ctx
            .put(&path, json!({"encrypted":false}), Some(&fixture.token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .ctx
            .put(
                &foreign_path,
                json!({"name":"Forbidden"}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .ctx
            .delete(&foreign_path, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .ctx
            .get("/api/v1/keys", Some(&fixture.token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = fixture
        .ctx
        .put(
            &path,
            json!({"value":"known replacement"}),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["value"].is_null());
    assert_eq!(
        fixture
            .ctx
            .delete(&path, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::OK
    );
    assert!(KeyRepository::find_by_ref(&fixture.ctx.pool, &local.r#ref)
        .await?
        .is_none());
    assert!(
        KeyRepository::find_by_ref(&fixture.ctx.pool, &foreign.r#ref)
            .await?
            .is_some()
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn standard_key_access_uses_pinned_owner_ids_and_never_follows_reused_action_refs(
) -> Result<()> {
    use attune_common::{
        models::OwnerType,
        repositories::{
            action::ActionRepository,
            key::{CreateKeyInput, KeyRepository},
            FindByRef,
        },
    };
    for workflow_context in [false, true] {
        let fixture = fixture_with_workflow(true, workflow_context).await?;
        let source_ref = if workflow_context {
            "recorded_emitter.workflow"
        } else {
            "recorded_emitter.emit"
        };
        let action = ActionRepository::find_by_ref(&fixture.ctx.pool, source_ref)
            .await?
            .unwrap();
        let sibling = create_test_action(
            &fixture.ctx.pool,
            fixture.pack_id,
            "recorded_emitter",
            "recorded_emitter.sibling",
        )
        .await?;
        let input = |local: &str, owner_type, action: Option<&attune_common::models::Action>| {
            CreateKeyInput {
                local_ref: local.into(),
                owner_type,
                owner_identity: None,
                owner_pack: (owner_type == OwnerType::Pack).then_some(fixture.pack_id),
                owner_pack_ref: (owner_type == OwnerType::Pack).then(|| "recorded_emitter".into()),
                owner_action: action.map(|action| action.id),
                owner_action_ref: action.map(|action| action.r#ref.clone()),
                owner_sensor: None,
                owner_sensor_ref: None,
                name: "Standard scope fixture".into(),
                encrypted: false,
                encryption_key_hash: None,
                value: json!("owned value"),
            }
        };
        let pack_key = KeyRepository::create(
            &fixture.ctx.pool,
            input("pack_input", OwnerType::Pack, None),
        )
        .await?;
        let action_key = KeyRepository::create(
            &fixture.ctx.pool,
            input("action_input", OwnerType::Action, Some(&action)),
        )
        .await?;
        let sibling_key = KeyRepository::create(
            &fixture.ctx.pool,
            input("sibling_input", OwnerType::Action, Some(&sibling)),
        )
        .await?;
        for (key, expected) in [
            (&pack_key, StatusCode::OK),
            (&action_key, StatusCode::OK),
            (&sibling_key, StatusCode::NOT_FOUND),
        ] {
            assert_eq!(
                fixture
                    .ctx
                    .get(&format!("/api/v1/keys/{}", key.r#ref), Some(&fixture.token))
                    .await?
                    .status(),
                expected
            );
        }
        let mut listed = Vec::new();
        for page in 1..=2 {
            let response = fixture
                .ctx
                .get(
                    &format!("/api/v1/keys?per_page=1&page={page}"),
                    Some(&fixture.token),
                )
                .await?;
            let status = response.status();
            let body: Value = response.json().await?;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["pagination"]["total_items"], 2, "{body}");
            let items = body["items"].as_array().unwrap();
            assert_eq!(items.len(), 1);
            assert!(items[0].get("value").is_none());
            listed.push(items[0]["ref"].as_str().unwrap().to_string());
        }
        listed.sort();
        let mut expected = vec![pack_key.r#ref.clone(), action_key.r#ref.clone()];
        expected.sort();
        assert_eq!(listed, expected);
        KeyRepository::delete(&fixture.ctx.pool, action_key.id).await?;
        ActionRepository::delete(&fixture.ctx.pool, action.id).await?;
        let replacement = create_test_action(
            &fixture.ctx.pool,
            fixture.pack_id,
            "recorded_emitter",
            source_ref,
        )
        .await?;
        assert_ne!(replacement.id, action.id);
        let rebound = KeyRepository::create(
            &fixture.ctx.pool,
            input("action_input", OwnerType::Action, Some(&replacement)),
        )
        .await?;
        assert_eq!(rebound.r#ref, action_key.r#ref);
        assert_eq!(
            fixture
                .ctx
                .get(
                    &format!("/api/v1/keys/{}", rebound.r#ref),
                    Some(&fixture.token)
                )
                .await?
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            fixture
                .ctx
                .get(
                    &format!("/api/v1/keys/{}", pack_key.r#ref),
                    Some(&fixture.token)
                )
                .await?
                .status(),
            StatusCode::OK
        );
        let listed: Value = fixture
            .ctx
            .get("/api/v1/keys", Some(&fixture.token))
            .await?
            .json()
            .await?;
        assert_eq!(listed["pagination"]["total_items"], 1, "{listed}");
        assert_eq!(listed["items"][0]["ref"], pack_key.r#ref);
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn recorded_signing_uses_external_approval_and_operator_profile_without_plaintext_key_access(
) -> Result<()> {
    let fixture = fixture(false).await?;
    let key = attune_common::repositories::key::KeyRepository::find_by_ref(
        &fixture.ctx.pool,
        "system.recorded_signer",
    )
    .await?
    .unwrap();
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let endpoint = format!("/api/v1/keys/{}/sign-jwt", key.r#ref);
    let request = json!({"profile_ref": "test", "subject": "approved-subject", "ttl_seconds": 60});
    let response = fixture
        .ctx
        .post(&endpoint, request.clone(), Some(&fixture.token))
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["assertion"]
        .as_str()
        .is_some_and(|token| token.split('.').count() == 3));
    assert!(!body.to_string().contains("private_key_pem"));
    assert_eq!(
        fixture
            .ctx
            .get(&format!("/api/v1/keys/{}", key.r#ref), Some(&fixture.token))
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .ctx
            .post(
                &endpoint,
                json!({"profile_ref":"test","subject":"forbidden-subject","ttl_seconds":60}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut transaction = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_approval(&mut transaction, fixture.signing_approval_id)
        .await?;
    transaction.commit().await?;
    assert_eq!(
        fixture
            .ctx
            .post(&endpoint, request.clone(), Some(&fixture.token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        fixture
            .ctx
            .post(&endpoint, request, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_key_reads_are_scoped_idempotently_tracked_and_forwarded_into_event_provenance(
) -> Result<()> {
    use attune_common::{
        models::OwnerType,
        repositories::{
            execution_key_read::ExecutionKeyReadRepository,
            key::{CreateKeyInput, KeyRepository},
        },
    };
    let fixture = fixture(false).await?;
    let key = KeyRepository::create(
        &fixture.ctx.pool,
        CreateKeyInput {
            local_ref: "runtime_input".into(),
            owner_type: OwnerType::Pack,
            owner_identity: None,
            owner_pack: Some(fixture.pack_id),
            owner_pack_ref: Some("recorded_emitter".into()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Runtime input".into(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!("key value"),
        },
    )
    .await?;
    let system_key = KeyRepository::create(
        &fixture.ctx.pool,
        CreateKeyInput {
            local_ref: "not_local".into(),
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "System input".into(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!("not delivered"),
        },
    )
    .await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    for _ in 0..2 {
        let response = fixture
            .ctx
            .get(&format!("/api/v1/keys/{}", key.r#ref), Some(&fixture.token))
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["value"], "key value");
    }
    assert_eq!(
        fixture
            .ctx
            .get(
                &format!("/api/v1/keys/{}", system_key.r#ref),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    let mut connection = fixture.ctx.pool.acquire().await?;
    let origins =
        ExecutionKeyReadRepository::origins(&mut connection, fixture.credential.admission_id)
            .await?;
    assert_eq!(origins.len(), 1);
    assert_eq!(origins[0].key_id, key.id);
    drop(connection);
    let listed: Value = fixture
        .ctx
        .get("/api/v1/keys", Some(&fixture.token))
        .await?
        .json()
        .await?;
    assert_eq!(listed["pagination"]["total_items"], 1, "{listed}");
    assert_eq!(listed["items"][0]["ref"], key.r#ref);
    assert!(!listed.to_string().contains("key value"));
    // No schema/input secret is needed to classify an SDK-capable log.
    let mut connection = fixture.ctx.pool.acquire().await?;
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut connection,
        ENTITY_EXECUTION_CONFIG,
        fixture.credential.execution_id,
        &[],
    )
    .await?;
    drop(connection);
    use attune_common::repositories::{
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        FindById,
    };
    let viewer = IdentityRepository::create(
        &fixture.ctx.pool,
        CreateIdentityInput {
            login: "sdk_log_viewer".into(),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await?;
    let set = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.sdk_log_viewer".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([
                {"resource":"executions","actions":["read","decrypt"]},
                {"resource":"artifacts","actions":["read"]}
            ]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: viewer.id,
            permset: set.id,
        },
    )
    .await?;
    let viewer_token = attune_common::auth::jwt::generate_access_token(
        viewer.id,
        &viewer.login,
        &fixture.ctx.state.jwt_config,
    )?;
    let viewer = attune_api::auth::middleware::AuthenticatedUser {
        claims: attune_common::auth::jwt::validate_token(
            &viewer_token,
            &fixture.ctx.state.jwt_config,
        )?,
    };
    let execution =
        ExecutionRepository::find_by_id(&fixture.ctx.pool, fixture.credential.execution_id)
            .await?
            .unwrap();
    use attune_common::{
        models::{ArtifactClassification, ArtifactType, ArtifactVisibility, RetentionPolicyType},
        repositories::artifact::{
            ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
            CreateArtifactVersionInput,
        },
    };
    let log = ArtifactRepository::create(
        &fixture.ctx.pool,
        CreateArtifactInput {
            r#ref: "recorded_emitter.emit.stdout.log".into(),
            scope: OwnerType::Action,
            owner: "recorded_emitter.emit".into(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::RuntimeLog,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 10,
            name: None,
            description: None,
            content_type: Some("text/plain".into()),
            data: None,
        },
    )
    .await?;
    ArtifactVersionRepository::create(
        &fixture.ctx.pool,
        CreateArtifactVersionInput {
            artifact: log.id,
            execution: Some(execution.id),
            content_type: Some("text/plain".into()),
            content: None,
            content_json: None,
            file_path: None,
            meta: None,
            created_by: None,
        },
    )
    .await?;
    let artifact_path = format!("/api/v1/executions/{}/artifacts", execution.id);
    let hidden: Value = fixture
        .ctx
        .get(&artifact_path, Some(&viewer_token))
        .await?
        .json()
        .await?;
    assert_eq!(
        hidden["items"].as_array().map(Vec::len),
        Some(0),
        "{hidden}"
    );
    assert!(attune_api::secret_disclosure::authorize_runtime_log(
        &fixture.ctx.state,
        &viewer,
        &execution
    )
    .await
    .is_err());
    let key_set = PermissionSetRepository::create(&fixture.ctx.pool, CreatePermissionSetInput {
        r#ref: "fixture.sdk_log_key".into(), pack: None, pack_ref: None, label: None, description: None,
        grants: json!([{"resource":"keys","actions":["read"],"constraints":{"refs":[key.r#ref]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: viewer.identity_id()?,
            permset: key_set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(viewer.identity_id()?)
        .await;
    attune_api::secret_disclosure::authorize_runtime_log(&fixture.ctx.state, &viewer, &execution)
        .await?;
    let visible: Value = fixture
        .ctx
        .get(&artifact_path, Some(&viewer_token))
        .await?
        .json()
        .await?;
    assert_eq!(
        visible["items"].as_array().map(Vec::len),
        Some(1),
        "{visible}"
    );
    let response = fixture
        .ctx
        .post(
            "/api/v1/events",
            json!({"trigger_ref": fixture.trigger_ref,
        "payload": {"secret": "key value"}}),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["data"]["id"].as_i64().unwrap();
    let mut connection = fixture.ctx.pool.acquire().await?;
    let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *connection,
        ENTITY_EVENT_PAYLOAD,
        id,
    )
    .await?;
    let provenance: attune_common::secret_provenance::SecretProvenance =
        serde_json::from_str(secrets[0].source_ref.as_deref().unwrap())?;
    assert!(provenance.origins.iter().any(|origin| matches!(origin, attune_common::secret_provenance::SecretOrigin::Key(origin) if origin.key_id == key.id)));
    drop(connection);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn execution_options_use_admitted_action_choices_without_personal_membership() -> Result<()> {
    let fixture = fixture(false).await?;
    let mut transaction = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::activate_policy(&mut transaction, fixture.policy_id).await?;
    // This preview fixture needs a real captured metadata revision, not the
    // deliberately synthetic metadata-less policy used by recorded-event tests.
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        managed_executable::ManagedExecutableRepository,
        FindByRef, Update,
    };
    let action = ActionRepository::find_by_ref(&mut *transaction, "recorded_emitter.emit")
        .await?
        .unwrap();
    ActionRepository::update(
        &mut *transaction,
        action.id,
        UpdateActionInput {
            default_execution_permission_set_refs: Some(vec![
                "recorded_emitter.events".into(),
                "standard".into(),
            ]),
            ..Default::default()
        },
    )
    .await?;
    let admitted =
        ManagedExecutableRepository::readmit_action(&mut transaction, action.id, fixture.requester)
            .await?
            .unwrap();
    transaction.commit().await?;
    let invoker = event_inspector(&fixture.ctx, "options_invoker", json!([
        {"resource":"actions","actions":["execute"],"constraints":{"refs":["recorded_emitter.emit"]}}
    ])).await?;
    let denied = event_inspector(&fixture.ctx, "options_denied", json!([])).await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let path = "/api/v1/actions/recorded_emitter.emit/execution-options";
    let response = fixture.ctx.get(path, Some(&invoker)).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["authority_source"], "pack");
    assert_eq!(body["data"]["policy_revision_id"], admitted.id);
    assert_eq!(
        body["data"]["default_permission_set_refs"],
        json!(["recorded_emitter.events", "standard"])
    );
    assert_eq!(
        body["data"]["selectable_permission_set_refs"],
        json!(["recorded_emitter.events", "standard"])
    );
    assert_eq!(body["data"]["default_selection_available"], true);
    assert!(body["data"].get("grants").is_none());
    assert_eq!(
        fixture.ctx.get(path, Some(&denied)).await?.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture.ctx.get(path, Some(&fixture.token)).await?.status(),
        StatusCode::FORBIDDEN
    );
    let mut transaction = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_approval(&mut transaction, fixture.signing_approval_id)
        .await?;
    transaction.commit().await?;
    let response = fixture.ctx.get(path, Some(&invoker)).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["selectable_permission_set_refs"],
        json!(["standard"])
    );
    assert_eq!(body["data"]["default_selection_available"], false);
    assert_eq!(
        body["data"]["default_permission_set_refs"],
        json!(["recorded_emitter.events", "standard"])
    );
    let mut transaction = fixture.ctx.pool.begin().await?;
    assert!(RuntimeAuthorityRepository::validate_execution_credential(
        &mut transaction,
        &fixture.credential,
    )
    .await
    .is_err());
    transaction.rollback().await?;
    let mut transaction = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_policy(&mut transaction, admitted.id).await?;
    transaction.commit().await?;
    assert_eq!(
        fixture.ctx.get(path, Some(&invoker)).await?.status(),
        StatusCode::FORBIDDEN
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn event_emitter_inspection_requires_execution_visibility_and_survives_retention(
) -> Result<()> {
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        FindByRef,
    };
    let fixture = fixture(false).await?;
    let action = ActionRepository::find_by_ref(&fixture.ctx.pool, "recorded_emitter.emit")
        .await?
        .unwrap();
    ActionRepository::update(
        &fixture.ctx.pool,
        action.id,
        UpdateActionInput {
            reference_visibility: Some(attune_common::models::ActionReferenceVisibility::Private),
            ..Default::default()
        },
    )
    .await?;
    let event_reader = event_inspector(&fixture.ctx, "event_only", json!([])).await?;
    let scoped_reader = event_inspector(&fixture.ctx, "execution_scoped", json!([
        {"resource":"executions","actions":["read"],"constraints":{"pack_refs":["recorded_emitter"]}}
    ])).await?;
    let wrong_reader = event_inspector(&fixture.ctx, "execution_foreign", json!([
        {"resource":"executions","actions":["read"],"constraints":{"pack_refs":["foreign_events"]}}
    ])).await?;
    let global_reader = event_inspector(
        &fixture.ctx,
        "execution_global",
        json!([
            {"resource":"executions","actions":["read"]}
        ]),
    )
    .await?;
    let response = fixture
        .ctx
        .post(
            "/api/v1/events",
            json!({"trigger_ref":fixture.trigger_ref,
        "payload":{"secret":"inspection-must-not-reveal"}}),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(body["data"].get("execution_emitter").is_none());
    let path = format!("/api/v1/events/{}", body["data"]["id"].as_i64().unwrap());
    for (token, visible) in [
        (&event_reader, false),
        (&wrong_reader, false),
        (&scoped_reader, true),
        (&global_reader, true),
    ] {
        let response = fixture.ctx.get(&path, Some(token)).await?;
        let status = response.status();
        let body: Value = response.json().await?;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["data"].get("execution_emitter").is_some(),
            visible,
            "{body}"
        );
        assert!(!body.to_string().contains("inspection-must-not-reveal"));
        if visible {
            let emitter = &body["data"]["execution_emitter"];
            assert_eq!(emitter["execution_id"], fixture.credential.execution_id);
            assert_eq!(emitter["admission_id"], fixture.credential.admission_id);
            assert_eq!(emitter["action_ref"], "recorded_emitter.emit");
            assert!(emitter.get("requester").is_none());
            assert!(body["data"]["source"].is_null());
            assert!(body["data"]["source_ref"].is_null());
        }
    }
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    ExecutionRepository::delete(&fixture.ctx.pool, fixture.credential.execution_id).await?;
    for (token, visible) in [
        (&event_reader, false),
        (&scoped_reader, false),
        (&global_reader, true),
    ] {
        let response = fixture.ctx.get(&path, Some(token)).await?;
        let status = response.status();
        let body: Value = response.json().await?;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["data"].get("execution_emitter").is_some(),
            visible,
            "{body}"
        );
    }
    fixture.ctx.cleanup().await?;
    Ok(())
}

async fn event_inspector(
    ctx: &TestContext,
    login: &str,
    execution_grants: Value,
) -> Result<String> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let response = ctx
        .post(
            "/auth/register",
            json!({"login":login,"password":"TestPassword123!"}),
            None,
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert!(
        matches!(status, StatusCode::OK | StatusCode::CREATED),
        "Inspector registration returned {status}"
    );
    let token = body["data"]["access_token"].as_str().unwrap().to_owned();
    let identity = IdentityRepository::find_by_login(&ctx.pool, login)
        .await?
        .unwrap();
    let mut grants = vec![json!({"resource":"events","actions":["read"]})];
    grants.extend(execution_grants.as_array().unwrap().clone());
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("fixture.{login}"),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: Value::Array(grants),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    Ok(token)
}

#[tokio::test]
async fn scoped_event_emission_survives_requester_freeze_and_records_emitter_and_input_origins(
) -> Result<()> {
    let fixture = fixture(false).await?;
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
        .post(
            "/api/v1/events",
            json!({"trigger_ref": fixture.trigger_ref,
        "payload": {"secret": "forwarded secret"}}),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(!body.to_string().contains("forwarded secret"));
    let event_id = body["data"]["id"].as_i64().unwrap();
    let created = serde_json::from_value(body["data"]["created"].clone())?;
    let mut connection = fixture.ctx.pool.acquire().await?;
    let emitter = ExecutionEventEmitterRepository::find(&mut connection, event_id, created)
        .await?
        .unwrap();
    assert_eq!(emitter.execution_id, fixture.credential.execution_id);
    assert_eq!(
        emitter.sponsor_identity_id,
        fixture.credential.sponsor_identity_id
    );
    assert_ne!(emitter.sponsor_identity_id, fixture.requester);
    assert_eq!(emitter.attempt, fixture.credential.attempt);
    let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *connection,
        ENTITY_EVENT_PAYLOAD,
        event_id,
    )
    .await?;
    assert_eq!(secrets.len(), 1);
    let provenance: attune_common::secret_provenance::SecretProvenance =
        serde_json::from_str(secrets[0].source_ref.as_deref().unwrap())?;
    assert!(provenance.origins.iter().any(|origin| matches!(origin,
        attune_common::secret_provenance::SecretOrigin::Entity { entity_type, entity_id, path }
        if entity_type == ENTITY_EXECUTION_CONFIG && *entity_id == fixture.credential.execution_id && path == "/input")));
    drop(connection);
    // Attribution survives execution retention without adding an execution FK.
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    ExecutionRepository::delete(&fixture.ctx.pool, fixture.credential.execution_id).await?;
    let mut connection = fixture.ctx.pool.acquire().await?;
    assert_eq!(
        ExecutionEventEmitterRepository::find(&mut connection, event_id, created).await?,
        Some(emitter)
    );
    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    assert_eq!(
        ExecutionEventEmitterRepository::cleanup_orphans(&mut connection, 100, cutoff).await?,
        0
    );
    drop(connection);
    attune_common::repositories::event::EventRepository::delete(&fixture.ctx.pool, event_id)
        .await?;
    let mut connection = fixture.ctx.pool.acquire().await?;
    attune_common::repositories::event_schema::EventSchemaRepository::cleanup_orphans(
        &mut connection,
        100,
        cutoff,
    )
    .await?;
    assert!(
        ExecutionEventEmitterRepository::find(&mut connection, event_id, created)
            .await?
            .is_none()
    );
    drop(connection);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn standard_or_foreign_scope_cannot_emit_and_unconverted_consumers_fail_closed() -> Result<()>
{
    let standard = fixture(true).await?;
    let response = standard
        .ctx
        .post(
            "/api/v1/events",
            json!({"trigger_ref": standard.trigger_ref,
        "payload": {"secret": "value"}}),
            Some(&standard.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    standard.ctx.cleanup().await?;
    let scoped = fixture(false).await?;
    let response = scoped
        .ctx
        .post(
            "/api/v1/events",
            json!({"trigger_ref": scoped.foreign_trigger_ref,
        "payload": {"secret": "value"}}),
            Some(&scoped.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        scoped
            .ctx
            .get("/api/v1/packs", Some(&scoped.token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    scoped.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn terminal_or_revoked_recorded_credentials_are_rejected_before_effects() -> Result<()> {
    let fixture = fixture(false).await?;
    let selected = vec!["recorded_emitter.events".to_string()];
    let mismatched_selection = generate_admitted_execution_token(
        &fixture.credential,
        &["standard".into()],
        &fixture.ctx.state.jwt_config,
        Some(360),
    )?;
    let mut wrong_attempt = fixture.credential.clone();
    wrong_attempt.attempt = uuid::Uuid::new_v4();
    let mismatched_attempt = generate_admitted_execution_token(
        &wrong_attempt,
        &selected,
        &fixture.ctx.state.jwt_config,
        Some(360),
    )?;
    for token in [mismatched_selection, mismatched_attempt] {
        assert_eq!(
            fixture
                .ctx
                .post(
                    "/api/v1/events",
                    json!({"trigger_ref": fixture.trigger_ref,
            "payload": {"secret": "value"}}),
                    Some(&token)
                )
                .await?
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/events",
                json!({"trigger_ref": fixture.trigger_ref,
        "payload": {"secret": "value"}}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    let fixture = self::fixture(false).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_policy(&mut tx, fixture.policy_id).await?;
    tx.commit().await?;
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/events",
                json!({"trigger_ref": fixture.trigger_ref,
        "payload": {"secret": "value"}}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}
