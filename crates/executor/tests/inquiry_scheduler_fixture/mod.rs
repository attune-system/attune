// Included only in scheduler::tests. This does not sign or enable credentials.
const INQUIRY_SCHEDULER_INPUT_KEY: &str = "inquiry-scheduler-fixture-only-input-encryption";
const INQUIRY_WAIT_TARGET_PERMISSION: &str = "scheduler_inquiry_it.wait_target_execute";
const INQUIRY_WAIT_READ_PERMISSION: &str = "scheduler_inquiry_it.wait_source_read";

enum InquiryFixtureSource {
    Human(i64),
    Execution(i64),
}

async fn create_admitted_inquiry_fixture_executions(
    pool: &PgPool,
    pack: &attune_common::models::Pack,
    actions: &[Action],
    definition: &attune_common::models::workflow::WorkflowDefinition,
    graph: &TaskGraph,
    queue_id: i64,
) -> Result<(Execution, i64, Execution)> {
    use attune_common::{
        repositories::{
            action::UpdateActionInput,
            component_lifecycle::PackProjectionIds,
            execution_secret_value::ExecutionSecretValueRepository,
            identity::{
                CreatePermissionAssignmentInput, CreatePermissionSetInput,
                PermissionAssignmentRepository,
            },
            managed_executable::ManagedExecutableRepository,
            native_workflow_materialization::NativeWorkflowMaterializationRepository as Variables,
            runtime_authority::RuntimeAuthorityRepository,
            schema_type::SchemaTypeRepository,
        },
        runtime_authority::{
            InvocationBinding, InvocationPolicy, PermissionSetDefinition, RuntimePolicy,
        },
        secret_values::ENTITY_WORKFLOW_VARIABLES,
    };
    let workflow_action = actions
        .first()
        .ok_or_else(|| anyhow::anyhow!("inquiry fixture requires its workflow action"))?;
    let request_action = actions
        .get(1)
        .ok_or_else(|| anyhow::anyhow!("inquiry fixture requires its request action"))?;
    let guarded_action = actions
        .iter()
        .find(|action| action.r#ref == "scheduler_inquiry_it.guarded")
        .ok_or_else(|| anyhow::anyhow!("inquiry fixture requires its guarded wait action"))?;
    ActionRepository::link_workflow_def(pool, workflow_action.id, definition.id).await?;
    let requester = IdentityRepository::create(
        pool,
        CreateIdentityInput {
            login: "scheduler-inquiry-requester".into(),
            display_name: None,
            password_hash: None,
            attributes: serde_json::json!({}),
        },
    )
    .await?;
    // Author both the selected native capability and the original human's
    // source-read authority before any release, policy or input capture.
    // These sources have no authored stable IDs yet. Their actual pinned pack
    // ownership supplies the scope; native wait validation still checks the
    // exact workflow, descendant tree or requesting root for each target.
    let read_definition: PermissionSetDefinition = serde_json::from_value(serde_json::json!({
        "ref": INQUIRY_WAIT_READ_PERMISSION,
        "grants": [
            {"resource": "executions", "actions": ["read", "decrypt"],
             "constraints": {"pack_refs": [pack.r#ref.clone()]}},
            {"resource": "inquiries", "actions": ["read", "decrypt"],
             "constraints": {"pack_refs": [pack.r#ref.clone()]}},
            {"resource": "queue_items", "actions": ["read", "decrypt"],
             "constraints": {"pack_refs": [pack.r#ref.clone()]}}
        ]
    }))?;
    let read_permission = PermissionSetRepository::create(
        pool,
        CreatePermissionSetInput {
            r#ref: read_definition.reference.clone(),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: None,
            description: None,
            grants: serde_json::to_value(&read_definition.grants)?,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        pool,
        CreatePermissionAssignmentInput {
            identity: requester.id,
            permset: read_permission.id,
        },
    )
    .await?;
    // Every use of this action in the authored graph is a guarded wait. Root,
    // request and recovery actions retain their original empty read selections.
    ActionRepository::update(
        pool,
        guarded_action.id,
        UpdateActionInput {
            default_execution_permission_set_refs: Some(vec![INQUIRY_WAIT_READ_PERMISSION.into()]),
            ..Default::default()
        },
    )
    .await?;
    let mut deployment = pool.begin().await?;
    SchemaTypeRepository::lock_reclamation(&mut deployment).await?;
    let release = PackReleaseRepository::create_or_get(
        &mut deployment,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: "1.0.0".into(),
            digest: "a".repeat(64),
            object_key: "scheduler-inquiry-integration".into(),
            provider_version: "test".into(),
            content_path: "/tmp/opencode/scheduler-inquiry-integration".into(),
            archive_size: 1,
            manifest: serde_json::json!({}),
        },
    )
    .await?;
    PackReleaseRepository::activate_projected(
        &mut deployment,
        pack.id,
        release.id,
        &PackProjectionIds {
            actions: actions.iter().map(|action| action.id).collect(),
            permission_sets: vec![read_permission.id],
            work_queues: vec![queue_id],
            ..Default::default()
        },
    )
    .await?;
    let owner = RuntimeAuthorityRepository::ensure_owner(&mut deployment, pack.id).await?;
    ManagedExecutableRepository::capture_deployment_actions(&mut deployment, release.id).await?;
    // Only the workflow root can invoke this one local action as nested work.
    // Its workflow tasks do not inherit this runtime API selection.
    let target_definition: PermissionSetDefinition = serde_json::from_value(serde_json::json!({
        "ref": INQUIRY_WAIT_TARGET_PERMISSION,
        "grants": [{
            "resource": "actions",
            "actions": ["execute"],
            "constraints": {"refs": [request_action.r#ref.clone()]}
        }]
    }))?;
    let target_set = target_definition.compile(&owner, &[])?;
    let read_set = read_definition.compile(&owner, &[])?;
    let invocation = |binding| InvocationPolicy {
        metadata_revision: None,
        source_id: None,
        dispatch_template_digest: None,
        foreign_workflow_policy: None,
        binding,
        target_release_id: release.id,
        target_release_digest: release.digest.clone(),
        permission_sets: BTreeMap::new(),
        standard_access: false,
    };
    let mut invocations = Vec::new();
    for action in actions {
        let mut selected = invocation(InvocationBinding::Action {
            action_ref: action.r#ref.clone(),
        });
        if action.id == workflow_action.id {
            selected
                .permission_sets
                .insert(target_set.reference.clone(), target_set.clone());
        }
        invocations.push(selected);
    }
    let tasks = definition
        .definition
        .get("tasks")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| anyhow::anyhow!("inquiry fixture requires an authored task array"))?;
    for (index, task) in tasks.iter().enumerate() {
        let action_ref = task
            .get("action")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| anyhow::anyhow!("inquiry fixture requires static action targets"))?;
        let mut selected = invocation(InvocationBinding::WorkflowTask {
            workflow_ref: workflow_action.r#ref.clone(),
            task_path: format!("/tasks/{index}"),
            action_ref: action_ref.into(),
        });
        if action_ref == guarded_action.r#ref {
            anyhow::ensure!(
                task.get("wait_for").is_some_and(|wait| !wait.is_null()),
                "only authored guarded waits may select source-read authority"
            );
            selected
                .permission_sets
                .insert(read_set.reference.clone(), read_set.clone());
        }
        invocations.push(selected);
    }
    // No queue invocation is fabricated. This fixture observes persisted queue
    // item lifecycle metadata, never admits or dispatches the queue's workload.
    let policy = RuntimePolicy {
        owner,
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: BTreeMap::new(),
        invocations,
    };
    let revision = RuntimeAuthorityRepository::capture_policy(&mut deployment, &policy).await?;
    RuntimeAuthorityRepository::activate_policy(&mut deployment, revision.id).await?;
    deployment.commit().await?;

    let provisional_parent = accept_inquiry_fixture_action(
        pool,
        pack.id,
        workflow_action.id,
        &workflow_action.r#ref,
        InquiryFixtureSource::Human(requester.id),
        vec![INQUIRY_WAIT_TARGET_PERMISSION.into()],
        serde_json::json!({}),
    )
    .await?;
    let mut startup = pool.begin().await?;
    CacheEntryRepository::protect_transaction(&mut startup, CacheTransactionMode::PinMutation)
        .await?;
    SchemaTypeRepository::lock_selection(&mut startup).await?;
    let parent = ExecutionRepository::update(
        &mut *startup,
        provisional_parent.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
    )
    .await?;
    let variables =
        Variables::prepare_initial_variables(&mut startup, parent.id, INQUIRY_SCHEDULER_INPUT_KEY)
            .await?;
    let workflow = WorkflowExecutionRepository::create(
        &mut *startup,
        CreateWorkflowExecutionInput {
            execution: parent.id,
            workflow_def: definition.id,
            task_graph: serde_json::to_value(graph)?,
            variables: variables.variables().clone(),
            status: ExecutionStatus::Running,
        },
    )
    .await?;
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut startup,
        ENTITY_WORKFLOW_VARIABLES,
        parent.id,
        variables.secrets(),
    )
    .await?;
    let request_task = graph
        .get_task("request")
        .ok_or_else(|| anyhow::anyhow!("inquiry fixture requires its authored request task"))?;
    let context = WorkflowContext::new(serde_json::json!({}), HashMap::new());
    let mut messages = WorkflowPublications::default();
    let mut completions = Vec::new();
    ExecutionScheduler::dispatch_workflow_task_with_conn(
        &mut startup,
        &AtomicUsize::new(0),
        &parent,
        &workflow.id,
        request_task,
        &context,
        Some(INQUIRY_SCHEDULER_INPUT_KEY),
        None,
        &mut messages,
        &mut completions,
    )
    .await?;
    anyhow::ensure!(
        messages.len() == 1,
        "request task must create exactly one child"
    );
    anyhow::ensure!(
        completions.is_empty(),
        "request task must not be a terminal sentinel"
    );
    let created = ExecutionRepository::find_by_id(&mut *startup, messages[0].execution_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("persisted request child is missing"))?;
    anyhow::ensure!(
        created.parent == Some(parent.id)
            && created.action == Some(request_action.id)
            && created.workflow_task.as_ref().is_some_and(|task| {
                task.workflow_execution == workflow.id && task.task_name == "request"
            }),
        "request child must retain its actual authored workflow lineage"
    );
    let request = ExecutionRepository::update(
        &mut *startup,
        created.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
    )
    .await?;
    // Do not seal the scheduler-created request here. Production owns its
    // acceptance boundary. The regression below proves any missing seal.
    variables.ensure_fresh_on(&mut startup).await?;
    startup.commit().await?;
    variables.ensure_fresh()?;
    Ok((parent, workflow.id, request))
}

async fn accept_inquiry_fixture_action(
    pool: &PgPool,
    pack: i64,
    action: i64,
    action_ref: &str,
    source: InquiryFixtureSource,
    permission_set_refs: Vec<String>,
    supplied: JsonValue,
) -> Result<Execution> {
    use attune_common::{
        repositories::{
            accepted_input::AcceptedInputRepository,
            declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
            execution_admission::ExecutionAdmissionRepository as Admission,
            recorded_input_authority::RecordedInputAuthorityRepository,
            rule_authority::RuleAuthorityRepository,
            runtime_authority::{
                RuntimeAuthorityRepository, RuntimeCredentialBinding, RuntimeOwner,
            },
            runtime_read::RuntimeReadRepository,
            runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
            schema_contract::{BindingOwner, SchemaContractRepository},
            schema_type::SchemaTypeRepository,
        },
        runtime_authority::{HumanAttribution, InvocationBinding, RequestAttribution},
        secret_values::{
            merge_contract_secret_redactions, prepare_secret_values,
            validate_contract_secret_destination_paths,
        },
        value_schema::CompiledContract,
    };
    let mut acceptance = pool.begin().await?;
    SchemaTypeRepository::lock_selection(&mut acceptance).await?;
    let mut source_authority = None;
    let mut source_input_fence = None;
    let (executor, parent, attribution) = match source {
        InquiryFixtureSource::Human(identity) => {
            let human = IdentityRepository::find_by_id(&mut *acceptance, identity)
                .await?
                .filter(|human| !human.frozen)
                .ok_or_else(|| anyhow::anyhow!("inquiry fixture requester is unavailable"))?;
            (
                Some(human.id),
                None,
                RequestAttribution::Human {
                    actor: HumanAttribution {
                        identity_id: human.id,
                        login: human.login,
                    },
                },
            )
        }
        InquiryFixtureSource::Execution(parent_id) => {
            let record =
                Admission::require(&mut acceptance, RuntimeOwner::Execution(parent_id)).await?;
            // Bind validation to the real recorded attempt, without minting a
            // JWT or creating a credential from live human grants.
            let credential = RuntimeCredentialBinding {
                execution_id: parent_id,
                admission_id: record.id,
                sponsor_identity_id: record.authority.owner.sponsor_identity_id,
                action_ref: record.authority.binding.action_ref().into(),
                attempt: record.authority.attempt,
            };
            let authority = RuntimeAuthorityRepository::validate_execution_credential(
                &mut acceptance,
                &credential,
            )
            .await?;
            RuntimeReadRepository::require_untainted(&mut acceptance, record.id).await?;
            source_input_fence = Some(
                AcceptedInputRepository::revalidate_execution(
                    &mut acceptance,
                    parent_id,
                    INQUIRY_SCHEDULER_INPUT_KEY,
                )
                .await?,
            );
            let target = RuntimeTargetRepository::resolve(
                &mut acceptance,
                RuntimeTarget::Component(ComponentTarget::Action, action),
                record.authority.owner.sponsor_identity_id,
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("nested wait target action is unavailable"))?;
            anyhow::ensure!(
                authority.allows_new_effect(&target, attune_common::rbac::Action::Execute),
                "the admitted parent cannot invoke this nested wait target action"
            );
            let parent = ExecutionRepository::find_by_id(&mut *acceptance, parent_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("nested wait target parent is unavailable"))?;
            source_authority = Some(authority);
            (
                parent.executor,
                Some(parent_id),
                RequestAttribution::ParentExecution {
                    execution_id: parent_id,
                },
            )
        }
    };
    let invocation = InvocationBinding::Action {
        action_ref: action_ref.into(),
    };
    let snapshot = Admission::resolve_current_snapshot(
        &mut acceptance,
        pack,
        RuntimeTarget::Component(ComponentTarget::Action, action),
        &invocation,
    )
    .await?;
    if let Some(parent_id) = parent {
        let parent = ExecutionRepository::find_by_id(&mut *acceptance, parent_id)
            .await?
            .and_then(|parent| parent.executable_snapshot)
            .ok_or_else(|| anyhow::anyhow!("nested parent executable is unavailable"))?;
        let authority = source_authority
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("nested parent authority is unavailable"))?;
        attune_common::action_visibility::ensure_action_reference_allowed(
            &snapshot.executable.action,
            Some(&parent.executable.action.pack_ref),
            "execution",
            authority.authority().binding.action_ref(),
        )?;
    }
    let binding =
        SchemaContractRepository::materialize_action_pair(&mut acceptance, &snapshot).await?;
    let contract = CompiledContract::new(&binding.input)?;
    let selected = Admission::select(
        &mut acceptance,
        pack,
        invocation,
        &snapshot,
        &permission_set_refs,
        attribution,
        RuntimeTarget::Component(ComponentTarget::Action, action),
    )
    .await?;
    let provisional = ExecutionRepository::create_pinned(
        &mut *acceptance,
        CreateExecutionInput {
            action: Some(action),
            action_ref: action_ref.into(),
            executor,
            parent,
            permission_set_refs,
            config: None,
            status: ExecutionStatus::Requested,
            ..Default::default()
        },
        &snapshot,
    )
    .await?;
    let record = selected
        .attach(&mut acceptance, RuntimeOwner::Execution(provisional.id))
        .await?;
    if let Some(parent) = parent {
        RecordedInputAuthorityRepository::copy(
            &mut acceptance,
            "execution",
            parent,
            "execution",
            provisional.id,
        )
        .await?;
        RuleAuthorityRepository::copy_execution(&mut acceptance, parent, provisional.id).await?;
    } else {
        RecordedInputAuthorityRepository::capture_human(
            &mut acceptance,
            "execution",
            provisional.id,
            executor
                .ok_or_else(|| anyhow::anyhow!("human requester attribution is unavailable"))?,
        )
        .await?;
    }
    SchemaContractRepository::attach(
        &mut acceptance,
        BindingOwner::Execution,
        provisional.id,
        &binding,
    )
    .await?;
    let defaults = contract.apply_defaults(&supplied)?;
    let (values, declared) = Materialize::action_defaults(
        &mut acceptance,
        provisional.id,
        &supplied,
        defaults,
        Some(INQUIRY_SCHEDULER_INPUT_KEY),
    )
    .await?;
    contract.validate(&values)?;
    validate_contract_secret_destination_paths(
        &values,
        &contract,
        &declared
            .iter()
            .map(|source| source.path.clone())
            .collect::<Vec<_>>(),
    )?;
    let (redacted, secret_inputs) = merge_contract_secret_redactions(values, &declared, &contract)?;
    let prepared = prepare_secret_values(secret_inputs, INQUIRY_SCHEDULER_INPUT_KEY)?;
    ExecutionRepository::initialize_config_with_conn(
        &mut acceptance,
        provisional.id,
        Some(record.id),
        &snapshot,
        Some(redacted),
        &prepared,
    )
    .await?;
    let receipts = Materialize::seal_execution_inputs(
        &mut acceptance,
        provisional.id,
        INQUIRY_SCHEDULER_INPUT_KEY,
    )
    .await?;
    selected.ensure_fresh_on(&mut acceptance).await?;
    if let Some(authority) = &source_authority {
        authority.ensure_fresh_on(&mut acceptance).await?;
    }
    if let Some(fence) = &source_input_fence {
        fence.ensure_fresh_on(&mut acceptance).await?;
    }
    for receipt in &receipts {
        receipt.ensure_fresh_on(&mut acceptance).await?;
    }
    let execution = ExecutionRepository::find_by_id(&mut *acceptance, provisional.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("accepted inquiry fixture execution is missing"))?;
    acceptance.commit().await?;
    selected.effect_fence().ensure_fresh()?;
    if let Some(authority) = &source_authority {
        authority.ensure_fresh()?;
    }
    if let Some(fence) = &source_input_fence {
        fence.ensure_fresh()?;
    }
    for receipt in &receipts {
        receipt.ensure_fresh()?;
    }
    Ok(execution)
}

// Stop at the real child-admission boundary, before one-shot config initialization.
// The caller owns the transaction and must roll it back after each input case.
async fn create_uninitialized_inquiry_fixture_child(
    conn: &mut PgConnection,
    fixture: &InquirySchedulerFixture,
    task: &TaskNode,
) -> Result<(
    Execution,
    attune_common::repositories::schema_contract::ActionSchemaBinding,
)> {
    use attune_common::repositories::{
        execution_admission::ExecutionAdmissionRepository as Admission,
        runtime_authority::RuntimeOwner,
        schema_contract::{BindingOwner, SchemaContractRepository},
    };
    let action_ref = task
        .action
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("static fixture task required"))?;
    let snapshot = ExecutionScheduler::workflow_task_snapshot_with_conn(
        conn,
        &fixture.parent,
        action_ref,
        &task.name,
    )
    .await?;
    let binding = Admission::workflow_schema(conn, fixture.parent.id, &snapshot).await?;
    let metadata = WorkflowTaskMetadata {
        workflow_execution: fixture.workflow_execution_id,
        task_name: task.name.clone(),
        triggered_by: Some("request".into()),
        task_index: None,
        task_batch: None,
        retry_count: 0,
        max_retries: 0,
        next_retry_at: None,
        timeout_seconds: None,
        timed_out: false,
        duration_ms: None,
        started_at: None,
        completed_at: None,
    };
    let created = Admission::create_workflow_task_if_absent_pinned_with_conn(
        conn,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: action_ref.into(),
            parent: Some(fixture.parent.id),
            executor: fixture.parent.executor,
            enforcement: fixture.parent.enforcement,
            permission_set_refs: ExecutionScheduler::workflow_task_permission_set_refs(
                task,
                &snapshot.executable.action,
                &fixture.context,
            )?,
            config: None,
            status: ExecutionStatus::Requested,
            workflow_task: Some(metadata),
            ..Default::default()
        },
        &snapshot,
        fixture.workflow_execution_id,
        &task.name,
        None,
    )
    .await?;
    anyhow::ensure!(
        created.created,
        "input case requires its own newly admitted child"
    );
    let child = created.execution;
    anyhow::ensure!(child.config.is_none(), "child must still be uninitialized");
    Admission::require(conn, RuntimeOwner::Execution(child.id)).await?;
    anyhow::ensure!(
        ExecutionRepository::is_in_execution_tree(&mut *conn, fixture.parent.id, child.id, false)
            .await?,
        "admitted child must be an actual strict descendant"
    );
    SchemaContractRepository::attach(conn, BindingOwner::Execution, child.id, &binding).await?;
    Ok((child, binding))
}

#[tokio::test]
async fn inquiry_scheduler_authored_request_keeps_original_input_acceptance() {
    use attune_common::repositories::accepted_input::AcceptedInputRepository;
    let fixture = InquirySchedulerFixture::create().await;
    let mut validation = fixture.database.pool().begin().await.unwrap();
    let root = AcceptedInputRepository::revalidate_execution(
        &mut validation,
        fixture.parent.id,
        INQUIRY_SCHEDULER_INPUT_KEY,
    )
    .await
    .expect("fixture root has a genuine original acceptance seal");
    let request = AcceptedInputRepository::revalidate_execution(
        &mut validation,
        fixture.request.id,
        INQUIRY_SCHEDULER_INPUT_KEY,
    )
    .await;
    root.ensure_fresh_on(&mut validation).await.unwrap();
    validation.rollback().await.unwrap();
    fixture.database.cleanup().await.unwrap();
    request
        .expect("scheduler-created empty request input must retain its original acceptance seal");
}

#[tokio::test]
async fn inquiry_scheduler_wait_targets_keep_action_admission_and_parent_ownership() {
    use attune_common::{
        repositories::{
            accepted_input::AcceptedInputRepository,
            execution_admission::ExecutionAdmissionRepository as Admission,
            identity::UpdateIdentityInput, runtime_authority::RuntimeOwner,
        },
        runtime_authority::{InvocationBinding, RequestAttribution},
    };
    let fixture = InquirySchedulerFixture::create().await;
    let unrelated = fixture
        .create_execution(ExecutionStatus::Completed, false)
        .await;
    IdentityRepository::update(
        fixture.database.pool(),
        fixture.parent.executor.unwrap(),
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let owned = fixture
        .create_execution(ExecutionStatus::Completed, true)
        .await;
    assert_eq!(owned.parent, Some(fixture.parent.id));
    assert_eq!(unrelated.parent, None);
    assert!(owned.workflow_task.is_none());
    assert!(unrelated.workflow_task.is_none());
    let mut validation = fixture.database.pool().begin().await.unwrap();
    for target in [&owned, &unrelated] {
        let admission = Admission::require(&mut validation, RuntimeOwner::Execution(target.id))
            .await
            .unwrap();
        assert_eq!(
            admission.authority.binding,
            InvocationBinding::Action {
                action_ref: fixture.request.action_ref.clone(),
            }
        );
        if target.id == owned.id {
            assert_eq!(
                admission.authority.attribution,
                RequestAttribution::ParentExecution {
                    execution_id: fixture.parent.id,
                }
            );
        } else {
            assert!(matches!(
                &admission.authority.attribution,
                RequestAttribution::Human { .. }
            ));
        }
        AcceptedInputRepository::revalidate_execution(
            &mut validation,
            target.id,
            INQUIRY_SCHEDULER_INPUT_KEY,
        )
        .await
        .unwrap()
        .ensure_fresh_on(&mut validation)
        .await
        .unwrap();
    }
    validation.rollback().await.unwrap();
    let before = ExecutionRepository::find_by_parent(fixture.database.pool(), fixture.parent.id)
        .await
        .unwrap()
        .len();
    let guarded = &fixture
        .parent
        .executable_snapshot
        .as_ref()
        .unwrap()
        .pack_executables["scheduler_inquiry_it.guarded"]
        .executable
        .action;
    let denied = accept_inquiry_fixture_action(
        fixture.database.pool(),
        guarded.pack,
        guarded.id,
        &guarded.r#ref,
        InquiryFixtureSource::Execution(fixture.parent.id),
        Vec::new(),
        serde_json::json!({}),
    )
    .await
    .expect_err("nested authority is limited to the exact request action");
    assert!(denied.to_string().contains("cannot invoke"), "{denied}");
    assert_eq!(
        ExecutionRepository::find_by_parent(fixture.database.pool(), fixture.parent.id)
            .await
            .unwrap()
            .len(),
        before,
        "a rejected nested target must not insert a child"
    );
    fixture.database.cleanup().await.unwrap();
}

#[tokio::test]
async fn empty_wait_recovery_control_cold_reopen_rejects_value_reuse_and_missing_root() {
    use attune_common::repositories::{
        accepted_input::AcceptedInputRepository as Accepted,
        file_evidence_retention::FileEvidenceRetentionRepository as Retention,
        inherited_input::{InheritedInputRepository as Inputs, InputSitePurpose},
        schema_type::SchemaTypeRepository,
    };

    let fixture = InquirySchedulerFixture::create().await;
    let prepared: anyhow::Result<(i64, i64)> = async {
        let snapshot = fixture
            .request
            .executable_snapshot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("fixture request snapshot missing"))?;
        let action = fixture
            .request
            .action
            .ok_or_else(|| anyhow::anyhow!("fixture request action missing"))?;
        let target = accept_inquiry_fixture_action(
            fixture.database.pool(),
            snapshot.executable.action.pack,
            action,
            &fixture.request.action_ref,
            InquiryFixtureSource::Execution(fixture.parent.id),
            Vec::new(),
            serde_json::json!({}),
        )
        .await?;
        ExecutionRepository::update(
            fixture.database.pool(),
            target.id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Failed),
                result: Some(serde_json::json!({"target":true})),
                ..Default::default()
            },
        )
        .await?;
        let result = serde_json::json!({"execution_failed":target.id});
        ExecutionRepository::update(
            fixture.database.pool(),
            fixture.request.id,
            UpdateExecutionInput {
                result: Some(result.clone()),
                ..Default::default()
            },
        )
        .await?;
        let mut context = WorkflowContext::new(serde_json::json!({}), HashMap::new());
        context.set_task_result("request", result);
        let task = fixture
            .graph
            .get_task("execution_failed_guarded")
            .ok_or_else(|| anyhow::anyhow!("authored failed wait task missing"))?;
        let mut activation = fixture.database.pool().begin().await?;
        let activated: anyhow::Result<()> = async {
            let mut pending = WorkflowPublications::default();
            ExecutionScheduler::activate_workflow_task_with_conn(
                &mut activation,
                &AtomicUsize::new(0),
                &fixture.parent,
                &fixture.workflow_execution_id,
                task,
                &context,
                Some(INQUIRY_SCHEDULER_INPUT_KEY),
                Some("request"),
                &mut pending,
                &mut Vec::new(),
            )
            .await?;
            anyhow::ensure!(pending.is_empty());
            Ok(())
        }
        .await;
        if let Err(error) = activated {
            activation.rollback().await?;
            return Err(error);
        }
        activation.commit().await?;
        drop(context);

        let mut advancement = fixture.database.pool().begin().await?;
        let advanced: anyhow::Result<(i64, i64)> = async {
            let wait = WorkflowTaskWaitRepository::find_by_workflow_task(
                &mut *advancement,
                fixture.workflow_execution_id,
                "execution_failed_guarded",
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("actual failed wait missing"))?;
            let checkpoint = NativeWaitCheckpoint::prepare(
                &mut advancement,
                &wait,
                Some(INQUIRY_SCHEDULER_INPUT_KEY),
            )
            .await?;
            anyhow::ensure!(
                checkpoint.is_some(),
                "actual terminal checkpoint must prepare"
            );
            let outcome = ExecutionScheduler::advance_resolved_task_wait_with_conn(
                &mut advancement,
                &AtomicUsize::new(0),
                Some(INQUIRY_SCHEDULER_INPUT_KEY),
                &fixture.parent,
                fixture.workflow_execution_id,
                "execution_failed_guarded",
                checkpoint,
            )
            .await?;
            anyhow::ensure!(outcome.execution_requests.len() == 1);
            let request = &outcome.execution_requests[0];
            anyhow::ensure!(request.action_ref == "scheduler_inquiry_it.failure_handler");
            let child = request.execution_id;
            drop(outcome);
            Ok((child, wait.id))
        }
        .await;
        match advanced {
            Ok(ids) => {
                advancement.commit().await?;
                Ok(ids)
            }
            Err(error) => {
                advancement.rollback().await?;
                Err(error)
            }
        }
    }
    .await;
    let checked: anyhow::Result<()> = async {
        let (child, wait) = prepared?;
        let mut tx = fixture.database.pool().begin().await?;
        // This test owns a scoped deletion probe. Enter the exclusive collector
        // gate before source protection, never upgrade an existing source read.
        let result: anyhow::Result<()> = async {
            Retention::lock_reclamation(&mut tx).await?;
            CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation).await?;
            SchemaTypeRepository::lock_selection(&mut tx).await?;
            let execution = ExecutionRepository::find_by_id(&mut *tx, child).await?
                .ok_or_else(|| anyhow::anyhow!("actual recovery child missing"))?;
            anyhow::ensure!(execution.config.unwrap_or_else(|| serde_json::json!({})) == serde_json::json!({}));
            let fields = attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(
                &mut *tx, "execution_config", child,
            ).await?;
            anyhow::ensure!(fields.is_empty(), "CONTROL must survive without any VALUE field");
            let roots = Accepted::prepare_input_controls(&mut tx, child, INQUIRY_SCHEDULER_INPUT_KEY).await?;
            anyhow::ensure!(roots.len() == 1);
            let root = &roots[0];
            anyhow::ensure!(root.site().execution == child && root.site().path.is_empty());
            anyhow::ensure!(root.site().purpose == InputSitePurpose::Control);
            root.ensure_fresh_on(&mut tx).await?;
            Accepted::revalidate_execution(&mut tx, child, INQUIRY_SCHEDULER_INPUT_KEY)
                .await?.ensure_fresh_on(&mut tx).await?;
            let origin = root.origin();
            let root_id: i64 = sqlx::query_scalar(
                "SELECT id FROM runtime_input_receipt WHERE execution=$1 AND entity_kind='execution_config' AND destination_purpose='control' AND destination_path=''",
            ).bind(child).fetch_one(&mut *tx).await?;
            let encrypted: serde_json::Value = sqlx::query_scalar(
                "SELECT encrypted_value FROM runtime_input_receipt WHERE id=$1",
            ).bind(root_id).fetch_one(&mut *tx).await?;
            anyhow::ensure!(attune_common::crypto::decrypt_json(&encrypted, INQUIRY_SCHEDULER_INPUT_KEY)? == serde_json::json!({"wait":wait}));
            let mut value_site = root.site().clone();
            value_site.purpose = InputSitePurpose::Value;
            anyhow::ensure!(matches!(Inputs::receipt_context(&mut tx, &value_site, &origin, INQUIRY_SCHEDULER_INPUT_KEY).await,
                Err(attune_common::Error::PermissionDenied(_))),
                "a real CONTROL origin cannot authenticate a VALUE site");
            anyhow::ensure!(matches!(Accepted::seal_execution(&mut tx, child, INQUIRY_SCHEDULER_INPUT_KEY).await,
                Err(attune_common::Error::PermissionDenied(_))),
                "an existing saved body cannot waive the held-root requirement");
            drop(roots);

            let original: (serde_json::Value, String) = sqlx::query_as(
                "SELECT body,body_digest FROM runtime_accepted_input WHERE execution=$1",
            ).bind(child).fetch_one(&mut *tx).await?;
            sqlx::query("SAVEPOINT missing_actual_control_root").execute(&mut *tx).await?;
            let removed = sqlx::query("DELETE FROM runtime_input_receipt WHERE id=$1 AND execution=$2 AND entity_kind='execution_config' AND destination_purpose='control' AND destination_path=''")
                .bind(root_id).bind(child).execute(&mut *tx).await?.rows_affected();
            anyhow::ensure!(removed == 1, "probe must remove the actual root, not skip it");
            let unchanged: (serde_json::Value, String) = sqlx::query_as(
                "SELECT body,body_digest FROM runtime_accepted_input WHERE execution=$1",
            ).bind(child).fetch_one(&mut *tx).await?;
            anyhow::ensure!(unchanged == original, "probe must not rewrite or reseal the accepted body");
            anyhow::ensure!(matches!(Accepted::prepare_input_controls(&mut tx, child, INQUIRY_SCHEDULER_INPUT_KEY).await,
                Err(attune_common::Error::PermissionDenied(_))));
            anyhow::ensure!(matches!(Accepted::revalidate_execution(&mut tx, child, INQUIRY_SCHEDULER_INPUT_KEY).await,
                Err(attune_common::Error::PermissionDenied(_))),
                "unchanged authenticated body must reject its missing live root");
            sqlx::query("ROLLBACK TO SAVEPOINT missing_actual_control_root").execute(&mut *tx).await?;
            sqlx::query("RELEASE SAVEPOINT missing_actual_control_root").execute(&mut *tx).await?;
            let restored = Accepted::prepare_input_controls(&mut tx, child, INQUIRY_SCHEDULER_INPUT_KEY).await?;
            anyhow::ensure!(restored.len() == 1 && restored[0].origin() == origin);
            Accepted::revalidate_execution(&mut tx, child, INQUIRY_SCHEDULER_INPUT_KEY)
                .await?.ensure_fresh_on(&mut tx).await?;
            Ok(())
        }.await;
        tx.rollback().await?;
        result
    }.await;
    let cleanup = fixture.database.cleanup().await;
    cleanup.expect("clean owned database before reporting the probe result");
    checked.expect("cold empty-input CONTROL recovery and scoped missing-root rejection");
}
