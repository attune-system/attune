//! Real native cache receipts through the execution-detail HTTP consumer.
use super::*;
use attune_common::{
    config::CacheAdmissionConfig,
    repositories::{
        action::{ActionRepository, UpdateActionInput},
        cache::{
            CacheEntryInput, CacheEntryRepository, CacheGenerationRepository,
            CacheIngestRepository, CacheNamespacePolicy, CacheNamespaceRepository, CacheOwnerScope,
            CacheTransactionMode, CreateCacheGenerationInput, CreateCacheGenerationResult,
            CreateCacheNamespaceInput,
        },
        declared_input::DeclaredInputRepository,
        execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        native_input_snapshot::NativeInputSnapshotRepository as Snapshots,
        pack::PackRepository,
        runtime_target::{ComponentTarget, RuntimeTarget},
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            WorkflowDefinitionRepository, WorkflowExecutionRepository,
        },
        workflow_cache_iteration::WorkflowCacheIterationRepository,
        FindById,
    },
    secret_values::{redaction_marker, ENTITY_EXECUTION_RESULT},
};

pub(super) async fn generation(
    fixture: &Fixture,
    namespace: i64,
    expected: Option<i64>,
    label: &str,
    value: &str,
) -> Result<i64> {
    let created = CacheGenerationRepository::create_or_get(
        &fixture.ctx.pool,
        &CreateCacheGenerationInput {
            namespace,
            client_refresh_id: label.into(),
            expected_active_generation: expected,
            expected_chunk_count: 1,
            expected_count: Some(1),
            expected_bytes: None,
            checksum_algorithm: None,
            checksum: None,
            source_revision: None,
            created_by: None,
            created_by_execution: None,
        },
    )
    .await?;
    let CreateCacheGenerationResult::Created(created) = created else {
        panic!("test owns a new generation")
    };
    CacheIngestRepository::insert_chunk(
        &fixture.ctx.pool,
        created.id,
        0,
        label,
        &[CacheEntryInput {
            external_id: "original-entry".into(),
            value: json!({"token":value}),
            source_updated_at: None,
            source_checksum: None,
        }],
    )
    .await?;
    CacheGenerationRepository::seal(&fixture.ctx.pool, created.id).await?;
    Ok(created.id)
}

#[tokio::test]
async fn native_cache_http_default_false_preserves_original_page_after_pointer_rotation(
) -> Result<()> {
    cache_http_case(false, 3600, None).await
}

#[tokio::test]
async fn native_cache_http_require_fresh_without_deadline_rejects_original_retired_generation(
) -> Result<()> {
    cache_http_case(true, 0, None).await
}

#[tokio::test]
async fn native_cache_http_finite_original_deadline_expires_without_pointer_rotation() -> Result<()>
{
    cache_http_case(true, 30, None).await
}

