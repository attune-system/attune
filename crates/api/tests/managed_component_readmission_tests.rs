mod helpers;
mod managed_readmission_fixture;

use attune_common::{
    models::{WorkQueueBatchMode, WorkQueueUpdateStrategy},
    repositories::{
        action::ActionRepository,
        execution::{CreateExecutionInput, ExecutionRepository},
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        managed_executable::ManagedExecutableRepository,
        rule::RuleRepository,
        runtime_authority::{RuntimeAuthorityRepository, RuntimeOwner},
        work_queue::{CreateWorkQueueInput, WorkQueueRepository},
        Create, FindByRef,
    },
    runtime_authority::{HumanAttribution, InvocationBinding, RequestAttribution},
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::json;

fn source_graph() -> serde_json::Value {
    json!({"version":"1.0", "tasks":[{"name":"run","action":"managed_api.leaf","permission_set_refs":[],"retry":{"count":2,"delay":0}}]})
}

const SOURCE_QUEUE: &str = "ref: managed_api.jobs\nlabel: Jobs\ndispatch_action: managed_api.leaf\npermission_set_refs: []\n";
const SOURCE_WORKFLOW_ACTION: &str = "ref: managed_api.flow\nworkflow_file: workflows/flow.yaml\n";

fn dynamic_workflow_update(allowed: Option<Vec<&str>>) -> serde_json::Value {
    let mut definition = source_graph();
    definition["tasks"][0]["action"] = json!("{{ parameters.target }}");
    if let Some(allowed) = allowed {
        definition["tasks"][0]["allowed_action_refs"] = json!(allowed);
    }
    json!({"definition":definition})
}

fn assert_unbounded_target_diagnostic(body: &serde_json::Value) {
    assert_eq!(body["code"], "UNPROCESSABLE_ENTITY", "{body}");
    assert_eq!(body["details"]["stage"], "metadata_validation", "{body}");
    assert_eq!(
        body["details"]["outcome"], "active_release_unchanged",
        "{body}"
    );
    let diagnostics = body["details"]["diagnostics"]
        .as_array()
        .expect("located finite-target diagnostics");
    assert_eq!(diagnostics.len(), 1, "{body}");
    assert_eq!(diagnostics[0]["code"], "pack.metadata_invalid", "{body}");
    assert_eq!(diagnostics[0]["path"], "api/workflows", "{body}");
    assert_eq!(
        diagnostics[0]["metadata_path"], "/tasks/0/allowed_action_refs",
        "{body}"
    );
    assert_eq!(
        diagnostics[0]["message"], "Dynamic actions require 1 to 256 literal allowed_action_refs",
        "{body}"
    );
}

#[test]
fn workflow_update_dto_reaches_located_finite_target_validation() -> Result<()> {
    use attune_api::{dto::workflow::UpdateWorkflowRequest, middleware::ApiError};
    use attune_common::{
        pack_registry::runtime_policy::CandidateRuntimePolicyPlan,
        workflow::parser::WorkflowDefinition,
    };
    use validator::Validate;
    let request: UpdateWorkflowRequest = serde_json::from_value(dynamic_workflow_update(None))?;
    request.validate()?;
    let definition = request
        .definition
        .as_ref()
        .ok_or("required fixture graph missing")?;
    let _: WorkflowDefinition = serde_json::from_value(definition.clone())?;
    let error = CandidateRuntimePolicyPlan::validate_workflow_targets(
        definition,
        "managed_api",
        "managed_api.flow",
        "api/workflows",
    )
    .err()
    .ok_or("unbounded dynamic target passed planner validation")?;
    let error = ApiError::from(error);
    assert_eq!(error.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_unbounded_target_diagnostic(&serde_json::to_value(error.response_body())?);
    let repaired: UpdateWorkflowRequest =
        serde_json::from_value(dynamic_workflow_update(Some(vec!["managed_api.leaf"])))?;
    repaired.validate()?;
    CandidateRuntimePolicyPlan::validate_workflow_targets(
        repaired
            .definition
            .as_ref()
            .ok_or("repaired graph missing")?,
        "managed_api",
        "managed_api.flow",
        "api/workflows",
    )?;
    Ok(())
}

async fn put_managed_file(
    ctx: &TestContext,
    token: &str,
    definition: serde_json::Value,
    label: &str,
) -> Result<()> {
    let response = ctx.put("/api/v1/workflows/managed_api.flow/file", json!({
        "name":"flow", "label":label, "version":"1.0", "pack_ref":"managed_api", "definition":definition
    }), Some(token)).await?;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "workflow file PUT {label}: {}",
        response.text().await?
    );
    Ok(())
}

#[test]
fn managed_queue_fixture_obeys_the_deployment_metadata_contract() -> Result<()> {
    let queue = attune_common::queue_definition::parse_work_queue_definition_yaml(SOURCE_QUEUE)?;
    assert_eq!(queue.label, "Jobs");
    assert_eq!(queue.dispatch_action, "managed_api.leaf");
    assert_eq!(queue.permission_set_refs, Some(vec![]));
    assert!(
        attune_common::queue_definition::parse_work_queue_definition_yaml(
            &SOURCE_QUEUE.replace("label: Jobs\n", "")
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn managed_file_fixture_obeys_the_workflow_parser_contract() -> Result<()> {
    use attune_common::workflow::parser::WorkflowDefinition;
    let graph = source_graph();
    let parsed: WorkflowDefinition = serde_json::from_value(graph.clone())?;
    let _: WorkflowDefinition = serde_yaml_ng::from_str(&serde_yaml_ng::to_string(&graph)?)?;
    assert_eq!(parsed.version, "1.0");
    let mut without_version = graph.clone();
    without_version.as_object_mut().unwrap().remove("version");
    assert!(serde_json::from_value::<WorkflowDefinition>(without_version).is_err());
    let mut without_delay = graph;
    without_delay["tasks"][0]["retry"]
        .as_object_mut()
        .unwrap()
        .remove("delay");
    assert!(serde_json::from_value::<WorkflowDefinition>(without_delay).is_err());
    Ok(())
}

#[tokio::test]
async fn direct_managed_edits_readmit_but_labels_need_no_key_or_runtime_grants() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let token = ctx.token.as_deref().unwrap();
    let editor = attune_common::auth::jwt::validate_token(token, &ctx.state.jwt_config)?
        .sub
        .parse::<i64>()?;
    let pack = helpers::create_test_pack(&ctx.pool, "managed_api").await?;
    let graph = source_graph();
    // The same complete graph must pass the strict API parser before admission.
    let _: attune_common::workflow::parser::WorkflowDefinition =
        serde_json::from_value(graph.clone())?;
    let source = ctx.test_packs_dir.join("managed_api-release");
    for directory in [
        "actions/workflows",
        "rules",
        "queues",
        "triggers",
        "permission_sets",
        "runtimes",
    ] {
        std::fs::create_dir_all(source.join(directory))?;
    }
    for (file, body) in [
        ("pack.yaml", "ref: managed_api\nversion: '1.0.0'\nconf_schema: {}\nconfig: {}\n"),
        ("runtimes/native.yaml", "ref: managed_api.native\nname: Managed fixture native\nexecution_config: {}\n"),
        ("actions/leaf.yaml", "ref: managed_api.leaf\nrunner_type: managed_api.native\nentry_point: leaf.sh\ndefault_execution_permission_set_refs: []\nparameters:\n  message:\n    type: string\n"),
        ("actions/other.yaml", "ref: managed_api.other\nrunner_type: managed_api.native\nentry_point: leaf.sh\ndefault_execution_permission_set_refs: []\nparameters:\n  message:\n    type: string\n"),
        ("actions/leaf.sh", "#!/bin/sh\nexit 0\n"),
        ("actions/flow.yaml", SOURCE_WORKFLOW_ACTION),
        ("triggers/tick.yaml", "ref: managed_api.tick\nlabel: Tick\n"),
        ("rules/route.yaml", "ref: managed_api.route\ntrigger_ref: managed_api.tick\naction_ref: managed_api.leaf\naction_params: {}\npermission_set_refs: []\ninput_permission_set_refs: [managed_api.inputs]\n"),
        ("queues/jobs.yaml", SOURCE_QUEUE),
        ("permission_sets/inputs.yaml", "ref: managed_api.inputs\nscope: pack\ngrants:\n  - resource: keys\n    actions: [read]\n"),
    ] {
        std::fs::write(source.join(file), body)?;
    }
    std::fs::write(
        source.join("actions/workflows/flow.yaml"),
        serde_yaml_ng::to_string(&graph)?,
    )?;
    let original = managed_readmission_fixture::deploy(&ctx, &pack, &source, editor).await?;
    let leaf = ActionRepository::find_by_ref(&ctx.pool, "managed_api.leaf")
        .await?
        .unwrap();
    let root = ActionRepository::find_by_ref(&ctx.pool, "managed_api.flow")
        .await?
        .unwrap();
    let authored_action: serde_json::Value = serde_yaml_ng::from_str(SOURCE_WORKFLOW_ACTION)?;
    let workflow_file = authored_action["workflow_file"]
        .as_str()
        .ok_or("action workflow_file missing")?;
    assert_eq!(root.entrypoint, workflow_file);
    let stored_graph = attune_common::workflow::parser::parse_workflow_file(
        &source.join("actions").join(workflow_file),
    )?;
    assert_eq!(
        stored_graph.tasks[0].action.as_deref(),
        Some("managed_api.leaf")
    );
    let rule = RuleRepository::find_by_ref(&ctx.pool, "managed_api.route")
        .await?
        .unwrap();
    let rights = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "managed_api.component_editor".into(), pack: Some(pack.id), pack_ref: Some(pack.r#ref.clone()), label: None, description: None,
        grants: json!([
            {"resource":"actions","actions":["update"],"constraints":{"refs":["managed_api.flow"]}},
            {"resource":"rules","actions":["update"],"constraints":{"refs":["managed_api.route"]}},
            {"resource":"queues","actions":["update"]}
        ]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: editor,
            permset: rights.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(editor).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;

    let queue_input = CreateWorkQueueInput {
        r#ref: "managed_api.jobs".into(),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        is_adhoc: false,
        label: "Jobs".into(),
        description: None,
        enabled: true,
        accepting_new_items: true,
        dispatch_action: Some(leaf.id),
        dispatch_action_ref: leaf.r#ref.clone(),
        default_priority: 0,
        allow_pending_update: false,
        update_strategy: WorkQueueUpdateStrategy::Immutable,
        batch_mode: WorkQueueBatchMode::Single,
        item_schema: json!({}),
        action_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: Some(vec![]),
        config: json!({}),
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: vec![],
    };
    WorkQueueRepository::create(
        &ctx.pool,
        CreateWorkQueueInput {
            r#ref: "managed_api.associated".into(),
            is_adhoc: true,
            ..queue_input
        },
    )
    .await?;
    let mut tx = ctx.pool.begin().await?;
    let input_revision: i64 =
        sqlx::query_scalar("SELECT revision FROM rule_runtime_authority_current WHERE rule = $1")
            .bind(rule.id)
            .fetch_one(&mut *tx)
            .await?;
    let root_binding = InvocationBinding::Action {
        action_ref: root.r#ref.clone(),
    };
    let original_root =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &original.policy, &root_binding)
            .await?;
    assert_eq!(
        original_root
            .executable
            .workflow_definition
            .as_ref()
            .map(|graph| graph.id),
        root.workflow_def
    );
    assert!(
        root.workflow_def.is_some(),
        "deployment must link the actual backing graph"
    );
    let historical = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(root.id),
            action_ref: root.r#ref.clone(),
            executor: Some(editor),
            permission_set_refs: vec![],
            config: Some(json!({})),
            ..Default::default()
        },
        &original_root,
    )
    .await?;
    let selected = original.policy.select(
        original.id,
        &root_binding,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: editor,
                login: ctx.user.as_ref().unwrap().login.clone(),
            },
        },
    )?;
    let original_admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(historical.id),
        &selected,
    )
    .await?;
    tx.commit().await?;

    for path in [
        "/api/v1/workflows/managed_api.flow",
        "/api/v1/rules/managed_api.route",
        "/api/v1/queues/managed_api.jobs",
    ] {
        ctx.put(path, json!({"label":"Only a label"}), Some(token))
            .await?
            .assert_status(StatusCode::OK);
    }
    ctx.put(
        "/api/v1/queues/managed_api.associated",
        json!({"label":"Must not authorize association"}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    ctx.put(
        "/api/v1/queues/managed_api.jobs",
        json!({"label":"Same owner", "pack_ref":{"op":"set","value":"managed_api"}}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::OK);
    ctx.put(
        "/api/v1/queues/managed_api.jobs",
        json!({"pack_ref":{"op":"clear"}}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    let mut label_graph = graph.clone();
    label_graph["label"] = json!("Only a file label");
    label_graph["tasks"][0]["__chart_meta__"] = json!({"position":{"x":10,"y":20}});
    put_managed_file(&ctx, token, label_graph, "Only a file label").await?;
    let mut tx = ctx.pool.begin().await?;
    assert_eq!(
        RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
            .await?
            .unwrap()
            .id,
        original.id,
        "typed-loader serialization plus label/layout edits must retain authority"
    );
    tx.rollback().await?;
    let denied = ctx
        .put(
            "/api/v1/workflows/managed_api.flow",
            dynamic_workflow_update(None),
            Some(token),
        )
        .await?;
    let status = denied.status();
    let body = denied.text().await?;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unbounded workflow semantic validation: {body}"
    );
    let denied: serde_json::Value = serde_json::from_str(&body)?;
    assert_unbounded_target_diagnostic(&denied);
    let mut tx = ctx.pool.begin().await?;
    assert_eq!(
        RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
            .await?
            .unwrap()
            .id,
        original.id
    );
    tx.rollback().await?;
    // Both behavioral file edits must independently readmit. A later rule or
    // queue change cannot mask a missing workflow admission hook.
    let mut input_graph = graph.clone();
    input_graph["tasks"][0]["input"] = json!({"message":"changed input"});
    put_managed_file(&ctx, token, input_graph.clone(), "Behavioral input edit").await?;
    let mut tx = ctx.pool.begin().await?;
    let input_policy = RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
        .await?
        .unwrap();
    assert_ne!(
        input_policy.id, original.id,
        "actual task input edit must readmit"
    );
    let input_snapshot = ManagedExecutableRepository::resolve_invocation(
        &mut tx,
        &input_policy.policy,
        &root_binding,
    )
    .await?;
    assert_eq!(
        input_snapshot
            .executable
            .workflow_definition
            .as_ref()
            .unwrap()
            .definition["tasks"][0]["input"],
        json!({"message":"changed input"})
    );
    tx.rollback().await?;
    let mut target_graph = input_graph;
    target_graph["tasks"][0]["action"] = json!("managed_api.other");
    put_managed_file(&ctx, token, target_graph, "Behavioral target edit").await?;
    let mut tx = ctx.pool.begin().await?;
    let target_policy = RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
        .await?
        .unwrap();
    assert_ne!(
        target_policy.id, input_policy.id,
        "actual task target edit must readmit"
    );
    let target_snapshot = ManagedExecutableRepository::resolve_invocation(
        &mut tx,
        &target_policy.policy,
        &root_binding,
    )
    .await?;
    assert_eq!(
        target_snapshot
            .executable
            .workflow_definition
            .as_ref()
            .unwrap()
            .definition["tasks"][0]["action"],
        "managed_api.other"
    );
    tx.rollback().await?;
    ctx.put(
        "/api/v1/rules/managed_api.route",
        json!({"trace_tag_template":"new-trace"}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::OK);
    let repaired = ctx
        .put(
            "/api/v1/workflows/managed_api.flow",
            dynamic_workflow_update(Some(vec!["managed_api.leaf"])),
            Some(token),
        )
        .await?;
    assert_eq!(
        repaired.status(),
        StatusCode::OK,
        "bounded dynamic workflow update: {}",
        repaired.text().await?
    );
    let mut tx = ctx.pool.begin().await?;
    let dynamic_policy = RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
        .await?
        .unwrap();
    let dynamic_snapshot = ManagedExecutableRepository::resolve_invocation(
        &mut tx,
        &dynamic_policy.policy,
        &root_binding,
    )
    .await?;
    let dynamic_graph = &dynamic_snapshot
        .executable
        .workflow_definition
        .as_ref()
        .unwrap()
        .definition;
    assert_eq!(
        dynamic_graph["tasks"][0]["action"],
        "{{ parameters.target }}"
    );
    assert_eq!(
        dynamic_graph["tasks"][0]["allowed_action_refs"],
        json!(["managed_api.leaf"])
    );
    assert!(dynamic_policy.policy.invocations.iter().any(|entry| matches!(&entry.binding, InvocationBinding::WorkflowTask { workflow_ref, action_ref, .. } if workflow_ref == "managed_api.flow" && action_ref == "managed_api.leaf")));
    tx.rollback().await?;
    ctx.put(
        "/api/v1/queues/managed_api.jobs",
        json!({"config":{"dispatch":{"separate_requesters":true}}}),
        Some(token),
    )
    .await?
    .assert_status(StatusCode::OK);
    let mut tx = ctx.pool.begin().await?;
    let revised = RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
        .await?
        .unwrap();
    assert_ne!(revised.id, original.id);
    let retained =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &original.policy, &root_binding)
            .await?;
    assert_eq!(
        serde_json::to_value(retained)?,
        serde_json::to_value(&original_root)?
    );
    assert_eq!(revised.policy.release_id, original.policy.release_id);
    let retained_admission =
        RuntimeAuthorityRepository::execution_admission(&mut tx, historical.id)
            .await?
            .unwrap();
    assert_eq!(retained_admission.id, original_admission.id);
    assert_eq!(retained_admission.policy_revision, original.id);
    assert_eq!(
        retained_admission.authority_digest,
        original_admission.authority_digest
    );
    let stored: (serde_json::Value, Vec<String>, Option<serde_json::Value>) = sqlx::query_as(
        "SELECT executable_snapshot,permission_set_refs,config FROM execution WHERE id=$1",
    )
    .bind(historical.id)
    .fetch_one(&mut *tx)
    .await?;
    assert_eq!(stored.0, serde_json::to_value(&original_root)?);
    assert!(stored.1.is_empty());
    assert_eq!(stored.2, Some(json!({})));
    let retained_input_revision: i64 =
        sqlx::query_scalar("SELECT revision FROM rule_runtime_authority_current WHERE rule = $1")
            .bind(rule.id)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(retained_input_revision, input_revision);
    tx.rollback().await?;
    ctx.cleanup().await?;
    Ok(())
}
