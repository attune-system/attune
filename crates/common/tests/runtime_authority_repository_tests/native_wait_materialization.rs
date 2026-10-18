#[path = "native_wait_materialization/output_release_cases.rs"]
mod output_release_cases;
#[path = "native_wait_materialization/successor_cases.rs"]
mod successor_cases;
use super::*;
#[path = "native_wait_materialization/cache_cases.rs"]
mod cache_cases;
#[path = "native_wait_materialization/initial_cases.rs"]
mod initial_cases;
use attune_common::{
    models::WorkflowTaskWaitTarget as Target,
    repositories::{
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        native_wait_materialization::{
            NativeWaitContinuationRepository as Continue,
            NativeWaitMaterializationRepository as Delivery, PreparedNativeWait,
            PreparedNativeWaitContinuation,
        },
        workflow::WorkflowExecutionRepository as Workflows,
        workflow_task_wait::WorkflowTaskWaitRepository as Waits,
        FindById,
    },
};
pub(super) use cache_cases::{check as check_cache, seed as seed_cache};
pub(super) use initial_cases::{initial_lifecycle, InitialCase};

#[derive(Clone, Copy)]
pub(super) enum Case {
    Delivery(usize),
    Terminal(usize, &'static str, &'static str),
    Overwrite(usize),
    Initial(usize, InitialCase),
    Dependent(usize),
    DependentChild(usize),
    DependentExpiry(usize),
    Cache(usize, bool),
    CacheExpiry(usize),
}
impl Case {
    pub(super) fn index(&self) -> usize {
        match self {
            Self::Delivery(index)
            | Self::Terminal(index, ..)
            | Self::Overwrite(index)
            | Self::Dependent(index)
            | Self::DependentChild(index)
            | Self::DependentExpiry(index)
            | Self::Cache(index, _)
            | Self::CacheExpiry(index)
            | Self::Initial(index, _) => *index,
        }
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "The test consumes the same inline owned delivery or terminal proof as production"
)]
enum Witness {
    Delivery(PreparedNativeWait),
    Terminal(PreparedNativeWaitContinuation),
}
impl Witness {
    async fn prepare(
        conn: &mut sqlx::PgConnection,
        wait: &attune_common::models::WorkflowTaskWait,
        key: &str,
        terminal: bool,
    ) -> Self {
        if terminal {
            let plan = Continue::prepare(conn, wait, Some(key))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(plan.wait(), wait.id);
            assert_eq!(plan.workflow(), wait.workflow_execution);
            Self::Terminal(plan)
        } else {
            Self::Delivery(
                Delivery::prepare(conn, wait, Some(key))
                    .await
                    .unwrap()
                    .unwrap(),
            )
        }
    }
    async fn apply(
        &mut self,
        conn: &mut sqlx::PgConnection,
        key: &str,
    ) -> attune_common::Result<Vec<attune_common::models::Execution>> {
        match self {
            Self::Delivery(plan) => plan
                .apply(conn, key)
                .await
                .map(|child| child.into_iter().collect()),
            Self::Terminal(plan) => plan.apply(conn, key).await,
        }
    }
    async fn fresh_on(&self, conn: &mut sqlx::PgConnection) {
        match self {
            Self::Delivery(plan) => plan.ensure_fresh_on(conn).await.unwrap(),
            Self::Terminal(plan) => plan.ensure_fresh_on(conn).await.unwrap(),
        }
    }
    fn fresh(&self) {
        match self {
            Self::Delivery(plan) => plan.ensure_fresh().unwrap(),
            Self::Terminal(plan) => plan.ensure_fresh().unwrap(),
        }
    }
}

pub(super) fn recovery_program() -> serde_json::Value {
    json!([
        {"when":"{{ succeeded() }}","do":["native_recovery"],"publish":{"native_success":"{{ keystore['system.native_wait_never_selected'] }}"}},
        {"when":"{{ failed() }}","do":["native_recovery"],"publish":{"native_failed":"handled"}},
        {"when":"{{ timed_out() }}","do":["native_recovery"],"publish":{"native_timeout":"handled"}},
        {"when":"{{ cancelled() }}","do":["native_recovery"],"publish":{"native_cancelled":"handled"}}
    ])
}

