use super::*;
use attune_api::{
    auth::{jwt::validate_token, middleware::AuthenticatedUser},
    authz::AuthorizationService,
};
use attune_common::{
    auth::jwt::{generate_access_token, generate_admitted_execution_token},
    repositories::{
        identity::{
            CreateIdentityRoleAssignmentInput, CreatePermissionSetRoleAssignmentInput,
            IdentityRoleAssignmentRepository, PermissionSetRoleAssignmentRepository,
            UpdateIdentityInput,
        },
        runtime_authority::RuntimeCredentialBinding,
        schema_type::SchemaTypeRepository,
        Delete,
    },
};
use std::time::Duration;

async fn single_connection_pool(ctx: &TestContext) -> TResult<PgPool> {
    let schema = ctx.schema.clone();
    Ok(sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(2))
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path', quote_ident($1), false)")
                    .bind(schema)
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with((*ctx.pool.connect_options()).clone())
        .await?)
}

fn principal(token: &str) -> TResult<AuthenticatedUser> {
    Ok(AuthenticatedUser {
        claims: validate_token(token, &jwt_config())?,
    })
}

#[tokio::test]
async fn transactional_access_delegation_uses_one_connection_and_canonical_live_grants(
) -> TResult<()> {
    let ctx = TestContext::new().await?;
    let pool = single_connection_pool(&ctx).await?;
    let result: TResult<()> = async {
        let identity = create_execution_identity(&ctx.pool, "transactional_access").await?;
        let pack = create_pack(&ctx.pool, "canonical_grants", "Canonical grants").await?;
        let direct = PermissionSetRepository::create(
            &ctx.pool,
            CreatePermissionSetInput {
                r#ref: "canonical_grants.read".into(),
                pack: Some(pack.id),
                pack_ref: Some(pack.r#ref.clone()),
                label: None,
                description: None,
                grants: json!([{"resource":"actions","actions":["read"]}]),
            },
        )
        .await?;
        assign_permission_set(&ctx.pool, &identity, &direct).await?;
        let role_set = create_permission_set(
            &ctx.pool,
            "transactional.role",
            json!([{"resource":"workers","actions":["read"]}]),
        )
        .await?;
        IdentityRoleAssignmentRepository::create(
            &ctx.pool,
            CreateIdentityRoleAssignmentInput {
                identity: identity.id,
                role: "transactional-reader".into(),
                source: "manual".into(),
                managed: false,
            },
        )
        .await?;
        PermissionSetRoleAssignmentRepository::create(
            &ctx.pool,
            CreatePermissionSetRoleAssignmentInput {
                permset: role_set.id,
                role: "transactional-reader".into(),
            },
        )
        .await?;
        let unrestricted = create_permission_set(
            &ctx.pool,
            "transactional.unrestricted",
            json!([
                {"resource":"actions","actions":["read"]},
                {"resource":"workers","actions":["read"]}
            ]),
        )
        .await?;
        let user = principal(&generate_access_token(identity.id, &identity.login, &jwt_config())?)?;
        let baseline = ctx.state.authorization_service().without_metadata_cache();
        // FromRow normalizes a pack-owned definition before either helper sees
        // it. The original fixture never covered unrestricted action reads.
        assert_eq!(direct.grants[0]["constraints"]["managing_pack_id"], json!(pack.id));
        assert!(!baseline.can_delegate_permission_sets(&user, std::slice::from_ref(&unrestricted.r#ref)).await?,
            "the original pool-based helper also denies this unrestricted request");
        PackRepository::set_installed_by(&ctx.pool, pack.id, identity.id).await?;
        let canonical = create_permission_set(
            &ctx.pool,
            "transactional.canonical_owner",
            json!([{"resource":"actions","actions":["read"],"constraints":{"pack_owner":"self"}}]),
        ).await?;
        assign_permission_set(&ctx.pool, &identity, &canonical).await?;
        let requested = create_permission_set(
            &ctx.pool,
            "transactional.requested",
            json!([
                {"resource":"actions","actions":["read"],"constraints":{"pack_owner":"self"}},
                {"resource":"workers","actions":["read"]}
            ]),
        ).await?;
        let snapshot = baseline.load_snapshot(&user).await?.ok_or("access snapshot missing")?;
        let requested_grants = serde_json::from_value::<Vec<attune_common::rbac::Grant>>(requested.grants.clone())?;
        assert!(!attune_common::rbac::can_delegate_grants(
            &snapshot.grants, &requested_grants, &attune_common::rbac::AuthorizationContext::new(identity.id),
        ), "resolved target-matching copies cannot substitute for canonical owner-relative grants");
        assert!(baseline.can_delegate_permission_sets_with_snapshot(
            &user, Some(&snapshot), std::slice::from_ref(&requested.r#ref),
        ).await?, "the original helper delegates matching canonical pack_owner grants");
        // Leave caches enabled and unprimed. Any pool/cache-namespace/shadow
        // acquisition inside the check must fail with this transaction held.
        let authz = AuthorizationService::new_with_audit(pool.clone(), ctx.state.audit_emitter.clone());
        let mut tx = pool.begin().await?;
        SchemaTypeRepository::lock_selection(&mut tx).await?;
        // Late delegation must not introduce identity row locks after the
        // caller's execution protection. A conflicting update remains open.
        let mut identity_update = ctx.pool.begin().await?;
        sqlx::query("SELECT id FROM identity WHERE id=$1 FOR UPDATE")
            .bind(identity.id)
            .execute(&mut *identity_update)
            .await?;
        let checked = tokio::time::timeout(
            Duration::from_secs(5),
            authz.can_delegate_permission_sets_on(
                &mut tx,
                &user,
                std::slice::from_ref(&requested.r#ref),
            ),
        )
        .await;
        identity_update.rollback().await?;
        assert!(checked??, "delegation must retain its nonlocking reads");
        tokio::time::timeout(Duration::from_secs(15), async {
            assert!(authz.can_delegate_permission_sets_on(&mut tx, &user, std::slice::from_ref(&requested.r#ref)).await?,
                "human delegation must retain canonical pack_owner grants, not resolved target copies");
            assert!(!authz.can_delegate_permission_sets_on(&mut tx, &user, std::slice::from_ref(&unrestricted.r#ref)).await?);
            assert!(!authz.can_delegate_permission_sets_on(&mut tx, &user, &["transactional.missing".into()]).await?);
            assert!(authz.can_delegate_permission_sets_on(&mut tx, &user, &[STANDARD_EXECUTION_ACCESS_REF.into()]).await?);
            let mut missing = user.clone();
            missing.claims.sub = i64::MAX.to_string();
            assert!(authz.can_delegate_permission_sets_on(&mut tx, &missing, std::slice::from_ref(&requested.r#ref)).await.is_err());
            // Uncommitted changes must be visible without cache invalidation.
            IdentityRepository::update(&mut *tx, identity.id, UpdateIdentityInput {
                frozen: Some(true), ..Default::default()
            }).await?;
            assert!(authz.can_delegate_permission_sets_on(&mut tx, &user, &[requested.r#ref]).await.is_err());
            Ok::<_, Box<dyn std::error::Error>>(())
        }).await??;
        tx.rollback().await?;
        Ok(())
    }.await;
    pool.close().await;
    ctx.cleanup().await?;
    result
}

#[tokio::test]
async fn transactional_execution_delegation_preserves_token_ceiling_and_current_identity_coverage(
) -> TResult<()> {
    let ctx = TestContext::new().await?;
    let pool = single_connection_pool(&ctx).await?;
    let result: TResult<()> = async {
        let identity = create_execution_identity(&ctx.pool, "transactional_execution").await?;
        let read = create_permission_set(&ctx.pool, "transactional.read", json!([
            {"resource":"runtimes","actions":["read"]}
        ])).await?;
        let alias = create_permission_set(&ctx.pool, "transactional.alias", read.grants.clone()).await?;
        let broader = create_permission_set(&ctx.pool, "transactional.broader", json!([
            {"resource":"workers","actions":["read"]}
        ])).await?;
        for set in [&read, &broader] {
            assign_permission_set(&ctx.pool, &identity, set).await?;
        }
        let standard_target = create_permission_set(&ctx.pool, "transactional.standard_target", json!([
            {"resource":"caches","actions":["read"],"constraints":{"owner_types":["pack"],"owner_refs":["fixture"]}}
        ])).await?;
        let foreign_standard = create_permission_set(&ctx.pool, "transactional.foreign_standard", json!([
            {"resource":"caches","actions":["read"],"constraints":{"owner_types":["pack"],"owner_refs":["foreign"]}}
        ])).await?;
        let standard_write = create_permission_set(&ctx.pool, "transactional.standard_write", json!([
            {"resource":"caches","actions":["update"],"constraints":{"owner_types":["pack"],"owner_refs":["fixture"]}}
        ])).await?;
        let pack = create_pack(&ctx.pool, "fixture", "Transactional execution").await?;
        let action = create_action_with_default_access(&ctx.pool, &pack, "run", Vec::new()).await?;
        helpers::activate_test_pack_release(&ctx.pool, &pack).await?;
        let refs = vec![read.r#ref.clone(), STANDARD_EXECUTION_ACCESS_REF.into()];
        let execution = create_pinned_ad_hoc_parent(&ctx.pool, &action, &identity, &refs).await?;
        let user = principal(&execution_token(&ctx.pool, &identity, &execution, &action, &refs).await?)?;
        let recorded = principal(&generate_admitted_execution_token(
            &RuntimeCredentialBinding {
                execution_id: 11, admission_id: 12, sponsor_identity_id: identity.id,
                action_ref: "fixture.run".into(), attempt: uuid::Uuid::new_v4(),
            }, std::slice::from_ref(&read.r#ref), &jwt_config(), Some(600),
        )?)?;
        let authz = AuthorizationService::new_with_audit(pool.clone(), ctx.state.audit_emitter.clone());
        let mut tx = pool.begin().await?;
        SchemaTypeRepository::lock_selection(&mut tx).await?;
        tokio::time::timeout(Duration::from_secs(15), async {
            assert!(authz.can_delegate_permission_sets_on(&mut tx, &user, std::slice::from_ref(&alias.r#ref)).await?,
                "the ceiling compares grants, not just ref-name membership");
            assert!(!authz.can_delegate_permission_sets_on(&mut tx, &user, &[broader.r#ref]).await?,
                "an execution cannot inherit its identity's additional grants");
            assert!(authz.can_delegate_permission_sets_on(&mut tx, &user, &[standard_target.r#ref]).await?,
                "standard access remains scoped independently of named IAM grants");
            assert!(!authz.can_delegate_permission_sets_on(&mut tx, &user, &[foreign_standard.r#ref]).await?,
                "standard access cannot delegate another pack's cache reads");
            assert!(!authz.can_delegate_permission_sets_on(&mut tx, &user, &[standard_write.r#ref]).await?,
                "standard cache access cannot delegate writes");
            let mut unavailable = user.clone();
            unavailable.claims.metadata.as_mut().unwrap()["permission_set_refs"] = json!(["transactional.missing"]);
            assert!(authz.can_delegate_permission_sets_on(&mut tx, &unavailable, std::slice::from_ref(&alias.r#ref)).await.is_err());
            assert!(authz.can_delegate_permission_sets_on(&mut tx, &recorded, std::slice::from_ref(&alias.r#ref)).await.is_err());
            let assignments = PermissionAssignmentRepository::find_by_identity(&mut *tx, identity.id).await?;
            let assignment = assignments.iter().find(|assignment| assignment.permset == read.id).unwrap();
            PermissionAssignmentRepository::delete(&mut *tx, assignment.id).await?;
            assert!(authz.can_delegate_permission_sets_on(&mut tx, &user, std::slice::from_ref(&alias.r#ref)).await.is_err(),
                "the token's named grants must still be covered by its current identity");
            PermissionAssignmentRepository::create(&mut *tx, CreatePermissionAssignmentInput {
                identity: identity.id, permset: read.id,
            }).await?;
            assert!(authz.can_delegate_permission_sets_on(&mut tx, &user, std::slice::from_ref(&alias.r#ref)).await?);
            IdentityRepository::update(&mut *tx, identity.id, UpdateIdentityInput {
                frozen: Some(true), ..Default::default()
            }).await?;
            assert!(authz.can_delegate_permission_sets_on(&mut tx, &user, &[alias.r#ref]).await.is_err());
            Ok::<_, Box<dyn std::error::Error>>(())
        }).await??;
        tx.rollback().await?;
        Ok(())
    }.await;
    pool.close().await;
    ctx.cleanup().await?;
    result
}
