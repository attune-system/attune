mod helpers;
mod managed_readmission_fixture;
use attune_common::repositories::{
    execution::ExecutionRepository,
    runtime::{CreateRuntimeInput, RuntimeRepository},
    schema_contract::{BindingOwner, SchemaContractRepository},
    Create, FindById,
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::{json, Value};

#[tokio::test]
async fn cli_metadata_round_trips_and_manual_execution_binds_defaults() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    // with_admin_auth grants pack/identity administration, not action execution.
    let identity = ctx.user.as_ref().unwrap().id;
    let rights = attune_common::repositories::identity::PermissionSetRepository::create(
        &ctx.pool,
        attune_common::repositories::identity::CreatePermissionSetInput {
            r#ref: "cli_http.author".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"actions","actions":["read","create","update","execute"],"constraints":{"refs":["cli_http.run"]}}]),
        },
    ).await?;
    attune_common::repositories::identity::PermissionAssignmentRepository::create(
        &ctx.pool,
        attune_common::repositories::identity::CreatePermissionAssignmentInput {
            identity,
            permset: rights.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let pack = helpers::create_test_pack(&ctx.pool, "cli_http").await?;
    helpers::activate_test_pack_release(&ctx.pool, &pack).await?;
    let runtime = RuntimeRepository::create(
        &ctx.pool,
        CreateRuntimeInput {
            r#ref: "cli_http.native".into(),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            description: None,
            name: "native".into(),
            aliases: vec![],
            distributions: json!({}),
            installation: None,
            execution_config: json!({}),
            auto_detected: false,
            detection_config: json!({}),
        },
    )
    .await?;
    let launch =
        json!({"type":"command","program":"printf","arguments":[{"literal":"%s","order":0}]});
    let schema = json!({"message":{"type":"string","default":"HTTP default","cli":{"kind":"positional","order":10}}});
    let response = ctx.post("/api/v1/actions",json!({"ref":"cli_http.run","pack_ref":pack.r#ref,"label":"CLI HTTP","runtime":runtime.id,"launch":launch,"param_schema":schema}),ctx.token()).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(body["data"]["entrypoint"].is_null());
    assert_eq!(body["data"]["launch"], launch);
    assert_eq!(body["data"]["parameter_delivery"], "none");
    assert_eq!(body["data"]["argument_preview"][1]["parameter"], "message");
    let path = "/api/v1/actions/cli_http.run";
    for patch in [
        json!({"entrypoint":"run"}),
        json!({"launch":null}),
        json!({"param_schema":{"message":{"type":"string","secret":true,"cli":{"kind":"positional","order":10}}}}),
    ] {
        let response = ctx.put(path, patch, ctx.token()).await?;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{}",
            response.text().await?
        );
    }
    let saved: Value = ctx
        .put(path, json!({"label":"Renamed"}), ctx.token())
        .await?
        .assert_status(StatusCode::OK)
        .json()
        .await?;
    assert_eq!(saved["data"]["launch"], launch);
    assert_eq!(saved["data"]["param_schema"], schema);
    let response = ctx
        .post(
            "/api/v1/executions/execute",
            json!({"action_ref":"cli_http.run","parameters":{}}),
            ctx.token(),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert!(status.is_success(), "{status}: {body}");
    let id = body["data"]["id"].as_i64().unwrap();
    let execution = ExecutionRepository::get_by_id(&ctx.pool, id).await?;
    assert_eq!(execution.config, Some(json!({"message":"HTTP default"})));
    assert_eq!(
        serde_json::to_value(
            execution
                .executable_snapshot
                .unwrap()
                .executable
                .action
                .launch
        )?,
        launch
    );
    let binding =
        SchemaContractRepository::find_with_executor(&ctx.pool, BindingOwner::Execution, id)
            .await?
            .unwrap();
    assert!(binding
        .input
        .annotated_schema
        .to_string()
        .contains("positional"));
    // Explicit source replacement, including null, is one validated patch.
    let response = ctx.put(path,json!({"launch":null,"entrypoint":"run","param_schema":{},"parameter_delivery":"stdin"}),ctx.token()).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["launch"].is_null());
    assert_eq!(body["data"]["entrypoint"], "run");
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn production_pack_registration_preserves_named_type_cli_and_resolved_delivery() -> Result<()>
{
    use attune_common::repositories::{
        action::ActionRepository, executable_snapshot::ExecutableSnapshotRepository, FindByRef,
    };
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let pack = helpers::create_test_pack(&ctx.pool, "cli_deploy").await?;
    let source = ctx.test_packs_dir.join("cli-deploy-source");
    for directory in ["actions", "runtimes", "types"] {
        std::fs::create_dir_all(source.join(directory))?;
    }
    std::fs::write(
        source.join("pack.yaml"),
        "ref: cli_deploy\nversion: 1.0.0\nconf_schema: {}\nconfig: {}\n",
    )?;
    std::fs::write(
        source.join("runtimes/native.yaml"),
        "ref: cli_deploy.native\nname: Native\naliases: [native]\nexecution_config: {}\n",
    )?;
    std::fs::write(
        source.join("types/message.yaml"),
        "ref: cli_deploy.message\nlabel: Message\nschema: {type: string, minLength: 1}\n",
    )?;
    std::fs::write(source.join("actions/run.yaml"),"ref: cli_deploy.run\nrunner_type: cli_deploy.native\nlaunch: {type: command, program: printf, arguments: [{literal: '%s', order: 0}]}\nparameters:\n  message:\n    type: cli_deploy.message\n    default: deployed\n    cli: {kind: positional, order: 10}\n")?;
    let actor = ctx.user.as_ref().unwrap().id;
    managed_readmission_fixture::deploy(&ctx, &pack, &source, actor).await?;
    let action = ActionRepository::find_by_ref(&ctx.pool, "cli_deploy.run")
        .await?
        .unwrap();
    assert!(action.entrypoint.is_none());
    assert_eq!(
        action.parameter_delivery,
        attune_common::models::ParameterDelivery::None
    );
    assert_eq!(
        action.param_schema.as_ref().unwrap()["message"]["cli"]["order"],
        10
    );
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&ctx.pool, action.id).await?;
    let mut tx = ctx.pool.begin().await?;
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
    tx.commit().await?;
    let plan = attune_common::action_cli::ArgumentPlan::from_bound_contract(
        action.param_schema.as_ref().unwrap(),
        action.launch.as_ref().unwrap().action_arguments()?,
        binding.input.clone(),
    )?;
    let value = attune_common::value_schema::CompiledContract::new(&binding.input)?
        .apply_defaults(&json!({}))?;
    let prepared = plan.materialize(
        "/usr/bin/printf",
        &value,
        &[],
        attune_common::action_cli::InputDelivery::None,
        Default::default(),
    )?;
    assert_eq!(prepared.argv(), &["%s", "deployed"]);
    ctx.cleanup().await?;
    Ok(())
}
