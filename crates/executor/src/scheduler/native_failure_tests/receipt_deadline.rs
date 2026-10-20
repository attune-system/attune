//! Original finite INPUT consent through the actual completion commit owner.
//! Postcommit application-handoff cases live in publication_wait.
//! No wire-deadline or default-suite passing claim.
use super::*;
use crate::scheduler::tests::CacheBrokerFixture;
use attune_common::repositories::{
    accepted_input::AcceptedInputRepository as Accepted,
    declared_input::DeclaredInputRepository as Declarations,
    declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
    execution_admission::ExecutionAdmissionRepository as Admissions,
    identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    },
    inherited_input::InheritedInputRepository as Inputs,
    runtime_authority::{
        ExternalApprovalRecord, RuntimeAuthorityRepository as Authority, RuntimeOwner,
    },
    Create,
};
use futures::FutureExt;
use std::{panic::AssertUnwindSafe, sync::Arc, time::Duration};
use tokio::sync::{oneshot, Mutex};

const KEY_REF: &str = "system.receipt_lifetime_original";
const FINITE_EXPRESSION: &str = "{{ keystore['system.receipt_lifetime_original'] }}";

// Resolve existing pinned metadata, not a compiled contract or runtime grant.
// The declaration belongs to the root graph; its VALUE destination is the child.
async fn finite_task_input_declaration(
    conn: &mut sqlx::PgConnection,
    parent: &Execution,
    task_path: &str,
) -> Result<attune_common::repositories::declared_input::DeclaredInputBinding> {
    let snapshot = parent
        .executable_snapshot
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("root pinned executable missing"))?;
    let graph = &snapshot
        .executable
        .workflow_definition
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("root pinned workflow missing"))?
        .definition;
    let task = graph
        .pointer(task_path)
        .ok_or_else(|| anyhow::anyhow!("finite task missing from original graph"))?;
    let consumer_ref = format!("{}.finite_consumer", snapshot.executable.action.pack_ref);
    let consumer = snapshot
        .pack_executables
        .get(&consumer_ref)
        .ok_or_else(|| anyhow::anyhow!("original pinned finite consumer missing"))?;
    anyhow::ensure!(
        task["action"].as_str() == Some(consumer.executable.action.r#ref.as_str()),
        "finite declaration must target the original pinned consumer"
    );
    anyhow::ensure!(
        task["input"]["finite"].as_str() == Some(FINITE_EXPRESSION),
        "finite Key selection must be authored at the exact task-input site"
    );
    let declarations = Declarations::bindings(conn, parent.id).await?;
    let finite = declarations
        .into_iter()
        .filter(|site| site.expression.contains(KEY_REF))
        .collect::<Vec<_>>();
    anyhow::ensure!(finite.len() == 1,
        "finite Key must occur at exactly one root task-input declaration, never config, variables or transitions");
    let declaration = finite
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("finite task-input declaration missing"))?;
    anyhow::ensure!(
        declaration.action == snapshot.executable.action.id
            && declaration.source_path == format!("/workflow{task_path}/input/finite")
            && declaration.expression == FINITE_EXPRESSION,
        "finite consent must identify the exact original root graph declaration"
    );
    Ok(declaration)
}

mod publication_wait;

// Test scheduling only: this Instant is never an input authority or proof.
fn original_consent_readiness_deadline(
    sample_started: tokio::time::Instant,
    remaining_ms: i64,
    original_limit: tokio::time::Instant,
) -> Result<tokio::time::Instant> {
    anyhow::ensure!(
        remaining_ms > 0 && original_limit > sample_started,
        "original finite input consent readiness budget exhausted before post-preparation barrier"
    );
    let remaining = Duration::from_millis(
        u64::try_from(remaining_ms)
            .map_err(|_| anyhow::anyhow!("invalid original consent readiness budget"))?,
    );
    let deadline = sample_started
        .checked_add(remaining)
        .ok_or_else(|| anyhow::anyhow!("original consent readiness budget overflow"))?;
    Ok(deadline.min(original_limit))
}

