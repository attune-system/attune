//! C consumer coverage. Uses A's committed receipt fixture, not a second renderer.
use super::*;
use attune_common::{
    repositories::{
        accepted_input::AcceptedInputRepository,
        declared_input::DeclaredInputRepository,
        execution_secret_value::ExecutionSecretValueRepository,
        immutable_input_origin::{
            bind_input_receipt_buffers, input_value_site, prepare_input_receipts,
            InputReceiptFrontier,
        },
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        key::KeyRepository,
        managed_executable::ManagedExecutableRepository,
        recorded_input_authority::RecordedInputAuthorityRepository,
        runtime_target::{RuntimeTarget, RuntimeTargetRepository},
        schema_contract::{BindingOwner, SchemaContractRepository},
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            WorkflowDefinitionRepository, WorkflowExecutionRepository,
        },
    },
    secret_values::{
        prepare_secret_values, redaction_marker, restore_secret_values, SecretValueInput,
    },
};

fn assert_denied<T>(result: attune_common::Result<T>) {
    match result {
        Err(attune_common::Error::PermissionDenied(_)) => {}
        Err(error) => panic!("Expected permission denial, got {error}"),
        Ok(_) => panic!("Unexpected receipt authority"),
    }
}

#[tokio::test]
async fn immutable_configuration_root_keeps_original_field_and_independent_input_ceilings() {
    use attune_common::{
        delegation::DelegationAuthority,
        repositories::{
            component_configuration::{
                ComponentConfigurationRepository as Configurations, ENTITY_COMPONENT_CONFIGURATION,
            },
            immutable_input_origin::ImmutableInputOriginRepository as Origins,
        },
        secret_provenance::{SecretOrigin, SecretProvenance},
    };
    let (db, fixture) = fixture().await;
    let mut source = PackFixture::new_unique("c_original_configuration");
    source.conf_schema =
        json!({"token":{"type":"string","secret":true},"later":{"type":"string","secret":true}});
    source.config = json!({"token":"original-configuration"});
    let source = source.create(&db).await.unwrap();
    let grants = json!([{"resource":"packs","actions":["read","decrypt"],"constraints":{"refs":[source.r#ref]}}]);
    let set = PermissionSetRepository::create(
        &db,
        CreatePermissionSetInput {
            r#ref: "fixture.c_original_configuration".into(),
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
            identity: fixture.human_id,
            permset: set.id,
        },
    )
    .await
    .unwrap();
    let key = "c-original-configuration-test-owned-key";
    let mut tx = db.begin().await.unwrap();
    let original = Configurations::bind_pack(
        &mut tx,
        &source,
        Some(&json!({"token":"original-configuration"})),
        Some(key),
    )
    .await
    .unwrap();
    let before = ExecutionSecretValueRepository::find_stored_at(
        &mut tx,
        ENTITY_COMPONENT_CONFIGURATION,
        original.id,
        "/token",
    )
    .await
    .unwrap()
    .unwrap();
    RecordedInputAuthorityRepository::capture_human(
        &mut tx,
        "execution",
        fixture.credential.execution_id,
        fixture.human_id,
    )
    .await
    .unwrap();
    let authority = RecordedInputAuthorityRepository::load(
        &mut tx,
        "execution",
        fixture.credential.execution_id,
    )
    .await
    .unwrap()
    .unwrap();
    let origin = SecretOrigin::Entity {
        entity_type: ENTITY_COMPONENT_CONFIGURATION.into(),
        entity_id: original.id,
        path: "/token".into(),
    };
    Origins::require(
        &mut tx,
        std::slice::from_ref(&authority),
        std::slice::from_ref(&origin),
    )
    .await
    .unwrap();
    let narrowed = DelegationAuthority::new(
        fixture.human_id,
        Default::default(),
        serde_json::from_value(json!([
            {"resource":"packs","actions":["read"],"constraints":{"refs":[source.r#ref]}}
        ]))
        .unwrap(),
    );
    assert_denied(
        Origins::require(
            &mut tx,
            &[authority.clone(), narrowed],
            std::slice::from_ref(&origin),
        )
        .await,
    );
    let current = Configurations::bind_pack(
        &mut tx,
        &source,
        Some(&json!({"token":"fresh-current-configuration","later":"only-in-current"})),
        Some(key),
    )
    .await
    .unwrap();
    assert_ne!(current.id, original.id);
    Origins::require(
        &mut tx,
        std::slice::from_ref(&authority),
        std::slice::from_ref(&origin),
    )
    .await
    .unwrap();
    assert_eq!(
        Configurations::restore(&mut tx, &original, Some(key))
            .await
            .unwrap(),
        json!({"token":"original-configuration"})
    );
    assert!(Configurations::restore(
        &mut tx,
        &original,
        Some("c-wrong-original-configuration-test-key")
    )
    .await
    .is_err());
    assert_eq!(
        Configurations::restore(&mut tx, &current, Some(key))
            .await
            .unwrap(),
        json!({"token":"fresh-current-configuration","later":"only-in-current"})
    );
    let after = ExecutionSecretValueRepository::find_stored_at(
        &mut tx,
        ENTITY_COMPONENT_CONFIGURATION,
        original.id,
        "/token",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        before, after,
        "authorization and rotation preserve the exact original ciphertext/provenance"
    );
    for (kind, path) in [
        (ENTITY_COMPONENT_CONFIGURATION, "/missing"),
        (ENTITY_COMPONENT_CONFIGURATION, "/later"),
        ("component_configuration_current", "/token"),
    ] {
        assert_denied(
            Origins::require(
                &mut tx,
                std::slice::from_ref(&authority),
                &[SecretOrigin::Entity {
                    entity_type: kind.into(),
                    entity_id: original.id,
                    path: path.into(),
                }],
            )
            .await,
        );
    }
    let later = SecretOrigin::Entity {
        entity_type: ENTITY_COMPONENT_CONFIGURATION.into(),
        entity_id: current.id,
        path: "/later".into(),
    };
    Origins::require(&mut tx, std::slice::from_ref(&authority), &[later])
        .await
        .unwrap();
    IdentityRepository::update(
        &mut *tx,
        fixture.human_id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    Origins::require(&mut tx, &[authority], &[origin])
        .await
        .unwrap();
    // Structural freeze negatives are tested by the canonical map selector;
    // RuntimeReadOrigin itself also refuses missing/wrong snapshot parents.
    let incomplete = attune_common::repositories::runtime_read::RuntimeReadOrigin {
        target: attune_common::repositories::runtime_read::RuntimeReadTarget::ExecutionMetadata {
            execution: fixture.credential.execution_id,
        },
        provenance: vec![SecretProvenance {
            origins: vec![SecretOrigin::Entity {
                entity_type: ENTITY_COMPONENT_CONFIGURATION.into(),
                entity_id: original.id,
                path: "/token".into(),
            }],
            templates: vec![],
        }],
        parents: vec![],
    };
    assert_denied(incomplete.validate());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_outcome_receipt_reaches_delivery_and_disclosure_frontier_without_result_plaintext()
{
    use attune_common::{
        models::WorkflowTaskMetadata,
        repositories::{
            action::ActionRepository, executable_snapshot::ExecutableSnapshotRepository,
        },
        runtime_authority::{InvocationBinding, InvocationPolicy, RuntimePolicy},
    };
    let expression = "{{ succeeded() }}";
    let (db, fixture) = fixture().await;
    let publisher = PackFixture::new_unique("c_native_outcome")
        .create(&db)
        .await
        .unwrap();
    let workflow = ActionFixture::new(publisher.id, &publisher.r#ref, "graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(publisher.id, &publisher.r#ref, "leaf")
        .create(&db)
        .await
        .unwrap();
    let graph = WorkflowDefinitionRepository::create(&db, CreateWorkflowDefinitionInput {
        r#ref: workflow.r#ref.clone(), pack: publisher.id, pack_ref: publisher.r#ref.clone(),
        label: "Native outcome consumer".into(), description: None, version: "1.0.0".into(),
        param_schema: None, out_schema: None, tags: vec![],
        definition: json!({"output_map":{"ok":expression},"tasks":[{"name":"leaf","action":leaf.r#ref}]}),
    }).await.unwrap();
    ActionRepository::link_workflow_def(&db, workflow.id, graph.id)
        .await
        .unwrap();
    let key = "c-native-outcome-consumer-owned-encryption-key";
    let mut tx = db.begin().await.unwrap();
    let release = activate_release(
        &mut tx,
        publisher.id,
        &publisher.r#ref,
        vec![workflow.id, leaf.id],
        'c',
    )
    .await;
    let owner = RuntimeAuthorityRepository::ensure_owner(&mut tx, publisher.id)
        .await
        .unwrap();
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let bindings = [
        InvocationBinding::Action {
            action_ref: workflow.r#ref.clone(),
        },
        InvocationBinding::WorkflowTask {
            workflow_ref: workflow.r#ref.clone(),
            task_path: "/tasks/leaf".into(),
            action_ref: leaf.r#ref.clone(),
        },
        InvocationBinding::Action {
            action_ref: leaf.r#ref.clone(),
        },
    ];
    let policy = RuntimePolicy {
        owner,
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: Default::default(),
        invocations: bindings
            .iter()
            .map(|binding| {
                InvocationPolicy::from_executable(
                    binding.clone(),
                    &snapshots[binding.action_ref()],
                    Some(&[]),
                    &Default::default(),
                )
                .unwrap()
            })
            .collect(),
    };
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let mut executions = Vec::new();
    let mut workflow_execution = None;
    for (index, binding) in bindings.iter().enumerate().take(2) {
        let pin = ManagedExecutableRepository::resolve_invocation(&mut tx, &policy.policy, binding)
            .await
            .unwrap();
        let schema = SchemaContractRepository::materialize_action_pair(&mut tx, &pin)
            .await
            .unwrap();
        let selection = policy
            .policy
            .select(
                policy.id,
                binding,
                &[],
                if index == 0 {
                    fixture.admission.authority.attribution.clone()
                } else {
                    RequestAttribution::ParentExecution {
                        execution_id: executions[0],
                    }
                },
            )
            .unwrap();
        assert!(!selection.has_api_access());
        let execution = ExecutionRepository::create_pinned(
            &mut *tx,
            CreateExecutionInput {
                action: Some(pin.executable.action.id),
                action_ref: pin.executable.action.r#ref.clone(),
                status: ExecutionStatus::Running,
                permission_set_refs: vec![],
                config: Some(json!({})),
                parent: executions.first().copied(),
                workflow_task: workflow_execution.map(|workflow_execution| WorkflowTaskMetadata {
                    workflow_execution,
                    task_name: "leaf".into(),
                    triggered_by: None,
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
                }),
                ..Default::default()
            },
            &pin,
        )
        .await
        .unwrap();
        RuntimeAuthorityRepository::record_admission(
            &mut tx,
            RuntimeOwner::Execution(execution.id),
            &selection,
        )
        .await
        .unwrap();
        SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &schema)
            .await
            .unwrap();
        if let Some(parent) = executions.first() {
            RecordedInputAuthorityRepository::copy(
                &mut tx,
                "execution",
                *parent,
                "execution",
                execution.id,
            )
            .await
            .unwrap();
        } else {
            RecordedInputAuthorityRepository::capture_human(
                &mut tx,
                "execution",
                execution.id,
                fixture.human_id,
            )
            .await
            .unwrap();
        }
        AcceptedInputRepository::seal_execution(&mut tx, execution.id, key)
            .await
            .unwrap();
        executions.push(execution.id);
        if index == 0 {
            workflow_execution = Some(
                WorkflowExecutionRepository::create(
                    &mut *tx,
                    CreateWorkflowExecutionInput {
                        execution: execution.id,
                        workflow_def: graph.id,
                        task_graph: json!({}),
                        variables: json!({}),
                        status: ExecutionStatus::Running,
                    },
                )
                .await
                .unwrap()
                .id,
            );
        }
    }
    let (renderer, child) = (executions[0], executions[1]);
    ExecutionRepository::update(
        &mut *tx,
        child,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            result: Some(json!({"unrelated_secret":redaction_marker()})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // The native status receipt observes completion, not this child's unrelated
    // result plaintext, and must not need a fabricated root secret-parent row.
    let unrelated = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/unrelated_secret".into(),
            value: json!("not-an-outcome-source"),
            source_kind: "provenance".into(),
            source_ref: Some(
                serde_json::to_string(
                    &attune_common::secret_values::SecretSource::ParameterSchema {
                        path: "/unrelated_secret".into(),
                    }
                    .provenance(),
                )
                .unwrap(),
            ),
        }],
        key,
    )
    .unwrap();
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut tx,
        "execution_result",
        child,
        &unrelated,
    )
    .await
    .unwrap();
    let declarations = DeclaredInputRepository::bindings(&mut tx, renderer)
        .await
        .unwrap();
    let declaration = declarations
        .iter()
        .find(|value| value.expression == expression)
        .unwrap();
    let outcome = Inputs::prepare_outcome(&mut tx, renderer, child)
        .await
        .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        renderer,
        &declaration.declaration_revision,
        &declaration.source_path,
        expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_outcome(&mut session, &outcome).unwrap();
    let rendered = Inputs::evaluate(&mut tx, session).await.unwrap();
    assert_eq!(rendered.value(), &json!(true));
    let site = input_value_site("execution_result", renderer, "/ok").unwrap();
    let receipt = Inputs::record_render(
        &mut tx,
        &rendered,
        &site,
        rendered.value(),
        rendered.selections(),
        key,
    )
    .await
    .unwrap();
    let secrets = prepare_secret_values(
        vec![SecretValueInput {
            json_path: site.path.clone(),
            value: json!(true),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&receipt.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        renderer,
        UpdateExecutionInput {
            result: Some(json!({"ok":redaction_marker()})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut tx,
        "execution_result",
        renderer,
        &secrets,
    )
    .await
    .unwrap();
    let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *tx,
        "execution_result",
        renderer,
    )
    .await
    .unwrap();
    let (ordinary, contexts) =
        prepare_input_receipts(&mut tx, "execution_result", renderer, &secrets, key)
            .await
            .unwrap();
    assert!(ordinary.is_empty());
    assert_eq!(contexts.len(), 1);
    // The original field frame alone cannot cover its typed receipt Entity.
    // Capture must add the authenticated leaf without removing that frame.
    let mut frozen_read = attune_common::repositories::runtime_read::RuntimeReadOrigin {
        target: attune_common::repositories::runtime_read::RuntimeReadTarget::Execution {
            execution: renderer,
        },
        provenance: vec![attune_common::secret_provenance::SecretProvenance {
            origins: vec![attune_common::secret_provenance::SecretOrigin::Entity {
                entity_type: "execution_result".into(),
                entity_id: renderer,
                path: "/ok".into(),
            }],
            templates: vec![],
        }],
        parents: vec![
            attune_common::repositories::runtime_read::RuntimeReadParent {
                entity_type: "execution_result".into(),
                entity_id: renderer,
                path: "/ok".into(),
                provenance: receipt.provenance(),
            },
        ],
    };
    assert_denied(frozen_read.validate());
    assert_eq!(frozen_read.parents[0].provenance, receipt.provenance());
    bind_input_receipt_buffers(&mut tx, "execution_result", renderer, &secrets)
        .await
        .unwrap();
    assert_eq!(
        restore_secret_values(json!({"ok":redaction_marker()}), &secrets, key).unwrap(),
        json!({"ok":true})
    );
    let rows = ExecutionSecretValueRepository::find_bounded_provenance(
        &mut *tx,
        &["execution_result"],
        &[renderer],
    )
    .await
    .unwrap();
    let mut frontier = InputReceiptFrontier::default();
    frontier.prepare(&mut tx, &rows, Some(key)).await.unwrap();
    let context = frontier.prepared_receipt(&site, &receipt.origin()).unwrap();
    let frozen =
        attune_common::repositories::immutable_input_origin::freeze_receipt_context(context)
            .unwrap();
    assert!(frozen.iter().any(|parent| {
        let attune_common::secret_provenance::SecretOrigin::Entity {
            entity_type,
            entity_id,
            path,
        } = receipt.origin()
        else {
            unreachable!()
        };
        parent.entity_type == entity_type
            && parent.entity_id == entity_id
            && parent.path == path
            && parent.provenance.origins == context.source_origins()
    }));
    frozen_read.parents.extend(frozen);
    frozen_read.validate().unwrap();
    let encoded = serde_json::to_value(&frozen_read).unwrap();
    let reloaded: attune_common::repositories::runtime_read::RuntimeReadOrigin =
        serde_json::from_value(encoded.clone()).unwrap();
    reloaded.validate().unwrap();
    assert_eq!(serde_json::to_value(&reloaded).unwrap(), encoded);
    // Exercise the real capture digest/storage/reload boundary as well as the
    // strict shape validator. This is not a reader-permission positive.
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await
            .unwrap();
    let captured = attune_common::repositories::runtime_read::RuntimeReadRepository::record(
        &mut tx,
        &authority,
        frozen_read.clone(),
    )
    .await
    .unwrap();
    let attune_common::secret_provenance::SecretOrigin::Entity {
        entity_id: captured_id,
        ..
    } = captured
    else {
        unreachable!()
    };
    let stored_read = attune_common::repositories::runtime_read::RuntimeReadRepository::find(
        &mut *tx,
        captured_id,
    )
    .await
    .unwrap();
    assert_eq!(serde_json::to_value(stored_read).unwrap(), encoded);
    for index in 1..frozen_read.parents.len() {
        let mut missing = frozen_read.clone();
        missing.parents.remove(index);
        assert_denied(missing.validate());
    }
    let mut mismatched = frozen_read.clone();
    mismatched.parents[1].path = "/different".into();
    assert_denied(mismatched.validate());
    let leaves = frontier
        .disclosure_leaves(&site, &receipt.origin())
        .unwrap();
    assert!(leaves.is_outcome_metadata("execution_result", child, ""));
    assert!(!leaves.is_outcome_metadata("execution_result", child, "/unrelated_secret"));
    assert!(!leaves.is_outcome_metadata("execution_result", renderer, ""));
    assert!(!leaves.is_outcome_metadata("execution_config", child, ""));
    assert!(!leaves.is_public_source("execution_result", child, ""));
    let wrong_site = InputSite {
        purpose: InputSitePurpose::Control,
        ..site.clone()
    };
    assert_denied(Inputs::receipt_context(&mut tx, &wrong_site, &receipt.origin(), key).await);
    let admission = RuntimeAuthorityRepository::execution_admission(&mut tx, child)
        .await
        .unwrap()
        .unwrap();
    RuntimeAuthorityRepository::revoke_admission(&mut tx, admission.id)
        .await
        .unwrap();
    assert_denied(
        InputReceiptFrontier::default()
            .prepare(&mut tx, &rows, Some(key))
            .await,
    );
    assert_denied(
        prepare_input_receipts(&mut tx, "execution_result", renderer, &secrets, key).await,
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn foreign_input_result_to_variables_to_child_has_exact_delivery_sites_and_no_api_grants() {
    let expressions = [
        "{{ keystore[parameters.selector] }}",
        "{{ result().token }}",
        "{{ saved_token }}",
    ];
    let (db, fixture) = fixture_with_input_defaults(Some(json!({
        "selected":{"type":"string","default":expressions[0]},
        "from_result":{"type":"string","default":expressions[1]},
        "from_variable":{"type":"string","default":expressions[2]},
        "token":{"type":"string","secret":true},
        "selector":{"type":"string"}
    })))
    .await;
    // The physical test database owns this reference. The real evaluator must
    // retain the public selector beside the protected foreign Key source.
    let foreign = helpers::KeyFixture::new_system("c_receipt_source", "foreign-purpose-value")
        .create(&db)
        .await
        .unwrap();
    // Issuance needs the human's exact setup grant. Execution API authority
    // remains empty and later purpose delivery uses the fixed approval.
    let setup = PermissionSetRepository::create(
        &db,
        CreatePermissionSetInput {
            r#ref: "fixture.c_receipt_setup".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[foreign.r#ref]}}]),
        },
    ).await.unwrap();
    PermissionAssignmentRepository::create(
        &db,
        CreatePermissionAssignmentInput {
            identity: fixture.human_id,
            permset: setup.id,
        },
    )
    .await
    .unwrap();
    let key = "c-owned-inherited-receipt-encryption-key";
    let mut tx = db.begin().await.unwrap();
    let binding = &fixture.policy.policy.invocations[0].binding;
    let pin =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &fixture.policy.policy, binding)
            .await
            .unwrap();
    let selected = fixture
        .policy
        .policy
        .select(
            fixture.policy.id,
            binding,
            &[],
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    let parent = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(pin.executable.action.id),
            action_ref: pin.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            permission_set_refs: vec![],
            config: Some(json!({"selector":foreign.r#ref})),
            ..Default::default()
        },
        &pin,
    )
    .await
    .unwrap();
    let parent_admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(parent.id),
        &selected,
    )
    .await
    .unwrap();
    assert!(!parent_admission.authority.has_api_access());
    let schema = SchemaContractRepository::materialize_action_pair(&mut tx, &pin)
        .await
        .unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, parent.id, &schema)
        .await
        .unwrap();
    RecordedInputAuthorityRepository::capture_human(
        &mut tx,
        "execution",
        parent.id,
        fixture.human_id,
    )
    .await
    .unwrap();
    AcceptedInputRepository::seal_execution(&mut tx, parent.id, key)
        .await
        .unwrap();
    let declarations = DeclaredInputRepository::bindings(&mut tx, parent.id)
        .await
        .unwrap();
    let declaration = |expression: &str| {
        declarations
            .iter()
            .find(|d| d.expression == expression)
            .unwrap()
    };
    let first = declaration(expressions[0]);
    let approval = Inputs::approve_external_input(
        &mut tx, parent.id, &first.declaration_revision, &first.source_path, expressions[0], fixture.human_id,
        serde_json::from_value(json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[foreign.r#ref]}}])).unwrap(),
        Some(Utc::now() + Duration::hours(1)),
    ).await.unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        parent.id,
        &first.declaration_revision,
        &first.source_path,
        expressions[0],
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_public_parameter(&mut tx, &mut session, "/selector")
        .await
        .unwrap();
    let rendered = Inputs::evaluate_with_keys(&mut tx, session, key)
        .await
        .unwrap();
    let result_site = input_value_site("execution_result", parent.id, "/token").unwrap();
    let result = Inputs::record_render(
        &mut tx,
        &rendered,
        &result_site,
        rendered.value(),
        rendered.selections(),
        key,
    )
    .await
    .unwrap();
    let stored_result = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/token".into(),
            value: foreign.value.clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&result.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        parent.id,
        UpdateExecutionInput {
            result: Some(json!({"token":redaction_marker()})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut tx,
        "execution_result",
        parent.id,
        &stored_result,
    )
    .await
    .unwrap();

    let source = Inputs::prepare_source(&mut tx, parent.id, &result_site, &result.origin(), key)
        .await
        .unwrap();
    let next = declaration(expressions[1]);
    let mut session = Inputs::begin_render(
        &mut tx,
        parent.id,
        &next.declaration_revision,
        &next.source_path,
        expressions[1],
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_prepared(&mut session, "/result/token", &source, &foreign.value).unwrap();
    let rendered = Inputs::evaluate(&mut tx, session).await.unwrap();
    let variable_site = input_value_site("workflow_variables", parent.id, "/token").unwrap();
    let variable = Inputs::record_render(
        &mut tx,
        &rendered,
        &variable_site,
        rendered.value(),
        rendered.selections(),
        key,
    )
    .await
    .unwrap();
    let definition = WorkflowDefinitionRepository::create(
        &mut *tx,
        CreateWorkflowDefinitionInput {
            r#ref: format!(
                "{}.c_receipt_variables",
                fixture.policy.policy.owner.pack_ref
            ),
            pack: fixture.policy.policy.owner.pack_id,
            pack_ref: fixture.policy.policy.owner.pack_ref.clone(),
            label: "Receipt variables".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            definition: json!({"tasks":[]}),
            tags: vec![],
        },
    )
    .await
    .unwrap();
    WorkflowExecutionRepository::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: parent.id,
            workflow_def: definition.id,
            task_graph: json!({}),
            variables: json!({"token":redaction_marker()}),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    let stored_variable = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/token".into(),
            value: foreign.value.clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&variable.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut tx,
        "workflow_variables",
        parent.id,
        &stored_variable,
    )
    .await
    .unwrap();

    let mut children = Vec::new();
    for index in 0..2 {
        let child = ExecutionRepository::create_pinned(
            &mut *tx,
            CreateExecutionInput {
                action: Some(pin.executable.action.id),
                action_ref: pin.executable.action.r#ref.clone(),
                status: ExecutionStatus::Running,
                parent: Some(parent.id),
                permission_set_refs: vec![],
                config: Some(if index == 0 {
                    json!({"token":redaction_marker()})
                } else {
                    json!({})
                }),
                ..Default::default()
            },
            &pin,
        )
        .await
        .unwrap();
        SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, child.id, &schema)
            .await
            .unwrap();
        RecordedInputAuthorityRepository::copy(
            &mut tx,
            "execution",
            parent.id,
            "execution",
            child.id,
        )
        .await
        .unwrap();
        let selection = fixture
            .policy
            .policy
            .select(
                fixture.policy.id,
                binding,
                &[],
                RequestAttribution::ParentExecution {
                    execution_id: parent.id,
                },
            )
            .unwrap();
        let admission = RuntimeAuthorityRepository::record_admission(
            &mut tx,
            RuntimeOwner::Execution(child.id),
            &selection,
        )
        .await
        .unwrap();
        if index == 1 {
            AcceptedInputRepository::seal_execution(&mut tx, child.id, key)
                .await
                .unwrap();
        }
        assert_eq!(
            admission.authority.owner.sponsor_identity_id,
            parent_admission.authority.owner.sponsor_identity_id
        );
        assert!(!admission.authority.has_api_access());
        children.push(child);
    }
    let source =
        Inputs::prepare_source(&mut tx, parent.id, &variable_site, &variable.origin(), key)
            .await
            .unwrap();
    let last = declaration(expressions[2]);
    let mut session = Inputs::begin_render(
        &mut tx,
        parent.id,
        &last.declaration_revision,
        &last.source_path,
        expressions[2],
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_prepared(&mut session, "/saved_token", &source, &foreign.value).unwrap();
    let rendered = Inputs::evaluate(&mut tx, session).await.unwrap();
    let child_site = input_value_site("execution_config", children[0].id, "/token").unwrap();
    let child_receipt = Inputs::record_render(
        &mut tx,
        &rendered,
        &child_site,
        rendered.value(),
        rendered.selections(),
        key,
    )
    .await
    .unwrap();
    let stored_child = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/token".into(),
            value: foreign.value.clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&child_receipt.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut tx,
        "execution_config",
        children[0].id,
        &stored_child,
    )
    .await
    .unwrap();
    AcceptedInputRepository::seal_execution(&mut tx, children[0].id, key)
        .await
        .unwrap();
    let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *tx,
        "execution_config",
        children[0].id,
    )
    .await
    .unwrap();
    assert_eq!(secrets.len(), 1);
    assert_eq!(secrets[0].encrypted_value, stored_child[0].encrypted_value);
    let delivered: attune_common::secret_provenance::SecretProvenance =
        serde_json::from_str(secrets[0].source_ref.as_deref().unwrap()).unwrap();
    assert_eq!(delivered.origins.len(), 1);
    let delivered_origin = &delivered.origins[0];
    assert_ne!(delivered_origin, &child_receipt.origin());
    let (ordinary, contexts) =
        prepare_input_receipts(&mut tx, "execution_config", children[0].id, &secrets, key)
            .await
            .unwrap();
    assert!(
        ordinary.is_empty(),
        "purpose delivery does not borrow API or human grants"
    );
    bind_input_receipt_buffers(&mut tx, "execution_config", children[0].id, &secrets)
        .await
        .unwrap();
    let mut wrong_buffer = secrets.clone();
    wrong_buffer[0].encrypted_value =
        attune_common::crypto::encrypt_json(&json!("not-the-receipt-value"), key).unwrap();
    assert_denied(
        bind_input_receipt_buffers(&mut tx, "execution_config", children[0].id, &wrong_buffer)
            .await,
    );
    assert_eq!(
        restore_secret_values(json!({"token":redaction_marker()}), &secrets, key).unwrap(),
        json!({"token":foreign.value})
    );
    for context in &contexts {
        context.ensure_fresh_on(&mut tx).await.unwrap();
        context.ensure_fresh().unwrap();
        assert_eq!(
            context.public_source_sites(),
            &[input_value_site("execution_config", parent.id, "/selector").unwrap()]
        );
        assert!(context.outcome_metadata().is_empty());
    }
    assert_denied(
        prepare_input_receipts(
            &mut tx,
            "execution_config",
            children[0].id,
            &secrets,
            "another-c-owned-receipt-encryption-key",
        )
        .await,
    );
    for wrong in [
        InputSite {
            kind: InputEntityKind::ExecutionResult,
            ..child_site.clone()
        },
        InputSite {
            purpose: InputSitePurpose::Condition,
            ..child_site.clone()
        },
        InputSite {
            path: "/other".into(),
            ..child_site.clone()
        },
        InputSite {
            execution: children[1].id,
            ..child_site.clone()
        },
    ] {
        assert_denied(Inputs::receipt_context(&mut tx, &wrong, delivered_origin, key).await);
    }
    assert_denied(
        prepare_input_receipts(&mut tx, "execution_config", children[1].id, &secrets, key).await,
    );
    assert_denied(
        prepare_input_receipts(&mut tx, "workflow_variables", children[0].id, &secrets, key).await,
    );

    let rows = ExecutionSecretValueRepository::find_bounded_provenance(
        &mut *tx,
        &["execution_config"],
        &[children[0].id],
    )
    .await
    .unwrap();
    let mut frontier = InputReceiptFrontier::default();
    frontier.prepare(&mut tx, &rows, Some(key)).await.unwrap();
    let leaves = frontier
        .disclosure_leaves(&child_site, delivered_origin)
        .unwrap();
    // The live field is now the first receipt, not this archived original.
    // Exercise C's actual prepared-source consumer, not just A's getter: it
    // must recover the original proof without traversing that replacement row.
    let original_source = leaves.source_receipt(&child_receipt.origin()).unwrap();
    assert!(leaves.source_descriptor_bytes().unwrap() > 0);
    assert_eq!(original_source.site(), &child_site);
    assert_eq!(original_source.origin(), child_receipt.origin());
    assert_ne!(&original_source.origin(), delivered_origin);
    let original_context = frontier.prepared_source(original_source).unwrap();
    assert_eq!(original_context.site(), &child_site);
    assert_eq!(original_context.origin(), child_receipt.origin());
    assert!(original_context.source_origins().contains(
        &attune_common::secret_provenance::SecretOrigin::Key(
            attune_common::key_access::key_origin(&foreign),
        ),
    ));
    assert!(!original_context.source_origins().contains(delivered_origin));
    // First-typed capture must retain BOTH receipt identities at this same
    // execution/path, plus every transitive receipt, without reading Key bytes.
    use attune_common::repositories::runtime_read::{
        RuntimeReadOrigin, RuntimeReadParent, RuntimeReadTarget,
    };
    let mut captured_parents = std::collections::BTreeMap::new();
    for (kind, execution, provenance) in [
        ("execution_config", children[0].id, contexts[0].provenance()),
        ("workflow_variables", parent.id, variable.provenance()),
        ("execution_result", parent.id, result.provenance()),
    ] {
        captured_parents.insert(
            (kind.to_owned(), execution, "/token".to_owned()),
            provenance,
        );
    }
    for context in [
        frontier
            .prepared_receipt(&child_site, delivered_origin)
            .unwrap(),
        original_context,
        frontier
            .prepared_receipt(&variable_site, &variable.origin())
            .unwrap(),
        frontier
            .prepared_receipt(&result_site, &result.origin())
            .unwrap(),
    ] {
        for frozen in
            attune_common::repositories::immutable_input_origin::freeze_receipt_context(context)
                .unwrap()
        {
            let key = (frozen.entity_type, frozen.entity_id, frozen.path);
            if let Some(previous) = captured_parents.insert(key, frozen.provenance.clone()) {
                assert_eq!(previous, frozen.provenance);
            }
        }
    }
    let captured = RuntimeReadOrigin {
        target: RuntimeReadTarget::Execution {
            execution: children[0].id,
        },
        provenance: vec![attune_common::secret_provenance::SecretProvenance {
            origins: vec![attune_common::secret_provenance::SecretOrigin::Entity {
                entity_type: "execution_config".into(),
                entity_id: children[0].id,
                path: "/token".into(),
            }],
            templates: vec![],
        }],
        parents: captured_parents
            .into_iter()
            .map(
                |((entity_type, entity_id, path), provenance)| RuntimeReadParent {
                    entity_type,
                    entity_id,
                    path,
                    provenance,
                },
            )
            .collect(),
    };
    captured.validate().unwrap();
    let encoded_capture = serde_json::to_string(&captured).unwrap();
    assert!(!encoded_capture.contains(foreign.value.as_str().unwrap()));
    for origin in [
        delivered_origin.clone(),
        child_receipt.origin(),
        variable.origin(),
        result.origin(),
    ] {
        let attune_common::secret_provenance::SecretOrigin::Entity {
            entity_type,
            entity_id,
            path,
        } = origin
        else {
            unreachable!()
        };
        let mut missing = captured.clone();
        let index = missing
            .parents
            .iter()
            .position(|parent| {
                parent.entity_type == entity_type
                    && parent.entity_id == entity_id
                    && parent.path == path
            })
            .unwrap();
        missing.parents.remove(index);
        assert_denied(missing.validate());
    }
    assert!(original_context.source_origins().contains(
        &attune_common::secret_provenance::SecretOrigin::Entity {
            entity_type: "workflow_variables".into(),
            entity_id: parent.id,
            path: "/token".into(),
        },
    ));
    assert!(frontier
        .source_origins(&variable_site, &variable.origin())
        .unwrap()
        .contains(&attune_common::secret_provenance::SecretOrigin::Entity {
            entity_type: "execution_result".into(),
            entity_id: parent.id,
            path: "/token".into(),
        },));
    assert_eq!(secrets[0].encrypted_value, stored_child[0].encrypted_value);
    let mut wrong_original_site = child_receipt.origin();
    if let attune_common::secret_provenance::SecretOrigin::Entity { path, .. } =
        &mut wrong_original_site
    {
        *path = "/other".into();
    }
    assert_denied(leaves.source_receipt(&wrong_original_site));
    assert_denied(leaves.source_receipt(delivered_origin));
    let mut wrong_original_purpose = child_receipt.origin();
    if let attune_common::secret_provenance::SecretOrigin::Entity { entity_type, .. } =
        &mut wrong_original_purpose
    {
        *entity_type = "declared_input:execution_config:control".into();
    }
    assert_denied(leaves.source_receipt(&wrong_original_purpose));
    frontier.ensure_fresh_on(&mut tx).await.unwrap();
    frontier.ensure_fresh().unwrap();
    assert!(leaves.is_public_source("execution_config", parent.id, "/selector"));
    assert!(!leaves.is_public_source("execution_result", parent.id, "/selector"));
    assert!(!leaves.is_public_source("execution_config", children[0].id, "/selector"));
    assert!(!leaves.is_public_source("execution_config", children[0].id, "/token"));
    assert!(!leaves.is_public_source("execution_result", parent.id, "/token"));
    assert!(!leaves.is_outcome_metadata("execution_result", parent.id, ""));
    let mut copied = rows.clone();
    copied[0].entity_type = "artifact_file_body".into();
    assert_denied(
        InputReceiptFrontier::default()
            .prepare(&mut tx, &copied, Some(key))
            .await,
    );
    copied[0].entity_type = "execution_config".into();
    copied[0].entity_id = children[1].id;
    assert_denied(
        InputReceiptFrontier::default()
            .prepare(&mut tx, &copied, Some(key))
            .await,
    );
    assert!(frontier
        .source_origins(&child_site, delivered_origin)
        .unwrap()
        .contains(&attune_common::secret_provenance::SecretOrigin::Key(
            attune_common::key_access::key_origin(&foreign)
        )));
    KeyRepository::delete(&mut *tx, foreign.id).await.unwrap();
    assert!(
        prepare_input_receipts(&mut tx, "execution_config", children[0].id, &secrets, key)
            .await
            .is_ok(),
        "purpose proof never reloads a historical Key"
    );
    assert!(
        RuntimeTargetRepository::resolve(
            &mut tx,
            RuntimeTarget::Key(foreign.id),
            selected.owner.sponsor_identity_id
        )
        .await
        .unwrap()
        .is_none(),
        "reader disclosure still has no current Key target"
    );
    RuntimeAuthorityRepository::revoke_approval(&mut tx, approval)
        .await
        .unwrap();
    assert_denied(
        prepare_input_receipts(&mut tx, "execution_config", children[0].id, &secrets, key).await,
    );
    assert_denied(
        InputReceiptFrontier::default()
            .prepare(&mut tx, &rows, Some(key))
            .await,
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}
