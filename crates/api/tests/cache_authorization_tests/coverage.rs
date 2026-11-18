use super::*;
use attune_common::{
    config::CacheAdmissionConfig,
    models::ExecutionStatus,
    repositories::{
        cache::{CacheNamespacePolicy, CreateCacheNamespaceInput, ManagedCacheNamespaceDefinition},
        executable_snapshot::ExecutableSnapshotRepository,
        execution::ExecutionRepository,
        identity::CreatePermissionSetInput,
        runtime_authority::{RuntimeCredentialBinding, RuntimeOwner},
        schema_contract::{BindingOwner, SchemaContractRepository},
    },
    runtime_authority::{HumanAttribution, InvocationBinding, RequestAttribution},
};

async fn delegated(
    ctx: &TestContext,
    fixture: &PublishedFixture,
    refs: &[&str],
) -> Result<(attune_common::models::Execution, String)> {
    let execution = helpers::delegated_source::create(
        &ctx.pool,
        CreateExecutionInput {
            action: Some(fixture.action.id),
            action_ref: fixture.action.r#ref.clone(),
            executor: Some(fixture.sponsor.id),
            permission_set_refs: refs.iter().map(|value| (*value).into()).collect(),
            ..Default::default()
        },
    )
    .await?;
    let token = helpers::delegated_source::execution_token(ctx, execution.id).await?;
    Ok((execution, token))
}

