//! A real released execution wait, variable/output receipts and HTTP readers.
//! No raw JSON is used as native evidence and no sealed input is rewritten.
use super::*;
use attune_common::{
    models::{OwnerType, WorkflowTaskWaitTarget},
    repositories::{
        action::{ActionRepository, UpdateActionInput},
        declared_input::DeclaredInputRepository,
        execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        immutable_input_origin::InputReceiptFrontier,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        key::{CreateKeyInput, KeyRepository, UpdateKeyInput},
        native_input_snapshot::NativeInputSnapshotRepository as Snapshots,
        pack::PackRepository,
        recorded_input_authority::RecordedInputAuthorityRepository,
        runtime_read::{RuntimeReadOrigin, RuntimeReadRepository, RuntimeReadTarget},
        runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            UpdateWorkflowExecutionInput, WorkflowDefinitionRepository,
            WorkflowExecutionRepository,
        },
        workflow_task_wait::{CreateWorkflowTaskWaitInput, WorkflowTaskWaitRepository},
        FindById,
    },
    runtime_authority::SelectedRuntimeAuthority,
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{redaction_marker, ENTITY_EXECUTION_RESULT},
};

pub(super) fn grant_digest(value: &impl serde::Serialize) -> Result<String> {
    use sha2::Digest;
    Ok(sha2::Sha256::digest(serde_jcs::to_vec(value)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Runtime executions have ownership scopes, not authored component-ref
/// selectors. Narrow their admitted structural grants to actual resolved rows
/// using the policy's retained concrete-target bindings, never authored IDs.
pub(super) async fn bind_execution_targets(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal: Id,
    sets: &BTreeMap<String, attune_common::runtime_authority::AdmittedPermissionSet>,
    selected: &[(&str, &[Id])],
) -> Result<BTreeMap<String, attune_common::runtime_authority::GrantTargetBindings>> {
    use attune_common::{
        rbac::Resource,
        runtime_authority::{BoundRuntimeTarget, GrantTargetBindings},
    };
    let mut bindings = BTreeMap::new();
    for (reference, ids) in selected {
        for grant in &sets[*reference].grants {
            if grant.grant().resource != Resource::Executions {
                continue;
            }
            let mut targets = std::collections::BTreeSet::new();
            let mut packs = std::collections::BTreeSet::new();
            for id in *ids {
                let target =
                    RuntimeTargetRepository::resolve(tx, RuntimeTarget::Execution(*id), principal)
                        .await?
                        .unwrap();
                assert_eq!(target.identity_kind(), "execution");
                assert_eq!(target.context().target_id, Some(*id));
                assert!(grant.grant().actions.iter().all(|action| grant.allows(
                    &Default::default(),
                    target.pack_id(),
                    Resource::Executions,
                    *action,
                    target.context()
                )));
                assert!(target.context().owner_type.is_none() && target.owner_id().is_none());
                let pack = target.pack_id().unwrap();
                packs.insert(pack);
                targets.insert(BoundRuntimeTarget {
                    kind: target.identity_kind().into(),
                    id: *id,
                    pack_id: Some(pack),
                    owner_type: None,
                    owner_id: None,
                });
            }
            assert!(!targets.is_empty());
            let key = grant_digest(grant)?;
            let binding = GrantTargetBindings {
                targets: Some(targets),
                owners: None,
                packs: Some(packs),
            };
            if let Some(previous) = bindings.insert(key, binding.clone()) {
                assert_eq!(
                    previous, binding,
                    "identical admitted grants must never merge different target sets"
                );
            }
        }
    }
    Ok(bindings)
}

pub(super) async fn capture_reader(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    pin: &attune_common::models::ExecutionExecutableSnapshot,
    selected: &SelectedRuntimeAuthority,
) -> Result<RuntimeCredentialBinding> {
    let execution = ExecutionRepository::create_pinned(
        &mut **tx,
        CreateExecutionInput {
            action: Some(pin.executable.action.id),
            action_ref: pin.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            config: Some(json!({})),
            permission_set_refs: selected.selected_refs.clone(),
            ..Default::default()
        },
        pin,
    )
    .await?;
    let schema = SchemaContractRepository::materialize_action_pair(tx, pin).await?;
    SchemaContractRepository::attach(tx, BindingOwner::Execution, execution.id, &schema).await?;
    let record = RuntimeAuthorityRepository::record_admission(
        tx,
        RuntimeOwner::Execution(execution.id),
        selected,
    )
    .await?;
    Ok(RuntimeCredentialBinding {
        execution_id: execution.id,
        admission_id: record.id,
        sponsor_identity_id: selected.owner.sponsor_identity_id,
        action_ref: selected.binding.action_ref().into(),
        attempt: selected.attempt,
    })
}

#[tokio::test]
async fn released_execution_wait_to_variable_output_http_keeps_archived_crypto_and_reader_ceiling(
) -> Result<()> {
    execution_wait(None).await
}

pub(super) async fn execution_wait(
    transport: Option<super::c_native_transport::Checkpoint>,
) -> Result<()> {
    let mut fixture = fixture(false).await?;
    let pack = PackRepository::find_by_id(&fixture.ctx.pool, fixture.pack_id)
        .await?
        .unwrap();
    let key = fixture
        .ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .unwrap();
    let leaf = create_test_action(
        &fixture.ctx.pool,
        pack.id,
        &pack.r#ref,
        "recorded_emitter.c_wait_value",
    )
    .await?;
    let graph = create_test_action(
        &fixture.ctx.pool,
        pack.id,
        &pack.r#ref,
        "recorded_emitter.c_wait_graph",
    )
    .await?;
    ActionRepository::update(
        &fixture.ctx.pool,
        leaf.id,
        UpdateActionInput {
            param_schema: Some(json!({"token":{"type":"string","secret":true,"required":false}})),
            out_schema: Some(json!({"token":{"type":"string","secret":true}})),
            ..Default::default()
        },
    )
    .await?;
    ActionRepository::update(
        &fixture.ctx.pool,
        graph.id,
        UpdateActionInput {
            // Revoking explicit read must not leave public-action metadata access.
            reference_visibility: Some(attune_common::models::ActionReferenceVisibility::Private),
            param_schema: Some(json!({})),
            out_schema: Some(json!({"token":{"type":"string","secret":true}})),
            ..Default::default()
        },
    )
    .await?;
    let mixed_case = matches!(
        transport,
        Some(super::c_native_transport::Checkpoint::Mixed { .. })
    );
    let mixed_cache = if mixed_case {
        Some(super::c_native_mixed::setup(&fixture, pack.id, &pack.r#ref).await?)
    } else {
        None
    };
    let contributor_case = mixed_case
        || matches!(
            transport,
            Some(super::c_native_transport::Checkpoint::OriginalContributor { .. })
        );
    let weak_contributor = matches!(
        transport,
        Some(super::c_native_transport::Checkpoint::OriginalContributor { weak: true, .. })
    );
    let source_refs = if contributor_case {
        vec!["recorded_emitter.c_original_contributor".to_owned()]
    } else {
        vec![]
    };
    let contributor_queue = if contributor_case {
        Some(
            super::c_native_contributors::queue(
                &fixture,
                &pack,
                leaf.id,
                &leaf.r#ref,
                &source_refs,
            )
            .await?,
        )
    } else {
        None
    };
    let mut definition = json!({"tasks":[{"name":"observed","action":leaf.r#ref,
        "wait_for":{"execution":"{{ target_execution }}"},"permission_set_refs":["recorded_emitter.c_wait_source"],
        "params":{"token":"{{ execution.observed.result.token }}"}},
        {"name":"source","action":leaf.r#ref,"permission_set_refs":source_refs}],"output_map":{"token":"{{ saved }}"}});
    if let Some(cache) = &mixed_cache {
        definition["tasks"].as_array_mut().unwrap().push(json!({"name":"cached","action":leaf.r#ref,
            "iterate_cache":{"namespace":cache.reference,"page_size":1,"require_fresh":false},"batch_size":1,
            "permission_set_refs":["standard"],"params":{"token":"{{ item.value.token }}"}}));
        // Both cache references share the same authenticated parent. Native
        // snapshot identity/budgets must not reset or turn into duplicate grants.
        // Array elements are evaluated before selecting the original saved value.
        definition["output_map"]["token"] = json!("{{ [saved, cached, cached][0] }}");
    }
    let workflow_def = WorkflowDefinitionRepository::create(
        &fixture.ctx.pool,
        CreateWorkflowDefinitionInput {
            r#ref: graph.r#ref.clone(),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "C wait reader".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            tags: vec![],
            definition: definition.clone(),
        },
    )
    .await?;
    ActionRepository::link_workflow_def(&fixture.ctx.pool, graph.id, workflow_def.id).await?;
    activate_test_pack_release_with_projections(
        &fixture.ctx.pool,
        &pack,
        &PackProjectionIds {
            actions: vec![leaf.id, graph.id],
            work_queues: contributor_queue
                .as_ref()
                .map(|queue| vec![queue.id])
                .unwrap_or_default(),
            ..Default::default()
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    if mixed_case {
        use attune_common::repositories::cache::{CacheEntryRepository, CacheTransactionMode};
        CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
            .await?;
    }
    let owner = RuntimeAuthorityRepository::find_owner(&mut tx, pack.id)
        .await?
        .unwrap();
    let leaf_pin =
        ExecutableSnapshotRepository::resolve_for_action_ref(&mut *tx, &leaf.r#ref).await?;
    let graph_pin =
        ExecutableSnapshotRepository::resolve_for_action_ref(&mut *tx, &graph.r#ref).await?;
    let root = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(graph.id),
            action_ref: graph.r#ref.clone(),
            status: ExecutionStatus::Running,
            config: Some(json!({})),
            ..Default::default()
        },
        &graph_pin,
    )
    .await?;
    let workflow = WorkflowExecutionRepository::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: root.id,
            workflow_def: workflow_def.id,
            task_graph: definition,
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await?;
    let source = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(leaf.id),
            action_ref: leaf.r#ref.clone(),
            parent: Some(root.id),
            permission_set_refs: source_refs.clone(),
            status: ExecutionStatus::Running,
            config: Some(json!({})),
            result: Some(json!({"token":redaction_marker()})),
            workflow_task: Some(serde_json::from_value(
                json!({"workflow_execution":workflow.id,"task_name":"source",
                "retry_count":0,"max_retries":0,"timed_out":false}),
            )?),
            ..Default::default()
        },
        &leaf_pin,
    )
    .await?;
    assert!(
        ExecutionRepository::is_in_execution_tree(&mut *tx, root.id, source.id, false).await?,
        "a native execution wait observes a real descendant, not an unrelated execution"
    );
    WorkflowExecutionRepository::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            variables: Some(json!({"target_execution":source.id})),
            ..Default::default()
        },
    )
    .await?;
    let original = KeyRepository::create_encrypted(
        &mut tx,
        CreateKeyInput {
            local_ref: "c_wait_original".into(),
            owner_type: OwnerType::Pack,
            owner_identity: None,
            owner_pack: Some(pack.id),
            owner_pack_ref: Some(pack.r#ref.clone()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Original wait field".into(),
            encrypted: true,
            encryption_key_hash: None,
            value: json!("native-wait-original"),
        },
        key,
    )
    .await?;
    let consent = PermissionSetRepository::create(&mut *tx, CreatePermissionSetInput {
        r#ref: "fixture.c_wait_original_consent".into(), pack: None, pack_ref: None, label: None, description: None,
        grants: json!([
            {"resource":"keys","actions":["read","decrypt"],"constraints":{"ids":[original.id]}},
            {"resource":"executions","actions":["read","decrypt"],"constraints":{"ids":[source.id]}}
        ]),
    }).await?;
    PermissionAssignmentRepository::create(
        &mut *tx,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: consent.id,
        },
    )
    .await?;
    // This source belongs to the actual managing pack. Its local task grant is
    // narrowed to the one source execution at policy admission below. Original
    // human input consent remains independent and exact-ID scoped above.
    let source_set = serde_json::from_value::<PermissionSetDefinition>(
        json!({"ref":"recorded_emitter.c_wait_source","grants":[
            {"resource":"executions","actions":["read","decrypt"]}
        ]}),
    )?
    .compile(&owner, &[])?;
    // Concrete bindings are keyed by the admitted grant, not by set name.
    // Separate Read/Decrypt grants keep the full reader distinct from the weak
    // reader's combined grant; differing target sets must never be unioned.
    let mut full_definition = json!({"ref":"recorded_emitter.c_wait_reader","grants":[
        {"resource":"executions","actions":["read"],"constraints":{"pack_refs":[pack.r#ref]}},
        {"resource":"executions","actions":["decrypt"],"constraints":{"pack_refs":[pack.r#ref]}},
        {"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[original.r#ref]}}
    ]});
    if let Some(cache) = &mixed_cache {
        full_definition["grants"].as_array_mut().unwrap().push(json!({"resource":"caches","actions":["read","decrypt"],
            "constraints":{"refs":[cache.reference],"owner_types":["pack"],"owner_refs":[pack.r#ref]}}));
    }
    let full_set =
        serde_json::from_value::<PermissionSetDefinition>(full_definition)?.compile(&owner, &[])?;
    let weak_set = serde_json::from_value::<PermissionSetDefinition>(
        json!({"ref":"recorded_emitter.c_wait_no_source_read","grants":[
            {"resource":"executions","actions":["read","decrypt"],"constraints":{"pack_refs":[pack.r#ref]}},
            {"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[original.r#ref]}}
        ]}),
    )?
    .compile(&owner, &[])?;
    // Reuse the independently bound full Read and root-only combined grant.
    // Their digests already have exact, nonconflicting concrete target sets.
    let no_decrypt_set = serde_json::from_value::<PermissionSetDefinition>(json!({
        "ref":"recorded_emitter.c_wait_no_source_decrypt","grants":[
            {"resource":"executions","actions":["read"],"constraints":{"pack_refs":[pack.r#ref]}},
            {"resource":"executions","actions":["read","decrypt"],"constraints":{"pack_refs":[pack.r#ref]}},
            {"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[original.r#ref]}}
        ]
    }))?.compile(&owner, &[])?;
    let mut sets = BTreeMap::from([
        (source_set.reference.clone(), source_set),
        (full_set.reference.clone(), full_set),
        (weak_set.reference.clone(), weak_set),
        (no_decrypt_set.reference.clone(), no_decrypt_set),
    ]);
    let contributor_deadline = if contributor_case {
        let deadline = chrono::Utc::now() + chrono::Duration::seconds(30);
        let grants = vec![serde_json::from_value(
            json!({"resource":"keys","actions":["read","decrypt"],"constraints":{"ids":[original.id]}}),
        )?];
        let approval = RuntimeAuthorityRepository::issue_approval(
            &mut tx,
            pack.id,
            fixture.requester,
            grants.clone(),
            Some(deadline),
        )
        .await?;
        let set = attune_common::runtime_authority::AdmittedPermissionSet::external(
            &source_refs[0],
            &owner,
            &grants,
            &[approval.approval()?],
        )?;
        sets.insert(set.reference.clone(), set);
        Some(deadline)
    } else {
        None
    };
    let root_binding = InvocationBinding::Action {
        action_ref: graph.r#ref.clone(),
    };
    let leaf_binding = InvocationBinding::Action {
        action_ref: leaf.r#ref.clone(),
    };
    let task_binding = InvocationBinding::WorkflowTask {
        workflow_ref: graph.r#ref.clone(),
        task_path: "/tasks/observed".into(),
        action_ref: leaf.r#ref.clone(),
    };
    let source_binding = if let Some(queue) = &contributor_queue {
        InvocationBinding::Queue {
            queue_ref: queue.r#ref.clone(),
            action_ref: leaf.r#ref.clone(),
        }
    } else {
        InvocationBinding::WorkflowTask {
            workflow_ref: graph.r#ref.clone(),
            task_path: "/tasks/source".into(),
            action_ref: leaf.r#ref.clone(),
        }
    };
    let cached_binding = InvocationBinding::WorkflowTask {
        workflow_ref: graph.r#ref.clone(),
        task_path: "/tasks/cached".into(),
        action_ref: leaf.r#ref.clone(),
    };
    let cache_invocation = if mixed_case {
        vec![InvocationPolicy::from_executable(
            cached_binding,
            &leaf_pin,
            Some(&["standard".into()]),
            &sets,
        )?]
    } else {
        vec![]
    };
    let reader_refs = vec![
        "recorded_emitter.c_wait_reader".to_owned(),
        "recorded_emitter.c_wait_no_source_read".to_owned(),
        "recorded_emitter.c_wait_no_source_decrypt".to_owned(),
    ];
    let concrete_targets = bind_execution_targets(
        &mut tx,
        owner.sponsor_identity_id,
        &sets,
        &[
            ("recorded_emitter.c_wait_source", &[source.id]),
            ("recorded_emitter.c_wait_reader", &[root.id, source.id]),
            ("recorded_emitter.c_wait_no_source_read", &[root.id]),
        ],
    )
    .await?;
    let policy = RuntimeAuthorityRepository::capture_policy(
        &mut tx,
        &RuntimePolicy {
            owner: owner.clone(),
            release_id: graph_pin.release.id,
            release_digest: graph_pin.release.digest.clone(),
            concrete_targets: concrete_targets.clone(),
            invocations: vec![
                InvocationPolicy::from_executable(
                    root_binding.clone(),
                    &graph_pin,
                    Some(&[]),
                    &sets,
                )?,
                InvocationPolicy::from_executable(
                    leaf_binding.clone(),
                    &leaf_pin,
                    Some(&reader_refs),
                    &sets,
                )?,
                InvocationPolicy::from_executable(
                    task_binding.clone(),
                    &leaf_pin,
                    Some(&["recorded_emitter.c_wait_source".to_owned()]),
                    &sets,
                )?,
                InvocationPolicy::from_executable(
                    source_binding.clone(),
                    &leaf_pin,
                    Some(&source_refs),
                    &sets,
                )?,
            ]
            .into_iter()
            .chain(cache_invocation)
            .collect(),
        },
    )
    .await?;
    for (grant, expected) in &concrete_targets {
        assert_eq!(policy.policy.concrete_targets.get(grant), Some(expected));
    }
    for grant in sets
        .values()
        .flat_map(|set| &set.grants)
        .filter(|grant| grant.grant().resource == attune_common::rbac::Resource::Keys)
    {
        let digest = grant_digest(grant)?;
        match policy.policy.concrete_targets[&digest].targets.as_ref() {
            Some(targets) => {
                assert_eq!(targets.len(), 1);
                let target = targets.iter().next().unwrap();
                assert_eq!(
                    (target.kind.as_str(), target.id, target.pack_id),
                    ("key", original.id, Some(pack.id))
                );
            }
            None => {
                // External approval grants retain exact IDs, not component-ref
                // selectors. They still must name this original key only.
                assert!(grant.approval_id().is_some());
                assert_eq!(
                    grant.grant().constraints.as_ref().unwrap().ids.as_deref(),
                    Some(&[original.id][..])
                );
            }
        }
    }
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id).await?;
    let attribution = RequestAttribution::Human {
        actor: HumanAttribution {
            identity_id: fixture.requester,
            login: fixture.ctx.user.as_ref().unwrap().login.clone(),
        },
    };
    let mut source_admission = 0;
    let contributor_items = if let Some(queue) = &contributor_queue {
        super::c_native_contributors::capture(
            &mut tx,
            queue,
            source.id,
            original.id,
            weak_contributor,
        )
        .await?
    } else {
        vec![]
    };
    let source_attribution = if let Some(queue) = &contributor_queue {
        super::c_native_contributors::attribution(&mut tx, queue, &contributor_items).await?
    } else {
        RequestAttribution::ParentExecution {
            execution_id: root.id,
        }
    };
    for (execution, binding, pin, attribution) in [
        (root.id, &root_binding, &graph_pin, attribution.clone()),
        (source.id, &source_binding, &leaf_pin, source_attribution),
    ] {
        let selected = policy.policy.select(
            policy.id,
            binding,
            if execution == source.id {
                &source_refs
            } else {
                &[]
            },
            attribution,
        )?;
        let record = RuntimeAuthorityRepository::record_admission(
            &mut tx,
            RuntimeOwner::Execution(execution),
            &selected,
        )
        .await?;
        if execution == source.id {
            source_admission = record.id;
            if contributor_case {
                assert_eq!(
                    record.authority.approval_ids().len(),
                    1,
                    "only the original source execution carries the finite contributor approval"
                );
            }
        }
        let schema = SchemaContractRepository::materialize_action_pair(&mut tx, pin).await?;
        SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution, &schema)
            .await?;
    }
    RecordedInputAuthorityRepository::capture_human(
        &mut tx,
        "execution",
        source.id,
        fixture.requester,
    )
    .await?;
    // Seal once, in the original creation transaction, after every independent
    // contributor has been recorded and before native source consumption.
    attune_common::repositories::accepted_input::AcceptedInputRepository::seal_execution(
        &mut tx, source.id, key,
    )
    .await?;
    let source = ExecutionRepository::update(
        &mut *tx,
        source.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    let original_fields = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/token".into(),
            value: json!("native-wait-original"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&SecretProvenance::key(
                attune_common::key_access::key_origin(&original),
            ))?),
        }],
        key,
    )?;
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        ENTITY_EXECUTION_RESULT,
        source.id,
        &original_fields,
    )
    .await?;
    RuntimeReadRepository::close_execution(&mut tx, source.id).await?;
    let wait = WorkflowTaskWaitRepository::create_or_get(
        &mut tx,
        CreateWorkflowTaskWaitInput {
            workflow_execution: workflow.id,
            task_name: "observed".into(),
            target: WorkflowTaskWaitTarget::Execution(source.id),
        },
    )
    .await?;
    let task = Admissions::select(
        &mut tx,
        pack.id,
        task_binding,
        &leaf_pin,
        &["recorded_emitter.c_wait_source".to_owned()],
        RequestAttribution::ParentExecution {
            execution_id: root.id,
        },
        RuntimeTarget::Component(ComponentTarget::Action, graph.id),
    )
    .await?;
    let declarations = DeclaredInputRepository::bindings(&mut tx, root.id).await?;
    let selector = declarations
        .iter()
        .find(|item| item.source_path.ends_with("/wait_for/execution"))
        .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &selector.declaration_revision,
        &selector.source_path,
        &selector.expression,
        InputSitePurpose::Control,
    )
    .await?;
    Inputs::bind_public_field(
        &mut tx,
        &mut session,
        "/target_execution",
        &InputSite {
            execution: root.id,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Value,
            path: "/target_execution".into(),
        },
    )
    .await?;
    let control_render = Inputs::evaluate(&mut tx, session).await?;
    let control = Inputs::record_render(
        &mut tx,
        &control_render,
        &InputSite {
            execution: root.id,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Control,
            path: selector.source_path.clone(),
        },
        control_render.value(),
        control_render.selections(),
        key,
    )
    .await?;
    assert_eq!(control_render.value(), &json!(source.id));
    assert_eq!(source.parent, Some(root.id));
    assert_eq!(source.status, ExecutionStatus::Completed);
    let prepared = Snapshots::prepare_wait(
        &mut tx,
        root.id,
        "/workflow/tasks/0",
        &task,
        wait.id,
        Some(&control),
        key,
    )
    .await;
    if weak_contributor {
        assert!(prepared.is_err(),"a strong human and first contributor cannot lend Decrypt to the second original contributor");
        tx.rollback().await?;
        fixture.ctx.cleanup().await?;
        return Ok(());
    }
    let prepared = prepared?;
    let declaration = declarations
        .iter()
        .find(|item| item.source_path.ends_with("/params/token"))
        .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &declaration.declaration_revision,
        &declaration.source_path,
        &declaration.expression,
        InputSitePurpose::Value,
    )
    .await?;
    Inputs::bind_prepared_wait_input(&mut session, &prepared)?;
    let rendered = Inputs::evaluate(&mut tx, session).await?;
    let variable_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Value,
        path: "/saved".into(),
    };
    let destination = Inputs::prepare_destination(&mut tx, &rendered, &variable_site, key).await?;
    Snapshots::release_wait(&mut tx, prepared).await?;
    let variable = Inputs::record_prepared(
        &mut tx,
        &rendered,
        &destination,
        rendered.value(),
        rendered.selections(),
        key,
    )
    .await?;
    WorkflowExecutionRepository::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            variables: Some(json!({"target_execution":source.id,"saved":redaction_marker()})),
            ..Default::default()
        },
    )
    .await?;
    let variables = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/saved".into(),
            value: rendered.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&variable.provenance())?),
        }],
        key,
    )?;
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        "workflow_variables",
        root.id,
        &variables,
    )
    .await?;
    let cached = if let Some(cache) = &mixed_cache {
        Some(
            cache
                .prepare_variable(
                    &mut tx,
                    root.id,
                    workflow.id,
                    source.id,
                    pack.id,
                    graph.id,
                    &graph.r#ref,
                    &leaf_pin,
                    key,
                )
                .await?,
        )
    } else {
        None
    };
    let saved =
        Inputs::prepare_source(&mut tx, root.id, &variable_site, &variable.origin(), key).await?;
    let output = declarations
        .iter()
        .find(|item| item.source_path.ends_with("/output_map/token"))
        .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &output.declaration_revision,
        &output.source_path,
        &output.expression,
        InputSitePurpose::Value,
    )
    .await?;
    Inputs::bind_prepared(&mut session, "/saved", &saved, rendered.value())?;
    if let Some((cached, value)) = &cached {
        Inputs::bind_prepared(&mut session, "/cached", cached, value)?;
    }
    let output_render = Inputs::evaluate(&mut tx, session)
        .await
        .map_err(|error| format!("native-wait evaluate-output: {error}"))?;
    if mixed_case {
        assert!(
            output_render.value() == rendered.value(),
            "mixed output must retain the original Key-tainted wait value"
        );
        let paths = output_render
            .selections()
            .iter()
            .map(|selection| selection.context_path.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            paths,
            std::collections::BTreeSet::from(["/cached", "/saved"]),
            "mixed output must retain both independently bound receipt parents"
        );
        assert_eq!(
            output_render.selections().len(),
            2,
            "repeated cache reads must reuse their original receipt selection"
        );
        assert!(
            output_render.approval_deadline().is_some(),
            "the original contributor's finite deadline must survive composition"
        );
        assert_eq!(
            output_render.approval_deadline(),
            rendered.approval_deadline(),
            "cache composition must not replace or extend the original wait deadline"
        );
    }
    let output_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::ExecutionResult,
        purpose: InputSitePurpose::Value,
        path: "/token".into(),
    };
    let output_destination =
        Inputs::prepare_destination(&mut tx, &output_render, &output_site, key).await?;
    let receipt = Inputs::record_prepared(
        &mut tx,
        &output_render,
        &output_destination,
        output_render.value(),
        output_render.selections(),
        key,
    )
    .await?;
    ExecutionRepository::update(
        &mut *tx,
        root.id,
        UpdateExecutionInput {
            result: Some(json!({"token":redaction_marker()})),
            ..Default::default()
        },
    )
    .await?;
    let output_fields = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/token".into(),
            value: output_render.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&receipt.provenance())?),
        }],
        key,
    )?;
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        ENTITY_EXECUTION_RESULT,
        root.id,
        &output_fields,
    )
    .await?;
    let mut readers = Vec::new();
    for reference in &reader_refs {
        let refs = vec![reference.clone()];
        let selected =
            policy
                .policy
                .select(policy.id, &leaf_binding, &refs, attribution.clone())?;
        let credential = capture_reader(&mut tx, &leaf_pin, &selected).await?;
        let token = generate_admitted_execution_token(
            &credential,
            &refs,
            &fixture.ctx.state.jwt_config,
            Some(360),
        )?;
        readers.push((credential, token));
    }
    let context = Inputs::receipt_context(&mut tx, &output_site, &receipt.origin(), key).await?;
    let wait_metadata = context
        .native_source_metadata()
        .iter()
        .find(|metadata| metadata.source_kind() == "execution_result")
        .unwrap();
    let snapshot = wait_metadata.snapshot();
    let frozen = Snapshots::source_evidence(&mut tx, wait_metadata, key).await?;
    if mixed_case {
        assert_eq!(
            context.native_source_metadata().len(),
            2,
            "the cache shared ancestor must retain one exact native snapshot"
        );
        assert_eq!(
            context
                .native_source_metadata()
                .iter()
                .filter(|metadata| metadata.source_kind() == "cache")
                .count(),
            1
        );
        assert!(
            context
                .native_source_metadata()
                .iter()
                .filter(|metadata| metadata.source_kind() == "cache")
                .all(|metadata| metadata.target().id == mixed_cache.as_ref().unwrap().namespace),
            "the shared cache parent must retain its actual original namespace target"
        );
        assert_eq!(context.source_origins().iter().filter(|origin|matches!(origin,SecretOrigin::Key(key) if key.key_id==original.id)).count(),1,
            "the same original Key ancestor must not become duplicated or unioned contributor grants");
    }
    assert_eq!(
        frozen.fields(),
        original_fields
            .iter()
            .map(|field| attune_common::secret_values::StoredSecretValue {
                json_path: field.json_path.clone(),
                encrypted_value: field.encrypted_value.clone(),
                encryption_key_hash: Some(field.encryption_key_hash.clone()),
                source_kind: field.source_kind.clone(),
                source_ref: field.source_ref.clone(),
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(
        frozen.contributor_manifests().len(),
        if contributor_case { 3 } else { 1 }
    );
    if contributor_case {
        let identities = frozen
            .contributor_manifests()
            .iter()
            .map(|manifest| manifest.body()["identity_id"].as_i64().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            identities.len(),
            3,
            "each original ceiling retains its own identity"
        );
        assert!(
            !identities.contains(&owner.sponsor_identity_id),
            "original ceilings cannot collapse to the runtime sponsor"
        );
        assert_eq!(
            frozen
                .contributor_manifests()
                .iter()
                .filter(|manifest| manifest.kind() == "queue")
                .count(),
            2
        );
    }
    // Exercise the exact canonical floor invoked by the native adapter using
    // the real archived Key origin. A second narrowed contributor cannot borrow
    // the first one's Decrypt. This does not construct a fake native proof.
    let manifest = frozen.contributor_manifests()[0].body();
    let grants: Vec<attune_common::rbac::Grant> =
        serde_json::from_value(manifest["grants"].clone())?;
    let attributes: std::collections::HashMap<String, Value> =
        serde_json::from_value(manifest["attributes"].clone())?;
    let identity = manifest["identity_id"].as_i64().unwrap();
    let strong = attune_common::delegation::DelegationAuthority::new(
        identity,
        attributes.clone(),
        grants.clone(),
    );
    let mut narrowed = grants;
    for grant in &mut narrowed {
        grant
            .actions
            .retain(|action| *action != attune_common::rbac::Action::Decrypt);
    }
    let weak = attune_common::delegation::DelegationAuthority::new(identity, attributes, narrowed);
    let retained_keys = frozen
        .origins()
        .iter()
        .filter(|origin| matches!(origin, SecretOrigin::Key(_)))
        .cloned()
        .collect::<Vec<_>>();
    attune_common::repositories::immutable_input_origin::ImmutableInputOriginRepository::require(
        &mut tx,
        std::slice::from_ref(&strong),
        &retained_keys,
    )
    .await?;
    assert!(attune_common::repositories::immutable_input_origin::ImmutableInputOriginRepository::require(&mut tx, &[strong, weak], &retained_keys).await.is_err());
    let rows = ExecutionSecretValueRepository::find_bounded_provenance(
        &mut *tx,
        &[ENTITY_EXECUTION_RESULT],
        &[root.id],
    )
    .await?;
    let mut frontier = InputReceiptFrontier::default();
    frontier.prepare(&mut tx, &rows, Some(key)).await?;
    let leaves = frontier.disclosure_leaves(&output_site, &receipt.origin())?;
    let original_root = SecretOrigin::Entity {
        entity_type: ENTITY_EXECUTION_RESULT.into(),
        entity_id: source.id,
        path: String::new(),
    };
    let held = frontier
        .prepared_execution_wait(&leaves, &original_root)?
        .unwrap();
    assert_eq!(
        held.contract().unwrap().source.component_ref(),
        leaf.r#ref.as_str()
    );
    let target = RuntimeTargetRepository::resolve_execution_wait_source(
        &mut tx,
        &held,
        owner.sponsor_identity_id,
    )
    .await?;
    assert!(!target.accepts_new_effects());
    assert_eq!(target.context().target_id, Some(source.id));
    for kind in ["execution_config", "inquiry_response"] {
        assert!(frontier
            .prepared_execution_wait(
                &leaves,
                &SecretOrigin::Entity {
                    entity_type: kind.into(),
                    entity_id: source.id,
                    path: String::new()
                }
            )?
            .is_none());
    }
    for mutation in 0..2 {
        let mut wrong = rows.clone();
        if mutation == 0 {
            wrong[0].entity_type = "execution_config".into();
        } else {
            wrong[0].json_path = "/wrong".into();
        }
        assert!(InputReceiptFrontier::default()
            .prepare(&mut tx, &wrong, Some(key))
            .await
            .is_err());
    }
    tx.commit().await?;
    let mut unrelated = fixture.ctx.pool.begin().await?;
    assert!(
        RuntimeTargetRepository::resolve_execution_wait_source(
            &mut unrelated,
            &held,
            owner.sponsor_identity_id
        )
        .await
        .is_err(),
        "held native evidence must not authorize on a different source transaction"
    );
    unrelated.rollback().await?;
    let path = format!("/api/v1/executions/{}?include_secret_values=true", root.id);
    let good = fixture.ctx.get(&path, Some(&readers[0].1)).await?;
    let status = good.status();
    let body: Value = good.json().await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "native execution wait reader must succeed"
    );
    assert_eq!(body["data"]["result"]["token"], "native-wait-original");
    assert_eq!(
        fixture
            .ctx
            .get(
                &format!("/api/v1/executions/{}", fixture.credential.execution_id),
                Some(&readers[0].1)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN,
        "exact admitted execution bindings must exclude an unrelated execution in the same pack"
    );
    assert_eq!(
        fixture
            .ctx
            .get(
                &format!("/api/v1/executions/{}", source.id),
                Some(&readers[1].1)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN,
        "the weak reader must not inherit the full reader's source binding"
    );
    assert_eq!(
        fixture.ctx.get(&path, Some(&readers[1].1)).await?.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .ctx
            .get(
                &format!("/api/v1/executions/{}", source.id),
                Some(&readers[2].1)
            )
            .await?
            .status(),
        StatusCode::OK,
        "the no-Decrypt reader still has actual source Read"
    );
    assert_eq!(
        fixture.ctx.get(&path, Some(&readers[2].1)).await?.status(),
        StatusCode::FORBIDDEN,
        "source Read and Key rights cannot replace source Decrypt"
    );
    let mut gc = fixture.ctx.pool.begin().await?;
    attune_common::repositories::file_evidence_retention::FileEvidenceRetentionRepository::lock_reclamation(&mut gc).await?;
    for item in &contributor_items {
        use attune_common::repositories::work_queue::WorkQueueItemRepository;
        assert!(WorkQueueItemRepository::delete(&mut *gc, *item).await?);
        assert!(WorkQueueItemRepository::find_by_id(&mut *gc, *item)
            .await?
            .is_none());
    }
    assert!(ExecutionRepository::delete(&mut *gc, source.id).await?);
    assert!(ExecutionRepository::find_by_id(&mut *gc, source.id)
        .await?
        .is_none());
    gc.commit().await?;
    KeyRepository::update(
        &fixture.ctx.pool,
        original.id,
        UpdateKeyInput {
            value: Some(attune_common::crypto::keystore::encrypt_json(
                &json!("new-current-key-value"),
                key,
                original.id,
            )?),
            ..Default::default()
        },
    )
    .await?;
    if let Some(at) = transport {
        if let Some(cache) = &mixed_cache {
            cache.retire(&fixture).await?;
        }
        super::c_native_transport::exercise(
            &mut fixture,
            root.id,
            readers[0].0.execution_id,
            policy.id,
            contributor_deadline,
            at,
        )
        .await?;
        if contributor_case {
            let mut remaining = fixture.ctx.pool.begin().await?;
            assert!(RuntimeAuthorityRepository::retained_admission_fence(&mut remaining,source_admission).await.is_err(),
                "the expired fence belongs to the original collected source, not the current reader");
            let mut independent = Vec::new();
            for manifest in frozen.contributor_manifests() {
                independent.push(attune_common::delegation::DelegationAuthority::new(
                    manifest.body()["identity_id"].as_i64().unwrap(),
                    serde_json::from_value(manifest.body()["attributes"].clone())?,
                    serde_json::from_value(manifest.body()["grants"].clone())?,
                ));
            }
            attune_common::repositories::immutable_input_origin::ImmutableInputOriginRepository::require(
                &mut remaining,&independent,&retained_keys).await?;
            remaining.rollback().await?;
        }
        fixture.ctx.cleanup().await?;
        return Ok(());
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
    let after = fixture.ctx.get(&path, Some(&readers[0].1)).await?;
    let status = after.status();
    let body: Value = after.json().await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "authenticated native archive must survive raw source/authority GC and issuer freeze"
    );
    assert_eq!(body["data"]["result"]["token"], "native-wait-original");
    let mut check = fixture.ctx.pool.begin().await?;
    let reloaded =
        Inputs::receipt_context(&mut check, &output_site, &receipt.origin(), key).await?;
    assert!(
        reloaded.source_origins().contains(&SecretOrigin::Entity {
            entity_type: "workflow_variables".into(),
            entity_id: root.id,
            path: "/saved".into()
        }),
        "the real native output includes its surviving workflow-variable receipt parent"
    );
    assert_eq!(reloaded.native_source_metadata()[0].snapshot(), snapshot);
    let archived =
        Snapshots::source_evidence(&mut check, &reloaded.native_source_metadata()[0], key).await?;
    assert_eq!(
        archived.fields()[0].encrypted_value,
        original_fields[0].encrypted_value
    );
    assert!(
        RecordedInputAuthorityRepository::load(&mut check, "execution", source.id)
            .await?
            .is_none()
    );
    let captured =
        RuntimeReadRepository::references_for_executions(&mut *check, &[readers[0].0.execution_id])
            .await?;
    for origin in captured.values().flatten() {
        let SecretOrigin::Entity { entity_id, .. } = origin else {
            unreachable!()
        };
        let saved: RuntimeReadOrigin = RuntimeReadRepository::find(&mut *check, *entity_id).await?;
        assert!(
            matches!(saved.target, RuntimeReadTarget::Execution { execution } if execution == root.id)
        );
        saved.validate()?;
        let mut missing = saved.clone();
        missing.parents.retain(|parent| {
            !(parent.entity_type == ENTITY_EXECUTION_RESULT
                && parent.entity_id == source.id
                && parent.path.is_empty())
        });
        assert!(missing.validate().is_err());
    }
    check.rollback().await?;
    // A distinct Access-token reader needs its own current exact-ID grants.
    // The frozen original human's capture is an input ceiling, not this IAM.
    let login = format!("c_native_reader_{}", uuid::Uuid::new_v4().simple());
    let registered = fixture
        .ctx
        .post(
            "/auth/register",
            json!({"login":login,"password":"TestPassword123!","display_name":"C native reader"}),
            None,
        )
        .await?;
    assert!(registered.status().is_success());
    let registration: Value = registered.json().await?;
    let human_token = registration["data"]["access_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let human = IdentityRepository::find_by_login(&fixture.ctx.pool, &login)
        .await?
        .unwrap();
    assert_eq!(
        fixture.ctx.get(&path, Some(&human_token)).await?.status(),
        StatusCode::FORBIDDEN
    );
    let human_set = PermissionSetRepository::create(&fixture.ctx.pool, CreatePermissionSetInput {
        r#ref: "fixture.c_native_human_reader".into(), pack: None, pack_ref: None, label: None, description: None,
        grants: json!([
            {"resource":"executions","actions":["read","decrypt"],"constraints":{"ids":[root.id,source.id]}},
            {"resource":"keys","actions":["read","decrypt"],"constraints":{"ids":[original.id]}}
        ]),
    }).await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: human.id,
            permset: human_set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(human.id).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let human_read = fixture.ctx.get(&path, Some(&human_token)).await?;
    let status = human_read.status();
    let body: Value = human_read.json().await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "human native reader needs the authenticated archive, not a surviving raw source execution"
    );
    assert_eq!(body["data"]["result"]["token"], "native-wait-original");
    // The same native IDs without the authenticated receipt classification
    // cannot borrow its archived proof, even for this fully authorized reader.
    for (kind, id, origin_path) in [
        (ENTITY_EXECUTION_RESULT, source.id, ""),
        (ENTITY_EXECUTION_RESULT, source.id, "/wrong"),
        ("workflow_variables", root.id, "/wrong"),
        (ENTITY_EXECUTION_CONFIG, source.id, ""),
    ] {
        let mut malformed = output_fields.clone();
        malformed[0].source_ref = Some(serde_json::to_string(&SecretProvenance {
            origins: vec![SecretOrigin::Entity {
                entity_type: kind.into(),
                entity_id: id,
                path: origin_path.into(),
            }],
            templates: vec![],
        })?);
        let mut mutate = fixture.ctx.pool.begin().await?;
        ExecutionSecretValueRepository::upsert_many_with_conn(
            &mut mutate,
            ENTITY_EXECUTION_RESULT,
            root.id,
            &malformed,
        )
        .await?;
        mutate.commit().await?;
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            fixture.ctx.get(&path, Some(&human_token)),
        )
        .await??;
        let status = response.status();
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            axum::body::to_bytes(response.into_response().into_body(), 64 * 1024),
        )
        .await??;
        let body: Value = serde_json::from_slice(&bytes)?;
        assert!(
            status == StatusCode::FORBIDDEN
                || status == StatusCode::OK
                    && body["data"]["result"]["token"] == redaction_marker(),
            "same-ID or wrong-site raw origins are not authenticated native evidence"
        );
        assert!(!body.to_string().contains("native-wait-original"));
    }
    let mut restore = fixture.ctx.pool.begin().await?;
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut restore,
        ENTITY_EXECUTION_RESULT,
        root.id,
        &output_fields,
    )
    .await?;
    restore.commit().await?;
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.ctx.get(&path, Some(&human_token)),
    )
    .await??;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        axum::body::to_bytes(response.into_response().into_body(), 64 * 1024),
    )
    .await??;
    let body: Value = serde_json::from_slice(&bytes)?;
    assert!(body["data"]["result"]["token"]==json!("native-wait-original"),
        "the exact restored original receipt must still disclose its original plaintext after source GC");
    let mut check = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_admission(&mut check, source_admission).await?;
    check.commit().await?;
    assert_eq!(
        fixture.ctx.get(&path, Some(&readers[0].1)).await?.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture.ctx.get(&path, Some(&human_token)).await?.status(),
        StatusCode::FORBIDDEN
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}
