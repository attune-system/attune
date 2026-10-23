mod helpers;
mod managed_readmission_fixture;

use attune_api::{
    auth::middleware::{AuthenticatedUser, RequireAuth},
    authz::AuthorizationService,
    middleware::ApiError,
};
use attune_common::{
    auth::jwt::{
        generate_execution_token_with_permission_sets, generate_sensor_token,
        generate_worker_token, validate_token,
    },
    repositories::{
        identity::{
            CreateIdentityRoleAssignmentInput, CreatePermissionAssignmentInput,
            CreatePermissionSetInput, IdentityRoleAssignmentRepository,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        managed_sensor_metadata::ManagedSensorMetadataRepository,
        pack::PackRepository,
        trigger::SensorRepository,
        Create, Delete, FindByRef,
    },
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use helpers::{Result, TestContext};
use reqwest::StatusCode;
use serde_json::{json, Value};

struct Actor {
    identity: i64,
    token: String,
    permission_ref: String,
    permission_id: i64,
}

async fn actor(ctx: &TestContext, grants: Value) -> Result<Actor> {
    let name = format!("owner_role_{}", uuid::Uuid::new_v4().simple());
    let response = ctx
        .post(
            "/auth/register",
            json!({"login":name,"password":"TestPassword123!"}),
            None,
        )
        .await?;
    assert!(response.status().is_success());
    let body: Value = response.json().await?;
    let token = body["data"]["access_token"]
        .as_str()
        .ok_or("access token missing")?
        .to_owned();
    let identity = validate_token(&token, &ctx.state.jwt_config)?.sub.parse()?;
    let permission_ref = format!("fixture.{name}");
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: permission_ref.clone(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants,
        },
    )
    .await?;
    assign(ctx, identity, set.id).await?;
    Ok(Actor {
        identity,
        token,
        permission_ref,
        permission_id: set.id,
    })
}

async fn assign(ctx: &TestContext, identity: i64, permission: i64) -> Result<()> {
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: permission,
        },
    )
    .await?;
    AuthorizationService::invalidate_identity_authz_cache(identity).await;
    Ok(())
}

async fn membership(ctx: &TestContext, identity: i64) -> Result<i64> {
    Ok(IdentityRoleAssignmentRepository::create(
        &ctx.pool,
        CreateIdentityRoleAssignmentInput {
            identity,
            role: "sensor-maintainers".into(),
            source: "test".into(),
            managed: false,
        },
    )
    .await?
    .id)
}

fn nonaccess_tokens(ctx: &TestContext, actor: &Actor) -> Result<Vec<String>> {
    Ok(vec![
        generate_execution_token_with_permission_sets(
            actor.identity,
            1234,
            "role_sensor.source",
            &ctx.state.jwt_config,
            Some(300),
            std::slice::from_ref(&actor.permission_ref),
        )?,
        generate_worker_token(
            actor.identity,
            "role-control-worker",
            &ctx.state.jwt_config,
            Some(300),
        )?,
    ])
}

fn sensor_user(ctx: &TestContext, actor: &Actor) -> Result<AuthenticatedUser> {
    let token = generate_sensor_token(
        actor.identity,
        "role_sensor.source",
        vec!["role_sensor.tick".into()],
        &ctx.state.jwt_config,
        Some(300),
    )?;
    Ok(AuthenticatedUser {
        claims: validate_token(&token, &ctx.state.jwt_config)?,
    })
}

