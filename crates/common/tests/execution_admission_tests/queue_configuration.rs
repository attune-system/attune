//! Exact queue declaration metadata, independently of runtime readiness activation.
use super::*;
use attune_common::repositories::{
    declared_input::DeclaredInputRepository,
    work_queue::{UpdateWorkQueueInput, WorkQueueRepository},
    FindByRef,
};

#[tokio::test]
async fn queue_configuration_declaration_retains_authored_site_after_projection_edit() {
    let (db, _, policy, _) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let queue_ref = format!("{}.inbox", policy.owner.pack_ref);
    let queue = WorkQueueRepository::find_by_ref(&mut *tx, &queue_ref)
        .await
        .unwrap()
        .unwrap();
    let expression = "{{ keystore['business.credential'] }}";
    WorkQueueRepository::update(
        &mut *tx,
        queue.id,
        UpdateWorkQueueInput {
            action_params: Some(json!({"token":expression})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let revision = Authority::capture_policy(&mut tx, &policy).await.unwrap();
    let invocation = revision
        .policy
        .invocations
        .iter()
        .find(|entry| matches!(entry.binding, InvocationBinding::Queue { .. }))
        .unwrap();
    let digest = invocation.dispatch_template_digest.as_deref().unwrap();
    let path = "/queue/action_params/token";
    let (_, declaration) = DeclaredInputRepository::declaration_for_policy(
        &mut tx,
        revision.id,
        &invocation.binding,
        digest,
        path,
        expression,
    )
    .await
    .unwrap();
    assert_eq!(declaration.pack, policy.owner.pack_id);
    assert_eq!(declaration.release, policy.release_id);
    // A live edit cannot authorize another selector through the old exact site.
    let replacement = "{{ keystore['unrelated.credential'] }}";
    WorkQueueRepository::update(
        &mut *tx,
        queue.id,
        UpdateWorkQueueInput {
            action_params: Some(json!({"token":replacement})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let retained = DeclaredInputRepository::declaration_for_policy(
        &mut tx,
        revision.id,
        &invocation.binding,
        digest,
        path,
        expression,
    )
    .await
    .unwrap();
    assert_eq!(retained.1, declaration);
    for (revision_digest, selected_path, selected_expression) in [
        (digest, path, replacement),
        (digest, "/queue/action_params/other", expression),
        ("invalid", path, expression),
    ] {
        assert!(DeclaredInputRepository::declaration_for_policy(
            &mut tx,
            revision.id,
            &invocation.binding,
            revision_digest,
            selected_path,
            selected_expression,
        )
        .await
        .is_err());
    }
    Authority::revoke_policy(&mut tx, revision.id)
        .await
        .unwrap();
    assert!(DeclaredInputRepository::declaration_for_policy(
        &mut tx,
        revision.id,
        &invocation.binding,
        digest,
        path,
        expression,
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}
