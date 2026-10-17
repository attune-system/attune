//! Connected-router regressions, scheduled for the integrator's combined PG18 run.
use super::*;
use attune_common::repositories::FindById;

#[path = "collections.rs"]
mod collections;

async fn managed_queue(
    fixture: &Fixture,
    reference: &str,
) -> Result<attune_common::models::WorkQueue> {
    use attune_common::repositories::{
        action::ActionRepository,
        pack::PackRepository,
        work_queue::{CreateWorkQueueInput, WorkQueueRepository},
        FindByRef,
    };
    let pack = PackRepository::find_by_id(&fixture.ctx.pool, fixture.pack_id)
        .await?
        .unwrap();
    let action = ActionRepository::find_by_ref(&fixture.ctx.pool, "recorded_emitter.emit")
        .await?
        .unwrap();
    let queue = WorkQueueRepository::create(
        &fixture.ctx.pool,
        CreateWorkQueueInput {
            r#ref: reference.into(),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            is_adhoc: false,
            label: "Managed test queue".into(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(action.id),
            dispatch_action_ref: action.r#ref,
            default_priority: 0,
            allow_pending_update: true,
            update_strategy: attune_common::models::WorkQueueUpdateStrategy::Replace,
            batch_mode: attune_common::models::WorkQueueBatchMode::Single,
            item_schema: json!({"message":{"type":"string"}}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: Some(vec![]),
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
            actions: vec![action.id],
            work_queues: vec![queue.id],
            ..Default::default()
        },
    )
    .await?;
    Ok(queue)
}

async fn sibling(fixture: &Fixture) -> Result<(RuntimeCredentialBinding, String)> {
    let mut tx = fixture.ctx.pool.begin().await?;
    let policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap();
    let refs = vec!["recorded_emitter.events".to_owned()];
    let original =
        RuntimeAuthorityRepository::execution_admission(&mut tx, fixture.credential.execution_id)
            .await?
            .unwrap();
    // A sibling is an independent accepted invocation, not an API-created child
    // whose operational lifetime must depend on the fixture's source admission.
    assert!(matches!(
        &original.authority.attribution,
        RequestAttribution::Human { .. }
    ));
    let selected = policy.policy.select(
        policy.id,
        &policy.policy.invocations[0].binding,
        &refs,
        original.authority.attribution.clone(),
    )?;
    let source = ExecutionRepository::find_by_id(&mut *tx, fixture.credential.execution_id)
        .await?
        .unwrap();
    let snapshot = source.executable_snapshot.unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: "recorded_emitter.emit".into(),
            status: ExecutionStatus::Running,
            executor: Some(fixture.requester),
            permission_set_refs: refs.clone(),
            config: Some(json!({})),
            ..Default::default()
        },
        &snapshot,
    )
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
        action_ref: "recorded_emitter.emit".into(),
        attempt: selected.attempt,
    };
    tx.commit().await?;
    let token = generate_admitted_execution_token(
        &credential,
        &refs,
        &fixture.ctx.state.jwt_config,
        Some(360),
    )?;
    Ok((credential, token))
}

