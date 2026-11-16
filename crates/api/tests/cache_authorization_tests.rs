//! A pack-published fixture for the delegated managed-cache reference mismatch.
//! The success assertion intentionally fails on affected platform revisions.

use anyhow::Context;
use attune_api::authz::AuthorizationService;
use attune_common::{
    audit::{AuditEventFilters, AuditRepository},
    repositories::{
        action::ActionRepository,
        cache::{CacheNamespaceRepository, CacheOwnerScope},
        delegated_execution::DelegatedExecutionRepository,
        execution::CreateExecutionInput,
        identity::{
            CreatePermissionAssignmentInput, IdentityRepository, PermissionAssignmentRepository,
            PermissionSetRepository,
        },
        pack::PackRepository,
        runtime_authority::RuntimeAuthorityRepository,
        runtime_target::{RuntimeTarget, RuntimeTargetRepository},
        Create, FindByRef,
    },
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::{json, Value};

mod helpers;

const PACK: &str = "cache_auth_repro";
const WRITER: &str = "cache_auth_repro.cache_writer";

struct PublishedFixture {
    pack: attune_common::models::Pack,
    action: attune_common::models::Action,
    sponsor: attune_common::models::Identity,
    namespace: attune_common::models::cache::CacheNamespace,
    access_token: String,
}

// Return errors instead of panicking so the caller can await owned teardown.
// Match helpers::Result's boxed error rather than anyhow::Result.
macro_rules! ensure {
    ($condition:expr $(,)?) => {
        ensure!($condition, "check failed: {}", stringify!($condition));
    };
    ($condition:expr, $($message:tt)*) => {
        if !$condition {
            return Err(std::io::Error::other(format!($($message)*)).into());
        }
    };
}

#[path = "cache_authorization_tests/coverage.rs"]
mod coverage;

async fn fixture_context() -> Result<TestContext> {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cache_authorization_repro");
    TestContext::new()
        .await?
        .with_local_source_roots(vec![source])
        .await?
        .with_admin_auth()
        .await
}

#[tokio::test]
async fn delegated_managed_cache_generation_uses_declaration_ref() -> Result<()> {
    let ctx = fixture_context().await?;
    let result = reproduce(&ctx).await;
    // Await the audit writer, background tasks and owned database teardown even
    // when the expected-success assertion returns the reproduction failure.
    ctx.cleanup().await?;
    result
}

async fn publish_fixture(ctx: &TestContext) -> Result<PublishedFixture> {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cache_authorization_repro");
    let published = ctx
        .post(
            "/api/v1/packs/register",
            json!({"path": fixture, "skip_tests": true}),
            ctx.token(),
        )
        .await?;
    let publication_status = published.status();
    let publication: Value = published.json().await?;
    ensure!(
        publication_status == StatusCode::CREATED,
        "fixture publication failed: {publication_status}: {publication}"
    );

    let pack = PackRepository::find_by_ref(&ctx.pool, PACK)
        .await?
        .context("published pack missing")?;
    let action = ActionRepository::find_by_ref(&ctx.pool, "cache_auth_repro.probe")
        .await?
        .context("published action missing")?;
    let writer = PermissionSetRepository::find_by_ref(&ctx.pool, WRITER)
        .await?
        .context("published writer missing")?;
    ensure!(writer
        .grants
        .as_array()
        .is_some_and(|grants| grants.len() == 1));
    ensure!(
        writer.grants[0]["constraints"]["refs"]
            == json!(["cache_auth_repro.users", "cache_auth_repro.control"])
    );
    let effective_grants =
        attune_common::runtime_authority::normalize_iam_grants(writer.grants.clone(), writer.pack)?;
    ensure!(effective_grants[0]["constraints"]["managing_pack_id"] == pack.id);

    // The publisher is used only for setup. Runtime requests use a separate
    // sponsor holding exactly this pack's scoped writer, never administrator IAM.
    let registered = ctx
        .post(
            "/auth/register",
            json!({
                "login": "cache-repro-sponsor",
                "password": "SyntheticTestPassword123!",
                "display_name": "Synthetic cache writer"
            }),
            None,
        )
        .await?;
    ensure!(
        registered.status().is_success(),
        "sponsor registration failed"
    );
    let registration: Value = registered.json().await?;
    let access_token = registration["data"]["access_token"]
        .as_str()
        .context("sponsor access token missing")?
        .to_owned();
    let sponsor = IdentityRepository::find_by_login(&ctx.pool, "cache-repro-sponsor")
        .await?
        .context("sponsor missing")?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: sponsor.id,
            permset: writer.id,
        },
    )
    .await?;
    AuthorizationService::invalidate_identity_authz_cache(sponsor.id).await;
    AuthorizationService::invalidate_permission_set_caches().await;

    let scope = CacheOwnerScope::pack(pack.id, Some(PACK.into()));
    let mut tx = ctx.pool.begin().await?;
    let namespace =
        CacheNamespaceRepository::resolve_including_tombstoned(&mut *tx, &scope, "repro.users")
            .await?
            .context("managed namespace missing")?;
    ensure!(namespace.definition_ref.as_deref() == Some("cache_auth_repro.users"));
    ensure!(namespace.active_generation.is_none());
    ensure!(namespace.retired_at.is_none() && namespace.tombstoned_at.is_none());
    let policy = RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
        .await?
        .context("publication did not admit a runtime policy")?;
    ensure!(
        policy.policy.concrete_targets.values().any(|binding| {
            binding.targets.as_ref().is_some_and(|targets| {
                targets
                    .iter()
                    .any(|target| target.kind == "cache_namespace" && target.id == namespace.id)
            })
        }),
        "admission did not bind the declaration-ref selector to the managed namespace"
    );
    let resolved = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::CacheNamespace(namespace.id),
        sponsor.id,
    )
    .await?
    .context("runtime cache target missing")?;
    ensure!(resolved.context().target_ref.as_deref() == Some("cache_auth_repro.users"));
    tx.commit().await?;

    Ok(PublishedFixture {
        pack,
        action,
        sponsor,
        namespace,
        access_token,
    })
}

