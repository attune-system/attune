//! Actual ordinary task source preparation. Final generated-output receipts and
//! completion caller cases extend this target after A's opaque API is published.
pub(crate) mod helpers;
#[path = "native_failure_materialization_tests/native_success.rs"]
mod native_success;
use attune_common::{
    models::{Execution, ExecutionStatus, WorkflowExecution, WorkflowTaskMetadata},
    pack_registry::runtime_policy::CandidateRuntimePolicyPlan,
    repositories::{
        action::ActionRepository,
        component_lifecycle::PackProjectionIds,
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        execution_admission::ExecutionAdmissionRepository as Admission,
        native_failure_materialization::NativeFailureMaterializationRepository as Failure,
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        recorded_input_authority::RecordedInputAuthorityRepository,
        runtime_authority::{RuntimeAuthorityRepository as Authority, RuntimeOwner},
        schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            WorkflowDefinitionRepository, WorkflowExecutionRepository as Workflows,
        },
        Create, Update,
    },
    runtime_authority::{HumanAttribution, InvocationBinding, RequestAttribution},
    test_database::TestDatabase,
};
use helpers::{create_test_pool, ActionFixture, IdentityFixture, PackFixture};
use serde_json::json;
pub(crate) const KEY: &str = "native-failure-test-owned-encryption-key";

async fn original_failed_task(
    status: ExecutionStatus,
) -> (
    TestDatabase,
    tempfile::TempDir,
    Execution,
    WorkflowExecution,
    Execution,
) {
    original_failed_task_with_output(status, false).await
}

pub(crate) async fn original_failed_task_with_output(
    status: ExecutionStatus,
    protected: bool,
) -> (
    TestDatabase,
    tempfile::TempDir,
    Execution,
    WorkflowExecution,
    Execution,
) {
    original_failed_task_fixture(status, protected, false, None, None).await
}

#[derive(Clone, Copy)]
pub enum NativeResultCase {
    WholeResult,
    ConditionalWholeResult,
    StdoutOutput,
    DataOutput,
    DependentShorthand,
    LegacyWholeResult,
    ExplicitNextOverridesLegacy,
    DefaultGeneratedSuccess,
    FalseDecisionGeneratedSuccess,
    GeneratedWarningCandidate,
    ProspectiveResultOutput,
    ProspectiveResultChild,
    ProspectiveCompositeChild,
    IndependentRecoveryChild,
    // Authored independently before release/policy capture. Executor's owned MQ
    // regression uses a finite external INPUT consent, not an SDK policy fence.
    FiniteOriginalInputChild,
    FiniteOriginalPublicationPair,
    AuthoredFalseStatus,
    AuthoredFalseWithControls,
}

fn generated_success_source_schema() -> serde_json::Value {
    json!({"succeeded":{"type":"boolean"},"error":{"type":"string","secret":true}})
}

#[test]
fn generated_success_fixture_uses_a_valid_protected_contract_not_a_secret_boolean() {
    use attune_common::value_schema::{compile_inline, CompiledContract, SchemaContext};
    let contract = compile_inline(
        "fixture.generated_success",
        SchemaContext::FlatDocument,
        &generated_success_source_schema(),
    )
    .unwrap();
    let compiled = CompiledContract::new(&contract).unwrap();
    assert!(compiled.declares_secrets());
    compiled.validate(&json!({"succeeded":true})).unwrap();
    assert!(compile_inline(
        "fixture.generated_success",
        SchemaContext::FlatDocument,
        &json!({"succeeded":{"type":"boolean","secret":true}})
    )
    .is_err());
}

pub async fn original_success_source(
    case: NativeResultCase,
) -> (
    TestDatabase,
    tempfile::TempDir,
    Execution,
    WorkflowExecution,
    Execution,
) {
    original_failed_task_fixture(ExecutionStatus::Completed, false, false, Some(case), None).await
}

// This fixture module is also included by executor tests, which do not exercise
// file custody. Keep the allowance on this custody-only entry point.
#[allow(dead_code)]
pub async fn original_success_file_source(
    case: NativeResultCase,
) -> (
    TestDatabase,
    tempfile::TempDir,
    Execution,
    WorkflowExecution,
    Execution,
) {
    original_failed_task_fixture_with_file_api(
        ExecutionStatus::Completed,
        false,
        false,
        Some(case),
        None,
        true,
    )
    .await
}

