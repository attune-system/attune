mod helpers;

use attune_common::{
    action_cli::{ArgumentLimits, ArgumentPlan, InputDelivery, LiteralArgument},
    models::ParameterDelivery,
    pack_format::LaunchSpec,
    repositories::{
        action::{ActionRepository, CreateActionInput, UpdateActionInput},
        component_lifecycle::PackProjectionIds,
        executable_snapshot::ExecutableSnapshotRepository,
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        schema_contract::SchemaContractRepository,
        FindById, Patch,
    },
    value_schema::CompiledContract,
};
use helpers::{create_test_pool, PackFixture, RuntimeFixture};
use serde_json::json;

fn input(pack: &attune_common::models::Pack, runtime: i64) -> CreateActionInput {
    CreateActionInput {
        r#ref: format!("{}.run", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "CLI action".into(),
        description: None,
        entrypoint: None,
        launch: Some(LaunchSpec::Command {
            program: "printf".into(),
            arguments: vec![LiteralArgument {
                literal: "%s".into(),
                order: 0,
            }],
        }),
        parameter_delivery: None,
        parameter_format: None,
        runtime: Some(runtime),
        enabled: true,
        runtime_version_constraint: None,
        required_worker_runtimes: json!({}),
        worker_selector: json!({}),
        worker_tolerations: json!([]),
        worker_affinity: json!({}),
        param_schema: Some(
            json!({"message":{"type":"string","default":"original", "cli":{"kind":"positional","order":10}}}),
        ),
        out_schema: None,
        output_policy: Default::default(),
        output_format: Default::default(),
        is_adhoc: false,
        accesses_mcp: false,
        default_execution_permission_set_refs: vec![],
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: vec![],
        log_retention_policy: None,
        log_retention_limit: None,
        artifact_retention_policy: None,
        artifact_retention_limit: None,
        timeout_seconds: None,
    }
}

#[tokio::test]
async fn command_defaults_and_source_replacement_are_atomic() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("command_sources")
        .create(&db)
        .await
        .unwrap();
    let mut runtime = RuntimeFixture::new(None, None, "native");
    runtime.execution_config = json!({});
    let runtime = runtime.create(&db).await.unwrap();
    let action = ActionRepository::create(&db, input(&pack, runtime.id))
        .await
        .unwrap();
    assert_eq!(action.parameter_delivery, ParameterDelivery::None);
    assert!(action.entrypoint.is_none());
    // Patch omission retains the source and the resolved delivery policy.
    let renamed = ActionRepository::update(
        &db,
        action.id,
        UpdateActionInput {
            label: Some("Renamed".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(renamed.launch, action.launch);
    for update in [
        UpdateActionInput {
            entrypoint: Some(Patch::Set("run".into())),
            ..Default::default()
        },
        UpdateActionInput {
            launch: Some(Patch::Clear),
            ..Default::default()
        },
        UpdateActionInput {
            param_schema: Some(
                json!({"secret":{"type":"string","secret":true,"cli":{"kind":"positional","order":10}}}),
            ),
            ..Default::default()
        },
    ] {
        assert!(ActionRepository::update(&db, action.id, update)
            .await
            .is_err());
    }
    let unchanged = ActionRepository::get_by_id(&db, action.id).await.unwrap();
    assert_eq!(unchanged.launch, action.launch);
    assert_eq!(unchanged.param_schema, action.param_schema);
    // Source changes must explicitly repair incompatible delivery and schema.
    let replacement = UpdateActionInput {
        launch: Some(Patch::Clear),
        entrypoint: Some(Patch::Set("run".into())),
        param_schema: Some(json!({})),
        ..Default::default()
    };
    assert!(
        ActionRepository::update(&db, action.id, replacement.clone())
            .await
            .is_err()
    );
    let file = ActionRepository::update(
        &db,
        action.id,
        UpdateActionInput {
            parameter_delivery: Some("stdin".into()),
            ..replacement
        },
    )
    .await
    .unwrap();
    assert!(file.launch.is_none());
    assert_eq!(file.entrypoint.as_deref(), Some("run"));
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn immutable_release_binding_keeps_original_literals_defaults_and_cli_schema() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("command_binding")
        .create(&db)
        .await
        .unwrap();
    let mut runtime = RuntimeFixture::new(None, None, "native");
    runtime.execution_config = json!({});
    let runtime = runtime.create(&db).await.unwrap();
    let action = ActionRepository::create(&db, input(&pack, runtime.id))
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version.clone(),
            digest: "c".repeat(64),
            object_key: "owned/command-binding".into(),
            provider_version: "fixture".into(),
            content_path: "/owned/command-binding".into(),
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
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id)
        .await
        .unwrap();
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    ActionRepository::update(&db, action.id, UpdateActionInput {
        launch: Some(Patch::Set(LaunchSpec::Command {program:"other".into(), arguments: vec![]})),
        param_schema: Some(json!({"message":{"type":"string","default":"changed","cli":{"kind":"option","name":"--message","order":20}}})),
        ..Default::default()
    }).await.unwrap();
    let old = &snapshot.executable.action;
    let plan = ArgumentPlan::from_bound_contract(
        old.param_schema.as_ref().unwrap(),
        old.launch.as_ref().unwrap().action_arguments().unwrap(),
        binding.input.clone(),
    )
    .unwrap();
    let values = CompiledContract::new(&binding.input)
        .unwrap()
        .apply_defaults(&json!({}))
        .unwrap();
    assert_eq!(values, json!({"message":"original"}));
    let prepared = plan
        .materialize(
            "/usr/bin/printf",
            &values,
            &[],
            InputDelivery::None,
            ArgumentLimits::default(),
        )
        .unwrap();
    assert_eq!(prepared.argv(), &["%s", "original"]);
    assert!(prepared.stdin_json().is_none());
    let mut invalid = old.param_schema.clone().unwrap();
    invalid["message"]["cli"]["order"] = json!(100);
    assert!(ArgumentPlan::from_bound_contract(&invalid, &[], binding.input).is_err());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn direct_native_runtime_is_required_before_action_write() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("command_runtime")
        .create(&db)
        .await
        .unwrap();
    let runtime = RuntimeFixture::new(None, None, "native")
        .create(&db)
        .await
        .unwrap();
    assert!(ActionRepository::create(&db, input(&pack, runtime.id))
        .await
        .is_err());
    let mut missing = input(&pack, runtime.id);
    missing.runtime = None;
    assert!(ActionRepository::create(&db, missing).await.is_err());
    db.cleanup().await.unwrap();
}
