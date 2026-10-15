mod helpers;

use attune_common::{
    auth::jwt::generate_admitted_execution_token,
    models::{ExecutionStatus, Id},
    repositories::{
        component_lifecycle::PackProjectionIds,
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        execution_event_emitter::ExecutionEventEmitterRepository,
        execution_secret_value::ExecutionSecretValueRepository,
        identity::{IdentityRepository, UpdateIdentityInput},
        rule_authority::RuleAuthorityRepository,
        runtime_authority::{RuntimeAuthorityRepository, RuntimeCredentialBinding, RuntimeOwner},
        schema_contract::{BindingOwner, SchemaContractRepository},
        trigger::{CreateTriggerInput, TriggerRepository},
        Create, Delete, Update,
    },
    runtime_authority::{
        HumanAttribution, InvocationBinding, InvocationPolicy, PermissionSetDefinition,
        RequestAttribution, RuntimePolicy,
    },
    secret_values::{
        prepare_secret_values, SecretValueInput, ENTITY_EVENT_PAYLOAD, ENTITY_EXECUTION_CONFIG,
    },
};
use helpers::{
    activate_test_pack_release_with_projections, create_test_action, create_test_pack, TestContext,
};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Fixture {
    ctx: TestContext,
    credential: RuntimeCredentialBinding,
    token: String,
    trigger_ref: String,
    foreign_trigger_ref: String,
    requester: Id,
    policy_id: Id,
    pack_id: Id,
    signing_approval_id: Id,
}

async fn fixture(standard_only: bool) -> Result<Fixture> {
    fixture_with_workflow(standard_only, false).await
}

async fn fixture_with_workflow(standard_only: bool, workflow_context: bool) -> Result<Fixture> {
    fixture_with_key_actions(
        standard_only,
        workflow_context,
        &["read", "decrypt", "update", "delete"],
    )
    .await
}

async fn fixture_with_key_actions(
    standard_only: bool,
    workflow_context: bool,
    key_actions: &[&str],
) -> Result<Fixture> {
    fixture_with_capabilities(standard_only, workflow_context, key_actions, &[]).await
}

async fn fixture_with_capabilities(
    standard_only: bool,
    workflow_context: bool,
    key_actions: &[&str],
    cache_actions: &[&str],
) -> Result<Fixture> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let requester = ctx.user.as_ref().unwrap().id;
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let issuer_set = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "fixture.recorded_signer_issuer".into(), pack: None, pack_ref: None,
        label: None, description: None,
        grants: json!([{"resource":"keys","actions":["use"],"constraints":{"refs":["system.recorded_signer"]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: requester,
            permset: issuer_set.id,
        },
    )
    .await?;
    let pack = create_test_pack(&ctx.pool, "recorded_emitter").await?;
    let other = create_test_pack(&ctx.pool, "foreign_events").await?;
    let action =
        create_test_action(&ctx.pool, pack.id, &pack.r#ref, "recorded_emitter.emit").await?;
    let workflow = if workflow_context {
        let workflow =
            create_test_action(&ctx.pool, pack.id, &pack.r#ref, "recorded_emitter.workflow")
                .await?;
        use attune_common::repositories::workflow::{
            CreateWorkflowDefinitionInput, WorkflowDefinitionRepository,
        };
        let definition = WorkflowDefinitionRepository::create(&ctx.pool, CreateWorkflowDefinitionInput {
            r#ref: workflow.r#ref.clone(), pack: pack.id, pack_ref: pack.r#ref.clone(),
            label: "Workflow".into(), description: None, version: "1.0.0".into(),
            param_schema: None, out_schema: None,
            definition: json!({"tasks":[{"name":"emit", "action":action.r#ref, "permission_set_refs":["standard"]}]}),
            tags: Vec::new(),
        }).await?;
        attune_common::repositories::action::ActionRepository::link_workflow_def(
            &ctx.pool,
            workflow.id,
            definition.id,
        )
        .await?;
        Some(workflow)
    } else {
        None
    };
    let mut trigger_refs = Vec::new();
    for (pack, reference) in [
        (&pack, "recorded_emitter.completed"),
        (&other, "foreign_events.completed"),
    ] {
        let trigger = TriggerRepository::create(
            &ctx.pool,
            CreateTriggerInput {
                r#ref: reference.into(),
                pack: Some(pack.id),
                pack_ref: Some(pack.r#ref.clone()),
                label: "Completed".into(),
                description: None,
                enabled: true,
                param_schema: Some(json!({})),
                event_config_schema: None,
                out_schema: Some(json!({"secret": {"type": "string", "secret": true}})),
                sensor: None,
                sensor_ref: None,
                is_adhoc: false,
                reference_visibility: Default::default(),
                reference_allowed_pack_refs: Vec::new(),
            },
        )
        .await?;
        trigger_refs.push(trigger.r#ref);
    }
    activate_test_pack_release_with_projections(
        &ctx.pool,
        &pack,
        &PackProjectionIds {
            actions: std::iter::once(action.id)
                .chain(workflow.as_ref().map(|workflow| workflow.id))
                .collect(),
            ..Default::default()
        },
    )
    .await?;
    let mut tx = ctx.pool.begin().await?;
    RuleAuthorityRepository::capture_deployment(&mut tx, pack.id).await?;
    let owner = RuntimeAuthorityRepository::find_owner(&mut tx, pack.id)
        .await?
        .unwrap();
    let snapshot =
        ExecutableSnapshotRepository::resolve_for_action_ref(&mut *tx, &action.r#ref).await?;
    let signing_grant = serde_json::from_value(json!({
        "resource": "keys", "actions": ["use"], "constraints": {"refs": ["system.recorded_signer"]}
    }))?;
    let signing_approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        pack.id,
        requester,
        vec![signing_grant],
        None,
    )
    .await?;
    let mut definition = json!({
        "ref": "recorded_emitter.events", "grants": [
            {"resource": "events", "actions": ["create"]},
            {"resource": "keys", "actions": key_actions},
            {"resource": "keys", "actions": ["use"], "scope": "external",
                "constraints": {"refs": ["system.recorded_signer"]}}
        ]
    });
    if !cache_actions.is_empty() {
        definition["grants"]
            .as_array_mut()
            .unwrap()
            .push(json!({"resource":"caches","actions":cache_actions}));
    }
    let definition: PermissionSetDefinition = serde_json::from_value(definition)?;
    let set = definition.compile(&owner, &[signing_approval.approval()?])?;
    let binding = match workflow {
        Some(workflow) => InvocationBinding::WorkflowTask {
            workflow_ref: workflow.r#ref,
            task_path: "/tasks/0".into(),
            action_ref: action.r#ref.clone(),
        },
        None => InvocationBinding::Action {
            action_ref: action.r#ref.clone(),
        },
    };
    let policy = RuntimePolicy {
        owner,
        release_id: snapshot.release.id,
        release_digest: snapshot.release.digest.clone(),
        invocations: vec![InvocationPolicy {
            binding: binding.clone(),
            target_release_id: snapshot.release.id,
            target_release_digest: snapshot.release.digest.clone(),
            permission_sets: BTreeMap::from([(set.reference.clone(), set)]),
            standard_access: true,
        }],
    };
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy).await?;
    let refs = vec![if standard_only {
        "standard".into()
    } else {
        "recorded_emitter.events".into()
    }];
    let selected = policy.policy.select(
        policy.id,
        &binding,
        &refs,
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: requester,
                login: ctx.user.as_ref().unwrap().login.clone(),
            },
        },
    )?;
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            executor: Some(requester),
            permission_set_refs: refs.clone(),
            status: ExecutionStatus::Running,
            config: Some(json!({})),
            ..Default::default()
        },
        &snapshot,
    )
    .await?;
    let schema = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &schema)
        .await?;
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(execution.id),
        &selected,
    )
    .await?;
    let credential = RuntimeCredentialBinding {
        execution_id: execution.id,
        admission_id: admission.id,
        sponsor_identity_id: selected.owner.sponsor_identity_id,
        action_ref: action.r#ref,
        attempt: selected.attempt,
    };
    let key = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let prepared = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/input".into(),
            value: json!("private input"),
            source_kind: "param_schema".into(),
            source_ref: None,
        }],
        key,
    )?;
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        ENTITY_EXECUTION_CONFIG,
        execution.id,
        &prepared,
    )
    .await?;
    tx.commit().await?;
    let token =
        generate_admitted_execution_token(&credential, &refs, &ctx.state.jwt_config, Some(360))?;
    Ok(Fixture {
        ctx,
        credential,
        token,
        trigger_ref: trigger_refs[0].clone(),
        foreign_trigger_ref: trigger_refs[1].clone(),
        requester,
        policy_id: policy.id,
        pack_id: pack.id,
        signing_approval_id: signing_approval.id,
    })
}