pub(super) async fn cache_http_case(
    require_fresh: bool,
    freshness_seconds: i64,
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
        "recorded_emitter.c_cache_leaf",
    )
    .await?;
    let graph = create_test_action(
        &fixture.ctx.pool,
        pack.id,
        &pack.r#ref,
        "recorded_emitter.c_cache_graph",
    )
    .await?;
    ActionRepository::update(
        &fixture.ctx.pool,
        leaf.id,
        UpdateActionInput {
            param_schema: Some(json!({"token":{"type":"string","secret":true}})),
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
            param_schema: Some(json!({"cache_name":{"type":"string"}})),
            out_schema: Some(json!({"token":{"type":"string","secret":true}})),
            ..Default::default()
        },
    )
    .await?;
    let mut definition = json!({"tasks":[{"name":"cached","action":leaf.r#ref,
        "iterate_cache":{"namespace":"{{ parameters.cache_name }}","page_size":1,"require_fresh":require_fresh},
        "batch_size":1,"permission_set_refs":["standard"],"params":{"token":"{{ item.value.token }}"}}]});
    if !require_fresh {
        definition["tasks"][0]["iterate_cache"]
            .as_object_mut()
            .unwrap()
            .remove("require_fresh");
    }
    let graph_definition = WorkflowDefinitionRepository::create(
        &fixture.ctx.pool,
        CreateWorkflowDefinitionInput {
            r#ref: graph.r#ref.clone(),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "C cache consumer".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            tags: vec![],
            definition: definition.clone(),
        },
    )
    .await?;
    ActionRepository::link_workflow_def(&fixture.ctx.pool, graph.id, graph_definition.id).await?;
    activate_test_pack_release_with_projections(
        &fixture.ctx.pool,
        &pack,
        &PackProjectionIds {
            actions: vec![leaf.id, graph.id],
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
            config: Some(json!({"cache_name":"c-native-page"})),
            ..Default::default()
        },
        &graph_pin,
    )
    .await?;
    tx.commit().await?;
    let namespace = CacheNamespaceRepository::create_api_with_policy(
        &fixture.ctx.pool,
        CreateCacheNamespaceInput {
            mode: Default::default(),
            owner: CacheOwnerScope::pack(pack.id, Some(pack.r#ref.clone())),
            namespace: "c-native-page".into(),
            policy: CacheNamespacePolicy {
                freshness_target_seconds: freshness_seconds,
                ..Default::default()
            },
        },
        &CacheAdmissionConfig::default(),
    )
    .await?;
    let original = generation(
        &fixture,
        namespace.id,
        None,
        "c-cache-original",
        "native-cache-original",
    )
    .await?;
    CacheGenerationRepository::promote(
        &fixture.ctx.pool,
        namespace.id,
        original,
        None,
        chrono::Utc::now() + chrono::Duration::hours(1),
    )
    .await?;
    let replacement = generation(
        &fixture,
        namespace.id,
        Some(original),
        "c-cache-replacement",
        "wrong-current-page",
    )
    .await?;
    let refs = [
        "recorded_emitter.c_cache_full",
        "recorded_emitter.c_cache_no_read",
        "recorded_emitter.c_cache_no_decrypt",
    ];
    assert_eq!(namespace.owner_type, attune_common::models::OwnerType::Pack);
    assert_eq!(namespace.owner_pack, Some(pack.id));
    assert_eq!(
        namespace.owner_pack_ref.as_deref(),
        Some(pack.r#ref.as_str())
    );
    let mut sets = BTreeMap::new();
    for (index, reference) in refs.iter().enumerate() {
        let actions = match index {
            1 => json!(["decrypt"]),
            2 => json!(["read"]),
            _ => json!(["read", "decrypt"]),
        };
        let set = serde_json::from_value::<PermissionSetDefinition>(json!({"ref":reference,"grants":[
            {"resource":"executions","actions":["read","decrypt"],"constraints":{"pack_refs":[pack.r#ref]}},
            {"resource":"caches","actions":actions,"constraints":{"refs":[namespace.namespace],"owner_types":[namespace.owner_type],"owner_refs":[namespace.owner_pack_ref.as_deref().unwrap()]}}
        ]}))?.compile(&owner, &[])?;
        sets.insert(set.reference.clone(), set);
    }
    let root_binding = InvocationBinding::Action {
        action_ref: graph.r#ref.clone(),
    };
    let leaf_binding = InvocationBinding::Action {
        action_ref: leaf.r#ref.clone(),
    };
    let task_binding = InvocationBinding::WorkflowTask {
        workflow_ref: graph.r#ref.clone(),
        task_path: "/tasks/cached".into(),
        action_ref: leaf.r#ref.clone(),
    };
    let reader_refs = refs
        .iter()
        .map(|reference| reference.to_string())
        .collect::<Vec<_>>();
    let mut tx = fixture.ctx.pool.begin().await?;
    let concrete_targets = super::c_native_execution_wait::bind_execution_targets(
        &mut tx,
        owner.sponsor_identity_id,
        &sets,
        &refs
            .iter()
            .map(|reference| (*reference, std::slice::from_ref(&root.id)))
            .collect::<Vec<_>>(),
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
                    Some(&["standard".to_owned()]),
                    &sets,
                )?,
            ],
        },
    )
    .await?;
    for (grant, expected) in &concrete_targets {
        assert_eq!(policy.policy.concrete_targets.get(grant), Some(expected));
    }
    for grant in sets
        .values()
        .flat_map(|set| &set.grants)
        .filter(|grant| grant.grant().resource == attune_common::rbac::Resource::Caches)
    {
        let digest = super::c_native_execution_wait::grant_digest(grant)?;
        let targets = policy.policy.concrete_targets[&digest]
            .targets
            .as_ref()
            .unwrap();
        assert_eq!(
            policy.policy.concrete_targets[&digest].owners,
            Some(std::collections::BTreeSet::from([(
                "pack".to_owned(),
                pack.id,
                Some(pack.id)
            )]))
        );
        assert_eq!(targets.len(), 1);
        let target = targets.iter().next().unwrap();
        assert_eq!(
            (target.kind.as_str(), target.id, target.pack_id),
            ("cache_namespace", namespace.id, Some(pack.id))
        );
    }
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id).await?;
    let attribution = RequestAttribution::Human {
        actor: HumanAttribution {
            identity_id: fixture.requester,
            login: fixture.ctx.user.as_ref().unwrap().login.clone(),
        },
    };
    let selected = policy
        .policy
        .select(policy.id, &root_binding, &[], attribution.clone())?;
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(root.id),
        &selected,
    )
    .await?;
    let schema = SchemaContractRepository::materialize_action_pair(&mut tx, &graph_pin).await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, root.id, &schema).await?;
    let workflow = WorkflowExecutionRepository::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: root.id,
            workflow_def: graph_definition.id,
            task_graph: definition,
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await?;
    tx.commit().await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation).await?;
    let task = Admissions::select(
        &mut tx,
        pack.id,
        task_binding,
        &leaf_pin,
        &["standard".into()],
        RequestAttribution::ParentExecution {
            execution_id: root.id,
        },
        RuntimeTarget::Component(ComponentTarget::Action, graph.id),
    )
    .await?;
    let declarations = DeclaredInputRepository::bindings(&mut tx, root.id).await?;
    let selector = declarations
        .iter()
        .find(|declaration| {
            declaration
                .source_path
                .ends_with("/iterate_cache/namespace")
        })
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
    Inputs::bind_public_parameter(&mut tx, &mut session, "/cache_name").await?;
    let rendered = Inputs::evaluate(&mut tx, session).await?;
    let control = Inputs::record_render(
        &mut tx,
        &rendered,
        &InputSite {
            execution: root.id,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Control,
            path: selector.source_path.clone(),
        },
        rendered.value(),
        rendered.selections(),
        key,
    )
    .await?;
    let prepared = Snapshots::prepare_cache_initial(
        &mut tx,
        root.id,
        "/workflow/tasks/0",
        &task,
        workflow.id,
        65536,
        &[&control],
        key,
    )
    .await?;
    assert!(prepared.value().is_some_and(|value|value.is_object() && value.pointer("/value/token")==Some(&json!("native-cache-original"))),
        "batch_size=1 binds the actual cache item object, including its value field, not an array of items");
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
    Inputs::bind_cache_input(&mut session, &prepared)?;
    let rendered = Inputs::evaluate(&mut tx, session).await?;
    assert_eq!(rendered.value(), &json!("native-cache-original"));
    let site = InputSite {
        execution: root.id,
        kind: InputEntityKind::ExecutionResult,
        purpose: InputSitePurpose::Value,
        path: "/token".into(),
    };
    let destination = Inputs::prepare_destination(&mut tx, &rendered, &site, key).await?;
    let iteration = WorkflowCacheIterationRepository::create(
        &mut *tx,
        prepared.initial_create_input().unwrap(),
    )
    .await?;
    Snapshots::bind_cache_frontier(&mut tx, &prepared, iteration.id).await?;
    let receipt = Inputs::record_prepared(
        &mut tx,
        &rendered,
        &destination,
        rendered.value(),
        rendered.selections(),
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
            value: rendered.value().clone(),
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
        let selected =
            policy
                .policy
                .select(policy.id, &leaf_binding, &refs, attribution.clone())?;
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
    tx.commit().await?;
    let path = format!("/api/v1/executions/{}?include_secret_values=true", root.id);
    let positive = fixture.ctx.get(&path, Some(&tokens[0])).await?;
    let status = positive.status();
    let body: Value = positive.json().await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "actual cache reader must disclose the sealed original page"
    );
    assert_eq!(body["data"]["result"]["token"], "native-cache-original");
    assert_eq!(
        fixture
            .ctx
            .get(
                &format!("/api/v1/executions/{}", fixture.credential.execution_id),
                Some(&tokens[0])
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN,
        "authored pack scope must be narrowed to the exact original execution at admission"
    );
    for token in &tokens[1..] {
        assert_eq!(
            fixture.ctx.get(&path, Some(token)).await?.status(),
            StatusCode::FORBIDDEN
        );
    }
    let login = format!("c_cache_reader_{}", uuid::Uuid::new_v4().simple());
    let registered = fixture
        .ctx
        .post(
            "/auth/register",
            json!({"login":login,"password":"TestPassword123!","display_name":"C cache reader"}),
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
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let human_set = PermissionSetRepository::create(&fixture.ctx.pool,CreatePermissionSetInput {
        r#ref:"fixture.c_cache_human".into(),pack:None,pack_ref:None,label:None,description:None,
        grants:json!([
            {"resource":"executions","actions":["read","decrypt"],"constraints":{"ids":[root.id]}},
            {"resource":"caches","actions":["read","decrypt"],"constraints":{"ids":[namespace.id]}}
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
    let human_positive = fixture.ctx.get(&path, Some(&human_token)).await?;
    let status = human_positive.status();
    let body: Value = human_positive.json().await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["result"]["token"], "native-cache-original");
    if let Some(at) = transport {
        let deadline = if require_fresh && freshness_seconds > 0 {
            let generation = CacheGenerationRepository::find_by_id(&fixture.ctx.pool, original)
                .await?
                .unwrap();
            Some(generation.activated.unwrap() + chrono::Duration::seconds(freshness_seconds))
        } else {
            None
        };
        super::c_native_transport::exercise(
            &mut fixture,
            root.id,
            reader_executions[0],
            policy.id,
            deadline,
            at,
        )
        .await
        .map_err(|error| format!("cache transport {at:?}: {error}"))?;
        fixture.ctx.cleanup().await?;
        return Ok(());
    }
    if require_fresh && freshness_seconds == 30 {
        // Prepare through the same frontier as production, then retain its
        // opaque policy after source commit through a real deadline wait. This
        // complements HTTP reads on both sides; it is not a transport-hook test.
        use attune_common::repositories::immutable_input_origin::InputReceiptFrontier;
        let mut held = fixture.ctx.pool.begin().await?;
        let rows = ExecutionSecretValueRepository::find_bounded_provenance(
            &mut *held,
            &[ENTITY_EXECUTION_RESULT],
            &[root.id],
        )
        .await?;
        let mut frontier = InputReceiptFrontier::default();
        frontier.prepare(&mut held, &rows, Some(key)).await?;
        let leaves = frontier.disclosure_leaves(&site, &receipt.origin())?;
        let root_origin = frontier.source_origins(&site, &receipt.origin())?.iter()
            .find(|origin| matches!(origin, attune_common::secret_provenance::SecretOrigin::Entity { entity_type, .. } if entity_type == "cache_namespace")).unwrap().clone();
        let source = frontier
            .prepared_cache_source(&leaves, &root_origin)?
            .unwrap();
        let original_generation = CacheGenerationRepository::find_by_id(&mut *held, original)
            .await?
            .unwrap();
        let deadline =
            original_generation.activated.unwrap() + chrono::Duration::seconds(freshness_seconds);
        frontier.ensure_fresh_on(&mut held).await?;
        held.commit().await?;
        let remaining = (deadline - chrono::Utc::now()).to_std().unwrap_or_default();
        tokio::time::sleep_until(tokio::time::Instant::now() + remaining).await;
        assert!(
            source.ensure_fresh().is_err(),
            "opaque policy must retain the original absolute deadline after commit"
        );
        assert!(frontier.ensure_fresh().is_err());
        assert!(!fixture
            .ctx
            .get(&path, Some(&tokens[0]))
            .await?
            .status()
            .is_success());
        assert!(!fixture
            .ctx
            .get(&path, Some(&human_token))
            .await?
            .status()
            .is_success());
        fixture.ctx.cleanup().await?;
        return Ok(());
    }
    CacheGenerationRepository::promote(
        &fixture.ctx.pool,
        namespace.id,
        replacement,
        Some(original),
        chrono::Utc::now() + chrono::Duration::hours(1),
    )
    .await?;
    let rotated = fixture.ctx.get(&path, Some(&tokens[0])).await?;
    if require_fresh {
        assert!(!rotated.status().is_success(), "require_fresh=true with no deadline still requires the exact original active generation");
    } else {
        let status = rotated.status();
        let body: Value = rotated.json().await?;
        assert_eq!(status, StatusCode::OK, "default-false native reads must not rebind to the new pointer or deny a retained retired page");
        assert_eq!(body["data"]["result"]["token"], "native-cache-original");
    }
    let human_rotated = fixture.ctx.get(&path, Some(&human_token)).await?;
    if require_fresh {
        assert!(!human_rotated.status().is_success());
    } else {
        let status = human_rotated.status();
        let body: Value = human_rotated.json().await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["result"]["token"], "native-cache-original");
    }
    fixture.ctx.cleanup().await?;
    Ok(())
}
