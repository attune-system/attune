//! Actual first-child APPLICATION handoff waits after commit. No claim of
//! blocked broker Ack, channel readiness, socket timing or queued withdrawal.
use super::*;
use attune_common::test_database::TestDatabase;

mod publisher_recovery;

// User-approved TEST authority window for pair and recovery fixtures only.
const PAIR_INPUT_CONSENT_WINDOW: Duration = Duration::from_secs(120);
// Poll/release control only. Never used for positive readiness or authority.
const PAIR_CONTROL_MARGIN: Duration = Duration::from_secs(10);

struct OriginalConsent {
    id: i64,
    deadline: chrono::DateTime<chrono::Utc>,
    // Scheduling metadata anchored before the ONE original deadline query.
    operator_limit: tokio::time::Instant,
    snapshot: serde_json::Value,
}

async fn issue_later_input_consent(
    db: &TestDatabase,
    parent: &Execution,
) -> Result<OriginalConsent> {
    use fixture::helpers::{IdentityFixture, KeyFixture};
    let pool = db.pool();
    anyhow::ensure!(
        parent.permission_set_refs.is_empty(),
        "root SDK refs must remain empty"
    );
    let issuer = IdentityFixture::new_unique("publication_wait_input_issuer")
        .create(db)
        .await?;
    KeyFixture::new_system("receipt_lifetime_original", "original finite input")
        .create(db)
        .await?;
    let grants = serde_json::json!([{
        "resource":"keys", "actions":["read"], "constraints":{"refs":[KEY_REF]}
    }]);
    let set = PermissionSetRepository::create(
        pool,
        CreatePermissionSetInput {
            r#ref: "fixture.publication_wait_input_issuer".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: grants.clone(),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        pool,
        CreatePermissionAssignmentInput {
            identity: issuer.id,
            permset: set.id,
        },
    )
    .await?;
    let mut tx = pool.begin().await?;
    let root = Admissions::require(&mut tx, RuntimeOwner::Execution(parent.id)).await?;
    let declaration = finite_task_input_declaration(&mut tx, parent, "/tasks/2").await?;
    let binding = root.authority.binding.clone();
    let revision = declaration.declaration_revision;
    let declaration_path = declaration.source_path;
    let expression = declaration.expression;
    Declarations::declaration_for_policy(
        &mut tx,
        root.policy_revision,
        &binding,
        &revision,
        &declaration_path,
        &expression,
    )
    .await?;
    let consent_window_started = tokio::time::Instant::now();
    let operator_limit = consent_window_started + PAIR_INPUT_CONSENT_WINDOW;
    let deadline = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
        "SELECT clock_timestamp() + INTERVAL '120 seconds'",
    )
    .fetch_one(&mut *tx)
    .await?;
    let id = Inputs::approve_external_declaration(
        &mut tx,
        root.policy_revision,
        &binding,
        &revision,
        &declaration_path,
        &expression,
        issuer.id,
        serde_json::from_value(grants)?,
        Some(deadline),
    )
    .await?;
    anyhow::ensure!(
        Declarations::approval_is_input_only(&mut tx, id).await?,
        "consent is not input-only"
    );
    anyhow::ensure!(
        !root.authority.approval_ids().contains(&id),
        "input consent entered root SDK authority"
    );
    let original = Authority::find_approval(&mut tx, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("original input consent unavailable"))?;
    anyhow::ensure!(
        original.expires_at == Some(deadline),
        "original deadline differs"
    );
    let accepted = Accepted::revalidate_execution(&mut tx, parent.id, fixture::KEY).await?;
    anyhow::ensure!(
        accepted.approval_deadline().is_none(),
        "root has an unrelated finite fence"
    );
    let snapshot = approval_snapshot(&original)?;
    tx.commit().await?;
    Ok(OriginalConsent {
        id,
        deadline,
        operator_limit,
        snapshot,
    })
}

impl OriginalConsent {
    fn control_wait_limit(&self) -> tokio::time::Instant {
        self.operator_limit + PAIR_CONTROL_MARGIN
    }
}