#[tokio::test]
async fn recorded_cache_namespace_lifecycle_is_local_and_keeps_managed_metadata_gated() -> Result<()>
{
    use attune_common::{
        config::CacheAdmissionConfig,
        repositories::cache::{
            CacheNamespacePolicy, CacheNamespaceRepository, CacheOwnerScope,
            ManagedCacheNamespaceDefinition,
        },
    };
    let fixture = fixture_with_capabilities(
        false,
        false,
        &["read"],
        &["read", "create", "update", "delete"],
    )
    .await?;
    CacheNamespaceRepository::upsert_managed_definitions(
        &fixture.ctx.pool,
        fixture.pack_id,
        "recorded_emitter",
        &[ManagedCacheNamespaceDefinition {
            mode: Default::default(),
            definition_ref: "recorded_emitter.defined".into(),
            owner: CacheOwnerScope::pack(fixture.pack_id, Some("recorded_emitter".into())),
            namespace: "defined".into(),
            policy: CacheNamespacePolicy::default(),
        }],
        &CacheAdmissionConfig::default(),
    )
    .await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let request =
        json!({"owner_type":"pack", "owner_ref":"recorded_emitter", "namespace":"created"});
    let response = fixture
        .ctx
        .post(
            "/api/v1/cache/namespaces",
            request.clone(),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        fixture
            .ctx
            .post("/api/v1/cache/namespaces", request, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    for owner in ["foreign_events", "missing_owner"] {
        assert_eq!(
            fixture
                .ctx
                .post(
                    "/api/v1/cache/namespaces",
                    json!({"owner_type":"pack", "owner_ref":owner,
            "namespace":"created"}),
                    Some(&fixture.token)
                )
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let response = fixture
        .ctx
        .put(
            "/api/v1/cache/namespaces/created",
            json!({"owner_type":"pack", "owner_ref":"recorded_emitter",
        "freshness_target_seconds":30}),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["freshness_target_seconds"], 30);
    let path = "/api/v1/cache/namespaces/created?owner_type=pack&owner_ref=recorded_emitter";
    assert_eq!(
        fixture
            .ctx
            .delete(path, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::OK
    );
    let updated = fixture
        .ctx
        .put(
            "/api/v1/cache/namespaces/defined",
            json!({"owner_type":"pack",
        "owner_ref":"recorded_emitter", "freshness_target_seconds":30}),
            Some(&fixture.token),
        )
        .await?;
    let deleted = fixture
        .ctx
        .delete(
            "/api/v1/cache/namespaces/defined?owner_type=pack&owner_ref=recorded_emitter",
            Some(&fixture.token),
        )
        .await?;
    for response in [updated, deleted] {
        let status = response.status();
        let body: Value = response.json().await?;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["code"], "pack_managed_namespace");
    }
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_cache_writes_use_selected_rights_and_keep_execution_attribution() -> Result<()> {
    let fixture =
        fixture_with_capabilities(false, false, &["read"], &["read", "create", "update"]).await?;
    grant_cache_setup(&fixture).await?;
    for (owner, namespace, mode) in [
        ("recorded_emitter", "published", "snapshot"),
        ("foreign_events", "published", "snapshot"),
        ("recorded_emitter", "live_data", "live"),
    ] {
        assert_eq!(
            fixture
                .ctx
                .post(
                    "/api/v1/cache/namespaces",
                    json!({"owner_type":"pack", "owner_ref":owner,
            "namespace":namespace, "mode":mode}),
                    fixture.ctx.token()
                )
                .await?
                .status(),
            StatusCode::CREATED
        );
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
    let generation = publish_cache_snapshot(&fixture, "published", &fixture.token).await?;
    let response = fixture.ctx.get(&format!("/api/v1/cache/namespaces/published/generations/{generation}?owner_type=pack&owner_ref=recorded_emitter"), Some(&fixture.token)).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["created_by_execution"], fixture.credential.execution_id,
        "{body}"
    );
    assert_eq!(fixture.ctx.post("/api/v1/cache/namespaces/published/generations", json!({"owner_type":"pack",
        "owner_ref":"foreign_events", "client_refresh_id":"denied", "expected_active_generation_id":null,
        "expected_chunk_count":0}), Some(&fixture.token)).await?.status(), StatusCode::FORBIDDEN);
    let applied = fixture.ctx.post("/api/v1/cache/namespaces/live_data/entries/apply", json!({
        "owner_type":"pack","owner_ref":"recorded_emitter","request_id":"recorded-write", "expected_revision":0,
        "entries":[{"external_id":"a","value":"recorded-live"}]
    }), Some(&fixture.token)).await?;
    let status = applied.status();
    let body: Value = applied.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["revision"], 1);
    let response = fixture
        .ctx
        .post(
            "/api/v1/cache/namespaces/live_data/entries/lookup",
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter","external_id":"a"
            }),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["item"]["value"], "recorded-live");
    fixture.ctx.flush_audit().await?;
    let audits = attune_common::audit::AuditRepository::search(
        &fixture.ctx.pool,
        &attune_common::audit::AuditEventFilters {
            event_type: Some("cache.generation.created".into()),
            actor_identity: Some(fixture.credential.sponsor_identity_id),
            outcome: Some(attune_common::audit::AuditOutcome::Success),
            limit: Some(10),
            ..Default::default()
        },
    )
    .await?;
    let audit = audits
        .first()
        .expect("Recorded cache refresh should be audited under its sponsor");
    let details = audit.details.as_ref().unwrap();
    assert_eq!(
        details["runtime_authority"]["admission_id"],
        fixture.credential.admission_id
    );
    assert_eq!(
        details["runtime_authority"]["execution_id"],
        fixture.credential.execution_id
    );
    assert_eq!(
        details["runtime_authority"]["sponsor_identity_id"],
        fixture.credential.sponsor_identity_id
    );
    assert!(!details.to_string().contains("recorded-live"));
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        fixture
            .ctx
            .post(
                &format!("/api/v1/cache/namespaces/published/generations/{generation}/abandon"),
                json!({"owner_type":"pack","owner_ref":"recorded_emitter"}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

async fn grant_cache_setup(fixture: &Fixture) -> Result<()> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let writer = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.cache_setup".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"caches","actions":["read","create","update","delete"]}]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: writer.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(fixture.requester)
        .await;
    Ok(())
}

async fn publish_cache_snapshot(fixture: &Fixture, namespace: &str, token: &str) -> Result<Id> {
    let generations = format!("/api/v1/cache/namespaces/{namespace}/generations");
    let response = fixture
        .ctx
        .post(
            &generations,
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter", "client_refresh_id":"seed",
                "expected_active_generation_id":null,"expected_chunk_count":1
            }),
            Some(token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let generation = body["data"]["generation_id"].as_i64().unwrap();
    let response = fixture
        .ctx
        .put(
            &format!("{generations}/{generation}/chunks/0"),
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter", "entries":[
                    {"external_id":"a","value":"first"}, {"external_id":"b","value":"second"}]
            }),
            Some(token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    for (operation, fields) in [
        ("seal", json!({"expected_chunk_count":1})),
        ("promote", json!({"expected_active_generation_id":null})),
    ] {
        let mut request = json!({"owner_type":"pack","owner_ref":"recorded_emitter"});
        request
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let response = fixture
            .ctx
            .post(
                &format!("{generations}/{generation}/{operation}"),
                request,
                Some(token),
            )
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    Ok(generation)
}

#[tokio::test]
async fn recorded_cache_clone_requires_read_as_well_as_refresh_rights() -> Result<()> {
    use attune_common::repositories::cache::{
        CacheGenerationRepository, CacheNamespaceRepository, CacheOwnerScope,
    };
    let fixture = fixture_with_capabilities(false, false, &["read"], &["create", "update"]).await?;
    grant_cache_setup(&fixture).await?;
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/cache/namespaces",
                json!({"owner_type":"pack", "owner_ref":"recorded_emitter",
        "namespace":"published"}),
                fixture.ctx.token()
            )
            .await?
            .status(),
        StatusCode::CREATED
    );
    let base = publish_cache_snapshot(&fixture, "published", fixture.ctx.token().unwrap()).await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let response = fixture.ctx.post("/api/v1/cache/namespaces/published/generations", json!({"owner_type":"pack",
        "owner_ref":"recorded_emitter", "client_refresh_id":"clone-denied", "expected_active_generation_id":base,
        "source_generation_id":base, "expected_chunk_count":0}), Some(&fixture.token)).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let scope = CacheOwnerScope::pack(fixture.pack_id, Some("recorded_emitter".into()));
    let namespace = CacheNamespaceRepository::resolve(&fixture.ctx.pool, &scope, "published")
        .await?
        .unwrap();
    assert_eq!(
        CacheGenerationRepository::list_for_namespace(&fixture.ctx.pool, namespace.id, 10)
            .await?
            .len(),
        1
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_standard_cache_reads_are_owner_bound_live_and_independent_of_requester_activity(
) -> Result<()> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let fixture = fixture(true).await?;
    let writer = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.cache_setup".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"caches","actions":["read","create","update","delete"]}]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: writer.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(fixture.requester)
        .await;
    for owner in ["recorded_emitter", "foreign_events"] {
        let created = fixture
            .ctx
            .post(
                "/api/v1/cache/namespaces",
                json!({"owner_type":"pack", "owner_ref":owner, "namespace":"users", "mode":"live"}),
                fixture.ctx.token(),
            )
            .await?;
        let status = created.status();
        let body: Value = created.json().await?;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let applied = fixture.ctx.post("/api/v1/cache/namespaces/users/entries/apply", json!({
            "owner_type":"pack", "owner_ref":owner, "request_id":"seed", "expected_revision":0,
            "entries":[{"external_id":"a","value":{"owner":owner}}]
        }), fixture.ctx.token()).await?;
        let status = applied.status();
        let body: Value = applied.json().await?;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let created = fixture
        .ctx
        .post(
            "/api/v1/cache/namespaces",
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter","namespace":"snapshot"
            }),
            fixture.ctx.token(),
        )
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let generations = "/api/v1/cache/namespaces/snapshot/generations";
    let response = fixture
        .ctx
        .post(
            generations,
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter", "client_refresh_id":"seed",
                "expected_active_generation_id":null,"expected_chunk_count":1
            }),
            fixture.ctx.token(),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let generation = body["data"]["generation_id"].as_i64().unwrap();
    assert_eq!(
        fixture
            .ctx
            .put(
                &format!("{generations}/{generation}/chunks/0"),
                json!({
                    "owner_type":"pack","owner_ref":"recorded_emitter", "entries":[
                        {"external_id":"a","value":"first"}, {"external_id":"b","value":"second"}]
                }),
                fixture.ctx.token()
            )
            .await?
            .status(),
        StatusCode::OK
    );
    for (operation, fields) in [
        ("seal", json!({"expected_chunk_count":1})),
        ("promote", json!({"expected_active_generation_id":null})),
    ] {
        let mut request = json!({"owner_type":"pack","owner_ref":"recorded_emitter"});
        request
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        assert_eq!(
            fixture
                .ctx
                .post(
                    &format!("{generations}/{generation}/{operation}"),
                    request,
                    fixture.ctx.token()
                )
                .await?
                .status(),
            StatusCode::OK
        );
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
    let local = "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref=recorded_emitter";
    assert_eq!(
        fixture.ctx.get(local, Some(&fixture.token)).await?.status(),
        StatusCode::OK
    );
    let response = fixture
        .ctx
        .post(
            "/api/v1/cache/namespaces/users/entries/lookup",
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter","external_id":"a"
            }),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["revision"], 1);
    assert_eq!(body["data"]["item"]["value"]["owner"], "recorded_emitter");
    let response = fixture
        .ctx
        .post(
            "/api/v1/cache/namespaces/users/entries/lookup-many",
            json!({
                "owner_type":"pack","owner_ref":"recorded_emitter","external_ids":["a","absent"]
            }),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(body["data"]["missing_external_ids"], json!(["absent"]));
    assert_eq!(
        fixture
            .ctx
            .get(
                "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref=foreign_events",
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = fixture
        .ctx
        .get("/api/v1/cache/namespaces?limit=1", Some(&fixture.token))
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["namespaces"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        body["data"]["namespaces"][0]["owner_ref"],
        "recorded_emitter"
    );
    let first_namespace = body["data"]["namespaces"][0]["namespace"].clone();
    let metadata_cursor = body["data"]["next_cursor"].as_str().unwrap();
    let metadata_continuation =
        format!("/api/v1/cache/namespaces?limit=1&cursor={metadata_cursor}");
    let response = fixture
        .ctx
        .get(&metadata_continuation, Some(&fixture.token))
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["namespaces"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        body["data"]["namespaces"][0]["owner_ref"],
        "recorded_emitter"
    );
    assert_ne!(body["data"]["namespaces"][0]["namespace"], first_namespace);
    assert!(body["data"]["next_cursor"].is_null());
    let response = fixture
        .ctx
        .get(
            "/api/v1/cache/namespaces?owner_type=pack&owner_ref=foreign_events",
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["namespaces"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/cache/namespaces/users/entries/apply",
                json!({"owner_type":"pack", "owner_ref":"recorded_emitter",
                    "request_id":"denied", "expected_revision":1, "entries":[{"external_id":"x","value":false}]}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    for path in [
        format!("{generations}?owner_type=pack&owner_ref=recorded_emitter"),
        format!("{generations}/{generation}?owner_type=pack&owner_ref=recorded_emitter"),
    ] {
        assert_eq!(
            fixture.ctx.get(&path, Some(&fixture.token)).await?.status(),
            StatusCode::OK
        );
    }
    let scan = "/api/v1/cache/namespaces/snapshot/entries?owner_type=pack&owner_ref=recorded_emitter&limit=1";
    let response = fixture.ctx.get(scan, Some(&fixture.token)).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["items"][0]["external_id"], "a");
    let cursor = body["data"]["next_cursor"].as_str().unwrap();
    let continuation = format!("{scan}&generation={generation}&cursor={cursor}");
    let response = fixture.ctx.get(&continuation, Some(&fixture.token)).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["items"][0]["external_id"], "b");
    let mut transaction = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_policy(&mut transaction, fixture.policy_id).await?;
    transaction.commit().await?;
    assert_eq!(
        fixture
            .ctx
            .get(&metadata_continuation, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .ctx
            .get(&continuation, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        fixture.ctx.get(local, Some(&fixture.token)).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_named_selection_without_cache_read_cannot_use_local_namespaces() -> Result<()> {
    use attune_common::repositories::cache::{
        CacheNamespacePolicy, CacheNamespaceRepository, CacheOwnerScope, CreateCacheNamespaceInput,
    };
    let fixture = fixture(false).await?;
    CacheNamespaceRepository::create(
        &fixture.ctx.pool,
        CreateCacheNamespaceInput {
            mode: Default::default(),
            owner: CacheOwnerScope::pack(fixture.pack_id, Some("recorded_emitter".into())),
            namespace: "users".into(),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await?;
    assert_eq!(
        fixture
            .ctx
            .get(
                "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref=recorded_emitter",
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_credentials_can_read_public_schema_metadata_but_terminal_tokens_cannot(
) -> Result<()> {
    let fixture = fixture(true).await?;
    for path in [
        "/api/v1/schema-types",
        "/api/v1/packs/recorded_emitter/schema-types",
    ] {
        assert_eq!(
            fixture.ctx.get(path, Some(&fixture.token)).await?.status(),
            StatusCode::OK
        );
    }
    let request = json!({"context":"flat_document", "schema":{"message":{"type":"string"}}});
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/schema-types/preview",
                request.clone(),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::OK
    );
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        fixture
            .ctx
            .get("/api/v1/schema-types", Some(&fixture.token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/schema-types/preview",
                request,
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_key_creation_checks_real_ownership_before_uniqueness_and_survives_requester_freeze(
) -> Result<()> {
    let fixture = fixture_with_key_actions(false, false, &["create"]).await?;
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        FindByRef,
    };
    let action = ActionRepository::find_by_ref(&fixture.ctx.pool, "recorded_emitter.emit")
        .await?
        .unwrap();
    ActionRepository::update(
        &fixture.ctx.pool,
        action.id,
        UpdateActionInput {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    for (owner_type, owner_field, owner_ref) in [
        ("pack", "owner_pack_ref", "recorded_emitter"),
        ("action", "owner_action_ref", "recorded_emitter.emit"),
    ] {
        let mut request = json!({"local_ref":"created", "owner_type":owner_type,
            "name":"Created by admitted runtime", "encrypted":true, "value":"caller supplied"});
        request[owner_field] = json!(owner_ref);
        let response = fixture
            .ctx
            .post("/api/v1/keys", request.clone(), Some(&fixture.token))
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["data"]["value"], "caller supplied");
        assert_eq!(
            fixture
                .ctx
                .post("/api/v1/keys", request, Some(&fixture.token))
                .await?
                .status(),
            StatusCode::CONFLICT
        );
    }
    for owner in ["foreign_events", "missing_owner"] {
        assert_eq!(
            fixture
                .ctx
                .post(
                    "/api/v1/keys",
                    json!({"local_ref":"created","owner_type":"pack", "owner_pack_ref":owner,
            "name":"Denied", "value":"not written"}),
                    Some(&fixture.token)
                )
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/keys",
                json!({"local_ref":"created","owner_type":"system",
        "name":"Denied", "value":"not written"}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(fixture.ctx.post("/api/v1/keys", json!({"local_ref":"after_completion","owner_type":"pack", "owner_pack_ref":"recorded_emitter",
        "name":"Denied", "value":"not written"}), Some(&fixture.token)).await?.status(), StatusCode::UNAUTHORIZED);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_key_mutations_require_selected_local_rights_and_preserve_material_disclosure_checks(
) -> Result<()> {
    use attune_common::{
        crypto,
        models::OwnerType,
        repositories::key::{CreateKeyInput, KeyRepository},
    };
    let fixture = fixture_with_key_actions(false, false, &["update", "delete"]).await?;
    let encryption = fixture
        .ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .unwrap();
    let input = |local: &str, owner_type| CreateKeyInput {
        local_ref: local.into(),
        owner_type,
        owner_identity: None,
        owner_pack: (owner_type == OwnerType::Pack).then_some(fixture.pack_id),
        owner_pack_ref: (owner_type == OwnerType::Pack).then(|| "recorded_emitter".into()),
        owner_action: None,
        owner_action_ref: None,
        owner_sensor: None,
        owner_sensor_ref: None,
        name: "Mutation fixture".into(),
        encrypted: true,
        encryption_key_hash: Some(crypto::hash_encryption_key(encryption)),
        value: crypto::encrypt_json(&json!("protected original"), encryption).unwrap(),
    };
    let local = KeyRepository::create(&fixture.ctx.pool, input("mutable", OwnerType::Pack)).await?;
    let foreign =
        KeyRepository::create(&fixture.ctx.pool, input("not_mutable", OwnerType::System)).await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let path = format!("/api/v1/keys/{}", local.r#ref);
    let foreign_path = format!("/api/v1/keys/{}", foreign.r#ref);
    let response = fixture
        .ctx
        .put(&path, json!({"name":"New label"}), Some(&fixture.token))
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["value"].is_null(), "{body}");
    assert!(!body.to_string().contains("protected original"));
    assert_eq!(
        fixture
            .ctx
            .put(&path, json!({"encrypted":false}), Some(&fixture.token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .ctx
            .put(
                &foreign_path,
                json!({"name":"Forbidden"}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .ctx
            .delete(&foreign_path, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .ctx
            .get("/api/v1/keys", Some(&fixture.token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = fixture
        .ctx
        .put(
            &path,
            json!({"value":"known replacement"}),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["value"].is_null());
    assert_eq!(
        fixture
            .ctx
            .delete(&path, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::OK
    );
    assert!(KeyRepository::find_by_ref(&fixture.ctx.pool, &local.r#ref)
        .await?
        .is_none());
    assert!(
        KeyRepository::find_by_ref(&fixture.ctx.pool, &foreign.r#ref)
            .await?
            .is_some()
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn standard_key_access_uses_pinned_owner_ids_and_never_follows_reused_action_refs(
) -> Result<()> {
    use attune_common::{
        models::OwnerType,
        repositories::{
            action::ActionRepository,
            key::{CreateKeyInput, KeyRepository},
            FindByRef,
        },
    };
    for workflow_context in [false, true] {
        let fixture = fixture_with_workflow(true, workflow_context).await?;
        let source_ref = if workflow_context {
            "recorded_emitter.workflow"
        } else {
            "recorded_emitter.emit"
        };
        let action = ActionRepository::find_by_ref(&fixture.ctx.pool, source_ref)
            .await?
            .unwrap();
        let sibling = create_test_action(
            &fixture.ctx.pool,
            fixture.pack_id,
            "recorded_emitter",
            "recorded_emitter.sibling",
        )
        .await?;
        let input = |local: &str, owner_type, action: Option<&attune_common::models::Action>| {
            CreateKeyInput {
                local_ref: local.into(),
                owner_type,
                owner_identity: None,
                owner_pack: (owner_type == OwnerType::Pack).then_some(fixture.pack_id),
                owner_pack_ref: (owner_type == OwnerType::Pack).then(|| "recorded_emitter".into()),
                owner_action: action.map(|action| action.id),
                owner_action_ref: action.map(|action| action.r#ref.clone()),
                owner_sensor: None,
                owner_sensor_ref: None,
                name: "Standard scope fixture".into(),
                encrypted: false,
                encryption_key_hash: None,
                value: json!("owned value"),
            }
        };
        let pack_key = KeyRepository::create(
            &fixture.ctx.pool,
            input("pack_input", OwnerType::Pack, None),
        )
        .await?;
        let action_key = KeyRepository::create(
            &fixture.ctx.pool,
            input("action_input", OwnerType::Action, Some(&action)),
        )
        .await?;
        let sibling_key = KeyRepository::create(
            &fixture.ctx.pool,
            input("sibling_input", OwnerType::Action, Some(&sibling)),
        )
        .await?;
        for (key, expected) in [
            (&pack_key, StatusCode::OK),
            (&action_key, StatusCode::OK),
            (&sibling_key, StatusCode::NOT_FOUND),
        ] {
            assert_eq!(
                fixture
                    .ctx
                    .get(&format!("/api/v1/keys/{}", key.r#ref), Some(&fixture.token))
                    .await?
                    .status(),
                expected
            );
        }
        let mut listed = Vec::new();
        for page in 1..=2 {
            let response = fixture
                .ctx
                .get(
                    &format!("/api/v1/keys?per_page=1&page={page}"),
                    Some(&fixture.token),
                )
                .await?;
            let status = response.status();
            let body: Value = response.json().await?;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["pagination"]["total_items"], 2, "{body}");
            let items = body["items"].as_array().unwrap();
            assert_eq!(items.len(), 1);
            assert!(items[0].get("value").is_none());
            listed.push(items[0]["ref"].as_str().unwrap().to_string());
        }
        listed.sort();
        let mut expected = vec![pack_key.r#ref.clone(), action_key.r#ref.clone()];
        expected.sort();
        assert_eq!(listed, expected);
        KeyRepository::delete(&fixture.ctx.pool, action_key.id).await?;
        ActionRepository::delete(&fixture.ctx.pool, action.id).await?;
        let replacement = create_test_action(
            &fixture.ctx.pool,
            fixture.pack_id,
            "recorded_emitter",
            source_ref,
        )
        .await?;
        assert_ne!(replacement.id, action.id);
        let rebound = KeyRepository::create(
            &fixture.ctx.pool,
            input("action_input", OwnerType::Action, Some(&replacement)),
        )
        .await?;
        assert_eq!(rebound.r#ref, action_key.r#ref);
        assert_eq!(
            fixture
                .ctx
                .get(
                    &format!("/api/v1/keys/{}", rebound.r#ref),
                    Some(&fixture.token)
                )
                .await?
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            fixture
                .ctx
                .get(
                    &format!("/api/v1/keys/{}", pack_key.r#ref),
                    Some(&fixture.token)
                )
                .await?
                .status(),
            StatusCode::OK
        );
        let listed: Value = fixture
            .ctx
            .get("/api/v1/keys", Some(&fixture.token))
            .await?
            .json()
            .await?;
        assert_eq!(listed["pagination"]["total_items"], 1, "{listed}");
        assert_eq!(listed["items"][0]["ref"], pack_key.r#ref);
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn recorded_signing_uses_external_approval_and_operator_profile_without_plaintext_key_access(
) -> Result<()> {
    use attune_common::{
        crypto,
        models::OwnerType,
        repositories::key::{CreateKeyInput, KeyRepository},
    };
    let fixture = fixture(false).await?;
    let encryption = fixture
        .ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .unwrap();
    let value = json!({
        "private_key_pem": include_str!("../../common/tests/fixtures/jwt_signing_test_private.pem"),
        "profiles": {"test": {"issuer": "approved-client", "audience": "https://example.test",
            "subjects": ["approved-subject"], "pack_refs": ["recorded_emitter"], "max_ttl_seconds": 120}}
    });
    let key = KeyRepository::create(
        &fixture.ctx.pool,
        CreateKeyInput {
            local_ref: "recorded_signer".into(),
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Recorded signer".into(),
            encrypted: true,
            encryption_key_hash: Some(crypto::hash_encryption_key(encryption)),
            value: crypto::encrypt_json(&value, encryption)?,
        },
    )
    .await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let endpoint = format!("/api/v1/keys/{}/sign-jwt", key.r#ref);
    let request = json!({"profile_ref": "test", "subject": "approved-subject", "ttl_seconds": 60});
    let response = fixture
        .ctx
        .post(&endpoint, request.clone(), Some(&fixture.token))
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["assertion"]
        .as_str()
        .is_some_and(|token| token.split('.').count() == 3));
    assert!(!body.to_string().contains("private_key_pem"));
    assert_eq!(
        fixture
            .ctx
            .get(&format!("/api/v1/keys/{}", key.r#ref), Some(&fixture.token))
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .ctx
            .post(
                &endpoint,
                json!({"profile_ref":"test","subject":"forbidden-subject","ttl_seconds":60}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut transaction = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_approval(&mut transaction, fixture.signing_approval_id)
        .await?;
    transaction.commit().await?;
    assert_eq!(
        fixture
            .ctx
            .post(&endpoint, request.clone(), Some(&fixture.token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        fixture
            .ctx
            .post(&endpoint, request, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_key_reads_are_scoped_idempotently_tracked_and_forwarded_into_event_provenance(
) -> Result<()> {
    use attune_common::{
        models::OwnerType,
        repositories::{
            execution_key_read::ExecutionKeyReadRepository,
            key::{CreateKeyInput, KeyRepository},
        },
    };
    let fixture = fixture(false).await?;
    let key = KeyRepository::create(
        &fixture.ctx.pool,
        CreateKeyInput {
            local_ref: "runtime_input".into(),
            owner_type: OwnerType::Pack,
            owner_identity: None,
            owner_pack: Some(fixture.pack_id),
            owner_pack_ref: Some("recorded_emitter".into()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Runtime input".into(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!("key value"),
        },
    )
    .await?;
    let system_key = KeyRepository::create(
        &fixture.ctx.pool,
        CreateKeyInput {
            local_ref: "not_local".into(),
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "System input".into(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!("not delivered"),
        },
    )
    .await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    for _ in 0..2 {
        let response = fixture
            .ctx
            .get(&format!("/api/v1/keys/{}", key.r#ref), Some(&fixture.token))
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["value"], "key value");
    }
    assert_eq!(
        fixture
            .ctx
            .get(
                &format!("/api/v1/keys/{}", system_key.r#ref),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    let mut connection = fixture.ctx.pool.acquire().await?;
    let origins =
        ExecutionKeyReadRepository::origins(&mut connection, fixture.credential.admission_id)
            .await?;
    assert_eq!(origins.len(), 1);
    assert_eq!(origins[0].key_id, key.id);
    drop(connection);
    let listed: Value = fixture
        .ctx
        .get("/api/v1/keys", Some(&fixture.token))
        .await?
        .json()
        .await?;
    assert_eq!(listed["pagination"]["total_items"], 1, "{listed}");
    assert_eq!(listed["items"][0]["ref"], key.r#ref);
    assert!(!listed.to_string().contains("key value"));
    // No schema/input secret is needed to classify an SDK-capable log.
    let mut connection = fixture.ctx.pool.acquire().await?;
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut connection,
        ENTITY_EXECUTION_CONFIG,
        fixture.credential.execution_id,
        &[],
    )
    .await?;
    drop(connection);
    use attune_common::repositories::{
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        FindById,
    };
    let viewer = IdentityRepository::create(
        &fixture.ctx.pool,
        CreateIdentityInput {
            login: "sdk_log_viewer".into(),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await?;
    let set = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.sdk_log_viewer".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([
                {"resource":"executions","actions":["read","decrypt"]},
                {"resource":"artifacts","actions":["read"]}
            ]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: viewer.id,
            permset: set.id,
        },
    )
    .await?;
    let viewer_token = attune_common::auth::jwt::generate_access_token(
        viewer.id,
        &viewer.login,
        &fixture.ctx.state.jwt_config,
    )?;
    let viewer = attune_api::auth::middleware::AuthenticatedUser {
        claims: attune_common::auth::jwt::validate_token(
            &viewer_token,
            &fixture.ctx.state.jwt_config,
        )?,
    };
    let execution =
        ExecutionRepository::find_by_id(&fixture.ctx.pool, fixture.credential.execution_id)
            .await?
            .unwrap();
    use attune_common::{
        models::{ArtifactClassification, ArtifactType, ArtifactVisibility, RetentionPolicyType},
        repositories::artifact::{
            ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
            CreateArtifactVersionInput,
        },
    };
    let log = ArtifactRepository::create(
        &fixture.ctx.pool,
        CreateArtifactInput {
            r#ref: "recorded_emitter.emit.stdout.log".into(),
            scope: OwnerType::Action,
            owner: "recorded_emitter.emit".into(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::RuntimeLog,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 10,
            name: None,
            description: None,
            content_type: Some("text/plain".into()),
            data: None,
        },
    )
    .await?;
    ArtifactVersionRepository::create(
        &fixture.ctx.pool,
        CreateArtifactVersionInput {
            artifact: log.id,
            execution: Some(execution.id),
            content_type: Some("text/plain".into()),
            content: None,
            content_json: None,
            file_path: None,
            meta: None,
            created_by: None,
        },
    )
    .await?;
    let artifact_path = format!("/api/v1/executions/{}/artifacts", execution.id);
    let hidden: Value = fixture
        .ctx
        .get(&artifact_path, Some(&viewer_token))
        .await?
        .json()
        .await?;
    assert_eq!(
        hidden["items"].as_array().map(Vec::len),
        Some(0),
        "{hidden}"
    );
    assert!(attune_api::secret_disclosure::authorize_runtime_log(
        &fixture.ctx.state,
        &viewer,
        &execution
    )
    .await
    .is_err());
    let key_set = PermissionSetRepository::create(&fixture.ctx.pool, CreatePermissionSetInput {
        r#ref: "fixture.sdk_log_key".into(), pack: None, pack_ref: None, label: None, description: None,
        grants: json!([{"resource":"keys","actions":["read"],"constraints":{"refs":[key.r#ref]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: viewer.identity_id()?,
            permset: key_set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(viewer.identity_id()?)
        .await;
    attune_api::secret_disclosure::authorize_runtime_log(&fixture.ctx.state, &viewer, &execution)
        .await?;
    let visible: Value = fixture
        .ctx
        .get(&artifact_path, Some(&viewer_token))
        .await?
        .json()
        .await?;
    assert_eq!(
        visible["items"].as_array().map(Vec::len),
        Some(1),
        "{visible}"
    );
    let response = fixture
        .ctx
        .post(
            "/api/v1/events",
            json!({"trigger_ref": fixture.trigger_ref,
        "payload": {"secret": "key value"}}),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["data"]["id"].as_i64().unwrap();
    let mut connection = fixture.ctx.pool.acquire().await?;
    let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *connection,
        ENTITY_EVENT_PAYLOAD,
        id,
    )
    .await?;
    let provenance: attune_common::secret_provenance::SecretProvenance =
        serde_json::from_str(secrets[0].source_ref.as_deref().unwrap())?;
    assert!(provenance.origins.iter().any(|origin| matches!(origin, attune_common::secret_provenance::SecretOrigin::Key(origin) if origin.key_id == key.id)));
    drop(connection);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn scoped_event_emission_survives_requester_freeze_and_records_emitter_and_input_origins(
) -> Result<()> {
    let fixture = fixture(false).await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let response = fixture
        .ctx
        .post(
            "/api/v1/events",
            json!({"trigger_ref": fixture.trigger_ref,
        "payload": {"secret": "forwarded secret"}}),
            Some(&fixture.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(!body.to_string().contains("forwarded secret"));
    let event_id = body["data"]["id"].as_i64().unwrap();
    let created = serde_json::from_value(body["data"]["created"].clone())?;
    let mut connection = fixture.ctx.pool.acquire().await?;
    let emitter = ExecutionEventEmitterRepository::find(&mut connection, event_id, created)
        .await?
        .unwrap();
    assert_eq!(emitter.execution_id, fixture.credential.execution_id);
    assert_eq!(
        emitter.sponsor_identity_id,
        fixture.credential.sponsor_identity_id
    );
    assert_ne!(emitter.sponsor_identity_id, fixture.requester);
    assert_eq!(emitter.attempt, fixture.credential.attempt);
    let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *connection,
        ENTITY_EVENT_PAYLOAD,
        event_id,
    )
    .await?;
    assert_eq!(secrets.len(), 1);
    let provenance: attune_common::secret_provenance::SecretProvenance =
        serde_json::from_str(secrets[0].source_ref.as_deref().unwrap())?;
    assert!(provenance.origins.iter().any(|origin| matches!(origin,
        attune_common::secret_provenance::SecretOrigin::Entity { entity_type, entity_id, path }
        if entity_type == ENTITY_EXECUTION_CONFIG && *entity_id == fixture.credential.execution_id && path == "/input")));
    drop(connection);
    // Attribution survives execution retention without adding an execution FK.
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    ExecutionRepository::delete(&fixture.ctx.pool, fixture.credential.execution_id).await?;
    let mut connection = fixture.ctx.pool.acquire().await?;
    assert_eq!(
        ExecutionEventEmitterRepository::find(&mut connection, event_id, created).await?,
        Some(emitter)
    );
    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    assert_eq!(
        ExecutionEventEmitterRepository::cleanup_orphans(&mut connection, 100, cutoff).await?,
        0
    );
    drop(connection);
    attune_common::repositories::event::EventRepository::delete(&fixture.ctx.pool, event_id)
        .await?;
    let mut connection = fixture.ctx.pool.acquire().await?;
    attune_common::repositories::event_schema::EventSchemaRepository::cleanup_orphans(
        &mut connection,
        100,
        cutoff,
    )
    .await?;
    assert!(
        ExecutionEventEmitterRepository::find(&mut connection, event_id, created)
            .await?
            .is_none()
    );
    drop(connection);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn standard_or_foreign_scope_cannot_emit_and_unconverted_consumers_fail_closed() -> Result<()>
{
    let standard = fixture(true).await?;
    let response = standard
        .ctx
        .post(
            "/api/v1/events",
            json!({"trigger_ref": standard.trigger_ref,
        "payload": {"secret": "value"}}),
            Some(&standard.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    standard.ctx.cleanup().await?;
    let scoped = fixture(false).await?;
    let response = scoped
        .ctx
        .post(
            "/api/v1/events",
            json!({"trigger_ref": scoped.foreign_trigger_ref,
        "payload": {"secret": "value"}}),
            Some(&scoped.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        scoped
            .ctx
            .get("/api/v1/packs", Some(&scoped.token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    scoped.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn terminal_or_revoked_recorded_credentials_are_rejected_before_effects() -> Result<()> {
    let fixture = fixture(false).await?;
    let selected = vec!["recorded_emitter.events".to_string()];
    let mismatched_selection = generate_admitted_execution_token(
        &fixture.credential,
        &["standard".into()],
        &fixture.ctx.state.jwt_config,
        Some(360),
    )?;
    let mut wrong_attempt = fixture.credential.clone();
    wrong_attempt.attempt = uuid::Uuid::new_v4();
    let mismatched_attempt = generate_admitted_execution_token(
        &wrong_attempt,
        &selected,
        &fixture.ctx.state.jwt_config,
        Some(360),
    )?;
    for token in [mismatched_selection, mismatched_attempt] {
        assert_eq!(
            fixture
                .ctx
                .post(
                    "/api/v1/events",
                    json!({"trigger_ref": fixture.trigger_ref,
            "payload": {"secret": "value"}}),
                    Some(&token)
                )
                .await?
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/events",
                json!({"trigger_ref": fixture.trigger_ref,
        "payload": {"secret": "value"}}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    let fixture = self::fixture(false).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_policy(&mut tx, fixture.policy_id).await?;
    tx.commit().await?;
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/events",
                json!({"trigger_ref": fixture.trigger_ref,
        "payload": {"secret": "value"}}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}
