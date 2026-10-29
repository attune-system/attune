//! Real inquiry/queue native waits through variable/output receipts and HTTP.
use super::*;
use attune_common::{
    models::{OwnerType, WorkQueueItemStatus, WorkflowTaskWaitTarget},
    rbac::Resource,
    repositories::{
        action::{ActionRepository, UpdateActionInput},
        declared_input::DeclaredInputRepository,
        execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
        file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        immutable_input_origin::InputReceiptFrontier,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        inquiry::{CreateWorkflowInquiryInput, InquiryRepository},
        inquiry_schema::InquirySchemaRepository,
        key::{CreateKeyInput, KeyRepository, UpdateKeyInput},
        native_input_snapshot::NativeInputSnapshotRepository as Snapshots,
        pack::PackRepository,
        recorded_input_authority::RecordedInputAuthorityRepository,
        runtime_read::RuntimeReadRepository,
        runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
        work_queue::{
            CreateWorkQueueInput, CreateWorkQueueItemInput, WorkQueueItemRepository,
            WorkQueueRepository,
        },
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            UpdateWorkflowExecutionInput, WorkflowDefinitionRepository,
            WorkflowExecutionRepository,
        },
        workflow_task_wait::{CreateWorkflowTaskWaitInput, WorkflowTaskWaitRepository},
        FindById,
    },
    runtime_authority::{BoundRuntimeTarget, GrantTargetBindings},
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{redaction_marker, ENTITY_EXECUTION_RESULT},
};

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Inquiry,
    Queue,
}
impl Kind {
    fn target(self) -> &'static str {
        match self {
            Self::Inquiry => "inquiry",
            Self::Queue => "work_queue_item",
        }
    }
    fn source(self) -> &'static str {
        match self {
            Self::Inquiry => "inquiry_response",
            Self::Queue => "queue_item_result",
        }
    }
    fn resource(self) -> Resource {
        match self {
            Self::Inquiry => Resource::Inquiries,
            Self::Queue => Resource::QueueItems,
        }
    }
    fn resource_name(self) -> &'static str {
        match self {
            Self::Inquiry => "inquiries",
            Self::Queue => "queue_items",
        }
    }
    fn runtime(self, id: Id) -> RuntimeTarget {
        match self {
            Self::Inquiry => RuntimeTarget::Inquiry(id),
            Self::Queue => RuntimeTarget::QueueItem(id),
        }
    }
    fn wait(self, id: Id) -> WorkflowTaskWaitTarget {
        match self {
            Self::Inquiry => WorkflowTaskWaitTarget::Inquiry(id),
            Self::Queue => WorkflowTaskWaitTarget::WorkQueueItem(id),
        }
    }
}

#[tokio::test]
async fn native_inquiry_response_receipt_http_survives_raw_source_gc_and_enforces_reader_rights(
) -> Result<()> {
    related_wait(Kind::Inquiry, true, None).await
}
#[tokio::test]
async fn native_queue_result_receipt_http_survives_raw_source_gc_and_enforces_reader_rights(
) -> Result<()> {
    related_wait(Kind::Queue, true, None).await
}
#[tokio::test]
async fn native_inquiry_response_wait_cannot_borrow_missing_original_decrypt_consent() -> Result<()>
{
    related_wait(Kind::Inquiry, false, None).await
}
#[tokio::test]
async fn native_queue_result_wait_cannot_borrow_missing_original_decrypt_consent() -> Result<()> {
    related_wait(Kind::Queue, false, None).await
}

async fn json_body(response: helpers::TestResponse) -> Result<(StatusCode, Value)> {
    let status = response.status();
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        axum::body::to_bytes(response.into_response().into_body(), 64 * 1024),
    )
    .await??;
    Ok((status, serde_json::from_slice(&body)?))
}

