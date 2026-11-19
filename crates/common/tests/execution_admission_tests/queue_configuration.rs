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

#[tokio::test]
async fn queue_fixed_inputs_seal_credentials_without_contributor_key_rights() {
    use attune_common::{
        crypto,
        models::WorkQueueItemStatus,
        repositories::{
            component_configuration::ComponentConfigurationRepository as Configuration,
            key::{KeyRepository, UpdateKeyInput},
            pack::PackRepository,
            queue_configuration_input::QueueConfigurationInputRepository as Fixed,
            schema_contract::SchemaContractRepository,
            work_queue::{CreateWorkQueueItemInput, WorkQueueItemRepository},
        },
        value_schema::{SchemaContext, ValueSchemaBinding},
    };
    const ENC: &str = "queue-input-contract-test-encryption-key";
    let (db, snapshot, policy, human) = fixture_with_defaults(true).await;
    let other = IdentityFixture::new_unique("enqueue_only")
        .create(&db)
        .await
        .unwrap();
    let key = helpers::KeyFixture::new_pack(
        policy.owner.pack_id,
        &policy.owner.pack_ref,
        "credential",
        "accepted-value",
    )
    .create(&db)
    .await
    .unwrap();
    KeyRepository::update(
        &db,
        key.id,
        UpdateKeyInput {
            encrypted: Some(true),
            encryption_key_hash: Some(crypto::keystore::fingerprint(ENC, key.id).unwrap()),
            value: Some(
                crypto::keystore::encrypt_json(&json!("accepted-value"), ENC, key.id).unwrap(),
            ),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let queue_ref = format!("{}.inbox", policy.owner.pack_ref);
    let queue = WorkQueueRepository::find_by_ref(&mut *tx, &queue_ref)
        .await
        .unwrap()
        .unwrap();
    let queue = WorkQueueRepository::update(
        &mut *tx,
        queue.id,
        UpdateWorkQueueInput {
            action_params: Some(
                json!({"token":format!("{{{{ keystore['{}'] }}}}", key.r#ref),
            "endpoint":"{{ config.default_value }}","record":"{{ item.message }}"}),
            ),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let revision = Authority::capture_policy(&mut tx, &policy).await.unwrap();
    Authority::activate_policy(&mut tx, revision.id)
        .await
        .unwrap();
    let source = WorkQueueRepository::item_schema_source(&mut tx, queue.id)
        .await
        .unwrap();
    let contract = SchemaContractRepository::materialize(
        &mut tx,
        source,
        SchemaContext::FlatDocument,
        &queue.item_schema,
    )
    .await
    .unwrap();
    let payload_binding = ValueSchemaBinding {
        authored_schema: queue.item_schema.clone(),
        contract,
    };
    let mut items = Vec::new();
    for identity in [human, other.id] {
        let item = WorkQueueItemRepository::create_bound_pinned(
            &mut *tx,
            CreateWorkQueueItemInput {
                queue: queue.id,
                queue_ref: queue.r#ref.clone(),
                item_key: None,
                priority: 0,
                status: WorkQueueItemStatus::Queued,
                payload: json!({"message":"public-record"}),
                metadata: json!({}),
                trace_tag: None,
                enqueue_source: "test".into(),
                requested_by_identity: Some(identity),
                requested_by_execution: None,
                requested_by_enforcement: None,
                leased_execution: None,
                lease_token: None,
                lease_expires_at: None,
                attempt_count: 0,
                last_error: None,
                ack_summary: None,
            },
            &snapshot,
            &payload_binding,
        )
        .await
        .unwrap();
        // Explicitly admitted fixture: exercise the owning input contract without
        // pretending readiness-gated production enqueue has been activated.
        let selected = Admission::select(
            &mut tx,
            policy.owner.pack_id,
            InvocationBinding::Queue {
                queue_ref: queue.r#ref.clone(),
                action_ref: snapshot.executable.action.r#ref.clone(),
            },
            &snapshot,
            queue.permission_set_refs.as_ref().unwrap(),
            RequestAttribution::QueueBatch {
                queue_ref: queue.r#ref.clone(),
                items: BTreeMap::from([(
                    item.id,
                    Some(HumanAttribution {
                        identity_id: identity,
                        login: format!("enqueue-only-{identity}"),
                    }),
                )]),
            },
            attune_common::repositories::runtime_target::RuntimeTarget::Component(
                attune_common::repositories::runtime_target::ComponentTarget::Queue,
                queue.id,
            ),
        )
        .await
        .unwrap();
        let authority = selected.authority();
        sqlx::query("INSERT INTO work_queue_item_runtime_admission(item,policy_revision,authority,authority_digest) VALUES($1,$2,$3,$4)")
            .bind(item.id).bind(authority.policy_id).bind(sqlx::types::Json(authority)).bind(fixture_digest(authority))
            .execute(&mut *tx).await.unwrap();
        let captured = Fixed::capture(&mut tx, &queue, &item, &snapshot, ENC)
            .await
            .unwrap();
        assert_eq!(
            captured.resolved_keys()[&key.r#ref].value,
            json!("accepted-value")
        );
        assert!(captured.resolved_keys()[&key.r#ref].origin.encrypted);
        assert_eq!(
            captured.configuration()["default_value"],
            "child-default-value"
        );
        items.push(item);
    }
    Admission::queue_batch(&mut tx, &queue, &items, &snapshot)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    KeyRepository::update(
        &db,
        key.id,
        UpdateKeyInput {
            value: Some(
                crypto::keystore::encrypt_json(&json!("later-key-value"), ENC, key.id).unwrap(),
            ),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let pack = PackRepository::find_by_id(&mut *tx, policy.owner.pack_id)
        .await
        .unwrap()
        .unwrap();
    Configuration::bind_pack(
        &mut tx,
        &pack,
        Some(&json!({"default_value":"later-config","selector":"default_value"})),
        Some(ENC),
    )
    .await
    .unwrap();
    WorkQueueRepository::update(
        &mut *tx,
        queue.id,
        UpdateWorkQueueInput {
            action_params: Some(json!({"token":"{{ keystore['unrelated.key'] }}"})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for item in &items {
        // Idempotent capture cannot reselect credentials/configuration, including
        // after commit. A changed payload selector never becomes a fixed input.
        let retained = Fixed::capture(&mut tx, &queue, item, &snapshot, ENC)
            .await
            .unwrap();
        assert_eq!(
            retained.resolved_keys()[&key.r#ref].value,
            json!("accepted-value")
        );
        assert_eq!(
            retained.configuration()["default_value"],
            "child-default-value"
        );
        let mut forged = item.clone();
        forged.queue_ref = "unrelated.queue".into();
        assert!(Fixed::load(&mut tx, &queue, &forged, &snapshot, ENC)
            .await
            .is_err());
        assert!(
            Fixed::load(&mut tx, &queue, item, &snapshot, "wrong-encryption-key")
                .await
                .is_err()
        );
    }
    let row: serde_json::Value = sqlx::query_scalar(
        "SELECT encrypted_context FROM work_queue_item_configuration_input WHERE item=$1",
    )
    .bind(items[0].id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(!row.to_string().contains("accepted-value"));
    Authority::revoke_policy(&mut tx, revision.id)
        .await
        .unwrap();
    for item in &items {
        assert!(Fixed::load(&mut tx, &queue, item, &snapshot, ENC)
            .await
            .is_err());
    }
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}