#[test]
fn original_consent_readiness_counts_query_time_against_the_original_window() {
    let sample = tokio::time::Instant::now();
    let response = sample + Duration::from_millis(500);
    let deadline =
        original_consent_readiness_deadline(sample, 2_000, sample + Duration::from_secs(20))
            .unwrap();
    assert_eq!(deadline, sample + Duration::from_millis(2_000));
    assert!(deadline < response + Duration::from_millis(2_000));
}

#[test]
fn original_consent_readiness_cannot_renew_the_initial_operator_limit() {
    let issued = tokio::time::Instant::now();
    let limit = issued + Duration::from_secs(20);
    let deadline =
        original_consent_readiness_deadline(issued + Duration::from_secs(18), 5_000, limit)
            .unwrap();
    assert_eq!(deadline, limit);
}

#[test]
fn original_consent_readiness_rejects_exhausted_or_nonpositive_windows() {
    let sample = tokio::time::Instant::now();
    let limit = sample + Duration::from_secs(20);
    assert!(original_consent_readiness_deadline(sample, 0, limit).is_err());
    assert!(original_consent_readiness_deadline(sample, -1, limit).is_err());
    assert!(original_consent_readiness_deadline(limit, 1, limit).is_err());
}

struct PreparedObservation {
    child: i64,
    receipt_ids: Vec<i64>,
    original_approval: i64,
}

struct InputCommitBarrier {
    original_approval: i64,
    ready: Mutex<Option<oneshot::Sender<PreparedObservation>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    held_receipt_expired: Mutex<Option<oneshot::Sender<bool>>>,
}

