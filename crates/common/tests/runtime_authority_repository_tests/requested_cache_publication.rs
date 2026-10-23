//! Indexed children are born through B's native cache source factory, not by
//! forging a child row or attaching provenance after source collection.
use super::*;
use attune_common::{
    config::CacheAdmissionConfig,
    models::{Execution, WorkflowExecution},
    repositories::{
        accepted_input::AcceptedInputRepository as Accepted,
        action::{ActionRepository, UpdateActionInput},
        cache::{
            CacheEntryInput, CacheEntryRepository, CacheGenerationRepository,
            CacheIngestRepository, CacheNamespacePolicy, CacheNamespaceRepository, CacheOwnerScope,
            CacheTransactionMode, CreateCacheGenerationInput, CreateCacheGenerationResult,
            CreateCacheNamespaceInput,
        },
        declared_input_materialization::DeclaredInputMaterializationRepository as Inputs,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        managed_executable::ManagedExecutableRepository as Managed,
        native_cache_materialization::NativeCacheMaterializationRepository as Cache,
        recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
        requested_execution_publication::RequestedExecutionPublicationRepository as Requested,
        schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            WorkflowDefinitionRepository, WorkflowExecutionRepository as Workflows,
        },
        workflow_cache_iteration::WorkflowCacheIterationRepository as Iterations,
        FindById,
    },
    secret_values::{
        prepare_secret_values, redaction_marker, SecretSource, SecretValueInput,
        ENTITY_EXECUTION_CONFIG,
    },
};

const KEY: &str = "requested-cache-publication-owned-key";
struct Case {
    db: TestDatabase,
    child: Execution,
    workflow: WorkflowExecution,
    producer: Id,
    iteration: Id,
    human: Id,
}

async fn cache_child() -> Case {
    cache_child_with_input(true).await
}