pub(super) fn overwrite_program() -> serde_json::Value {
    let mut program = recovery_program();
    program[1]["publish"] =
        json!({"native_failed":{"obsolete":"first", "nested":{"token":"first"}}});
    program.as_array_mut().unwrap().insert(
        2,
        json!({"when":"{{ failed() }}","do":["native_recovery"],
        "publish":{"native_failed":{"nested":{"token":"handled"},"history":"last"}}}),
    );
    program
}

pub(super) fn dependent_program() -> serde_json::Value {
    let mut program = recovery_program();
    program[1]["publish"] = json!({"a":"{{ execution.observed_execution.result.token }}"});
    program.as_array_mut().unwrap().insert(2,json!({
        "when":"{{ failed() and a == 'execution-original' }}","do":["native_recovery"],"publish":{"b":"{{ a }}"}
    }));
    program
}

pub(super) fn ordered_array_program() -> serde_json::Value {
    let mut program = dependent_program();
    program[1]["publish"] = json!([
        {"a":"{{ execution.observed_execution.result.token }}"},
        {"b":"{{ a }}"},
        {"a":"replacement must not overwrite captured b"},
        {"a":"{{ b }}"}
    ]);
    program[2]["publish"] = json!([{"b":"{{ a }}"}]);
    program
}