#[async_trait::async_trait]
impl WorkflowInputCommitObserver for InputCommitBarrier {
    async fn after_input_mutations(
        &self,
        conn: &mut PgConnection,
        publications: &WorkflowPublications,
    ) -> Result<()> {
        anyhow::ensure!(
            publications.len() == 1,
            "expected one actual prepared child"
        );
        let child_id = publications[0].execution_id;
        anyhow::ensure!(
            publications.owner_ids().collect::<Vec<_>>() == vec![child_id],
            "the actual owner registry must retain the prepared child"
        );
        let child = ExecutionRepository::find_by_id(&mut *conn, child_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("actual child disappeared in its own transaction"))?;
        anyhow::ensure!(
            child
                .workflow_task
                .as_ref()
                .is_some_and(|task| task.task_name == "consumer"),
            "barrier did not observe the authored finite-input child"
        );
        // Exercise the actual owner's transaction, not a synthetic raw BEGIN.
        // Nested repository callers must only release savepoints, never COMMIT
        // the outer transaction that captured and sealed this real child.
        use sqlx::TransactionManager;
        anyhow::ensure!(
            sqlx::postgres::PgTransactionManager::get_transaction_depth(conn) == 1,
            "actual workflow completion owner must retain one SQLx-managed root"
        );
        let original_transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        let mut nested = sqlx::Connection::begin(&mut *conn).await?;
        anyhow::ensure!(
            sqlx::postgres::PgTransactionManager::get_transaction_depth(&nested) == 2,
            "actual workflow completion owner must nest through a savepoint"
        );
        anyhow::ensure!(
            ExecutionRepository::find_by_id(&mut *nested, child_id)
                .await?
                .is_some(),
            "actual prepared child must remain visible inside the owner's savepoint"
        );
        nested.commit().await?;
        let retained_transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        anyhow::ensure!(
            original_transaction == retained_transaction
                && sqlx::postgres::PgTransactionManager::get_transaction_depth(conn) == 1,
            "releasing a nested savepoint must retain the actual original owner transaction"
        );
        // Canonical read-only reopening of an ACTUAL already initialized and
        // sealed child. The observer never creates acceptance/receipt evidence.
        let acceptance = Accepted::revalidate_execution(conn, child_id, fixture::KEY).await?;
        let receipts =
            Materialize::prepare_execution_input_receipts(conn, child_id, fixture::KEY).await?;
        anyhow::ensure!(
            !receipts.is_empty(),
            "the child must have actual original typed input receipts"
        );
        acceptance.ensure_fresh_on(conn).await?;
        for receipt in &receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        let fields = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn,
            ENTITY_EXECUTION_CONFIG,
            child_id,
        )
        .await?;
        anyhow::ensure!(
            fields.iter().any(|field| field.json_path == "/finite"),
            "the first child must have its real sensitive input delivery"
        );
        let restored = attune_common::secret_values::restore_secret_values(
            child
                .config
                .clone()
                .ok_or_else(|| anyhow::anyhow!("child config unavailable"))?,
            &fields,
            fixture::KEY,
        )?;
        anyhow::ensure!(
            restored["finite"] == serde_json::json!("original finite input"),
            "actual typed delivery has the wrong fixture value"
        );
        let admission = Admissions::require(conn, RuntimeOwner::Execution(child_id)).await?;
        anyhow::ensure!(
            child.permission_set_refs.is_empty(),
            "callee must have no SDK Key selection"
        );
        anyhow::ensure!(
            !admission
                .authority
                .approval_ids()
                .contains(&self.original_approval),
            "the input-only approval must not be substituted by selected SDK authority"
        );
        // Diagnostic reads only, in the SAME actual owner transaction. These
        // follow the original receipt graph to the canonical archived key read
        // and its exact independently issued approval. They grant no authority.
        let actual_approval_edges: i64 = sqlx::query_scalar(
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
        .bind(child_id)
        .bind(self.original_approval)
        .fetch_one(&mut *conn)
        .await?;
        anyhow::ensure!(
            actual_approval_edges > 0,
            "actual child receipt closure did not retain the original finite input approval"
        );
        let receipt_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM runtime_input_receipt WHERE execution=$1 ORDER BY id",
        )
        .bind(child_id)
        .fetch_all(&mut *conn)
        .await?;
        anyhow::ensure!(
            !receipt_ids.is_empty(),
            "actual child has no persisted receipts"
        );
        let ready = self
            .ready
            .lock()
            .await
            .take()
            .ok_or_else(|| anyhow::anyhow!("commit observer invoked more than once"))?;
        ready
            .send(PreparedObservation {
                child: child_id,
                receipt_ids,
                original_approval: self.original_approval,
            })
            .map_err(|_| anyhow::anyhow!("owning test stopped observing the prepared child"))?;
        let release = self
            .release
            .lock()
            .await
            .take()
            .ok_or_else(|| anyhow::anyhow!("commit barrier receiver unavailable"))?;
        tokio::time::timeout(Duration::from_secs(40), release)
            .await
            .map_err(|_| anyhow::anyhow!("owned commit barrier was not released"))?
            .map_err(|_| anyhow::anyhow!("owned commit barrier controller was dropped"))?;
        // Deliberately do NOT return the held proof's expiry error here. The
        // actual production collector/commit owner, after this observer returns,
        // MUST independently reject. Record only non-authoritative test evidence.
        let expired = receipts
            .iter()
            .any(|receipt| receipt.ensure_fresh().is_err());
        if let Some(observed) = self.held_receipt_expired.lock().await.take() {
            let _ = observed.send(expired);
        }
        Ok(())
    }
}

fn approval_snapshot(record: &ExternalApprovalRecord) -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "id":record.id, "pack":record.subject_pack,
        "issuer":record.issuer_identity, "issuer_login":record.issuer_login,
        "digest":record.approval_digest, "grants":serde_json::to_value(&record.grants.0)?,
        "expires":record.expires_at, "revoked":record.revoked_at,
    }))
}