#[tokio::test]
async fn managed_sensor_owner_roles_authorize_update_and_revocation_only_for_current_humans(
) -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let owner = ctx.user.as_ref().ok_or("owner missing")?.id;
    let pack = helpers::create_test_pack(&ctx.pool, "role_sensor").await?;
    PackRepository::set_installed_by(&ctx.pool, pack.id, owner).await?;
    let source = ctx.test_packs_dir.join("role_sensor");
    for directory in ["runtimes", "sensors", "triggers"] {
        std::fs::create_dir_all(source.join(directory))?;
    }
    for (path, content) in [
        ("pack.yaml", "ref: role_sensor\nversion: '1.0.0'\nconf_schema: {}\nconfig: {}\n"),
        ("runtimes/native.yaml", "ref: role_sensor.native\nname: role_sensor_native\nexecution_config: {}\n"),
        ("sensors/source.yaml", "ref: role_sensor.source\nrunner_type: role_sensor.native\nentry_point: source.sh\ntrigger_types: [role_sensor.tick]\n"),
        ("sensors/source.sh", "#!/bin/sh\nexit 0\n"),
        ("triggers/tick.yaml", "ref: role_sensor.tick\nreference_visibility: public\n"),
    ] {
        std::fs::write(source.join(path), content)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            source.join("sensors/source.sh"),
            std::fs::Permissions::from_mode(0o755),
        )?;
    }
    managed_readmission_fixture::deploy(&ctx, &pack, &source, owner).await?;
    let mut tx = ctx.pool.begin().await?;
    PackRepository::set_owner_roles(&mut tx, pack.id, vec!["sensor-maintainers".into()]).await?;
    tx.commit().await?;
    let grants = json!([{
        "resource":"triggers", "actions":["update"],
        "constraints":{"pack_owner":"self", "refs":["role_sensor.source"]}
    }]);
    let member = actor(&ctx, grants.clone()).await?;
    let outsider = actor(&ctx, grants).await?;
    let scoped_elsewhere = actor(
        &ctx,
        json!([{
            "resource":"triggers", "actions":["update"],
            "constraints":{"pack_owner":"self", "refs":["role_sensor.other"]}
        }]),
    )
    .await?;
    membership(&ctx, scoped_elsewhere.identity).await?;
    assign(&ctx, owner, member.permission_id).await?;
    let member_row = membership(&ctx, member.identity).await?;
    let path = "/api/v1/sensors/role_sensor.source";
    let revoke = format!("{path}/revoke-runtime");
    let sensor = SensorRepository::find_by_ref(&ctx.pool, "role_sensor.source")
        .await?
        .ok_or("sensor missing")?;
    let mut tx = ctx.pool.begin().await?;
    let revision = ManagedSensorMetadataRepository::current_revision(&mut *tx, sensor.id)
        .await?
        .ok_or("managed revision missing")?;
    tx.rollback().await?;
    // Test the handler's own credential gate independently of sensor-fence
    // middleware. Even a sensor credential naming the member cannot borrow roles.
    assert!(matches!(
        attune_api::routes::triggers::update_sensor(
            State(ctx.state.clone()),
            RequireAuth(sensor_user(&ctx, &member)?),
            Path("role_sensor.source".into()),
            Json(serde_json::from_value(json!({"label":"Sensor denied"}))?),
        )
        .await,
        Err(ApiError::Forbidden(_))
    ));
    assert!(matches!(
        attune_api::routes::triggers::revoke_sensor_runtime(
            State(ctx.state.clone()),
            RequireAuth(sensor_user(&ctx, &member)?),
            Path("role_sensor.source".into()),
            Json(serde_json::from_value(
                json!({"metadata_revision":revision})
            )?),
        )
        .await,
        Err(ApiError::Forbidden(_))
    ));
    for token in [outsider.token.clone(), scoped_elsewhere.token.clone()]
        .into_iter()
        .chain(nonaccess_tokens(&ctx, &member)?)
    {
        assert_eq!(
            ctx.put(path, json!({"label":"Denied"}), Some(&token))
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            ctx.post(&revoke, json!({"metadata_revision":revision}), Some(&token))
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    for token in [
        member.token.as_str(),
        ctx.token().ok_or("owner token missing")?,
    ] {
        let response = ctx
            .put(path, json!({"label":"Authorized"}), Some(token))
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            response.text().await?
        );
        let response = ctx
            .post(&revoke, json!({"metadata_revision":revision}), Some(token))
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            response.text().await?
        );
    }
    IdentityRoleAssignmentRepository::delete(&ctx.pool, member_row).await?;
    // Deliberately leave grant/role caches intact. Ownership membership is fresh.
    assert_eq!(
        ctx.put(path, json!({"label":"Revoked member"}), Some(&member.token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ctx.post(
            &revoke,
            json!({"metadata_revision":revision}),
            Some(&member.token)
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        SensorRepository::find_by_ref(&ctx.pool, "role_sensor.source")
            .await?
            .ok_or("sensor missing")?
            .label,
        "Authorized"
    );
    ctx.cleanup().await?;
    Ok(())
}

async fn approval_owner_role_access(selector: &str) -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let owner = ctx.user.as_ref().ok_or("owner missing")?.id;
    let pack = helpers::create_test_pack(&ctx.pool, "role_approval").await?;
    PackRepository::set_installed_by(&ctx.pool, pack.id, owner).await?;
    let mut tx = ctx.pool.begin().await?;
    PackRepository::set_owner_roles(&mut tx, pack.id, vec!["sensor-maintainers".into()]).await?;
    tx.commit().await?;
    let mut constraints = json!({"refs":["role_approval"]});
    constraints[selector] = json!("self");
    let grants = json!([{"resource":"packs","actions":["read"],"constraints":constraints}]);
    let member = actor(&ctx, grants.clone()).await?;
    let outsider = actor(&ctx, grants).await?;
    assign(&ctx, owner, member.permission_id).await?;
    let member_row = membership(&ctx, member.identity).await?;
    let path = "/api/v1/packs/role_approval/external-approvals";
    assert!(matches!(
        attune_api::routes::runtime_approvals::list_runtime_approvals(
            State(ctx.state.clone()),
            RequireAuth(sensor_user(&ctx, &member)?),
            Path("role_approval".into()),
            Query(serde_json::from_value(json!({"page":1,"page_size":10}))?),
        )
        .await,
        Err(ApiError::Forbidden(_))
    ));
    for token in std::iter::once(outsider.token.clone()).chain(nonaccess_tokens(&ctx, &member)?) {
        assert_eq!(
            ctx.get(path, Some(&token)).await?.status(),
            StatusCode::FORBIDDEN
        );
    }
    for token in [
        member.token.as_str(),
        ctx.token().ok_or("owner token missing")?,
    ] {
        let response = ctx.get(path, Some(token)).await?;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            response.text().await?
        );
    }
    // Owner-role pack access is not consent to publish arbitrary runtime grants.
    assert_eq!(
        ctx.post(
            path,
            json!({"grants":[{"resource":"events","actions":["create"]}]}),
            Some(&member.token)
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    IdentityRoleAssignmentRepository::delete(&ctx.pool, member_row).await?;
    assert_eq!(
        ctx.get(path, Some(&member.token)).await?.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(ctx.get(path, ctx.token()).await?.status(), StatusCode::OK);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn approval_owner_self_access_uses_current_declared_roles() -> Result<()> {
    approval_owner_role_access("owner").await
}

#[tokio::test]
async fn approval_pack_owner_self_access_uses_current_declared_roles() -> Result<()> {
    approval_owner_role_access("pack_owner").await
}
