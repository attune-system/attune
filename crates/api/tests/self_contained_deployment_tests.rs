//! Supported registrar deployment through ordinary authenticated multipart upload.
//! Six historical acceptance cases remain compiled and explicitly deferred, not passed.
//! See docs/testing/supported-deployment-scope.md for the scope exception.
mod helpers;
mod self_contained_deployment;

use attune_common::{
    repositories::{
        action::ActionRepository,
        execution::ExecutionRepository,
        identity::{IdentityRepository, PermissionSetRepository, UpdateIdentityInput},
        managed_executable::ManagedExecutableRepository,
        pack::PackRepository,
        rule::RuleRepository,
        runtime::RuntimeRepository,
        runtime_authority::RuntimeAuthorityRepository,
        trigger::TriggerRepository,
        work_queue::WorkQueueRepository,
        FindById, FindByRef, Update,
    },
    runtime_authority::{AdmittedGrant, InvocationBinding},
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use self_contained_deployment::{
    actor, consenting_installer, installer, remove_tick_consent, Archive,
};
use serde_json::{json, Value};

#[test]
fn fresh_archive_uses_canonical_workflow_and_queue_authoring() -> Result<()> {
    for reverse in [false, true] {
        let fixture = Archive::new("source_contract", reverse)?;
        for name in ["inner", "outer"] {
            let path = fixture
                .root
                .path()
                .join(format!("actions/workflows/{name}.workflow.yaml"));
            let graph = std::fs::read_to_string(path)?;
            attune_common::workflow::parser::parse_workflow_yaml(&graph)?;
            let graph: Value = serde_yaml_ng::from_str(&graph)?;
            assert!(
                graph.get("ref").is_none(),
                "graph must not embed action metadata"
            );
            assert!(graph.get("workflow_file").is_none());
        }
        let queue = std::fs::read_to_string(fixture.root.path().join("queues/inbox.yaml"))?;
        attune_common::queue_definition::parse_work_queue_definition_yaml(&queue)?;
        let rule = std::fs::read_to_string(fixture.root.path().join("rules/feedback.yaml"))?;
        let rule: Value = serde_yaml_ng::from_str(&rule)?;
        assert_eq!(
            rule["conditions"],
            json!({"expression": "{{ event.payload.message == 'start' }}"})
        );
        fixture.root.close()?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "Deferred SOW #112: approval-specific admission requires the disabled registrar exemption; see docs/testing/supported-deployment-scope.md"]
async fn unused_global_policy_definition_cannot_bypass_external_approval() -> Result<()> {
    let ctx = TestContext::new().await?;
    let deployer = installer(&ctx, &["global_subject"]).await?;
    let fixture = Archive::new("global_subject", false)?;
    fixture.write("permission_sets/unused_global.yaml", "ref: global_subject.unused\nscope: external\ngrants:\n  - resource: events\n    actions: [create]\n")?;
    let response = fixture.upload(&ctx, &deployer.token).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    located(
        &body,
        "permission_sets/unused_global.yaml",
        "/grants",
        "approval",
    );
    assert_absent(&ctx, "global_subject").await?;
    assert!(
        PermissionSetRepository::find_by_ref(&ctx.pool, "global_subject.unused")
            .await?
            .is_none()
    );
    ctx.cleanup().await?;
    fixture.root.close()?;
    Ok(())
}

async fn assert_absent(ctx: &TestContext, p: &str) -> Result<()> {
    assert!(PackRepository::find_by_ref(&ctx.pool, p).await?.is_none());
    for name in ["outer", "inner", "emit", "alternate"] {
        assert!(
            ActionRepository::find_by_ref(&ctx.pool, &format!("{p}.{name}"))
                .await?
                .is_none()
        );
    }
    assert!(
        TriggerRepository::find_by_ref(&ctx.pool, &format!("{p}.tick"))
            .await?
            .is_none()
    );
    assert!(
        RuntimeRepository::find_by_ref(&ctx.pool, &format!("{p}.python"))
            .await?
            .is_none()
    );
    assert!(
        PermissionSetRepository::find_by_ref(&ctx.pool, &format!("{p}.events"))
            .await?
            .is_none()
    );
    assert!(
        RuleRepository::find_by_ref(&ctx.pool, &format!("{p}.feedback"))
            .await?
            .is_none()
    );
    assert!(
        WorkQueueRepository::find_by_ref(&ctx.pool, &format!("{p}.inbox"))
            .await?
            .is_none()
    );
    Ok(())
}

async fn uploaded(fixture: &Archive, ctx: &TestContext, token: &str) -> Result<Value> {
    let response = fixture.upload(ctx, token).await?;
    let status = response.status();
    let body = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    Ok(body)
}

#[tokio::test]
#[ignore = "Deferred SOW #112: deployment without initial runtime consent requires the disabled registrar exemption; see docs/testing/supported-deployment-scope.md"]
async fn fresh_scoped_upload_pins_every_source_and_defaults_survive_installer_freeze() -> Result<()>
{
    fresh_scoped_upload(false).await
}

#[tokio::test]
#[ignore = "Deferred SOW #112: deployment without initial runtime consent requires the disabled registrar exemption; see docs/testing/supported-deployment-scope.md"]
async fn reversed_fresh_upload_resolves_nested_and_finite_targets_without_preallocated_ids(
) -> Result<()> {
    fresh_scoped_upload(true).await
}

async fn fresh_scoped_upload(reverse: bool) -> Result<()> {
    // Each order gets a fresh physical database and identity, not a redeploy of
    // an already-resolved graph. No pack/component rows are seeded here.
    let ctx = TestContext::new().await?;
    let fixture = Archive::new("fresh_acceptance", reverse)?;
    let deployer = installer(&ctx, &[&fixture.reference]).await?;
    assert_absent(&ctx, &fixture.reference).await?;
    assert_eq!(
        fixture.upload(&ctx, "invalid-token").await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_absent(&ctx, &fixture.reference).await?;
    uploaded(&fixture, &ctx, &deployer.token).await?;

    let pack = PackRepository::find_by_ref(&ctx.pool, &fixture.reference)
        .await?
        .unwrap();
    assert_eq!(pack.installed_by, Some(deployer.id));
    let mut tx = ctx.pool.begin().await?;
    let policy = RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
        .await?
        .unwrap();
    assert_eq!(pack.active_release, Some(policy.policy.release_id));
    let mut sources = std::collections::BTreeSet::new();
    for invocation in &policy.policy.invocations {
        assert!(
            invocation.metadata_revision.is_some(),
            "initial upload must capture immutable metadata: {:?}",
            invocation.binding
        );
        assert_eq!(invocation.target_release_id, policy.policy.release_id);
        assert_eq!(
            invocation.target_release_digest,
            policy.policy.release_digest
        );
        let local = invocation
            .permission_sets
            .get("fresh_acceptance.events")
            .expect("default local selection");
        assert!(!local.grants.is_empty());
        assert!(local.grants.iter().all(
            |grant| matches!(grant, AdmittedGrant::PackOwned { pack_id, .. } if *pack_id == pack.id)
        ));
        let snapshot = ManagedExecutableRepository::resolve_invocation(
            &mut tx,
            &policy.policy,
            &invocation.binding,
        )
        .await?;
        assert_eq!(
            snapshot.executable.action.r#ref,
            invocation.binding.action_ref()
        );
        match &invocation.binding {
            InvocationBinding::Action { action_ref } => {
                assert_eq!(invocation.source_id, Some(snapshot.executable.action.id));
                sources.insert(format!("action:{action_ref}"));
            }
            InvocationBinding::WorkflowTask {
                workflow_ref,
                action_ref,
                ..
            } => {
                let source = ActionRepository::find_by_ref(&mut *tx, workflow_ref)
                    .await?
                    .unwrap();
                assert_eq!(invocation.source_id, Some(source.id));
                sources.insert(format!("task:{workflow_ref}:{action_ref}"));
            }
            InvocationBinding::Rule { rule_ref, .. } => {
                let source = RuleRepository::find_by_ref(&mut *tx, rule_ref)
                    .await?
                    .unwrap();
                assert_eq!(invocation.source_id, Some(source.id));
                assert!(invocation.dispatch_template_digest.is_some());
                assert_ne!(source.owner_identity, Some(deployer.id));
                sources.insert("rule".into());
            }
            InvocationBinding::Queue { queue_ref, .. } => {
                let source = WorkQueueRepository::find_by_ref(&mut *tx, queue_ref)
                    .await?
                    .unwrap();
                assert_eq!(invocation.source_id, Some(source.id));
                assert!(invocation.dispatch_template_digest.is_some());
                sources.insert("queue".into());
            }
        }
    }
    assert_eq!(
        sources,
        [
            "action:fresh_acceptance.outer",
            "action:fresh_acceptance.inner",
            "action:fresh_acceptance.emit",
            "action:fresh_acceptance.alternate",
            "task:fresh_acceptance.outer:fresh_acceptance.inner",
            "task:fresh_acceptance.outer:fresh_acceptance.emit",
            "task:fresh_acceptance.outer:fresh_acceptance.alternate",
            "task:fresh_acceptance.inner:fresh_acceptance.emit",
            "rule",
            "queue",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    tx.rollback().await?;

    let invoker = actor(&ctx, json!([{"resource":"actions","actions":["execute"],"constraints":{"refs":["fresh_acceptance.emit","fresh_acceptance.outer"]}}])).await?;
    // Neither actor holds the runtime policy or internal Events/Create.
    for id in [deployer.id, invoker.id] {
        let sets = PermissionSetRepository::find_by_identity(&ctx.pool, id).await?;
        assert_eq!(sets.len(), 1);
        assert_ne!(sets[0].r#ref, "fresh_acceptance.events");
    }
    IdentityRepository::update(
        &ctx.pool,
        deployer.id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(deployer.id).await;
    assert_eq!(
        ctx.get("/api/v1/packs/fresh_acceptance", Some(&deployer.token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let options = ctx
        .get(
            "/api/v1/actions/fresh_acceptance.emit/execution-options",
            Some(&invoker.token),
        )
        .await?;
    let status = options.status();
    let options: Value = options.json().await?;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(options["data"]["policy_revision_id"], policy.id);
    assert_eq!(
        options["data"]["default_permission_set_refs"],
        json!(["fresh_acceptance.events"])
    );
    assert_eq!(options["data"]["default_selection_available"], true);
    for target in ["emit", "outer"] {
        // Omit permission_set_refs to exercise approved defaults, not an
        // explicit empty selection or an assigned human runtime permission.
        let response = ctx.post("/api/v1/executions/execute", json!({"action_ref":format!("fresh_acceptance.{target}"),"parameters":{"message":"done"}}), Some(&invoker.token)).await?;
        let status = response.status();
        let body: Value = response.json().await?;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let id = body["data"]["id"].as_i64().unwrap();
        let execution = ExecutionRepository::find_by_id(&ctx.pool, id)
            .await?
            .unwrap();
        let config = execution
            .config
            .as_ref()
            .expect("execution config persisted");
        assert_eq!(config["message"], "done");
        assert!(config.get("parameters").is_none());
        assert_eq!(
            execution.permission_set_refs,
            vec!["fresh_acceptance.events".to_owned()]
        );
        let mut tx = ctx.pool.begin().await?;
        let admission = RuntimeAuthorityRepository::execution_admission(&mut tx, id)
            .await?
            .unwrap();
        assert_eq!(admission.policy_revision, policy.id);
        tx.rollback().await?;
    }
    assert_eq!(
        ctx.post(
            "/api/v1/events",
            json!({"trigger_ref":"fresh_acceptance.tick","payload":{"message":"start"}}),
            Some(&invoker.token)
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    ctx.cleanup().await?;
    fixture.root.close()?;
    Ok(())
}

#[tokio::test]
async fn packs_only_installer_cannot_publish_tick_runtime_policy() -> Result<()> {
    let ctx = TestContext::new().await?;
    let deployer = installer(&ctx, &["no_tick_consent"]).await?;
    let fixture = Archive::new("no_tick_consent", false)?;
    let response = fixture.upload(&ctx, &deployer.token).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    located(
        &body,
        "permission_sets/events.yaml",
        "/grants/0",
        "registering identity",
    );
    assert_absent(&ctx, "no_tick_consent").await?;
    ctx.cleanup().await?;
    fixture.root.close()?;
    Ok(())
}

#[tokio::test]
async fn exact_consent_upload_captures_defaults_before_consent_removal_and_installer_freeze(
) -> Result<()> {
    consented_upload(false).await
}

#[tokio::test]
async fn reversed_exact_consent_upload_captures_defaults_before_consent_removal_and_installer_freeze(
) -> Result<()> {
    consented_upload(true).await
}

async fn consented_upload(reverse: bool) -> Result<()> {
    let ctx = TestContext::new().await?;
    let fixture = Archive::new("consented_upload", reverse)?;
    let deployer = consenting_installer(&ctx, &[&fixture.reference]).await?;
    assert_absent(&ctx, &fixture.reference).await?;
    assert_eq!(
        fixture.upload(&ctx, "invalid-token").await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_absent(&ctx, &fixture.reference).await?;
    let sets = PermissionSetRepository::find_by_identity(&ctx.pool, deployer.id).await?;
    assert_eq!(sets.len(), 1);
    assert_eq!(
        sets[0].grants,
        json!([
            {"resource":"packs","actions":["read","install","configure"],"constraints":{"refs":["consented_upload"]}},
            {"resource":"events","actions":["create"],"constraints":{"refs":["consented_upload.tick"]}}
        ])
    );
    uploaded(&fixture, &ctx, &deployer.token).await?;
    let pack = PackRepository::find_by_ref(&ctx.pool, &fixture.reference)
        .await?
        .unwrap();
    assert_eq!(pack.installed_by, Some(deployer.id));
    let mut tx = ctx.pool.begin().await?;
    let policy = RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
        .await?
        .unwrap();
    assert_eq!(pack.active_release, Some(policy.policy.release_id));
    assert_eq!(policy.policy.invocations.len(), 10);
    for invocation in &policy.policy.invocations {
        assert!(invocation.metadata_revision.is_some());
        assert!(invocation.source_id.is_some());
        assert_eq!(invocation.target_release_id, policy.policy.release_id);
        assert_eq!(
            invocation.target_release_digest,
            policy.policy.release_digest
        );
        let local = &invocation.permission_sets["consented_upload.events"];
        assert!(!local.grants.is_empty());
        assert!(local.grants.iter().all(
            |grant| matches!(grant, AdmittedGrant::PackOwned { pack_id, .. } if *pack_id == pack.id)
        ));
        let snapshot = ManagedExecutableRepository::resolve_invocation(
            &mut tx,
            &policy.policy,
            &invocation.binding,
        )
        .await?;
        assert_eq!(
            snapshot.executable.action.r#ref,
            invocation.binding.action_ref()
        );
    }
    tx.rollback().await?;

    remove_tick_consent(&ctx, &deployer, &[&fixture.reference]).await?;
    // Establish read access while active, including any warm grant cache.
    assert_eq!(
        ctx.get("/api/v1/packs/consented_upload", Some(&deployer.token))
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.post(
            "/api/v1/events",
            json!({"trigger_ref":"consented_upload.tick","payload":{"message":"start"}}),
            Some(&deployer.token),
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    IdentityRepository::update(
        &ctx.pool,
        deployer.id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    // Freeze must deny a warm access credential without depending on cache
    // invalidation. It does not revoke the separately captured pack policy.
    assert_eq!(
        ctx.get("/api/v1/packs/consented_upload", Some(&deployer.token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(deployer.id).await;
    assert_eq!(
        ctx.get("/api/v1/packs/consented_upload", Some(&deployer.token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ctx.get("/auth/me", Some(&deployer.token)).await?.status(),
        StatusCode::FORBIDDEN
    );
    let invoker = actor(&ctx, json!([{"resource":"actions","actions":["execute"],"constraints":{"refs":["consented_upload.emit","consented_upload.outer"]}}])).await?;
    let response = ctx
        .get(
            "/api/v1/actions/consented_upload.emit/execution-options",
            Some(&invoker.token),
        )
        .await?;
    let status = response.status();
    let options: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(options["data"]["policy_revision_id"], policy.id);
    assert_eq!(
        options["data"]["default_permission_set_refs"],
        json!(["consented_upload.events"])
    );
    assert_eq!(options["data"]["default_selection_available"], true);
    for target in ["emit", "outer"] {
        let response = ctx.post(
            "/api/v1/executions/execute",
            json!({"action_ref":format!("consented_upload.{target}"),"parameters":{"message":"done"}}),
            Some(&invoker.token),
        ).await?;
        let status = response.status();
        let body: Value = response.json().await?;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let id = body["data"]["id"].as_i64().unwrap();
        let execution = ExecutionRepository::find_by_id(&ctx.pool, id)
            .await?
            .unwrap();
        let config = execution.config.as_ref().unwrap();
        assert_eq!(config["message"], "done");
        assert!(config.get("parameters").is_none());
        assert_eq!(
            execution.permission_set_refs,
            vec!["consented_upload.events".to_owned()]
        );
        let mut tx = ctx.pool.begin().await?;
        let admission = RuntimeAuthorityRepository::execution_admission(&mut tx, id)
            .await?
            .unwrap();
        assert_eq!(admission.policy_revision, policy.id);
        tx.rollback().await?;
    }
    assert_eq!(
        ctx.post(
            "/api/v1/events",
            json!({"trigger_ref":"consented_upload.tick","payload":{"message":"start"}}),
            Some(&invoker.token),
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    ctx.cleanup().await?;
    fixture.root.close()?;
    Ok(())
}

/// Failures must identify the authored file and field, not just return any 4xx.
fn located(body: &Value, path: &str, field: &str, reason: &str) {
    let diagnostics = body["details"]["diagnostics"]
        .as_array()
        .expect("located deployment diagnostics");
    assert!(
        diagnostics.iter().any(|d| d["path"] == path
            && d["metadata_path"]
                .as_str()
                .is_some_and(|p| p.starts_with(field))
            && d["message"]
                .as_str()
                .is_some_and(|m| m.to_lowercase().contains(reason))),
        "{body}"
    );
}

#[tokio::test]
async fn missing_workflow_ref_fails_atomically_with_authored_location() -> Result<()> {
    rejected_graph(
        "missing",
        "broken_graph.absent",
        "",
        "missing local action 'broken_graph.absent'",
    )
    .await
}

#[tokio::test]
#[ignore = "Deferred historical graph-contract acceptance: recursive workflow calls remain valid by user policy; revisit only with explicit graph-contract redesign; see docs/testing/supported-deployment-scope.md"]
async fn recursive_nested_workflow_fails_atomically_with_authored_location() -> Result<()> {
    rejected_graph("recursive", "broken_graph.outer", "", "cycl").await
}

#[tokio::test]
async fn recursive_workflow_calls_upload_with_exact_consent_and_pinned_edges() -> Result<()> {
    let ctx = TestContext::new().await?;
    let deployer = consenting_installer(&ctx, &["recursive_allowed"]).await?;
    let fixture = Archive::new("recursive_allowed", false)?;
    fixture.write(
        "actions/workflows/inner.workflow.yaml",
        "version: '1.0'\ntasks:\n  - name: bad\n    action: recursive_allowed.outer\n",
    )?;
    assert_absent(&ctx, "recursive_allowed").await?;
    uploaded(&fixture, &ctx, &deployer.token).await?;
    let pack = PackRepository::find_by_ref(&ctx.pool, "recursive_allowed")
        .await?
        .unwrap();
    assert_eq!(pack.installed_by, Some(deployer.id));
    let mut tx = ctx.pool.begin().await?;
    let policy = RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
        .await?
        .unwrap();
    assert_eq!(pack.active_release, Some(policy.policy.release_id));
    for (source, target) in [
        ("recursive_allowed.outer", "recursive_allowed.inner"),
        ("recursive_allowed.inner", "recursive_allowed.outer"),
    ] {
        let binding = InvocationBinding::WorkflowTask {
            workflow_ref: source.into(),
            task_path: "/tasks/0".into(),
            action_ref: target.into(),
        };
        let invocation = policy
            .policy
            .invocations
            .iter()
            .find(|invocation| invocation.binding == binding)
            .expect("both recursive call edges must be captured");
        let source = ActionRepository::find_by_ref(&mut *tx, source)
            .await?
            .unwrap();
        assert_eq!(invocation.source_id, Some(source.id));
        assert!(invocation.metadata_revision.is_some());
        assert_eq!(invocation.target_release_id, policy.policy.release_id);
        assert_eq!(
            invocation.target_release_digest,
            policy.policy.release_digest
        );
        let snapshot =
            ManagedExecutableRepository::resolve_invocation(&mut tx, &policy.policy, &binding)
                .await?;
        assert_eq!(snapshot.release.id, policy.policy.release_id);
        assert_eq!(snapshot.executable.action.r#ref, target);
        assert_eq!(snapshot.executable.action.pack, pack.id);
        assert!(snapshot.executable.workflow_definition.is_some());
    }
    tx.rollback().await?;
    // Deployment acceptance does not require running an unbounded recursion.
    ctx.cleanup().await?;
    fixture.root.close()?;
    Ok(())
}

#[tokio::test]
async fn invalid_workflow_transition_fails_atomically_with_authored_location() -> Result<()> {
    rejected_graph(
        "transition",
        "broken_graph.emit",
        "    next:\n      - when: '{{ succeeded() }}'\n        do: [absent_task]\n",
        "absent_task",
    )
    .await
}

async fn rejected_graph(case: &str, action: &str, transition: &str, reason: &str) -> Result<()> {
    let ctx = TestContext::new().await?;
    let deployer = consenting_installer(&ctx, &["broken_graph"]).await?;
    let fixture = Archive::new("broken_graph", false)?;
    fixture.write(
        "actions/workflows/inner.workflow.yaml",
        &format!("version: '1.0'\ntasks:\n  - name: bad\n    action: {action}\n{transition}"),
    )?;
    let response = fixture.upload(&ctx, &deployer.token).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert!(status.is_client_error(), "{case}: {status}: {body}");
    assert_eq!(body["details"]["outcome"], "active_release_unchanged");
    if case == "missing" {
        assert!(
            body["details"]["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| {
                    diagnostic["code"] == "reference.missing_local_target"
                        && diagnostic["path"] == "actions/workflows/inner.workflow.yaml"
                        && diagnostic["metadata_path"] == "/tasks/0/action"
                }),
            "{body}"
        );
    }
    located(
        &body,
        "actions/workflows/inner.workflow.yaml",
        "/tasks",
        reason,
    );
    assert_absent(&ctx, "broken_graph").await?;
    ctx.cleanup().await?;
    fixture.root.close()?;
    Ok(())
}

#[tokio::test]
async fn repository_rejects_action_ref_outside_its_structural_pack() -> Result<()> {
    let ctx = TestContext::new().await?;
    let foreign = helpers::create_test_pack(&ctx.pool, "actual_owner").await?;
    // The ordinary repository must reject malformed ownership at creation.
    // Matching-prefix structural defense is tested at the loader unit boundary.
    let error = helpers::create_test_action(
        &ctx.pool,
        foreign.id,
        &foreign.r#ref,
        "collision_subject.emit",
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Schema consumer must belong to its declaring pack"),
        "{error}"
    );
    assert_absent(&ctx, "collision_subject").await?;
    assert_eq!(
        PackRepository::find_by_ref(&ctx.pool, "actual_owner")
            .await?
            .unwrap()
            .id,
        foreign.id
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn private_foreign_action_is_a_visibility_error_not_missing_external_approval() -> Result<()>
{
    let ctx = TestContext::new().await?;
    let deployer = consenting_installer(&ctx, &["private_publisher", "private_subject"]).await?;
    let publisher = Archive::new("private_publisher", false)?;
    publisher.replace(
        "actions/2_emit.yaml",
        "label: emit",
        "label: emit\nreference_visibility: private",
    )?;
    uploaded(&publisher, &ctx, &deployer.token).await?;
    let fixture = Archive::new("private_subject", false)?;
    fixture.write("actions/workflows/inner.workflow.yaml", "version: '1.0'\ntasks:\n  - name: private\n    action: private_publisher.emit\n    permission_set_refs: []\n")?;
    let response = fixture.upload(&ctx, &deployer.token).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert!(status.is_client_error(), "{body}");
    located(
        &body,
        "actions/workflows/inner.workflow.yaml",
        "/tasks/0/action",
        "because the action is private",
    );
    assert_absent(&ctx, "private_subject").await?;
    assert!(PackRepository::find_by_ref(&ctx.pool, "private_publisher")
        .await?
        .unwrap()
        .active_release
        .is_some());
    ctx.cleanup().await?;
    publisher.root.close()?;
    fixture.root.close()?;
    Ok(())
}

#[tokio::test]
#[ignore = "Deferred SOW #112: approval-specific admission requires the disabled registrar exemption; see docs/testing/supported-deployment-scope.md"]
async fn unapproved_external_scope_fails_atomically_with_authored_location() -> Result<()> {
    let ctx = TestContext::new().await?;
    let deployer = installer(&ctx, &["approval_publisher", "approval_subject"]).await?;
    let publisher = Archive::new("approval_publisher", false)?;
    uploaded(&publisher, &ctx, &deployer.token).await?;
    let fixture = Archive::new("approval_subject", false)?;
    fixture.replace(
        "permission_sets/events.yaml",
        "scope: pack",
        "scope: external",
    )?;
    fixture.replace(
        "permission_sets/events.yaml",
        "refs: [approval_subject.tick]",
        "refs: [approval_publisher.tick]",
    )?;
    let response = fixture.upload(&ctx, &deployer.token).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    located(&body, "permission_sets/events.yaml", "/grants", "approval");
    assert_absent(&ctx, "approval_subject").await?;
    ctx.cleanup().await?;
    publisher.root.close()?;
    fixture.root.close()?;
    Ok(())
}

#[tokio::test]
#[ignore = "Deferred SOW #112: independent approval reuse without deployer runtime grants requires the disabled registrar exemption; see docs/testing/supported-deployment-scope.md"]
async fn fixed_external_approval_is_reused_without_deployer_runtime_grants_after_issuer_freeze(
) -> Result<()> {
    let ctx = TestContext::new().await?;
    let deployer = installer(&ctx, &["approval_publisher", "approval_subject"]).await?;
    let publisher = Archive::new("approval_publisher", false)?;
    uploaded(&publisher, &ctx, &deployer.token).await?;
    let fixture = Archive::new("approval_subject", false)?;
    fixture.replace(
        "permission_sets/events.yaml",
        "scope: pack",
        "scope: external",
    )?;
    fixture.replace(
        "permission_sets/events.yaml",
        "refs: [approval_subject.tick]",
        "refs: [approval_publisher.tick]",
    )?;

    // Approval issuance is an explicit separate operation by a permission
    // administrator, never an assignment of external Events/Create to deployer.
    // The empty subject row is needed only by the approval control API. The
    // all-component fresh-install test above deliberately has no such seed.
    let approver = actor(&ctx, json!([
        {"resource":"packs","actions":["create","read"],"constraints":{"refs":["approval_subject"]}},
        {"resource":"permissions","actions":["manage"]}
    ])).await?;
    let response = ctx
        .post(
            "/api/v1/packs",
            json!({"ref":"approval_subject","label":"Approval subject","version":"1.0.0"}),
            Some(&approver.token),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let response = ctx.post("/api/v1/packs/approval_subject/external-approvals", json!({"grants":[{"resource":"events","actions":["create"],"constraints":{"refs":["approval_publisher.tick"]}}]}), Some(&approver.token)).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let approval = body["data"]["id"].as_i64().unwrap();
    IdentityRepository::update(
        &ctx.pool,
        approver.id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(approver.id).await;
    // Replacement is required by the ordinary API because the approval subject
    // row already exists. No component or admission has been created in it.
    let response = fixture
        .upload_with_force(&ctx, &deployer.token, true)
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let pack = PackRepository::find_by_ref(&ctx.pool, "approval_subject")
        .await?
        .unwrap();
    let mut tx = ctx.pool.begin().await?;
    let policy = RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
        .await?
        .unwrap();
    let approvals = policy
        .policy
        .invocations
        .iter()
        .flat_map(|invocation| invocation.permission_sets.values())
        .flat_map(|set| set.grants.iter())
        .filter_map(AdmittedGrant::approval_id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(approvals, [approval].into_iter().collect());
    RuntimeAuthorityRepository::require_deployment_policy(&mut tx, policy.id).await?;
    tx.rollback().await?;
    assert_eq!(
        PermissionSetRepository::find_by_identity(&ctx.pool, deployer.id)
            .await?
            .len(),
        1
    );
    ctx.cleanup().await?;
    publisher.root.close()?;
    fixture.root.close()?;
    Ok(())
}
