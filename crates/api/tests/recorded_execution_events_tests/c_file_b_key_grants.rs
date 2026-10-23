//! Observe File B's real reader-grant boundary after direct fixture IAM writes.
use super::*;
use attune_api::{auth::middleware::AuthenticatedUser, authz::AuthorizationService};
use attune_common::{
    key_access::origin_action_allowed_verified,
    rbac::{Action, AuthorizationContext, Grant},
    repositories::{
        iam_ownership::VerifiedKeyOrigins,
        identity::{IdentityRoleAssignmentRepository, PermissionSetRepository},
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
    },
    secret_provenance::{KeyOrigin, SecretOrigin, SecretProvenance},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FileKeyRights {
    None,
    Read,
    Decrypt,
    ReadDecrypt,
}

/// Capture the actual original descriptor before raw producer GC, not current
/// mutable Key classification or a fixture-authored substitute source proof.
pub(super) async fn original_input_key(
    fixture: &Fixture,
    execution: Id,
    key_id: Id,
) -> Result<KeyOrigin> {
    let fields = ExecutionSecretValueRepository::find_stored_by_entity(
        &fixture.ctx.pool,
        ENTITY_EXECUTION_CONFIG,
        execution,
    )
    .await?;
    let field = fields
        .iter()
        .find(|field| field.json_path == "/input")
        .ok_or("original input field unavailable")?;
    let provenance: SecretProvenance = serde_json::from_str(
        field
            .source_ref
            .as_deref()
            .ok_or("original input provenance unavailable")?,
    )?;
    let receipt = provenance
        .origins
        .iter()
        .find(|origin| {
            attune_common::repositories::immutable_input_origin::is_input_receipt(origin)
        })
        .ok_or("original input receipt unavailable")?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let context = Inputs::receipt_context(
        &mut tx,
        &InputSite {
            execution,
            kind: InputEntityKind::ExecutionConfig,
            purpose: InputSitePurpose::Value,
            path: "/input".into(),
        },
        receipt,
        fixture
            .ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .unwrap(),
    )
    .await?;
    let origin = context
        .source_origins()
        .iter()
        .find_map(|origin| match origin {
            SecretOrigin::Key(origin) if origin.key_id == key_id => Some(origin.clone()),
            _ => None,
        })
        .ok_or("original Key origin unavailable")?;
    assert!(
        origin.encrypted,
        "File B must still require original source Decrypt after current metadata changes"
    );
    context.ensure_fresh_on(&mut tx).await?;
    tx.rollback().await?;
    Ok(origin)
}

fn rights(
    grants: &[Grant],
    caller: &AuthorizationContext,
    origin: &KeyOrigin,
    ownership: &VerifiedKeyOrigins,
) -> FileKeyRights {
    match (
        origin_action_allowed_verified(grants, caller, origin, Action::Read, ownership),
        origin_action_allowed_verified(grants, caller, origin, Action::Decrypt, ownership),
    ) {
        (false, false) => FileKeyRights::None,
        (true, false) => FileKeyRights::Read,
        (false, true) => FileKeyRights::Decrypt,
        (true, true) => FileKeyRights::ReadDecrypt,
    }
}

/// The actual permission routes invalidate these same caches before publishing
/// an event. Direct repository fixture writes need that boundary explicitly.
/// Compare fresh repository rights, the token's cached snapshot, and refreshed
/// rights without assuming cache TTL, enablement, or a stale-before outcome.
pub(super) async fn refresh_and_observe(
    fixture: &Fixture,
    token: &str,
    origin: &KeyOrigin,
    expected: FileKeyRights,
) -> Result<()> {
    let user = AuthenticatedUser {
        claims: attune_common::auth::jwt::validate_token(token, &fixture.ctx.state.jwt_config)?,
    };
    let service = fixture.ctx.state.authorization_service();
    let before = service
        .load_snapshot(&user)
        .await?
        .ok_or("viewer snapshot unavailable")?;
    let mut sets =
        PermissionSetRepository::find_by_identity(&fixture.ctx.pool, before.identity_id).await?;
    let roles = IdentityRoleAssignmentRepository::find_role_names_by_identity(
        &fixture.ctx.pool,
        before.identity_id,
    )
    .await?;
    sets.extend(PermissionSetRepository::find_by_roles(&fixture.ctx.pool, &roles).await?);
    let mut seen = std::collections::HashSet::new();
    sets.retain(|set| seen.insert(set.id));
    let mut raw = Vec::<Grant>::new();
    for set in sets {
        raw.extend(serde_json::from_value::<Vec<Grant>>(set.grants)?);
    }
    AuthorizationService::invalidate_identity_authz_cache(before.identity_id).await;
    AuthorizationService::invalidate_permission_set_caches().await;
    let after = service
        .load_snapshot(&user)
        .await?
        .ok_or("refreshed viewer snapshot unavailable")?;
    assert_eq!(before.identity_id, after.identity_id);
    let mut caller = AuthorizationContext::new(after.identity_id);
    caller.identity_attributes = after.identity_attributes.clone();
    let mut tx = fixture.ctx.pool.begin().await?;
    let mut ownership = VerifiedKeyOrigins::default();
    assert!(
        ownership
            .retain_recorded(&mut tx, after.identity_id, origin)
            .await?
            .is_some(),
        "the actual original Key identity and concrete scope must survive"
    );
    let raw_rights = rights(&raw, &caller, origin, &ownership);
    let cached_before = rights(&before.grants, &caller, origin, &ownership);
    let refreshed = rights(&after.grants, &caller, origin, &ownership);
    // Finite enum observations only. Never print grants, IDs, refs, tokens,
    // provenance, encrypted material or successful plaintext response bodies.
    eprintln!("File-B Key grant boundary: raw={raw_rights:?}, cached_before={cached_before:?}, refreshed={refreshed:?}");
    assert_eq!(
        raw_rights, expected,
        "direct IAM mutation must establish the intended source rights"
    );
    assert_eq!(
        refreshed, raw_rights,
        "the real token snapshot must observe current repository rights after invalidation"
    );
    tx.rollback().await?;
    Ok(())
}