async fn reproduce(ctx: &TestContext) -> Result<()> {
    let PublishedFixture {
        action,
        sponsor,
        namespace,
        ..
    } = publish_fixture(ctx).await?;
    let selected_refs = action.default_execution_permission_set_refs.clone();
    ensure!(selected_refs == ["standard", WRITER]);
    let execution = helpers::delegated_source::create(
        &ctx.pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref,
            executor: Some(sponsor.id),
            permission_set_refs: selected_refs,
            ..Default::default()
        },
    )
    .await?;
    let mut tx = ctx.pool.begin().await?;
    let authority = DelegatedExecutionRepository::find(&mut tx, execution.id).await?;
    ensure!(authority.mode == "delegated" && authority.admission.is_none());
    tx.commit().await?;
    let token = helpers::delegated_source::execution_token(ctx, execution.id).await?;
    println!("fixture published; declaration selector bound to namespace {}; authority=delegated; admission=NULL; refs={:?}", namespace.id, execution.permission_set_refs);

    let read = ctx
        .get(
            "/api/v1/cache/namespaces/repro.users?owner_type=pack&owner_ref=cache_auth_repro",
            Some(&token),
        )
        .await?;
    ensure!(
        read.status() == StatusCode::OK,
        "standard metadata read failed"
    );
    println!("GET repro.users: {}", read.status());

    let mut affected_status = StatusCode::INTERNAL_SERVER_ERROR;
    for name in ["cache_auth_repro.control", "repro.users"] {
        let response = ctx
            .post(
                &format!("/api/v1/cache/namespaces/{name}/generations"),
                json!({
                    "owner_type": "pack",
                    "owner_ref": PACK,
                    "client_refresh_id": format!("auth-repro-{}", uuid::Uuid::new_v4().simple()),
                    "expected_active_generation_id": null,
                    "expected_chunk_count": 0,
                    "expected_record_count": 0
                }),
                Some(&token),
            )
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        // These responses contain only this fixture's empty generation metadata
        // or sanitized API errors. Never print either authentication token.
        println!("POST {name}/generations: {status}; response={body}");
        if name == "cache_auth_repro.control" {
            ensure!(
                status == StatusCode::CREATED,
                "same-name control failed: {body}"
            );
        } else {
            affected_status = status;
        }
    }

    ctx.flush_audit().await?;
    let denials = AuditRepository::search(
        &ctx.pool,
        &AuditEventFilters {
            event_type: Some("rbac.denied".into()),
            actor_identity: Some(sponsor.id),
            resource_type: Some("caches".into()),
            limit: Some(10),
            ..Default::default()
        },
    )
    .await?;
    for denial in &denials {
        println!(
            "RBAC denial: {}",
            json!({
                "actor_token_type": denial.actor_token_type,
                "resource_ref": denial.resource_ref,
                "details": denial.details
            })
        );
    }
    if affected_status == StatusCode::FORBIDDEN {
        ensure!(
            denials.iter().any(|denial| {
                denial.actor_token_type.as_deref() == Some("execution")
                    && denial.resource_ref.as_deref() == Some("repro.users")
                    && denial.details.as_ref().is_some_and(|details| {
                        details["action"] == "create"
                            && details["reason"] == "grant_not_found_or_constraints_not_matched"
                    })
            }),
            "403 did not carry the reported namespace-reference RBAC denial"
        );
    }
    ensure!(
        affected_status == StatusCode::CREATED,
        "managed declaration-ref grant passed publication but generation creation returned {affected_status}; expected 201 Created"
    );
    Ok(())
}