pub async fn original_executor_failure(
    status: ExecutionStatus,
    condition: bool,
    build: fn(&serde_json::Value) -> serde_json::Value,
) -> (
    TestDatabase,
    tempfile::TempDir,
    Execution,
    WorkflowExecution,
    Execution,
) {
    original_failed_task_fixture(status, true, condition, None, Some(build)).await
}

/// Actual failed/timeout recovery source authored before immutable capture.
pub async fn original_executor_independent_recovery(
    status: ExecutionStatus,
    build: fn(&serde_json::Value) -> serde_json::Value,
) -> (
    TestDatabase,
    tempfile::TempDir,
    Execution,
    WorkflowExecution,
    Execution,
) {
    assert!(matches!(
        status,
        ExecutionStatus::Failed | ExecutionStatus::Timeout
    ));
    original_failed_task_fixture(
        status,
        false,
        false,
        Some(NativeResultCase::IndependentRecoveryChild),
        Some(build),
    )
    .await
}

pub async fn original_executor_native_result(
    case: NativeResultCase,
    build: fn(&serde_json::Value) -> serde_json::Value,
) -> (
    TestDatabase,
    tempfile::TempDir,
    Execution,
    WorkflowExecution,
    Execution,
) {
    original_failed_task_fixture(
        ExecutionStatus::Completed,
        false,
        false,
        Some(case),
        Some(build),
    )
    .await
}

/// Same genuinely admitted prospective-result fixture, with a separate callee
/// whose supplied finite task input receives independent input consent before
/// that callee exists. No original execution or historical seal is rewritten.
pub async fn original_executor_finite_input_child(
    build: fn(&serde_json::Value) -> serde_json::Value,
) -> (
    TestDatabase,
    tempfile::TempDir,
    Execution,
    WorkflowExecution,
    Execution,
) {
    original_executor_native_result(NativeResultCase::FiniteOriginalInputChild, build).await
}

/// Authored before capture: ordinary consumer first, independent finite-input
/// consumer second. Existing commit-boundary fixture/case remains unchanged.
pub async fn original_executor_finite_publication_pair(
    build: fn(&serde_json::Value) -> serde_json::Value,
) -> (
    TestDatabase,
    tempfile::TempDir,
    Execution,
    WorkflowExecution,
    Execution,
) {
    original_executor_native_result(NativeResultCase::FiniteOriginalPublicationPair, build).await
}

pub async fn original_executor_success_with_approval(
    build: fn(&serde_json::Value) -> serde_json::Value,
) -> (
    TestDatabase,
    tempfile::TempDir,
    Execution,
    WorkflowExecution,
    Execution,
) {
    original_failed_task_fixture(
        ExecutionStatus::Completed,
        false,
        true,
        Some(NativeResultCase::DefaultGeneratedSuccess),
        Some(build),
    )
    .await
}

async fn original_failed_task_fixture(
    status: ExecutionStatus,
    protected: bool,
    condition: bool,
    native_result: Option<NativeResultCase>,
    graph_builder: Option<fn(&serde_json::Value) -> serde_json::Value>,
) -> (
    TestDatabase,
    tempfile::TempDir,
    Execution,
    WorkflowExecution,
    Execution,
) {
    original_failed_task_fixture_with_file_api(
        status,
        protected,
        condition,
        native_result,
        graph_builder,
        false,
    )
    .await
}

