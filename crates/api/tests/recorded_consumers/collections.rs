//! Source-only collection acceptance tests for the central PostgreSQL 18 run.
use super::*;
use attune_common::repositories::{
    execution::ExecutionSearchFilters,
    runtime_authority::recorded_collections,
    work_queue::{WorkQueueItemSearchFilters, WorkQueueSearchFilters},
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tracing::{
    field::{Field, Visit},
    Event, Subscriber,
};
use tracing_subscriber::{layer::Context, prelude::*, Layer};

#[path = "bulk_metadata.rs"]
mod bulk_metadata;

#[path = "requested_publication.rs"]
mod requested_publication;
#[path = "root_cancellation.rs"]
mod root_cancellation;

// A genuinely independent invocation, not B's ParentExecution helper. Parent
// admissions deliberately share revocation dependencies and are not siblings for
// the same-sponsor/different-execution revocation acceptance case.
async fn independent_source(fixture: &Fixture) -> Result<(RuntimeCredentialBinding, String)> {
    let mut tx = fixture.ctx.pool.begin().await?;
    let policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap();
    let refs = vec!["recorded_emitter.events".to_owned()];
    let selected = policy.policy.select(
        policy.id,
        &policy.policy.invocations[0].binding,
        &refs,
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: fixture.requester,
                login: fixture.ctx.user.as_ref().unwrap().login.clone(),
            },
        },
    )?;
    let source = ExecutionRepository::find_by_id(&mut *tx, fixture.credential.execution_id)
        .await?
        .unwrap();
    let snapshot = source.executable_snapshot.unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: "recorded_emitter.emit".into(),
            status: ExecutionStatus::Running,
            executor: Some(fixture.requester),
            permission_set_refs: refs.clone(),
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
    let credential = RuntimeCredentialBinding {
        execution_id: execution.id,
        admission_id: admission.id,
        sponsor_identity_id: selected.owner.sponsor_identity_id,
        action_ref: "recorded_emitter.emit".into(),
        attempt: selected.attempt,
    };
    tx.commit().await?;
    let token = generate_admitted_execution_token(
        &credential,
        &refs,
        &fixture.ctx.state.jwt_config,
        Some(360),
    )?;
    Ok((credential, token))
}

#[derive(Clone, Default)]
struct Statements(Arc<AtomicUsize>);