// Shared by pair and recovery readiness, never by an authority check. Query
// RTT and all later scheduling consume the same originally issued window.
async fn pair_consent_readiness_deadline(
    clock: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
    consent: &OriginalConsent,
) -> Result<tokio::time::Instant> {
    let sample_started = tokio::time::Instant::now();
    let remaining_query = sqlx::query_scalar::<_, i64>(
        "SELECT GREATEST(0::numeric, FLOOR(EXTRACT(EPOCH FROM \
         ($1::timestamptz - clock_timestamp())) * 1000))::bigint",
    )
    .bind(consent.deadline)
    .fetch_one(&mut **clock);
    let remaining_ms = match tokio::time::timeout_at(consent.operator_limit, remaining_query).await
    {
        Ok(remaining) => remaining?,
        Err(_) => {
            clock.close_on_drop();
            anyhow::bail!("original pair consent readiness budget exhausted during measurement");
        }
    };
    let deadline =
        original_consent_readiness_deadline(sample_started, remaining_ms, consent.operator_limit)?;
    anyhow::ensure!(
        tokio::time::Instant::now() < deadline,
        "original pair consent readiness budget exhausted before preparation"
    );
    Ok(deadline)
}

// Keep the parent's passing single-child 30s wrapper byte-identical. This pair-
// specific poll uses the SAME original ID/deadline predicate and interval, with
// an absolute original-window cap plus control-only margin for real expiry.
async fn wait_for_pair_consent_expiry(
    conn: &mut PgConnection,
    consent: &OriginalConsent,
) -> Result<()> {
    tokio::time::timeout_at(consent.control_wait_limit(), async {
        let mut poll = tokio::time::interval(Duration::from_millis(25));
        loop {
            poll.tick().await;
            let expired: bool = sqlx::query_scalar(
                "SELECT expires_at=$2 AND expires_at<=clock_timestamp() \
                   FROM runtime_external_approval WHERE id=$1",
            )
            .bind(consent.id)
            .bind(consent.deadline)
            .fetch_one(&mut *conn)
            .await?;
            if expired {
                return Ok::<(), anyhow::Error>(());
            }
        }
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "original pair input approval did not expire at its unchanged database deadline"
        )
    })?
}

struct PairProofs {
    ids: [i64; 2],
    inputs: TransactionInputReceipts,
}

struct FirstApplicationHandoffBarrier {
    approval: i64,
    original_deadline: chrono::DateTime<chrono::Utc>,
    // Absolute poll/release bound, not a finite proof or publication allowance.
    release_limit: tokio::time::Instant,
    state: Mutex<Option<PairProofs>>,
    ready: Mutex<Option<oneshot::Sender<[i64; 2]>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    observed: Mutex<Option<oneshot::Sender<(bool, bool)>>>,
}