pub(super) async fn related_wait(
    kind: Kind,
    original_decrypt: bool,
    transport: Option<super::c_native_transport::Checkpoint>,
) -> Result<()> {
    let mut fixture = fixture(false).await?;
    let key = fixture
        .ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .unwrap();
    let pack = PackRepository::find_by_id(&fixture.ctx.pool, fixture.pack_id)
        .await?
        .unwrap();
    let leaf = create_test_action(
        &fixture.ctx.pool,
        pack.id,
        &pack.r#ref,
        "recorded_emitter.c_related_leaf",
    )
    .await?;
    let graph = create_test_action(
        &fixture.ctx.pool,
        pack.id,
        &pack.r#ref,
        "recorded_emitter.c_related_graph",
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
    let mut wait_for = serde_json::Map::new();
    wait_for.insert(kind.target().into(), json!("{{ target }}"));
    let expression = format!(
        "{{{{ {}.observed.{}.token }}}}",
        kind.target(),
        if matches!(kind, Kind::Inquiry) {
            "response"
        } else {
            "result"
        }
    );
    let contributor_case = matches!(
        transport,
        Some(super::c_native_transport::Checkpoint::OriginalContributor { .. })
    );
    let weak_contributor = matches!(
        transport,
        Some(super::c_native_transport::Checkpoint::OriginalContributor { weak: true, .. })
    );
    let producer_refs = if contributor_case {
        vec!["recorded_emitter.c_related_contributor".to_owned()]
    } else {
        vec![]
    };
    let definition = json!({"tasks":[
        {"name":"observed","action":leaf.r#ref,"wait_for":wait_for,"permission_set_refs":["recorded_emitter.c_related_source"],"params":{"token":expression}},
        {"name":"producer","action":leaf.r#ref,"permission_set_refs":producer_refs}
    ],"output_map":{"token":"{{ saved }}"}});
    let graph_def = WorkflowDefinitionRepository::create(
        &fixture.ctx.pool,
        CreateWorkflowDefinitionInput {
            r#ref: graph.r#ref.clone(),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "C related wait".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            tags: vec![],
            definition: definition.clone(),
        },
    )
    .await?;
    ActionRepository::link_workflow_def(&fixture.ctx.pool, graph.id, graph_def.id).await?;
    let queue = WorkQueueRepository::create(
        &fixture.ctx.pool,
        CreateWorkQueueInput {
            r#ref: "recorded_emitter.c_related_jobs".into(),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            is_adhoc: false,
            label: "Related source".into(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(leaf.id),
            dispatch_action_ref: leaf.r#ref.clone(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: attune_common::models::WorkQueueUpdateStrategy::Immutable,
            batch_mode: if contributor_case {
                attune_common::models::WorkQueueBatchMode::Batch
            } else {
                attune_common::models::WorkQueueBatchMode::Single
            },
            item_schema: json!({}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: Some(producer_refs.clone()),
            config: json!({}),
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: vec![],
        },
    )
    .await?;
    activate_test_pack_release_with_projections(
        &fixture.ctx.pool,
        &pack,
        &PackProjectionIds {
            actions: vec![leaf.id, graph.id],
            work_queues: vec![queue.id],
            ..Default::default()
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
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
            workflow_def: graph_def.id,
            task_graph: definition,
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await?;
    let producer=ExecutionRepository::create_pinned(&mut *tx,CreateExecutionInput {
        action:Some(leaf.id),action_ref:leaf.r#ref.clone(),parent:Some(root.id),status:ExecutionStatus::Running,config:Some(json!({})),
        permission_set_refs:producer_refs.clone(),workflow_task:Some(serde_json::from_value(json!({"workflow_execution":workflow.id,"task_name":"producer","retry_count":0,"max_retries":0,"timed_out":false}))?),..Default::default()
    },&leaf_pin).await?;
    let original = KeyRepository::create_encrypted(
        &mut tx,
        CreateKeyInput {
            local_ref: "c_related_original".into(),
            owner_type: OwnerType::Pack,
            owner_identity: None,
            owner_pack: Some(pack.id),
            owner_pack_ref: Some(pack.r#ref.clone()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Original related source".into(),
            encrypted: true,
            encryption_key_hash: None,
            value: json!("related-wait-original"),
        },
        key,
    )
    .await?;
    let original_provenance =
        SecretProvenance::key(attune_common::key_access::key_origin(&original));
    let consent_actions = if original_decrypt {
        vec!["read", "decrypt"]
    } else {
        vec!["read"]
    };
    let consent=PermissionSetRepository::create(&mut *tx,CreatePermissionSetInput {
        r#ref:"fixture.c_related_consent".into(),pack:None,pack_ref:None,label:None,description:None,
        grants:json!([{"resource":"keys","actions":consent_actions,"constraints":{"ids":[original.id]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &mut *tx,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: consent.id,
        },
    )
    .await?;
    let source_id = match kind {
        Kind::Inquiry => {
            let inquiry = InquirySchemaRepository::create_workflow(
                &mut tx,
                CreateWorkflowInquiryInput {
                    created_by_execution: producer.id,
                    purpose: "related-reader".into(),
                    prompt: "Response".into(),
                    response_schema: Some(json!({"token":{"type":"string","secret":true}})),
                    response_options: vec![attune_common::models::inquiry::InquiryResponseOption {
                        r#ref: "answer".into(),
                        label: "Answer".into(),
                        style: attune_common::models::inquiry::InquiryResponseOptionStyle::Default,
                        response: json!({"token":"related-wait-original"}),
                    }],
                    assigned_to: None,
                    timeout_seconds: None,
                },
                key,
            )
            .await?;
            let binding = InquirySchemaRepository::find(&mut *tx, inquiry.id)
                .await?
                .unwrap();
            let (response, fields) = binding.binding.prepare_response(
                &json!({"token":"related-wait-original"}),
                &original_provenance,
            )?;
            InquiryRepository::respond_pending(
                &mut *tx,
                inquiry.id,
                response,
                fixture.requester,
                None,
            )
            .await?
            .unwrap();
            ExecutionSecretValueRepository::upsert_many_with_conn(
                &mut tx,
                kind.source(),
                inquiry.id,
                &prepare_secret_values(fields, key)?,
            )
            .await?;
            inquiry.id
        }
        Kind::Queue => {
            let item = WorkQueueItemRepository::create(
                &mut *tx,
                CreateWorkQueueItemInput {
                    queue: queue.id,
                    queue_ref: queue.r#ref.clone(),
                    item_key: None,
                    priority: 0,
                    status: WorkQueueItemStatus::Completed,
                    payload: json!({}),
                    metadata: json!({}),
                    trace_tag: None,
                    enqueue_source: "c-native-related".into(),
                    requested_by_identity: None,
                    requested_by_execution: Some(producer.id),
                    requested_by_enforcement: None,
                    leased_execution: None,
                    lease_token: None,
                    lease_expires_at: None,
                    attempt_count: 1,
                    last_error: None,
                    ack_summary: Some(json!({"token":redaction_marker()})),
                },
            )
            .await?;
            let fields = prepare_secret_values(
                vec![SecretValueInput {
                    json_path: "/token".into(),
                    value: json!("related-wait-original"),
                    source_kind: "provenance".into(),
                    source_ref: Some(serde_json::to_string(&original_provenance)?),
                }],
                key,
            )?;
            ExecutionSecretValueRepository::upsert_many_with_conn(
                &mut tx,
                kind.source(),
                item.id,
                &fields,
            )
            .await?;
            item.id
        }
    };
    WorkflowExecutionRepository::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            variables: Some(json!({"target":source_id})),
            ..Default::default()
        },
    )
    .await?;
    let source_set = serde_json::from_value::<PermissionSetDefinition>(
        json!({"ref":"recorded_emitter.c_related_source","grants":[
            {"resource":kind.resource_name(),"actions":["read","decrypt"]}
        ]}),
    )?
    .compile(&owner, &[])?;
    let reader_refs = [
        "recorded_emitter.c_related_full",
        "recorded_emitter.c_related_no_read",
        "recorded_emitter.c_related_no_decrypt",
    ]
    .map(str::to_owned);
    let mut sets = BTreeMap::from([(source_set.reference.clone(), source_set)]);
    for (index, reference) in reader_refs.iter().enumerate() {
        let actions = match index {
            1 => json!(["decrypt"]),
            2 => json!(["read"]),
            _ => json!(["read", "decrypt"]),
        };
        let set=serde_json::from_value::<PermissionSetDefinition>(json!({"ref":reference,"grants":[
            {"resource":"executions","actions":["read","decrypt"],"constraints":{"pack_refs":[pack.r#ref]}},
            {"resource":kind.resource_name(),"actions":actions,"constraints":{"pack_refs":[pack.r#ref]}},
            {"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[original.r#ref]}}
        ]}))?.compile(&owner,&[])?;
        sets.insert(set.reference.clone(), set);
    }
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
            &producer_refs[0],
            &owner,
            &grants,
            &[approval.approval()?],
        )?;
        sets.insert(set.reference.clone(), set);
        Some(deadline)
    } else {
        None
    };
    let selected = reader_refs
        .iter()
        .map(|reference| (reference.as_str(), std::slice::from_ref(&root.id)))
        .collect::<Vec<_>>();
    let mut concrete = super::c_native_execution_wait::bind_execution_targets(
        &mut tx,
        owner.sponsor_identity_id,
        &sets,
        &selected,
    )
    .await?;
    let actual = RuntimeTargetRepository::resolve(
        &mut tx,
        kind.runtime(source_id),
        owner.sponsor_identity_id,
    )
    .await?
    .unwrap();
    assert_eq!(actual.pack_id(), Some(pack.id));
    assert_eq!(actual.identity_kind(), kind.target());
    assert!(actual.owner_id().is_none() && actual.context().owner_type.is_none());
    for grant in sets
        .values()
        .flat_map(|set| &set.grants)
        .filter(|grant| grant.grant().resource == kind.resource())
    {
        assert!(grant.grant().actions.iter().all(|action| grant.allows(
            &Default::default(),
            actual.pack_id(),
            kind.resource(),
            *action,
            actual.context()
        )));
        let digest = super::c_native_execution_wait::grant_digest(grant)?;
        concrete.insert(
            digest,
            GrantTargetBindings {
                targets: Some(std::collections::BTreeSet::from([BoundRuntimeTarget {
                    kind: actual.identity_kind().into(),
                    id: source_id,
                    pack_id: actual.pack_id(),
                    owner_type: None,
                    owner_id: None,
                }])),
                owners: None,
                packs: Some(std::collections::BTreeSet::from([pack.id])),
            },
        );
    }
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
    let producer_binding = if contributor_case {
        InvocationBinding::Queue {
            queue_ref: queue.r#ref.clone(),
            action_ref: leaf.r#ref.clone(),
        }
    } else {
        InvocationBinding::WorkflowTask {
            workflow_ref: graph.r#ref.clone(),
            task_path: "/tasks/producer".into(),
            action_ref: leaf.r#ref.clone(),
        }
    };
    let policy = RuntimeAuthorityRepository::capture_policy(
        &mut tx,
        &RuntimePolicy {
            owner: owner.clone(),
            release_id: graph_pin.release.id,
            release_digest: graph_pin.release.digest.clone(),
            concrete_targets: concrete.clone(),
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
                    Some(&["recorded_emitter.c_related_source".into()]),
                    &sets,
                )?,
                InvocationPolicy::from_executable(
                    producer_binding.clone(),
                    &leaf_pin,
                    Some(&producer_refs),
                    &sets,
                )?,
            ],
        },
    )
    .await?;
    for (digest, binding) in &concrete {
        assert_eq!(policy.policy.concrete_targets.get(digest), Some(binding));
    }
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id).await?;
    let human = RequestAttribution::Human {
        actor: HumanAttribution {
            identity_id: fixture.requester,
            login: fixture.ctx.user.as_ref().unwrap().login.clone(),
        },
    };
    let mut source_admission = 0;
    let contributor_items = if contributor_case {
        super::c_native_contributors::capture(
            &mut tx,
            &queue,
            producer.id,
            original.id,
            weak_contributor,
        )
        .await?
    } else {
        vec![]
    };
    let producer_attribution = if contributor_case {
        super::c_native_contributors::attribution(&mut tx, &queue, &contributor_items).await?
    } else {
        RequestAttribution::ParentExecution {
            execution_id: root.id,
        }
    };
    for (execution, binding, pin, attribution) in [
        (root.id, &root_binding, &graph_pin, human.clone()),
        (
            producer.id,
            &producer_binding,
            &leaf_pin,
            producer_attribution,
        ),
    ] {
        let selected = policy.policy.select(
            policy.id,
            binding,
            if execution == producer.id {
                producer_refs.as_slice()
            } else {
                &[]
            },
            attribution,
        )?;
        let admission = RuntimeAuthorityRepository::record_admission(
            &mut tx,
            RuntimeOwner::Execution(execution),
            &selected,
        )
        .await?;
        if execution == producer.id {
            source_admission = admission.id;
            if contributor_case {
                assert_eq!(admission.authority.approval_ids().len(), 1);
            }
        }
        let schema = SchemaContractRepository::materialize_action_pair(&mut tx, pin).await?;
        SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution, &schema)
            .await?;
    }
    RecordedInputAuthorityRepository::capture_human(
        &mut tx,
        "execution",
        producer.id,
        fixture.requester,
    )
    .await?;
    attune_common::repositories::accepted_input::AcceptedInputRepository::seal_execution(
        &mut tx,
        producer.id,
        key,
    )
    .await?;
    RuntimeReadRepository::close_execution(&mut tx, producer.id).await?;
    ExecutionRepository::update(
        &mut *tx,
        producer.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    let wait = WorkflowTaskWaitRepository::create_or_get(
        &mut tx,
        CreateWorkflowTaskWaitInput {
            workflow_execution: workflow.id,
            task_name: "observed".into(),
            target: kind.wait(source_id),
        },
    )
    .await?;
    let task = Admissions::select(
        &mut tx,
        pack.id,
        task_binding,
        &leaf_pin,
        &["recorded_emitter.c_related_source".into()],
        RequestAttribution::ParentExecution {
            execution_id: root.id,
        },
        RuntimeTarget::Component(ComponentTarget::Action, graph.id),
    )
    .await?;
    let declarations = DeclaredInputRepository::bindings(&mut tx, root.id).await?;
    let selector = declarations
        .iter()
        .find(|declaration| declaration.source_path.contains("/wait_for/"))
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
        "/target",
        &InputSite {
            execution: root.id,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Value,
            path: "/target".into(),
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
    if !original_decrypt || weak_contributor {
        assert!(prepared.is_err(),"a capable task/current reader cannot replace the original contributor's missing Key Decrypt consent");
        tx.rollback().await?;
        fixture.ctx.cleanup().await?;
        return Ok(());
    }
    let prepared = prepared?;
    let declaration = declarations
        .iter()
        .find(|declaration| declaration.source_path.ends_with("/params/token"))
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
    assert_eq!(rendered.value(), &json!("related-wait-original"));
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
            variables: Some(json!({"target":source_id,"saved":redaction_marker()})),
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
    let saved =
        Inputs::prepare_source(&mut tx, root.id, &variable_site, &variable.origin(), key).await?;
    let declaration = declarations
        .iter()
        .find(|declaration| declaration.source_path.ends_with("/output_map/token"))
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
    Inputs::bind_prepared(&mut session, "/saved", &saved, rendered.value())?;
    let output = Inputs::evaluate(&mut tx, session).await?;
    let site = InputSite {
        execution: root.id,
        kind: InputEntityKind::ExecutionResult,
        purpose: InputSitePurpose::Value,
        path: "/token".into(),
    };
    let destination = Inputs::prepare_destination(&mut tx, &output, &site, key).await?;
    let receipt = Inputs::record_prepared(
        &mut tx,
        &output,
        &destination,
        output.value(),
        output.selections(),
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
    let fields = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/token".into(),
            value: output.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&receipt.provenance())?),
        }],
        key,
    )?;
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        ENTITY_EXECUTION_RESULT,
        root.id,
        &fields,
    )
    .await?;
    let mut tokens = Vec::new();
    let mut reader_executions = Vec::new();
    for reference in &reader_refs {
        let refs = vec![reference.clone()];
        let selected = policy
            .policy
            .select(policy.id, &leaf_binding, &refs, human.clone())?;
        let credential =
            super::c_native_execution_wait::capture_reader(&mut tx, &leaf_pin, &selected).await?;
        reader_executions.push(credential.execution_id);
        tokens.push(generate_admitted_execution_token(
            &credential,
            &refs,
            &fixture.ctx.state.jwt_config,
            Some(360),
        )?);
    }
    let rows = ExecutionSecretValueRepository::find_bounded_provenance(
        &mut *tx,
        &[ENTITY_EXECUTION_RESULT],
        &[root.id],
    )
    .await?;
    let mut frontier = InputReceiptFrontier::default();
    frontier.prepare(&mut tx, &rows, Some(key)).await?;
    let leaves = frontier.disclosure_leaves(&site, &receipt.origin())?;
    let contributor_evidence = if contributor_case {
        let context = Inputs::receipt_context(&mut tx, &site, &receipt.origin(), key).await?;
        let metadata = context
            .native_source_metadata()
            .iter()
            .find(|metadata| metadata.source_kind() == kind.source())
            .unwrap();
        let evidence = Snapshots::source_evidence(&mut tx, metadata, key).await?;
        assert_eq!(evidence.contributor_manifests().len(), 3);
        let identities = evidence
            .contributor_manifests()
            .iter()
            .map(|manifest| manifest.body()["identity_id"].as_i64().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            identities.len() == 3 && !identities.contains(&owner.sponsor_identity_id),
            "every original ceiling keeps its independent non-sponsor identity"
        );
        Some(evidence)
    } else {
        None
    };
    let origin = SecretOrigin::Entity {
        entity_type: kind.source().into(),
        entity_id: source_id,
        path: String::new(),
    };
    let held = frontier.prepared_related_wait(&leaves, &origin)?.unwrap();
    assert_eq!(held.source_admission(), source_admission);
    let target = RuntimeTargetRepository::resolve_related_wait_source(
        &mut tx,
        &held,
        owner.sponsor_identity_id,
    )
    .await?;
    assert_eq!(target.resource(), kind.resource());
    assert!(!target.accepts_new_effects());
    assert!(
        RuntimeTargetRepository::resolve_execution_wait_source(
            &mut tx,
            &held,
            owner.sponsor_identity_id,
        )
        .await
        .is_err(),
        "related native sources cannot enter the execution-result target factory"
    );
    for wrong in ["execution_result", "inquiry_response", "queue_item_result"]
        .into_iter()
        .filter(|wrong| *wrong != kind.source())
    {
        assert!(frontier
            .prepared_related_wait(
                &leaves,
                &SecretOrigin::Entity {
                    entity_type: wrong.into(),
                    entity_id: source_id,
                    path: String::new()
                }
            )?
            .is_none());
    }
    assert!(frontier
        .prepared_related_wait(
            &leaves,
            &SecretOrigin::Entity {
                entity_type: kind.source().into(),
                entity_id: source_id,
                path: "/token".into()
            }
        )?
        .is_none());
    tx.commit().await?;
    let path = format!("/api/v1/executions/{}?include_secret_values=true", root.id);
    let mut unrelated = fixture.ctx.pool.begin().await?;
    assert!(
        RuntimeTargetRepository::resolve_related_wait_source(
            &mut unrelated,
            &held,
            owner.sponsor_identity_id
        )
        .await
        .is_err(),
        "native source scope cannot be replayed on a different transaction"
    );
    unrelated.rollback().await?;
    let (status, body) = json_body(fixture.ctx.get(&path, Some(&tokens[0])).await?).await?;
    if status != StatusCode::OK {
        panic!("native related wait positive: {status}; bounded error body={body}");
    }
    assert_eq!(body["data"]["result"]["token"], "related-wait-original");
    for token in &tokens[1..] {
        assert_eq!(
            fixture.ctx.get(&path, Some(token)).await?.status(),
            StatusCode::FORBIDDEN
        );
    }
    let login = format!("c_related_human_{}", uuid::Uuid::new_v4().simple());
    let registered = fixture
        .ctx
        .post(
            "/auth/register",
            json!({"login":login,"password":"TestPassword123!","display_name":"Related reader"}),
            None,
        )
        .await?;
    assert!(registered.status().is_success());
    let body: Value = registered.json().await?;
    let human_token = body["data"]["access_token"].as_str().unwrap().to_owned();
    let human_reader = IdentityRepository::find_by_login(&fixture.ctx.pool, &login)
        .await?
        .unwrap();
    let grants=PermissionSetRepository::create(&fixture.ctx.pool,CreatePermissionSetInput {
        r#ref:format!("fixture.{login}"),pack:None,pack_ref:None,label:None,description:None,grants:json!([
            {"resource":"executions","actions":["read","decrypt"],"constraints":{"ids":[root.id]}},
            {"resource":"keys","actions":["read","decrypt"],"constraints":{"ids":[original.id]}}
        ]),
    }).await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: human_reader.id,
            permset: grants.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(human_reader.id).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    // Keep execution and Key rights fixed. Replace, never union, the actual
    // distinct inquiry/item grant at each reader boundary.
    let mut previous = None;
    for (index, actions) in [
        json!(["decrypt"]),
        json!(["read"]),
        json!(["read", "decrypt"]),
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(assignment) = previous {
            assert!(PermissionAssignmentRepository::delete(&fixture.ctx.pool, assignment).await?);
        }
        let source_set = PermissionSetRepository::create(&fixture.ctx.pool, CreatePermissionSetInput {
            r#ref: format!("fixture.{login}_source_{index}"), pack:None, pack_ref:None, label:None, description:None,
            grants: json!([{"resource":kind.resource_name(),"actions":actions,"constraints":{"ids":[source_id]}}]),
        }).await?;
        let assignment = PermissionAssignmentRepository::create(
            &fixture.ctx.pool,
            CreatePermissionAssignmentInput {
                identity: human_reader.id,
                permset: source_set.id,
            },
        )
        .await?;
        previous = Some(assignment.id);
        attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(human_reader.id)
            .await;
        attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
        if index < 2 {
            let (status, body) =
                json_body(fixture.ctx.get(&path, Some(&human_token)).await?).await?;
            assert!(status==StatusCode::FORBIDDEN || status==StatusCode::OK && body["data"]["result"]["token"]==redaction_marker(),
                "missing human source Read or Decrypt must deny or retain the explicit redaction marker, never disclose plaintext");
            assert!(!body.to_string().contains("related-wait-original"),
                "a missing human source right must not disclose the original value anywhere in the bounded body");
        }
    }
    let (status, body) = json_body(fixture.ctx.get(&path, Some(&human_token)).await?).await?;
    if status != StatusCode::OK {
        panic!("native related human positive: {status}; bounded error body={body}");
    }
    assert_eq!(body["data"]["result"]["token"], "related-wait-original");
    let before = ExecutionSecretValueRepository::find_stored_by_entity(
        &fixture.ctx.pool,
        ENTITY_EXECUTION_RESULT,
        root.id,
    )
    .await?;
    let mut gc = fixture.ctx.pool.begin().await?;
    Evidence::lock_reclamation(&mut gc).await?;
    for item in &contributor_items {
        assert!(WorkQueueItemRepository::delete(&mut *gc, *item).await?);
        assert!(WorkQueueItemRepository::find_by_id(&mut *gc, *item)
            .await?
            .is_none());
    }
    match kind {
        Kind::Inquiry => {
            assert!(InquiryRepository::delete(&mut *gc, source_id).await?);
            assert!(InquiryRepository::find_by_id(&mut *gc, source_id)
                .await?
                .is_none());
        }
        Kind::Queue => {
            assert!(WorkQueueItemRepository::delete(&mut *gc, source_id).await?);
            assert!(WorkQueueItemRepository::find_by_id(&mut *gc, source_id)
                .await?
                .is_none());
        }
    }
    assert!(ExecutionRepository::delete(&mut *gc, producer.id).await?);
    assert!(ExecutionRepository::find_by_id(&mut *gc, producer.id)
        .await?
        .is_none());
    gc.commit().await?;
    KeyRepository::update(
        &fixture.ctx.pool,
        original.id,
        UpdateKeyInput {
            value: Some(attune_common::crypto::keystore::encrypt_json(
                &json!("wrong-current-value"),
                key,
                original.id,
            )?),
            ..Default::default()
        },
    )
    .await?;
    if let Some(at) = transport {
        super::c_native_transport::exercise(
            &mut fixture,
            root.id,
            reader_executions[0],
            policy.id,
            contributor_deadline,
            at,
        )
        .await?;
        if let Some(evidence) = &contributor_evidence {
            let mut remaining = fixture.ctx.pool.begin().await?;
            assert!(
                RuntimeAuthorityRepository::retained_admission_fence(
                    &mut remaining,
                    source_admission
                )
                .await
                .is_err(),
                "the expired admission is the original collected creator/requester, not the reader"
            );
            let authorities = evidence
                .contributor_manifests()
                .iter()
                .map(|manifest| {
                    Ok(attune_common::delegation::DelegationAuthority::new(
                        manifest.body()["identity_id"].as_i64().unwrap(),
                        serde_json::from_value(manifest.body()["attributes"].clone())?,
                        serde_json::from_value(manifest.body()["grants"].clone())?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            let keys = evidence
                .origins()
                .iter()
                .filter(|origin| matches!(origin, SecretOrigin::Key(_)))
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(keys.len(),1,"the remaining independent ceilings must be checked against the actual original Key origin");
            attune_common::repositories::immutable_input_origin::ImmutableInputOriginRepository::require(&mut remaining,&authorities,&keys).await?;
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
    for token in [&tokens[0], &human_token] {
        let (status, body) = json_body(fixture.ctx.get(&path, Some(token)).await?).await?;
        if status != StatusCode::OK {
            panic!("native related archived positive: {status}; bounded error body={body}");
        }
        assert_eq!(body["data"]["result"]["token"], "related-wait-original");
    }
    assert_eq!(
        before,
        ExecutionSecretValueRepository::find_stored_by_entity(
            &fixture.ctx.pool,
            ENTITY_EXECUTION_RESULT,
            root.id
        )
        .await?
    );
    let mut check = fixture.ctx.pool.begin().await?;
    let references =
        RuntimeReadRepository::references_for_executions(&mut *check, &[reader_executions[0]])
            .await?;
    let mut found = false;
    for origin in references.values().flatten() {
        let SecretOrigin::Entity { entity_id, .. } = origin else {
            unreachable!()
        };
        let read = RuntimeReadRepository::find(&mut *check, *entity_id).await?;
        read.validate()?;
        if read.parents.iter().any(|parent| {
            parent.entity_type == kind.source()
                && parent.entity_id == source_id
                && parent.path.is_empty()
        }) {
            found = true;
        }
    }
    assert!(found,"actual HTTP readers must commit a complete capture with the original distinct source kind and ID");
    RuntimeAuthorityRepository::revoke_admission(&mut check, source_admission).await?;
    check.commit().await?;
    for token in [&tokens[0], &human_token] {
        let (status, _) = json_body(fixture.ctx.get(&path, Some(token)).await?).await?;
        assert!(!status.is_success());
    }
    fixture.ctx.cleanup().await?;
    Ok(())
}