#[derive(Default)]
struct StatementKind {
    catalog: bool,
}
impl Visit for StatementKind {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "db.statement" {
            let sql = format!("{value:?}");
            self.catalog = sql.contains("pg_catalog.pg_type") || sql.contains("pg_catalog.pg_enum");
        }
    }
}
impl<S: Subscriber> Layer<S> for Statements {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            let mut kind = StatementKind::default();
            event.record(&mut kind);
            if !kind.catalog {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

#[tokio::test]
async fn recorded_collection_noise_does_not_limit_scoped_pages_and_queries_are_bounded(
) -> Result<()> {
    use tracing::instrument::WithSubscriber;
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[
            json!({"resource":"executions","actions":["read"]}),
            json!({"resource":"queues","actions":["read"]}),
            json!({"resource":"queue_items","actions":["read"]}),
        ],
    )
    .await?;
    let queue = managed_queue(&fixture, "recorded_emitter.collection").await?;
    sqlx::query("INSERT INTO work_queue (ref,pack,pack_ref,is_adhoc,label,dispatch_action_ref) SELECT 'recorded_emitter.collection_' || n::text,$1,'recorded_emitter',false,'Collection','recorded_emitter.emit' FROM generate_series(1,99) n")
        .bind(fixture.pack_id).execute(&fixture.ctx.pool).await?;
    sqlx::query("INSERT INTO work_queue (ref,pack,pack_ref,is_adhoc,label,dispatch_action_ref) SELECT 'recorded_emitter.foreign_prefix',id,ref,false,'Foreign','recorded_emitter.emit' FROM pack WHERE ref = 'foreign_events'")
        .execute(&fixture.ctx.pool).await?;
    // Unrelated executions need no admission: they are read targets, not sources.
    sqlx::query("INSERT INTO execution (action_ref,status) SELECT 'unrelated.run','completed'::execution_status_enum FROM generate_series(1,10001)")
        .execute(&fixture.ctx.pool).await?;
    sqlx::query("INSERT INTO execution (action_ref,status,pack_release,pack_release_digest,executable_snapshot) SELECT action_ref,'completed'::execution_status_enum,pack_release,pack_release_digest,executable_snapshot FROM execution CROSS JOIN generate_series(1,100) WHERE id = $1")
        .bind(fixture.credential.execution_id).execute(&fixture.ctx.pool).await?;
    sqlx::query("INSERT INTO execution (action_ref,status,pack_release,pack_release_digest,executable_snapshot) SELECT action_ref,'completed'::execution_status_enum,pack_release,repeat('0',64),executable_snapshot FROM execution WHERE id = $1")
        .bind(fixture.credential.execution_id).execute(&fixture.ctx.pool).await?;
    sqlx::query("INSERT INTO work_queue_item (queue,queue_ref,item_key,status,payload,enqueue_source) SELECT $1,$2,n::text,'queued'::work_queue_item_status_enum,'{}'::jsonb,'collection-test' FROM generate_series(1,100) n")
        .bind(queue.id).bind(&queue.r#ref).execute(&fixture.ctx.pool).await?;
    let response = fixture
        .ctx
        .get("/api/v1/executions?per_page=1", Some(&fixture.token))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    for targets in [1, 64, 100] {
        let observer = Statements::default();
        let subscriber = tracing_subscriber::registry().with(observer.clone());
        let result = recorded_collections::executions(
            &mut tx,
            &authority.execution_collection(attune_common::rbac::Action::Read),
            &ExecutionSearchFilters {
                limit: targets,
                ..Default::default()
            },
        )
        .with_subscriber(subscriber)
        .await?;
        assert_eq!(result.rows.len(), targets as usize);
        assert_eq!(result.total, Some(101));
        assert!(result
            .rows
            .iter()
            .all(|row| row.config.is_none() && row.result.is_none() && row.parent.is_none()));
        // Count, page, DB-clock deadline. Source validation/pool setup is outside
        // this repository budget; it does not depend on the target count.
        assert_eq!(
            observer.0.load(Ordering::Relaxed),
            3,
            "execution targets={targets}"
        );

        let observer = Statements::default();
        let subscriber = tracing_subscriber::registry().with(observer.clone());
        let result = recorded_collections::queue_items(
            &mut tx,
            &authority.queue_item_collection(),
            &WorkQueueItemSearchFilters {
                queue: Some(queue.id),
                limit: targets,
                ..Default::default()
            },
            None,
        )
        .with_subscriber(subscriber)
        .await?;
        assert_eq!(result.rows.len(), targets as usize);
        assert_eq!(result.total, 100);
        assert_eq!(
            observer.0.load(Ordering::Relaxed),
            3,
            "item targets={targets}"
        );
        let observer = Statements::default();
        let subscriber = tracing_subscriber::registry().with(observer.clone());
        let result = recorded_collections::queues(
            &mut tx,
            &authority.queue_collection(),
            &WorkQueueSearchFilters {
                limit: targets,
                ..Default::default()
            },
        )
        .with_subscriber(subscriber)
        .await?;
        assert_eq!(result.rows.len(), targets as usize);
        assert_eq!(result.total, 100);
        assert!(result.rows.iter().all(|queue| queue.config
            == attune_common::secret_values::redaction_marker()
            && queue.action_params == attune_common::secret_values::redaction_marker()
            && queue.item_schema == json!({})));
        assert_eq!(
            observer.0.load(Ordering::Relaxed),
            3,
            "queue targets={targets}"
        );
    }
    let counts = recorded_collections::execution_status_counts(
        &mut tx,
        &authority.execution_collection(attune_common::rbac::Action::Read),
    )
    .await?;
    assert_eq!(counts.iter().map(|(_, count)| count).sum::<i64>(), 101);
    let wildcard = recorded_collections::executions(
        &mut tx,
        &authority.execution_collection(attune_common::rbac::Action::Read),
        &ExecutionSearchFilters {
            action_ref: Some("recorded_*.*".into()),
            limit: 1,
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(wildcard.total, Some(101));
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_item_id_scope_filters_pages_and_preview_totals_not_queue_ids() -> Result<()> {
    let mut fixture = fixture_with_additional_grants(false, false, &[], &[], &[]).await?;
    append_local_reader(
        &mut fixture,
        json!({"resource":"queue_items","actions":["read"],"constraints":{"ids":[700001]}}),
    )
    .await?;
    let queue = managed_queue(&fixture, "recorded_emitter.narrow_items").await?;
    sqlx::query("INSERT INTO work_queue_item (id,queue,queue_ref,item_key,status,payload,metadata,ack_summary,enqueue_source) SELECT n,$1,$2,n::text,'queued'::work_queue_item_status_enum,'{}'::jsonb,'{\"copied\":\"private\"}'::jsonb,'{\"batch\":\"private\"}'::jsonb,'collection-test' FROM generate_series(700001,700003) n")
        .bind(queue.id).bind(&queue.r#ref).execute(&fixture.ctx.pool).await?;
    let path = format!("/api/v1/queues/{}/items", queue.r#ref);
    let response = fixture.ctx.get(&path, Some(&fixture.token)).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert!(!body.to_string().contains("private"));
    let response = fixture
        .ctx
        .post(
            &format!("{path}/query/preview"),
            json!({"selector":{"path":"$","vars":{}},"limit":100}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["matched_count"], 1);
    assert_eq!(body["data"]["items"][0]["id"], 700001);
    assert!(body["data"]["items"][0]["ack_summary"].is_null());
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_collection_same_sponsor_source_revocation_is_execution_specific() -> Result<()> {
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[json!({"resource":"executions","actions":["read"]})],
    )
    .await?;
    let (sibling_credential, sibling_token) = independent_source(&fixture).await?;
    assert_eq!(
        sibling_credential.sponsor_identity_id,
        fixture.credential.sponsor_identity_id
    );
    assert_ne!(
        sibling_credential.execution_id,
        fixture.credential.execution_id
    );
    assert_ne!(
        sibling_credential.admission_id,
        fixture.credential.admission_id
    );
    let mut tx = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_admission(&mut tx, fixture.credential.admission_id).await?;
    tx.commit().await?;
    assert_eq!(
        fixture
            .ctx
            .get("/api/v1/executions", Some(&fixture.token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .ctx
            .get("/api/v1/executions", Some(&sibling_token))
            .await?
            .status(),
        StatusCode::OK
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

async fn append_local_reader(fixture: &mut Fixture, grant: Value) -> Result<()> {
    let mut tx = fixture.ctx.pool.begin().await?;
    let mut policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap()
        .policy
        .0;
    policy.invocations[0]
        .permission_sets
        .get_mut("recorded_emitter.events")
        .unwrap()
        .grants
        .push(attune_common::runtime_authority::AdmittedGrant::PackOwned {
            pack_id: fixture.pack_id,
            grant: serde_json::from_value(grant)?,
        });
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy).await?;
    tx.commit().await?;
    fixture.policy_id = policy.id;
    let (credential, token) = sibling(fixture).await?;
    fixture.credential = credential;
    fixture.token = token;
    Ok(())
}

#[tokio::test]
async fn recorded_queue_collection_pins_do_not_follow_deleted_recreated_refs() -> Result<()> {
    let mut fixture = fixture(false).await?;
    let queue = managed_queue(&fixture, "recorded_emitter.pinned_collection").await?;
    append_local_reader(
        &mut fixture,
        json!({"resource":"queues","actions":["read"],"constraints":{"refs":[queue.r#ref]}}),
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    let result = recorded_collections::queues(
        &mut tx,
        &authority.queue_collection(),
        &WorkQueueSearchFilters {
            limit: 100,
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(result.total, 1);
    assert_eq!(result.rows[0].id, queue.id);
    tx.rollback().await?;
    // Preserve the namespace while replacing the concrete resource identity.
    attune_common::repositories::work_queue::WorkQueueRepository::delete(
        &fixture.ctx.pool,
        queue.id,
    )
    .await?;
    let replacement = managed_queue(&fixture, "recorded_emitter.pinned_collection").await?;
    assert_ne!(replacement.id, queue.id);
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    let result = recorded_collections::queues(
        &mut tx,
        &authority.queue_collection(),
        &WorkQueueSearchFilters {
            limit: 100,
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(result.total, 0);
    assert!(result.rows.is_empty());
    tx.rollback().await?;
    ExecutionRepository::delete(&fixture.ctx.pool, fixture.credential.execution_id).await?;
    // RequireAuth rejects a missing recorded source as an invalid credential,
    // before the collection route can return an operation-level 403.
    assert_eq!(
        fixture
            .ctx
            .get("/api/v1/queues", Some(&fixture.token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_collection_empty_execution_selection_and_pack_association_do_not_grant_local_ownership(
) -> Result<()> {
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[json!({"resource":"queues","actions":["read"]})],
    )
    .await?;
    sqlx::query("INSERT INTO work_queue (ref,pack,pack_ref,is_adhoc,label,dispatch_action_ref) VALUES ('recorded_emitter.associated',$1,'recorded_emitter',true,'Not managed','recorded_emitter.emit')")
        .bind(fixture.pack_id).execute(&fixture.ctx.pool).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    let queues = recorded_collections::queues(
        &mut tx,
        &authority.queue_collection(),
        &WorkQueueSearchFilters {
            limit: 100,
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(queues.total, 0);
    let executions = recorded_collections::executions(
        &mut tx,
        &authority.execution_collection(attune_common::rbac::Action::Read),
        &ExecutionSearchFilters {
            limit: 100,
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(executions.total, Some(0));
    assert!(executions.rows.is_empty());
    tx.rollback().await?;
    // An ordinary selected owner scope remains open to future managed rows.
    managed_queue(&fixture, "recorded_emitter.future").await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    let queues = recorded_collections::queues(
        &mut tx,
        &authority.queue_collection(),
        &WorkQueueSearchFilters {
            limit: 100,
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(queues.total, 1);
    assert_eq!(queues.rows[0].r#ref, "recorded_emitter.future");
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_collection_expiry_after_query_wait_denies_rows_and_counts() -> Result<()> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let mut fixture = fixture(false).await?;
    let queue = managed_queue(&fixture, "recorded_emitter.expiring_collection").await?;
    let grant: attune_common::rbac::Grant =
        serde_json::from_value(json!({"resource":"queue_items","actions":["read"]}))?;
    let issuer = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.collection_issuer".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: serde_json::to_value(vec![grant.clone()])?,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: issuer.id,
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let deadline = chrono::Utc::now() + chrono::Duration::seconds(20);
    let approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        fixture.pack_id,
        fixture.requester,
        vec![grant.clone()],
        Some(deadline),
    )
    .await?;
    let deadline = approval.expires_at.unwrap();
    let mut policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap()
        .policy
        .0;
    policy.invocations[0]
        .permission_sets
        .get_mut("recorded_emitter.events")
        .unwrap()
        .grants
        .push(attune_common::runtime_authority::AdmittedGrant::External {
            approval_id: approval.id,
            grant,
        });
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy).await?;
    tx.commit().await?;
    fixture.policy_id = policy.id;
    let (credential, token) = sibling(&fixture).await?;
    fixture.credential = credential;
    fixture.token = token;

    // Validate source first. The target table lock below never holds up source
    // validation, and the final deadline check must not relock source/approval.
    let mut source = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut source, &fixture.credential)
            .await?;
    assert_eq!(authority.effect_fence().approval_deadline(), Some(deadline));
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *source)
        .await?;
    let mut blocker = fixture.ctx.pool.begin().await?;
    sqlx::query("LOCK TABLE work_queue_item IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    let queue_id = queue.id;
    let query = tokio::spawn(async move {
        recorded_collections::queue_items(
            &mut source,
            &authority.queue_item_collection(),
            &WorkQueueItemSearchFilters {
                queue: Some(queue_id),
                limit: 1,
                ..Default::default()
            },
            None,
        )
        .await
    });
    let blocked = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT cardinality(pg_blocking_pids($1)) > 0")
                .bind(pid)
                .fetch_one(&fixture.ctx.pool)
                .await?;
            if waiting {
                break Ok::<_, sqlx::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    // Always release our lock and join the operation, including failed barriers.
    if !matches!(blocked, Ok(Ok(()))) {
        blocker.rollback().await?;
        let _ = query.await?;
        return Err("collection did not reach the real target query wait".into());
    }
    let expired = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let expired: bool = sqlx::query_scalar("SELECT clock_timestamp() >= $1")
                .bind(deadline)
                .fetch_one(&fixture.ctx.pool)
                .await?;
            if expired {
                break Ok::<_, sqlx::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    blocker.rollback().await?;
    let result = query.await?;
    expired??;
    assert!(matches!(
        result,
        Err(attune_common::Error::PermissionDenied(_))
    ));
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_preview_counts_cannot_probe_redacted_batch_metadata_or_execution_links(
) -> Result<()> {
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[
            json!({"resource":"queue_items","actions":["read"]}),
            json!({"resource":"executions","actions":["read"]}),
        ],
    )
    .await?;
    let queue = managed_queue(&fixture, "recorded_emitter.private_counts").await?;
    sqlx::query("INSERT INTO work_queue_item(queue,queue_ref,payload,enqueue_source,metadata,ack_summary,leased_execution,requested_by_execution) SELECT $1,$2,'{}'::jsonb,'privacy-test','{\"mixed_batch\":{\"peer_secret\":\"peer_private\"}}'::jsonb,'{\"result\":\"peer_private\"}'::jsonb,$3,$3 FROM generate_series(1,2)")
        .bind(queue.id).bind(&queue.r#ref).bind(fixture.credential.execution_id).execute(&fixture.ctx.pool).await?;
    let path = format!("/api/v1/queues/{}/items/query/preview", queue.r#ref);
    for selector in [
        "$ ? (@.metadata.mixed_batch.peer_secret == \"peer_private\")",
        "$ ? (@.ack_summary.result == \"peer_private\")",
        "$ ? (exists(@.leased_execution))",
    ] {
        let response = fixture
            .ctx
            .post(
                &path,
                json!({"selector":{"path":selector,"vars":{}},"limit":100}),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await?;
        assert_eq!(body["data"]["matched_count"], 0);
    }
    let response = fixture
        .ctx
        .post(
            &path,
            json!({"selector":{"path":"$","vars":{}},"limit":100}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["matched_count"], 2);
    assert!(!body.to_string().contains("peer_private"));
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    let metadata_rows = recorded_collections::queue_items(
        &mut tx,
        &authority.queue_item_collection(),
        &WorkQueueItemSearchFilters {
            queue: Some(queue.id),
            limit: 100,
            ..Default::default()
        },
        None,
    )
    .await?;
    assert_eq!(metadata_rows.total, 2);
    assert!(metadata_rows.rows.iter().all(|row| row.metadata.is_null()
        && row.ack_summary.is_none()
        && row.last_error.is_none()
        && row.leased_execution.is_none()
        && row.requested_by_execution.is_none()
        && row.schema_binding.is_none()
        && row.executable_snapshot.is_none()));
    for filters in [
        WorkQueueItemSearchFilters {
            queue: Some(queue.id),
            leased_execution: Some(fixture.credential.execution_id),
            ..Default::default()
        },
        WorkQueueItemSearchFilters {
            queue: Some(queue.id),
            requested_by_execution: Some(fixture.credential.execution_id),
            ..Default::default()
        },
        WorkQueueItemSearchFilters {
            queue: Some(queue.id),
            requested_by_enforcement: Some(1),
            ..Default::default()
        },
        WorkQueueItemSearchFilters {
            queue: Some(queue.id),
            lease_token: Some(uuid::Uuid::new_v4()),
            ..Default::default()
        },
        WorkQueueItemSearchFilters {
            queue: Some(queue.id),
            only_expired_leases: true,
            ..Default::default()
        },
    ] {
        assert!(matches!(
            recorded_collections::queue_items(
                &mut tx,
                &authority.queue_item_collection(),
                &filters,
                None
            )
            .await,
            Err(attune_common::Error::PermissionDenied(_))
        ));
    }
    for filters in [
        ExecutionSearchFilters {
            parent: Some(fixture.credential.execution_id),
            ..Default::default()
        },
        ExecutionSearchFilters {
            enforcement: Some(1),
            ..Default::default()
        },
        ExecutionSearchFilters {
            rule_ref: Some("foreign.*".into()),
            ..Default::default()
        },
        ExecutionSearchFilters {
            trigger_ref: Some("foreign.*".into()),
            ..Default::default()
        },
    ] {
        assert!(matches!(
            recorded_collections::executions(
                &mut tx,
                &authority.execution_collection(attune_common::rbac::Action::Read),
                &filters
            )
            .await,
            Err(attune_common::Error::PermissionDenied(_))
        ));
    }
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_item_listing_keeps_source_action_disablement_separate_from_terminal_source(
) -> Result<()> {
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[json!({"resource":"queue_items","actions":["read"]})],
    )
    .await?;
    let queue = managed_queue(&fixture, "recorded_emitter.disabled_reader").await?;
    sqlx::query("INSERT INTO work_queue_item(queue,queue_ref,payload,enqueue_source) VALUES($1,$2,'{}','continuity-test')")
        .bind(queue.id).bind(&queue.r#ref).execute(&fixture.ctx.pool).await?;
    sqlx::query("UPDATE action SET enabled = false WHERE ref = 'recorded_emitter.emit'")
        .execute(&fixture.ctx.pool)
        .await?;
    sqlx::query(
        "UPDATE work_queue SET enabled = false,retired_at = clock_timestamp() WHERE id = $1",
    )
    .bind(queue.id)
    .execute(&fixture.ctx.pool)
    .await?;
    let path = format!("/api/v1/queues/{}/items", queue.r#ref);
    assert_eq!(
        fixture.ctx.get(&path, Some(&fixture.token)).await?.status(),
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
        fixture.ctx.get(&path, Some(&fixture.token)).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_unselected_approval_cannot_authorize_items_but_selected_open_external_scope_allows_new_ids(
) -> Result<()> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let mut fixture = fixture(false).await?;
    let grant: attune_common::rbac::Grant =
        serde_json::from_value(json!({"resource":"queue_items","actions":["read"]}))?;
    let issuer = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.unselected_item_approval".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: serde_json::to_value(vec![grant.clone()])?,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: issuer.id,
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        fixture.pack_id,
        fixture.requester,
        vec![grant.clone()],
        None,
    )
    .await?;
    tx.commit().await?;
    let queue: i64 = sqlx::query_scalar("INSERT INTO work_queue(ref,pack,pack_ref,is_adhoc,label,dispatch_action_ref) VALUES('recorded_emitter.external_items',$1,'recorded_emitter',true,'Ad hoc','recorded_emitter.emit') RETURNING id")
        .bind(fixture.pack_id).fetch_one(&fixture.ctx.pool).await?;
    sqlx::query("INSERT INTO work_queue_item(queue,queue_ref,payload,enqueue_source) VALUES($1,'recorded_emitter.external_items','{}','external-test')")
        .bind(queue).execute(&fixture.ctx.pool).await?;
    let filters = WorkQueueItemSearchFilters {
        queue: Some(queue),
        limit: 100,
        ..Default::default()
    };
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    assert!(matches!(
        recorded_collections::queue_items(
            &mut tx,
            &authority.queue_item_collection(),
            &filters,
            None
        )
        .await,
        Err(attune_common::Error::PermissionDenied(_))
    ));
    tx.rollback().await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let mut policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap()
        .policy
        .0;
    policy.invocations[0]
        .permission_sets
        .get_mut("recorded_emitter.events")
        .unwrap()
        .grants
        .push(attune_common::runtime_authority::AdmittedGrant::External {
            approval_id: approval.id,
            grant,
        });
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy).await?;
    tx.commit().await?;
    fixture.policy_id = policy.id;
    let (credential, token) = sibling(&fixture).await?;
    fixture.credential = credential;
    fixture.token = token;
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    let rows = recorded_collections::queue_items(
        &mut tx,
        &authority.queue_item_collection(),
        &filters,
        None,
    )
    .await?;
    assert_eq!(rows.total, 1);
    // No concrete selector was admitted. This explicitly selected approved open
    // scope, unlike a pinned ref scope, may see later actual target IDs.
    sqlx::query("INSERT INTO work_queue_item(queue,queue_ref,payload,enqueue_source) VALUES($1,'recorded_emitter.external_items','{}','external-test')")
        .bind(queue).execute(&mut *tx).await?;
    let rows = recorded_collections::queue_items(
        &mut tx,
        &authority.queue_item_collection(),
        &filters,
        None,
    )
    .await?;
    assert_eq!(rows.total, 2);
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}