#[async_trait::async_trait]
impl WorkflowInputCommitObserver for FirstApplicationHandoffBarrier {
    async fn after_input_mutations(
        &self,
        conn: &mut PgConnection,
        pending: &WorkflowPublications,
    ) -> Result<()> {
        anyhow::ensure!(pending.len() == 2, "expected two actual successor messages");
        let ids = [pending[0].execution_id, pending[1].execution_id];
        let expected: std::collections::BTreeSet<_> = ids.into_iter().collect();
        anyhow::ensure!(
            expected.len() == 2
                && pending
                    .owner_ids()
                    .collect::<std::collections::BTreeSet<_>>()
                    == expected,
            "actual explicit owner registry lost or duplicated a successor"
        );
        let first = ExecutionRepository::find_by_id(&mut *conn, ids[0])
            .await?
            .ok_or_else(|| anyhow::anyhow!("actual first child unavailable"))?;
        let later = ExecutionRepository::find_by_id(&mut *conn, ids[1])
            .await?
            .ok_or_else(|| anyhow::anyhow!("actual later child unavailable"))?;
        anyhow::ensure!(
            first
                .workflow_task
                .as_ref()
                .is_some_and(|task| task.task_name == "consumer")
                && later
                    .workflow_task
                    .as_ref()
                    .is_some_and(|task| task.task_name == "finite_later"),
            "actual owner must publish ordinary consumer first and finite consumer second"
        );
        anyhow::ensure!(
            first.permission_set_refs.is_empty() && later.permission_set_refs.is_empty(),
            "neither child may obtain SDK Key authority"
        );
        let mut inputs = TransactionInputReceipts::default();
        inputs
            .retain_existing(conn, &first, Some(fixture::KEY))
            .await?;
        inputs
            .retain_existing(conn, &later, Some(fixture::KEY))
            .await?;
        inputs.ensure_fresh_on(conn).await?;
        anyhow::ensure!(
            inputs.publication_wait_deadline(first.id)?.is_none(),
            "first child's complete input closure must be independently nonexpiring"
        );
        anyhow::ensure!(
            inputs.publication_wait_deadline(later.id)? == Some(self.original_deadline),
            "later child's complete original input closure lacks its exact finite deadline"
        );
        let fields = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn,
            ENTITY_EXECUTION_CONFIG,
            later.id,
        )
        .await?;
        anyhow::ensure!(
            fields.iter().any(|field| field.json_path == "/finite"),
            "real finite delivery missing"
        );
        let value = attune_common::secret_values::restore_secret_values(
            later
                .config
                .clone()
                .ok_or_else(|| anyhow::anyhow!("later config missing"))?,
            &fields,
            fixture::KEY,
        )?;
        anyhow::ensure!(
            value["finite"] == serde_json::json!("original finite input"),
            "wrong actual default value"
        );
        let admission = Admissions::require(conn, RuntimeOwner::Execution(later.id)).await?;
        anyhow::ensure!(
            !admission.authority.approval_ids().contains(&self.approval),
            "later input-only approval must not become SDK authority"
        );
        // Read diagnostic lineage in the SAME transaction as original child
        // preparation. Follow actual typed first delivery to original consent.
        let edges: i64 = sqlx::query_scalar(
            "WITH RECURSIVE held AS ( \
                SELECT id,declaration FROM runtime_input_receipt \
                 WHERE execution=$1 AND entity_kind='execution_config' \
                   AND destination_purpose='value' AND destination_path='/finite' \
                UNION \
                SELECT source.id,source.declaration FROM held \
                 JOIN runtime_input_receipt_source edge ON edge.receipt=held.id \
                 JOIN runtime_input_receipt source ON source.id=edge.source_receipt \
             ) SELECT count(*) FROM held \
               JOIN runtime_declared_input_key_read read ON read.declaration=held.declaration \
              WHERE read.approval=$2",
        )
        .bind(later.id)
        .bind(self.approval)
        .fetch_one(&mut *conn)
        .await?;
        anyhow::ensure!(
            edges > 0,
            "later typed input lost its original independent finite approval"
        );
        let mut state = self.state.lock().await;
        anyhow::ensure!(state.is_none(), "actual mutation observer called twice");
        *state = Some(PairProofs { ids, inputs });
        // No precommit wait. The REAL collector must now validate BOTH children
        // fresh and commit. This observer contributes no replacement evidence.
        Ok(())
    }

    async fn before_child_publication(&self, pending: &PendingExecutionRequested) -> Result<()> {
        let ids = self
            .state
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("original prepared child observations missing"))?
            .ids;
        if pending.execution_id == ids[0] {
            let ready = self
                .ready
                .lock()
                .await
                .take()
                .ok_or_else(|| anyhow::anyhow!("first handoff observed twice"))?;
            ready
                .send(ids)
                .map_err(|_| anyhow::anyhow!("postcommit observer controller dropped"))?;
            let release = self
                .release
                .lock()
                .await
                .take()
                .ok_or_else(|| anyhow::anyhow!("first handoff release missing"))?;
            tokio::time::timeout_at(self.release_limit, release)
                .await
                .map_err(|_| anyhow::anyhow!("first application handoff was not released"))?
                .map_err(|_| anyhow::anyhow!("handoff controller dropped before release"))?;
            let (first_fresh, later_expired) = {
                let state = self.state.lock().await;
                let held = state
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("prepared observations missing"))?;
                (
                    held.inputs.ensure_execution_fresh(ids[0]).is_ok(),
                    held.inputs.ensure_execution_fresh(ids[1]).is_err(),
                )
            };
            if let Some(observed) = self.observed.lock().await.take() {
                let _ = observed.send((first_fresh, later_expired));
            }
            // Never return expiry denial from the test observer. First remains
            // authorized; production's separately HELD later-child proofs must
            // reject that later NEW local attempt after the earlier wait.
        } else {
            anyhow::ensure!(
                pending.execution_id == ids[1],
                "unexpected publication owner"
            );
        }
        Ok(())
    }
}

