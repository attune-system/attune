mod helpers;

use attune_common::{
    key_access::{key_origin, origin_action_allowed_verified},
    rbac::{Action, AuthorizationContext, Grant, Resource},
    repositories::{
        iam_ownership::{IamOwnershipRepository, VerifiedKeyOrigins},
        key::KeyRepository,
        runtime_target::ComponentTarget,
        Delete,
    },
    runtime_authority::normalize_iam_grants,
};
use helpers::{create_test_pool, ActionFixture, KeyFixture, PackFixture};
use serde_json::json;

#[tokio::test]
async fn verified_key_origins_bind_exact_ids_owners_and_retained_tuples() {
    let db = create_test_pool().await.unwrap();
    let local = PackFixture::new_unique("origin_local")
        .create(&db)
        .await
        .unwrap();
    let foreign = PackFixture::new_unique("origin_foreign")
        .create(&db)
        .await
        .unwrap();
    let key = KeyFixture::new_pack(local.id, &local.r#ref, "value", "data")
        .create(&db)
        .await
        .unwrap();
    let foreign_key = KeyFixture::new_pack(foreign.id, &foreign.r#ref, "value", "data")
        .create(&db)
        .await
        .unwrap();
    let system_key = KeyFixture::new_system_unique("origin_system", "data")
        .create(&db)
        .await
        .unwrap();
    let grants: Vec<Grant> = serde_json::from_value(
        normalize_iam_grants(
            json!([
                {"resource":"keys", "actions":["read"]}
            ]),
            Some(local.id),
        )
        .unwrap(),
    )
    .unwrap();
    let caller = AuthorizationContext::new(1);
    let ownership = VerifiedKeyOrigins::load(&*db, &[key.id, foreign_key.id, system_key.id])
        .await
        .unwrap();
    let origin = key_origin(&key);
    assert!(origin_action_allowed_verified(
        &grants,
        &caller,
        &origin,
        Action::Read,
        &ownership
    ));
    for other in [&foreign_key, &system_key] {
        assert!(!origin_action_allowed_verified(
            &grants,
            &caller,
            &key_origin(other),
            Action::Read,
            &ownership
        ));
    }
    let mut forged = origin.clone();
    forged.owner_ref = Some(foreign.r#ref.clone());
    assert!(!origin_action_allowed_verified(
        &grants,
        &caller,
        &forged,
        Action::Read,
        &ownership
    ));
    KeyRepository::delete(&*db, key.id).await.unwrap();
    let replacement = KeyFixture::new_pack(local.id, &local.r#ref, "value", "new")
        .create(&db)
        .await
        .unwrap();
    assert_ne!(replacement.id, key.id);
    let ownership = VerifiedKeyOrigins::load(&*db, &[key.id, replacement.id])
        .await
        .unwrap();
    assert_eq!(ownership.managing_pack(&origin), None);
    assert!(!origin_action_allowed_verified(
        &grants,
        &caller,
        &origin,
        Action::Read,
        &ownership
    ));
    // Historical broad rights keep their original semantics after deletion.
    let global = vec![Grant {
        resource: Resource::Keys,
        actions: vec![Action::Read],
        constraints: None,
    }];
    assert!(origin_action_allowed_verified(
        &global,
        &caller,
        &origin,
        Action::Read,
        &ownership
    ));
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn component_ownership_is_batched_and_keeps_foreign_pack_ids_distinct() {
    let db = create_test_pool().await.unwrap();
    let local = PackFixture::new_unique("batch_local")
        .create(&db)
        .await
        .unwrap();
    let foreign = PackFixture::new_unique("batch_foreign")
        .create(&db)
        .await
        .unwrap();
    let a = ActionFixture::new(local.id, &local.r#ref, "a")
        .create(&db)
        .await
        .unwrap();
    let b = ActionFixture::new(foreign.id, &foreign.r#ref, "b")
        .create(&db)
        .await
        .unwrap();
    let owners = IamOwnershipRepository::component_packs(
        &*db,
        ComponentTarget::Action,
        &[a.id, b.id, i64::MAX],
    )
    .await
    .unwrap();
    assert_eq!(owners.len(), 2);
    assert_eq!(owners.get(&a.id), Some(&local.id));
    assert_eq!(owners.get(&b.id), Some(&foreign.id));
    let grants: Vec<Grant> = serde_json::from_value(
        normalize_iam_grants(
            json!([
                {"resource":"actions", "actions":["execute"], "constraints":{"refs":[a.r#ref]}}
            ]),
            Some(local.id),
        )
        .unwrap(),
    )
    .unwrap();
    let mut context = AuthorizationContext::new(1);
    context.target_ref = Some(a.r#ref.clone());
    context.managing_pack_id = owners.get(&a.id).copied();
    assert!(grants[0].allows(Resource::Actions, Action::Execute, &context));
    context.managing_pack_id = owners.get(&b.id).copied();
    assert!(!grants[0].allows(Resource::Actions, Action::Execute, &context));
    context.managing_pack_id = Some(local.id);
    context.target_ref = Some(b.r#ref);
    assert!(!grants[0].allows(Resource::Actions, Action::Execute, &context));
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn prospective_cache_owner_proof_fences_deletion_until_the_effect_transaction_commits() {
    use attune_common::repositories::{
        action::ActionRepository,
        runtime_target::{ProspectiveDataOwner, ProspectiveRuntimeTarget, RuntimeTargetRepository},
    };
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("cache_owner_fence")
        .create(&db)
        .await
        .unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "owner")
        .create(&db)
        .await
        .unwrap();
    let mut effect = db.begin().await.unwrap();
    let target = RuntimeTargetRepository::resolve_creation_target(
        &mut effect,
        ProspectiveDataOwner::Action(action.id),
        ProspectiveRuntimeTarget::CacheNamespace("future"),
        1,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(target.pack_id(), Some(pack.id));
    let pool = (*db).clone();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let action_id = action.id;
    let mut deletion = tokio::spawn(async move {
        let _ = started.send(());
        ActionRepository::delete(&pool, action_id).await
    });
    waiting.await.unwrap();
    let first = tokio::time::timeout(std::time::Duration::from_millis(100), &mut deletion).await;
    let blocked = first.is_err();
    effect.commit().await.unwrap();
    let deleted = match first {
        Ok(result) => result,
        Err(_) => {
            match tokio::time::timeout(std::time::Duration::from_secs(5), &mut deletion).await {
                Ok(result) => result,
                Err(error) => {
                    deletion.abort();
                    let _ = deletion.await;
                    panic!("owner deletion did not finish after effect commit: {error}");
                }
            }
        }
    }
    .unwrap()
    .unwrap();
    assert!(deleted);
    assert!(blocked, "owner deletion passed the admitted effect fence");
    let mut next = db.begin().await.unwrap();
    assert!(RuntimeTargetRepository::resolve_creation_target(
        &mut next,
        ProspectiveDataOwner::Action(action.id),
        ProspectiveRuntimeTarget::CacheNamespace("future"),
        1
    )
    .await
    .unwrap()
    .is_none());
    next.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}