async fn original_failed_task_fixture_with_file_api(
    status: ExecutionStatus,
    protected: bool,
    condition: bool,
    native_result: Option<NativeResultCase>,
    graph_builder: Option<fn(&serde_json::Value) -> serde_json::Value>,
    file_api: bool,
) -> (
    TestDatabase,
    tempfile::TempDir,
    Execution,
    WorkflowExecution,
    Execution,
) {
    assert!(
        matches!(status, ExecutionStatus::Failed | ExecutionStatus::Timeout)
            || (native_result.is_some() && status == ExecutionStatus::Completed)
    );
    let db = create_test_pool().await.unwrap();
    // Await cleanup even when setup assertions fail, before returning owners.
    use futures::FutureExt;
    let files = match tempfile::tempdir() {
        Ok(files) => files,
        Err(error) => {
            db.cleanup()
                .await
                .expect("fixture database cleanup after tempdir failure");
            panic!("fixture tempdir creation failed: {error}");
        }
    };
    let built = std::panic::AssertUnwindSafe(async {
    let pack = PackFixture::new_unique("native_failure")
        .create(&db)
        .await
        .unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "run")
        .create(&db)
        .await
        .unwrap();
    let execution_refs = if file_api {
        vec!["standard".to_owned()]
    } else {
        vec![]
    };
    if file_api {
        ActionRepository::update(
            &db,
            action.id,
            attune_common::repositories::action::UpdateActionInput {
                default_execution_permission_set_refs: Some(execution_refs.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let mut leaf = ActionFixture::new(pack.id, &pack.r#ref, "leaf");
    if matches!(
        native_result,
        Some(NativeResultCase::ProspectiveResultChild | NativeResultCase::ProspectiveCompositeChild | NativeResultCase::IndependentRecoveryChild)
            | Some(NativeResultCase::FiniteOriginalInputChild)
            | Some(NativeResultCase::FiniteOriginalPublicationPair)
    ) {
        leaf.param_schema = Some(
            json!({"value":{"type":"string","secret":true},"whole":{"type":"object","secret":true}}),
        );
    }
    let leaf = leaf.create(&db).await.unwrap();
    // Secret fields cannot carry schema defaults. The root graph authors an
    // exact task-input Key selection, evaluated only for the finite successor.
    // Its input-only consent is not a root SDK or outer-transition grant.
    let finite_leaf = if matches!(native_result,
        Some(NativeResultCase::FiniteOriginalInputChild | NativeResultCase::FiniteOriginalPublicationPair)) {
        let mut consumer = ActionFixture::new(pack.id, &pack.r#ref, "finite_consumer");
        consumer.param_schema = Some(json!({
            "value":{"type":"string","secret":true},
            "whole":{"type":"object","secret":true},
            "finite":{"type":"string","secret":true}
        }));
        Some(consumer.create(&db).await.unwrap())
    } else {
        None
    };
    let human = IdentityFixture::new_unique("native_failure_submitter")
        .create(&db)
        .await
        .unwrap();
    let mut graph = json!({"version":"1.0","tasks":[{"name":"next","action":leaf.r#ref,"permission_set_refs":[],"next":[]}]});
    if let Some(case) = native_result {
        match case {
            NativeResultCase::WholeResult => {
                graph["tasks"][0]["next"] = json!([{"publish":["whole"]}]);
                graph["output_map"] =
                    json!({"succeeded":"{{ true }}","echo":"{{ result().literal }}"});
            }
            NativeResultCase::ConditionalWholeResult => {
                graph["tasks"][0]["next"] = json!([
                {"when":"{{ failed() }}","publish":["not_fired"]},
                {"when":"{{ succeeded() }}","publish":["whole"]}
                ]);
                graph["output_map"] =
                    json!({"succeeded":"{{ true }}","echo":"{{ result().literal }}"});
            }
            NativeResultCase::StdoutOutput => {
                graph["output_map"] =
                    json!({"succeeded":"{{ true }}","value":"{{ result().token }}"})
            }
            NativeResultCase::DataOutput => {
                graph["output_map"] = json!({"succeeded":"{{ true }}","value":"{{ result().literal }}","collision":"{{ result().token }}"})
            }
            NativeResultCase::DependentShorthand => {
                graph["tasks"][0]["next"] =
                    json!([{"publish":["whole",{"copied":"{{ vars.whole }}"}]}]);
                graph["output_map"] = json!({"succeeded":"{{ true }}"});
            }
            NativeResultCase::LegacyWholeResult => {
                graph["tasks"][0].as_object_mut().unwrap().remove("next");
                graph["tasks"][0]["publish"] = json!(["whole"]);
                graph["output_map"] =
                    json!({"succeeded":"{{ true }}","echo":"{{ result().literal }}"});
            }
            NativeResultCase::ExplicitNextOverridesLegacy => {
                graph["tasks"][0]["publish"] = json!(["ignored"]);
                graph["tasks"][0]["next"] = json!([{"publish":["whole"]}]);
                graph["output_map"] =
                    json!({"succeeded":"{{ true }}","echo":"{{ result().literal }}"});
            }
            NativeResultCase::DefaultGeneratedSuccess => {}
            NativeResultCase::FalseDecisionGeneratedSuccess => {
                graph["tasks"][0]["next"] = json!([{"when":"{{ failed() }}"}]);
            }
            NativeResultCase::GeneratedWarningCandidate => {
                graph["output_map"] = json!({"value":"{{ result().literal }}"});
            }
            NativeResultCase::AuthoredFalseStatus => {
                graph["output_map"] =
                    json!({"succeeded":"{{ false }}","echo":"{{ result().literal }}"});
            }
            NativeResultCase::AuthoredFalseWithControls => {
                graph["output_map"] =
                    json!({"succeeded":"{{ false }}","echo":"{{ result().literal }}"});
                graph["tasks"][0]["next"] = json!([
                    {"when":"{{ failed() }}","publish":["not_fired"]},
                    {"when":"{{ succeeded() }}","publish":["whole"]}
                ]);
            }
            NativeResultCase::IndependentRecoveryChild => {
                graph["tasks"][0]["next"] = json!([{
                    "when": if status == ExecutionStatus::Timeout { "{{ timed_out() }}" } else { "{{ failed() }}" },
                    "publish":[{"recovered":{"literal":"recovery"}}], "do":["consumer"]
                }]);
                graph["tasks"].as_array_mut().unwrap().push(json!({
                    "name":"consumer", "action":leaf.r#ref,
                    "input":{"whole":"{{ variables.recovered }}"}, "next":[]
                }));
            }
            NativeResultCase::ProspectiveResultOutput
            | NativeResultCase::ProspectiveResultChild
            | NativeResultCase::ProspectiveCompositeChild
            | NativeResultCase::FiniteOriginalInputChild
            | NativeResultCase::FiniteOriginalPublicationPair => {
                graph["tasks"][0]["next"] = json!([
                    {"when":"{{ failed() }}","publish":["not_fired"]},
                    {"when":"{{ result().literal == result().data.literal }}","publish":["whole"]},
                    {"when":"{{ vars.whole.literal == variables.whole.literal }}","publish":[{"copied":"{{ workflow.whole.literal }}"}]},
                    {"when":"{{ copied == vars.whole.literal }}","publish":[{"whole":{"literal":"replacement"}}]},
                    {"when":"{{ workflow.whole.literal == 'replacement' }}","publish":[{"latest":"{{ variables.whole.literal }}"}]}
                ]);
                if matches!(case, NativeResultCase::ProspectiveResultChild | NativeResultCase::ProspectiveCompositeChild | NativeResultCase::FiniteOriginalInputChild | NativeResultCase::FiniteOriginalPublicationPair) {
                    graph["tasks"][0]["next"][4]["do"] = json!(["consumer"]);
                    graph["tasks"].as_array_mut().unwrap().push(json!({"name":"consumer","action":leaf.r#ref,"input":{"value":"{{ vars.copied }}","whole":"{{ variables.whole }}"},"next":[]}));
                    if matches!(case, NativeResultCase::ProspectiveCompositeChild) {
                        graph["tasks"][0]["next"][3]["publish"] = json!([{"whole":{
                            "literal":"replacement", "other":"{{ copied }}"
                        }}]);
                    }
                    if matches!(case, NativeResultCase::FiniteOriginalInputChild) {
                        // The graph declaration owns consent; only actual child
                        // input evaluation consumes it and creates the finite proof.
                        graph["tasks"][1]["action"] = json!(finite_leaf.as_ref().unwrap().r#ref);
                        graph["tasks"][1]["input"]["finite"] =
                            json!("{{ keystore['system.receipt_lifetime_original'] }}");
                    }
                    if matches!(case, NativeResultCase::FiniteOriginalPublicationPair) {
                        graph["tasks"][0]["next"][4]["do"] = json!(["consumer", "finite_later"]);
                        let mut later = graph["tasks"][1].clone();
                        later["name"] = json!("finite_later");
                        later["action"] = json!(finite_leaf.as_ref().unwrap().r#ref);
                        later["input"]["finite"] =
                            json!("{{ keystore['system.receipt_lifetime_original'] }}");
                        graph["tasks"].as_array_mut().unwrap().push(later);
                    }
                } else {
                    graph["output_map"] =
                        json!({"first":"{{ copied }}","latest":"{{ vars.whole.literal }}"});
                }
            }
        }
    }
    if condition {
        use attune_common::repositories::identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        };
        let mut key = helpers::KeyFixture::new_system("native_failure_decision", "unused");
        key.value = json!(false);
        key.create(&db).await.unwrap();
        let grants = json!([{"resource":"keys","actions":["read"],"constraints":{"refs":["system.native_failure_decision"]}}]);
        let set = PermissionSetRepository::create(
            &db,
            CreatePermissionSetInput {
                r#ref: "fixture.native_failure_decision".into(),
                pack: None,
                pack_ref: None,
                label: None,
                description: None,
                grants,
            },
        )
        .await
        .unwrap();
        PermissionAssignmentRepository::create(
            &db,
            CreatePermissionAssignmentInput {
                identity: human.id,
                permset: set.id,
            },
        )
        .await
        .unwrap();
        graph["tasks"][0]["next"] =
            json!([{"when":"{{ keystore['system.native_failure_decision'] }}"}]);
    }
    let definition = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: action.r#ref.clone(),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Native failure".into(),
            description: None,
            version: "1.0".into(),
            param_schema: None,
            out_schema: match native_result {
                // Secret scalar booleans are not valid authored schemas. Keep a
                // protected original contract using the native error string;
                // generated status itself remains an ordinary boolean field.
                Some(NativeResultCase::DefaultGeneratedSuccess) => Some(generated_success_source_schema()),
                Some(NativeResultCase::GeneratedWarningCandidate) => Some(json!({"value":{"type":"integer"}})),
                _ => protected.then(|| json!({"error":{"type":"string","secret":true},"succeeded":{"type":"boolean"}})),
            },
            definition: graph.clone(),
            tags: vec![],
        },
    )
    .await
    .unwrap();
    ActionRepository::link_workflow_def(&db, action.id, definition.id)
        .await
        .unwrap();
    std::fs::create_dir_all(files.path().join("actions/workflows")).unwrap();
    std::fs::write(
        files.path().join("actions/run.yaml"),
        format!(
            "ref: {}\nworkflow_file: workflows/run.yaml\npermission_set_refs: {}\n",
            action.r#ref,
            serde_json::to_string(&execution_refs).unwrap()
        ),
    )
    .unwrap();
    std::fs::write(
        files.path().join("actions/leaf.yaml"),
        format!("ref: {}\n", leaf.r#ref),
    )
    .unwrap();
    if let Some(consumer) = &finite_leaf {
        std::fs::write(files.path().join("actions/finite_consumer.yaml"), format!(
            "ref: {}\nparam_schema: {}\n", consumer.r#ref,
            serde_json::to_string(consumer.param_schema.as_ref().unwrap()).unwrap(),
        )).unwrap();
    }
    std::fs::write(
        files.path().join("actions/workflows/run.yaml"),
        serde_yaml_ng::to_string(&graph).unwrap(),
    )
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version.clone(),
            digest: "d".repeat(64),
            object_key: "packs/blobs/native-failure-fixture.tar.gz".into(),
            provider_version: "e:native-failure-fixture".into(),
            content_path: files.path().to_string_lossy().into_owned(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    let mut action_ids = vec![action.id, leaf.id];
    if let Some(consumer) = &finite_leaf { action_ids.push(consumer.id); }
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        release.id,
        &PackProjectionIds {
            actions: action_ids,
            workflows: vec![definition.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    Authority::ensure_owner(&mut tx, pack.id).await.unwrap();
    let policy = CandidateRuntimePolicyPlan::read(files.path(), &pack.r#ref)
        .unwrap()
        .admit_deployment(&mut tx, pack.id, human.id)
        .await
        .unwrap();
    let binding = InvocationBinding::Action {
        action_ref: action.r#ref.clone(),
    };
    let snapshot = attune_common::repositories::managed_executable::ManagedExecutableRepository::resolve_invocation(&mut tx, &policy.policy, &binding).await.unwrap();
    let admitted = Admission::select(
        &mut tx,
        pack.id,
        binding,
        &snapshot,
        &execution_refs,
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: human.id,
                login: human.login,
            },
        },
        attune_common::repositories::runtime_target::RuntimeTarget::Component(
            attune_common::repositories::runtime_target::ComponentTarget::Action,
            action.id,
        ),
    )
    .await
    .unwrap();
    let parent = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            status: ExecutionStatus::Running,
            executor: Some(human.id),
            config: Some(json!({})),
            permission_set_refs: execution_refs,
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    admitted
        .attach(&mut tx, RuntimeOwner::Execution(parent.id))
        .await
        .unwrap();
    let schema = Schemas::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, parent.id, &schema)
        .await
        .unwrap();
    RecordedInputAuthorityRepository::capture_human(&mut tx, "execution", parent.id, human.id)
        .await
        .unwrap();
    let parent_inputs = Materialize::seal_execution_inputs(&mut tx, parent.id, KEY)
        .await
        .unwrap();
    let state = Workflows::create_or_get_by_execution_with_conn(
        &mut tx,
        CreateWorkflowExecutionInput {
            execution: parent.id,
            workflow_def: definition.id,
            task_graph: graph_builder
                .map(|build| build(&graph))
                .unwrap_or_else(|| json!({"nodes":{"next":{"transitions":[]}}})),
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap()
    .workflow_execution;
    let child_snapshot = attune_common::repositories::managed_executable::ManagedExecutableRepository::resolve_invocation(&mut tx, &policy.policy, &InvocationBinding::WorkflowTask {
        workflow_ref: action.r#ref, task_path: "/tasks/0".into(), action_ref: leaf.r#ref.clone(),
    }).await.unwrap();
    let child = Admission::create_workflow_task_if_absent_pinned_with_conn(
        &mut tx,
        CreateExecutionInput {
            action: Some(leaf.id),
            action_ref: leaf.r#ref,
            parent: Some(parent.id),
            executor: Some(human.id),
            status: ExecutionStatus::Requested,
            config: Some(json!({})),
            workflow_task: Some(WorkflowTaskMetadata {
                workflow_execution: state.id,
                task_name: "next".into(),
                task_index: None,
                task_batch: None,
                triggered_by: None,
                retry_count: 0,
                max_retries: 0,
                next_retry_at: None,
                timeout_seconds: None,
                timed_out: status == ExecutionStatus::Timeout,
                duration_ms: None,
                started_at: None,
                completed_at: None,
            }),
            ..Default::default()
        },
        &child_snapshot,
        state.id,
        "next",
        None,
    )
    .await
    .unwrap()
    .execution;
    let schema = Schemas::materialize_action_pair(&mut tx, &child_snapshot)
        .await
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, child.id, &schema)
        .await
        .unwrap();
    let child_admission = Admission::require(&mut tx, RuntimeOwner::Execution(child.id))
        .await
        .unwrap();
    ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        child.id,
        Some(child_admission.id),
        &child_snapshot,
        Some(json!({})),
        &[],
    )
    .await
    .unwrap();
    let child_inputs = Materialize::seal_execution_inputs(&mut tx, child.id, KEY)
        .await
        .unwrap();
    let literal = "{{ keystore['foreign.never_parse'] }}";
    let result = match native_result {
        Some(
            NativeResultCase::WholeResult
            | NativeResultCase::ConditionalWholeResult
            | NativeResultCase::DependentShorthand
            | NativeResultCase::LegacyWholeResult
            | NativeResultCase::ExplicitNextOverridesLegacy
            | NativeResultCase::ProspectiveResultOutput
            | NativeResultCase::ProspectiveResultChild
            | NativeResultCase::ProspectiveCompositeChild
            | NativeResultCase::FiniteOriginalInputChild
            | NativeResultCase::FiniteOriginalPublicationPair
            | NativeResultCase::AuthoredFalseStatus
            | NativeResultCase::AuthoredFalseWithControls,
        ) => json!({"whole":"must_not_select_this_field","data":{"literal":literal}}),
        Some(NativeResultCase::StdoutOutput) => {
            json!({"stdout":serde_json::to_string(&json!({"token":literal,"stdout":"must_not_overwrite_envelope"})).unwrap()})
        }
        Some(NativeResultCase::DataOutput) => {
            json!({"token":"envelope_wins","stdout":"{\"literal\":\"must_not_parse_when_data_exists\"}","data":{"token":"data_must_not_overwrite_envelope","literal":literal}})
        }
        Some(
            NativeResultCase::DefaultGeneratedSuccess
            | NativeResultCase::FalseDecisionGeneratedSuccess
            | NativeResultCase::GeneratedWarningCandidate,
        ) => json!({"data":{"literal":literal}}),
        Some(NativeResultCase::IndependentRecoveryChild) => json!({"error":"ordinary recovery source","data":{"literal":literal}}),
        None => json!({"error":"ordinary failure","data":{"literal":literal}}),
    };
    let child = ExecutionRepository::update(
        &mut *tx,
        child.id,
        UpdateExecutionInput {
            status: Some(status),
            result: Some(result),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for receipt in parent_inputs.iter().chain(&child_inputs) {
        receipt.ensure_fresh_on(&mut tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    for receipt in parent_inputs.iter().chain(&child_inputs) {
        receipt.ensure_fresh().unwrap();
    }
        (parent, state, child)
    }).catch_unwind().await;
    match built {
        Ok((parent, state, child)) => (db, files, parent, state, child),
        Err(panic) => {
            let database_cleanup = db.cleanup().await;
            let files_cleanup = files.close();
            database_cleanup.expect("fixture database cleanup after setup panic");
            files_cleanup.expect("fixture file cleanup after setup panic");
            std::panic::resume_unwind(panic)
        }
    }
}

#[tokio::test]
async fn normal_failed_completion_uses_original_admitted_child_schema_and_result() {
    let (db, _files, parent, workflow, child) = original_failed_task(ExecutionStatus::Failed).await;
    let mut tx = db.begin().await.unwrap();
    let plan = Failure::prepare_for_completion(&mut tx, child.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(plan.failed_tasks(), &["next"]);
    assert_eq!(
        plan.metadata(),
        &json!({"succeeded":false,"error":"Workflow failed: 1 task(s) failed: next"})
    );
    assert_eq!(plan.outcomes().next().unwrap().execution(), child.id);
    plan.require_completed_execution(&child).unwrap();
    assert!(plan.require_completed_execution(&parent).is_err());
    assert_eq!(
        plan.original_task_path(child.id).unwrap(),
        "/workflow/tasks/0"
    );
    assert_eq!(
        plan.source_executions().next().unwrap().result,
        child.result
    );
    plan.require_current(&parent, &workflow, &["next".into()])
        .unwrap();
    plan.require_original_child(
        &child,
        plan.original_contract(child.id).unwrap(),
        plan.original_result_fields(child.id).unwrap(),
    )
    .unwrap();
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    _files.close().unwrap();
}

#[tokio::test]
async fn failed_completion_rejects_changed_target_and_original_result_before_writes() {
    let (db, _files, parent, workflow, child) = original_failed_task(ExecutionStatus::Failed).await;
    let mut tx = db.begin().await.unwrap();
    let plan = Failure::prepare_for_completion(&mut tx, child.id)
        .await
        .unwrap()
        .unwrap();
    let mut changed = workflow.clone();
    changed.variables = json!({"not_prepared":"new"});
    assert!(plan
        .require_current(&parent, &changed, &["next".into()])
        .is_err());
    let mut changed = child.clone();
    changed.result = Some(json!({"error":"not the prepared original result"}));
    assert!(plan.require_completed_execution(&changed).is_err());
    assert!(plan
        .require_original_child(
            &changed,
            plan.original_contract(child.id).unwrap(),
            plan.original_result_fields(child.id).unwrap()
        )
        .is_err());
    assert!(plan
        .require_metadata(&json!({"succeeded":false,"error":"supplied summary"}))
        .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    _files.close().unwrap();
}

#[tokio::test]
async fn failed_completion_missing_original_schema_never_uses_publisher_current_schema() {
    let (db, _files, _, _, child) = original_failed_task(ExecutionStatus::Failed).await;
    let mut tx = db.begin().await.unwrap();
    sqlx::query(
        "DELETE FROM runtime_schema_binding WHERE entity_kind='execution' AND entity_id=$1",
    )
    .bind(child.id)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert!(Failure::prepare_for_completion(&mut tx, child.id)
        .await
        .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    _files.close().unwrap();
}

#[tokio::test]
async fn timed_out_completion_uses_the_actual_admitted_timeout_outcome() {
    let (db, _files, parent, workflow, child) =
        original_failed_task(ExecutionStatus::Timeout).await;
    let mut tx = db.begin().await.unwrap();
    let plan = Failure::prepare_for_completion(&mut tx, child.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        plan.outcomes().next().unwrap().value("timed_out"),
        Some(json!(true))
    );
    assert_eq!(plan.completed_execution(), child.id);
    plan.require_current_rows(&parent, &workflow).unwrap();
    plan.require_projected_failure(&["next".into()]).unwrap();
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    _files.close().unwrap();
}