async fn protect(conn: &mut sqlx::PgConnection) {
    attune_common::repositories::cache::CacheEntryRepository::protect_transaction(
        conn,
        attune_common::repositories::cache::CacheTransactionMode::PinMutation,
    )
    .await
    .unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(conn)
        .await
        .unwrap();
}
async fn status(conn: &mut sqlx::PgConnection, target: Target, status: &str) {
    let (sql, id) = match target {
        Target::Execution(id) => (
            "UPDATE execution SET status=$2::TEXT::execution_status_enum WHERE id=$1",
            id,
        ),
        Target::Inquiry(id) => (
            "UPDATE inquiry SET status=$2::TEXT::inquiry_status_enum WHERE id=$1",
            id,
        ),
        Target::WorkQueueItem(id) => (
            "UPDATE work_queue_item SET status=$2::TEXT::work_queue_item_status_enum WHERE id=$1",
            id,
        ),
    };
    assert_eq!(
        sqlx::query(sql)
            .bind(id)
            .bind(status)
            .execute(conn)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
}
async fn delete(conn: &mut sqlx::PgConnection, target: Target) {
    match target {
        Target::Execution(id) => ExecutionRepository::delete(conn, id).await.unwrap(),
        Target::Inquiry(id) => {
            attune_common::repositories::inquiry::InquiryRepository::delete(conn, id)
                .await
                .unwrap()
        }
        Target::WorkQueueItem(id) => {
            attune_common::repositories::work_queue::WorkQueueItemRepository::delete(conn, id)
                .await
                .unwrap()
        }
    };
}

async fn reclaim(pool: &sqlx::PgPool, target: Target) {
    use attune_common::repositories::file_evidence_retention::FileEvidenceRetentionRepository as Retention;
    // A source-phase shared entry cannot later be upgraded for collection.
    let mut late = pool.begin().await.unwrap();
    protect(&mut late).await;
    let error = Retention::lock_reclamation(&mut late)
        .await
        .expect_err("late reclamation must not upgrade an existing source/target transaction");
    assert!(error
        .to_string()
        .contains("Reclamation must enter exclusive graph gate"));
    late.rollback().await.unwrap();

    let mut gc = pool.begin().await.unwrap();
    Retention::lock_reclamation(&mut gc).await.unwrap();
    attune_common::repositories::cache::CacheEntryRepository::protect_transaction(
        &mut gc,
        attune_common::repositories::cache::CacheTransactionMode::PinMutation,
    )
    .await
    .unwrap();
    let (kind, id) = match target {
        Target::Execution(id) => ("execution", id),
        Target::Inquiry(id) => ("inquiry", id),
        Target::WorkQueueItem(id) => ("work_queue_item", id),
    };
    assert!(
        Retention::eligible(&mut gc, kind, id).await.unwrap(),
        "the archived source must be eligible under the owning retention graph"
    );
    delete(&mut gc, target).await;
    let absent = match target {
        Target::Execution(id) => ExecutionRepository::find_by_id(&mut *gc, id)
            .await
            .unwrap()
            .is_none(),
        Target::Inquiry(id) => {
            attune_common::repositories::inquiry::InquiryRepository::find_by_id(&mut *gc, id)
                .await
                .unwrap()
                .is_none()
        }
        Target::WorkQueueItem(id) => {
            attune_common::repositories::work_queue::WorkQueueItemRepository::find_by_id(
                &mut *gc, id,
            )
            .await
            .unwrap()
            .is_none()
        }
    };
    assert!(
        absent,
        "the owning exclusive collector must really remove the raw source, not skip collection"
    );
    gc.commit().await.unwrap();
}

/// A fresh owned fixture for each case. First publication, GC and replay use
/// separate committed transactions, never source authorization behind old locks.
pub(super) async fn lifecycle(
    pool: &sqlx::PgPool,
    workflow: Id,
    parent: Id,
    task: &str,
    target: Target,
    key: &str,
    case: Case,
) {
    let terminal = !matches!(case, Case::Delivery(..));
    let overwrite = matches!(case, Case::Overwrite(..));
    let dependent = matches!(
        case,
        Case::Dependent(..) | Case::DependentExpiry(..) | Case::DependentChild(_)
    );
    let dependent_child = matches!(case, Case::DependentChild(_));
    let source_status = match &case {
        Case::Terminal(_, source_status, _) => Some(*source_status),
        Case::Overwrite(_) => Some("failed"),
        Case::Dependent(_) | Case::DependentExpiry(_) | Case::DependentChild(_) => Some("failed"),
        Case::Delivery(_) | Case::Initial(..) | Case::Cache(..) | Case::CacheExpiry(_) => None,
    };
    if let Some(source_status) = source_status {
        let mut tx = pool.begin().await.unwrap();
        status(&mut tx, target, source_status).await;
        tx.commit().await.unwrap();
    }
    assert!(
        attune_common::repositories::key::KeyRepository::find_by_ref(
            pool,
            "system.native_wait_never_selected"
        )
        .await
        .unwrap()
        .is_none()
    );
    let mut tx = pool.begin().await.unwrap();
    if !terminal {
        // The missing-parent-acceptance negative case deletes retained input
        // evidence. Enter its exclusive gate before cache/source protection.
        attune_common::repositories::file_evidence_retention::FileEvidenceRetentionRepository::lock_reclamation(&mut tx)
            .await.unwrap();
    }
    protect(&mut tx).await;
    let wait = Waits::find_by_workflow_task(&mut *tx, workflow, task)
        .await
        .unwrap()
        .unwrap();
    if !terminal {
        missing_parent_acceptance(&mut tx, &wait, parent, key).await;
    }
    let mut plan = Witness::prepare(&mut tx, &wait, key, terminal).await;
    if dependent {
        let state = Workflows::find_by_id(&mut *tx, workflow)
            .await
            .unwrap()
            .unwrap();
        assert!(state.variables.get("a").is_none() && state.variables.get("b").is_none());
        let pending:i64=sqlx::query_scalar("SELECT count(*) FROM runtime_input_receipt_staged s JOIN runtime_input_receipt r ON r.id=s.receipt WHERE r.execution=$1 AND r.entity_kind='workflow_variables' AND r.destination_path IN ('/a','/b') AND s.publication_transaction IS NULL")
            .bind(parent).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(pending, 2);
        let released: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_native_wait_release WHERE wait=$1)",
        )
        .bind(wait.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert!(!released);
    }
    sqlx::query("SAVEPOINT native_wait_target_change")
        .execute(&mut *tx)
        .await
        .unwrap();
    if matches!(target, Target::Execution(_)) {
        sqlx::query("UPDATE workflow_task_wait SET target_execution=$2 WHERE id=$1")
            .bind(wait.id)
            .bind(parent)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(plan.apply(&mut tx, key).await.is_err());
        sqlx::query("ROLLBACK TO SAVEPOINT native_wait_target_change")
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE execution SET status='cancelled' WHERE id=$1")
        .bind(parent)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(plan.apply(&mut tx, key).await.is_err());
    sqlx::query("ROLLBACK TO SAVEPOINT native_wait_target_change")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("RELEASE SAVEPOINT native_wait_target_change")
        .execute(&mut *tx)
        .await
        .unwrap();
    let children = plan.apply(&mut tx, key).await.unwrap();
    assert_eq!(children.len(), 1);
    let child = &children[0];
    original_child_acceptance(&mut tx, parent, child).await;
    if dependent {
        let finalized:i64=sqlx::query_scalar("SELECT count(*) FROM runtime_input_receipt_staged s JOIN runtime_input_receipt r ON r.id=s.receipt WHERE r.execution=$1 AND r.entity_kind='workflow_variables' AND r.destination_path IN ('/a','/b') AND s.publication_transaction=pg_current_xact_id()::TEXT")
            .bind(parent).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(finalized, 2);
    }
    assert_eq!(child.parent, Some(parent));
    assert_eq!(child.status, ExecutionStatus::Requested);
    let expected = if dependent_child {
        "execution-original"
    } else if terminal {
        "recovered"
    } else {
        match target {
            Target::Execution(_) => "execution-original",
            Target::Inquiry(_) => "inquiry-original",
            Target::WorkQueueItem(_) => "queue-original",
        }
    };
    let fields = Secrets::find_stored_by_entity(&mut *tx, "execution_config", child.id)
        .await
        .unwrap();
    let actual = attune_common::secret_values::restore_secret_values(
        child.config.clone().unwrap(),
        &fields,
        key,
    )
    .unwrap();
    assert_eq!(actual["token"], expected);
    if dependent_child {
        let receipts=attune_common::repositories::declared_input_materialization::DeclaredInputMaterializationRepository::prepare_execution_input_receipts(&mut tx,child.id,key).await.unwrap();
        assert!(receipts.iter().flat_map(|receipt|receipt.source_receipts()).any(|source|source.site().execution==parent
            && source.site().kind==attune_common::repositories::inherited_input::InputEntityKind::WorkflowVariables
            && source.site().purpose==attune_common::repositories::inherited_input::InputSitePurpose::Value
            && source.site().path=="/b"),"the actual child VALUE must retain the genuine issued b stage");
    }
    if terminal && !dependent_child {
        assert_eq!(actual, json!({"token":"recovered"}));
        assert_eq!(child.config.as_ref(),Some(&attune_common::secret_values::redaction_marker()),
            "the authored all-literal params object is one controlled value, encrypted at the config root");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].json_path, "");
        assert_eq!(fields[0].source_kind, "provenance");
        assert_eq!(
            fields[0].encryption_key_hash.as_deref(),
            Some(attune_common::crypto::hash_encryption_key(key).as_str())
        );
        let same_cipher:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_accepted_input a CROSS JOIN LATERAL jsonb_array_elements(a.body->'fields') f WHERE a.execution=$1 AND f->>'json_path'='' AND f->'encrypted_value'=$2 AND f->>'encryption_key_hash'=$3)")
            .bind(child.id).bind(&fields[0].encrypted_value).bind(&fields[0].encryption_key_hash).fetch_one(&mut *tx).await.unwrap();
        assert!(
            same_cipher,
            "the first seal must archive exactly the original controlled root ciphertext"
        );
    } else {
        assert_eq!(
            fields
                .iter()
                .map(|field| field.json_path.as_str())
                .collect::<Vec<_>>(),
            vec!["/token"]
        );
        assert_eq!(
            child.config.as_ref().unwrap()["token"],
            attune_common::secret_values::redaction_marker()
        );
    }
    let published = match &case {
        Case::Terminal(_, _, published) => Some(*published),
        Case::Overwrite(_) => Some("native_failed"),
        Case::Dependent(_) | Case::DependentExpiry(_) | Case::DependentChild(_) => Some("b"),
        Case::Delivery(_) | Case::Initial(..) | Case::Cache(..) | Case::CacheExpiry(_) => None,
    };
    if let Some(published) = published {
        let metadata = child.workflow_task.as_ref().unwrap();
        assert_eq!(metadata.task_name, "native_recovery");
        assert_eq!(metadata.triggered_by.as_deref(), Some(task));
        assert!(
            ExecutionRepository::find_by_workflow_task(&mut *tx, workflow, task, None)
                .await
                .unwrap()
                .is_none()
        );
        let state = Workflows::find_by_id(&mut *tx, workflow)
            .await
            .unwrap()
            .unwrap();
        let fields = Secrets::find_stored_by_entity(&mut *tx, "workflow_variables", parent)
            .await
            .unwrap();
        let variables =
            attune_common::secret_values::restore_secret_values(state.variables, &fields, key)
                .unwrap();
        let expected = if dependent {
            json!("execution-original")
        } else if overwrite {
            json!({"nested":{"token":"handled"},"history":"last"})
        } else {
            json!("handled")
        };
        assert_eq!(variables[published], expected);
        if dependent {
            assert_eq!(variables["a"], "execution-original");
        }
        if overwrite {
            assert!(
                fields
                    .iter()
                    .all(|field| !field.json_path.starts_with("/native_failed/obsolete")),
                "whole-variable overwrite removes the previous subtree ciphertext"
            );
        }
        assert!(variables.get("native_success").is_none());
        assert!(state.completed_tasks.iter().any(|name| name == task));
        assert!(!state.failed_tasks.iter().any(|name| name == task));
    }
    plan.fresh_on(&mut tx).await;
    assert!(
        plan.apply(&mut tx, key).await.is_err(),
        "a source plan is one-shot"
    );
    tx.commit().await.unwrap();
    plan.fresh();
    if terminal {
        let mut inspect = pool.begin().await.unwrap();
        protect(&mut inspect).await;
        terminal_controls(
            &mut inspect,
            wait.id,
            parent,
            child,
            task,
            key,
            overwrite,
            dependent,
        )
        .await;
        inspect.commit().await.unwrap();
        plan.fresh();
    }
    reclaim(pool, target).await;
    plan.fresh();
    let mut tx = pool.begin().await.unwrap();
    protect(&mut tx).await;
    let released = Waits::find_by_workflow_task(&mut *tx, workflow, task)
        .await
        .unwrap()
        .unwrap();
    let mut replay = Witness::prepare(&mut tx, &released, key, terminal).await;
    let replayed = replay.apply(&mut tx, key).await.unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].id, child.id);
    assert_eq!(replayed[0].config, child.config);
    replay.fresh_on(&mut tx).await;
    tx.commit().await.unwrap();
    replay.fresh();
    plan.fresh();
}