async fn cache_child_with_input(uses_item: bool) -> Case {
    let (db, base) = fixture().await;
    let owner = &base.policy.policy.owner;
    let root_action = ActionFixture::new(owner.pack_id, &owner.pack_ref, "requested_cache_root")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(owner.pack_id, &owner.pack_ref, "requested_cache_leaf")
        .create(&db)
        .await
        .unwrap();
    ActionRepository::update(
        &db,
        root_action.id,
        UpdateActionInput {
            param_schema: Some(if uses_item { json!({"cache_name":{"type":"string"}}) } else {
                json!({"cache_name":{"type":"string"},"constant_token":{"type":"string","secret":true}})
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ActionRepository::update(
        &db,
        leaf.id,
        UpdateActionInput {
            param_schema: Some(json!({"token":{"type":"string","secret":true}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let graph = json!({"version":"1.0","tasks":[{"name":"cached","action":leaf.r#ref,
        "iterate_cache":{"namespace":"{{ parameters.cache_name }}","page_size":1,"require_fresh":false},
        // batch_size=1 binds item to ONE projected cache entry, not an array.
        "batch_size":1,"concurrency":1,"permission_set_refs":["standard"],"params":{"token": if uses_item { "{{ item.value }}" } else { "{{ parameters.constant_token }}" }}}]});
    let definition = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: root_action.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "Requested cache recovery".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            definition: graph.clone(),
            tags: vec![],
        },
    )
    .await
    .unwrap();
    ActionRepository::link_workflow_def(&db, root_action.id, definition.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = activate_release(
        &mut tx,
        owner.pack_id,
        &owner.pack_ref,
        vec![root_action.id, leaf.id],
        'c',
    )
    .await;
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let root_binding = InvocationBinding::Action {
        action_ref: root_action.r#ref.clone(),
    };
    let task_binding = InvocationBinding::WorkflowTask {
        workflow_ref: root_action.r#ref.clone(),
        task_path: "/tasks/0".into(),
        action_ref: leaf.r#ref.clone(),
    };
    let policy = RuntimePolicy {
        owner: owner.clone(),
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: BTreeMap::new(),
        invocations: vec![
            InvocationPolicy::from_executable(
                root_binding.clone(),
                &snapshots[&root_action.r#ref],
                Some(&[]),
                &BTreeMap::new(),
            )
            .unwrap(),
            InvocationPolicy::from_executable(
                task_binding,
                &snapshots[&leaf.r#ref],
                Some(&["standard".into()]),
                &BTreeMap::new(),
            )
            .unwrap(),
        ],
    };
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let snapshot = Managed::resolve_invocation(&mut tx, &policy.policy, &root_binding)
        .await
        .unwrap();
    let authority = policy
        .policy
        .select(
            policy.id,
            &root_binding,
            &[],
            base.admission.authority.attribution.clone(),
        )
        .unwrap();
    let root = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(root_action.id),
            action_ref: root_action.r#ref,
            executor: Some(base.human_id),
            status: ExecutionStatus::Requested,
            config: None,
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(root.id),
        &authority,
    )
    .await
    .unwrap();
    let schema = Schemas::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, root.id, &schema)
        .await
        .unwrap();
    // The constant batch input is supplied and accepted at the root's birth.
    // It is not a cache ITEM read or a post-Running contributor recapture.
    let mut original_input = json!({"cache_name":"requested-cache-owned"});
    let original_fields = if uses_item {
        vec![]
    } else {
        original_input["constant_token"] = redaction_marker();
        prepare_secret_values(
            vec![SecretValueInput {
                json_path: "/constant_token".into(),
                value: json!("constant original batch input"),
                source_kind: "provenance".into(),
                source_ref: Some(
                    serde_json::to_string(
                        &SecretSource::ParameterSchema {
                            path: "/constant_token".into(),
                        }
                        .provenance(),
                    )
                    .unwrap(),
                ),
            }],
            KEY,
        )
        .unwrap()
    };
    ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        root.id,
        Some(admission.id),
        &snapshot,
        Some(original_input.clone()),
        &original_fields,
    )
    .await
    .unwrap();
    Contributors::capture_human(&mut tx, "execution", root.id, base.human_id)
        .await
        .unwrap();
    Inputs::seal_execution_inputs(&mut tx, root.id, KEY)
        .await
        .unwrap();
    Accepted::revalidate_execution(&mut tx, root.id, KEY)
        .await
        .unwrap();
    let accepted: (serde_json::Value, i64, bool) = sqlx::query_as("SELECT accepted.body->'config',jsonb_array_length(accepted.body->'contributors')::BIGINT,epoch.transaction_id=pg_current_xact_id()::TEXT FROM runtime_accepted_input accepted JOIN runtime_accepted_input_epoch epoch ON epoch.admission=accepted.admission WHERE accepted.execution=$1")
        .bind(root.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(accepted, (original_input, 1, true),
        "the original root input and real supplier must be sealed in its creation transaction before Running");
    let declarations =
        attune_common::repositories::declared_input::DeclaredInputRepository::bindings(
            &mut tx, root.id,
        )
        .await
        .unwrap();
    assert!(declarations.iter().any(|binding| binding.source_path
        == "/workflow/tasks/0/iterate_cache/namespace"
        && binding.expression == "{{ parameters.cache_name }}"));
    let expected_value = if uses_item {
        "{{ item.value }}"
    } else {
        "{{ parameters.constant_token }}"
    };
    assert!(
        declarations.iter().any(
            |binding| binding.source_path == "/workflow/tasks/0/params/token"
                && binding.expression == expected_value
        ),
        "the native factory must use the original declared scalar-item or accepted root VALUE"
    );
    let root = ExecutionRepository::update(
        &mut *tx,
        root.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let workflow = Workflows::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: root.id,
            workflow_def: definition.id,
            task_graph: graph,
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let namespace = CacheNamespaceRepository::create_api_with_policy(
        &db,
        CreateCacheNamespaceInput {
            mode: Default::default(),
            owner: CacheOwnerScope::pack(owner.pack_id, Some(owner.pack_ref.clone())),
            namespace: "requested-cache-owned".into(),
            policy: CacheNamespacePolicy::default(),
        },
        &CacheAdmissionConfig::default(),
    )
    .await
    .unwrap();
    let CreateCacheGenerationResult::Created(generation) =
        CacheGenerationRepository::create_or_get(
            &db,
            &CreateCacheGenerationInput {
                namespace: namespace.id,
                client_refresh_id: "requested-cache-source".into(),
                expected_active_generation: None,
                expected_chunk_count: 1,
                expected_count: Some(1),
                expected_bytes: None,
                checksum_algorithm: None,
                checksum: None,
                source_revision: None,
                created_by: None,
                created_by_execution: Some(base.credential.execution_id),
            },
        )
        .await
        .unwrap()
    else {
        panic!("run-owned generation must be new");
    };
    CacheIngestRepository::insert_chunk(
        &db,
        generation.id,
        0,
        "requested-cache-chunk",
        &[CacheEntryInput {
            external_id: "a".into(),
            value: json!("{{ literal_original_not_a_new_key_lookup }}"),
            source_updated_at: None,
            source_checksum: None,
        }],
    )
    .await
    .unwrap();
    CacheGenerationRepository::seal(&db, generation.id)
        .await
        .unwrap();
    CacheGenerationRepository::promote(
        &db,
        namespace.id,
        generation.id,
        None,
        Utc::now() + Duration::hours(1),
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut plan = Cache::prepare(&mut tx, &root, "cached", KEY).await.unwrap();
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    let result = plan.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(result.children.len(), 1);
    let child = result.children.into_iter().next().unwrap();
    // Both acceptance and dispatch must exist before the source producer can GC.
    Accepted::revalidate_execution(&mut tx, child.id, KEY)
        .await
        .unwrap();
    let original_epoch: bool = sqlx::query_scalar("SELECT epoch.transaction_id=pg_current_xact_id()::TEXT FROM runtime_accepted_input accepted JOIN runtime_accepted_input_epoch epoch ON epoch.admission=accepted.admission WHERE accepted.execution=$1")
        .bind(child.id).fetch_one(&mut *tx).await.unwrap();
    assert!(
        original_epoch,
        "the native child must receive its first seal in its original creation transaction"
    );
    let iteration = Iterations::find_by_workflow_task(&mut *tx, workflow.id, "cached")
        .await
        .unwrap()
        .unwrap();
    let dispatch: Option<Id> = sqlx::query_scalar("SELECT execution_id FROM workflow_task_dispatch WHERE workflow_execution=$1 AND task_name='cached' AND task_index=0")
        .bind(workflow.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(dispatch, Some(child.id));
    let frontiers: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_native_cache_frontier WHERE bound_iteration=$1 AND ordinal=0 AND bound_transaction=prepared_transaction")
        .bind(iteration.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(frontiers, 1);
    assert_eq!(child.workflow_task.as_ref().unwrap().task_index, Some(0));
    assert_eq!(child.workflow_task.as_ref().unwrap().task_batch, Some(1));
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    Case {
        db,
        child,
        workflow,
        producer: base.credential.execution_id,
        iteration: iteration.id,
        human: base.human_id,
    }
}

#[tokio::test]
async fn requested_indexed_cache_child_retains_original_input_and_exact_pins_after_producer_gc() {
    let case = cache_child().await;
    let before = Secrets::find_stored_by_entity(&case.db, ENTITY_EXECUTION_CONFIG, case.child.id)
        .await
        .unwrap();
    assert!(
        !before.is_empty(),
        "the native factory must first-seal a real cache-derived field"
    );
    let token = before
        .iter()
        .find(|field| field.json_path == "/token")
        .unwrap();
    assert_eq!(
        attune_common::crypto::decrypt_json(&token.encrypted_value, KEY).unwrap(),
        json!("{{ literal_original_not_a_new_key_lookup }}")
    );
    assert!(ExecutionRepository::delete(&case.db, case.producer)
        .await
        .unwrap());
    IdentityRepository::update(
        &case.db,
        case.human,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = case.db.begin().await.unwrap();
    let original = RuntimeAuthorityRepository::execution_admission(&mut tx, case.child.id)
        .await
        .unwrap()
        .unwrap();
    let prepared = Requested::prepare(&mut tx, &case.child, KEY)
        .await
        .unwrap()
        .unwrap();
    prepared.lock_target(&mut tx).await.unwrap();
    prepared
        .publish_bounded(&mut tx, async { Ok(()) })
        .await
        .unwrap();
    let after = RuntimeAuthorityRepository::execution_admission(&mut tx, case.child.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original.id, after.id);
    assert_eq!(original.authority.attempt, after.authority.attempt);
    assert_eq!(
        Secrets::find_stored_by_entity(&mut *tx, ENTITY_EXECUTION_CONFIG, case.child.id)
            .await
            .unwrap(),
        before
    );
    tx.commit().await.unwrap();
    case.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn requested_cache_child_cannot_fall_back_to_ordinary_task_after_graph_first_cleanup() {
    let case = cache_child().await;
    // Prove the real setup reaches guarded publication before any cleanup.
    let mut tx = case.db.begin().await.unwrap();
    let prepared = Requested::prepare(&mut tx, &case.child, KEY)
        .await
        .unwrap()
        .unwrap();
    prepared.lock_target(&mut tx).await.unwrap();
    prepared
        .publish_bounded(&mut tx, async { Ok(()) })
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    // Only the real graph-first repository cleanup may remove cursor/dispatch
    // rows. No direct proof DELETE, FK disabling or accepted-body mutation.
    assert!(Workflows::delete(&case.db, case.workflow.id).await.unwrap());
    let mut tx = case.db.begin().await.unwrap();
    assert!(Iterations::find_by_id(&mut *tx, case.iteration)
        .await
        .unwrap()
        .is_none());
    let dispatch: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_task_dispatch WHERE workflow_execution=$1 AND task_name='cached' AND task_index=0)")
        .bind(case.workflow.id).fetch_one(&mut *tx).await.unwrap();
    assert!(!dispatch);
    let archived_frontiers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_native_cache_frontier WHERE bound_iteration=$1 AND ordinal=0",
    )
    .bind(case.iteration)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        archived_frontiers, 1,
        "cleanup must not erase the child's retained frontier evidence"
    );
    // Accepted child and original archived cache receipt remain valid. They
    // cannot recreate the reclaimed cursor or dispatch slot.
    Accepted::revalidate_execution(&mut tx, case.child.id, KEY)
        .await
        .unwrap();
    assert!(Requested::prepare(&mut tx, &case.child, KEY).await.is_err());
    tx.rollback().await.unwrap();
    let actual = ExecutionRepository::find_by_id(&case.db, case.child.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.status, ExecutionStatus::Requested);
    assert_eq!(actual.workflow_task, case.child.workflow_task);
    case.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn requested_cache_child_needs_its_exact_index_dispatch_and_archived_batch() {
    for wrong_batch in [false, true] {
        let case = cache_child().await;
        let mut tx = case.db.begin().await.unwrap();
        let prepared = Requested::prepare(&mut tx, &case.child, KEY)
            .await
            .unwrap()
            .unwrap();
        prepared.lock_target(&mut tx).await.unwrap();
        prepared
            .publish_bounded(&mut tx, async { Ok(()) })
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        let mut metadata = case.child.workflow_task.clone().unwrap();
        if wrong_batch {
            metadata.task_batch = Some(2);
        } else {
            metadata.task_index = Some(1);
        }
        // Ordinary mutable scheduling metadata cannot select another dispatch
        // slot or widen the actual original archived cache page. No proof row
        // is deleted and no accepted/admitted record is changed.
        let changed = ExecutionRepository::update(
            &case.db,
            case.child.id,
            UpdateExecutionInput {
                workflow_task: Some(metadata),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let mut tx = case.db.begin().await.unwrap();
        Accepted::revalidate_execution(&mut tx, changed.id, KEY)
            .await
            .unwrap();
        assert!(Requested::prepare(&mut tx, &changed, KEY).await.is_err());
        tx.rollback().await.unwrap();
        case.db.cleanup().await.unwrap();
    }
}

#[tokio::test]
async fn requested_indexed_cache_child_with_constant_input_still_uses_the_real_frontier() {
    let case = cache_child_with_input(false).await;
    let mut tx = case.db.begin().await.unwrap();
    let receipts = Inputs::prepare_execution_input_receipts(&mut tx, case.child.id, KEY)
        .await
        .unwrap();
    assert!(
        receipts.iter().all(|receipt| receipt
            .native_source_metadata()
            .iter()
            .all(|source| source.source_kind() != "cache")),
        "constant-input coverage must not quietly gain a cache ITEM VALUE dependency"
    );
    let fields = Secrets::find_stored_by_entity(&mut *tx, ENTITY_EXECUTION_CONFIG, case.child.id)
        .await
        .unwrap();
    let token = fields
        .iter()
        .find(|field| field.json_path == "/token")
        .unwrap();
    assert_eq!(
        attune_common::crypto::decrypt_json(&token.encrypted_value, KEY).unwrap(),
        json!("constant original batch input")
    );
    let prepared = Requested::prepare(&mut tx, &case.child, KEY)
        .await
        .unwrap()
        .unwrap();
    prepared.lock_target(&mut tx).await.unwrap();
    prepared
        .publish_bounded(&mut tx, async { Ok(()) })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    case.db.cleanup().await.unwrap();
}
