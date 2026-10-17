//! Source acceptance fixture for the central PG18 lane. No service deployment.
//! Exercises the same admission service called by normal upload/core bootstrap.
mod helpers;

use tower::ServiceExt;

use attune_common::{
    delegation::DelegationAuthority,
    pack_registry::{deployment_admission::DeploymentAdmission, PackComponentLoader},
    repositories::{
        component_configuration::ComponentConfigurationRepository,
        identity::{CreateIdentityInput, IdentityRepository, UpdateIdentityInput},
        pack::{PackRepository, UpdatePackInput},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        platform_catalog::PlatformCatalogRepository,
        runtime_authority::RuntimeAuthorityRepository,
        Create, FindById, FindByRef, Update,
    },
};
use helpers::*;
use serde_json::json;

const ACCEPTANCE_QUEUE: &str = "\
ref: core.acceptance_queue
label: Actual core installer acceptance queue
description: Supplementary queue dispatching the shipped canonical installer
enabled: true
accepting_new_items: true
dispatch_action: core.install_packs
permission_set_refs: [core.executor]
item_schema: {}
action_params:
  packs: ['https://example.invalid/acceptance.tar.gz']
config: {}
";

#[test]
fn supplementary_actual_core_queue_satisfies_canonical_metadata_and_schema_contract() -> Result<()>
{
    use attune_common::queue_definition::{
        parse_work_queue_definition_yaml, validate_work_queue_definition,
    };
    let definition = parse_work_queue_definition_yaml(ACCEPTANCE_QUEUE)?;
    validate_work_queue_definition(&definition)?;
    assert_eq!(definition.r#ref, "core.acceptance_queue");
    assert_eq!(definition.dispatch_action, "core.install_packs");
    assert_eq!(
        definition.permission_set_refs,
        Some(vec!["core.executor".into()])
    );
    assert_eq!(
        definition.action_params["packs"].as_array().unwrap().len(),
        1
    );
    let missing_label =
        ACCEPTANCE_QUEUE.replace("label: Actual core installer acceptance queue\n", "");
    assert!(parse_work_queue_definition_yaml(&missing_label).is_err());
    Ok(())
}

/// Copy the real shipped tree through an owned archive. Never synthesize native
/// executables. The shipped launcher is enough for metadata capture; executing
/// its timer requires the separately built, packaged binary and is NOT covered.
fn actual_core_tree(destination: &std::path::Path) -> Result<std::path::PathBuf> {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/core");
    let mut archive = tar::Builder::new(Vec::new());
    archive.append_dir_all("core", source)?;
    let bytes = archive.into_inner()?;
    tar::Archive::new(std::io::Cursor::new(bytes)).unpack(destination)?;
    Ok(destination.join("core"))
}

async fn reserved_policy_bodies(pool: &sqlx::PgPool) -> Result<serde_json::Value> {
    use attune_common::repositories::identity::PermissionSetRepository;
    let mut bodies = serde_json::Map::new();
    for reference in ["core.admin", "core.editor", "core.executor", "core.viewer"] {
        let policy = PermissionSetRepository::find_by_ref(pool, reference)
            .await?
            .unwrap();
        assert!(
            policy.pack.is_none(),
            "catalog policy must remain platform-owned"
        );
        bodies.insert(reference.into(), serde_json::to_value(policy)?);
    }
    Ok(bodies.into())
}

/// Read the actual archive's platform definition, not a hand-written runtime
/// ceiling. Exact legacy catalog bodies remain platform-owned at registration.
fn actual_core_global_ceiling(
    source: &std::path::Path,
    name: &str,
) -> Result<Vec<attune_common::rbac::Grant>> {
    let body: serde_json::Value = serde_yaml_ng::from_str(&std::fs::read_to_string(
        source.join("permission_sets").join(format!("{name}.yaml")),
    )?)?;
    assert_eq!(body["ref"], format!("core.{name}"));
    let catalog = match name {
        "executor" => {
            include_str!("../../common/src/platform_catalog/permission_sets/executor.yaml")
        }
        "viewer" => include_str!("../../common/src/platform_catalog/permission_sets/viewer.yaml"),
        _ => panic!("Unsupported actual-core fixture ceiling"),
    };
    assert_eq!(body, serde_yaml_ng::from_str::<serde_json::Value>(catalog)?);
    Ok(serde_json::from_value(body["grants"].clone())?)
}

#[test]
fn actual_core_global_approval_ceiling_is_shipped_catalog_not_a_synthetic_runtime_policy(
) -> Result<()> {
    use attune_common::rbac::{Action, Resource};
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/core");
    let executor = actual_core_global_ceiling(&source, "executor")?;
    let viewer = actual_core_global_ceiling(&source, "viewer")?;
    assert!(executor.iter().any(
        |grant| grant.resource == Resource::Actions && grant.actions.contains(&Action::Execute)
    ));
    assert!(!viewer
        .iter()
        .any(|grant| grant.actions.contains(&Action::Execute)));
    assert!(executor.iter().all(|grant| grant.constraints.is_none()));
    Ok(())
}

async fn explicitly_approve_core_ceiling(
    ctx: &TestContext,
    grants: &[attune_common::rbac::Grant],
) -> Result<attune_api::dto::runtime_approval::RuntimeApprovalResponse> {
    let response = ctx
        .post(
            "/api/v1/packs/core/external-approvals",
            json!({"grants": grants}),
            ctx.token(),
        )
        .await?;
    let status = response.status();
    let body: serde_json::Value = response.json().await?;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{body}");
    let approval: attune_api::dto::runtime_approval::RuntimeApprovalResponse =
        serde_json::from_value(body["data"].clone())?;
    assert_eq!(approval.issuer_identity_id, ctx.user.as_ref().unwrap().id);
    assert_eq!(approval.grants.as_slice(), grants);
    assert!(approval.active);
    assert!(approval.revoked_at.is_none());
    assert!(!approval.approval_digest.is_empty());
    Ok(approval)
}

async fn stage_actual_core(
    ctx: &TestContext,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    pack_id: i64,
    installer_id: i64,
    source: &std::path::Path,
) -> Result<i64> {
    use attune_common::{models::AbsentMetadataPolicy, pack_registry::PackStorage};
    let metadata: serde_json::Value =
        serde_yaml_ng::from_str(&std::fs::read_to_string(source.join("pack.yaml"))?)?;
    let version = metadata["version"].as_str().unwrap();
    let storage = PackStorage::new(ctx.test_packs_dir.join("actual-core-releases"));
    let published = storage.publish_release(source, "core", version)?;
    assert_eq!(
        attune_common::pack_registry::calculate_file_checksum(&published.archive_path)?,
        published.digest
    );
    use attune_common::blob_store::{body_from_file, hash_file, BlobStoreError, ObjectKey};
    let (archive_size, archive_digest) = hash_file(&published.archive_path).await?;
    let object_key = ObjectKey::new(format!("packs/blobs/sha256/{}.tar.gz", published.digest))?;
    let stored = match ctx
        .state
        .blob_store
        .put(
            &object_key,
            body_from_file(&published.archive_path).await?,
            archive_digest,
        )
        .await
    {
        Ok(stored) => stored,
        Err(BlobStoreError::Conflict) => ctx.state.blob_store.head(&object_key).await?.unwrap(),
        Err(error) => return Err(error.into()),
    };
    assert_eq!(stored.sha256, archive_digest);
    assert_eq!(stored.size, archive_size);
    DeploymentAdmission::lock(tx).await?;
    DeploymentAdmission::preserve(
        tx,
        pack_id,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    let installer = DelegationAuthority::load_for_share(tx, installer_id).await?;
    let admission = DeploymentAdmission::read(&published.pack_path, "core")?;
    PackRepository::update_for_deployment(
        tx,
        pack_id,
        UpdatePackInput {
            version: Some(version.into()),
            conf_schema: Some(metadata["conf_schema"].clone()),
            label: metadata["label"].as_str().map(str::to_owned),
            meta: Some(metadata["meta"].clone()),
            tags: Some(serde_json::from_value(metadata["tags"].clone())?),
            runtime_deps: Some(serde_json::from_value(metadata["runtime_deps"].clone())?),
            ..Default::default()
        },
        &published.pack_path,
        AbsentMetadataPolicy::Remove,
    )
    .await?;
    let release = PackReleaseRepository::create_or_get(
        tx,
        CreatePackReleaseInput {
            pack: pack_id,
            pack_ref: "core".into(),
            version: version.into(),
            digest: published.digest.clone(),
            object_key: object_key.as_str().into(),
            provider_version: stored.provider_version.as_stored().into(),
            content_path: published.pack_path.to_string_lossy().into_owned(),
            archive_size: i64::try_from(stored.size)?,
            manifest: serde_json::to_value(published.manifest)?,
        },
    )
    .await?;
    let loaded = admission
        .load(
            PackComponentLoader::new(
                &ctx.pool,
                pack_id,
                "core",
                &ctx.state.config.cache_admission,
                installer.clone(),
            ),
            tx,
        )
        .await?;
    assert_eq!(loaded.total_skipped(), 0, "{:?}", loaded.warnings);
    use attune_common::repositories::sensor_admission::{
        SensorAdmissionRepository, SensorAdmissionRequirement,
    };
    assert!(SensorAdmissionRepository::assess_pack(
        tx,
        pack_id,
        SensorAdmissionRequirement::Structural,
    )
    .await?
    .is_empty());
    PackReleaseRepository::activate_projected(tx, pack_id, release.id, &loaded.projections).await?;
    admission
        .finish(
            tx,
            pack_id,
            release.id,
            ctx.state.config.security.encryption_key.as_deref(),
            &installer,
        )
        .await?;
    Ok(release.id)
}

#[tokio::test]
async fn actual_core_archive_admission_preserves_platform_roles_defaults_and_exact_rollback_pins(
) -> Result<()> {
    use attune_common::{
        repositories::{
            action::ActionRepository,
            managed_executable::ManagedExecutableRepository,
            managed_sensor_metadata::ManagedSensorMetadataRepository,
            trigger::{SensorRepository, TriggerRepository},
        },
        runtime_authority::InvocationBinding,
    };
    let ctx = TestContext::new().await?.with_auth().await?;
    PlatformCatalogRepository::reconcile(&ctx.pool).await?;
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, PermissionAssignmentRepository, PermissionSetRepository,
    };
    // A real authorized approval actor, separate from the unprivileged registrar.
    // Role assignment is not consent: the explicit public POST below records it.
    let issuer = ctx.user.as_ref().unwrap();
    let administrator = PermissionSetRepository::find_by_ref(&ctx.pool, "core.admin")
        .await?
        .unwrap();
    assert!(administrator.pack.is_none());
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: issuer.id,
            permset: administrator.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(issuer.id).await;
    let catalog = reserved_policy_bodies(&ctx.pool).await?;
    let installer = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "actual-core-deployer".into(),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await?;
    assert!(!DelegationAuthority::load(&ctx.pool, installer.id)
        .await?
        .manages_permissions());
    let pack = create_test_pack(&ctx.pool, "core").await?;
    let source = actual_core_tree(&ctx.test_packs_dir.join("actual-core-source"))?;
    let ceiling = actual_core_global_ceiling(&source, "executor")?;
    assert_eq!(
        PermissionSetRepository::find_by_ref(&ctx.pool, "core.executor")
            .await?
            .unwrap()
            .grants,
        serde_json::to_value(&ceiling)?
    );
    // Pre-provision fixed, explicit consent for the shipped global ceiling. The
    // unmodified core actions select only local key_creator; the supplementary
    // queue below deliberately selects this real platform definition at upgrade.
    let initial_approval = explicitly_approve_core_ceiling(&ctx, &ceiling).await?;
    assert_eq!(initial_approval.subject_pack_id, pack.id);
    let mut fresh = ctx.pool.begin().await?;
    let release_id = stage_actual_core(&ctx, &mut fresh, pack.id, installer.id, &source).await?;
    let policy = RuntimeAuthorityRepository::current_policy(&mut fresh, pack.id)
        .await?
        .unwrap();
    let original_policy_body = serde_json::to_value(&policy.policy)?;
    for reference in [
        "core.echo",
        "core.noop",
        "core.sleep",
        "core.http_request",
        "core.generate_ssh_key_pair",
        "core.run_agent_command",
        "core.download_packs",
        "core.register_packs",
        "core.get_pack_dependencies",
        "core.build_pack_envs",
        "core.install_packs",
    ] {
        assert_eq!(
            ActionRepository::find_by_ref(&mut *fresh, reference)
                .await?
                .unwrap()
                .pack,
            pack.id
        );
        assert!(policy.policy.invocations.iter().any(|entry| entry.binding
            == InvocationBinding::Action {
                action_ref: reference.into()
            }));
    }
    for (index, action_ref) in [
        "core.noop",
        "core.download_packs",
        "core.noop",
        "core.get_pack_dependencies",
        "core.noop",
        "core.build_pack_envs",
        "core.register_packs",
        "core.noop",
        "core.download_packs",
        "core.get_pack_dependencies",
        "core.build_pack_envs",
        "core.register_packs",
    ]
    .into_iter()
    .enumerate()
    {
        assert!(policy.policy.invocations.iter().any(|entry| entry.binding
            == InvocationBinding::WorkflowTask {
                workflow_ref: "core.install_packs".into(),
                task_path: format!("/tasks/{index}"),
                action_ref: action_ref.into(),
            }));
    }
    let installer_binding = InvocationBinding::Action {
        action_ref: "core.install_packs".into(),
    };
    let workflow_executable = ManagedExecutableRepository::resolve_invocation(
        &mut fresh,
        &policy.policy,
        &installer_binding,
    )
    .await?;
    assert_eq!(workflow_executable.release.id, release_id);
    assert!(workflow_executable.executable.runtime.is_none());
    assert!(workflow_executable.executable.workflow_definition.is_some());
    for reference in [
        "core.intervaltimer",
        "core.crontimer",
        "core.datetimetimer",
        "core.rruletimer",
        "core.queue_empty",
        "core.queue_started",
        "core.alert",
    ] {
        assert!(
            TriggerRepository::find_by_ref(&mut *fresh, reference)
                .await?
                .is_some(),
            "{reference}"
        );
    }
    let sensor = SensorRepository::find_by_ref(&mut *fresh, "core.timer_sensor")
        .await?
        .unwrap();
    let sensor_revision = ManagedSensorMetadataRepository::current_revision(&mut *fresh, sensor.id)
        .await?
        .unwrap();
    let binding = InvocationBinding::Action {
        action_ref: "core.generate_ssh_key_pair".into(),
    };
    let executable =
        ManagedExecutableRepository::resolve_invocation(&mut fresh, &policy.policy, &binding)
            .await?;
    use attune_common::{
        repositories::{
            execution::{CreateExecutionInput, ExecutionRepository},
            runtime_authority::{RuntimeCredentialBinding, RuntimeOwner},
        },
        runtime_authority::{HumanAttribution, RequestAttribution},
    };
    let selected = policy.policy.select(
        policy.id,
        &binding,
        &["core.key_creator".into()],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: installer.id,
                login: installer.login.clone(),
            },
        },
    )?;
    let execution = ExecutionRepository::create_pinned(
        &mut *fresh,
        CreateExecutionInput {
            action: Some(executable.executable.action.id),
            action_ref: "core.generate_ssh_key_pair".into(),
            executor: Some(installer.id),
            permission_set_refs: vec!["core.key_creator".into()],
            status: attune_common::models::ExecutionStatus::Running,
            config: Some(json!({"local_ref":"acceptance-only"})),
            ..Default::default()
        },
        &executable,
    )
    .await?;
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut fresh,
        RuntimeOwner::Execution(execution.id),
        &selected,
    )
    .await?;
    let credential = RuntimeCredentialBinding {
        execution_id: execution.id,
        admission_id: admission.id,
        sponsor_identity_id: admission.authority.owner.sponsor_identity_id,
        action_ref: "core.generate_ssh_key_pair".into(),
        attempt: admission.authority.attempt,
    };
    fresh.commit().await?;
    let original_release = PackReleaseRepository::find_by_id(&ctx.pool, release_id)
        .await?
        .unwrap();
    assert_eq!(executable.release.id, release_id);
    assert_eq!(executable.release.digest, original_release.digest);
    assert_eq!(reserved_policy_bodies(&ctx.pool).await?, catalog);
    assert_eq!(
        PackRepository::find_by_id(&ctx.pool, pack.id)
            .await?
            .unwrap()
            .config["max_action_timeout"],
        300
    );
    let mut overrides = ctx.pool.begin().await?;
    DeploymentAdmission::lock(&mut overrides).await?;
    let current = PackRepository::find_by_id(&mut *overrides, pack.id)
        .await?
        .unwrap();
    let configuration = ComponentConfigurationRepository::bind_pack(
        &mut overrides,
        &current,
        Some(&json!({"max_action_timeout":77})),
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    overrides.commit().await?;

    // Core intentionally ships no queue. Only the queue is supplementary; it
    // dispatches the actual shipped canonical installer workflow.
    std::fs::create_dir_all(source.join("queues"))?;
    std::fs::write(source.join("queues/00_acceptance.yaml"), ACCEPTANCE_QUEUE)?;
    let mut metadata: serde_json::Value =
        serde_yaml_ng::from_str(&std::fs::read_to_string(source.join("pack.yaml"))?)?;
    metadata["version"] = json!("1.0.5");
    metadata["config"]["max_action_timeout"] = json!(600);
    metadata["config"]["enable_debug_logging"] = json!(true);
    std::fs::write(
        source.join("pack.yaml"),
        serde_yaml_ng::to_string(&metadata)?,
    )?;
    // Revoke only the test's explicit pre-provisioned approval. The original
    // shipped policy is local and remains pinned; queue dispatch now requests
    // the actual core.executor global ceiling, never a synthetic action/policy.
    let revoked = ctx
        .post(
            &format!(
                "/api/v1/packs/core/external-approvals/{}/revoke",
                initial_approval.id
            ),
            json!({}),
            ctx.token(),
        )
        .await?;
    assert_eq!(revoked.status(), axum::http::StatusCode::OK);
    let revoked: serde_json::Value = revoked.json().await?;
    assert_eq!(revoked["data"]["active"], false);
    assert_eq!(
        revoked["data"]["approval_digest"],
        initial_approval.approval_digest
    );
    let mut mismatched_approval = None;
    for with_insufficient_ceiling in [false, true] {
        if with_insufficient_ceiling {
            // Another real shipped catalog definition permits inspection but
            // not actions:execute. Consent to viewer is not consent to executor.
            let viewer = actual_core_global_ceiling(&source, "viewer")?;
            assert!(!viewer.iter().any(|grant| grant
                .actions
                .contains(&attune_common::rbac::Action::Execute)));
            mismatched_approval = Some(explicitly_approve_core_ceiling(&ctx, &viewer).await?);
        }
        let mut failed = ctx.pool.begin().await?;
        let denial = stage_actual_core(&ctx, &mut failed, pack.id, installer.id, &source)
            .await
            .expect_err("missing or viewer-only approval must not admit executor's global ceiling");
        assert!(
            matches!(
                denial.downcast_ref::<attune_common::Error>(),
                Some(attune_common::Error::PermissionDenied(_))
            ),
            "{denial}"
        );
        assert!(
            denial
                .to_string()
                .contains("External runtime scope requires separate approval"),
            "{denial}"
        );
        assert_ne!(PackRepository::find_by_id(&mut *failed, pack.id).await?.unwrap().active_release, Some(release_id), "the actual global ceiling must fail final admission after projection, not an unrelated parse");
        failed.rollback().await?;
        let retained_release = PackReleaseRepository::find_active_by_pack_ref(&ctx.pool, "core")
            .await?
            .unwrap();
        assert_eq!(retained_release.id, release_id);
        assert_eq!(retained_release.digest, original_release.digest);
        assert_eq!(retained_release.content_path, original_release.content_path);
        assert_eq!(retained_release.object_key, original_release.object_key);
        assert_eq!(
            retained_release.provider_version,
            original_release.provider_version
        );
        assert_eq!(
            ComponentConfigurationRepository::current(&ctx.pool, "pack", pack.id)
                .await?
                .unwrap()
                .id,
            configuration.id
        );
        assert_eq!(
            ManagedSensorMetadataRepository::current_revision(&ctx.pool, sensor.id).await?,
            Some(sensor_revision)
        );
        let mut unchanged = ctx.pool.begin().await?;
        let retained_policy = RuntimeAuthorityRepository::current_policy(&mut unchanged, pack.id)
            .await?
            .unwrap();
        assert_eq!(retained_policy.id, policy.id);
        assert_eq!(
            serde_json::to_value(&retained_policy.policy)?,
            original_policy_body
        );
        unchanged.rollback().await?;
        assert_eq!(
            PackReleaseRepository::list_by_pack(&ctx.pool, pack.id)
                .await?
                .len(),
            1
        );
        assert!(
            attune_common::repositories::work_queue::WorkQueueRepository::find_by_ref(
                &ctx.pool,
                "core.acceptance_queue"
            )
            .await?
            .is_none()
        );
        assert_eq!(
            PackRepository::find_by_id(&ctx.pool, pack.id)
                .await?
                .unwrap()
                .config,
            configuration.config
        );
        assert_eq!(reserved_policy_bodies(&ctx.pool).await?, catalog);
    }
    // Explicit public consent is separate from registrar or administrator role
    // assignment, and covers the exact grants read from the actual core archive.
    let covering_approval = explicitly_approve_core_ceiling(&ctx, &ceiling).await?;
    assert_ne!(covering_approval.id, initial_approval.id);
    assert_ne!(
        covering_approval.id,
        mismatched_approval.as_ref().unwrap().id
    );
    let mut upgrade = ctx.pool.begin().await?;
    DeploymentAdmission::lock(&mut upgrade).await?;
    let new_release = stage_actual_core(&ctx, &mut upgrade, pack.id, installer.id, &source).await?;
    let new_policy = RuntimeAuthorityRepository::current_policy(&mut upgrade, pack.id)
        .await?
        .unwrap();
    assert_eq!(new_policy.policy.release_id, new_release);
    let queued = new_policy
        .policy
        .invocations
        .iter()
        .find(|entry| {
            entry.binding
                == InvocationBinding::Queue {
                    queue_ref: "core.acceptance_queue".into(),
                    action_ref: "core.install_packs".into(),
                }
        })
        .unwrap();
    let admitted = &queued.permission_sets["core.executor"];
    assert_eq!(
        admitted
            .grants
            .iter()
            .map(|grant| grant.grant().clone())
            .collect::<Vec<_>>(),
        ceiling
    );
    assert!(admitted
        .grants
        .iter()
        .all(|grant| grant.approval_id().is_some()));
    assert!(admitted.grants.iter().any(|grant| grant
        .grant()
        .actions
        .contains(&attune_common::rbac::Action::Execute)));
    assert!(admitted
        .grants
        .iter()
        .filter(|grant| grant
            .grant()
            .actions
            .contains(&attune_common::rbac::Action::Execute))
        .all(|grant| grant.approval_id() == Some(covering_approval.id)));
    // Admission consumes fixed approval records; it must never rewrite consent.
    let recorded = RuntimeAuthorityRepository::find_approval(&mut upgrade, covering_approval.id)
        .await?
        .unwrap();
    assert_eq!(recorded.approval_digest, covering_approval.approval_digest);
    assert_eq!(recorded.grants.0, covering_approval.grants);
    assert_eq!(recorded.issuer_identity, issuer.id);
    assert_eq!(recorded.subject_pack, pack.id);
    assert_ne!(new_release, release_id);
    assert!(new_policy
        .policy
        .invocations
        .iter()
        .any(|entry| entry.binding
            == InvocationBinding::Queue {
                queue_ref: "core.acceptance_queue".into(),
                action_ref: "core.install_packs".into(),
            }));
    upgrade.commit().await?;
    let current = PackRepository::find_by_id(&ctx.pool, pack.id)
        .await?
        .unwrap();
    assert_eq!(current.config["max_action_timeout"], 77);
    assert_eq!(current.config["enable_debug_logging"], true);
    let upgraded_config = ComponentConfigurationRepository::current(&ctx.pool, "pack", pack.id)
        .await?
        .unwrap();
    assert_ne!(upgraded_config.id, configuration.id);
    assert_eq!(upgraded_config.config, current.config);
    IdentityRepository::update(
        &ctx.pool,
        issuer.id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let mut approved = ctx.pool.begin().await?;
    RuntimeAuthorityRepository::require_deployment_policy(&mut approved, new_policy.id).await?;
    let after_freeze =
        RuntimeAuthorityRepository::find_approval(&mut approved, covering_approval.id)
            .await?
            .unwrap();
    assert_eq!(
        after_freeze.approval_digest,
        covering_approval.approval_digest
    );
    assert_eq!(after_freeze.grants.0, covering_approval.grants);
    assert!(after_freeze.is_active(chrono::Utc::now()));
    approved.rollback().await?;

    // Explicit rollback is a new admission of the retained immutable code, not
    // a blind pointer rewind. Original defaults must replace upgrade defaults
    // while the operator layer and already admitted execution stay pinned.
    let mut rollback = ctx.pool.begin().await?;
    let restored_release = stage_actual_core(
        &ctx,
        &mut rollback,
        pack.id,
        installer.id,
        std::path::Path::new(&original_release.content_path),
    )
    .await?;
    assert_eq!(restored_release, release_id);
    let rollback_policy = RuntimeAuthorityRepository::current_policy(&mut rollback, pack.id)
        .await?
        .unwrap();
    assert_ne!(rollback_policy.id, new_policy.id);
    assert_eq!(rollback_policy.policy.release_id, release_id);
    rollback.commit().await?;
    let restored = PackRepository::find_by_id(&ctx.pool, pack.id)
        .await?
        .unwrap();
    assert_eq!(restored.active_release, Some(release_id));
    assert_eq!(restored.config["max_action_timeout"], 77);
    assert_eq!(restored.config["enable_debug_logging"], false);
    let rollback_config = ComponentConfigurationRepository::current(&ctx.pool, "pack", pack.id)
        .await?
        .unwrap();
    assert_eq!(rollback_config.config, restored.config);
    assert_ne!(rollback_config.id, upgraded_config.id);
    IdentityRepository::update(
        &ctx.pool,
        installer.id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let mut retained = ctx.pool.begin().await?;
    let original = RuntimeAuthorityRepository::require_policy(&mut retained, policy.id).await?;
    assert_eq!(original.policy.release_id, release_id);
    RuntimeAuthorityRepository::validate_execution_credential(&mut retained, &credential).await?;
    assert_eq!(
        serde_json::to_value(
            ManagedExecutableRepository::resolve_invocation(
                &mut retained,
                &original.policy,
                &installer_binding,
            )
            .await?
        )?,
        serde_json::to_value(workflow_executable)?
    );
    assert_eq!(
        serde_json::to_value(
            ManagedExecutableRepository::resolve_invocation(
                &mut retained,
                &original.policy,
                &binding
            )
            .await?
        )?,
        serde_json::to_value(executable)?
    );
    assert_eq!(
        RuntimeAuthorityRepository::current_policy(&mut retained, pack.id)
            .await?
            .unwrap()
            .id,
        rollback_policy.id
    );
    assert_eq!(
        serde_json::to_value(&original.policy)?,
        original_policy_body
    );
    assert_eq!(
        RuntimeAuthorityRepository::require_deployment_policy(&mut retained, new_policy.id)
            .await?
            .policy
            .release_id,
        new_release
    );
    retained.rollback().await?;
    assert_eq!(
        attune_common::pack_registry::calculate_file_checksum(
            std::path::Path::new(&original_release.content_path)
                .parent()
                .unwrap()
                .join("pack.tar.gz")
        )?,
        original_release.digest
    );
    assert_eq!(reserved_policy_bodies(&ctx.pool).await?, catalog);
    let ready = attune_common::repositories::HealthRepository::admitted_core(&ctx.pool)
        .await?
        .unwrap();
    assert_eq!(ready.release_id, release_id);
    assert_eq!(ready.policy_revision, rollback_policy.id);
    assert_eq!(ready.digest, original_release.digest);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn bootstrap_upload_checks_required_digest_before_replacing_active_release() -> Result<()> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, PermissionAssignmentRepository, PermissionSetRepository,
    };
    let ctx = TestContext::new().await?.with_auth().await?;
    PlatformCatalogRepository::reconcile(&ctx.pool).await?;
    // with_admin_auth creates an ad hoc core.admin, which must conflict with
    // canonical catalog ownership. Assign the reconciled platform role instead.
    let administrator = PermissionSetRepository::find_by_ref(&ctx.pool, "core.admin")
        .await?
        .unwrap();
    assert!(administrator.pack.is_none());
    let identity = ctx.user.as_ref().unwrap();
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: administrator.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    let pack = create_test_pack(&ctx.pool, "core").await?;
    activate_test_pack_release(&ctx.pool, &pack).await?;
    let original = PackReleaseRepository::find_active_by_pack_ref(&ctx.pool, "core")
        .await?
        .unwrap();
    let source = ctx.test_packs_dir.join("wrong-pin-candidate");
    std::fs::create_dir_all(&source)?;
    std::fs::write(source.join("pack.yaml"), "ref: core\nversion: '2.0.0'\n")?;
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(gzip);
    archive.append_dir_all(".", &source)?;
    let archive = archive.into_inner()?.finish()?;
    let boundary = "bootstrap-pin-fixture";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"force\"\r\n\r\ntrue\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"skip_tests\"\r\n\r\ntrue\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"expected_release_digest\"\r\n\r\n{}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"pack\"; filename=\"core.tar.gz\"\r\nContent-Type: application/gzip\r\n\r\n",
        "f".repeat(64),
    ).into_bytes();
    body.extend_from_slice(&archive);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/v1/packs/upload")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("authorization", format!("Bearer {}", ctx.token().unwrap()))
        .body(axum::body::Body::from(body))?;
    let response = helpers::TestResponse::new(ctx.app.clone().oneshot(request).await?);
    assert_eq!(
        response.status(),
        axum::http::StatusCode::CONFLICT,
        "{}",
        response.text().await?
    );
    assert_eq!(
        PackReleaseRepository::find_active_by_pack_ref(&ctx.pool, "core")
            .await?
            .unwrap()
            .id,
        original.id
    );
    assert_eq!(
        PackReleaseRepository::list_by_pack(&ctx.pool, pack.id)
            .await?
            .len(),
        1
    );
    ctx.cleanup().await?;
    Ok(())
}

