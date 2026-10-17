//! Cold readers consume H's real B/A success publication, not a supplied receipt.
use super::*;
#[path = "../../../common/tests/native_failure_materialization_tests.rs"]
#[allow(dead_code)]
mod source;

use attune_common::{
    models::{Execution, ExecutionStatus},
    repositories::{
        cache::{CacheEntryRepository, CacheTransactionMode},
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        native_failure_materialization::NativeFailureMaterializationRepository as Success,
        native_transition_materialization::NativeTransitionMaterializationRepository as Transitions,
        runtime_authority::{
            RuntimeAuthorityRepository as Authority, RuntimeCredentialBinding, RuntimeOwner,
        },
        runtime_read::{RuntimeReadOrigin, RuntimeReadRepository, RuntimeReadTarget},
        schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
        workflow::{UpdateWorkflowExecutionInput, WorkflowExecutionRepository as Workflows},
        Update,
    },
    runtime_authority::{
        BoundRuntimeTarget, GrantTargetBindings, HumanAttribution, InvocationBinding,
        InvocationPolicy, PermissionSetDefinition, RequestAttribution, RuntimePolicy,
    },
    secret_values::{prepare_secret_values, redact_secret_path_sources},
};
use std::collections::BTreeMap;

#[tokio::test]
async fn cold_success_reader_accepts_generated_default_and_denies_missing_child_scope() {
    readable(source::NativeResultCase::DefaultGeneratedSuccess, false).await;
}

#[tokio::test]
async fn cold_success_reader_accepts_original_schema_warning_and_retains_real_leaf() {
    readable(source::NativeResultCase::GeneratedWarningCandidate, false).await;
}

#[tokio::test]
async fn cold_success_reader_preserves_authored_false_and_both_real_control_decisions() {
    readable(source::NativeResultCase::AuthoredFalseWithControls, true).await;
}