async fn wait_for_original_approval_expiry(
    conn: &mut PgConnection,
    approval: i64,
    original_deadline: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut poll = tokio::time::interval(Duration::from_millis(25));
        loop {
            poll.tick().await;
            let expired: bool = sqlx::query_scalar(
                "SELECT expires_at=$2 AND expires_at<=clock_timestamp() \
                   FROM runtime_external_approval WHERE id=$1",
            )
            .bind(approval)
            .bind(original_deadline)
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
            "original finite input approval did not expire at its unchanged database deadline"
        )
    })?
}

async fn original_input_deadline_case(expire: bool) {
    // Strict owned URL/vhost fixture. Missing prerequisites fail; there is no
    // early return and no ambient developer-broker fallback.
    let broker = Arc::new(
        CacheBrokerFixture::connect(if expire {
            "receipt-input-expiry"
        } else {
            "receipt-input-control"
        })
        .await,
    );
    let setup = AssertUnwindSafe(fixture::original_executor_finite_input_child(
        canonical_task_graph,
    ))
    .catch_unwind()
    .await;
    let (db, files, parent, workflow, completed_source) = match setup {
        Ok(value) => value,
        Err(panic) => {
            let cleanup = AssertUnwindSafe(
                Arc::try_unwrap(broker)
                    .ok()
                    .expect("broker has one owner")
                    .close(),
            )
            .catch_unwind()
            .await;
            if cleanup.is_err() {
                eprintln!("owned broker cleanup also failed during fixture setup failure");
            }
            std::panic::resume_unwind(panic);
        }
    };
    // Assertions below return errors inside this future. Every path afterward
    // releases and joins the owner before closing its pool, queue and connection.
    let mut operation: Option<tokio::task::JoinHandle<Result<()>>> = None;
    let mut release_owner = None;
    let result = AssertUnwindSafe(async {
        use fixture::helpers::{IdentityFixture, KeyFixture};
        let pool = db.pool();
        anyhow::ensure!(parent.permission_set_refs.is_empty(), "root must have no SDK Key selection");
        // The external input issuer is independent of the original requester/
        // sponsor. Do not widen or rebind any recorded execution's human IAM.
        let issuer = IdentityFixture::new_unique("receipt_input_consent_issuer")
            .create(&db).await?.id;
        KeyFixture::new_system("receipt_lifetime_original", "original finite input").create(&db).await?;
        let exact_grants = serde_json::json!([{
            "resource":"keys", "actions":["read"], "constraints":{"refs":[KEY_REF]}
        }]);
        let set = PermissionSetRepository::create(pool, CreatePermissionSetInput {
            r#ref:"fixture.receipt_lifetime_input_issuer".into(), pack:None, pack_ref:None,
            label:None, description:None, grants:exact_grants.clone(),
        }).await?;
        PermissionAssignmentRepository::create(pool, CreatePermissionAssignmentInput {
            identity:issuer, permset:set.id,
        }).await?;
        // Reserve the observer connection BEFORE issuing finite consent. No
        // connection-pool wait is allowed to consume the fixture's consent window.
        let mut clock = pool.acquire().await?;
        let before_children = ExecutionRepository::find_by_parent(pool, parent.id).await?;
        let before_workflow = WorkflowExecutionRepository::find_by_id(pool, workflow.id).await?
            .ok_or_else(|| anyhow::anyhow!("original workflow missing"))?;
        let mut consent = pool.begin().await?;
        let root_admission = Admissions::require(&mut consent, RuntimeOwner::Execution(parent.id)).await?;
        let declaration = finite_task_input_declaration(&mut consent, &parent, "/tasks/1").await?;
        // This is the root graph author's invocation, not the callee's default owner.
        let binding = root_admission.authority.binding.clone();
        let revision = declaration.declaration_revision;
        let declaration_path = declaration.source_path;
        let expression = declaration.expression;
        Declarations::declaration_for_policy(
            &mut consent, root_admission.policy_revision, &binding,
            &revision, &declaration_path, &expression,
        ).await?;
        // Anchor the finite operator cap BEFORE the original deadline query,
        // never twenty new seconds from readiness or approval completion.
        let consent_window_started = tokio::time::Instant::now();
        let original_consent_limit = consent_window_started + Duration::from_secs(20);
        let deadline: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "SELECT clock_timestamp() + INTERVAL '20 seconds'"
        ).fetch_one(&mut *consent).await?;
        let approval = Inputs::approve_external_declaration(
            &mut consent, root_admission.policy_revision, &binding, &revision, &declaration_path,
            &expression, issuer, serde_json::from_value(exact_grants)?, Some(deadline),
        ).await?;
        let original_approval = Authority::find_approval(&mut consent, approval).await?
            .ok_or_else(|| anyhow::anyhow!("original approval missing after issue"))?;
        anyhow::ensure!(Declarations::approval_is_input_only(&mut consent, approval).await?,
            "the independent finite approval must be input-only");
        anyhow::ensure!(original_approval.expires_at == Some(deadline), "finite deadline was not retained");
        let original_snapshot = approval_snapshot(&original_approval)?;
        anyhow::ensure!(!root_admission.authority.approval_ids().contains(&approval),
            "input consent must not become root SDK selection");
        let root_acceptance = Accepted::revalidate_execution(&mut consent, parent.id, fixture::KEY).await?;
        anyhow::ensure!(root_acceptance.approval_deadline().is_none(),
            "this regression must not substitute an unrelated finite policy fence");
        consent.commit().await?;
        // Charge query round-trip time to the original remaining consent.
        // FLOOR never rounds a sub-millisecond remainder into extra grace.
        let budget_sample_started = tokio::time::Instant::now();
        let remaining_query = sqlx::query_scalar::<_, i64>(
            "SELECT GREATEST(0::numeric, FLOOR(EXTRACT(EPOCH FROM \
             ($1::timestamptz - clock_timestamp())) * 1000))::bigint",
        ).bind(deadline).fetch_one(&mut *clock);
        let remaining_ms = match tokio::time::timeout_at(original_consent_limit, remaining_query).await {
            Ok(remaining) => remaining?,
            Err(_) => {
                clock.close_on_drop();
                return Err(anyhow::anyhow!("original finite input consent readiness budget exhausted during measurement"));
            }
        };
        let readiness_deadline = original_consent_readiness_deadline(
            budget_sample_started, remaining_ms, original_consent_limit,
        )?;
        anyhow::ensure!(tokio::time::Instant::now() < readiness_deadline,
            "original finite input consent readiness budget exhausted before owner preparation");
        let (ready_send, ready_recv) = oneshot::channel();
        let (release_send, release_recv) = oneshot::channel();
        let (expired_send, expired_recv) = oneshot::channel();
        release_owner = Some(release_send);
        let barrier = InputCommitBarrier {
            original_approval: approval, ready:Mutex::new(Some(ready_send)),
            release:Mutex::new(Some(release_recv)), held_receipt_expired:Mutex::new(Some(expired_send)),
        };
        let owner_pool = pool.clone();
        let owner_broker = Arc::clone(&broker);
        let owner_files = files.path().to_string_lossy().into_owned();
        operation = Some(tokio::spawn(async move {
            let transport: Arc<dyn attune_common::artifact_transport::ArtifactFileTransport> =
                Arc::new(attune_common::artifact_transport::VolumeTransport::new(&owner_files));
            ExecutionScheduler::advance_workflow_owned(
                &owner_pool, &owner_broker.publisher, &AtomicUsize::new(0), &owner_files,
                &transport, 1024 * 1024, 1000, Some(fixture::KEY), &completed_source,
                &SchedulerMetadataCaches::new(), Some(&barrier),
            ).await
        }));
        let prepared = match tokio::time::timeout_at(readiness_deadline, ready_recv).await
            .map_err(|_| anyhow::anyhow!("original finite input consent readiness budget exhausted before post-preparation barrier"))? {
            Ok(prepared) => prepared,
            Err(_) => {
                let pending = operation.as_mut()
                    .ok_or_else(|| anyhow::anyhow!("actual scheduler owner missing after readiness channel closed"))?;
                let joined = tokio::time::timeout(Duration::from_secs(15), pending).await;
                match joined {
                    Ok(joined) => {
                        operation.take(); // Join completed; never poll this handle again.
                        return Err(match joined {
                            Ok(Err(error)) => error.context("actual scheduler owner failed before the post-preparation barrier"),
                            Ok(Ok(_)) => anyhow::anyhow!("actual scheduler owner unexpectedly completed successfully before the post-preparation barrier"),
                            Err(error) => anyhow::Error::new(error)
                                .context("actual scheduler owner task failed before the post-preparation barrier"),
                        });
                    }
                    Err(_) => return Err(anyhow::anyhow!("actual scheduler owner did not finish after readiness channel closed")),
                }
            }
        };
        // timeout_at may poll a ready value after delayed scheduling. Never
        // accept a barrier observation outside the conservative original budget.
        anyhow::ensure!(tokio::time::Instant::now() < readiness_deadline,
            "original finite input consent readiness budget exhausted before barrier observation");
        anyhow::ensure!(prepared.original_approval == approval, "wrong original approval was observed");
        // The only other connection reads the ALREADY COMMITTED approval clock.
        // It never attempts to read the owner's uncommitted child or receipts.
        if expire { wait_for_original_approval_expiry(&mut clock, approval, deadline).await?; }
        release_owner.take().ok_or_else(|| anyhow::anyhow!("release sender missing"))?
            .send(()).map_err(|_| anyhow::anyhow!("actual owner left the barrier unexpectedly"))?;
        let held_expired = tokio::time::timeout(Duration::from_secs(5), expired_recv).await
            .map_err(|_| anyhow::anyhow!("held original receipt expiry observation missing"))??;
        anyhow::ensure!(held_expired == expire,
            "the ACTUAL prepared child's original receipt did not exhibit the expected finite deadline");
        let joined = tokio::time::timeout(Duration::from_secs(15), operation.as_mut().unwrap()).await;
        let outcome = match joined {
            Ok(joined) => {
                operation.take(); // Never poll a completed JoinHandle twice.
                joined?
            }
            Err(_) => return Err(anyhow::anyhow!("actual scheduler owner did not finish after barrier release")),
        };
        let mut validation = pool.begin().await?;
        if expire {
            let error = outcome.err().ok_or_else(|| anyhow::anyhow!("actual owner committed expired original input"))?;
            anyhow::ensure!(matches!(error.downcast_ref::<attune_common::Error>(),
                Some(attune_common::Error::PermissionDenied(_))),
                "actual owner did not reject expired original input with an authorization failure");
            anyhow::ensure!(ExecutionRepository::find_by_id(pool, prepared.child).await?.is_none(),
                "expired child's business row was committed");
            anyhow::ensure!(Admissions::find(&mut validation, RuntimeOwner::Execution(prepared.child)).await?.is_none(),
                "expired child's admission survived rollback");
            let retained_receipts: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM runtime_input_receipt WHERE id=ANY($1)"
            ).bind(&prepared.receipt_ids).fetch_one(&mut *clock).await?;
            anyhow::ensure!(retained_receipts == 0, "uncommitted child receipts survived actual owner rollback");
            let after_children = ExecutionRepository::find_by_parent(pool, parent.id).await?;
            let after_workflow = WorkflowExecutionRepository::find_by_id(pool, workflow.id).await?
                .ok_or_else(|| anyhow::anyhow!("original workflow disappeared"))?;
            anyhow::ensure!(serde_json::to_value(&before_children)? == serde_json::to_value(&after_children)?,
                "business children changed despite rejection/rollback");
            anyhow::ensure!(serde_json::to_value(&before_workflow)? == serde_json::to_value(&after_workflow)?,
                "workflow business state changed despite rejection/rollback");
            broker.assert_empty().await;
        } else {
            outcome?;
            let delivered = broker.receive().await;
            anyhow::ensure!(delivered["payload"]["execution_id"] == serde_json::json!(prepared.child),
                "positive control did not publish the actual prepared child");
            let committed = ExecutionRepository::find_by_id(pool, prepared.child).await?
                .ok_or_else(|| anyhow::anyhow!("positive-control child did not commit"))?;
            anyhow::ensure!(committed.status == ExecutionStatus::Requested, "positive control changed child status");
            let accepted = Accepted::revalidate_execution(&mut validation, prepared.child, fixture::KEY).await?;
            accepted.ensure_fresh_on(&mut validation).await?;
            broker.assert_empty().await;
        }
        let after_approval = Authority::find_approval(&mut validation, approval).await?
            .ok_or_else(|| anyhow::anyhow!("original finite approval was removed"))?;
        anyhow::ensure!(approval_snapshot(&after_approval)? == original_snapshot,
            "the original approval ID/deadline/grants/digest were rewritten");
        // The original root remains valid after expiry: the rejection is the
        // independent input receipt, not current human IAM or root policy expiry.
        let root_after = Accepted::revalidate_execution(&mut validation, parent.id, fixture::KEY).await?;
        root_after.ensure_fresh_on(&mut validation).await?;
        validation.rollback().await?;
        Ok::<(), anyhow::Error>(())
    }).catch_unwind().await;
    // Teardown is outside every scenario assertion and panic path. Unblock first,
    // then await or abort-and-join the operation before closing owned resources.
    if let Some(release) = release_owner.take() {
        let _ = release.send(());
    }
    let mut join_failure = None;
    let mut owner_failure = None;
    if let Some(mut pending) = operation.take() {
        match tokio::time::timeout(Duration::from_secs(15), &mut pending).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => {
                owner_failure =
                    Some(error.context("actual scheduler owner returned an error during teardown"));
            }
            Ok(Err(error)) => {
                join_failure = Some(format!("owner task failed during teardown: {error}"))
            }
            Err(_) => {
                pending.abort();
                let _ = pending.await;
                join_failure = Some("owner task required abort-and-join during teardown".into());
            }
        }
    }
    let broker_result = AssertUnwindSafe(async {
        Arc::try_unwrap(broker)
            .ok()
            .expect("all owner publisher borrows were joined")
            .close()
            .await;
    })
    .catch_unwind()
    .await;
    let database_result = db.cleanup().await;
    let files_result = files.close();
    // Assert only AFTER all three teardown operations have been attempted/awaited.
    assert!(
        broker_result.is_ok(),
        "owned queue/channel/connection teardown failed"
    );
    database_result.expect("owned database cleanup failed");
    files_result.expect("owned fixture files cleanup failed");
    // Keep the scenario failure primary and retain a genuine owner/JoinError
    // as its cause. No separate diagnostic dump is needed after cleanup.
    let owner_failure = owner_failure.or_else(|| join_failure.map(anyhow::Error::msg));
    match result {
        Ok(result) => {
            let result = match (result, owner_failure) {
                (Err(primary), Some(owner)) => Err(owner.context(primary)),
                (Ok(()), Some(owner)) => Err(owner),
                (result, None) => result,
            };
            result.expect("finite original input commit boundary regression failed");
        }
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test]
#[ignore = "requires owned RabbitMQ and PostgreSQL"]
async fn original_finite_input_expires_at_actual_completion_commit_and_publishes_nothing() {
    original_input_deadline_case(true).await;
}

#[tokio::test]
#[ignore = "requires owned RabbitMQ and PostgreSQL"]
async fn original_finite_input_actual_completion_commits_and_publishes_before_expiry_control() {
    original_input_deadline_case(false).await;
}