async fn scoped_token(
    ctx: &TestContext,
    fixture: &PublishedFixture,
    name: &str,
    managing_pack: Option<&attune_common::models::Pack>,
    actions: &[&str],
    owner_ref: &str,
    refs: &[&str],
) -> Result<String> {
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!(
                "{}.{}",
                managing_pack.map_or("test", |pack| pack.r#ref.as_str()),
                name
            ),
            pack: managing_pack.map(|pack| pack.id),
            pack_ref: managing_pack.map(|pack| pack.r#ref.clone()),
            label: None,
            description: None,
            grants: json!([{"resource":"caches", "actions":actions,
                "constraints":{"owner_types":["pack"], "owner_refs":[owner_ref], "refs":refs}}]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.sponsor.id,
            permset: set.id,
        },
    )
    .await?;
    AuthorizationService::invalidate_identity_authz_cache(fixture.sponsor.id).await;
    AuthorizationService::invalidate_permission_set_caches().await;
    Ok(delegated(ctx, fixture, &[&set.r#ref]).await?.1)
}

async fn begin(
    ctx: &TestContext,
    token: &str,
    namespace: &str,
    expected: Option<i64>,
    count: i64,
    status: StatusCode,
) -> Result<Value> {
    let response = ctx
        .post(
            &format!("/api/v1/cache/namespaces/{namespace}/generations"),
            json!({"owner_type":"pack", "owner_ref":PACK,
            "client_refresh_id":format!("coverage-{}", uuid::Uuid::new_v4().simple()),
            "expected_active_generation_id":expected,
            "expected_chunk_count":count, "expected_record_count":count}),
            Some(token),
        )
        .await?;
    let actual = response.status();
    let body: Value = response.json().await?;
    ensure!(
        actual == status,
        "generation {namespace}: expected {status}, got {actual}: {body}"
    );
    Ok(body)
}

async fn namespace_names(ctx: &TestContext, token: &str) -> Result<Vec<String>> {
    // No owner filter: exercise the repository's SQL visibility predicate.
    let response = ctx
        .get("/api/v1/cache/namespaces?limit=100", Some(token))
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    ensure!(
        status == StatusCode::OK,
        "namespace listing: {status}: {body}"
    );
    let rows = body["data"]["namespaces"]
        .as_array()
        .context("namespace list missing")?;
    let mut names = rows
        .iter()
        .map(|row| {
            row["namespace"]
                .as_str()
                .map(str::to_owned)
                .context("namespace missing")
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

async fn collision_namespace(ctx: &TestContext, fixture: &PublishedFixture) -> Result<()> {
    // Its URL name equals the permitted cache's declaration ref. Exact canonical
    // matching must not treat that namespace as an alias for its own definition.
    CacheNamespaceRepository::upsert_managed_definitions(
        &ctx.pool,
        fixture.pack.id,
        PACK,
        &[ManagedCacheNamespaceDefinition {
            mode: Default::default(),
            definition_ref: "cache_auth_repro.collision".into(),
            namespace: "cache_auth_repro.users".into(),
            owner: CacheOwnerScope::pack(fixture.pack.id, Some(PACK.into())),
            policy: CacheNamespacePolicy::default(),
        }],
        &CacheAdmissionConfig::default(),
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn managed_cache_reads_lists_and_publication_use_exact_declaration_refs() -> Result<()> {
    let ctx = fixture_context().await?;
    let result = lifecycle(&ctx).await;
    ctx.cleanup().await?;
    result
}

async fn lifecycle(ctx: &TestContext) -> Result<()> {
    let fixture = publish_fixture(ctx).await?;
    let (_, token) = delegated(ctx, &fixture, &[WRITER]).await?;
    collision_namespace(ctx, &fixture).await?;
    let foreign = helpers::create_test_pack(&ctx.pool, "cache_foreign_owner").await?;
    CacheNamespaceRepository::create_api_with_policy(
        &ctx.pool,
        CreateCacheNamespaceInput {
            mode: Default::default(),
            owner: CacheOwnerScope::pack(foreign.id, Some(foreign.r#ref.clone())),
            namespace: "cache_auth_repro.users".into(),
            policy: CacheNamespacePolicy::default(),
        },
        &CacheAdmissionConfig::default(),
    )
    .await?;
    for credential in [&token, &fixture.access_token] {
        ensure!(
            namespace_names(ctx, credential).await? == ["cache_auth_repro.control", "repro.users"]
        );
        let response = ctx
            .get(
                "/api/v1/cache/namespaces/repro.users?owner_type=pack&owner_ref=cache_auth_repro",
                Some(credential),
            )
            .await?;
        ensure!(response.status() == StatusCode::OK);
        let response = ctx.get(
            "/api/v1/cache/namespaces/cache_auth_repro.users?owner_type=pack&owner_ref=cache_auth_repro",
            Some(credential),
        ).await?;
        ensure!(
            response.status() == StatusCode::FORBIDDEN,
            "namespace collision disclosed another definition"
        );
        let response = ctx
            .get(
                &format!(
                    "/api/v1/cache/namespaces/cache_auth_repro.users?owner_type=pack&owner_ref={}",
                    foreign.r#ref
                ),
                Some(credential),
            )
            .await?;
        ensure!(
            response.status() == StatusCode::FORBIDDEN,
            "foreign owner disclosed a matching ref"
        );
    }
    begin(
        ctx,
        &token,
        "cache_auth_repro.users",
        None,
        0,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let created = begin(ctx, &token, "repro.users", None, 1, StatusCode::CREATED).await?;
    let generation = created["data"]["generation_id"]
        .as_i64()
        .context("generation missing")?;
    let response = ctx
        .put(
            &format!("/api/v1/cache/namespaces/repro.users/generations/{generation}/chunks/0"),
            json!({"owner_type":"pack", "owner_ref":PACK,
            "entries":[{"external_id":"one", "value":{"label":"synthetic"}}]}),
            Some(&token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    ensure!(status == StatusCode::OK, "upload: {status}: {body}");
    for (operation, body) in [
        (
            "seal",
            json!({"owner_type":"pack","owner_ref":PACK,"expected_chunk_count":1}),
        ),
        (
            "promote",
            json!({"owner_type":"pack","owner_ref":PACK,"expected_active_generation_id":null}),
        ),
    ] {
        let response = ctx
            .post(
                &format!(
                    "/api/v1/cache/namespaces/repro.users/generations/{generation}/{operation}"
                ),
                body,
                Some(&token),
            )
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        ensure!(status == StatusCode::OK, "{operation}: {status}: {body}");
    }
    for credential in [&token, &fixture.access_token] {
        for suffix in [
            "generations".to_owned(),
            format!("generations/{generation}"),
            "entries".into(),
        ] {
            let response = ctx.get(
                &format!("/api/v1/cache/namespaces/repro.users/{suffix}?owner_type=pack&owner_ref={PACK}"),
                Some(credential),
            ).await?;
            ensure!(
                response.status() == StatusCode::OK,
                "read {suffix}: {}",
                response.status()
            );
        }
        let response = ctx
            .post(
                "/api/v1/cache/namespaces/repro.users/entries/lookup",
                json!({"owner_type":"pack","owner_ref":PACK,"external_id":"one"}),
                Some(credential),
            )
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        ensure!(
            status == StatusCode::OK && body["data"]["item"]["value"]["label"] == "synthetic",
            "lookup: {status}: {body}"
        );
    }
    let abandoned = begin(
        ctx,
        &token,
        "repro.users",
        Some(generation),
        0,
        StatusCode::CREATED,
    )
    .await?;
    let abandoned = abandoned["data"]["generation_id"]
        .as_i64()
        .context("abandoned generation missing")?;
    let response = ctx
        .post(
            &format!("/api/v1/cache/namespaces/repro.users/generations/{abandoned}/abandon"),
            json!({"owner_type":"pack","owner_ref":PACK}),
            Some(&token),
        )
        .await?;
    ensure!(response.status() == StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn canonical_cache_refs_preserve_scope_actions_and_tombstone_fences() -> Result<()> {
    let ctx = fixture_context().await?;
    let result = boundaries(&ctx).await;
    ctx.cleanup().await?;
    result
}

async fn boundaries(ctx: &TestContext) -> Result<()> {
    let fixture = publish_fixture(ctx).await?;
    let foreign = helpers::create_test_pack(&ctx.pool, "foreign_cache_manager").await?;
    for (name, manager, actions, owner, refs) in [
        (
            "read_only",
            Some(&fixture.pack),
            vec!["read"],
            PACK,
            vec!["cache_auth_repro.users"],
        ),
        (
            "wrong_owner",
            None,
            vec!["read", "create"],
            foreign.r#ref.as_str(),
            vec!["cache_auth_repro.users"],
        ),
        (
            "wrong_manager",
            Some(&foreign),
            vec!["read", "create"],
            PACK,
            vec!["cache_auth_repro.users"],
        ),
        (
            "namespace_only",
            None,
            vec!["read", "create"],
            PACK,
            vec!["repro.users"],
        ),
    ] {
        let token = scoped_token(ctx, &fixture, name, manager, &actions, owner, &refs).await?;
        begin(ctx, &token, "repro.users", None, 0, StatusCode::FORBIDDEN).await?;
        if name == "read_only" {
            ensure!(namespace_names(ctx, &token).await? == ["repro.users"]);
        } else {
            ensure!(
                namespace_names(ctx, &token).await?.is_empty(),
                "{name} disclosed a namespace"
            );
            let response = ctx.get(
                "/api/v1/cache/namespaces/repro.users?owner_type=pack&owner_ref=cache_auth_repro",
                Some(&token),
            ).await?;
            ensure!(
                response.status() == StatusCode::FORBIDDEN,
                "{name}: {}",
                response.status()
            );
        }
    }
    let create_only = scoped_token(
        ctx,
        &fixture,
        "create_only",
        Some(&fixture.pack),
        &["read", "create"],
        PACK,
        &["cache_auth_repro.users"],
    )
    .await?;
    let generation = begin(
        ctx,
        &create_only,
        "repro.users",
        None,
        0,
        StatusCode::CREATED,
    )
    .await?;
    let generation = generation["data"]["generation_id"]
        .as_i64()
        .context("create-only generation missing")?;
    for (operation, body) in [
        (
            "chunks/0",
            json!({"owner_type":"pack","owner_ref":PACK,"checksum":"denied","entries":[]}),
        ),
        (
            "seal",
            json!({"owner_type":"pack","owner_ref":PACK,"expected_chunk_count":0}),
        ),
        (
            "promote",
            json!({"owner_type":"pack","owner_ref":PACK,"expected_active_generation_id":null}),
        ),
        ("abandon", json!({"owner_type":"pack","owner_ref":PACK})),
    ] {
        let path =
            format!("/api/v1/cache/namespaces/repro.users/generations/{generation}/{operation}");
        let response = if operation == "chunks/0" {
            ctx.put(&path, body, Some(&create_only)).await?
        } else {
            ctx.post(&path, body, Some(&create_only)).await?
        };
        ensure!(
            response.status() == StatusCode::FORBIDDEN,
            "create-only token authorized {operation}"
        );
    }
    let (_, token) = delegated(ctx, &fixture, &[WRITER]).await?;
    CacheNamespaceRepository::tombstone(&ctx.pool, fixture.namespace.id).await?;
    let body = begin(ctx, &token, "repro.users", None, 0, StatusCode::CONFLICT).await?;
    ensure!(body["code"] == "namespace_deleted");
    ensure!(namespace_names(ctx, &token).await? == ["cache_auth_repro.control"]);
    // Exercise retirement independently of tombstoning through the normal
    // projection reconciliation repository, retaining the other components.
    let control = CacheNamespaceRepository::resolve_managed_definition(
        &ctx.pool,
        PACK,
        "cache_auth_repro.control",
    )
    .await?
    .context("control namespace missing")?;
    let mut tx = ctx.pool.begin().await?;
    use attune_common::repositories::component_lifecycle::ComponentLifecycleRepository;
    let mut keep =
        ComponentLifecycleRepository::active_projection_ids(&mut tx, fixture.pack.id).await?;
    keep.caches.retain(|id| *id != control.id);
    ComponentLifecycleRepository::reconcile_omissions(&mut tx, fixture.pack.id, &keep).await?;
    tx.commit().await?;
    let retired = CacheNamespaceRepository::find_by_id_including_retired(&ctx.pool, control.id)
        .await?
        .context("retired control missing")?;
    ensure!(retired.retired_at.is_some() && retired.tombstoned_at.is_none());
    begin(
        ctx,
        &token,
        "cache_auth_repro.control",
        None,
        0,
        StatusCode::CONFLICT,
    )
    .await?;
    ensure!(namespace_names(ctx, &token).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn unmanaged_and_prospective_cache_targets_keep_namespace_authorization() -> Result<()> {
    let ctx = fixture_context().await?;
    let result = unmanaged(&ctx).await;
    ctx.cleanup().await?;
    result
}

async fn unmanaged(ctx: &TestContext) -> Result<()> {
    let fixture = publish_fixture(ctx).await?;
    let token = scoped_token(
        ctx,
        &fixture,
        "unmanaged",
        None,
        &["read", "create", "update"],
        PACK,
        &["repro.unmanaged"],
    )
    .await?;
    for (namespace, expected) in [
        ("repro.unmanaged", StatusCode::CREATED),
        ("repro.other", StatusCode::FORBIDDEN),
    ] {
        let response = ctx
            .post(
                "/api/v1/cache/namespaces",
                json!({"owner_type":"pack","owner_ref":PACK,"namespace":namespace}),
                Some(&token),
            )
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        ensure!(
            status == expected,
            "prospective {namespace}: {status}: {body}"
        );
        if status == StatusCode::CREATED {
            ensure!(body["data"]["definition_ref"].is_null());
        }
    }
    ensure!(namespace_names(ctx, &token).await? == ["repro.unmanaged"]);
    begin(ctx, &token, "repro.unmanaged", None, 0, StatusCode::CREATED).await?;
    Ok(())
}

async fn recorded_token(ctx: &TestContext, fixture: &PublishedFixture) -> Result<String> {
    let mut tx = ctx.pool.begin().await?;
    let policy = RuntimeAuthorityRepository::current_policy(&mut tx, fixture.pack.id)
        .await?
        .context("policy missing")?;
    let selected = policy.policy.select(
        policy.id,
        &InvocationBinding::Action {
            action_ref: fixture.action.r#ref.clone(),
        },
        &[WRITER.into()],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: fixture.sponsor.id,
                login: fixture.sponsor.login.clone(),
            },
        },
    )?;
    let snapshot =
        ExecutableSnapshotRepository::resolve_for_action_ref(&mut *tx, &fixture.action.r#ref)
            .await?;
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(fixture.action.id),
            action_ref: fixture.action.r#ref.clone(),
            executor: Some(fixture.sponsor.id),
            status: ExecutionStatus::Running,
            config: Some(json!({"namespace":"repro.users"})),
            permission_set_refs: vec![WRITER.into()],
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
    attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(
        &mut tx, "execution", execution.id, fixture.sponsor.id).await?;
    attune_common::repositories::declared_input_materialization::DeclaredInputMaterializationRepository::seal_execution_inputs(
        &mut tx, execution.id, ctx.state.config.security.encryption_key.as_deref().context("fixture encryption unavailable")?).await?;
    tx.commit().await?;
    let credential = RuntimeCredentialBinding {
        execution_id: execution.id,
        admission_id: admission.id,
        sponsor_identity_id: selected.owner.sponsor_identity_id,
        action_ref: fixture.action.r#ref.clone(),
        attempt: selected.attempt,
    };
    Ok(attune_common::auth::jwt::generate_admitted_execution_token(
        &credential,
        &[WRITER.into()],
        &ctx.state.jwt_config,
        Some(600),
    )?)
}

#[tokio::test]
async fn recorded_and_delegated_cache_authority_agree_on_canonical_identity() -> Result<()> {
    let ctx = fixture_context().await?;
    let result = authority_paths(&ctx).await;
    ctx.cleanup().await?;
    result
}

async fn authority_paths(ctx: &TestContext) -> Result<()> {
    let fixture = publish_fixture(ctx).await?;
    let recorded = recorded_token(ctx, &fixture).await?;
    let (_, delegated) = delegated(ctx, &fixture, &[WRITER]).await?;
    collision_namespace(ctx, &fixture).await?;
    for token in [&recorded, &delegated] {
        ensure!(namespace_names(ctx, token).await? == ["cache_auth_repro.control", "repro.users"]);
        begin(
            ctx,
            token,
            "cache_auth_repro.users",
            None,
            0,
            StatusCode::FORBIDDEN,
        )
        .await?;
        begin(ctx, token, "repro.users", None, 0, StatusCode::CREATED).await?;
    }
    Ok(())
}

#[tokio::test]
async fn worker_process_runtime_can_publish_with_the_unchanged_writer_grant() -> Result<()> {
    let ctx = fixture_context().await?;
    let result = worker_probe(&ctx).await;
    ctx.cleanup().await?;
    result
}

async fn worker_probe(ctx: &TestContext) -> Result<()> {
    use attune_worker::runtime::{
        ExecutionContext, OutputFormat, ParameterDelivery, ParameterFormat, ProcessRuntime, Runtime,
    };
    use std::collections::HashMap;
    let fixture = publish_fixture(ctx).await?;
    let (execution, token) = delegated(ctx, &fixture, &[WRITER]).await?;
    let runtime = attune_common::repositories::runtime::RuntimeRepository::find_by_ref(
        &ctx.pool,
        "cache_auth_repro.python",
    )
    .await?
    .context("probe runtime missing")?;
    let runtime = ProcessRuntime::new(
        "cache-auth-python".into(),
        serde_json::from_value(runtime.execution_config)?,
        ctx.test_packs_dir.clone(),
        ctx.test_packs_dir.join("runtime_envs"),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let router = ctx.app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let result = runtime
        .execute(ExecutionContext {
            native_invocation: None,
            execution_id: execution.id,
            action_ref: fixture.action.r#ref,
            parameters: HashMap::from([("namespace".into(), json!("repro.users"))]),
            env: HashMap::from([
                ("ATTUNE_API_URL".into(), format!("http://{address}")),
                ("ATTUNE_API_TOKEN".into(), token),
            ]),
            execution_env: HashMap::new(),
            secrets: HashMap::new(),
            timeout: Some(30),
            working_dir: Some(ctx.test_packs_dir.join(PACK)),
            entry_point: "probe.py".into(),
            code: None,
            code_path: None,
            runtime_name: Some("cache-auth-python".into()),
            runtime_config_override: None,
            runtime_env_dir_suffix: None,
            selected_runtime_version: None,
            max_stdout_bytes: 4096,
            max_stderr_bytes: 4096,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer: None,
            stderr_log_writer: None,
            parameter_delivery: ParameterDelivery::Stdin,
            parameter_format: ParameterFormat::Json,
            output_format: OutputFormat::Json,
            output_policy: Default::default(),
            out_schema: None,
            cancel_token: None,
        })
        .await;
    let _ = stop.send(());
    tokio::time::timeout(std::time::Duration::from_secs(10), server).await???;
    let result = result?;
    ensure!(
        result.is_success(),
        "worker probe failed: exit={}, stderr={}",
        result.exit_code,
        result.stderr
    );
    let body: Value = serde_json::from_str(&result.stdout)?;
    ensure!(
        body["http_status"] == 201 && body["namespace"] == "repro.users",
        "worker probe: {body}"
    );
    Ok(())
}

#[tokio::test]
async fn signed_sensor_reader_uses_managed_declaration_refs_without_standard() -> Result<()> {
    let ctx = fixture_context().await?;
    let result = sensor_reader(&ctx).await;
    ctx.cleanup().await?;
    result
}

async fn sensor_reader(ctx: &TestContext) -> Result<()> {
    use attune_common::{
        auth::jwt::generate_worker_token_with_instance,
        models::{ActionReferenceVisibility, WorkerStatus, WorkerType},
        repositories::{
            component_lifecycle::PackProjectionIds,
            runtime::{CreateWorkerInput, RuntimeRepository, WorkerRepository},
            sensor_workload::{
                AcquireSensorWorkloadInput, AcquireSensorWorkloadOutcome, SensorWorkloadRepository,
            },
            trigger::{CreateSensorInput, CreateTriggerInput, SensorRepository, TriggerRepository},
        },
    };
    publish_fixture(ctx).await?;
    let pack_ref = "sensor_cache_refs";
    let sensor_pack = helpers::create_test_pack(&ctx.pool, pack_ref).await?;
    let owner = CacheOwnerScope::pack(sensor_pack.id, Some(pack_ref.into()));
    CacheNamespaceRepository::upsert_managed_definitions(
        &ctx.pool,
        sensor_pack.id,
        pack_ref,
        &[
            ManagedCacheNamespaceDefinition {
                mode: Default::default(),
                definition_ref: format!("{pack_ref}.users"),
                namespace: "repro.users".into(),
                owner: owner.clone(),
                policy: CacheNamespacePolicy::default(),
            },
            ManagedCacheNamespaceDefinition {
                mode: Default::default(),
                definition_ref: format!("{pack_ref}.collision"),
                namespace: format!("{pack_ref}.users"),
                owner,
                policy: CacheNamespacePolicy::default(),
            },
        ],
        &CacheAdmissionConfig::default(),
    )
    .await?;
    let reader = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref:format!("{pack_ref}.sensor_reader"), pack:Some(sensor_pack.id), pack_ref:Some(pack_ref.into()),
        label:None, description:None,
        grants:json!([{"resource":"caches","actions":["read"],
            "constraints":{"owner_types":["pack"],"owner_refs":[pack_ref],"refs":["sensor_cache_refs.users"]}}]),
    }).await?;
    let runtime = RuntimeRepository::find_by_ref(&ctx.pool, "cache_auth_repro.python")
        .await?
        .context("sensor runtime missing")?;
    let sensor_ref = format!("{pack_ref}.cache_reader");
    let sensor = SensorRepository::create(
        &ctx.pool,
        CreateSensorInput {
            r#ref: sensor_ref.clone(),
            pack: Some(sensor_pack.id),
            pack_ref: Some(pack_ref.into()),
            label: "Synthetic cache reader".into(),
            description: None,
            entrypoint: "cache_reader.py".into(),
            runtime: runtime.id,
            runtime_ref: runtime.r#ref,
            runtime_version_constraint: None,
            enabled: true,
            param_schema: None,
            config: Some(json!({"cache_permission_set_refs":[reader.r#ref]})),
            worker_selector: json!({}),
            worker_tolerations: json!({}),
            worker_affinity: json!({}),
            log_retention_policy: None,
            log_retention_limit: None,
            artifact_retention_policy: None,
            artifact_retention_limit: None,
        },
    )
    .await?;
    let trigger = TriggerRepository::create(
        &ctx.pool,
        CreateTriggerInput {
            r#ref: format!("{pack_ref}.cache_probe"),
            pack: Some(sensor_pack.id),
            pack_ref: Some(pack_ref.into()),
            label: "Synthetic sensor trigger".into(),
            description: None,
            enabled: true,
            param_schema: None,
            event_config_schema: None,
            out_schema: None,
            sensor: Some(sensor.id),
            sensor_ref: Some(sensor_ref.clone()),
            is_adhoc: false,
            reference_visibility: ActionReferenceVisibility::Private,
            reference_allowed_pack_refs: vec![],
        },
    )
    .await?;
    helpers::activate_test_pack_release_with_projections(
        &ctx.pool,
        &sensor_pack,
        &PackProjectionIds {
            actions: vec![],
            runtimes: vec![],
            sensors: vec![sensor.id],
            triggers: vec![trigger.id],
            ..Default::default()
        },
    )
    .await?;
    let worker = WorkerRepository::create(
        &ctx.pool,
        CreateWorkerInput {
            name: format!("cache-sensor-{}", uuid::Uuid::new_v4()),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: Some(json!({})),
            meta: None,
        },
    )
    .await?;
    let instance = uuid::Uuid::new_v4();
    let workload = match SensorWorkloadRepository::acquire_or_renew(
        &ctx.pool,
        AcquireSensorWorkloadInput {
            sensor_id: sensor.id,
            worker_id: worker.id,
            worker_instance: instance,
            lease_seconds: 300,
        },
    )
    .await?
    {
        AcquireSensorWorkloadOutcome::Acquired(workload) => workload,
        AcquireSensorWorkloadOutcome::HeldByOther(_) => {
            return Err("fixture sensor lease already held".into())
        }
    };
    let workload = SensorWorkloadRepository::begin_process(&ctx.pool, workload)
        .await?
        .context("sensor generation unavailable")?;
    let mut tx = ctx.pool.begin().await?;
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(&mut tx).await?;
    attune_common::repositories::sensor_schema::SensorSchemaRepository::admit(
        &mut tx,
        &workload,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    tx.commit().await?;
    let worker_token = generate_worker_token_with_instance(
        1,
        &worker.id.to_string(),
        instance,
        &ctx.state.jwt_config,
        None,
    )?;
    let response = ctx.post("/auth/internal/sensor-token", json!({
        "sensor_ref":sensor_ref,"pack_ref":pack_ref,"trigger_types":[trigger.r#ref],
        "permission_set_refs":[reader.r#ref],"workload_id":workload.workload_id,
        "assignment_generation":workload.generation,"worker_instance":instance,"ttl_seconds":3600,
    }),Some(&worker_token)).await?;
    let status = response.status();
    // This response contains the signed credential: inspect it in memory only.
    ensure!(
        status == StatusCode::OK,
        "sensor token mint failed: {status}"
    );
    let body: Value = response.json().await?;
    let token = body["data"]["token"]
        .as_str()
        .context("sensor token missing")?;
    ensure!(namespace_names(ctx, token).await? == ["repro.users"]);
    for (namespace, status) in [
        ("repro.users", StatusCode::OK),
        ("sensor_cache_refs.users", StatusCode::FORBIDDEN),
    ] {
        let response = ctx
            .get(
                &format!(
                    "/api/v1/cache/namespaces/{namespace}?owner_type=pack&owner_ref={pack_ref}"
                ),
                Some(token),
            )
            .await?;
        ensure!(
            response.status() == status,
            "sensor read {namespace}: {}",
            response.status()
        );
    }
    let response = ctx
        .post(
            "/api/v1/cache/namespaces/repro.users/generations",
            json!({"owner_type":"pack","owner_ref":pack_ref,"client_refresh_id":"read-only-sensor",
            "expected_active_generation_id":null,"expected_chunk_count":0}),
            Some(token),
        )
        .await?;
    ensure!(response.status() == StatusCode::FORBIDDEN);
    Ok(())
}