fn release_source(
    root: &std::path::Path,
    name: &str,
    external: bool,
) -> Result<std::path::PathBuf> {
    let source = root.join(name);
    std::fs::create_dir_all(source.join("actions/workflows"))?;
    std::fs::create_dir_all(source.join("permission_sets"))?;
    std::fs::write(source.join("pack.yaml"), "ref: core\nversion: '1.0.0'\nconf_schema:\n  mode:\n    type: string\nconfig:\n  mode: authored\n")?;
    std::fs::write(
        source.join("permission_sets/effects.yaml"),
        format!(
            "ref: core.effects\nscope: {}\ngrants:\n  - resource: keys\n    actions: [read]\n",
            if external { "external" } else { "pack" }
        ),
    )?;
    std::fs::write(source.join("actions/leaf.yaml"), "ref: core.leaf\nrunner_type: shell\nentry_point: leaf.sh\ndefault_execution_permission_set_refs: [core.effects]\n")?;
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/core/actions/echo.sh"),
        source.join("actions/leaf.sh"),
    )?;
    std::fs::write(
        source.join("actions/flow.yaml"),
        "ref: core.flow\nworkflow_file: workflows/flow.workflow.yaml\n",
    )?;
    std::fs::write(
        source.join("actions/workflows/flow.workflow.yaml"),
        "version: '1.0'\ntasks:\n  named/step:\n    action: core.leaf\n",
    )?;
    Ok(source)
}