async fn missing_parent_acceptance(
    conn: &mut sqlx::PgConnection,
    wait: &attune_common::models::WorkflowTaskWait,
    parent: Id,
    key: &str,
) {
    sqlx::query("SAVEPOINT native_wait_missing_parent_acceptance")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query("DELETE FROM runtime_accepted_input WHERE execution=$1")
            .bind(parent)
            .execute(&mut *conn)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
    assert!(attune_common::repositories::native_wait_materialization::NativeWaitMaterializationRepository::prepare(conn, wait, Some(key))
        .await.is_err(), "a missing original parent acceptance must fail, never be repaired or skipped");
    let accepted: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_accepted_input WHERE execution=$1)",
    )
    .bind(parent)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert!(!accepted, "preparation must not reseal the parent");
    sqlx::query("ROLLBACK TO SAVEPOINT native_wait_missing_parent_acceptance")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("RELEASE SAVEPOINT native_wait_missing_parent_acceptance")
        .execute(&mut *conn)
        .await
        .unwrap();
}

async fn original_child_acceptance(
    conn: &mut sqlx::PgConnection,
    parent: Id,
    child: &attune_common::models::Execution,
) {
    let (config, same_contributors, original_transaction): (serde_json::Value, bool, bool) = sqlx::query_as(
        "SELECT child.body->'config',child.body->'contributors'=parent.body->'contributors',epoch.transaction_id=pg_current_xact_id()::TEXT FROM runtime_accepted_input child JOIN runtime_accepted_input_epoch epoch ON epoch.admission=child.admission JOIN runtime_accepted_input parent ON parent.execution=$2 WHERE child.execution=$1",
    ).bind(child.id).bind(parent).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(Some(config), child.config);
    assert!(
        same_contributors,
        "the child must copy every original parent contributor without union or recapture"
    );
    assert!(
        original_transaction,
        "the real child must first-seal in its creation/admission transaction"
    );
}

