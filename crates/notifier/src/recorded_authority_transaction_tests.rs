//! Owned database checks for the notifier's validation-only transaction scopes.

use super::*;
use anyhow::{ensure, Context, Result};
use attune_common::{
    config::Config,
    models::{ActionReferenceVisibility, ExecutionStatus},
    repositories::{
        action::{ActionRepository, CreateActionInput},
        component_lifecycle::PackProjectionIds,
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository},
        identity::{CreateIdentityInput, IdentityRepository},
        pack::{CreatePackInput, PackRepository},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        rule_authority::RuleAuthorityRepository,
        runtime_authority::RuntimeOwner,
        schema_type::SchemaTypeRepository,
        Create, FindById,
    },
    runtime_authority::{
        HumanAttribution, InvocationBinding, InvocationPolicy, PermissionSetDefinition,
        RequestAttribution, RuntimePolicy,
    },
    test_database::TestDatabase,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use std::{collections::BTreeMap, sync::Arc, time::Duration};

const DEADLINE: Duration = Duration::from_secs(15);
const APPLICATION_NAME: &str = "notifier-authority-validation-test";

struct Fixture {
    db: TestDatabase,
    pool: PgPool,
    recorded: RecordedConnection,
    notification: Notification,
    backend: i32,
}

impl Fixture {
    async fn new() -> Result<Self> {
        let config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))?;
        let db = TestDatabase::create(&config.database)
            .await?
            .with_cleanup_on_drop();
        let pack = PackRepository::create(
            db.pool(),
            CreatePackInput {
                r#ref: "notifier_authority".into(),
                label: "Notifier authority".into(),
                description: None,
                version: "1.0.0".into(),
                conf_schema: json!({}),
                config: json!({}),
                meta: json!({}),
                tags: vec![],
                runtime_deps: vec![],
                dependencies: vec![],
                is_standard: false,
                installers: json!({}),
            },
        )
        .await?;
        let action = ActionRepository::create(
            db.pool(),
            CreateActionInput {
                r#ref: format!("{}.run", pack.r#ref),
                pack: pack.id,
                pack_ref: pack.r#ref.clone(),
                label: "Run".into(),
                description: None,
                entrypoint: "run.py".into(),
                runtime: None,
                enabled: true,
                runtime_version_constraint: None,
                required_worker_runtimes: json!({}),
                worker_selector: json!({}),
                worker_tolerations: json!([]),
                worker_affinity: json!({}),
                param_schema: None,
                out_schema: None,
                is_adhoc: false,
                accesses_mcp: false,
                default_execution_permission_set_refs: vec![],
                reference_visibility: ActionReferenceVisibility::Public,
                reference_allowed_pack_refs: vec![],
                log_retention_policy: None,
                log_retention_limit: None,
                artifact_retention_policy: None,
                artifact_retention_limit: None,
                timeout_seconds: None,
            },
        )
        .await?;
        let human = IdentityRepository::create(
            db.pool(),
            CreateIdentityInput {
                login: "notifier_requester".into(),
                display_name: None,
                password_hash: None,
                attributes: json!({}),
            },
        )
        .await?;
        let mut tx = db.pool().begin().await?;
        let release = PackReleaseRepository::create_or_get(
            &mut tx,
            CreatePackReleaseInput {
                pack: pack.id,
                pack_ref: pack.r#ref.clone(),
                version: pack.version,
                digest: "a".repeat(64),
                object_key: "packs/blobs/notifier-authority.tar.gz".into(),
                provider_version: "e:notifier-authority".into(),
                content_path: "/packs/.releases/notifier-authority".into(),
                archive_size: 1,
                manifest: json!({}),
            },
        )
        .await?;
        PackReleaseRepository::activate_projected(
            &mut tx,
            pack.id,
            release.id,
            &PackProjectionIds {
                actions: vec![action.id],
                ..Default::default()
            },
        )
        .await?;
        RuleAuthorityRepository::capture_deployment(&mut tx, pack.id).await?;
        let owner = RuntimeAuthorityRepository::find_owner(&mut tx, pack.id)
            .await?
            .context("runtime owner")?;
        let set_ref = format!("{}.observe", pack.r#ref);
        let definition: PermissionSetDefinition = serde_json::from_value(json!({
            "ref": set_ref, "grants": [{"resource":"executions", "actions":["read"]}]
        }))?;
        let set = definition.compile(&owner, &[])?;
        let binding = InvocationBinding::Action {
            action_ref: action.r#ref.clone(),
        };
        let policy = RuntimeAuthorityRepository::capture_policy(
            &mut tx,
            &RuntimePolicy {
                owner,
                release_id: release.id,
                release_digest: release.digest.clone(),
                concrete_targets: BTreeMap::new(),
                invocations: vec![InvocationPolicy {
                    binding: binding.clone(),
                    target_release_id: release.id,
                    target_release_digest: release.digest,
                    permission_sets: BTreeMap::from([(set.reference.clone(), set)]),
                    standard_access: false,
                    metadata_revision: None,
                    source_id: None,
                    dispatch_template_digest: None,
                    foreign_workflow_policy: None,
                }],
            },
        )
        .await?;
        RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id).await?;
        let selected = policy.policy.select(
            policy.id,
            &binding,
            std::slice::from_ref(&set_ref),
            RequestAttribution::Human {
                actor: HumanAttribution {
                    identity_id: human.id,
                    login: human.login,
                },
            },
        )?;
        let snapshot =
            ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id).await?;
        let execution = ExecutionRepository::create_pinned(
            &mut *tx,
            CreateExecutionInput {
                action: Some(action.id),
                action_ref: action.r#ref.clone(),
                executor: Some(human.id),
                permission_set_refs: vec![set_ref.clone()],
                status: ExecutionStatus::Running,
                config: Some(json!({})),
                ..Default::default()
            },
            &snapshot,
        )
        .await?;
        let admission = RuntimeAuthorityRepository::record_admission(
            &mut tx,
            RuntimeOwner::Execution(execution.id),
            &selected,
        )
        .await?;
        let recorded = RecordedConnection {
            binding: RuntimeCredentialBinding {
                execution_id: execution.id,
                admission_id: admission.id,
                sponsor_identity_id: admission.authority.owner.sponsor_identity_id,
                action_ref: action.r#ref,
                attempt: admission.authority.attempt,
            },
            selected_refs: vec![set_ref],
        };
        tx.commit().await?;
        let schema = db.schema().to_owned();
        // A nested acquisition cannot hide behind spare notifier connections.
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .after_connect(move |connection, _| {
                let schema = schema.clone();
                Box::pin(async move {
                    sqlx::query("SELECT set_config('search_path', quote_ident($1), false)")
                        .bind(schema)
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(
                (*db.pool().connect_options())
                    .clone()
                    .application_name(APPLICATION_NAME),
            )
            .await?;
        let backend = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&pool)
            .await?;
        Ok(Self {
            db,
            pool,
            recorded,
            notification: Notification {
                notification_type: "execution_status_changed".into(),
                entity_type: "execution".into(),
                entity_id: execution.id,
                user_id: None,
                payload: json!({}),
                timestamp: chrono::Utc::now(),
            },
            backend,
        })
    }

    async fn released(&self) -> Result<()> {
        let held: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid \
             WHERE a.datname = current_database() AND a.application_name = $1 \
             AND l.locktype = 'advisory' AND l.granted)",
        )
        .bind(APPLICATION_NAME)
        .fetch_one(self.db.pool())
        .await?;
        ensure!(
            !held,
            "validation returned with an advisory lock still held"
        );
        let idle: bool = sqlx::query_scalar(
            "SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname = current_database() \
             AND application_name = $1 AND xact_start IS NOT NULL)",
        )
        .bind(APPLICATION_NAME)
        .fetch_one(self.db.pool())
        .await?;
        ensure!(idle, "validation backend still has an open transaction");
        Ok(())
    }

    async fn cleanup(self) -> Result<()> {
        tokio::time::timeout(DEADLINE, self.pool.close()).await?;
        let gone = tokio::time::timeout(DEADLINE, async {
            loop {
                let gone: bool = sqlx::query_scalar(
                    "SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname = current_database() \
                     AND application_name = $1)",
                )
                .bind(APPLICATION_NAME)
                .fetch_one(self.db.pool())
                .await?;
                if gone {
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        let cleanup = self.db.cleanup().await;
        gone.context("owned notifier backend survived pool shutdown")??;
        cleanup?;
        Ok(())
    }
}

async fn check(fixture: &Fixture, notification: bool) -> Option<ApprovalEffectFence> {
    if notification {
        fixture
            .recorded
            .notification_fence(&fixture.pool, &fixture.notification)
            .await
    } else {
        fixture.recorded.source_fence(&fixture.pool).await
    }
}

async fn wait_blocked(observer: &PgPool, backend: i32, blocker: i32) -> Result<()> {
    tokio::time::timeout(DEADLINE, async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT $2 = ANY(pg_blocking_pids($1))")
                .bind(backend)
                .bind(blocker)
                .fetch_one(observer)
                .await?;
            if blocked {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("expected backend did not enter its lock wait")??;
    Ok(())
}

async fn wait_backend_gone(observer: &PgPool, backend: i32) -> Result<()> {
    tokio::time::timeout(DEADLINE, async {
        loop {
            let gone: bool = sqlx::query_scalar(
                "SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid = $1 \
                 AND datname = current_database())",
            )
            .bind(backend)
            .fetch_one(observer)
            .await?;
            if gone {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("owned validation backend survived socket disposal")??;
    Ok(())
}

#[tokio::test]
async fn successful_and_denied_checks_release_validation_transactions() -> Result<()> {
    let fixture = Fixture::new().await?;
    let result = async {
        let original_timeout: String = sqlx::query_scalar("SHOW statement_timeout")
            .fetch_one(&fixture.pool)
            .await?;
        for notification in [false, true] {
            let fence = check(&fixture, notification)
                .await
                .context("valid authority denied")?;
            fixture.released().await?;
            // The websocket's later fresh-clock acquire is sequential, not reentry.
            let mut connection = fixture.pool.acquire().await?;
            fence.ensure_fresh_on(&mut connection).await?;
            let returned_timeout: String = sqlx::query_scalar("SHOW statement_timeout")
                .fetch_one(&mut *connection)
                .await?;
            ensure!(
                returned_timeout == original_timeout,
                "validation deadline leaked into the session"
            );
            drop(connection);
        }
        let mut denied = fixture.recorded.clone();
        denied.selected_refs.clear();
        ensure!(denied.source_fence(&fixture.pool).await.is_none());
        fixture.released().await?;
        ensure!(denied
            .notification_fence(&fixture.pool, &fixture.notification)
            .await
            .is_none());
        fixture.released().await?;
        denied = fixture.recorded.clone();
        denied.binding.attempt = uuid::Uuid::new_v4();
        ensure!(denied.source_fence(&fixture.pool).await.is_none());
        fixture.released().await?;
        let mut missing = fixture.notification.clone();
        missing.entity_id = i64::MAX;
        ensure!(fixture
            .recorded
            .notification_fence(&fixture.pool, &missing)
            .await
            .is_none());
        fixture.released().await
    }
    .await;
    fixture.cleanup().await?;
    result
}

#[tokio::test]
async fn sql_errors_release_source_and_notification_transactions() -> Result<()> {
    let fixture = Fixture::new().await?;
    let result = async {
        // Session-local shadow fails the admission SELECT after shared admission.
        sqlx::query("CREATE TEMP TABLE runtime_authority_admission (id BIGINT)")
            .execute(&fixture.pool)
            .await?;
        for notification in [false, true] {
            ensure!(check(&fixture, notification).await.is_none());
            fixture.released().await?;
        }
        sqlx::query("DROP TABLE pg_temp.runtime_authority_admission")
            .execute(&fixture.pool)
            .await?;
        ensure!(check(&fixture, true).await.is_some());
        fixture.released().await
    }
    .await;
    fixture.cleanup().await?;
    result
}

#[tokio::test]
async fn validation_rolls_back_incidental_writes_and_denies_rollback_failure() -> Result<()> {
    let fixture = Fixture::new().await?;
    let result = async {
        let fence = check(&fixture, false)
            .await
            .context("valid authority denied")?;
        let mut holder = ValidationHolder::new();
        let mut owner =
            ValidationConnection::acquire(&fixture.pool, TokioInstant::now() + VALIDATION_TIMEOUT)
                .await
                .context("validation connection")?;
        let mut tx = owner.0.as_mut().unwrap().begin().await?;
        holder.select(&mut tx).await.context("shared admission")?;
        let incidental = IdentityRepository::create(
            &mut *tx,
            CreateIdentityInput {
                login: "notifier_incidental".into(),
                display_name: None,
                password_hash: None,
                attributes: json!({}),
            },
        )
        .await?;
        ensure!(rollback_validation(tx, Some(fence.clone()), &mut holder)
            .await
            .is_some());
        ensure!(holder.confirmed);
        ensure!(owner.release().await);
        ensure!(
            IdentityRepository::find_by_id(fixture.db.pool(), incidental.id)
                .await?
                .is_none()
        );
        fixture.released().await?;
        let mut holder = ValidationHolder::new();
        let mut owner =
            ValidationConnection::acquire(&fixture.pool, TokioInstant::now() + VALIDATION_TIMEOUT)
                .await
                .context("validation connection")?;
        let mut tx = owner.0.as_mut().unwrap().begin().await?;
        holder.select(&mut tx).await.context("shared admission")?;
        let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
            .bind(fixture.backend)
            .fetch_one(fixture.db.pool())
            .await?;
        ensure!(terminated, "could not terminate owned validation backend");
        ensure!(rollback_validation(tx, Some(fence), &mut holder)
            .await
            .is_none());
        ensure!(!holder.confirmed);
        drop(owner);
        ensure!(
            check(&fixture, true).await.is_some(),
            "pool did not recover after backend disposal"
        );
        fixture.released().await
    }
    .await;
    fixture.cleanup().await?;
    result
}

#[tokio::test]
async fn queued_exclusive_waiter_does_not_require_a_second_admitted_connection() -> Result<()> {
    let fixture = Fixture::new().await?;
    let result = async {
        for notification in [false, true] {
            let mut blocker = fixture.db.pool().begin().await?;
            let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await?;
            sqlx::query("SELECT id FROM identity WHERE id = $1 FOR UPDATE")
                .bind(fixture.recorded.binding.sponsor_identity_id)
                .execute(&mut *blocker)
                .await?;
            let mut writer = fixture.db.pool().begin().await?;
            let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *writer)
                .await?;
            let validation = async {
                ensure!(check(&fixture, notification).await.is_some(), "validation failed behind queued writer");
                Ok::<_, anyhow::Error>(())
            };
            let ordering = async {
                wait_blocked(fixture.db.pool(), fixture.backend, blocker_pid).await?;
                let acquisition = SchemaTypeRepository::lock_reclamation(&mut writer);
                tokio::pin!(acquisition);
                tokio::select! {
                    outcome = &mut acquisition => anyhow::bail!("writer did not queue: {outcome:?}"),
                    observed = wait_blocked(fixture.db.pool(), writer_pid, fixture.backend) => observed?,
                }
                blocker.rollback().await?;
                acquisition.await?;
                Ok::<_, anyhow::Error>(())
            };
            let (validated, ordered) = tokio::time::timeout(DEADLINE, async {
                tokio::join!(validation, ordering)
            })
            .await?;
            // Neither future escapes this scope, including on timeout.
            writer.rollback().await?;
            validated?;
            ordered?;
            fixture.released().await?;
        }
        Ok(())
    }
    .await;
    fixture.cleanup().await?;
    result
}

#[tokio::test]
async fn cancellation_recovers_the_owned_pool_and_releases_shared_admission() -> Result<()> {
    let fixture = Fixture::new().await?;
    let result = async {
        for notification in [false, true] {
            let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&fixture.pool)
                .await?;
            let mut blocker = fixture.db.pool().begin().await?;
            let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await?;
            sqlx::query("SELECT id FROM identity WHERE id = $1 FOR UPDATE")
                .bind(fixture.recorded.binding.sponsor_identity_id)
                .execute(&mut *blocker)
                .await?;
            let mut writer = fixture.db.pool().begin().await?;
            let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *writer)
                .await?;
            {
                let acquisition = SchemaTypeRepository::lock_reclamation(&mut writer);
                tokio::pin!(acquisition);
                {
                    let validation = check(&fixture, notification);
                    tokio::pin!(validation);
                    tokio::select! {
                        _ = &mut validation => anyhow::bail!("validation did not wait on the owned blocker"),
                        observed = wait_blocked(fixture.db.pool(), backend, blocker_pid) => observed?,
                    }
                    tokio::select! {
                        outcome = &mut acquisition => anyhow::bail!("writer did not queue: {outcome:?}"),
                        observed = wait_blocked(fixture.db.pool(), writer_pid, backend) => observed?,
                    }
                    // Drop the actual validation future, without releasing identity.
                    // Ordinary SQLx async pool return cannot pass this assertion.
                }
                wait_backend_gone(fixture.db.pool(), backend).await?;
                tokio::time::timeout(DEADLINE, &mut acquisition).await??;
                tokio::time::timeout(DEADLINE, async {
                    let mut connection = fixture.pool.acquire().await?;
                    let ready: i32 = sqlx::query_scalar("SELECT 1")
                        .fetch_one(&mut *connection)
                        .await?;
                    ensure!(ready == 1, "pool did not recover after cancellation");
                    Ok::<_, anyhow::Error>(())
                })
                .await??;
                fixture.released().await?;
            }
            writer.rollback().await?;
            blocker.rollback().await?;
            ensure!(check(&fixture, notification).await.is_some());
            fixture.released().await?;
        }
        Ok(())
    }
    .await;
    fixture.cleanup().await?;
    result
}

#[tokio::test]
async fn pending_statement_cancellation_is_drained_before_confirmed_rollback() -> Result<()> {
    let fixture = Fixture::new().await?;
    let result = async {
        let mut blocker = fixture.db.pool().begin().await?;
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        sqlx::query("SELECT id FROM identity WHERE id = $1 FOR UPDATE")
            .bind(fixture.recorded.binding.sponsor_identity_id)
            .execute(&mut *blocker)
            .await?;
        let mut holder = ValidationHolder::new();
        let mut owner = ValidationConnection::acquire(
            &fixture.pool,
            TokioInstant::now() + VALIDATION_TIMEOUT,
        )
        .await
        .context("validation connection")?;
        let mut tx = owner.0.as_mut().unwrap().begin().await?;
        holder.select(&mut tx).await.context("shared admission")?;
        sqlx::query("SET LOCAL statement_timeout = '1000ms'")
            .execute(&mut *tx)
            .await?;
        {
            let query = sqlx::query("SELECT id FROM identity WHERE id = $1 FOR SHARE")
                .bind(fixture.recorded.binding.sponsor_identity_id)
                .execute(&mut *tx);
            tokio::pin!(query);
            tokio::select! {
                outcome = &mut query => anyhow::bail!("query did not wait: {outcome:?}"),
                observed = wait_blocked(fixture.db.pool(), fixture.backend, blocker_pid) => observed?,
            }
            // The unread 57014 arrives during cleanup, not during this query.
        }
        ensure!(rollback_validation(tx, None, &mut holder).await.is_none());
        ensure!(holder.confirmed, "pending 57014 prevented confirmed rollback");
        ensure!(owner.release().await, "pool return did not complete");
        fixture.released().await?;
        let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&fixture.pool)
            .await?;
        ensure!(backend == fixture.backend, "clean connection was not reused");
        let mut writer = fixture.db.pool().begin().await?;
        tokio::time::timeout(DEADLINE, SchemaTypeRepository::lock_reclamation(&mut writer)).await??;
        writer.rollback().await?;
        blocker.rollback().await?;
        Ok(())
    }
    .await;
    fixture.cleanup().await?;
    result
}

#[tokio::test]
async fn statement_deadline_denies_validation_while_identity_remains_blocked() -> Result<()> {
    let fixture = Fixture::new().await?;
    let result = async {
        for notification in [false, true] {
            let mut blocker = fixture.db.pool().begin().await?;
            let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await?;
            sqlx::query("SELECT id FROM identity WHERE id = $1 FOR UPDATE")
                .bind(fixture.recorded.binding.sponsor_identity_id)
                .execute(&mut *blocker)
                .await?;
            {
                let validation = check(&fixture, notification);
                tokio::pin!(validation);
                tokio::select! {
                    _ = &mut validation => anyhow::bail!("validation did not wait"),
                    observed = wait_blocked(fixture.db.pool(), fixture.backend, blocker_pid) => observed?,
                }
                ensure!(tokio::time::timeout(DEADLINE, &mut validation).await?.is_none());
            }
            fixture.released().await?;
            let mut writer = fixture.db.pool().begin().await?;
            tokio::time::timeout(DEADLINE, SchemaTypeRepository::lock_reclamation(&mut writer)).await??;
            writer.rollback().await?;
            blocker.rollback().await?;
            ensure!(check(&fixture, notification).await.is_some());
        }
        Ok(())
    }
    .await;
    fixture.cleanup().await?;
    result
}

#[tokio::test]
async fn cancelled_validation_task_and_working_pool_drain_before_shutdown_returns() -> Result<()> {
    let fixture = Fixture::new().await?;
    let result = async {
        let mut blocker = fixture.db.pool().begin().await?;
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        sqlx::query("SELECT id FROM identity WHERE id = $1 FOR UPDATE")
            .bind(fixture.recorded.binding.sponsor_identity_id)
            .execute(&mut *blocker)
            .await?;
        let recorded = fixture.recorded.clone();
        let pool = fixture.pool.clone();
        let mut tasks = tokio::task::JoinSet::new();
        tasks.spawn(async move { recorded.source_fence(&pool).await });
        // JoinSet owns the task even if readiness observation fails.
        let observed = wait_blocked(fixture.db.pool(), fixture.backend, blocker_pid).await;
        tasks.abort_all();
        while let Some(joined) = tasks.join_next().await {
            ensure!(joined.is_err_and(|error| error.is_cancelled()));
        }
        observed?;
        tokio::time::timeout(DEADLINE, crate::service::drain_working_pool(&fixture.pool)).await??;
        ensure!(
            fixture.pool.size() == 0,
            "working pool retained a connection"
        );
        // Pool drain alone is not evidence that PostgreSQL released its locks.
        wait_backend_gone(fixture.db.pool(), fixture.backend).await?;
        fixture.released().await?;
        let mut writer = fixture.db.pool().begin().await?;
        tokio::time::timeout(
            DEADLINE,
            SchemaTypeRepository::lock_reclamation(&mut writer),
        )
        .await??;
        writer.rollback().await?;
        blocker.rollback().await?;
        Ok(())
    }
    .await;
    fixture.cleanup().await?;
    result
}

#[tokio::test]
async fn shutdown_drains_an_active_validation_with_identity_still_blocked() -> Result<()> {
    let fixture = Fixture::new().await?;
    let result = async {
        let mut blocker = fixture.db.pool().begin().await?;
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        sqlx::query("SELECT id FROM identity WHERE id = $1 FOR UPDATE")
            .bind(fixture.recorded.binding.sponsor_identity_id)
            .execute(&mut *blocker)
            .await?;
        let recorded = fixture.recorded.clone();
        let pool = fixture.pool.clone();
        let mut tasks = tokio::task::JoinSet::new();
        tasks.spawn(async move { recorded.source_fence(&pool).await });
        let observed = wait_blocked(fixture.db.pool(), fixture.backend, blocker_pid).await;
        let drained =
            tokio::time::timeout(DEADLINE, crate::service::drain_working_pool(&fixture.pool)).await;
        if observed.is_err() || !matches!(&drained, Ok(Ok(()))) {
            tasks.abort_all();
        }
        // Always join the owner, including failed shutdown/readiness paths.
        let joined = tasks.join_next().await;
        observed?;
        drained??;
        ensure!(
            matches!(joined, Some(Ok(None))),
            "blocked validation did not fail closed"
        );
        ensure!(fixture.pool.size() == 0, "working pool was not drained");
        wait_backend_gone(fixture.db.pool(), fixture.backend).await?;
        fixture.released().await?;
        blocker.rollback().await?;
        Ok(())
    }
    .await;
    fixture.cleanup().await?;
    result
}

/// Exercise the production socket router, including the final frame fence.
#[tokio::test]
async fn delegated_socket_rejects_stale_terminal_and_closed_attempts() -> Result<()> {
    use crate::{subscriber_manager::SubscriberManager, websocket_server::WebSocketServer};
    use attune_common::{
        auth::jwt::{generate_delegated_execution_token, JwtConfig},
        repositories::{
            delegated_execution::DelegatedExecutionRepository as Delegated,
            execution::UpdateExecutionInput,
            runtime::{CreateWorkerInput, WorkerRepository},
            schema_contract::{BindingOwner, SchemaContractRepository},
            Update,
        },
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };
    use tokio_util::sync::CancellationToken;

    async fn connect(address: std::net::SocketAddr, token: &str) -> Result<(TcpStream, u16)> {
        let mut socket = TcpStream::connect(address).await?;
        let request = format!("GET /ws HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: attune.v1, attune.jwt.{token}\r\n\r\n");
        socket.write_all(request.as_bytes()).await?;
        let mut headers = Vec::new();
        tokio::time::timeout(DEADLINE, async {
            while !headers.ends_with(b"\r\n\r\n") {
                ensure!(headers.len() < 16384, "oversized socket handshake");
                headers.push(socket.read_u8().await?);
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        let status = std::str::from_utf8(&headers)?
            .split_whitespace()
            .nth(1)
            .context("socket status missing")?
            .parse()?;
        Ok((socket, status))
    }
    async fn read_frame(socket: &mut TcpStream) -> Result<(u8, Vec<u8>)> {
        tokio::time::timeout(DEADLINE, async {
            let opcode = socket.read_u8().await? & 15;
            let length = socket.read_u8().await? & 127;
            let length = if length == 126 {
                socket.read_u16().await? as usize
            } else {
                length as usize
            };
            ensure!(length < 8192, "oversized socket frame");
            let mut bytes = vec![0; length];
            socket.read_exact(&mut bytes).await?;
            Ok((opcode, bytes))
        })
        .await?
    }

    let fixture = Fixture::new().await?;
    let mut config = Config::load_from_file(&format!(
        "{}/../../config.test.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))?;
    let jwt = JwtConfig {
        secret: "delegated-notifier-socket-test".into(),
        access_token_expiration: 600,
        refresh_token_expiration: 600,
    };
    config.security.jwt_secret = Some(jwt.secret.clone());
    let shutdown = CancellationToken::new();
    let (notifications, _) = tokio::sync::broadcast::channel(8);
    let server = WebSocketServer::new(
        config,
        notifications,
        Arc::new(SubscriberManager::new()),
        shutdown.clone(),
        fixture.db.pool().clone(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let stop = shutdown.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, server.router())
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
    });
    let result: Result<()> = async {
        let original = ExecutionRepository::find_by_id(
            fixture.db.pool(),
            fixture.recorded.binding.execution_id,
        )
        .await?
        .context("source missing")?;
        let snapshot = original.executable_snapshot.context("snapshot missing")?;
        let mut tx = fixture.db.begin().await?;
        let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
        let execution = ExecutionRepository::create_pinned(
            &mut *tx,
            CreateExecutionInput {
                action: original.action,
                action_ref: original.action_ref.clone(),
                executor: original.executor,
                permission_set_refs: vec!["standard".into()],
                ..Default::default()
            },
            &snapshot,
        )
        .await?;
        SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
            .await?;
        Delegated::select(&mut tx, execution.id, None).await?;
        let worker = WorkerRepository::create(
            &mut *tx,
            CreateWorkerInput {
                name: "delegated-notifier-test".into(),
                worker_type: attune_common::models::WorkerType::Local,
                runtime: None,
                host: None,
                port: None,
                status: Some(attune_common::models::WorkerStatus::Active),
                capabilities: None,
                meta: None,
            },
        )
        .await?;
        ExecutionRepository::update(
            &mut *tx,
            execution.id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Scheduled),
                worker: Some(worker.id),
                ..Default::default()
            },
        )
        .await?;
        let attempt = Delegated::start(&mut tx, execution.id, worker.id).await?;
        ExecutionRepository::transition_worker(
            &mut *tx,
            execution.id,
            Some(worker.id),
            ExecutionStatus::Running,
            None,
        )
        .await?;
        tx.commit().await?;
        let token = |attempt| {
            generate_delegated_execution_token(
                original.executor.unwrap(),
                execution.id,
                &execution.action_ref,
                &jwt,
                600,
                &["standard".into()],
                std::slice::from_ref(&execution.action_ref),
                attempt,
            )
        };
        let valid = token(attempt)?;
        let (_, status) = connect(address, &token(uuid::Uuid::new_v4())?).await?;
        ensure!(status == 401, "stale socket attempt accepted: {status}");
        let (mut socket, status) = connect(address, &valid).await?;
        ensure!(status == 101, "live delegated socket rejected: {status}");
        let (opcode, welcome) = read_frame(&mut socket).await?;
        ensure!(
            opcode == 1
                && serde_json::from_slice::<serde_json::Value>(&welcome)?["type"] == "welcome",
            "welcome frame missing"
        );
        let message = br#"{"type":"subscribe","filter":"unknown"}"#;
        let mut frame = vec![0x81, 0x80 | message.len() as u8, 1, 2, 3, 4];
        frame.extend(
            message
                .iter()
                .enumerate()
                .map(|(i, byte)| byte ^ [1, 2, 3, 4][i % 4]),
        );
        socket.write_all(&frame).await?;
        let (opcode, error) = read_frame(&mut socket).await?;
        ensure!(
            opcode == 1 && serde_json::from_slice::<serde_json::Value>(&error)?["type"] == "error",
            "live subscription error frame missing"
        );
        let mut tx = fixture.db.begin().await?;
        Delegated::close(&mut tx, execution.id).await?;
        tx.commit().await?;
        // The same request emits no data after closure, even when its control
        // reply was queued before actual socket readiness.
        socket.write_all(&frame).await?;
        match read_frame(&mut socket).await {
            Ok((opcode, _)) => ensure!(opcode == 8, "closed attempt emitted a data frame"),
            Err(error) => ensure!(
                error.downcast_ref::<std::io::Error>().is_some(),
                "socket failed to close: {error}"
            ),
        }
        let (_, status) = connect(address, &valid).await?;
        ensure!(status == 401, "closed socket attempt accepted: {status}");
        ExecutionRepository::transition_worker(
            fixture.db.pool(),
            execution.id,
            Some(worker.id),
            ExecutionStatus::Completed,
            None,
        )
        .await?;
        let (_, status) = connect(address, &valid).await?;
        ensure!(
            status == 401,
            "terminal socket credential accepted: {status}"
        );
        Ok(())
    }
    .await;
    shutdown.cancel();
    let stopped = tokio::time::timeout(DEADLINE, task).await;
    let cleaned = fixture.cleanup().await;
    stopped???;
    cleaned?;
    result
}