async fn first_application_wait_case(expire: bool) {
    let broker = Arc::new(
        CacheBrokerFixture::connect(if expire {
            "receipt-publication-expiry"
        } else {
            "receipt-publication-control"
        })
        .await,
    );
    let setup = AssertUnwindSafe(fixture::original_executor_finite_publication_pair(
        canonical_task_graph,
    ))
    .catch_unwind()
    .await;
    let (db, files, parent, workflow, completed) = match setup {
        Ok(fixture) => fixture,
        Err(panic) => {
            let cleanup = AssertUnwindSafe(
                Arc::try_unwrap(broker)
                    .ok()
                    .expect("no broker owner task exists during setup")
                    .close(),
            )
            .catch_unwind()
            .await;
            if cleanup.is_err() {
                eprintln!("owned broker cleanup also failed during pair setup failure");
            }
            std::panic::resume_unwind(panic);
        }
    };
    let mut operation: Option<tokio::task::JoinHandle<Result<()>>> = None;
    let mut release_owner = None;
    let result = AssertUnwindSafe(async {
        let pool = db.pool();
        // Reserve before issuing finite consent; diagnostic pool acquisition
        // must not consume the original approval's time window.
        let mut clock = pool.acquire().await?;
        let before = ExecutionRepository::find_by_parent(pool, parent.id).await?;
        let consent = issue_later_input_consent(&db, &parent).await?;
        let readiness_deadline = pair_consent_readiness_deadline(&mut clock, &consent).await?;
        let (ready_send, ready_recv) = oneshot::channel();
        let (release_send, release_recv) = oneshot::channel();
        let (observed_send, observed_recv) = oneshot::channel();
        release_owner = Some(release_send);
        let barrier = FirstApplicationHandoffBarrier {
            approval:consent.id, original_deadline:consent.deadline,
            release_limit:consent.control_wait_limit(), state:Mutex::new(None),
            ready:Mutex::new(Some(ready_send)), release:Mutex::new(Some(release_recv)),
            observed:Mutex::new(Some(observed_send)),
        };
        let owner_pool = pool.clone();
        let owner_broker = Arc::clone(&broker);
        let owner_files = files.path().to_string_lossy().into_owned();
        operation = Some(tokio::spawn(async move {
            let transport: Arc<dyn attune_common::artifact_transport::ArtifactFileTransport> =
                Arc::new(attune_common::artifact_transport::VolumeTransport::new(&owner_files));
            ExecutionScheduler::advance_workflow_owned(
                &owner_pool, &owner_broker.publisher, &AtomicUsize::new(0), &owner_files,
                &transport, 1024 * 1024, 1000, Some(fixture::KEY), &completed,
                &SchedulerMetadataCaches::new(), Some(&barrier),
            ).await
        }));
        let ids = match tokio::time::timeout_at(readiness_deadline, ready_recv).await
            .map_err(|_| anyhow::anyhow!("actual owner did not commit and reach first handoff while consent was fresh"))? {
            Ok(ids) => ids,
            Err(_) => {
                let pending = operation.as_mut()
                    .ok_or_else(|| anyhow::anyhow!("actual scheduler owner missing after handoff readiness channel closed"))?;
                let joined = tokio::time::timeout(Duration::from_secs(15), pending).await;
                match joined {
                    Ok(joined) => {
                        operation.take(); // Completed join; never poll this handle again.
                        return Err(match joined {
                            Ok(Err(error)) => error.context("actual scheduler owner failed before first application handoff"),
                            Ok(Ok(())) => anyhow::anyhow!("actual scheduler owner unexpectedly completed successfully before first application handoff"),
                            Err(error) => anyhow::Error::new(error)
                                .context("actual scheduler owner task failed before first application handoff"),
                        });
                    }
                    Err(_) => return Err(anyhow::anyhow!("actual scheduler owner did not finish after handoff readiness channel closed")),
                }
            }
        };
        anyhow::ensure!(tokio::time::Instant::now() < readiness_deadline,
            "original pair consent readiness observed after its conservative deadline");
        // This is the real POSTCOMMIT hook. Unlike the precommit 06 case,
        // another connection MUST now see BOTH actual Requested child rows.
        for id in ids {
            let row = ExecutionRepository::find_by_id(&mut *clock, id).await?
                .ok_or_else(|| anyhow::anyhow!("handoff hook ran before actual child commit"))?;
            anyhow::ensure!(row.status == ExecutionStatus::Requested && row.parent == Some(parent.id),
                "unexpected committed child lifecycle or parent");
        }
        broker.assert_empty().await; // The first application handoff is still held.
        if expire { wait_for_pair_consent_expiry(&mut clock, &consent).await?; }
        release_owner.take().ok_or_else(|| anyhow::anyhow!("release sender missing"))?
            .send(()).map_err(|_| anyhow::anyhow!("owner left first handoff unexpectedly"))?;
        let (first_fresh, later_expired) = tokio::time::timeout(Duration::from_secs(5), observed_recv).await
            .map_err(|_| anyhow::anyhow!("original held input evidence missing after application wait"))??;
        anyhow::ensure!(first_fresh && later_expired == expire,
            "only the independent later child's ORIGINAL input receipt should expire");
        let joined = tokio::time::timeout(Duration::from_secs(15), operation.as_mut().unwrap()).await;
        let outcome = match joined {
            Ok(joined) => { operation.take(); joined? }
            Err(_) => return Err(anyhow::anyhow!("actual owner did not finish after application handoff release")),
        };
        if expire {
            let error = outcome.err().ok_or_else(|| anyhow::anyhow!("expired later local attempt was accepted"))?;
            anyhow::ensure!(matches!(error.downcast_ref::<attune_common::Error>(),
                Some(attune_common::Error::PermissionDenied(_))),
                "denial must come from production's original-proof check, not observer instrumentation");
        } else {
            outcome?;
        }
        let first = broker.receive().await;
        anyhow::ensure!(first["payload"]["execution_id"] == serde_json::json!(ids[0]),
            "ordinary first child did not publish after its valid handoff");
        if !expire {
            let later = broker.receive().await;
            anyhow::ensure!(later["payload"]["execution_id"] == serde_json::json!(ids[1]),
                "positive control did not publish the genuine finite later child");
        }
        broker.assert_empty().await;
        let after = ExecutionRepository::find_by_parent(pool, parent.id).await?;
        let expected: std::collections::BTreeSet<_> = before.iter().map(|row| row.id)
            .chain(ids).collect();
        anyhow::ensure!(after.iter().map(|row| row.id).collect::<std::collections::BTreeSet<_>>() == expected,
            "postcommit denial must not mint replacement children or pretend to undo commit");
        for id in ids {
            let row = after.iter().find(|row| row.id == id).ok_or_else(|| anyhow::anyhow!("committed child disappeared"))?;
            anyhow::ensure!(row.status == ExecutionStatus::Requested, "test must not claim that committed children were rolled back");
        }
        let mut validation = pool.begin().await?;
        let original = Authority::find_approval(&mut validation, consent.id).await?
            .ok_or_else(|| anyhow::anyhow!("original finite input approval disappeared"))?;
        anyhow::ensure!(approval_snapshot(&original)? == consent.snapshot, "original consent was replaced or rewritten");
        let first_accepted = Accepted::revalidate_execution(&mut validation, ids[0], fixture::KEY).await?;
        first_accepted.ensure_fresh_on(&mut validation).await?;
        let first_inputs = Materialize::prepare_execution_input_receipts(&mut validation, ids[0], fixture::KEY).await?;
        for receipt in &first_inputs { receipt.ensure_fresh_on(&mut validation).await?; }
        if expire {
            let denied = Materialize::prepare_execution_input_receipts(&mut validation, ids[1], fixture::KEY).await;
            anyhow::ensure!(matches!(denied, Err(attune_common::Error::PermissionDenied(_))),
                "persisted later original input must still reject after its original expiry");
        } else {
            let later_inputs = Materialize::prepare_execution_input_receipts(&mut validation, ids[1], fixture::KEY).await?;
            for receipt in &later_inputs { receipt.ensure_fresh_on(&mut validation).await?; }
        }
        let root = Accepted::revalidate_execution(&mut validation, parent.id, fixture::KEY).await?;
        root.ensure_fresh_on(&mut validation).await?;
        let durable = WorkflowExecutionRepository::find_by_id(&mut *validation, workflow.id).await?
            .ok_or_else(|| anyhow::anyhow!("committed workflow disappeared"))?;
        anyhow::ensure!(durable.status == ExecutionStatus::Running, "both consumers must remain durable pending work");
        validation.rollback().await?;
        Ok::<(), anyhow::Error>(())
    }).catch_unwind().await;
    if let Some(release) = release_owner.take() {
        let _ = release.send(());
    }
    let mut join_failure = None;
    let mut owner_failure = None;
    if let Some(mut pending) = operation.take() {
        match tokio::time::timeout(Duration::from_secs(15), &mut pending).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => {
                owner_failure = Some(error.context(
                    "actual scheduler owner returned an error during application-handoff teardown",
                ));
            }
            Ok(Err(error)) => {
                join_failure = Some(format!("owner failed during awaited teardown: {error}"))
            }
            Err(_) => {
                pending.abort();
                let _ = pending.await;
                join_failure = Some("owner required abort-and-join during teardown".into());
            }
        }
    }
    let broker_cleanup = AssertUnwindSafe(async {
        Arc::try_unwrap(broker)
            .ok()
            .expect("all publisher borrows were joined")
            .close()
            .await;
    })
    .catch_unwind()
    .await;
    let database_cleanup = db.cleanup().await;
    let files_cleanup = files.close();
    // All owned teardown has been attempted/awaited before any final assertion.
    assert!(broker_cleanup.is_ok(), "owned broker cleanup failed");
    database_cleanup.expect("owned database cleanup failed");
    files_cleanup.expect("owned fixture file cleanup failed");
    // Keep the scenario failure primary and retain the real owner/JoinError
    // cause once, after every existing resource cleanup attempt.
    let owner_failure = owner_failure.or_else(|| join_failure.map(anyhow::Error::msg));
    match result {
        Ok(result) => {
            let result = match (result, owner_failure) {
                (Err(primary), Some(owner)) => Err(owner.context(primary)),
                (Ok(()), Some(owner)) => Err(owner),
                (result, None) => result,
            };
            result.expect("original later input application-handoff regression failed");
        }
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test]
#[ignore = "requires owned RabbitMQ and PostgreSQL"]
async fn original_finite_later_input_expires_during_first_application_handoff_wait() {
    first_application_wait_case(true).await;
}

#[tokio::test]
#[ignore = "requires owned RabbitMQ and PostgreSQL"]
async fn original_finite_later_input_two_publications_before_expiry_control() {
    first_application_wait_case(false).await;
}
