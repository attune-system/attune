mod helpers;

use attune_common::{
    models::{EnforcementCondition, EnforcementStatus, ExecutionStatus},
    repositories::{
        component_lifecycle::PackProjectionIds,
        event::{CreateEnforcementInput, EnforcementRepository},
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        schema_contract::{BindingOwner, SchemaContractRepository},
        Delete,
    },
    value_schema::{compile, Limits, RegistryView, SchemaContext},
};
use helpers::{create_test_pool, ActionFixture, PackFixture};
use serde_json::json;

#[tokio::test]
async fn workflow_graph_schemas_are_bound_and_conflicting_metadata_is_rejected() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("workflow_graph_schema")
        .create(&db)
        .await
        .unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "run")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version.clone(),
            digest: "a".repeat(64),
            object_key: "packs/blobs/workflow-schema-fixture.tar.gz".into(),
            provider_version: "e:fixture".into(),
            content_path: "/packs/.releases/workflow-schema-fixture".into(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        release.id,
        &PackProjectionIds {
            actions: vec![action.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id)
        .await
        .unwrap();
    let input = json!({"count":{"type":"integer","default":0}});
    let output = json!({"count":{"type":"integer"}});
    snapshot.executable.workflow_definition = Some(
        attune_common::models::workflow::WorkflowDefinition {
            id: 1,
            r#ref: action.r#ref.clone(),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Workflow fixture".into(),
            description: None,
            version: pack.version.clone(),
            param_schema: Some(input.clone()),
            out_schema: Some(output.clone()),
            definition: json!({"ref":action.r#ref,"label":"Workflow fixture","version":"1.0.0","tasks":[],"parameters":input,"output":output}),
            tags: vec![],
            retired_at: None,
            created: chrono::Utc::now(),
            updated: chrono::Utc::now(),
        },
    );
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot)
        .await
        .unwrap();
    let compiled = attune_common::value_schema::CompiledContract::new(&binding.input).unwrap();
    assert_eq!(
        compiled.apply_defaults(&json!({})).unwrap(),
        json!({"count":0})
    );
    assert!(
        attune_common::value_schema::CompiledContract::new(binding.output.as_ref().unwrap())
            .unwrap()
            .validate(&json!({"count":"wrong"}))
            .is_err()
    );
    snapshot.executable.action.param_schema = Some(json!({"count":{"type":"string"}}));
    assert!(
        SchemaContractRepository::materialize_action(&mut tx, &snapshot)
            .await
            .unwrap_err()
            .to_string()
            .contains("different schemas")
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn static_task_bindings_protect_external_packs_before_child_creation() {
    use attune_common::repositories::{
        action::ActionRepository, pack::PackRepository, schema_contract::StaticActionSchemaBinding,
    };
    let db = create_test_pool().await.unwrap();
    let mut selected = Vec::new();
    for name in ["static-root", "static-task"] {
        let pack = PackFixture::new(name).create(&db).await.unwrap();
        let action = ActionFixture::new(pack.id, &pack.r#ref, "run")
            .create(&db)
            .await
            .unwrap();
        let mut transaction = db.begin().await.unwrap();
        let release = PackReleaseRepository::create_or_get(
            &mut transaction,
            CreatePackReleaseInput {
                pack: pack.id,
                pack_ref: pack.r#ref.clone(),
                version: "1.0.0".into(),
                digest: format!("{:0>64}", pack.id),
                object_key: format!("owned/{name}"),
                provider_version: "1".into(),
                content_path: format!("owned/{name}"),
                archive_size: 1,
                manifest: json!({"dependencies": {}}),
            },
        )
        .await
        .unwrap();
        PackReleaseRepository::activate_projected(
            &mut transaction,
            pack.id,
            release.id,
            &PackProjectionIds {
                actions: vec![action.id],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let snapshot =
            ExecutableSnapshotRepository::resolve_for_action(&mut *transaction, action.id)
                .await
                .unwrap();
        let binding = SchemaContractRepository::materialize_action(&mut transaction, &snapshot)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        selected.push((pack, action, snapshot, binding));
    }
    let (task_pack, task_action, task_snapshot, task_binding) = selected.pop().unwrap();
    let (_, root_action, root_snapshot, mut binding) = selected.pop().unwrap();
    binding.static_actions.insert(
        task_action.r#ref.clone(),
        StaticActionSchemaBinding {
            input: task_binding.input,
            output: task_binding.output,
            executable_snapshot: serde_json::to_value(&task_snapshot).unwrap(),
        },
    );
    let mut transaction = db.begin().await.unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *transaction,
        CreateExecutionInput {
            action: Some(root_action.id),
            action_ref: root_action.r#ref,
            config: Some(json!({})),
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: vec![],
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Requested,
            trace_tag: None,
            timeout_seconds: Some(60),
            result: None,
            workflow_task: None,
        },
        &root_snapshot,
    )
    .await
    .unwrap();
    SchemaContractRepository::attach(
        &mut transaction,
        BindingOwner::Execution,
        execution.id,
        &binding,
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    // Only the immutable task snapshot remains; no child execution exists.
    ActionRepository::delete(&db, task_action.id).await.unwrap();
    assert!(matches!(
        PackRepository::delete(&*db, task_pack.id).await,
        Err(attune_common::Error::PackDeletionBlocked(_))
    ));
    ExecutionRepository::delete(&db, execution.id)
        .await
        .unwrap();
    assert!(PackRepository::delete(&*db, task_pack.id).await.unwrap());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn bindings_are_immutable_inherited_and_independently_retained() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new("binding-test").create(&db).await.unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "action")
        .create(&db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: "1.0.0".into(),
            digest: "f".repeat(64),
            object_key: "owned/binding-test".into(),
            provider_version: "1".into(),
            content_path: "owned/binding-test".into(),
            archive_size: 1,
            manifest: json!({"dependencies": {}}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        release.id,
        &PackProjectionIds {
            actions: vec![action.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id)
        .await
        .unwrap();
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot)
        .await
        .unwrap();
    let enforcement = EnforcementRepository::create_or_get_by_rule_event_pinned(
        &mut tx,
        CreateEnforcementInput {
            rule: None,
            rule_ref: "binding-test.rule".into(),
            trigger_ref: "binding-test.trigger".into(),
            config: Some(json!({})),
            event: None,
            status: EnforcementStatus::Created,
            payload: json!({}),
            condition: EnforcementCondition::All,
            conditions: json!({}),
        },
        &snapshot,
    )
    .await
    .unwrap()
    .enforcement;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Enforcement, enforcement.id, &binding)
        .await
        .unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Enforcement, enforcement.id, &binding)
        .await
        .unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: Some(json!({})),
            env_vars: None,
            parent: None,
            enforcement: Some(enforcement.id),
            executor: None,
            permission_set_refs: vec![],
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Requested,
            trace_tag: None,
            timeout_seconds: None,
            result: None,
            workflow_task: None,
        },
        &snapshot,
    )
    .await
    .unwrap();
    assert!(
        SchemaContractRepository::inherit_enforcement(&mut tx, enforcement.id, execution.id)
            .await
            .unwrap()
    );
    assert_eq!(
        SchemaContractRepository::find(&mut tx, BindingOwner::Execution, execution.id)
            .await
            .unwrap(),
        Some(binding.clone())
    );
    let mut changed = binding.clone();
    changed.input = compile(
        binding.input.source.clone(),
        SchemaContext::FlatDocument,
        &json!({"value": {"type": "integer"}}),
        &RegistryView::new(),
        Limits::default(),
    )
    .unwrap();
    assert!(SchemaContractRepository::attach(
        &mut tx,
        BindingOwner::Execution,
        execution.id,
        &changed
    )
    .await
    .is_err());
    tx.commit().await.unwrap();

    EnforcementRepository::delete(&db, enforcement.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    assert!(
        SchemaContractRepository::find(&mut tx, BindingOwner::Enforcement, enforcement.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        SchemaContractRepository::find(&mut tx, BindingOwner::Execution, execution.id)
            .await
            .unwrap(),
        Some(binding)
    );
    tx.rollback().await.unwrap();
    ExecutionRepository::delete(&db, execution.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    assert!(
        SchemaContractRepository::find(&mut tx, BindingOwner::Execution, execution.id)
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
}