async fn stage(
    ctx: &TestContext,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    pack_id: i64,
    installer_id: i64,
    source: &std::path::Path,
    marker: char,
) -> Result<i64> {
    DeploymentAdmission::lock(tx).await?;
    DeploymentAdmission::preserve(
        tx,
        pack_id,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    let installer = DelegationAuthority::load_for_share(tx, installer_id).await?;
    let admission = DeploymentAdmission::read(source, "core")?;
    let release = PackReleaseRepository::create_or_get(
        tx,
        CreatePackReleaseInput {
            pack: pack_id,
            pack_ref: "core".into(),
            version: format!("1.0.{}", marker as u32),
            digest: marker.to_string().repeat(64),
            object_key: format!("packs/blobs/{marker}.tar.gz"),
            provider_version: format!("e:{marker}"),
            content_path: source.to_string_lossy().into_owned(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await?;
    let loader = PackComponentLoader::new(
        &ctx.pool,
        pack_id,
        "core",
        &ctx.state.config.cache_admission,
        installer.clone(),
    );
    let loaded = admission.load(loader, tx).await?;
    assert_eq!(loaded.total_skipped(), 0, "{:?}", loaded.warnings);
    PackReleaseRepository::activate_projected(tx, pack_id, release.id, &loaded.projections).await?;
    admission
        .finish(
            tx,
            pack_id,
            release.id,
            ctx.state.config.security.encryption_key.as_deref(),
            &installer,
        )
        .await?;
    Ok(release.id)
}

#[tokio::test]
async fn unified_bootstrap_admission_retains_pins_and_overrides_and_rolls_back_partial_plan(
) -> Result<()> {
    let ctx = TestContext::new().await?;
    PlatformCatalogRepository::reconcile(&ctx.pool).await?;
    // No Key/runtime grants, core.admin assignment, SQL identity bootstrap or
    // impersonation. Ingress separately requires the ordinary deployment rights.
    let installer = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "bootstrap-fixture-installer".into(),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await?;
    let authority = DelegationAuthority::load(&ctx.pool, installer.id).await?;
    assert!(!authority.manages_permissions());
    assert!(
        !authority.covers(&serde_json::from_value::<Vec<attune_common::rbac::Grant>>(
            json!([
                {"resource":"keys", "actions":["read"]}
            ])
        )?)
    );
    let pack = create_test_pack(&ctx.pool, "core").await?;
    PackRepository::update(
        &ctx.pool,
        pack.id,
        UpdatePackInput {
            conf_schema: Some(json!({"mode":{"type":"string"}})),
            ..Default::default()
        },
    )
    .await?;
    let original = release_source(&ctx.test_packs_dir, "admission-original", false)?;
    let mut tx = ctx.pool.begin().await?;
    let original_release = stage(&ctx, &mut tx, pack.id, installer.id, &original, 'a').await?;
    let original_policy = RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
        .await?
        .unwrap();
    assert!(!original_policy.policy.invocations.is_empty());
    use attune_common::{
        repositories::{
            execution::{CreateExecutionInput, ExecutionRepository},
            managed_executable::ManagedExecutableRepository,
            runtime_authority::{RuntimeCredentialBinding, RuntimeOwner},
        },
        runtime_authority::{HumanAttribution, InvocationBinding, RequestAttribution},
    };
    let binding = InvocationBinding::Action {
        action_ref: "core.leaf".into(),
    };
    let selected = original_policy.policy.select(
        original_policy.id,
        &binding,
        &["core.effects".into()],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: installer.id,
                login: installer.login.clone(),
            },
        },
    )?;
    let snapshot =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &original_policy.policy, &binding)
            .await?;
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: "core.leaf".into(),
            executor: Some(installer.id),
            permission_set_refs: vec!["core.effects".into()],
            status: attune_common::models::ExecutionStatus::Running,
            config: Some(json!({})),
            ..Default::default()
        },
        &snapshot,
    )
    .await?;
    let admitted = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(execution.id),
        &selected,
    )
    .await?;
    let credential = RuntimeCredentialBinding {
        execution_id: execution.id,
        admission_id: admitted.id,
        sponsor_identity_id: admitted.authority.owner.sponsor_identity_id,
        action_ref: "core.leaf".into(),
        attempt: admitted.authority.attempt,
    };
    tx.commit().await?;

    let mut config = ctx.pool.begin().await?;
    DeploymentAdmission::lock(&mut config).await?;
    let current_pack = PackRepository::find_by_id(&mut *config, pack.id)
        .await?
        .unwrap();
    let original_config = ComponentConfigurationRepository::bind_pack(
        &mut config,
        &current_pack,
        Some(&json!({"mode":"operator"})),
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    config.commit().await?;

    // Fail AFTER projection/release mutations: the final canonical compiler must
    // deny external authority without fixed approval. Only the outer rollback
    // restores the whole plan, including permission-set and component projections.
    let failed = release_source(&ctx.test_packs_dir, "admission-denied", true)?;
    let mut partial = ctx.pool.begin().await?;
    assert!(
        stage(&ctx, &mut partial, pack.id, installer.id, &failed, 'b')
            .await
            .is_err()
    );
    assert_ne!(
        PackRepository::find_by_id(&mut *partial, pack.id)
            .await?
            .unwrap()
            .active_release,
        Some(original_release)
    );
    partial.rollback().await?;
    let active = PackReleaseRepository::find_active_by_pack_ref(&ctx.pool, "core")
        .await?
        .unwrap();
    assert_eq!(active.id, original_release);
    let retained_config = ComponentConfigurationRepository::current(&ctx.pool, "pack", pack.id)
        .await?
        .unwrap();
    assert_eq!(retained_config.id, original_config.id);

    let replacement = release_source(&ctx.test_packs_dir, "admission-replacement", false)?;
    let mut upgrade = ctx.pool.begin().await?;
    let next_release = stage(&ctx, &mut upgrade, pack.id, installer.id, &replacement, 'c').await?;
    let next_policy = RuntimeAuthorityRepository::current_policy(&mut upgrade, pack.id)
        .await?
        .unwrap();
    assert_ne!(next_policy.id, original_policy.id);
    assert_ne!(next_release, original_release);
    upgrade.commit().await?;
    let projected = PackRepository::find_by_id(&ctx.pool, pack.id)
        .await?
        .unwrap();
    assert_eq!(projected.config["mode"], "operator");

    // Freezing the deployer cannot revoke the independent runtime sponsor or
    // rewrite either immutable policy/release, including an admitted old run.
    IdentityRepository::update(
        &ctx.pool,
        installer.id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let mut retained = ctx.pool.begin().await?;
    let old = RuntimeAuthorityRepository::require_policy(&mut retained, original_policy.id).await?;
    assert_eq!(old.policy.release_id, original_release);
    let current = RuntimeAuthorityRepository::current_policy(&mut retained, pack.id)
        .await?
        .unwrap();
    assert_eq!(current.id, next_policy.id);
    RuntimeAuthorityRepository::validate_execution_credential(&mut retained, &credential).await?;
    retained.rollback().await?;
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn bootstrap_fixed_approval_expiry_denies_readiness_and_replacement_without_mutating_old_pins(
) -> Result<()> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    use attune_common::repositories::HealthRepository;
    let ctx = TestContext::new().await?;
    PlatformCatalogRepository::reconcile(&ctx.pool).await?;
    let issuer = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "bootstrap-approval-issuer".into(),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await?;
    let administration = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "core.bootstrap_fixture_approver".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"permissions", "actions":["manage"]}]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: issuer.id,
            permset: administration.id,
        },
    )
    .await?;
    let installer = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: "bootstrap-expiry-deployer".into(),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await?;
    let pack = create_test_pack(&ctx.pool, "core").await?;
    PackRepository::update(
        &ctx.pool,
        pack.id,
        UpdatePackInput {
            conf_schema: Some(json!({"mode":{"type":"string"}})),
            ..Default::default()
        },
    )
    .await?;
    let source = release_source(&ctx.test_packs_dir, "approved-original", true)?;
    let mut tx = ctx.pool.begin().await?;
    DeploymentAdmission::lock(&mut tx).await?;
    RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        pack.id,
        issuer.id,
        serde_json::from_value(json!([{"resource":"keys", "actions":["read"]}]))?,
        Some(chrono::Utc::now() + chrono::Duration::seconds(30)),
    )
    .await?;
    let release_id = stage(&ctx, &mut tx, pack.id, installer.id, &source, 'a').await?;
    let policy = RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
        .await?
        .unwrap();
    tx.commit().await?;
    assert_eq!(
        HealthRepository::admitted_core(&ctx.pool)
            .await?
            .unwrap()
            .release_id,
        release_id
    );
    // Elapsed time is the contract. Poll the canonical repository fence, not a
    // sleep that assumes expiry or a mutation of immutable approval history.
    tokio::time::timeout(std::time::Duration::from_secs(45), async {
        loop {
            if HealthRepository::admitted_core(&ctx.pool).await.is_err() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await?;
    let replacement = release_source(&ctx.test_packs_dir, "expired-replacement", true)?;
    let mut denied = ctx.pool.begin().await?;
    assert!(
        stage(&ctx, &mut denied, pack.id, installer.id, &replacement, 'b')
            .await
            .is_err()
    );
    denied.rollback().await?;
    assert_eq!(
        PackReleaseRepository::find_active_by_pack_ref(&ctx.pool, "core")
            .await?
            .unwrap()
            .id,
        release_id
    );
    let mut retained = ctx.pool.begin().await?;
    assert_eq!(
        RuntimeAuthorityRepository::find_policy(&mut retained, policy.id)
            .await?
            .unwrap()
            .policy
            .release_id,
        release_id
    );
    retained.rollback().await?;
    ctx.cleanup().await?;
    Ok(())
}
