mod helpers;
#[path = "execution_admission_tests/native_staged_completion.rs"]
mod native_staged_completion;

use attune_common::{
    models::{ExecutionExecutableSnapshot, ExecutionStatus},
    repositories::{
        component_lifecycle::PackProjectionIds,
        execution::{CreateExecutionInput, ExecutionRepository},
        execution_admission::ExecutionAdmissionRepository as Admission,
        identity::{IdentityRepository, UpdateIdentityInput},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        rule_authority::RuleAuthorityRepository,
        runtime_authority::{RuntimeAuthorityRepository as Authority, RuntimeOwner},
        Create, Update,
    },
    runtime_authority::{
        HumanAttribution, InvocationBinding, InvocationPolicy, PermissionSetDefinition,
        RequestAttribution, RuntimePolicy,
    },
    test_database::TestDatabase,
};
use helpers::{create_test_pool, ActionFixture, IdentityFixture, PackFixture};
use serde_json::json;

fn fixture_digest(value: &impl serde::Serialize) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(serde_jcs::to_vec(value).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn create_queue(
    db: &TestDatabase,
    action: &attune_common::models::Action,
) -> attune_common::models::WorkQueue {
    use attune_common::{
        models::{ActionReferenceVisibility, WorkQueueBatchMode, WorkQueueUpdateStrategy},
        repositories::work_queue::{CreateWorkQueueInput, WorkQueueRepository},
    };
    WorkQueueRepository::create(
        db.pool(),
        CreateWorkQueueInput {
            r#ref: format!("{}.inbox", action.pack_ref),
            pack: Some(action.pack),
            pack_ref: Some(action.pack_ref.clone()),
            is_adhoc: false,
            label: "inbox".into(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(action.id),
            dispatch_action_ref: action.r#ref.clone(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: WorkQueueUpdateStrategy::Immutable,
            batch_mode: WorkQueueBatchMode::Batch,
            item_schema: json!({"message": {"type": "string"}}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: Some(vec![format!("{}.effects", action.pack_ref)]),
            config: json!({}),
            reference_visibility: ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: vec![],
        },
    )
    .await
    .unwrap()
}
use std::collections::BTreeMap;

async fn fixture() -> (
    TestDatabase,
    ExecutionExecutableSnapshot,
    RuntimePolicy,
    i64,
) {
    fixture_with_defaults(false).await
}

async fn fixture_with_defaults(
    authored_defaults: bool,
) -> (
    TestDatabase,
    ExecutionExecutableSnapshot,
    RuntimePolicy,
    i64,
) {
    fixture_with_native_shape(authored_defaults, false).await
}

async fn fixture_with_native_shape(
    authored_defaults: bool,
    empty_workflow: bool,
) -> (
    TestDatabase,
    ExecutionExecutableSnapshot,
    RuntimePolicy,
    i64,
) {
    fixture_with_native_variant(authored_defaults, empty_workflow, None).await
}

async fn fixture_with_native_variant(
    authored_defaults: bool,
    empty_workflow: bool,
    staged: Option<native_staged_completion::Case>,
) -> (
    TestDatabase,
    ExecutionExecutableSnapshot,
    RuntimePolicy,
    i64,
) {
    let db = create_test_pool().await.unwrap();
    let mut pack_fixture = PackFixture::new_unique("exec_admission");
    if authored_defaults {
        pack_fixture.conf_schema =
            json!({"default_value":{"type":"string"},"selector":{"type":"string"}});
        pack_fixture.config =
            json!({"default_value":"child-default-value","selector":"default_value"});
    }
    let pack = pack_fixture.create(&db).await.unwrap();
    if authored_defaults {
        let mut configuration = db.begin().await.unwrap();
        attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(
            &mut configuration,
        )
        .await
        .unwrap();
        attune_common::repositories::component_configuration::ComponentConfigurationRepository::bind_pack(
            &mut configuration, &pack, Some(&pack.config), None,
        ).await.unwrap();
        configuration.commit().await.unwrap();
    }
    let action = ActionFixture::new(pack.id, &pack.r#ref, "run")
        .create(&db)
        .await
        .unwrap();
    if authored_defaults {
        attune_common::repositories::action::ActionRepository::update(
            &db,
            action.id,
            attune_common::repositories::action::UpdateActionInput {
                param_schema: Some(json!({
                    "token":{"type":"string","default":"{{ pack.config.default_value }}"},
                    "value":{"type":"string","secret":true},
                    "key_ref":{"type":"string"},
                    "condition_value":{"type":"string"}
                })),
                out_schema: Some(json!({
                    "keyed_output":{"type":"string","secret":true},
                    "supplied_output":{"type":"string","secret":true},
                    "variable_output":{"type":"string","secret":true},
                    "started_output":{"type":"string"},
                    "whole_result_output":{"type":"object","secret":true},
                    "collision_output":{"type":"string"},
                    "envelope_output":{"type":"integer"},
                    "result_output":{"type":"string"},
                    "literal_output":{"type":"string"}
                })),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let leaf = ActionFixture::new(pack.id, &pack.r#ref, "leaf")
        .create(&db)
        .await
        .unwrap();
    let queue = create_queue(&db, &action).await;
    use attune_common::repositories::{
        action::ActionRepository,
        workflow::{CreateWorkflowDefinitionInput, WorkflowDefinitionRepository},
    };
    let mut graph = attune_common::workflow::parse_workflow_yaml(&format!(
        "version: '1.0.0'\ntasks:\n  - name: next\n    action: {}\n",
        leaf.r#ref
    ))
    .unwrap();
    if authored_defaults {
        graph
            .vars
            .insert("selected".into(), json!("{{ config.default_value }}"));
        graph.vars.insert("literal".into(), json!("public literal"));
        graph
            .vars
            .insert("copied_literal".into(), json!("{{ workflow.literal }}"));
        graph
            .vars
            .insert("dynamic".into(), json!("{{ config[config.selector] }}"));
        graph
            .vars
            .insert("started".into(), json!("{{ system.workflow_start }}"));
        graph
            .vars
            .insert("supplied".into(), json!("{{ parameters.value }}"));
        graph
            .vars
            .insert("keyed".into(), json!("{{ keystore[parameters.key_ref] }}"));
    }
    let mut authored_graph = serde_json::to_value(graph).unwrap();
    if authored_defaults {
        authored_graph["tasks"][0]["next"] = json!([
            {"publish":[{"unconditional_literal":{"accepted":true}},
                {"unconditional_value":"{{ parameters.value }}"}], "do":[]},
            {"when":"{{ succeeded() and config.selector == 'default_value' }}", "publish":[
                {"selected_key":"{{ keystore[parameters.key_ref] }}"},
                {"published":"{{ parameters.value }}"},
                {"literal_publication":{"value":"immutable literal"}},
                {"task_publication":"{{ result().message }}"}
            ], "do":[]},
            {"when":"{{ false and keystore['foreign.never_selected'] }}", "publish":[{"must_not_publish":"{{ keystore['foreign.never_read'] }}"}], "do":[]},
            {"when":"parameters.condition_value", "publish":[{"bare_truthiness":"bare nonempty string"}], "do":[]},
            {"when":"{{ parameters.condition_value }}", "publish":[{"must_not_template_publish":"template false string"}], "do":[]},
            {"when":"{{ keystore[parameters.key_ref] }}", "publish":[{"key_truthiness":"canonical selected Key"}], "do":[]}
        ]);
        authored_graph["output_map"] = json!({
            "keyed_output":"{{ keystore[parameters.key_ref] }}",
            "supplied_output":"{{ parameters.value }}",
            "result_output":"{{ result().message }}",
            "whole_result_output":"{{ result() }}",
            "collision_output":"{{ result().collision }}",
            "envelope_output":"{{ result().exit_code }}",
            "literal_output":"immutable output literal"
        });
        if empty_workflow {
            authored_graph["tasks"] = json!([]);
            authored_graph["output_map"] = json!({
                "keyed_output":"{{ keystore[parameters.key_ref] }}",
                "supplied_output":"{{ parameters.value }}",
                "variable_output":"{{ workflow.supplied }}",
                "started_output":"{{ system.workflow_start }}",
                "literal_output":"immutable empty output literal"
            });
        }
    }
    if let Some(case) = staged {
        native_staged_completion::configure_graph(&mut authored_graph, case);
        attune_common::repositories::action::ActionRepository::update(
            &db,
            action.id,
            attune_common::repositories::action::UpdateActionInput {
                out_schema: Some(json!({
                    "b_output":{"type":"string","secret":true},
                    "a_output":{"type":"string","secret":true},
                    "untouched_output":{"type":"string"},
                    "literal_output":{"type":"string"}
                })),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let definition = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: format!("{}.run", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "run".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            definition: authored_graph,
            tags: vec![],
        },
    )
    .await
    .unwrap();
    ActionRepository::link_workflow_def(&db, action.id, definition.id)
        .await
        .unwrap();
    let human = IdentityFixture::new_unique("requester")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version,
            digest: "b".repeat(64),
            object_key: "packs/blobs/admission.tar.gz".into(),
            provider_version: "e:admission".into(),
            content_path: "/packs/.releases/admission".into(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        release.id,
        &PackProjectionIds {
            actions: vec![action.id, leaf.id],
            work_queues: vec![queue.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    RuleAuthorityRepository::capture_deployment(&mut tx, pack.id)
        .await
        .unwrap();
    let owner = Authority::find_owner(&mut tx, pack.id)
        .await
        .unwrap()
        .unwrap();
    let definition: PermissionSetDefinition = serde_json::from_value(json!({
        "ref": format!("{}.effects", pack.r#ref), "grants": [{"resource":"events", "actions":["create"]}]
    })).unwrap();
    let set = definition.compile(&owner, &[]).unwrap();
    let invocation = InvocationPolicy {
        metadata_revision: None,
        source_id: None,
        dispatch_template_digest: None,
        foreign_workflow_policy: None,
        binding: InvocationBinding::Action {
            action_ref: action.r#ref.clone(),
        },
        target_release_id: release.id,
        target_release_digest: release.digest.clone(),
        permission_sets: BTreeMap::from([(set.reference.clone(), set)]),
        standard_access: true,
    };
    let mut task = invocation.clone();
    task.binding = InvocationBinding::WorkflowTask {
        workflow_ref: action.r#ref.clone(),
        task_path: "/tasks/0".into(),
        action_ref: leaf.r#ref.clone(),
    };
    let mut queue = invocation.clone();
    queue.binding = InvocationBinding::Queue {
        queue_ref: format!("{}.inbox", pack.r#ref),
        action_ref: action.r#ref.clone(),
    };
    let mut leaf_invocation = invocation.clone();
    leaf_invocation.binding = InvocationBinding::Action {
        action_ref: leaf.r#ref.clone(),
    };
    let policy = RuntimePolicy {
        concrete_targets: BTreeMap::new(),
        owner,
        release_id: release.id,
        release_digest: release.digest,
        invocations: if empty_workflow {
            vec![invocation, queue, leaf_invocation]
        } else {
            vec![invocation, task, queue, leaf_invocation]
        },
    };
    let revision = Authority::capture_policy(&mut tx, &policy).await.unwrap();
    Authority::activate_policy(&mut tx, revision.id)
        .await
        .unwrap();
    let snapshot = attune_common::repositories::managed_executable::ManagedExecutableRepository::resolve_invocation(&mut tx, &revision.policy, &InvocationBinding::Action { action_ref: action.r#ref.clone() })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (db, snapshot, policy, human.id)
}

async fn pending_literal(
    db: &TestDatabase,
    snapshot: &ExecutionExecutableSnapshot,
    requester: i64,
) -> attune_common::models::WorkQueueItem {
    use attune_common::{
        models::WorkQueueItemStatus,
        repositories::{
            work_queue::{CreateWorkQueueItemInput, WorkQueueItemRepository, WorkQueueRepository},
            FindByRef,
        },
    };
    let queue = WorkQueueRepository::find_by_ref(
        db.pool(),
        &format!("{}.inbox", snapshot.executable.action.pack_ref),
    )
    .await
    .unwrap()
    .unwrap();
    WorkQueueItemRepository::create_pinned(
        db.pool(),
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref,
            item_key: None,
            priority: 0,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"message":"literal payload"}),
            metadata: json!({}),
            trace_tag: None,
            enqueue_source: "test".into(),
            requested_by_identity: Some(requester),
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
        snapshot,
    )
    .await
    .unwrap()
}

async fn assign_input_grants(
    db: &TestDatabase,
    identity: i64,
    name: &str,
    grants: serde_json::Value,
) {
    use attune_common::repositories::{
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        Create,
    };
    let set = PermissionSetRepository::create(
        db.pool(),
        CreatePermissionSetInput {
            r#ref: format!("fixture.{name}"),
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
        db.pool(),
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await
    .unwrap();
}

#[test]
fn queue_attribution_round_trips_canonical_numeric_json_keys() {
    let original = RequestAttribution::QueueBatch {
        queue_ref: "example.inbox".into(),
        items: BTreeMap::from([
            (
                42,
                Some(HumanAttribution {
                    identity_id: 99,
                    login: "historical".into(),
                }),
            ),
            (43, None),
        ]),
    };
    let encoded = serde_json::to_value(&original).unwrap();
    assert_eq!(encoded["items"]["42"]["identity_id"], 99);
    assert_eq!(
        serde_json::from_value::<RequestAttribution>(encoded.clone()).unwrap(),
        original
    );
    assert_eq!(
        serde_json::from_str::<RequestAttribution>(&encoded.to_string()).unwrap(),
        original
    );
    for invalid in ["-1", "01", "not-an-id"] {
        let value =
            json!({"kind":"queue_batch","queue_ref":"example.inbox","items":{(invalid): null}});
        assert!(serde_json::from_value::<RequestAttribution>(value).is_err());
    }
}

#[tokio::test]
async fn recorded_admission_selection_retry_and_revocation() {
    use attune_common::repositories::{
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        Create,
    };
    let (db, snapshot, policy, human) = fixture().await;
    let input_key = attune_common::repositories::key::KeyRepository::create(
        &db,
        attune_common::repositories::key::CreateKeyInput {
            local_ref: "input_key".into(),
            owner_type: attune_common::models::OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Input".into(),
            encrypted: true,
            encryption_key_hash: None,
            value: json!("ciphertext-not-delivered-by-this-proof-test"),
        },
    )
    .await
    .unwrap();
    let leaf = snapshot
        .pack_executables
        .get(&format!("{}.leaf", policy.owner.pack_ref))
        .unwrap();
    let task_snapshot = ExecutionExecutableSnapshot {
        release: leaf.release.clone(),
        executable: leaf.executable.clone(),
        pack_executables: snapshot.pack_executables.clone(),
    };
    let input_set = PermissionSetRepository::create(&db, CreatePermissionSetInput { r#ref: "fixture.accepted_input".into(), pack: None, pack_ref: None, label: None, description: None, grants: json!([{"resource":"keys", "actions":["read","decrypt"], "constraints":{"refs":["system.input_key"]}}]) }).await.unwrap();
    PermissionAssignmentRepository::create(
        &db,
        CreatePermissionAssignmentInput {
            identity: human,
            permset: input_set.id,
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let refs = vec![format!("{}.effects", policy.owner.pack_ref)];
    let attribution = RequestAttribution::Human {
        actor: HumanAttribution {
            identity_id: human,
            login: "historical-requester".into(),
        },
    };
    let admission = Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &refs,
        attribution.clone(),
        attune_common::repositories::runtime_target::RuntimeTarget::Component(
            attune_common::repositories::runtime_target::ComponentTarget::Action,
            snapshot.executable.action.id,
        ),
    )
    .await
    .unwrap();
    let input = CreateExecutionInput {
        action: Some(snapshot.executable.action.id),
        action_ref: snapshot.executable.action.r#ref.clone(),
        executor: Some(human),
        permission_set_refs: refs.clone(),
        status: ExecutionStatus::Running,
        ..Default::default()
    };
    let source = ExecutionRepository::create_pinned(&mut *tx, input.clone(), &snapshot)
        .await
        .unwrap();
    let original = admission
        .attach(&mut tx, RuntimeOwner::Execution(source.id))
        .await
        .unwrap();
    attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(&mut tx, "execution", source.id, human).await.unwrap();
    tx.commit().await.unwrap();
    IdentityRepository::update(
        &db,
        human,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    attune_common::repositories::action::ActionRepository::update(
        &db,
        snapshot.executable.action.id,
        attune_common::repositories::action::UpdateActionInput {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let inherited = Admission::inherit(&mut tx, RuntimeOwner::Execution(source.id), &snapshot)
        .await
        .unwrap();
    assert!(Admission::input_authority(&mut tx, source.id, Some(human))
        .await
        .unwrap()
        .is_some());
    let origin = attune_common::secret_provenance::KeyOrigin {
        key_id: input_key.id,
        key_ref: "system.input_key".into(),
        owner_type: attune_common::models::OwnerType::System,
        owner_identity: None,
        owner_ref: None,
        encrypted: true,
    };
    assert!(Admission::input_key_origin_allowed(
        &mut tx,
        source.id,
        &origin,
        attune_common::rbac::Action::Decrypt
    )
    .await
    .unwrap());
    assert_eq!(inherited.authority().policy_id, original.policy_revision);
    assert_eq!(inherited.authority().attribution, attribution);
    assert_ne!(inherited.authority().attempt, original.authority.attempt);
    let retry = ExecutionRepository::create_pinned(&mut *tx, input, &snapshot)
        .await
        .unwrap();
    let retry_admission = inherited
        .attach(&mut tx, RuntimeOwner::Execution(retry.id))
        .await
        .unwrap();
    assert_ne!(retry_admission.id, original.id);
    assert_eq!(
        Admission::require(&mut tx, RuntimeOwner::Execution(source.id))
            .await
            .unwrap()
            .id,
        original.id
    );
    assert!(
        Admission::workflow_child(&mut tx, source.id, "next", &task_snapshot, &refs)
            .await
            .is_ok()
    );
    assert!(
        Admission::workflow_child(&mut tx, source.id, "undeclared", &task_snapshot, &refs)
            .await
            .is_err()
    );
    use attune_common::repositories::workflow::{
        CreateWorkflowExecutionInput, WorkflowExecutionRepository,
    };
    let workflow = WorkflowExecutionRepository::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: source.id,
            workflow_def: snapshot.executable.workflow_definition.as_ref().unwrap().id,
            task_graph: json!({}),
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    let child_input = CreateExecutionInput {
        action: Some(task_snapshot.executable.action.id),
        action_ref: task_snapshot.executable.action.r#ref.clone(),
        executor: Some(human),
        parent: Some(source.id),
        workflow_task: Some(attune_common::models::WorkflowTaskMetadata {
            workflow_execution: workflow.id,
            task_name: "next".into(),
            triggered_by: None,
            task_index: Some(0),
            task_batch: None,
            retry_count: 0,
            max_retries: 0,
            next_retry_at: None,
            timeout_seconds: None,
            timed_out: false,
            duration_ms: None,
            started_at: None,
            completed_at: None,
        }),
        permission_set_refs: refs.clone(),
        status: ExecutionStatus::Requested,
        ..Default::default()
    };
    let child = Admission::create_workflow_task_if_absent_pinned_with_conn(
        &mut tx,
        child_input.clone(),
        &task_snapshot,
        workflow.id,
        "next",
        Some(0),
    )
    .await
    .unwrap();
    assert!(child.created);
    assert!(Admission::input_key_origin_allowed(
        &mut tx,
        child.execution.id,
        &origin,
        attune_common::rbac::Action::Decrypt
    )
    .await
    .unwrap());
    let admitted = Admission::require(&mut tx, RuntimeOwner::Execution(child.execution.id))
        .await
        .unwrap();
    let mut duplicate_input = child_input;
    duplicate_input.permission_set_refs = vec!["foreign.admin".into()];
    duplicate_input.config = Some(json!({"unused":"{{ keystore['foreign.secret'] }}"}));
    let original_child = serde_json::to_value(&child.execution).unwrap();
    let duplicate = Admission::create_workflow_task_if_absent_pinned_with_conn(
        &mut tx,
        duplicate_input,
        &task_snapshot,
        workflow.id,
        "next",
        Some(0),
    )
    .await
    .unwrap();
    assert!(!duplicate.created);
    assert_eq!(duplicate.execution.id, child.execution.id);
    assert_eq!(
        serde_json::to_value(&duplicate.execution).unwrap(),
        original_child
    );
    let retained_duplicate =
        Admission::require(&mut tx, RuntimeOwner::Execution(child.execution.id))
            .await
            .unwrap();
    assert_eq!(
        retained_duplicate.authority_digest,
        admitted.authority_digest
    );
    assert_eq!(retained_duplicate.authority.0, admitted.authority.0);
    assert_eq!(
        Admission::require(&mut tx, RuntimeOwner::Execution(child.execution.id))
            .await
            .unwrap()
            .id,
        admitted.id
    );
    let disabled_new_invocation = Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &[],
        attribution,
        attune_common::repositories::runtime_target::RuntimeTarget::Component(
            attune_common::repositories::runtime_target::ComponentTarget::Action,
            snapshot.executable.action.id,
        ),
    )
    .await;
    assert!(disabled_new_invocation.is_err());
    Authority::revoke_admission(&mut tx, original.id)
        .await
        .unwrap();
    assert!(
        Admission::inherit(&mut tx, RuntimeOwner::Execution(source.id), &snapshot)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn workflow_state_insert_paths_require_an_actual_admissible_parent() {
    use attune_common::repositories::workflow::{
        CreateWorkflowExecutionInput, WorkflowExecutionRepository,
    };
    let (db, snapshot, _, human) = fixture().await;
    for status in [
        ExecutionStatus::Requested,
        ExecutionStatus::Scheduling,
        ExecutionStatus::Scheduled,
        ExecutionStatus::Running,
        ExecutionStatus::Canceling,
        ExecutionStatus::Cancelled,
        ExecutionStatus::Completed,
        ExecutionStatus::Failed,
        ExecutionStatus::Timeout,
        ExecutionStatus::Abandoned,
    ] {
        let admissible = matches!(
            status,
            ExecutionStatus::Requested
                | ExecutionStatus::Scheduling
                | ExecutionStatus::Scheduled
                | ExecutionStatus::Running
        );
        for entry in 0..3 {
            let parent = ExecutionRepository::create_pinned(
                &db,
                CreateExecutionInput {
                    action: Some(snapshot.executable.action.id),
                    action_ref: snapshot.executable.action.r#ref.clone(),
                    status,
                    executor: Some(human),
                    config: Some(json!({})),
                    ..Default::default()
                },
                &snapshot,
            )
            .await
            .unwrap();
            let input = CreateWorkflowExecutionInput {
                execution: parent.id,
                workflow_def: snapshot.executable.workflow_definition.as_ref().unwrap().id,
                task_graph: json!({}),
                variables: json!({}),
                status: ExecutionStatus::Running,
            };
            let result = match entry {
                0 => WorkflowExecutionRepository::create(&db, input)
                    .await
                    .map(|row| row.id),
                1 => WorkflowExecutionRepository::create_or_get_by_execution(&db, input)
                    .await
                    .map(|row| row.workflow_execution.id),
                _ => {
                    let mut transaction = db.begin().await.unwrap();
                    let result = WorkflowExecutionRepository::create_or_get_by_execution_with_conn(
                        &mut transaction,
                        input,
                    )
                    .await
                    .map(|row| row.workflow_execution.id);
                    transaction.commit().await.unwrap();
                    result
                }
            };
            if admissible {
                assert!(result.is_ok(), "{status:?}, entry {entry}: {result:?}");
            } else {
                assert!(
                    matches!(result, Err(attune_common::Error::PermissionDenied(_))),
                    "{status:?}, entry {entry}: {result:?}"
                );
                assert!(
                    WorkflowExecutionRepository::find_by_execution(&db, parent.id)
                        .await
                        .unwrap()
                        .is_none()
                );
            }
        }
    }
    let absent = CreateWorkflowExecutionInput {
        execution: i64::MAX,
        workflow_def: snapshot.executable.workflow_definition.as_ref().unwrap().id,
        task_graph: json!({}),
        variables: json!({}),
        status: ExecutionStatus::Running,
    };
    assert!(matches!(
        WorkflowExecutionRepository::create_or_get_by_execution(&db, absent).await,
        Err(attune_common::Error::PermissionDenied(_))
    ));
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn existing_cancelled_workflow_state_is_reused_without_resurrection_or_rebinding() {
    use attune_common::repositories::{
        execution::UpdateExecutionInput,
        workflow::{
            CreateWorkflowExecutionInput, UpdateWorkflowExecutionInput, WorkflowExecutionRepository,
        },
    };
    let (db, snapshot, _, human) = fixture().await;
    let parent = ExecutionRepository::create_pinned(
        &db,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            executor: Some(human),
            config: Some(json!({})),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let input = CreateWorkflowExecutionInput {
        execution: parent.id,
        workflow_def: snapshot.executable.workflow_definition.as_ref().unwrap().id,
        task_graph: json!({"original":true}),
        variables: json!({"original":"retained"}),
        status: ExecutionStatus::Running,
    };
    let first = WorkflowExecutionRepository::create_or_get_by_execution(&db, input.clone())
        .await
        .unwrap();
    assert!(first.created);
    let mut cancellation = db.begin().await.unwrap();
    let cancelled = WorkflowExecutionRepository::update(
        &mut *cancellation,
        first.workflow_execution.id,
        UpdateWorkflowExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ExecutionRepository::update(
        &mut *cancellation,
        parent.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    cancellation.commit().await.unwrap();
    let changed = CreateWorkflowExecutionInput {
        task_graph: json!({"must_not_replace":true}),
        variables: json!({"unused":"{{ keystore['foreign.not_selected'] }}"}),
        ..input
    };
    let existing = WorkflowExecutionRepository::create_or_get_by_execution(&db, changed.clone())
        .await
        .unwrap();
    assert!(!existing.created);
    assert_eq!(
        serde_json::to_value(&existing.workflow_execution).unwrap(),
        serde_json::to_value(&cancelled).unwrap()
    );
    let mut transaction = db.begin().await.unwrap();
    let existing = WorkflowExecutionRepository::create_or_get_by_execution_with_conn(
        &mut transaction,
        changed,
    )
    .await
    .unwrap();
    assert!(!existing.created);
    assert_eq!(
        serde_json::to_value(&existing.workflow_execution).unwrap(),
        serde_json::to_value(&cancelled).unwrap()
    );
    transaction.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn recorded_admission_rejects_ref_and_executable_substitution() {
    let (db, snapshot, policy, human) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let attribution = RequestAttribution::Human {
        actor: HumanAttribution {
            identity_id: human,
            login: "requester".into(),
        },
    };
    let empty = Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &[],
        attribution.clone(),
        attune_common::repositories::runtime_target::RuntimeTarget::Component(
            attune_common::repositories::runtime_target::ComponentTarget::Action,
            snapshot.executable.action.id,
        ),
    )
    .await
    .unwrap();
    assert!(!empty.authority().has_api_access());
    assert!(Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &["foreign.admin".into()],
        attribution.clone(),
        attune_common::repositories::runtime_target::RuntimeTarget::Component(
            attune_common::repositories::runtime_target::ComponentTarget::Action,
            snapshot.executable.action.id
        )
    )
    .await
    .is_err());
    let mut substituted = snapshot.clone();
    substituted.release.digest = "c".repeat(64);
    assert!(Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &substituted,
        &[],
        attribution,
        attune_common::repositories::runtime_target::RuntimeTarget::Component(
            attune_common::repositories::runtime_target::ComponentTarget::Action,
            snapshot.executable.action.id
        )
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn accepted_mixed_requester_queue_keeps_policy_and_attribution_after_human_deletion() {
    use attune_common::{
        models::WorkQueueItemStatus,
        repositories::{
            work_queue::{CreateWorkQueueItemInput, WorkQueueItemRepository, WorkQueueRepository},
            Delete, FindByRef,
        },
    };
    let (db, snapshot, policy, human) = fixture().await;
    let other = IdentityFixture::new_unique("other_requester")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let queue =
        WorkQueueRepository::find_by_ref(&mut *tx, &format!("{}.inbox", policy.owner.pack_ref))
            .await
            .unwrap()
            .unwrap();
    let mut items = Vec::new();
    for identity in [human, other.id] {
        let item = WorkQueueItemRepository::create_pinned(
            &mut *tx,
            CreateWorkQueueItemInput {
                queue: queue.id,
                queue_ref: queue.r#ref.clone(),
                item_key: None,
                priority: 0,
                status: WorkQueueItemStatus::Queued,
                payload: json!({}),
                metadata: json!({}),
                trace_tag: None,
                enqueue_source: "test".into(),
                requested_by_identity: Some(identity),
                requested_by_execution: None,
                requested_by_enforcement: None,
                leased_execution: None,
                lease_token: None,
                lease_expires_at: None,
                attempt_count: 0,
                last_error: None,
                ack_summary: None,
            },
            &snapshot,
        )
        .await
        .unwrap();
        Admission::admit_queue_item(
            &mut tx,
            &queue,
            &item,
            Some(HumanAttribution {
                identity_id: identity,
                login: format!("historical-{identity}"),
            }),
            &snapshot,
        )
        .await
        .unwrap();
        items.push(item);
    }
    let old = Admission::queue_batch(&mut tx, &queue, &items, &snapshot)
        .await
        .unwrap();
    let mut replacement = policy.clone();
    replacement.invocations[2].permission_sets.clear();
    let new = Authority::capture_policy(&mut tx, &replacement)
        .await
        .unwrap();
    let mut activation = sqlx::Connection::begin(&mut *tx).await.unwrap();
    assert!(
        Authority::activate_policy(&mut activation, new.id)
            .await
            .is_err(),
        "A's drain guard must retain accepted queue policy pins"
    );
    activation.rollback().await.unwrap();
    tx.commit().await.unwrap();
    IdentityRepository::delete(&db, human).await.unwrap();
    IdentityRepository::delete(&db, other.id).await.unwrap();
    let mut tx = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let selected = Admission::queue_batch(&mut tx, &queue, &items, &snapshot)
        .await
        .unwrap();
    assert_eq!(selected.authority().policy_id, old.authority().policy_id);
    match &selected.authority().attribution {
        RequestAttribution::QueueBatch { items: actors, .. } => {
            assert_eq!(actors.len(), 2);
            assert_eq!(actors[&items[0].id].as_ref().unwrap().identity_id, human);
            assert_eq!(actors[&items[1].id].as_ref().unwrap().identity_id, other.id);
        }
        _ => panic!("Expected batch attribution"),
    }
    let dispatch = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            executor: Some(selected.authority().owner.sponsor_identity_id),
            permission_set_refs: selected.authority().selected_refs.clone(),
            status: ExecutionStatus::Requested,
            config: Some(json!({})),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let dispatch_admission = selected
        .attach(&mut tx, RuntimeOwner::Execution(dispatch.id))
        .await
        .unwrap();
    assert_eq!(
        dispatch_admission.policy_revision,
        old.authority().policy_id
    );
    assert_eq!(
        dispatch_admission.authority.attribution,
        selected.authority().attribution
    );
    let mut revoked = sqlx::Connection::begin(&mut *tx).await.unwrap();
    Authority::revoke_policy(&mut revoked, old.authority().policy_id)
        .await
        .unwrap();
    assert!(
        Admission::queue_batch(&mut revoked, &queue, &items, &snapshot)
            .await
            .is_err()
    );
    revoked.rollback().await.unwrap();
    for item in &items {
        attune_common::repositories::work_queue::WorkQueueItemRepository::update_if_statuses(&mut *tx, item.id,
            attune_common::repositories::work_queue::WorkQueueItemRepository::mutable_pending_statuses(),
            attune_common::repositories::work_queue::UpdateWorkQueueItemInput { status: Some(WorkQueueItemStatus::Cancelled), ..Default::default() }).await.unwrap().unwrap();
    }
    // Activate only in a new graph-first transaction after the accepted items
    // have drained. Already recorded dispatch authority still retains old pins.
    tx.commit().await.unwrap();
    let mut activation = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(
        &mut activation,
    )
    .await
    .unwrap();
    Authority::activate_policy(&mut activation, new.id)
        .await
        .unwrap();
    let retained = Admission::require(&mut activation, RuntimeOwner::Execution(dispatch.id))
        .await
        .unwrap();
    assert_eq!(retained.policy_revision, old.authority().policy_id);
    activation.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn payload_editor_must_cover_accepted_inputs_without_rebinding_or_original_liveness() {
    use attune_common::{
        delegation::DelegationAuthority,
        rbac::Action,
        repositories::{
            recorded_input_authority::RecordedInputAuthorityRepository as Inputs,
            work_queue::{UpdateWorkQueueItemInput, WorkQueueItemRepository},
            FindById,
        },
    };
    let (db, snapshot, policy, original) = fixture().await;
    let editor = IdentityFixture::new_unique("input_editor")
        .create(&db)
        .await
        .unwrap();
    let denied = IdentityFixture::new_unique("input_denied")
        .create(&db)
        .await
        .unwrap();
    let literal = IdentityFixture::new_unique("literal_requester")
        .create(&db)
        .await
        .unwrap();
    assign_input_grants(&db, original, "accepted_edit_inputs", json!([
        {"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":["system.input_key"]}},
        {"resource":"packs","actions":["read","decrypt"],"constraints":{"refs":[policy.owner.pack_ref]}}
    ])).await;
    assign_input_grants(&db, editor.id, "editor_inputs", json!([
        {"resource":"keys","actions":["read","decrypt"]}, {"resource":"packs","actions":["read","decrypt"]}
    ])).await;
    assign_input_grants(&db, denied.id, "denied_inputs", json!([
        {"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":["system.other_key"]}},
        {"resource":"packs","actions":["read","decrypt"]}
    ])).await;
    let item = pending_literal(&db, &snapshot, original).await;
    let literal_item = pending_literal(&db, &snapshot, literal.id).await;
    let mut tx = db.begin().await.unwrap();
    Inputs::capture_human(&mut tx, "queue_item", item.id, original)
        .await
        .unwrap();
    Inputs::capture_human(&mut tx, "queue_item", literal_item.id, literal.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    IdentityRepository::update(
        &db,
        original,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let mut tx = db.begin().await.unwrap();
    // Source IAM is captured/locked before item mutation locks, as in the API.
    let denied_editor = DelegationAuthority::load_for_share(&mut tx, denied.id)
        .await
        .unwrap();
    let denied_target = WorkQueueItemRepository::find_by_id_for_update(&mut tx, item.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        Inputs::authorize_queue_payload_edit(&mut tx, item.id, &[denied_editor])
            .await
            .is_err()
    );
    assert_eq!(denied_target.payload, item.payload);
    tx.rollback().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let allowed_editor = DelegationAuthority::load_for_share(&mut tx, editor.id)
        .await
        .unwrap();
    WorkQueueItemRepository::find_by_id_for_update(&mut tx, item.id)
        .await
        .unwrap()
        .unwrap();
    Inputs::authorize_queue_payload_edit(&mut tx, item.id, &[allowed_editor])
        .await
        .unwrap();
    WorkQueueItemRepository::update_if_statuses(
        &mut *tx,
        item.id,
        WorkQueueItemRepository::mutable_pending_statuses(),
        UpdateWorkQueueItemInput {
            payload: Some(json!({"message":"authorized edit"})),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        Inputs::capture_human(&mut tx, "queue_item", item.id, editor.id)
            .await
            .is_err()
    );
    let retained = Inputs::load(&mut tx, "queue_item", item.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.identity_id, original);
    let mut key = attune_common::secret_provenance::KeyOrigin {
        key_id: 999,
        key_ref: "system.input_key".into(),
        owner_type: attune_common::models::OwnerType::System,
        owner_identity: None,
        owner_ref: None,
        encrypted: true,
    };
    assert!(retained.key_allows(&key, Action::Decrypt));
    key.key_ref = "system.other_key".into();
    assert!(!retained.key_allows(&key, Action::Read));
    // Literal inputs with no accepted Key/Pack authority need no unrelated grant.
    Inputs::authorize_queue_payload_edit(&mut tx, literal_item.id, &[])
        .await
        .unwrap();
    WorkQueueItemRepository::update_if_statuses(
        &mut *tx,
        literal_item.id,
        WorkQueueItemRepository::mutable_pending_statuses(),
        UpdateWorkQueueItemInput {
            payload: Some(json!({"message":"literal edit"})),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        WorkQueueItemRepository::find_by_id(db.pool(), item.id)
            .await
            .unwrap()
            .unwrap()
            .payload,
        json!({"message":"authorized edit"})
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn payload_editor_cannot_reinterpret_another_requesters_self_scope() {
    use attune_common::{
        delegation::DelegationAuthority,
        repositories::recorded_input_authority::RecordedInputAuthorityRepository as Inputs,
    };
    let (db, snapshot, _, original) = fixture().await;
    let other = IdentityFixture::new_unique("self_editor")
        .create(&db)
        .await
        .unwrap();
    assign_input_grants(
        &db,
        original,
        "original_self",
        json!([{"resource":"keys","actions":["read"],"constraints":{"owner":"self"}}]),
    )
    .await;
    assign_input_grants(
        &db,
        other.id,
        "editor_self",
        json!([{"resource":"keys","actions":["read"],"constraints":{"owner":"self"}}]),
    )
    .await;
    let item = pending_literal(&db, &snapshot, original).await;
    let mut tx = db.begin().await.unwrap();
    Inputs::capture_human(&mut tx, "queue_item", item.id, original)
        .await
        .unwrap();
    let own = DelegationAuthority::load_for_share(&mut tx, original)
        .await
        .unwrap();
    Inputs::authorize_queue_payload_edit(&mut tx, item.id, &[own])
        .await
        .unwrap();
    let unrelated = DelegationAuthority::load_for_share(&mut tx, other.id)
        .await
        .unwrap();
    assert!(
        Inputs::authorize_queue_payload_edit(&mut tx, item.id, &[unrelated])
            .await
            .is_err()
    );
    let absolute = DelegationAuthority::new(
        other.id,
        Default::default(),
        serde_json::from_value(json!([{"resource":"keys","actions":["read"]}])).unwrap(),
    );
    Inputs::authorize_queue_payload_edit(&mut tx, item.id, &[absolute])
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn recorded_local_key_input_uses_verified_owner_after_requester_freeze() {
    use attune_common::repositories::{
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        recorded_input_authority::RecordedInputAuthorityRepository,
        Create,
    };
    use helpers::KeyFixture;
    let (db, snapshot, policy, human) = fixture().await;
    let key = KeyFixture::new_pack(
        policy.owner.pack_id,
        &policy.owner.pack_ref,
        "accepted_local",
        "value",
    )
    .create(&db)
    .await
    .unwrap();
    let set = PermissionSetRepository::create(
        &db,
        CreatePermissionSetInput {
            r#ref: format!("{}.input_reader", policy.owner.pack_ref),
            pack: Some(policy.owner.pack_id),
            pack_ref: Some(policy.owner.pack_ref.clone()),
            label: None,
            description: None,
            grants: json!([{"resource":"keys","actions":["read","decrypt"]}]),
        },
    )
    .await
    .unwrap();
    PermissionAssignmentRepository::create(
        &db,
        CreatePermissionAssignmentInput {
            identity: human,
            permset: set.id,
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let admission = Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: human,
                login: "accepted".into(),
            },
        },
        attune_common::repositories::runtime_target::RuntimeTarget::Component(
            attune_common::repositories::runtime_target::ComponentTarget::Action,
            snapshot.executable.action.id,
        ),
    )
    .await
    .unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            executor: Some(human),
            permission_set_refs: vec![],
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    admission
        .attach(&mut tx, RuntimeOwner::Execution(execution.id))
        .await
        .unwrap();
    RecordedInputAuthorityRepository::capture_human(&mut tx, "execution", execution.id, human)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    IdentityRepository::update(
        &db,
        human,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let origin = attune_common::key_access::key_origin(&key);
    assert!(Admission::input_key_origin_allowed(
        &mut tx,
        execution.id,
        &origin,
        attune_common::rbac::Action::Read
    )
    .await
    .unwrap());
    let mut forged = origin.clone();
    forged.owner_ref = Some("not_the_captured_owner".into());
    assert!(!Admission::input_key_origin_allowed(
        &mut tx,
        execution.id,
        &forged,
        attune_common::rbac::Action::Read
    )
    .await
    .unwrap());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn workflow_materialization_requires_the_exact_retained_task_input_site() {
    use attune_common::repositories::{
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        runtime_target::{ComponentTarget, RuntimeTarget},
    };
    let (db, snapshot, policy, human) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let admitted = Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: human,
                login: "accepted".into(),
            },
        },
        RuntimeTarget::Component(ComponentTarget::Action, snapshot.executable.action.id),
    )
    .await
    .unwrap();
    let parent = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            executor: Some(human),
            permission_set_refs: vec![],
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
    let target = format!("{}.leaf", policy.owner.pack_ref);
    assert!(
        Materialize::workflow_sites(&mut tx, parent.id, "next", &target, &json!({}))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(Materialize::workflow_sites(
        &mut tx,
        parent.id,
        "next",
        &target,
        &json!({"injected":"{{ keystore['system.not_declared'] }}"})
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn recorded_task_schema_ignores_same_ref_release_only_parent_cache() {
    use attune_common::repositories::{
        runtime_target::{ComponentTarget, RuntimeTarget},
        schema_contract::{BindingOwner, SchemaContractRepository, StaticActionSchemaBinding},
    };
    let (db, snapshot, policy, human) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let admitted = Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: human,
                login: "accepted".into(),
            },
        },
        RuntimeTarget::Component(ComponentTarget::Action, snapshot.executable.action.id),
    )
    .await
    .unwrap();
    let parent = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            executor: Some(human),
            permission_set_refs: vec![],
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
    let leaf_ref = format!("{}.leaf", policy.owner.pack_ref);
    let actual = Admission::resolve_workflow_snapshot(&mut tx, parent.id, "next", &leaf_ref)
        .await
        .unwrap();
    let mut other_metadata = actual.clone();
    other_metadata.executable.action.param_schema =
        Some(json!({"wrong_revision":{"type":"integer","required":true}}));
    let other = SchemaContractRepository::materialize_action_pair(&mut tx, &other_metadata)
        .await
        .unwrap();
    let mut root = SchemaContractRepository::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    root.static_actions.insert(
        leaf_ref,
        StaticActionSchemaBinding {
            input: other.input.clone(),
            output: other.output,
            executable_snapshot: serde_json::to_value(other_metadata).unwrap(),
        },
    );
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, parent.id, &root)
        .await
        .unwrap();
    let exact = Admission::workflow_schema(&mut tx, parent.id, &actual)
        .await
        .unwrap();
    assert!(
        attune_common::value_schema::CompiledContract::new(&exact.input)
            .unwrap()
            .validate(&json!({}))
            .is_ok()
    );
    assert!(
        attune_common::value_schema::CompiledContract::new(&other.input)
            .unwrap()
            .validate(&json!({}))
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn qualified_enforcement_dedup_preserves_timestamp_twins_and_dispatch_source() {
    use attune_common::{
        event_schema::EventSchemaBinding,
        models::{EnforcementCondition, EnforcementStatus},
        repositories::{
            event::{
                CreateEnforcementInput, CreateEventInput, EnforcementRepository, EventRepository,
            },
            event_origin::EventOriginRepository,
            rule::{CreateRuleInput, RuleRepository},
            Create,
        },
    };
    let (db, snapshot, policy, human) = fixture().await;
    let action = &snapshot.executable.action;
    let trigger = helpers::TriggerFixture::new(
        Some(action.pack),
        Some(action.pack_ref.clone()),
        "qualified",
    )
    .create(&db)
    .await
    .unwrap();
    let rule = RuleRepository::create(
        &db,
        CreateRuleInput {
            r#ref: format!("{}.qualified", action.pack_ref),
            pack: action.pack,
            pack_ref: action.pack_ref.clone(),
            label: "qualified".into(),
            description: None,
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!([]),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: Some(vec![]),
            enabled: true,
            is_adhoc: true,
            owner_identity: Some(human),
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let first = EventRepository::create(
        &mut *tx,
        CreateEventInput {
            trigger: Some(trigger.id),
            trigger_ref: trigger.r#ref.clone(),
            payload: Some(json!({"twin":"first"})),
            config: None,
            trace_tag: None,
            source: None,
            source_ref: None,
            rule: None,
            rule_ref: None,
        },
    )
    .await
    .unwrap();
    let binding = EventSchemaBinding {
        trigger_id: Some(trigger.id),
        trigger_ref: trigger.r#ref.clone(),
        payload: None,
        config: None,
        webhook: false,
    };
    EventOriginRepository::capture(&mut tx, &first, &binding, &[], &[])
        .await
        .unwrap();
    let second_created = first.created + chrono::Duration::seconds(1);
    sqlx::query(
        "INSERT INTO event(id,created,trigger,trigger_ref,payload) VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(first.id)
    .bind(second_created)
    .bind(trigger.id)
    .bind(&trigger.r#ref)
    .bind(json!({"twin":"second"}))
    .execute(&mut *tx)
    .await
    .unwrap();
    let second = EventRepository::find_by_identity(&mut *tx, first.id, second_created)
        .await
        .unwrap()
        .unwrap();
    EventOriginRepository::capture(&mut tx, &second, &binding, &[], &[])
        .await
        .unwrap();
    let input = |event: &attune_common::models::Event| CreateEnforcementInput {
        rule: Some(rule.id),
        rule_ref: rule.r#ref.clone(),
        trigger_ref: trigger.r#ref.clone(),
        config: Some(json!({})),
        event: Some(event.id),
        status: EnforcementStatus::Created,
        payload: event.payload.clone().unwrap(),
        condition: EnforcementCondition::All,
        conditions: json!([]),
    };
    let a = EnforcementRepository::create_or_get_by_qualified_event_pinned(
        &mut tx,
        input(&first),
        &snapshot,
        &first,
    )
    .await
    .unwrap();
    let b = EnforcementRepository::create_or_get_by_qualified_event_pinned(
        &mut tx,
        input(&second),
        &snapshot,
        &second,
    )
    .await
    .unwrap();
    assert!(a.created && b.created);
    assert_ne!(a.enforcement.id, b.enforcement.id);
    let duplicate = EnforcementRepository::create_or_get_by_qualified_event_pinned(
        &mut tx,
        input(&first),
        &snapshot,
        &first,
    )
    .await
    .unwrap();
    assert!(!duplicate.created);
    assert_eq!(duplicate.enforcement.id, a.enforcement.id);
    let mut wrong = input(&first);
    wrong.event = Some(first.id + 1);
    assert!(
        EnforcementRepository::create_or_get_by_qualified_event_pinned(
            &mut tx, wrong, &snapshot, &first
        )
        .await
        .is_err()
    );
    tx.commit().await.unwrap();
    assert_eq!(
        EnforcementRepository::source_event(&*db, &a.enforcement)
            .await
            .unwrap()
            .unwrap()
            .created,
        first.created
    );
    assert_eq!(
        EnforcementRepository::source_event(&*db, &b.enforcement)
            .await
            .unwrap()
            .unwrap()
            .created,
        second.created
    );
    // A historical ID-only link cannot choose either accepted twin.
    let legacy = EnforcementRepository::create(&*db, input(&first))
        .await
        .unwrap();
    assert!(EnforcementRepository::source_event(&*db, &legacy)
        .await
        .is_err());
    assert_eq!(policy.owner.pack_id, action.pack);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn accepted_generic_inputs_survive_freeze_without_live_widening_or_contributor_union() {
    use attune_common::repositories::{
        native_input_origin::NativeInputOriginRepository,
        recorded_input_authority::RecordedInputAuthorityRepository as Inputs,
        runtime_read::{RuntimeReadOrigin, RuntimeReadRepository, RuntimeReadTarget},
        runtime_target::{ComponentTarget, RuntimeTarget},
        schema_contract::{BindingOwner, SchemaContractRepository},
    };
    let (db, snapshot, policy, human) = fixture().await;
    let other = IdentityFixture::new_unique("generic_other")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let selection = Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: human,
                login: "accepted".into(),
            },
        },
        RuntimeTarget::Component(ComponentTarget::Action, snapshot.executable.action.id),
    )
    .await
    .unwrap();
    let mut executions = Vec::new();
    for _ in 0..3 {
        let execution = ExecutionRepository::create_pinned(
            &mut *tx,
            CreateExecutionInput {
                action: Some(snapshot.executable.action.id),
                action_ref: snapshot.executable.action.r#ref.clone(),
                status: ExecutionStatus::Running,
                executor: Some(human),
                permission_set_refs: vec![],
                config: Some(json!({})),
                ..Default::default()
            },
            &snapshot,
        )
        .await
        .unwrap();
        let record = selection
            .attach(&mut tx, RuntimeOwner::Execution(execution.id))
            .await
            .unwrap();
        let binding = SchemaContractRepository::materialize_action_pair(&mut tx, &snapshot)
            .await
            .unwrap();
        SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
            .await
            .unwrap();
        executions.push((execution, record));
    }
    let target = executions[0].0.id;
    let consumer = executions[1].0.id;
    let late = executions[2].0.id;
    tx.commit().await.unwrap();
    assign_input_grants(
        &db,
        human,
        "generic_input_purpose",
        json!([
            {"resource":"executions","actions":["read","decrypt"],"constraints":{"ids":[target]}}
        ]),
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    Inputs::capture_human(&mut tx, "execution", consumer, human)
        .await
        .unwrap();
    Inputs::capture_human(&mut tx, "execution", late, other.id)
        .await
        .unwrap();
    // Immutable registry fixture representing a successful generic delivery.
    let read = serde_json::to_value(RuntimeReadOrigin {
        target: RuntimeReadTarget::Execution { execution: target },
        provenance: vec![],
        parents: vec![],
    })
    .unwrap();
    sqlx::query("INSERT INTO execution_runtime_read_origin(admission,origin_digest,origin) VALUES ($1,$2,$3)")
            .bind(executions[1].1.id).bind(fixture_digest(&read)).bind(&read).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    // New current rights after acceptance are not retroactive input authority.
    assign_input_grants(
        &db,
        other.id,
        "generic_late_rights",
        json!([
            {"resource":"executions","actions":["read","decrypt"],"constraints":{"ids":[target]}}
        ]),
    )
    .await;
    IdentityRepository::update(
        &db,
        human,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let closure = NativeInputOriginRepository::capture(&mut tx, consumer, &[])
        .await
        .unwrap();
    assert_eq!(closure.origins().len(), 1);
    let accepted = Inputs::load(&mut tx, "execution", consumer)
        .await
        .unwrap()
        .unwrap();
    let late_accepted = Inputs::load(&mut tx, "execution", late)
        .await
        .unwrap()
        .unwrap();
    assert!(attune_common::repositories::recorded_input_origins::RecordedInputOriginRepository::require_runtime_reads(&mut tx, &[accepted.clone(), late_accepted.clone()], closure.origins()).await.is_err());
    assert!(attune_common::repositories::recorded_input_origins::RecordedInputOriginRepository::require_runtime_reads(&mut tx, &[late_accepted], closure.origins()).await.is_err());
    assert!(attune_common::repositories::recorded_input_origins::RecordedInputOriginRepository::require_runtime_reads(&mut tx, &[accepted], closure.origins()).await.is_ok());
    let refs = RuntimeReadRepository::references_for_executions(&mut *tx, &[consumer])
        .await
        .unwrap();
    assert_eq!(refs[&consumer], closure.origins());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn child_defaults_use_the_child_declaration_and_never_reinterpret_supplied_strings() {
    use attune_common::repositories::{
        declared_input::DeclaredInputRepository,
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        runtime_target::{ComponentTarget, RuntimeTarget},
        schema_contract::{BindingOwner, SchemaContractRepository},
    };
    let (db, snapshot, policy, human) = fixture_with_defaults(true).await;
    let mut tx = db.begin().await.unwrap();
    let configuration = attune_common::repositories::component_configuration::ComponentConfigurationRepository::current(
        &mut *tx, "pack", policy.owner.pack_id,
    ).await.unwrap().unwrap();
    let origin = attune_common::secret_provenance::SecretOrigin::Entity {
        entity_type:
            attune_common::repositories::component_configuration::ENTITY_COMPONENT_CONFIGURATION
                .into(),
        entity_id: configuration.id,
        path: "/default_value".into(),
    };
    let admitted = Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: human,
                login: "accepted".into(),
            },
        },
        RuntimeTarget::Component(ComponentTarget::Action, snapshot.executable.action.id),
    )
    .await
    .unwrap();
    let binding = SchemaContractRepository::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    for supplied in [
        json!({}),
        json!({"token":"{{ keystore['system.not_an_authored_selector'] }}"}),
    ] {
        let execution = ExecutionRepository::create_pinned(
            &mut *tx,
            CreateExecutionInput {
                action: Some(snapshot.executable.action.id),
                action_ref: snapshot.executable.action.r#ref.clone(),
                status: ExecutionStatus::Requested,
                executor: Some(human),
                permission_set_refs: vec![],
                config: None,
                ..Default::default()
            },
            &snapshot,
        )
        .await
        .unwrap();
        let admission = admitted
            .attach(&mut tx, RuntimeOwner::Execution(execution.id))
            .await
            .unwrap();
        SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
            .await
            .unwrap();
        let (config, secrets) = Materialize::workflow_child_defaults(
            &mut tx,
            execution.id,
            &supplied,
            vec![],
            Some("child-default-test-owned-encryption-key"),
        )
        .await
        .unwrap();
        assert!(secrets.is_empty());
        if supplied.get("token").is_none() {
            assert_eq!(config["token"], "child-default-value");
        } else {
            assert_eq!(config["token"], supplied["token"]);
        }
        let prepared = attune_common::secret_values::prepare_secret_values(
            secrets.clone(),
            "child-default-test-owned-encryption-key",
        )
        .unwrap();
        ExecutionRepository::initialize_config_with_conn(
            &mut tx,
            execution.id,
            Some(admission.id),
            &snapshot,
            Some(config),
            &prepared,
        )
        .await
        .unwrap();
        let delivered =
            DeclaredInputRepository::delivery_allowed(&mut tx, execution.id, "/token", &origin)
                .await
                .unwrap();
        if supplied.get("token").is_none() {
            assert!(delivered);
        } else {
            assert!(!delivered);
        }
        assert!(!DeclaredInputRepository::delivery_allowed(
            &mut tx,
            execution.id,
            "/other",
            &origin
        )
        .await
        .unwrap());
    }
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_variable_restoration_consumes_prepared_exact_receipts_and_ciphertext() {
    use attune_common::repositories::{
        declared_input::DeclaredInputRepository,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        native_input_origin::NativeInputOriginRepository as Native,
        runtime_target::{ComponentTarget, RuntimeTarget},
        workflow::{CreateWorkflowExecutionInput, WorkflowExecutionRepository},
    };
    use attune_common::secret_values::{
        prepare_secret_values, redact_secret_path_sources, SecretPathSource, SecretSource,
        ENTITY_WORKFLOW_VARIABLES,
    };
    let (db, snapshot, policy, human) = fixture_with_defaults(true).await;
    let mut tx = db.begin().await.unwrap();
    let admitted = Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: human,
                login: "accepted".into(),
            },
        },
        RuntimeTarget::Component(ComponentTarget::Action, snapshot.executable.action.id),
    )
    .await
    .unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            executor: Some(human),
            config: Some(json!({})),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    admitted
        .attach(&mut tx, RuntimeOwner::Execution(execution.id))
        .await
        .unwrap();
    let declaration = DeclaredInputRepository::bindings(&mut tx, execution.id)
        .await
        .unwrap()
        .into_iter()
        .find(|site| site.source_path == "/workflow/vars/selected")
        .unwrap();
    let key = "native-purpose-receipt-test-owned-encryption-key";
    let mut session = Inputs::begin_render(
        &mut tx,
        execution.id,
        &declaration.declaration_revision,
        &declaration.source_path,
        &declaration.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_configuration(&mut tx, &mut session, key)
        .await
        .unwrap();
    let rendered = Inputs::evaluate_with_keys(&mut tx, session, key)
        .await
        .unwrap();
    let site = InputSite {
        execution: execution.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Value,
        path: "/selected".into(),
    };
    let destination = Inputs::prepare_destination(&mut tx, &rendered, &site, key)
        .await
        .unwrap();
    let receipt = Inputs::record_prepared(
        &mut tx,
        &rendered,
        &destination,
        rendered.value(),
        rendered.selections(),
        key,
    )
    .await
    .unwrap();
    let (variables, secrets) = redact_secret_path_sources(
        json!({"selected": rendered.value()}),
        &[SecretPathSource {
            path: "/selected".into(),
            source: SecretSource::Bound(receipt.provenance()),
        }],
    );
    WorkflowExecutionRepository::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: execution.id,
            workflow_def: snapshot.executable.workflow_definition.as_ref().unwrap().id,
            task_graph: json!({}),
            variables: variables.clone(),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    let stored = prepare_secret_values(secrets, key).unwrap();
    attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut tx, ENTITY_WORKFLOW_VARIABLES, execution.id, &stored,
    ).await.unwrap();
    // This is the actual native source entry: source proofs precede capture and
    // buffer locks, and no input receipt is moved to a fake config/result site.
    let prepared = Native::capture_with_key(&mut tx, execution.id, &[], Some(key))
        .await
        .unwrap();
    let plaintext = prepared
        .restore_prepared(
            &mut tx,
            ENTITY_WORKFLOW_VARIABLES,
            execution.id,
            variables.clone(),
            Some(key),
        )
        .await
        .unwrap();
    assert_eq!(plaintext, json!({"selected":"child-default-value"}));
    prepared.ensure_fresh_on(&mut tx).await.unwrap();
    // Even with the same valid receipt/provenance, changed ACTUAL ciphertext is
    // not the selected source buffer and cannot be restored after the wait.
    let mut changed = stored;
    changed[0].encrypted_value =
        attune_common::crypto::encrypt_json(&json!("changed"), key).unwrap();
    attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut tx, ENTITY_WORKFLOW_VARIABLES, execution.id, &changed,
    ).await.unwrap();
    assert!(matches!(
        prepared
            .restore_prepared(
                &mut tx,
                ENTITY_WORKFLOW_VARIABLES,
                execution.id,
                variables,
                Some(key)
            )
            .await,
        Err(attune_common::Error::PermissionDenied(_))
    ));
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn initial_native_variables_prepare_before_state_insert_with_literal_dynamic_and_system_proofs(
) {
    run_native_initial_variable_case(None).await;
}

async fn run_native_initial_variable_case(case: Option<native_staged_completion::Case>) {
    use attune_common::repositories::{
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        execution_secret_value::ExecutionSecretValueRepository,
        native_input_origin::NativeInputOriginRepository as Native,
        native_workflow_materialization::NativeWorkflowMaterializationRepository as Variables,
        recorded_input_authority::RecordedInputAuthorityRepository,
        runtime_target::{ComponentTarget, RuntimeTarget},
        schema_contract::{BindingOwner, SchemaContractRepository},
        workflow::{CreateWorkflowExecutionInput, WorkflowExecutionRepository},
        FindById,
    };
    let (db, snapshot, policy, human) = fixture_with_native_variant(true, false, case).await;
    let key = "initial-native-variables-test-owned-encryption-key";
    let authored_key = helpers::KeyFixture::new_pack(
        policy.owner.pack_id,
        &policy.owner.pack_ref,
        "initial_variable",
        "authored variable key",
    )
    .create(&db)
    .await
    .unwrap();
    let supplied = "{{ keystore['foreign.never_selected'] }}";
    if case.is_some() {
        native_staged_completion::prepare_issuer(&db, human, supplied).await;
    }
    let mut tx = db.begin().await.unwrap();
    let admitted = Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: human,
                login: "accepted".into(),
            },
        },
        RuntimeTarget::Component(ComponentTarget::Action, snapshot.executable.action.id),
    )
    .await
    .unwrap();
    let (config, input) = attune_common::secret_values::redact_secret_path_sources(
        json!({"value":supplied,"key_ref":authored_key.r#ref,"condition_value":"false"}),
        &[attune_common::secret_values::SecretPathSource {
            path: "/value".into(),
            source: attune_common::secret_values::SecretSource::ParameterSchema {
                path: "/value".into(),
            },
        }],
    );
    let input = attune_common::secret_values::prepare_secret_values(input, key).unwrap();
    let parent = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            executor: Some(human),
            config: Some(config),
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
    let binding = SchemaContractRepository::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, parent.id, &binding)
        .await
        .unwrap();
    RecordedInputAuthorityRepository::capture_human(&mut tx, "execution", parent.id, human)
        .await
        .unwrap();
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        attune_common::secret_values::ENTITY_EXECUTION_CONFIG,
        parent.id,
        &input,
    )
    .await
    .unwrap();
    let accepted = Materialize::seal_execution_inputs(&mut tx, parent.id, key)
        .await
        .unwrap();
    let source = Native::capture_with_key(&mut tx, parent.id, &[], Some(key))
        .await
        .unwrap();
    assert!(
        WorkflowExecutionRepository::find_by_execution(&mut *tx, parent.id)
            .await
            .unwrap()
            .is_none()
    );
    let prepared = Variables::prepare_initial_variables(&mut tx, parent.id, key)
        .await
        .unwrap();
    assert!(
        WorkflowExecutionRepository::find_by_execution(&mut *tx, parent.id)
            .await
            .unwrap()
            .is_none()
    );
    let state = WorkflowExecutionRepository::create_or_get_by_execution_with_conn(
        &mut tx,
        CreateWorkflowExecutionInput {
            execution: parent.id,
            workflow_def: snapshot.executable.workflow_definition.as_ref().unwrap().id,
            task_graph: if case.is_some() {
                native_staged_completion::saved_graph(&snapshot)
            } else {
                json!({})
            },
            variables: prepared.variables().clone(),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    assert!(state.created);
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut tx,
        attune_common::secret_values::ENTITY_WORKFLOW_VARIABLES,
        parent.id,
        prepared.secrets(),
    )
    .await
    .unwrap();
    prepared.ensure_fresh_on(&mut tx).await.unwrap();
    source.ensure_fresh_on(&mut tx).await.unwrap();
    for receipt in &accepted {
        receipt.ensure_fresh_on(&mut tx).await.unwrap();
    }
    let delivery = Native::capture_with_key(&mut tx, parent.id, &[], Some(key))
        .await
        .unwrap();
    let restored = delivery
        .restore_prepared(
            &mut tx,
            attune_common::secret_values::ENTITY_WORKFLOW_VARIABLES,
            parent.id,
            state.workflow_execution.variables.clone(),
            Some(key),
        )
        .await
        .unwrap();
    assert_eq!(restored["literal"], "public literal");
    assert_eq!(restored["copied_literal"], "public literal");
    assert_eq!(restored["selected"], "child-default-value");
    assert_eq!(restored["dynamic"], "child-default-value");
    assert_eq!(restored["supplied"], supplied);
    assert_eq!(restored["keyed"], "authored variable key");
    assert_eq!(
        restored["started"],
        serde_json::to_value(parent.created).unwrap()
    );
    assert!(parent.permission_set_refs.is_empty());
    tx.commit().await.unwrap();
    prepared.ensure_fresh().unwrap();
    source.ensure_fresh().unwrap();
    delivery.ensure_fresh().unwrap();
    for receipt in &accepted {
        receipt.ensure_fresh().unwrap();
    }
    use attune_common::repositories::{
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        native_transition_materialization::NativeTransitionMaterializationRepository as Transitions,
        workflow::UpdateWorkflowExecutionInput,
    };
    let leaf = snapshot
        .pack_executables
        .get(&format!("{}.leaf", policy.owner.pack_ref))
        .unwrap();
    let task_snapshot = ExecutionExecutableSnapshot {
        release: leaf.release.clone(),
        executable: leaf.executable.clone(),
        pack_executables: snapshot.pack_executables.clone(),
    };
    let mut creation = db.begin().await.unwrap();
    let mut child = Admission::create_workflow_task_if_absent_pinned_with_conn(
        &mut creation,
        CreateExecutionInput {
            action: Some(task_snapshot.executable.action.id),
            action_ref: task_snapshot.executable.action.r#ref.clone(),
            parent: Some(parent.id),
            executor: Some(human),
            // Completed-at-insert is a credential-free cache sentinel, not an
            // admitted task outcome. Create the real task before completing it.
            status: ExecutionStatus::Requested,
            config: Some(json!({})),
            workflow_task: Some(attune_common::models::WorkflowTaskMetadata {
                workflow_execution: state.workflow_execution.id,
                task_name: "next".into(),
                task_index: None,
                task_batch: None,
                triggered_by: None,
                retry_count: 0,
                max_retries: 0,
                next_retry_at: None,
                timeout_seconds: None,
                timed_out: false,
                duration_ms: None,
                started_at: None,
                completed_at: None,
            }),
            ..Default::default()
        },
        &task_snapshot,
        state.workflow_execution.id,
        "next",
        None,
    )
    .await
    .unwrap();
    let binding = SchemaContractRepository::materialize_action_pair(&mut creation, &task_snapshot)
        .await
        .unwrap();
    SchemaContractRepository::attach(
        &mut creation,
        BindingOwner::Execution,
        child.execution.id,
        &binding,
    )
    .await
    .unwrap();
    let child_admission =
        Admission::require(&mut creation, RuntimeOwner::Execution(child.execution.id))
            .await
            .unwrap();
    assert!(matches!(
        &child_admission.authority.binding,
        InvocationBinding::WorkflowTask { .. }
    ));
    ExecutionRepository::initialize_config_with_conn(
        &mut creation,
        child.execution.id,
        Some(child_admission.id),
        &task_snapshot,
        Some(json!({})),
        &[],
    )
    .await
    .unwrap();
    let child_inputs = Materialize::seal_execution_inputs(&mut creation, child.execution.id, key)
        .await
        .unwrap();
    child.execution = ExecutionRepository::update(
        &mut *creation, child.execution.id,
        attune_common::repositories::execution::UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            result: Some(json!({"data":{"message":"actual task result","collision":"data loses"},"collision":"envelope wins","exit_code":0})),
            ..Default::default()
        },
    ).await.unwrap();
    for receipt in &child_inputs {
        receipt.ensure_fresh_on(&mut creation).await.unwrap();
    }
    creation.commit().await.unwrap();
    for receipt in &child_inputs {
        receipt.ensure_fresh().unwrap();
    }

    if let Some(case) = case {
        native_staged_completion::check(
            case,
            &db,
            &snapshot,
            &parent,
            &child.execution,
            &state.workflow_execution,
            human,
            key,
            supplied,
        )
        .await;
        db.cleanup().await.unwrap();
        return;
    }

    let mut advance = db.begin().await.unwrap();
    // This is the same source-first repository entry used by the real completion
    // listener, before either the workflow advisory lock or its FOR UPDATE.
    let plan = Transitions::prepare_for_completion(&mut advance, &child.execution, key)
        .await
        .unwrap()
        .unwrap();
    assert!(plan.fires(0).unwrap());
    assert!(plan.fires(1).unwrap());
    assert!(!plan.fires(2).unwrap());
    assert!(
        plan.fires(3).unwrap(),
        "bare nonempty string is canonically truthy"
    );
    assert!(
        !plan.fires(4).unwrap(),
        "template string false uses the historical string coercion"
    );
    assert!(
        plan.fires(5).unwrap(),
        "a selected Key value is coerced in its one canonical pass"
    );
    let authored_next = &snapshot
        .executable
        .workflow_definition
        .as_ref()
        .unwrap()
        .definition["tasks"][0]["next"];
    let mut executable_transitions = serde_json::Value::Array(
        authored_next
            .as_array()
            .unwrap()
            .iter()
            .map(|transition| {
                let publish: Vec<_> = transition["publish"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|directive| {
                        directive
                            .as_object()
                            .unwrap()
                            .iter()
                            .map(|(name, value)| json!({"name":name,"value":value}))
                    })
                    .collect();
                json!({"when":transition.get("when"),"publish":publish,"do_tasks":transition["do"]})
            })
            .collect(),
    );
    plan.require_transitions(&executable_transitions).unwrap();
    executable_transitions[0]["when"] = json!("{{ true }}");
    assert!(plan.require_transitions(&executable_transitions).is_err());
    assert!(plan.control_sources().iter().all(|source| source.source.provenance().origins.iter().all(|origin|
        !matches!(origin, attune_common::secret_provenance::SecretOrigin::Entity { path, .. } if path.ends_with("/next/0/when")))));
    let unchanged = WorkflowExecutionRepository::find_by_execution(&mut *advance, parent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.variables, state.workflow_execution.variables);
    let mut stale = unchanged.clone();
    stale.variables = json!({"stale":true});
    assert!(plan.require_current(&stale).is_err());
    let locked = WorkflowExecutionRepository::find_by_id_for_update(&mut *advance, unchanged.id)
        .await
        .unwrap()
        .unwrap();
    plan.require_current(&locked).unwrap();
    let recorded = plan.record(&mut advance, &locked, key).await.unwrap();
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut advance,
        attune_common::secret_values::ENTITY_WORKFLOW_VARIABLES,
        parent.id,
        &recorded.secrets,
    )
    .await
    .unwrap();
    WorkflowExecutionRepository::update(
        &mut *advance,
        locked.id,
        UpdateWorkflowExecutionInput {
            variables: Some(recorded.variables.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let delivered = attune_common::secret_values::restore_secret_values(
        recorded.variables.clone(),
        &ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *advance,
            attune_common::secret_values::ENTITY_WORKFLOW_VARIABLES,
            parent.id,
        )
        .await
        .unwrap(),
        key,
    )
    .unwrap();
    assert_eq!(delivered["published"], supplied);
    assert_eq!(delivered["unconditional_literal"], json!({"accepted":true}));
    assert_eq!(delivered["unconditional_value"], supplied);
    assert_eq!(delivered["selected_key"], "authored variable key");
    assert_eq!(
        delivered["literal_publication"],
        json!({"value":"immutable literal"})
    );
    assert_eq!(delivered["task_publication"], "actual task result");
    assert!(delivered.get("must_not_publish").is_none());
    assert_eq!(delivered["bare_truthiness"], "bare nonempty string");
    assert_eq!(delivered["key_truthiness"], "canonical selected Key");
    assert!(delivered.get("must_not_template_publish").is_none());
    for (_, name, _, sources) in &recorded.publications {
        for source in sources {
            let attune_common::secret_values::SecretSource::Bound(provenance) = &source.source
            else {
                panic!("missing typed publication proof")
            };
            let site = InputSite {
                execution: parent.id,
                kind: InputEntityKind::WorkflowVariables,
                purpose: InputSitePurpose::Value,
                path: format!(
                    "/{}{}",
                    name.replace('~', "~0").replace('/', "~1"),
                    source.path
                ),
            };
            assert!(
                Inputs::delivery_allowed(&mut advance, &site, &provenance.origins[0], key)
                    .await
                    .unwrap()
            );
        }
    }
    let output_contract =
        SchemaContractRepository::find(&mut advance, BindingOwner::Execution, parent.id)
            .await
            .unwrap()
            .unwrap();
    let mut output = plan
        .output()
        .expect("terminal independent output plan")
        .clone();
    output["succeeded"] = json!(true);
    if let Some(contract) = &output_contract.output {
        if let Err(error) = attune_common::value_schema::CompiledContract::new(contract)
            .unwrap()
            .validate(&output)
        {
            output["output_validation_warning"] = json!({"code":error.code,"path":error.path});
        }
    }
    let mut wrong = output.clone();
    wrong["keyed_output"] = json!("not the canonical evaluated value");
    assert!(plan
        .record_output(&mut advance, output_contract.output.as_ref(), &wrong, key)
        .await
        .is_err());
    let mut extra = output.clone();
    extra["unproved_extra"] = json!("must not become native result data");
    assert!(plan
        .record_output(&mut advance, output_contract.output.as_ref(), &extra, key)
        .await
        .is_err());
    let output_sources = plan
        .record_output(&mut advance, output_contract.output.as_ref(), &output, key)
        .await
        .unwrap();
    let (redacted_output, output_secrets) =
        attune_common::secret_values::redact_secret_path_sources(output, &output_sources);
    let output_secrets =
        attune_common::secret_values::prepare_secret_values(output_secrets, key).unwrap();
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut advance,
        attune_common::secret_values::ENTITY_EXECUTION_RESULT,
        parent.id,
        &output_secrets,
    )
    .await
    .unwrap();
    ExecutionRepository::update(
        &mut *advance,
        parent.id,
        attune_common::repositories::execution::UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            result: Some(redacted_output),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for source in output_sources {
        let attune_common::secret_values::SecretSource::Bound(provenance) = source.source else {
            panic!("missing exact output proof")
        };
        let site = InputSite {
            execution: parent.id,
            kind: InputEntityKind::ExecutionResult,
            purpose: InputSitePurpose::Value,
            path: source.path,
        };
        assert!(
            Inputs::delivery_allowed(&mut advance, &site, &provenance.origins[0], key)
                .await
                .unwrap()
        );
        if site.path == "/collision_output" {
            let proof = Inputs::receipt_context(&mut advance, &site, &provenance.origins[0], key)
                .await
                .unwrap();
            assert!(proof
                .public_source_sites()
                .iter()
                .any(|source| source.execution == child.execution.id
                    && source.kind == InputEntityKind::ExecutionResult
                    && source.path == "/collision"));
            assert!(!proof
                .public_source_sites()
                .iter()
                .any(|source| source.execution == child.execution.id
                    && source.kind == InputEntityKind::ExecutionResult
                    && source.path == "/data/collision"));
        }
    }
    let actual_result = ExecutionRepository::find_by_id(&mut *advance, parent.id)
        .await
        .unwrap()
        .unwrap();
    let result_fields = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *advance,
        attune_common::secret_values::ENTITY_EXECUTION_RESULT,
        parent.id,
    )
    .await
    .unwrap();
    let restored_output = attune_common::secret_values::restore_secret_values(
        actual_result.result.unwrap(),
        &result_fields,
        key,
    )
    .unwrap();
    assert_eq!(restored_output["supplied_output"], supplied);
    assert_eq!(restored_output["keyed_output"], "authored variable key");
    assert_eq!(restored_output["result_output"], "actual task result");
    assert_eq!(
        restored_output["whole_result_output"],
        json!({"data":{"message":"actual task result","collision":"data loses"},
        "message":"actual task result","collision":"envelope wins","exit_code":0})
    );
    assert_eq!(restored_output["collision_output"], "envelope wins");
    assert_eq!(restored_output["envelope_output"], 0);
    assert_eq!(
        restored_output["literal_output"],
        "immutable output literal"
    );
    assert_eq!(restored_output["succeeded"], true);
    plan.ensure_fresh_on(&mut advance).await.unwrap();
    advance.commit().await.unwrap();
    plan.ensure_fresh().unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn empty_workflow_output_uses_persisted_variables_without_a_synthetic_child_or_outcome() {
    use attune_common::repositories::{
        cache::{CacheEntryRepository, CacheTransactionMode},
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        execution_secret_value::ExecutionSecretValueRepository,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        native_input_origin::NativeInputOriginRepository as Native,
        native_transition_materialization::NativeTransitionMaterializationRepository as Outputs,
        native_workflow_materialization::NativeWorkflowMaterializationRepository as Variables,
        recorded_input_authority::RecordedInputAuthorityRepository,
        runtime_target::{ComponentTarget, RuntimeTarget},
        schema_contract::{BindingOwner, SchemaContractRepository},
        workflow::{
            CreateWorkflowExecutionInput, UpdateWorkflowExecutionInput, WorkflowExecutionRepository,
        },
    };
    use attune_common::secret_values::{
        prepare_secret_values, redact_secret_path_sources, SecretPathSource, SecretSource,
        ENTITY_EXECUTION_CONFIG, ENTITY_EXECUTION_RESULT, ENTITY_WORKFLOW_VARIABLES,
    };
    let (db, snapshot, policy, human) = fixture_with_native_shape(true, true).await;
    let key = "empty-workflow-output-owned-encryption-key";
    let selected = helpers::KeyFixture::new_pack(
        policy.owner.pack_id,
        &policy.owner.pack_ref,
        "empty_output",
        "selected empty output key",
    )
    .create(&db)
    .await
    .unwrap();
    let supplied = "{{ keystore['foreign.never_evaluated'] }}";
    let (config, secret_inputs) = redact_secret_path_sources(
        json!({"value":supplied,"key_ref":selected.r#ref}),
        &[SecretPathSource {
            path: "/value".into(),
            source: SecretSource::ParameterSchema {
                path: "/value".into(),
            },
        }],
    );
    let mut initial = db.begin().await.unwrap();
    let selected = Admission::select(
        &mut initial,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: human,
                login: "empty accepted input".into(),
            },
        },
        RuntimeTarget::Component(ComponentTarget::Action, snapshot.executable.action.id),
    )
    .await
    .unwrap();
    let parent = ExecutionRepository::create_pinned(
        &mut *initial,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            executor: Some(human),
            status: ExecutionStatus::Running,
            config: Some(config),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    selected
        .attach(&mut initial, RuntimeOwner::Execution(parent.id))
        .await
        .unwrap();
    let schema = SchemaContractRepository::materialize_action_pair(&mut initial, &snapshot)
        .await
        .unwrap();
    SchemaContractRepository::attach(&mut initial, BindingOwner::Execution, parent.id, &schema)
        .await
        .unwrap();
    RecordedInputAuthorityRepository::capture_human(&mut initial, "execution", parent.id, human)
        .await
        .unwrap();
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut initial,
        ENTITY_EXECUTION_CONFIG,
        parent.id,
        &prepare_secret_values(secret_inputs, key).unwrap(),
    )
    .await
    .unwrap();
    let accepted = Materialize::seal_execution_inputs(&mut initial, parent.id, key)
        .await
        .unwrap();
    let variables = Variables::prepare_initial_variables(&mut initial, parent.id, key)
        .await
        .unwrap();
    let state = WorkflowExecutionRepository::create_or_get_by_execution_with_conn(&mut initial,
        CreateWorkflowExecutionInput { execution:parent.id,workflow_def:snapshot.executable.workflow_definition.as_ref().unwrap().id,
            task_graph:json!({"nodes":{},"entry_points":[],"inbound_edges":{},"outbound_edges":{}}),
            variables:variables.variables().clone(),status:ExecutionStatus::Running }).await.unwrap();
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut initial,
        ENTITY_WORKFLOW_VARIABLES,
        parent.id,
        variables.secrets(),
    )
    .await
    .unwrap();
    variables.ensure_fresh_on(&mut initial).await.unwrap();
    initial.commit().await.unwrap();
    variables.ensure_fresh().unwrap();
    for receipt in &accepted {
        receipt.ensure_fresh().unwrap();
    }

    let mut completion = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut completion, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Outputs::prepare_empty_workflow_output(&mut completion, parent.id, key)
        .await
        .unwrap()
        .unwrap();
    assert!(
        ExecutionRepository::find_by_parent(&mut *completion, parent.id)
            .await
            .unwrap()
            .is_empty()
    );
    let sources = Native::capture_workflow_with_key(&mut completion, parent.id, Some(key))
        .await
        .unwrap();
    let current = WorkflowExecutionRepository::find_by_id_for_update(
        &mut *completion,
        state.workflow_execution.id,
    )
    .await
    .unwrap()
    .unwrap();
    plan.require_current(&current).unwrap();
    let mut output = plan.output().unwrap().clone();
    output["succeeded"] = json!(true);
    if let Some(contract) = &schema.output {
        if let Err(error) = attune_common::value_schema::CompiledContract::new(contract)
            .unwrap()
            .validate(&output)
        {
            output["output_validation_warning"] = json!({"code":error.code,"path":error.path});
        }
    }
    let output_sources = plan
        .record_output(&mut completion, schema.output.as_ref(), &output, key)
        .await
        .unwrap();
    let (redacted, secret_inputs) = redact_secret_path_sources(output.clone(), &output_sources);
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut completion,
        ENTITY_EXECUTION_RESULT,
        parent.id,
        &prepare_secret_values(secret_inputs, key).unwrap(),
    )
    .await
    .unwrap();
    ExecutionRepository::update(
        &mut *completion,
        parent.id,
        attune_common::repositories::execution::UpdateExecutionInput {
            result: Some(redacted),
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    WorkflowExecutionRepository::update(
        &mut *completion,
        current.id,
        UpdateWorkflowExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for source in output_sources {
        let SecretSource::Bound(provenance) = source.source else {
            panic!("missing exact empty output receipt")
        };
        let site = InputSite {
            execution: parent.id,
            kind: InputEntityKind::ExecutionResult,
            purpose: InputSitePurpose::Value,
            path: source.path,
        };
        assert!(
            Inputs::delivery_allowed(&mut completion, &site, &provenance.origins[0], key)
                .await
                .unwrap()
        );
    }
    assert_eq!(output["supplied_output"], supplied);
    assert_eq!(output["variable_output"], supplied);
    assert_eq!(output["keyed_output"], "selected empty output key");
    assert_eq!(output["literal_output"], "immutable empty output literal");
    assert_eq!(
        output["started_output"],
        serde_json::to_value(parent.created).unwrap()
    );
    assert!(parent.permission_set_refs.is_empty());
    assert!(plan.control_sources().iter().all(|source| source.source.provenance().origins.iter().all(|origin|
        !matches!(origin, attune_common::secret_provenance::SecretOrigin::Entity { entity_type, .. } if entity_type.ends_with(":condition")))));
    plan.ensure_fresh_on(&mut completion).await.unwrap();
    sources.ensure_fresh_on(&mut completion).await.unwrap();
    completion.commit().await.unwrap();
    plan.ensure_fresh().unwrap();
    sources.ensure_fresh().unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn nested_queue_input_snapshots_keep_each_contributor_after_source_retention() {
    use attune_common::repositories::{
        recorded_input_authority::RecordedInputAuthorityRepository as Inputs, Delete,
    };
    let (db, snapshot, _, human) = fixture().await;
    let other = IdentityFixture::new_unique("nested_contributor")
        .create(&db)
        .await
        .unwrap();
    assign_input_grants(&db, human, "nested_first", json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":["system.first"]}}])).await;
    assign_input_grants(&db, other.id, "nested_second", json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":["system.second"]}}])).await;
    let a = pending_literal(&db, &snapshot, human).await;
    let b = pending_literal(&db, &snapshot, other.id).await;
    let next = pending_literal(&db, &snapshot, human).await;
    let mut tx = db.begin().await.unwrap();
    Inputs::capture_human(&mut tx, "queue_item", a.id, human)
        .await
        .unwrap();
    Inputs::capture_human(&mut tx, "queue_item", b.id, other.id)
        .await
        .unwrap();
    let input = || CreateExecutionInput {
        action: Some(snapshot.executable.action.id),
        action_ref: snapshot.executable.action.r#ref.clone(),
        status: ExecutionStatus::Requested,
        config: Some(json!({})),
        executor: None,
        permission_set_refs: vec![],
        ..Default::default()
    };
    let source = ExecutionRepository::create_pinned(&mut *tx, input(), &snapshot)
        .await
        .unwrap();
    Inputs::capture_queue_execution(&mut tx, source.id, &[a.id, b.id])
        .await
        .unwrap();
    Inputs::copy(&mut tx, "execution", source.id, "queue_item", next.id)
        .await
        .unwrap();
    assert!(Inputs::load(&mut tx, "queue_item", next.id)
        .await
        .unwrap()
        .is_none());
    let retained = Inputs::item_authorities(&mut tx, next.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.len(), 2);
    tx.commit().await.unwrap();
    IdentityRepository::update(
        &db,
        human,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    IdentityRepository::update(
        &db,
        other.id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ExecutionRepository::delete(&db, source.id).await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let target = ExecutionRepository::create_pinned(&mut *tx, input(), &snapshot)
        .await
        .unwrap();
    Inputs::capture_queue_execution(&mut tx, target.id, &[next.id])
        .await
        .unwrap();
    let authorities = Inputs::queue_authorities(&mut tx, target.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(authorities.len(), 2);
    assert_eq!(authorities[0].identity_id, human);
    assert_eq!(authorities[1].identity_id, other.id);
    let first = attune_common::secret_provenance::KeyOrigin {
        key_id: 1,
        key_ref: "system.first".into(),
        owner_type: attune_common::models::OwnerType::System,
        owner_identity: None,
        owner_ref: None,
        encrypted: true,
    };
    let mut second = first.clone();
    second.key_id = 2;
    second.key_ref = "system.second".into();
    assert!(authorities[0].key_allows(&first, attune_common::rbac::Action::Read));
    assert!(!authorities[0].key_allows(&second, attune_common::rbac::Action::Read));
    assert!(!authorities[1].key_allows(&first, attune_common::rbac::Action::Read));
    assert!(authorities[1].key_allows(&second, attune_common::rbac::Action::Read));
    // Payload editor coverage is checked against BOTH retained snapshots.
    assert!(
        Inputs::authorize_queue_payload_edit(&mut tx, next.id, &[authorities[0].clone()])
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_authored_site_selection_requires_exact_retained_path_and_value() {
    use attune_common::repositories::{
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        runtime_target::{ComponentTarget, RuntimeTarget},
    };
    let (db, snapshot, policy, human) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let selected = Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: human,
                login: "native-site".into(),
            },
        },
        RuntimeTarget::Component(ComponentTarget::Action, snapshot.executable.action.id),
    )
    .await
    .unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            executor: Some(human),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    selected
        .attach(&mut tx, RuntimeOwner::Execution(execution.id))
        .await
        .unwrap();
    let graph = &snapshot
        .executable
        .workflow_definition
        .as_ref()
        .unwrap()
        .definition;
    let authored = graph.pointer("/tasks/0/action").unwrap();
    assert!(Materialize::workflow_value_sites(
        &mut tx,
        execution.id,
        "/workflow/tasks/0/action",
        authored
    )
    .await
    .unwrap()
    .is_empty());
    assert!(Materialize::workflow_value_sites(
        &mut tx,
        execution.id,
        "/workflow/tasks/0/action",
        &json!("{{ keystore['foreign.supplied'] }}")
    )
    .await
    .is_err());
    assert!(Materialize::workflow_value_sites(
        &mut tx,
        execution.id,
        "/workflow/tasks/0/action_suffix",
        authored
    )
    .await
    .is_err());
    assert!(
        Materialize::workflow_value_sites(&mut tx, execution.id, "/config/action", authored)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn initial_input_projection_is_once_only_same_transaction_and_digest_only_history() {
    use attune_common::repositories::{
        runtime_target::{ComponentTarget, RuntimeTarget},
        schema_contract::{BindingOwner, SchemaContractRepository},
        FindById,
    };
    let (db, snapshot, policy, human) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    let selected = Admission::select(
        &mut tx,
        policy.owner.pack_id,
        policy.invocations[0].binding.clone(),
        &snapshot,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: human,
                login: "accepted".into(),
            },
        },
        RuntimeTarget::Component(ComponentTarget::Action, snapshot.executable.action.id),
    )
    .await
    .unwrap();
    let binding = SchemaContractRepository::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    let input = || CreateExecutionInput {
        action: Some(snapshot.executable.action.id),
        action_ref: snapshot.executable.action.r#ref.clone(),
        config: None,
        status: ExecutionStatus::Requested,
        executor: Some(human),
        permission_set_refs: vec![],
        ..Default::default()
    };
    let first = ExecutionRepository::create_pinned(&mut *tx, input(), &snapshot)
        .await
        .unwrap();
    let admission = selected
        .attach(&mut tx, RuntimeOwner::Execution(first.id))
        .await
        .unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, first.id, &binding)
        .await
        .unwrap();
    assert!(ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        first.id,
        Some(admission.id + 1),
        &snapshot,
        None,
        &[]
    )
    .await
    .is_err());
    let initialized = ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        first.id,
        Some(admission.id),
        &snapshot,
        None,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(initialized.config, Some(json!({})));
    assert!(ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        first.id,
        Some(admission.id),
        &snapshot,
        Some(json!({"replacement":"denied"})),
        &[]
    )
    .await
    .is_err());
    let history:serde_json::Value = sqlx::query_scalar("SELECT new_values->'config' FROM execution_history WHERE entity_id=$1 AND operation='UPDATE' AND 'config'=ANY(changed_fields) ORDER BY time DESC LIMIT 1")
        .bind(first.id).fetch_one(&mut *tx).await.unwrap();
    let expected: serde_json::Value = sqlx::query_scalar("SELECT _jsonb_digest_summary($1::JSONB)")
        .bind(json!({}))
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(history, expected);
    let retained = ExecutionRepository::create_pinned(&mut *tx, input(), &snapshot)
        .await
        .unwrap();
    let retained_admission = selected
        .attach(&mut tx, RuntimeOwner::Execution(retained.id))
        .await
        .unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, retained.id, &binding)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut later = db.begin().await.unwrap();
    // A later update changes xmin, but cannot change the insertion transaction.
    sqlx::query("UPDATE execution SET updated = clock_timestamp() WHERE id = $1")
        .bind(retained.id)
        .execute(&mut *later)
        .await
        .unwrap();
    assert!(ExecutionRepository::initialize_config_with_conn(
        &mut later,
        retained.id,
        Some(retained_admission.id),
        &snapshot,
        Some(json!({"late":"denied"})),
        &[]
    )
    .await
    .is_err());
    assert!(ExecutionRepository::find_by_id(&mut *later, retained.id)
        .await
        .unwrap()
        .unwrap()
        .config
        .is_none());
    later.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}