#[expect(
    clippy::too_many_arguments,
    reason = "Test independent wait, parent, authority, deadline, and dependency controls"
)]
async fn terminal_controls(
    conn: &mut sqlx::PgConnection,
    wait: Id,
    parent: Id,
    child: &attune_common::models::Execution,
    task: &str,
    key: &str,
    overwrite: bool,
    dependent: bool,
) {
    use attune_common::repositories::{
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        inherited_input::{InheritedInputRepository as Inputs, InputEntityKind, InputSitePurpose},
    };
    let snapshot: Id =
        sqlx::query_scalar("SELECT snapshot FROM runtime_native_wait_release WHERE wait=$1")
            .bind(wait)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let parent_row = ExecutionRepository::find_by_id(&mut *conn, parent)
        .await
        .unwrap()
        .unwrap();
    let graph = &parent_row
        .executable_snapshot
        .as_ref()
        .unwrap()
        .executable
        .workflow_definition
        .as_ref()
        .unwrap()
        .definition;
    let action = graph["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == task)
        .unwrap()["action"]
        .as_str()
        .unwrap();
    let path = Materialize::workflow_task_site_path(conn, parent, task, action)
        .await
        .unwrap();
    let receipts = Materialize::prepare_execution_input_receipts(conn, child.id, key)
        .await
        .unwrap();
    let mut controls = BTreeMap::new();
    for receipt in receipts {
        for descriptor in receipt.source_receipts() {
            let site = descriptor.site();
            if site.execution == parent
                && site.kind == InputEntityKind::WorkflowVariables
                && site.purpose == InputSitePurpose::Condition
                && site.path.starts_with(&format!("{path}/next/"))
            {
                let original = Inputs::prepare_receipt_source(conn, descriptor, key)
                    .await
                    .unwrap();
                assert!(original
                    .native_source_metadata()
                    .iter()
                    .any(|source| source.snapshot() == snapshot));
                let control = Inputs::prepare_source(conn, parent, site, &descriptor.origin(), key)
                    .await
                    .unwrap();
                let mut wrong_purpose = site.clone();
                wrong_purpose.purpose = InputSitePurpose::Value;
                assert!(Inputs::prepare_source(conn,parent,&wrong_purpose,&descriptor.origin(),key).await.is_err(),
                    "an original Condition cannot be relabeled as a Boolean VALUE in the real child lineage");
                controls.insert(site.path.clone(), control.value().as_bool().unwrap());
            }
        }
    }
    let expected_controls = if overwrite || dependent { 5 } else { 4 };
    assert_eq!(controls.len(), expected_controls, "the real child retains every original true and false Condition, not fabricated Boolean VALUE proofs");
    assert_eq!(
        controls.values().filter(|value| **value).count(),
        if overwrite || dependent { 2 } else { 1 }
    );
    for index in 0..expected_controls {
        assert!(controls.contains_key(&format!("{path}/next/{index}/when")));
    }
    if dependent {
        let fields = Secrets::find_stored_by_entity(&mut *conn, "workflow_variables", parent)
            .await
            .unwrap();
        assert_eq!(
            fields
                .iter()
                .map(|field| field.json_path.as_str())
                .collect::<std::collections::BTreeSet<_>>(),
            ["/a", "/b"].into_iter().collect()
        );
        let field = fields.iter().find(|field| field.json_path == "/b").unwrap();
        let provenance: attune_common::secret_provenance::SecretProvenance =
            serde_json::from_str(field.source_ref.as_deref().unwrap()).unwrap();
        let origin=provenance.origins.iter().find(|origin|matches!(origin,attune_common::secret_provenance::SecretOrigin::Entity {entity_type,path,..} if entity_type=="declared_input:workflow_variables:value"&&path=="/b")).unwrap();
        let site = attune_common::repositories::inherited_input::InputSite {
            execution: parent,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Value,
            path: "/b".into(),
        };
        let value = Inputs::receipt_context(conn, &site, origin, key)
            .await
            .unwrap();
        assert!(value
            .native_source_metadata()
            .iter()
            .any(|source| source.snapshot() == snapshot));
        assert!(
            value
                .source_receipts()
                .iter()
                .any(|source| source.site().execution == parent
                    && source.site().purpose == InputSitePurpose::Value
                    && source.site().path == "/a"),
            "b retains the real a VALUE stage, not substituted returned bytes"
        );
        let prefix: std::collections::BTreeSet<_> = value
            .source_receipts()
            .iter()
            .filter(|source| {
                source.site().execution == parent
                    && source.site().purpose == InputSitePurpose::Condition
            })
            .map(|source| source.site().path.clone())
            .collect();
        assert_eq!(
            prefix,
            (0..3)
                .map(|index| format!("{path}/next/{index}/when"))
                .collect()
        );
        let prepared = Inputs::prepare_source(conn, parent, &site, origin, key)
            .await
            .unwrap();
        assert_eq!(prepared.value(), "execution-original");
    }
    if overwrite {
        let fields = Secrets::find_stored_by_entity(&mut *conn, "workflow_variables", parent)
            .await
            .unwrap();
        let published = "/native_failed";
        assert_eq!(fields.len(), 1);
        let field = &fields[0];
        assert_eq!(
            field.json_path, published,
            "the whole authored literal subtree carries one controlled VALUE receipt"
        );
        let state = Workflows::find_by_id(
            &mut *conn,
            child.workflow_task.as_ref().unwrap().workflow_execution,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            state.variables["native_failed"],
            attune_common::secret_values::redaction_marker()
        );
        let provenance: attune_common::secret_provenance::SecretProvenance =
            serde_json::from_str(field.source_ref.as_deref().unwrap()).unwrap();
        let origin = provenance
            .origins
            .iter()
            .find(|origin| {
                matches!(origin,
                attune_common::secret_provenance::SecretOrigin::Entity { entity_type, path, .. }
                if entity_type == "declared_input:workflow_variables:value" && path == published)
            })
            .unwrap();
        let site = attune_common::repositories::inherited_input::InputSite {
            execution: parent,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Value,
            path: published.into(),
        };
        let value = Inputs::receipt_context(conn, &site, origin, key)
            .await
            .unwrap();
        let prefix: std::collections::BTreeSet<_> = value
            .source_receipts()
            .iter()
            .filter(|source| {
                source.site().execution == parent
                    && source.site().purpose == InputSitePurpose::Condition
            })
            .map(|source| source.site().path.clone())
            .collect();
        assert_eq!(prefix, (0..3).map(|index| format!("{path}/next/{index}/when")).collect(),
                "the final published subtree retains exactly its ordered Condition prefix, never later Conditions");
        let source = Inputs::prepare_source(conn, parent, &site, origin, key)
            .await
            .unwrap();
        assert_eq!(
            source.value(),
            &json!({"nested":{"token":"handled"},"history":"last"})
        );
        assert_eq!(
            source.value().pointer("/nested/token"),
            Some(&json!("handled"))
        );
        assert_eq!(source.value().pointer("/history"), Some(&json!("last")));
        assert!(source.value().get("obsolete").is_none());
    }
}

pub(super) async fn expiry(
    pool: &sqlx::PgPool,
    workflow: Id,
    task: &str,
    key: &str,
    expiry: chrono::DateTime<Utc>,
    terminal: bool,
) {
    if terminal {
        let mut tx = pool.begin().await.unwrap();
        let wait = Waits::find_by_workflow_task(&mut *tx, workflow, task)
            .await
            .unwrap()
            .unwrap();
        status(&mut tx, wait.target().unwrap(), "failed").await;
        tx.commit().await.unwrap();
    }
    let mut transaction = pool.begin().await.unwrap();
    protect(&mut transaction).await;
    let wait = Waits::find_by_workflow_task(&mut *transaction, workflow, task)
        .await
        .unwrap()
        .unwrap();
    let mut prepared = Witness::prepare(&mut transaction, &wait, key, terminal).await;
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
        .bind(workflow)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let waiter_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *transaction)
        .await
        .unwrap();
    let observation_pool = pool.clone();
    let release = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(60), async {
            let mut observed_wait = false;
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT $2 = ANY(pg_blocking_pids($1))")
                    .bind(waiter_pid)
                    .bind(blocker_pid)
                    .fetch_one(&observation_pool)
                    .await
                    .unwrap();
                let expired: bool = sqlx::query_scalar("SELECT clock_timestamp() >= $1")
                    .bind(expiry)
                    .fetch_one(&observation_pool)
                    .await
                    .unwrap();
                if blocked && !observed_wait {
                    assert!(
                        !expired,
                        "approval expired before the observed native wait target barrier"
                    );
                    observed_wait = true;
                }
                if blocked && observed_wait && expired {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        blocker.rollback().await.unwrap();
    });
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(70),
        prepared.apply(&mut transaction, key),
    )
    .await
    .unwrap();
    release.await.unwrap();
    assert!(outcome.is_err(), "selected native read expiry must deny release and config publication after the real target wait");
    transaction.rollback().await.unwrap();
    for name in [task, "native_recovery"] {
        assert!(
            ExecutionRepository::find_by_workflow_task(pool, workflow, name, None)
                .await
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(
        Waits::find_by_workflow_task(pool, workflow, task)
            .await
            .unwrap()
            .unwrap()
            .state,
        attune_common::models::WorkflowTaskWaitState::Waiting
    );
}