#[tokio::test]
async fn recorded_queue_operations_use_structural_ownership_and_exact_execution_revocation(
) -> Result<()> {
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[
            json!({"resource":"queues","actions":["read","create","update","delete"]}),
            json!({"resource":"queue_items","actions":["read","create","update","delete"]}),
            json!({"resource":"actions","actions":["execute"]}),
        ],
    )
    .await?;
    let created = fixture.ctx.post("/api/v1/queues", json!({
        "ref":"recorded_emitter.api_queue", "pack_ref":"recorded_emitter", "label":"API queue",
        "dispatch_action_ref":"recorded_emitter.emit", "permission_set_refs":[],
        "item_schema":{"message":{"type":"string"}}, "allow_pending_update":true, "update_strategy":"replace"
    }), Some(&fixture.token)).await?;
    assert_eq!(
        created.status(),
        StatusCode::FORBIDDEN,
        "{}",
        created.text().await?
    );
    // API-created queue metadata is ad hoc. Deployment supplies a real retained
    // queue projection and finite invocation binding before item acceptance.
    use attune_common::repositories::{action::ActionRepository, FindByRef};
    let installed = managed_queue(&fixture, "recorded_emitter.api_queue").await?;
    let dispatch = ActionRepository::find_by_ref(&fixture.ctx.pool, "recorded_emitter.emit")
        .await?
        .unwrap();
    let mut deployment = fixture.ctx.pool.begin().await?;
    let old_policy = RuntimeAuthorityRepository::find_policy(&mut deployment, fixture.policy_id)
        .await?
        .unwrap();
    let snapshot = ExecutableSnapshotRepository::resolve_for_action_ref(
        &mut *deployment,
        "recorded_emitter.emit",
    )
    .await?;
    let policy = RuntimeAuthorityRepository::capture_policy(
        &mut deployment,
        &RuntimePolicy {
            concrete_targets: BTreeMap::new(),
            owner: old_policy.policy.owner.clone(),
            release_id: snapshot.release.id,
            release_digest: snapshot.release.digest.clone(),
            invocations: vec![InvocationPolicy {
                foreign_workflow_policy: None,
                metadata_revision: None,
                source_id: None,
                dispatch_template_digest: None,
                binding: InvocationBinding::Queue {
                    queue_ref: installed.r#ref,
                    action_ref: dispatch.r#ref.clone(),
                },
                target_release_id: snapshot.release.id,
                target_release_digest: snapshot.release.digest.clone(),
                permission_sets: BTreeMap::new(),
                standard_access: false,
            }],
        },
    )
    .await?;
    RuntimeAuthorityRepository::activate_policy(&mut deployment, policy.id).await?;
    deployment.commit().await?;
    let queue = "/api/v1/queues/recorded_emitter.api_queue";
    let items = format!("{queue}/items");
    let accepted = fixture
        .ctx
        .post(
            &items,
            json!({"item_key":"one","payload":{"message":"literal"}}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(accepted.status(), StatusCode::CREATED);
    let body: Value = accepted.json().await?;
    let id = body["data"]["id"].as_i64().unwrap();
    use attune_common::repositories::action::UpdateActionInput;
    ActionRepository::update(
        &fixture.ctx.pool,
        dispatch.id,
        UpdateActionInput {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await?;
    let replaced = fixture
        .ctx
        .post(
            &items,
            json!({"item_key":"one","payload":{"message":"replacement"}}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(replaced.status(), StatusCode::OK);
    assert_eq!(
        fixture
            .ctx
            .post(
                &items,
                json!({"item_key":"new","payload":{"message":"new acceptance"}}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    ActionRepository::update(
        &fixture.ctx.pool,
        dispatch.id,
        UpdateActionInput {
            enabled: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let bulk = fixture
        .ctx
        .post(
            &format!("{items}/bulk"),
            json!({"items":[
                {"item_key":"two","payload":{"message":"literal two"}},
                {"item_key":"three","payload":{"message":"literal three"}}
            ]}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(bulk.status(), StatusCode::CREATED);
    let reprioritized = fixture
        .ctx
        .post(
            &format!("{items}/query/apply"),
            json!({
                "selector":{"path":"$","vars":{}}, "operation":"reprioritize", "priority":11
            }),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(reprioritized.status(), StatusCode::OK);
    assert_eq!(
        fixture
            .ctx
            .get(&items, Some(&fixture.token))
            .await?
            .status(),
        StatusCode::OK
    );
    let updated = fixture
        .ctx
        .put(
            &format!("{items}/{id}"),
            json!({"priority":9}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(updated.status(), StatusCode::OK);
    // A result copied from a whole execution must not become item-owner data.
    use attune_common::repositories::work_queue::{
        UpdateWorkQueueItemInput, WorkQueueItemRepository,
    };
    WorkQueueItemRepository::update_if_statuses(
        &fixture.ctx.pool,
        id,
        WorkQueueItemRepository::mutable_pending_statuses(),
        UpdateWorkQueueItemInput {
            ack_summary: Some(attune_common::repositories::Patch::Set(
                json!({"other_requester":"private batch output"}),
            )),
            ..Default::default()
        },
    )
    .await?;
    let detail = fixture
        .ctx
        .get(&format!("{items}/{id}"), Some(&fixture.token))
        .await?;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail: Value = detail.json().await?;
    assert!(detail["data"]["ack_summary"].is_null());
    assert!(detail["data"]["leased_execution"].is_null());
    // This item inherited the runtime creator's original input ceiling. A later
    // human grant does not enlarge it, so this literal payload edit is legitimate.
    let runtime_update = fixture
        .ctx
        .put(
            &format!("{items}/{id}"),
            json!({"payload":{"message":"runtime-owned replacement"}}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(
        runtime_update.status(),
        StatusCode::OK,
        "{}",
        runtime_update.text().await?
    );
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository, UpdatePermissionSetInput,
    };
    use attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository as Inputs;
    let key_grants: Vec<attune_common::rbac::Grant> = serde_json::from_value(json!([
        {"resource":"keys","actions":["read"],"constraints":{"refs":["system.accepted_input"]}}
    ]))?;
    let accepted_inputs = PermissionSetRepository::create(&fixture.ctx.pool, CreatePermissionSetInput {
        r#ref: "fixture.queue_inputs".into(), pack: None, pack_ref: None, label: None, description: None,
        grants: json!([
            {"resource":"keys","actions":["read"],"constraints":{"refs":["system.accepted_input"]}},
            {"resource":"queue_items","actions":["create","read"],"constraints":{"refs":["recorded_emitter.api_queue"]}},
            {"resource":"queues","actions":["read"],"constraints":{"refs":["recorded_emitter.api_queue"]}}
        ]),
    }).await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: accepted_inputs.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(fixture.requester)
        .await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    // Real Access-token enqueue captures this DIFFERENT input ceiling in its
    // original acceptance transaction. No post-acceptance capture/proof rewrite.
    let human_acceptance = fixture
        .ctx
        .post(
            &items,
            json!({"item_key":"human-input","payload":{"message":"human accepted literal"}}),
            fixture.ctx.token(),
        )
        .await?;
    let human_status = human_acceptance.status();
    let human_body: Value = human_acceptance.json().await?;
    assert_eq!(human_status, StatusCode::CREATED, "{human_body}");
    let human_item = human_body["data"]["id"].as_i64().unwrap();
    assert_eq!(
        human_body["data"]["requested_by_identity"],
        fixture.requester
    );
    assert!(human_body["data"]["requested_by_execution"].is_null());
    let (_, sibling_token) = sibling(&fixture).await?;

    for phase in ["original acceptance", "live grants removed", "human frozen"] {
        if phase == "live grants removed" {
            PermissionSetRepository::update(
                &fixture.ctx.pool,
                accepted_inputs.id,
                UpdatePermissionSetInput {
                    grants: Some(json!([])),
                    ..Default::default()
                },
            )
            .await?;
            attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(
                fixture.requester,
            )
            .await;
            attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
        } else if phase == "human frozen" {
            IdentityRepository::update(
                &fixture.ctx.pool,
                fixture.requester,
                UpdateIdentityInput {
                    frozen: Some(true),
                    ..Default::default()
                },
            )
            .await?;
        }
        let mut source = fixture.ctx.pool.begin().await?;
        let human_ceiling = Inputs::load(&mut source, "queue_item", human_item)
            .await?
            .unwrap();
        let runtime_ceiling = Inputs::load(&mut source, "queue_item", id).await?.unwrap();
        assert_eq!(human_ceiling.identity_id, fixture.requester, "{phase}");
        assert!(
            human_ceiling.covers(&key_grants),
            "original human Key Read must remain captured: {phase}"
        );
        assert!(
            !runtime_ceiling.covers(&key_grants),
            "runtime acceptance must not borrow a later human grant: {phase}"
        );
        source.rollback().await?;
        // Operation Update does not prove coverage of the original human input
        // ceiling. All three mutation routes must deny this runtime actor.
        let direct = fixture
            .ctx
            .put(
                &format!("{items}/{human_item}"),
                json!({"payload":{"message":"borrowed input"}}),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(
            direct.status(),
            StatusCode::FORBIDDEN,
            "{phase}: {}",
            direct.text().await?
        );
        let replacement = fixture
            .ctx
            .post(
                &items,
                json!({"item_key":"human-input","payload":{"message":"borrowed replace"}}),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(
            replacement.status(),
            StatusCode::FORBIDDEN,
            "{phase}: {}",
            replacement.text().await?
        );
        let bulk = fixture.ctx.post(&format!("{items}/query/apply"),
            json!({"selector":{"path":"$","vars":{}},"operation":"patch_payload","payload_patch":{"message":"borrowed bulk"}}),
            Some(&fixture.token),
        ).await?;
        assert_eq!(
            bulk.status(),
            StatusCode::FORBIDDEN,
            "{phase}: {}",
            bulk.text().await?
        );
        let retained_human = WorkQueueItemRepository::find_by_id(&fixture.ctx.pool, human_item)
            .await?
            .unwrap();
        assert_eq!(
            retained_human.payload,
            json!({"message":"human accepted literal"}),
            "{phase}"
        );
        let retained_runtime = WorkQueueItemRepository::find_by_id(&fixture.ctx.pool, id)
            .await?
            .unwrap();
        assert_eq!(
            retained_runtime.payload,
            json!({"message":"runtime-owned replacement"}),
            "failed bulk edits must roll back: {phase}"
        );
        let mut after = fixture.ctx.pool.begin().await?;
        assert!(
            Inputs::load(&mut after, "queue_item", human_item)
                .await?
                .unwrap()
                .covers(&key_grants),
            "denied edits must preserve the original human ceiling: {phase}"
        );
        assert!(
            !Inputs::load(&mut after, "queue_item", id)
                .await?
                .unwrap()
                .covers(&key_grants),
            "denied bulk edits must not replace the runtime ceiling: {phase}"
        );
        after.rollback().await?;
    }
    assert_eq!(
        fixture
            .ctx
            .put(
                &format!("{items}/{id}"),
                json!({"priority":10,"metadata":{"operator":"literal"}}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::OK
    );
    let retained = WorkQueueItemRepository::find_by_id(&fixture.ctx.pool, id)
        .await?
        .unwrap();
    assert_eq!(
        retained.payload,
        json!({"message":"runtime-owned replacement"})
    );
    let foreign = fixture.ctx.post("/api/v1/queues", json!({
        "ref":"recorded_emitter.looks_local", "pack_ref":"foreign_events", "label":"Foreign queue",
        "dispatch_action_ref":"foreign_events.nonexistent", "permission_set_refs":[]
    }), Some(&fixture.token)).await?;
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        fixture.ctx.get(queue, Some(&fixture.token)).await?.status(),
        StatusCode::OK
    );
    let mut tx = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_admission(&mut tx, fixture.credential.admission_id).await?;
    tx.commit().await?;
    assert_eq!(
        fixture.ctx.get(queue, Some(&fixture.token)).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture.ctx.get(queue, Some(&sibling_token)).await?.status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .ctx
            .delete(&format!("{items}/{id}"), Some(&sibling_token))
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .ctx
            .delete(queue, Some(&sibling_token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_api_child_is_admitted_and_metadata_control_does_not_borrow_human_iam(
) -> Result<()> {
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[
            json!({"resource":"actions","actions":["execute"]}),
            // This case exercises metadata/control, not separately authorized
            // plaintext. Requester freeze does not revoke recorded grants.
            json!({"resource":"executions","actions":["read","cancel"]}),
        ],
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::activate_policy(&mut tx, fixture.policy_id).await?;
    tx.commit().await?;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let child = fixture.ctx.post("/api/v1/executions/execute", json!({"action_ref":"recorded_emitter.emit", "parameters":{}, "permission_set_refs":[]}), Some(&fixture.token)).await?;
    assert_eq!(
        child.status(),
        StatusCode::CREATED,
        "{}",
        child.text().await?
    );
    let child: Value = child.json().await?;
    let id = child["data"]["id"].as_i64().unwrap();
    let persisted = ExecutionRepository::find_by_id(&fixture.ctx.pool, id)
        .await?
        .unwrap();
    assert_eq!(persisted.parent, Some(fixture.credential.execution_id));
    assert!(persisted.permission_set_refs.is_empty());
    let mut subtree_tx = fixture.ctx.pool.begin().await?;
    let pinned = persisted.executable_snapshot.as_ref().unwrap();
    let grandchild = ExecutionRepository::create_pinned(
        &mut *subtree_tx,
        CreateExecutionInput {
            action: persisted.action,
            action_ref: persisted.action_ref.clone(),
            parent: Some(id),
            status: ExecutionStatus::Requested,
            permission_set_refs: vec![],
            config: Some(json!({})),
            ..Default::default()
        },
        pinned,
    )
    .await?;
    subtree_tx.commit().await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    assert!(
        attune_common::repositories::execution_admission::ExecutionAdmissionRepository::find(
            &mut tx,
            RuntimeOwner::Execution(id)
        )
        .await?
        .is_some()
    );
    tx.commit().await?;
    let detail = fixture
        .ctx
        .get(&format!("/api/v1/executions/{id}"), Some(&fixture.token))
        .await?;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail: Value = detail.json().await?;
    assert!(detail["data"]["config"].is_null());
    assert!(detail["data"]["result"].is_null());
    let cancelled = fixture
        .ctx
        .post(
            &format!("/api/v1/executions/{id}/cancel"),
            json!({}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(cancelled.status(), StatusCode::OK);
    assert_eq!(
        ExecutionRepository::find_by_id(&fixture.ctx.pool, id)
            .await?
            .unwrap()
            .status,
        ExecutionStatus::Cancelled
    );
    assert_eq!(
        ExecutionRepository::find_by_id(&fixture.ctx.pool, grandchild.id)
            .await?
            .unwrap()
            .status,
        ExecutionStatus::Cancelled
    );
    let plaintext = fixture
        .ctx
        .get(
            &format!(
                "/api/v1/executions/{}?include_secret_values=true",
                fixture.credential.execution_id
            ),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(plaintext.status(), StatusCode::FORBIDDEN);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_standard_does_not_imply_execution_or_queue_operations() -> Result<()> {
    let fixture = fixture(true).await?;
    let execution = fixture
        .ctx
        .get(
            &format!("/api/v1/executions/{}", fixture.credential.execution_id),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(execution.status(), StatusCode::FORBIDDEN);
    let created = fixture.ctx.post("/api/v1/queues", json!({"ref":"recorded_emitter.no_standard_queue", "pack_ref":"recorded_emitter", "label":"Denied", "dispatch_action_ref":"recorded_emitter.emit", "permission_set_refs":[]}), Some(&fixture.token)).await?;
    assert_eq!(created.status(), StatusCode::FORBIDDEN);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_queue_metadata_grants_do_not_authorize_item_operations() -> Result<()> {
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[
            json!({"resource":"queues","actions":["read","create"]}),
            json!({"resource":"actions","actions":["execute"]}),
        ],
    )
    .await?;
    let created = fixture.ctx.post("/api/v1/queues", json!({
        "ref":"recorded_emitter.metadata_only", "pack_ref":"recorded_emitter", "label":"Metadata only",
        "dispatch_action_ref":"recorded_emitter.emit", "permission_set_refs":[],
    }), Some(&fixture.token)).await?;
    assert_eq!(created.status(), StatusCode::FORBIDDEN);
    managed_queue(&fixture, "recorded_emitter.metadata_only").await?;
    assert_eq!(
        fixture
            .ctx
            .get(
                "/api/v1/queues/recorded_emitter.metadata_only",
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .ctx
            .post(
                "/api/v1/queues/recorded_emitter.metadata_only/items",
                json!({"payload":{}}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .ctx
            .get(
                "/api/v1/queues/recorded_emitter.metadata_only/items",
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
async fn recorded_ad_hoc_queue_metadata_uses_explicit_external_approval() -> Result<()> {
    let mut fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[json!({"resource":"actions","actions":["execute"]})],
    )
    .await?;
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let grant: attune_common::rbac::Grant = serde_json::from_value(json!({
        "resource":"queues", "actions":["create","read","update","delete"],
        "constraints":{}
    }))?;
    let issuer = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.queue_metadata_issuer".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: serde_json::to_value(vec![grant.clone()])?,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: issuer.id,
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        fixture.pack_id,
        fixture.requester,
        vec![grant.clone()],
        None,
    )
    .await?;
    let mut policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap()
        .policy
        .0;
    policy.invocations[0]
        .permission_sets
        .get_mut("recorded_emitter.events")
        .unwrap()
        .grants
        .push(attune_common::runtime_authority::AdmittedGrant::External {
            approval_id: approval.id,
            grant,
        });
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy).await?;
    tx.commit().await?;
    fixture.policy_id = policy.id;
    let (credential, token) = sibling(&fixture).await?;
    fixture.credential = credential;
    fixture.token = token;
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let created = fixture.ctx.post("/api/v1/queues", json!({
        "ref":"recorded_emitter.external_created", "pack_ref":"recorded_emitter", "label":"Externally approved metadata",
        "dispatch_action_ref":"recorded_emitter.emit", "permission_set_refs":[]
    }), Some(&fixture.token)).await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let path = "/api/v1/queues/recorded_emitter.external_created";
    assert_eq!(
        fixture.ctx.get(path, Some(&fixture.token)).await?.status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .ctx
            .put(
                path,
                json!({"label":"Updated metadata"}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::OK
    );
    // Metadata approval does not authorize delegated ad-hoc item acceptance.
    assert_eq!(
        fixture
            .ctx
            .post(
                &format!("{path}/items"),
                json!({"payload":{}}),
                Some(&fixture.token)
            )
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let deleted = fixture.ctx.delete(path, Some(&fixture.token)).await?;
    assert_eq!(
        deleted.status(),
        StatusCode::OK,
        "{}",
        deleted.text().await?
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_ad_hoc_literal_item_retains_action_admission_without_creator_iam() -> Result<()> {
    ad_hoc_literal_dispatch_case(attune_common::models::WorkQueueBatchMode::Single).await
}

#[tokio::test]
async fn recorded_ad_hoc_batch_seals_every_independent_source_before_authority_use() -> Result<()> {
    ad_hoc_literal_dispatch_case(attune_common::models::WorkQueueBatchMode::Batch).await
}

async fn ad_hoc_literal_dispatch_case(
    mode: attune_common::models::WorkQueueBatchMode,
) -> Result<()> {
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        ad_hoc_queue_admission::AdHocQueueAdmissionRepository,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        work_queue::{CreateWorkQueueInput, WorkQueueItemRepository, WorkQueueRepository},
        FindByRef,
    };
    let mut fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[json!({"resource":"actions","actions":["execute"]})],
    )
    .await?;
    let grant: attune_common::rbac::Grant = serde_json::from_value(
        json!({"resource":"queue_items","actions":["read","create","update","delete"]}),
    )?;
    let issuer = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.ad_hoc_item_issuer".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: serde_json::to_value(vec![grant.clone()])?,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: issuer.id,
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        fixture.pack_id,
        fixture.requester,
        vec![grant.clone()],
        None,
    )
    .await?;
    let mut policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap()
        .policy
        .0;
    policy.invocations[0]
        .permission_sets
        .get_mut("recorded_emitter.events")
        .unwrap()
        .grants
        .push(attune_common::runtime_authority::AdmittedGrant::External {
            approval_id: approval.id,
            grant,
        });
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy).await?;
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id).await?;
    tx.commit().await?;
    fixture.policy_id = policy.id;
    let (credential, token) = sibling(&fixture).await?;
    fixture.credential = credential;
    fixture.token = token;
    let action = ActionRepository::find_by_ref(&fixture.ctx.pool, "recorded_emitter.emit")
        .await?
        .unwrap();
    let queue = WorkQueueRepository::create(
        &fixture.ctx.pool,
        CreateWorkQueueInput {
            r#ref: "recorded_emitter.ad_hoc_literal".into(),
            pack: Some(fixture.pack_id),
            pack_ref: Some("recorded_emitter".into()),
            is_adhoc: true,
            label: "Ad-hoc literal dispatch".into(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(action.id),
            dispatch_action_ref: action.r#ref.clone(),
            default_priority: 0,
            allow_pending_update: true,
            update_strategy: attune_common::models::WorkQueueUpdateStrategy::Replace,
            batch_mode: mode,
            item_schema: json!({"message":{"type":"string"}}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: Some(vec![]),
            config: json!({}),
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: vec![],
        },
    )
    .await?;
    // Exercise the unapproved ceiling while the queue is drained, then restore
    // it BEFORE any immutable item template is accepted.
    for refs in [vec!["recorded_emitter.not_admitted".to_owned()], vec![]] {
        WorkQueueRepository::update(
            &fixture.ctx.pool,
            queue.id,
            attune_common::repositories::work_queue::UpdateWorkQueueInput {
                permission_set_refs: Some(attune_common::repositories::Patch::Set(refs.clone())),
                ..Default::default()
            },
        )
        .await?;
        if !refs.is_empty() {
            let denied = fixture
                .ctx
                .post(
                    &format!("/api/v1/queues/{}/items", queue.r#ref),
                    json!({"payload":{"message":"unapproved selection"}}),
                    Some(&fixture.token),
                )
                .await?;
            assert!(!denied.status().is_success());
        }
    }
    let response = fixture
        .ctx
        .post(
            &format!("/api/v1/queues/{}/items", queue.r#ref),
            json!({"payload":{"message":"literal"}}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "{}",
        response.text().await?
    );
    let item = WorkQueueItemRepository::search(
        &fixture.ctx.pool,
        &attune_common::repositories::work_queue::WorkQueueItemSearchFilters {
            queue: Some(queue.id),
            limit: 10,
            ..Default::default()
        },
    )
    .await?
    .rows
    .into_iter()
    .next()
    .unwrap();
    let second_source = if mode == attune_common::models::WorkQueueBatchMode::Batch {
        let (source, token) = sibling(&fixture).await?;
        let response = fixture
            .ctx
            .post(
                &format!("/api/v1/queues/{}/items", queue.r#ref),
                json!({"payload":{"message":"independent contributor"}}),
                Some(&token),
            )
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "{}",
            response.text().await?
        );
        Some(source)
    } else {
        None
    };
    let change = WorkQueueRepository::update(
        &fixture.ctx.pool,
        queue.id,
        attune_common::repositories::work_queue::UpdateWorkQueueInput {
            permission_set_refs: Some(attune_common::repositories::Patch::Set(vec![
                "recorded_emitter.not_admitted".into(),
            ])),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(change.to_string().contains("Drain accepted queue items"));
    IdentityRepository::update(
        &fixture.ctx.pool,
        fixture.requester,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    ActionRepository::update(
        &fixture.ctx.pool,
        action.id,
        UpdateActionInput {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await?;
    let response = fixture
        .ctx
        .put(
            &format!("/api/v1/queues/{}/items/{}", queue.r#ref, item.id),
            json!({"payload":{"message":"updated literal"}}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        response.text().await?
    );
    let response = fixture
        .ctx
        .post(
            &format!("/api/v1/queues/{}/items", queue.r#ref),
            json!({"payload":{"message":"new denied"}}),
            Some(&fixture.token),
        )
        .await?;
    assert!(!response.status().is_success());
    let mut tx = fixture.ctx.pool.begin().await?;
    let item = WorkQueueItemRepository::find_by_id(&mut *tx, item.id)
        .await?
        .unwrap();
    let items = WorkQueueItemRepository::search_with_conn(
        &mut *tx,
        &attune_common::repositories::work_queue::WorkQueueItemSearchFilters {
            queue: Some(queue.id),
            limit: 10,
            ..Default::default()
        },
    )
    .await?
    .rows;
    assert_eq!(items.len(), if second_source.is_some() { 2 } else { 1 });
    let pinned = AdHocQueueAdmissionRepository::template(&mut tx, &queue, &items)
        .await?
        .unwrap();
    let snapshot: attune_common::models::ExecutionExecutableSnapshot =
        serde_json::from_value(item.executable_snapshot.clone().unwrap())?;
    let selected = attune_common::repositories::execution_admission::ExecutionAdmissionRepository::queue_batch(&mut tx, &pinned, &items, &snapshot).await?;
    assert!(selected.authority().selected_refs.is_empty());
    assert!(matches!(
        selected.authority().binding,
        InvocationBinding::Action { .. }
    ));
    if second_source.is_some() {
        assert_eq!(
            selected.authority().attribution,
            RequestAttribution::QueueBatch {
                queue_ref: queue.r#ref.clone(),
                items: items.iter().map(|item| (item.id, None)).collect(),
            }
        );
    } else {
        assert_eq!(
            selected.authority().attribution,
            RequestAttribution::ParentExecution {
                execution_id: fixture.credential.execution_id,
            }
        );
    }
    let dispatch = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            config: Some(json!({"message":"updated literal"})),
            status: ExecutionStatus::Requested,
            executor: None,
            permission_set_refs: vec![],
            ..Default::default()
        },
        &snapshot,
    )
    .await?;
    let admitted = selected
        .attach(&mut tx, RuntimeOwner::Execution(dispatch.id))
        .await?;
    if second_source.is_some() {
        use attune_common::repositories::{
            accepted_batch_dependency::AcceptedBatchDependencyRepository,
            recorded_input_authority::RecordedInputAuthorityRepository,
        };
        assert!(
            RuntimeAuthorityRepository::retained_admission_fence(&mut tx, admitted.id)
                .await
                .is_err()
        );
        let ids: Vec<_> = items.iter().map(|item| item.id).collect();
        RecordedInputAuthorityRepository::capture_queue_execution(&mut tx, dispatch.id, &ids)
            .await?;
        assert!(
            AcceptedBatchDependencyRepository::capture(&mut tx, admitted.id, &[ids[0]])
                .await
                .is_err()
        );
        let fence = AcceptedBatchDependencyRepository::capture(&mut tx, admitted.id, &ids).await?;
        RuntimeAuthorityRepository::retained_admission_fence(&mut tx, admitted.id).await?;
        fence.ensure_fresh_on(&mut tx).await?;
    }
    assert!(
        AdHocQueueAdmissionRepository::require_drained(&mut tx, queue.id)
            .await
            .is_err()
    );
    RuntimeAuthorityRepository::revoke_admission(
        &mut tx,
        second_source
            .as_ref()
            .map(|source| source.admission_id)
            .unwrap_or(fixture.credential.admission_id),
    )
    .await?;
    assert!(attune_common::repositories::execution_admission::ExecutionAdmissionRepository::queue_batch(&mut tx, &pinned, &items, &snapshot).await.is_err());
    assert!(
        attune_common::repositories::execution_admission::ExecutionAdmissionRepository::require(
            &mut tx,
            RuntimeOwner::Execution(dispatch.id)
        )
        .await
        .is_err()
    );
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn recorded_execution_detail_commits_content_capture_before_returning_plaintext() -> Result<()>
{
    use attune_common::repositories::runtime_read::RuntimeReadRepository;
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[json!({"resource":"executions","actions":["read","decrypt"]})],
    )
    .await?;
    let id = fixture.credential.execution_id;
    let response = fixture
        .ctx
        .get(
            &format!("/api/v1/executions/{id}?include_secret_values=true"),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        response.text().await?
    );
    let mut tx = fixture.ctx.pool.begin().await?;
    let references = RuntimeReadRepository::references_for_executions(&mut *tx, &[id]).await?;
    assert_eq!(
        references
            .get(&id)
            .expect("plaintext detail must commit its runtime-read capture")
            .len(),
        1
    );
    assert!(
        RuntimeReadRepository::require_untainted(&mut tx, fixture.credential.admission_id)
            .await
            .is_err()
    );
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn generic_read_taint_denies_child_and_queue_sinks_before_input_admission() -> Result<()> {
    use attune_common::repositories::runtime_read::{
        RuntimeReadOrigin, RuntimeReadRepository, RuntimeReadTarget,
    };
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[
            json!({"resource":"actions","actions":["execute"]}),
            json!({"resource":"queue_items","actions":["create","update","read"]}),
        ],
    )
    .await?;
    managed_queue(&fixture, "recorded_emitter.taint_sink").await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    RuntimeReadRepository::record(
        &mut tx,
        &authority,
        RuntimeReadOrigin {
            target: RuntimeReadTarget::Execution {
                execution: fixture.credential.execution_id,
            },
            provenance: vec![],
            parents: vec![],
        },
    )
    .await?;
    tx.commit().await?;
    for (path, body) in [
        (
            "/api/v1/executions/execute",
            json!({"action_ref":"recorded_emitter.emit","parameters":{},"permission_set_refs":[]}),
        ),
        (
            "/api/v1/queues/recorded_emitter.taint_sink/items",
            json!({"payload":{"message":"copied plaintext"}}),
        ),
    ] {
        let response = fixture.ctx.post(path, body, Some(&fixture.token)).await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let error: Value = response.json().await?;
        assert!(
            error.to_string().contains("Runtime read evidence"),
            "wrong rejection: {error}"
        );
    }
    assert!(ExecutionRepository::find_by_parent(
        &fixture.ctx.pool,
        fixture.credential.execution_id
    )
    .await?
    .is_empty());
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn admitted_manual_default_keeps_supplied_strings_literal_with_empty_api_selection(
) -> Result<()> {
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        managed_executable::ManagedExecutableRepository,
        schema_type::SchemaTypeRepository,
        FindByRef,
    };
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &[],
        &[],
        &[json!({"resource":"actions","actions":["execute"]})],
    )
    .await?;
    let encryption_key = fixture
        .ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .unwrap();
    let action = ActionRepository::find_by_ref(&fixture.ctx.pool, "recorded_emitter.emit")
        .await?
        .unwrap();
    let human_execute = PermissionSetRepository::create(&fixture.ctx.pool, CreatePermissionSetInput {
        r#ref: "fixture.manual_default_execute".into(), pack: None, pack_ref: None,
        label: None, description: None,
        grants: json!([{"resource":"actions","actions":["execute"],"constraints":{"refs":[action.r#ref]}}]),
    }).await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: human_execute.id,
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    SchemaTypeRepository::lock_reclamation(&mut tx).await?;
    // This fixture captures a retained source policy without activating it.
    // Editing current managed metadata first needs that real deployment binding.
    RuntimeAuthorityRepository::activate_policy(&mut tx, fixture.policy_id).await?;
    // Secret defaults are forbidden by the flat schema contract, even when
    // their text looks like a Key expression. Keep that negative case separate
    // from the supported public default and supplied-string creation path.
    let forbidden_default = ActionRepository::update(&mut *tx, action.id, UpdateActionInput {
        param_schema: Some(json!({"token":{"type":"string","secret":true,"default":"{{ keystore['system.not_a_literal'] }}"}})),
        ..Default::default()
    }).await.unwrap_err();
    assert!(forbidden_default.to_string().contains("Secret literals"));
    ActionRepository::update(
        &mut *tx,
        action.id,
        UpdateActionInput {
            param_schema: Some(
                json!({"token":{"type":"string","default":"authored public value"},
                    "private_token":{"type":"string","secret":true}}),
            ),
            ..Default::default()
        },
    )
    .await?;
    ManagedExecutableRepository::readmit_action(&mut tx, action.id, fixture.requester)
        .await?
        .unwrap();
    tx.commit().await?;
    let created = fixture
        .ctx
        .post(
            "/api/v1/executions/execute",
            json!({"action_ref":action.r#ref,"parameters":{},"permission_set_refs":[]}),
            fixture.ctx.token(),
        )
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let body: Value = created.json().await?;
    let execution = body["data"]["id"].as_i64().unwrap();
    let persisted = ExecutionRepository::find_by_id(&fixture.ctx.pool, execution)
        .await?
        .unwrap();
    assert!(persisted.permission_set_refs.is_empty());
    let mut tx = fixture.ctx.pool.begin().await?;
    let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *tx,
        ENTITY_EXECUTION_CONFIG,
        execution,
    )
    .await?;
    assert_eq!(
        attune_common::secret_values::restore_secret_values(
            persisted.config.unwrap(),
            &secrets,
            encryption_key
        )?["token"],
        "authored public value"
    );
    assert!(secrets.is_empty());
    tx.commit().await?;
    let literal = "{{ keystore['system.not_an_authorized_selector'] }}";
    let supplied = fixture.ctx.post("/api/v1/executions/execute", json!({"action_ref":action.r#ref,"parameters":{"token":literal},"permission_set_refs":[]}), fixture.ctx.token()).await?;
    assert_eq!(supplied.status(), StatusCode::CREATED);
    let body: Value = supplied.json().await?;
    let execution = body["data"]["id"].as_i64().unwrap();
    let persisted = ExecutionRepository::find_by_id(&fixture.ctx.pool, execution)
        .await?
        .unwrap();
    let mut tx = fixture.ctx.pool.begin().await?;
    let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *tx,
        ENTITY_EXECUTION_CONFIG,
        execution,
    )
    .await?;
    assert_eq!(
        attune_common::secret_values::restore_secret_values(
            persisted.config.unwrap(),
            &secrets,
            encryption_key
        )?["token"],
        literal
    );
    assert!(secrets.is_empty());
    tx.rollback().await?;
    let private = fixture.ctx.post("/api/v1/executions/execute",
        json!({"action_ref":action.r#ref,"parameters":{"private_token":"caller-owned secret"},"permission_set_refs":[]}),
        fixture.ctx.token()).await?;
    assert_eq!(
        private.status(),
        StatusCode::CREATED,
        "{}",
        private.text().await?
    );
    let id = private.json::<Value>().await?["data"]["id"]
        .as_i64()
        .unwrap();
    let mut tx = fixture.ctx.pool.begin().await?;
    let actual = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *tx,
        ENTITY_EXECUTION_CONFIG,
        id,
    )
    .await?;
    assert_eq!(actual.len(), 1);
    let provenance: attune_common::secret_provenance::SecretProvenance =
        serde_json::from_str(actual[0].source_ref.as_deref().unwrap())?;
    assert_eq!(provenance.origins.len(), 1);
    let site = attune_common::repositories::inherited_input::InputSite {
        execution: id,
        kind: attune_common::repositories::inherited_input::InputEntityKind::ExecutionConfig,
        purpose: attune_common::repositories::inherited_input::InputSitePurpose::Value,
        path: "/private_token".into(),
    };
    assert!(
        attune_common::repositories::inherited_input::InheritedInputRepository::delivery_allowed(
            &mut tx,
            &site,
            &provenance.origins[0],
            encryption_key,
        )
        .await?
    );
    assert!(
        matches!(&provenance.origins[0], attune_common::secret_provenance::SecretOrigin::Entity { entity_type, path, .. }
        if entity_type == "declared_input:execution_config:value" && path == "/private_token")
    );
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn generic_contributor_proof_uses_frozen_parents_and_never_unions_input_authority(
) -> Result<()> {
    use attune_common::{
        delegation::DelegationAuthority,
        repositories::{
            key::KeyRepository,
            recorded_input_origins::RecordedInputOriginRepository,
            runtime_read::{
                RuntimeReadOrigin, RuntimeReadParent, RuntimeReadRepository, RuntimeReadTarget,
            },
        },
        secret_provenance::{SecretOrigin, SecretProvenance},
    };
    let fixture = fixture(false).await?;
    let key = KeyRepository::find_by_ref(&fixture.ctx.pool, "system.recorded_signer")
        .await?
        .unwrap();
    let source = fixture.credential.execution_id;
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    let read = RuntimeReadRepository::record(
        &mut tx,
        &authority,
        RuntimeReadOrigin {
            target: RuntimeReadTarget::Execution { execution: source },
            provenance: vec![SecretProvenance {
                origins: vec![SecretOrigin::Entity {
                    entity_type: ENTITY_EXECUTION_CONFIG.into(),
                    entity_id: source,
                    path: "/captured".into(),
                }],
                templates: vec![],
            }],
            parents: vec![RuntimeReadParent {
                entity_type: ENTITY_EXECUTION_CONFIG.into(),
                entity_id: source,
                path: "/captured".into(),
                provenance: SecretProvenance::key(attune_common::key_access::key_origin(&key)),
            }],
        },
    )
    .await?;
    // A wider live parent must not replace the Key requirement captured above.
    let live = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/captured".into(),
            value: json!("later value"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&SecretProvenance {
                origins: vec![SecretOrigin::Local {
                    source_kind: "parameter_schema".into(),
                    source_ref: None,
                }],
                templates: vec![],
            })?),
        }],
        fixture
            .ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .unwrap(),
    )?;
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        ENTITY_EXECUTION_CONFIG,
        source,
        &live,
    )
    .await?;
    let exec_grant = json!({"resource":"executions","actions":["read","decrypt"],"constraints":{"ids":[source]}});
    let allowed = DelegationAuthority::new(
        fixture.requester,
        Default::default(),
        serde_json::from_value(json!([
            exec_grant.clone(), {"resource":"keys","actions":["read","decrypt"],"constraints":{"ids":[key.id]}}
        ]))?,
    );
    let denied = DelegationAuthority::new(
        fixture.requester + 10000,
        Default::default(),
        serde_json::from_value(json!([exec_grant]))?,
    );
    assert!(RecordedInputOriginRepository::require_runtime_reads(
        &mut tx,
        &[denied.clone()],
        &[read.clone()]
    )
    .await
    .is_err());
    RecordedInputOriginRepository::require_runtime_reads(
        &mut tx,
        &[allowed.clone()],
        &[read.clone()],
    )
    .await?;
    assert!(RecordedInputOriginRepository::require_runtime_reads(
        &mut tx,
        &[allowed, denied],
        &[read]
    )
    .await
    .is_err());
    RecordedInputOriginRepository::require_runtime_reads(&mut tx, &[], &[]).await?;
    tx.rollback().await?;
    fixture.ctx.cleanup().await?;
    Ok(())
}