async fn readable(case: source::NativeResultCase, false_status: bool) {
    let (db, files, parent, workflow, child) = source::original_success_source(case).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, source::KEY)
        .await
        .unwrap()
        .unwrap();
    let state = Success::prepare_native_success(&mut tx, parent.id, workflow.id, child.id, &plan)
        .await
        .unwrap()
        .unwrap();
    let output = Success::prepare_success_output(
        &mut tx,
        parent.id,
        workflow.id,
        child.id,
        &plan,
        source::KEY,
    )
    .await
    .unwrap()
    .unwrap();
    output.require_original_rows(&parent, &workflow).unwrap();
    if false_status {
        assert_eq!(state.decisions().len(), 2);
        assert_eq!(state.decisions()[0].value(), &serde_json::json!(false));
        assert_eq!(state.decisions()[1].value(), &serde_json::json!(true));
    }
    plan.record(&mut tx, &workflow, source::KEY).await.unwrap();
    let projected = Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            completed_tasks: Some(state.projected_completed_tasks().to_vec()),
            current_tasks: Some(state.projected_current_tasks().to_vec()),
            failed_tasks: Some(state.projected_failed_tasks().to_vec()),
            skipped_tasks: Some(state.projected_skipped_tasks().to_vec()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (value, sources) = output
        .record(&mut tx, &projected, source::KEY)
        .await
        .unwrap();
    assert_eq!(value["succeeded"], !false_status);
    let warning = matches!(case, source::NativeResultCase::GeneratedWarningCandidate);
    assert_eq!(value.get("output_validation_warning").is_some(), warning);
    let (public, fields) = redact_secret_path_sources(value.clone(), &sources);
    let fields = prepare_secret_values(fields, source::KEY).unwrap();
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut tx,
        ENTITY_EXECUTION_RESULT,
        parent.id,
        &fields,
    )
    .await
    .unwrap();
    // The official encrypted root receipt is persisted before either terminal state.
    ExecutionRepository::update(
        &mut *tx,
        parent.id,
        UpdateExecutionInput {
            result: Some(public),
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let readers = readers(&mut tx, &parent, &child).await;
    tx.commit().await.unwrap();

    // New transactions/frontiers force replay from persisted ciphertext/release.
    for (credential, allowed) in readers {
        let mut tx = db.begin().await.unwrap();
        let authority = Authority::validate_execution_credential(&mut tx, &credential)
            .await
            .unwrap();
        let rows = ExecutionSecretValueRepository::find_bounded_provenance(
            &mut *tx,
            &[ENTITY_EXECUTION_RESULT],
            &[parent.id],
        )
        .await
        .unwrap();
        assert!(!rows.is_empty());
        let result =
            authorize_origins_with_key(&mut tx, &authority, &rows, Some(source::KEY)).await;
        if allowed {
            let budget =
                result.expect("a fully scoped current reader must read the genuine success result");
            budget.ensure_receipts_fresh_on(&mut tx).await.unwrap();
            // Capture only the production evaluator's complete parents, then
            // replay the normal runtime-read record in a cold transaction.
            RuntimeReadRepository::record(
                &mut tx,
                &authority,
                RuntimeReadOrigin {
                    target: RuntimeReadTarget::Execution {
                        execution: parent.id,
                    },
                    provenance: snapshot(&rows).unwrap(),
                    parents: budget.retained_parents(),
                },
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            let mut replay = db.begin().await.unwrap();
            let authority = Authority::validate_execution_credential(&mut replay, &credential)
                .await
                .unwrap();
            let recorded = execution_origins(&mut replay, credential.execution_id)
                .await
                .unwrap();
            let replayed =
                authorize_origins_with_key(&mut replay, &authority, &recorded, Some(source::KEY))
                    .await
                    .unwrap();
            replayed
                .ensure_receipts_fresh_on(&mut replay)
                .await
                .unwrap();
            replay.rollback().await.unwrap();
        } else {
            assert!(
                matches!(result, Err(ApiError::Forbidden(_))),
                "missing original child Read/Decrypt must deny"
            );
            tx.rollback().await.unwrap();
        }
    }
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

async fn readers(
    conn: &mut PgConnection,
    parent: &Execution,
    child: &Execution,
) -> Vec<(RuntimeCredentialBinding, bool)> {
    let original = Authority::execution_admission(conn, parent.id)
        .await
        .unwrap()
        .unwrap();
    let owner = original.authority.owner.clone();
    let pin = child.executable_snapshot.as_ref().unwrap();
    let full = format!("{}.success_reader", owner.pack_ref);
    let root_only = format!("{}.success_reader_root_only", owner.pack_ref);
    let full_set = serde_json::from_value::<PermissionSetDefinition>(serde_json::json!({"ref":full,"grants":[
        {"resource":"executions","actions":["read"],"constraints":{"pack_refs":[owner.pack_ref]}},
        {"resource":"executions","actions":["decrypt"],"constraints":{"pack_refs":[owner.pack_ref]}}
    ]})).unwrap().compile(&owner, &[]).unwrap();
    let root_set = serde_json::from_value::<PermissionSetDefinition>(serde_json::json!({"ref":root_only,"grants":[
        {"resource":"executions","actions":["read","decrypt"],"constraints":{"pack_refs":[owner.pack_ref]}}
    ]})).unwrap().compile(&owner, &[]).unwrap();
    let sets = BTreeMap::from([(full.clone(), full_set), (root_only.clone(), root_set)]);
    let mut concrete_targets = BTreeMap::new();
    for (reference, ids) in [
        (&full, vec![parent.id, child.id]),
        (&root_only, vec![parent.id]),
    ] {
        for grant in &sets[reference].grants {
            let mut targets = BTreeSet::new();
            let mut packs = BTreeSet::new();
            for id in &ids {
                let resolved = RuntimeTargetRepository::resolve(
                    conn,
                    RuntimeTarget::Execution(*id),
                    owner.sponsor_identity_id,
                )
                .await
                .unwrap()
                .unwrap();
                targets.insert(BoundRuntimeTarget {
                    kind: resolved.identity_kind().into(),
                    id: *id,
                    pack_id: resolved.pack_id(),
                    owner_type: None,
                    owner_id: None,
                });
                packs.insert(resolved.pack_id().unwrap());
            }
            let digest = {
                use sha2::Digest;
                hex::encode(sha2::Sha256::digest(serde_jcs::to_vec(grant).unwrap()))
            };
            assert!(concrete_targets
                .insert(
                    digest,
                    GrantTargetBindings {
                        targets: Some(targets),
                        owners: None,
                        packs: Some(packs),
                    }
                )
                .is_none());
        }
    }
    let binding = InvocationBinding::Action {
        action_ref: child.action_ref.clone(),
    };
    let refs = vec![full.clone(), root_only.clone()];
    let policy = Authority::capture_policy(
        conn,
        &RuntimePolicy {
            owner: owner.clone(),
            release_id: pin.release.id,
            release_digest: pin.release.digest.clone(),
            concrete_targets,
            invocations: vec![InvocationPolicy::from_executable(
                binding.clone(),
                pin,
                Some(&refs),
                &sets,
            )
            .unwrap()],
        },
    )
    .await
    .unwrap();
    let human = attune_common::repositories::identity::IdentityRepository::find_by_id(
        &mut *conn,
        owner.sponsor_identity_id,
    )
    .await
    .unwrap()
    .unwrap();
    let mut result = Vec::new();
    for (reference, allowed) in [(full, true), (root_only, false)] {
        let selected = policy
            .policy
            .select(
                policy.id,
                &binding,
                &[reference],
                RequestAttribution::Human {
                    actor: HumanAttribution {
                        identity_id: human.id,
                        login: human.login.clone(),
                    },
                },
            )
            .unwrap();
        let reader = ExecutionRepository::create_pinned(
            &mut *conn,
            CreateExecutionInput {
                action: Some(pin.executable.action.id),
                action_ref: child.action_ref.clone(),
                status: ExecutionStatus::Running,
                config: Some(serde_json::json!({})),
                permission_set_refs: selected.selected_refs.clone(),
                ..Default::default()
            },
            pin,
        )
        .await
        .unwrap();
        let schemas = Schemas::materialize_action_pair(conn, pin).await.unwrap();
        Schemas::attach(conn, BindingOwner::Execution, reader.id, &schemas)
            .await
            .unwrap();
        let admission =
            Authority::record_admission(conn, RuntimeOwner::Execution(reader.id), &selected)
                .await
                .unwrap();
        result.push((
            RuntimeCredentialBinding {
                execution_id: reader.id,
                admission_id: admission.id,
                sponsor_identity_id: owner.sponsor_identity_id,
                action_ref: child.action_ref.clone(),
                attempt: selected.attempt,
            },
            allowed,
        ));
    }
    result
}
