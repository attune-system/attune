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
        attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(&mut tx,"queue_item",item.id,identity).await.unwrap();
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
    let mut edit = sqlx::Connection::begin(&mut *tx).await.unwrap();
    let denied = WorkQueueRepository::update(
        &mut *edit,
        queue.id,
        UpdateWorkQueueInput {
            action_params: Some(json!({"token":"{{ keystore['unrelated.key'] }}"})),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(denied.to_string().contains("Drain accepted queue items"));
    edit.rollback().await.unwrap();
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

pub(super) const QUEUE_KEY: &str = "queue-input-contract-test-encryption-key";

async fn stage_record(
    conn: &mut sqlx::PgConnection,
    queue: &attune_common::models::WorkQueue,
    snapshot: &ExecutionExecutableSnapshot,
    actor: i64,
    payload: serde_json::Value,
) -> attune_common::models::WorkQueueItem {
    use attune_common::{
        models::WorkQueueItemStatus,
        repositories::{
            recorded_input_authority::RecordedInputAuthorityRepository,
            schema_contract::SchemaContractRepository,
            work_queue::{CreateWorkQueueItemInput, WorkQueueItemRepository},
        },
        value_schema::{SchemaContext, ValueSchemaBinding},
    };
    let source = WorkQueueRepository::item_schema_source(conn, queue.id)
        .await
        .unwrap();
    let contract = SchemaContractRepository::materialize(
        conn,
        source,
        SchemaContext::FlatDocument,
        &queue.item_schema,
    )
    .await
    .unwrap();
    let binding = ValueSchemaBinding {
        authored_schema: queue.item_schema.clone(),
        contract,
    };
    let item = WorkQueueItemRepository::create_bound_pinned(
        &mut *conn,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: None,
            priority: 0,
            status: WorkQueueItemStatus::Queued,
            payload,
            metadata: json!({}),
            trace_tag: None,
            enqueue_source: "owned-contract".into(),
            requested_by_identity: Some(actor),
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
        snapshot,
        &binding,
    )
    .await
    .unwrap();
    let selected = Admission::select(
        conn,
        queue.pack.unwrap(),
        InvocationBinding::Queue {
            queue_ref: queue.r#ref.clone(),
            action_ref: snapshot.executable.action.r#ref.clone(),
        },
        snapshot,
        queue.permission_set_refs.as_ref().unwrap(),
        RequestAttribution::QueueBatch {
            queue_ref: queue.r#ref.clone(),
            items: BTreeMap::from([(
                item.id,
                Some(HumanAttribution {
                    identity_id: actor,
                    login: format!("accepted-{actor}"),
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
    sqlx::query("INSERT INTO work_queue_item_runtime_admission(item,policy_revision,authority,authority_digest) VALUES($1,$2,$3,$4)")
        .bind(item.id).bind(selected.authority().policy_id).bind(sqlx::types::Json(selected.authority())).bind(fixture_digest(selected.authority())).execute(&mut *conn).await.unwrap();
    RecordedInputAuthorityRepository::capture_human(conn, "queue_item", item.id, actor)
        .await
        .unwrap();
    item
}
async fn accepted_record(
    conn: &mut sqlx::PgConnection,
    queue: &attune_common::models::WorkQueue,
    snapshot: &ExecutionExecutableSnapshot,
    actor: i64,
    payload: serde_json::Value,
) -> attune_common::models::WorkQueueItem {
    let item = stage_record(conn, queue, snapshot, actor, payload).await;
    attune_common::repositories::queue_configuration_input::QueueConfigurationInputRepository::capture(conn,queue,&item,snapshot,QUEUE_KEY).await.unwrap();
    item
}

#[tokio::test]
async fn mixed_queue_receipts_bind_each_contributor_and_structured_secret_after_item_retention() {
    use attune_common::{
        crypto,
        models::WorkQueueItemStatus,
        repositories::{
            accepted_batch_dependency::AcceptedBatchDependencyRepository as Batch,
            declared_input_materialization::DeclaredInputMaterializationRepository as Materialization,
            execution_secret_value::ExecutionSecretValueRepository as Secrets,
            inherited_input::{
                InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
                SelectionSource,
            },
            key::{KeyRepository, UpdateKeyInput},
            queue_configuration_input::QueueConfigurationInputRepository as Fixed,
            recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
            schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
            work_queue::{LeaseWorkQueueItemsInput, WorkQueueItemRepository},
            Delete,
        },
        secret_values::{prepare_secret_values, SecretValueInput, ENTITY_EXECUTION_CONFIG},
    };
    let database = create_test_pool().await.unwrap();
    let (db, snapshot, policy, human) =
        fixture_with_queue_shape(database, true, false, None, false, true).await;
    let other = IdentityFixture::new_unique("second_contributor")
        .create(&db)
        .await
        .unwrap();
    let key = helpers::KeyFixture::new_pack(
        policy.owner.pack_id,
        &policy.owner.pack_ref,
        "mixed_credential",
        "queue-key-secret",
    )
    .create(&db)
    .await
    .unwrap();
    KeyRepository::update(
        &db,
        key.id,
        UpdateKeyInput {
            encrypted: Some(true),
            encryption_key_hash: Some(crypto::keystore::fingerprint(QUEUE_KEY, key.id).unwrap()),
            value: Some(
                crypto::keystore::encrypt_json(&json!("queue-key-secret"), QUEUE_KEY, key.id)
                    .unwrap(),
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
    let queue =
        WorkQueueRepository::find_by_ref(&mut *tx, &format!("{}.inbox", policy.owner.pack_ref))
            .await
            .unwrap()
            .unwrap();
    let queue=WorkQueueRepository::update(&mut *tx,queue.id,UpdateWorkQueueInput {
        action_params:Some(json!({"value":format!("{{{{ keystore['{}'] + ':' + items[0].message + ':' + items[1].message }}}}",key.r#ref),
            "connection":"{{ config.connection }}"})),..Default::default()
    }).await.unwrap();
    let revision = Authority::capture_policy(&mut tx, &policy).await.unwrap();
    Authority::activate_policy(&mut tx, revision.id)
        .await
        .unwrap();
    let first = accepted_record(
        &mut tx,
        &queue,
        &snapshot,
        human,
        json!({"message":"first"}),
    )
    .await;
    let second = accepted_record(
        &mut tx,
        &queue,
        &snapshot,
        other.id,
        json!({"message":"second"}),
    )
    .await;
    WorkQueueItemRepository::update(
        &mut *tx,
        first.id,
        attune_common::repositories::work_queue::UpdateWorkQueueItemInput {
            metadata: Some(json!({"exact_integer":9_007_199_254_740_992_i64})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let lease = uuid::Uuid::new_v4();
    let items = WorkQueueItemRepository::lease_next_batch(
        &mut *tx,
        LeaseWorkQueueItemsInput {
            separate_requesters: false,
            queue: queue.id,
            ready_statuses: vec![WorkQueueItemStatus::Queued],
            limit: 2,
            batch_coalescing: None,
            leased_execution: None,
            lease_token: lease,
            lease_expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        items.iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![first.id, second.id]
    );
    let selected = Admission::queue_batch(&mut tx, &queue, &items, &snapshot)
        .await
        .unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            permission_set_refs: selected.authority().selected_refs.clone(),
            status: ExecutionStatus::Requested,
            config: None,
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let record = selected
        .attach(&mut tx, RuntimeOwner::Execution(execution.id))
        .await
        .unwrap();
    Contributors::capture_queue_execution(&mut tx, execution.id, &[first.id, second.id])
        .await
        .unwrap();
    assert!(
        Batch::capture_managed(&mut tx, record.id, &queue, &items, None, QUEUE_KEY)
            .await
            .is_err(),
        "an unattached lease is not an execution membership proof"
    );
    // Deliberately forged attachment sentinel; never consumed as authority.
    WorkQueueItemRepository::attach_execution_to_lease(&mut *tx, lease, -execution.id)
        .await
        .unwrap();
    assert!(
        Batch::capture_managed(&mut tx, record.id, &queue, &items, None, QUEUE_KEY)
            .await
            .is_err(),
        "another execution's lease cannot be captured"
    );
    WorkQueueItemRepository::attach_execution_to_lease(&mut *tx, lease, execution.id)
        .await
        .unwrap();
    let mut missing_source = sqlx::Connection::begin(&mut *tx).await.unwrap();
    sqlx::query("DELETE FROM recorded_queue_execution_input WHERE execution=$1 AND item=$2")
        .bind(execution.id)
        .bind(second.id)
        .execute(&mut *missing_source)
        .await
        .unwrap();
    assert!(
        Batch::capture_managed(
            &mut missing_source,
            record.id,
            &queue,
            &items,
            None,
            QUEUE_KEY
        )
        .await
        .is_err(),
        "membership cannot replace a missing original contributor manifest"
    );
    missing_source.rollback().await.unwrap();
    assert!(
        Batch::capture_managed(&mut tx, record.id, &queue, &items[..1], None, QUEUE_KEY)
            .await
            .is_err(),
        "a caller cannot omit a canonical member"
    );
    let duplicated = vec![items[0].clone(), items[0].clone()];
    assert!(
        Batch::capture_managed(&mut tx, record.id, &queue, &duplicated, None, QUEUE_KEY)
            .await
            .is_err(),
        "duplicate members cannot stand for different original items"
    );
    Batch::capture_managed(&mut tx, record.id, &queue, &items, None, QUEUE_KEY)
        .await
        .unwrap();
    let mut missing_item = sqlx::Connection::begin(&mut *tx).await.unwrap();
    sqlx::query("DELETE FROM runtime_accepted_batch_item WHERE admission=$1 AND item=$2")
        .bind(record.id)
        .bind(second.id)
        .execute(&mut *missing_item)
        .await
        .unwrap();
    assert!(
        Admission::require(&mut missing_item, RuntimeOwner::Execution(execution.id))
            .await
            .is_err(),
        "a retained batch requires every exact original item proof"
    );
    missing_item.rollback().await.unwrap();
    let fixed = Fixed::load(&mut tx, &queue, &first, &snapshot, QUEUE_KEY)
        .await
        .unwrap();
    Schemas::attach(
        &mut tx,
        BindingOwner::Execution,
        execution.id,
        fixed.schema(),
    )
    .await
    .unwrap();
    let contributors:Vec<serde_json::Value>=sqlx::query_scalar("SELECT authority->'grants' FROM recorded_queue_execution_input WHERE execution=$1 ORDER BY item")
        .bind(execution.id).fetch_all(&mut *tx).await.unwrap();
    assert_eq!(
        contributors,
        vec![json!([]), json!([])],
        "enqueue-only contributors carry no Key grants"
    );
    let value_site = fixed
        .declarations()
        .iter()
        .find(|site| site.source_path == "/queue/action_params/value")
        .unwrap();
    let original: serde_json::Value = sqlx::query_scalar(
        "SELECT proof FROM runtime_accepted_batch_item WHERE admission=$1 AND item=$2",
    )
    .bind(record.id)
    .bind(first.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let other_proof: serde_json::Value = sqlx::query_scalar(
        "SELECT proof FROM runtime_accepted_batch_item WHERE admission=$1 AND item=$2",
    )
    .bind(record.id)
    .bind(second.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let mut tampered = Vec::new();
    let mut swapped = original.clone();
    for field in ["nonce", "ciphertext"] {
        swapped["queue_payload"]["attestation"][field] =
            other_proof["queue_payload"]["attestation"][field].clone();
    }
    tampered.push(swapped);
    let mut purpose = original.clone();
    purpose["queue_payload"]["attestation"]["purpose"] = json!("other-purpose");
    tampered.push(purpose);
    let mut admission = original.clone();
    admission["queue_payload"]["attestation"]["original_admission"] = json!(record.id + 1);
    tampered.push(admission);
    let mut payload = original.clone();
    payload["queue_payload"]["payload"]["message"] = json!("rebound-payload");
    tampered.push(payload);
    let mut schema = original.clone();
    schema["queue_payload"]["schema"]["authored_schema"]["message"]["description"] =
        json!("rebound-schema");
    tampered.push(schema);
    let mut number = original.clone();
    number["queue_payload"]["context"]["metadata"]["exact_integer"] =
        json!(9_007_199_254_740_993_i64);
    tampered.push(number);
    let mut omitted = original.clone();
    omitted["queue_payload"]
        .as_object_mut()
        .unwrap()
        .remove("attestation");
    tampered.push(omitted);
    for body in tampered {
        reject_queue_manifest(
            &mut tx,
            execution.id,
            record.id,
            first.id,
            fixed.id(),
            value_site,
            &body,
        )
        .await;
    }
    let mut session = Inputs::begin_render(
        &mut tx,
        execution.id,
        &value_site.declaration_revision,
        &value_site.source_path,
        &value_site.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    assert!(
        Inputs::bind_key(&mut tx, &mut session, &key.r#ref, QUEUE_KEY)
            .await
            .is_err(),
        "queue declarations cannot downgrade to ambient/current Key binding"
    );
    Inputs::bind_queue_configuration(&mut tx, &mut session, fixed.id(), QUEUE_KEY)
        .await
        .unwrap();
    Inputs::bind_queue_contributors(&mut tx, &mut session, QUEUE_KEY)
        .await
        .unwrap();
    let rendered = Inputs::evaluate(&mut tx, session).await.unwrap();
    assert_eq!(rendered.value(), &json!("queue-key-secret:first:second"));
    let members = rendered
        .selections()
        .iter()
        .filter_map(|selection| match &selection.source {
            SelectionSource::QueueContributor { item } => Some(*item),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(members, [first.id, second.id].into_iter().collect());
    let site = InputSite {
        execution: execution.id,
        kind: InputEntityKind::ExecutionConfig,
        purpose: InputSitePurpose::Value,
        path: "/value".into(),
    };
    let mut swapped = rendered.selections().to_vec();
    for selection in &mut swapped {
        if let SelectionSource::QueueContributor { item } = &mut selection.source {
            if *item == first.id {
                *item = second.id;
                break;
            }
        }
    }
    assert!(Inputs::record_render(
        &mut tx,
        &rendered,
        &site,
        rendered.value(),
        &swapped,
        QUEUE_KEY
    )
    .await
    .is_err());
    let without_marker = rendered
        .selections()
        .iter()
        .filter(|selection| selection.context_path != "$queue_input")
        .cloned()
        .collect::<Vec<_>>();
    assert!(Inputs::record_render(
        &mut tx,
        &rendered,
        &site,
        rendered.value(),
        &without_marker,
        QUEUE_KEY
    )
    .await
    .is_err());
    let receipt = Inputs::record_render(
        &mut tx,
        &rendered,
        &site,
        rendered.value(),
        rendered.selections(),
        QUEUE_KEY,
    )
    .await
    .unwrap();
    assert!(receipt.source_origins().iter().any(|origin|matches!(origin,attune_common::secret_provenance::SecretOrigin::Key(origin) if origin.key_id==key.id && origin.encrypted)));
    let connection = fixed
        .declarations()
        .iter()
        .find(|site| site.source_path == "/queue/action_params/connection")
        .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        execution.id,
        &connection.declaration_revision,
        &connection.source_path,
        &connection.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_queue_configuration(&mut tx, &mut session, fixed.id(), QUEUE_KEY)
        .await
        .unwrap();
    Inputs::bind_queue_contributors(&mut tx, &mut session, QUEUE_KEY)
        .await
        .unwrap();
    let connection_value = Inputs::evaluate(&mut tx, session).await.unwrap();
    let projected = Inputs::project_queue_render(&mut tx, &connection_value, "/token")
        .await
        .unwrap();
    let secret_site = InputSite {
        execution: execution.id,
        kind: InputEntityKind::ExecutionConfig,
        purpose: InputSitePurpose::Value,
        path: "/connection/token".into(),
    };
    let leaf = Inputs::record_render(
        &mut tx,
        &projected,
        &secret_site,
        projected.value(),
        projected.selections(),
        QUEUE_KEY,
    )
    .await
    .unwrap();
    assert_eq!(projected.value(), &json!("queue-config-secret"));
    let supplied = json!({"value":rendered.value(),"connection":connection_value.value()});
    let inputs = vec![
        SecretValueInput {
            json_path: "/value".into(),
            value: rendered.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&receipt.provenance()).unwrap()),
        },
        SecretValueInput {
            json_path: "/connection/token".into(),
            value: projected.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&leaf.provenance()).unwrap()),
        },
    ];
    let (config, inputs) = Materialization::workflow_child_defaults(
        &mut tx,
        execution.id,
        &supplied,
        inputs,
        Some(QUEUE_KEY),
    )
    .await
    .unwrap();
    let fields = prepare_secret_values(inputs, QUEUE_KEY).unwrap();
    ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        execution.id,
        Some(record.id),
        &snapshot,
        Some(config),
        &fields,
    )
    .await
    .unwrap();
    Materialization::seal_execution_inputs(&mut tx, execution.id, QUEUE_KEY)
        .await
        .unwrap();
    WorkQueueItemRepository::attach_execution_to_lease(&mut *tx, lease, execution.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    // Repository retention boundary, not a claim of worker/terminal-ack execution.
    WorkQueueItemRepository::delete(&mut *tx, first.id)
        .await
        .unwrap();
    WorkQueueItemRepository::delete(&mut *tx, second.id)
        .await
        .unwrap();
    assert_eq!(Fixed::reclaim_with_conn(&mut tx, 10).await.unwrap(), 0);
    let restored = Inputs::receipt_context(&mut tx, &site, &receipt.origin(), QUEUE_KEY)
        .await
        .unwrap();
    assert_eq!(restored.site(), &site);
    Inputs::receipt_context(&mut tx, &secret_site, &leaf.origin(), QUEUE_KEY)
        .await
        .unwrap();
    let stored = Secrets::find_stored_by_entity(&mut *tx, ENTITY_EXECUTION_CONFIG, execution.id)
        .await
        .unwrap();
    assert!(
        attune_common::key_access::authorize_execution_parameter_key_origins_with_conn(
            &mut tx,
            execution.id,
            None,
            &stored,
        )
        .await
        .is_err(),
        "purpose receipts cannot be consumed as generic Key authority"
    );
    // Match the worker's typed receipt preparation, ordinary-source check and
    // exact buffer binding; generic Key authorization deliberately denies them.
    let deliveries =
        Materialization::prepare_execution_input_receipts(&mut tx, execution.id, QUEUE_KEY)
            .await
            .unwrap();
    assert!(!deliveries.is_empty());
    for delivery in &deliveries {
        delivery.ensure_fresh_on(&mut tx).await.unwrap();
    }
    let current = ExecutionRepository::find_by_id(&mut *tx, execution.id)
        .await
        .unwrap()
        .unwrap();
    let plaintext = attune_common::secret_values::restore_secret_values(
        current.config.clone().unwrap(),
        &stored,
        QUEUE_KEY,
    )
    .unwrap();
    assert_eq!(plaintext["value"], "queue-key-secret:first:second");
    assert_eq!(plaintext["connection"]["token"], "queue-config-secret");
    let old_config = fixed.configuration_revision().unwrap();
    let pack = attune_common::repositories::pack::PackRepository::find_by_id(
        &mut *tx,
        policy.owner.pack_id,
    )
    .await
    .unwrap()
    .unwrap();
    attune_common::repositories::component_configuration::ComponentConfigurationRepository::bind_pack(&mut tx,&pack,
        Some(&json!({"default_value":"later-default","selector":"default_value","connection":{"host":"later.invalid","token":"later-secret"}})),Some(QUEUE_KEY)).await.unwrap();
    KeyRepository::update(
        &mut *tx,
        key.id,
        UpdateKeyInput {
            value: Some(
                crypto::keystore::encrypt_json(&json!("later-key-secret"), QUEUE_KEY, key.id)
                    .unwrap(),
            ),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut deletion = sqlx::Connection::begin(&mut *tx).await.unwrap();
    assert!(
        sqlx::query("DELETE FROM component_configuration_revision WHERE id=$1")
            .bind(old_config)
            .execute(&mut *deletion)
            .await
            .is_err()
    );
    deletion.rollback().await.unwrap();
    ExecutionRepository::update(
        &mut *tx,
        execution.id,
        attune_common::repositories::execution::UpdateExecutionInput {
            status: Some(ExecutionStatus::Failed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let inherited = Admission::inherit(&mut tx, RuntimeOwner::Execution(execution.id), &snapshot)
        .await
        .unwrap();
    let retry = ExecutionRepository::create_retry(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            permission_set_refs: inherited.authority().selected_refs.clone(),
            status: ExecutionStatus::Requested,
            config: current.config.clone(),
            ..Default::default()
        },
        &snapshot,
        1,
        Some(1),
        Some("owned retention fixture".into()),
        execution.id,
    )
    .await
    .unwrap();
    let retry_record = inherited
        .attach(&mut tx, RuntimeOwner::Execution(retry.id))
        .await
        .unwrap();
    Contributors::copy(&mut tx, "execution", execution.id, "execution", retry.id)
        .await
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, retry.id, fixed.schema())
        .await
        .unwrap();
    Secrets::copy_entity(
        &mut *tx,
        ENTITY_EXECUTION_CONFIG,
        execution.id,
        ENTITY_EXECUTION_CONFIG,
        retry.id,
    )
    .await
    .unwrap();
    Materialization::seal_execution_inputs(&mut tx, retry.id, QUEUE_KEY)
        .await
        .unwrap();
    let replay = Secrets::find_stored_by_entity(&mut *tx, ENTITY_EXECUTION_CONFIG, retry.id)
        .await
        .unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|field| (&field.json_path, &field.encrypted_value))
            .collect::<Vec<_>>(),
        replay
            .iter()
            .map(|field| (&field.json_path, &field.encrypted_value))
            .collect::<Vec<_>>(),
        "retries retain ciphertext rather than resolving current credentials"
    );
    let inputs:Vec<i64>=sqlx::query_scalar("SELECT configuration_input FROM runtime_accepted_batch_item WHERE admission=$1 ORDER BY item").bind(retry_record.id).fetch_all(&mut *tx).await.unwrap();
    assert_eq!(inputs.len(), 2);
    assert!(inputs.contains(&fixed.id()));
    ExecutionRepository::delete(&mut *tx, execution.id)
        .await
        .unwrap();
    IdentityRepository::delete(&mut *tx, human).await.unwrap();
    IdentityRepository::delete(&mut *tx, other.id)
        .await
        .unwrap();
    let retry_deliveries =
        Materialization::prepare_execution_input_receipts(&mut tx, retry.id, QUEUE_KEY)
            .await
            .unwrap();
    assert!(!retry_deliveries.is_empty());
    for delivery in &retry_deliveries {
        delivery.ensure_fresh_on(&mut tx).await.unwrap();
    }
    let retry_current = ExecutionRepository::find_by_id(&mut *tx, retry.id)
        .await
        .unwrap()
        .unwrap();
    let retry_value = attune_common::secret_values::restore_secret_values(
        retry_current.config.clone().unwrap(),
        &replay,
        QUEUE_KEY,
    )
    .unwrap();
    assert_eq!(retry_value["value"], "queue-key-secret:first:second");
    assert_eq!(retry_value["connection"]["token"], "queue-config-secret");
    assert_eq!(Fixed::reclaim_with_conn(&mut tx, 10).await.unwrap(), 0);
    let mut revoked = sqlx::Connection::begin(&mut *tx).await.unwrap();
    Authority::revoke_policy(&mut revoked, revision.id)
        .await
        .unwrap();
    assert!(
        Inputs::receipt_context(&mut revoked, &site, &receipt.origin(), QUEUE_KEY)
            .await
            .is_err()
    );
    assert!(
        Materialization::prepare_execution_input_receipts(&mut revoked, retry.id, QUEUE_KEY)
            .await
            .is_err(),
        "revoked policy denies new retry input delivery despite retained evidence"
    );
    revoked.rollback().await.unwrap();
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn queue_input_creation_markers_survive_nested_savepoint_release_and_rollback() {
    use attune_common::repositories::queue_configuration_input::QueueConfigurationInputRepository as Fixed;
    let (db, snapshot, policy, human) = fixture().await;
    let mut tx = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let queue =
        WorkQueueRepository::find_by_ref(&mut *tx, &format!("{}.inbox", policy.owner.pack_ref))
            .await
            .unwrap()
            .unwrap();
    let mut nested = sqlx::Connection::begin(&mut *tx).await.unwrap();
    let released = stage_record(
        &mut nested,
        &queue,
        &snapshot,
        human,
        json!({"message":"nested"}),
    )
    .await;
    nested.commit().await.unwrap();
    Fixed::capture(&mut tx, &queue, &released, &snapshot, QUEUE_KEY)
        .await
        .unwrap();
    let mut rolled_back = sqlx::Connection::begin(&mut *tx).await.unwrap();
    let discarded = stage_record(
        &mut rolled_back,
        &queue,
        &snapshot,
        human,
        json!({"message":"discarded"}),
    )
    .await;
    rolled_back.rollback().await.unwrap();
    assert!(
        attune_common::repositories::work_queue::WorkQueueItemRepository::find_by_id(
            &mut *tx,
            discarded.id
        )
        .await
        .unwrap()
        .is_none()
    );
    let current = accepted_record(
        &mut tx,
        &queue,
        &snapshot,
        human,
        json!({"message":"after rollback-to"}),
    )
    .await;
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    for item in [released, current] {
        let first = Fixed::load(&mut tx, &queue, &item, &snapshot, QUEUE_KEY)
            .await
            .unwrap();
        let second = Fixed::capture(&mut tx, &queue, &item, &snapshot, QUEUE_KEY)
            .await
            .unwrap();
        assert_eq!(
            first.id(),
            second.id(),
            "a sealed load is idempotent after commit"
        );
    }
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn queue_input_creation_marker_never_backfills_history_or_accepts_updated_old_rows() {
    use attune_common::repositories::{
        queue_configuration_input::QueueConfigurationInputRepository as Fixed,
        work_queue::{UpdateWorkQueueItemInput, WorkQueueItemRepository},
    };
    helpers::init_test_env();
    let config = attune_common::config::Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))
    .unwrap();
    let db = TestDatabase::create_before(
        &config.database,
        "20261119000003_queue_input_creation_transaction.sql",
    )
    .await
    .unwrap()
    .with_cleanup_on_drop();
    let (db, snapshot, policy, human) =
        fixture_with_queue_shape(db, false, false, None, false, false).await;
    let mut tx = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let queue =
        WorkQueueRepository::find_by_ref(&mut *tx, &format!("{}.inbox", policy.owner.pack_ref))
            .await
            .unwrap()
            .unwrap();
    let historical = stage_record(
        &mut tx,
        &queue,
        &snapshot,
        human,
        json!({"message":"historical"}),
    )
    .await;
    tx.commit().await.unwrap();
    sqlx::raw_sql(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../migrations/20261119000003_queue_input_creation_transaction.sql"
    )))
    .execute(&*db)
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let root_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let _: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let mut foreign = db.begin().await.unwrap();
    let foreign_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *foreign)
        .await
        .unwrap();
    assert_ne!(root_pid, foreign_pid);
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(&mut foreign)
        .await
        .unwrap();
    let foreign_item = stage_record(
        &mut foreign,
        &queue,
        &snapshot,
        human,
        json!({"message":"foreign committed"}),
    )
    .await;
    foreign.commit().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let absent: bool = sqlx::query_scalar(
        "SELECT input_creation_transaction IS NULL FROM work_queue_item WHERE id=$1",
    )
    .bind(historical.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(
        absent,
        "forward migration must not invent historical creation evidence"
    );
    for item in [historical, foreign_item] {
        assert!(Fixed::capture(&mut tx, &queue, &item, &snapshot, QUEUE_KEY)
            .await
            .is_err());
        let updated = WorkQueueItemRepository::update(
            &mut *tx,
            item.id,
            UpdateWorkQueueItemInput {
                metadata: Some(json!({"updated":true})),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            Fixed::capture(&mut tx, &queue, &updated, &snapshot, QUEUE_KEY)
                .await
                .is_err(),
            "UPDATE cannot turn an old item into original creation"
        );
        let mut overwrite = sqlx::Connection::begin(&mut *tx).await.unwrap();
        assert!(sqlx::query("UPDATE work_queue_item SET input_creation_transaction=pg_current_xact_id() WHERE id=$1").bind(item.id).execute(&mut *overwrite).await.is_err());
        overwrite.rollback().await.unwrap();
    }
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn queue_key_versions_are_durable_non_value_cohorts_with_independent_binding_checks() {
    use attune_common::{
        crypto,
        models::WorkQueueItemStatus,
        repositories::{
            key::{KeyRepository, UpdateKeyInput},
            queue_configuration_input::QueueConfigurationInputRepository as Fixed,
            work_queue::{LeaseWorkQueueItemsInput, WorkQueueItemRepository},
        },
    };
    let (db, snapshot, policy, human) = fixture_with_defaults(true).await;
    let plain = helpers::KeyFixture::new_pack(
        policy.owner.pack_id,
        &policy.owner.pack_ref,
        "plain_revision",
        "plain-before",
    )
    .create(&db)
    .await
    .unwrap();
    let encrypted = helpers::KeyFixture::new_pack(
        policy.owner.pack_id,
        &policy.owner.pack_ref,
        "encrypted_revision",
        "unused",
    )
    .create(&db)
    .await
    .unwrap();
    KeyRepository::update(
        &db,
        encrypted.id,
        UpdateKeyInput {
            encrypted: Some(true),
            value: Some(
                crypto::keystore::encrypt_json(&json!("encrypted-before"), QUEUE_KEY, encrypted.id)
                    .unwrap(),
            ),
            encryption_key_hash: Some(
                crypto::keystore::fingerprint(QUEUE_KEY, encrypted.id).unwrap(),
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
    let queue =
        WorkQueueRepository::find_by_ref(&mut *tx, &format!("{}.inbox", policy.owner.pack_ref))
            .await
            .unwrap()
            .unwrap();
    let queue=WorkQueueRepository::update(&mut *tx,queue.id,UpdateWorkQueueInput {action_params:Some(json!({"value":format!("{{{{ keystore['{}'] + ':' + keystore['{}'] }}}}",plain.r#ref,encrypted.r#ref)})),..Default::default()}).await.unwrap();
    let revision = Authority::capture_policy(&mut tx, &policy).await.unwrap();
    Authority::activate_policy(&mut tx, revision.id)
        .await
        .unwrap();
    let first = accepted_record(
        &mut tx,
        &queue,
        &snapshot,
        human,
        json!({"message":"first"}),
    )
    .await;
    let second = accepted_record(
        &mut tx,
        &queue,
        &snapshot,
        human,
        json!({"message":"second"}),
    )
    .await;
    let original = Fixed::load(&mut tx, &queue, &first, &snapshot, QUEUE_KEY)
        .await
        .unwrap();
    let stable = Fixed::load(&mut tx, &queue, &second, &snapshot, QUEUE_KEY)
        .await
        .unwrap();
    assert!(original.compatible_fixed_inputs(&stable));
    let versions: serde_json::Value = sqlx::query_scalar(
        "SELECT binding->'key_versions' FROM work_queue_item_configuration_input WHERE item=$1",
    )
    .bind(first.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        versions,
        json!({plain.r#ref.clone():{"key":plain.id,"revision":1},encrypted.r#ref.clone():{"key":encrypted.id,"revision":2}}),
        "cohort identity contains only repository-owned IDs/revisions, never value fingerprints"
    );
    let changed = KeyRepository::update(
        &mut *tx,
        plain.id,
        UpdateKeyInput {
            value: Some(json!("plain-after")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let third = accepted_record(
        &mut tx,
        &queue,
        &snapshot,
        human,
        json!({"message":"plain changed"}),
    )
    .await;
    let changed_plain = Fixed::load(&mut tx, &queue, &third, &snapshot, QUEUE_KEY)
        .await
        .unwrap();
    assert!(!original.compatible_fixed_inputs(&changed_plain));
    KeyRepository::update(
        &mut *tx,
        encrypted.id,
        UpdateKeyInput {
            value: Some(
                crypto::keystore::encrypt_json(&json!("encrypted-after"), QUEUE_KEY, encrypted.id)
                    .unwrap(),
            ),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let fourth = accepted_record(
        &mut tx,
        &queue,
        &snapshot,
        human,
        json!({"message":"encrypted changed"}),
    )
    .await;
    let changed_encrypted = Fixed::load(&mut tx, &queue, &fourth, &snapshot, QUEUE_KEY)
        .await
        .unwrap();
    assert!(!changed_plain.compatible_fixed_inputs(&changed_encrypted));
    assert_eq!(
        original.resolved_keys()[&plain.r#ref].value,
        json!("plain-before")
    );
    assert_eq!(
        original.resolved_keys()[&encrypted.r#ref].value,
        json!("encrypted-before")
    );
    for expected in [vec![first.id, second.id], vec![third.id], vec![fourth.id]] {
        let leased = WorkQueueItemRepository::lease_next_batch(
            &mut *tx,
            LeaseWorkQueueItemsInput {
                separate_requesters: false,
                queue: queue.id,
                ready_statuses: vec![WorkQueueItemStatus::Queued],
                limit: 10,
                batch_coalescing: None,
                leased_execution: None,
                lease_token: uuid::Uuid::new_v4(),
                lease_expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            },
        )
        .await
        .unwrap();
        assert_eq!(leased.iter().map(|item|item.id).collect::<Vec<_>>(),expected,"retained version cohorts preserve FIFO progress without mixing different accepted credentials");
    }
    sqlx::query("UPDATE key SET input_revision=999999 WHERE id=$1")
        .bind(plain.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let ignored: i64 = sqlx::query_scalar("SELECT input_revision FROM key WHERE id=$1")
        .bind(plain.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        ignored, 3,
        "supplied revisions never replace the database increment"
    );
    let latest = KeyRepository::update(
        &mut *tx,
        plain.id,
        UpdateKeyInput {
            value: Some(json!("plain-latest")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        changed.updated, latest.updated,
        "same transaction timestamps are deliberately equal"
    );
    let latest_revision: i64 = sqlx::query_scalar("SELECT input_revision FROM key WHERE id=$1")
        .bind(plain.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        latest_revision, 4,
        "same-transaction edits have distinct durable revisions"
    );
    let fifth = accepted_record(
        &mut tx,
        &queue,
        &snapshot,
        human,
        json!({"message":"before synthetic collision"}),
    )
    .await;
    let before_collision = Fixed::load(&mut tx, &queue, &fifth, &snapshot, QUEUE_KEY)
        .await
        .unwrap();
    // Owned corruption fixture only: bypass the update trigger to force a tag
    // collision. A cohort tag must never authorize different accepted bytes.
    sqlx::query("ALTER TABLE key DISABLE TRIGGER advance_key_input_revision")
        .execute(&mut *tx)
        .await
        .unwrap();
    KeyRepository::update(
        &mut *tx,
        plain.id,
        UpdateKeyInput {
            value: Some(json!("collision-value")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    sqlx::query("ALTER TABLE key ENABLE TRIGGER advance_key_input_revision")
        .execute(&mut *tx)
        .await
        .unwrap();
    let sixth = accepted_record(
        &mut tx,
        &queue,
        &snapshot,
        human,
        json!({"message":"synthetic collision"}),
    )
    .await;
    let collision = Fixed::load(&mut tx, &queue, &sixth, &snapshot, QUEUE_KEY)
        .await
        .unwrap();
    let colliding_versions:Vec<serde_json::Value>=sqlx::query_scalar("SELECT binding->'key_versions' FROM work_queue_item_configuration_input WHERE item=ANY($1) ORDER BY item").bind([fifth.id,sixth.id].as_slice()).fetch_all(&mut *tx).await.unwrap();
    assert_eq!(colliding_versions[0], colliding_versions[1]);
    assert!(
        !before_collision.compatible_fixed_inputs(&collision),
        "exact sealed values/origins remain decisive even if optimization tags collide"
    );
    assert_eq!(
        before_collision.resolved_keys()[&plain.r#ref].value,
        json!("plain-latest")
    );
    sqlx::query("ALTER TABLE key DISABLE TRIGGER advance_key_input_revision")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE key SET input_revision=9223372036854775807 WHERE id=$1")
        .bind(plain.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE key ENABLE TRIGGER advance_key_input_revision")
        .execute(&mut *tx)
        .await
        .unwrap();
    let mut overflow = sqlx::Connection::begin(&mut *tx).await.unwrap();
    let failure = KeyRepository::update(
        &mut *overflow,
        plain.id,
        UpdateKeyInput {
            value: Some(json!("must-not-commit")),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&failure,attune_common::Error::Database(sqlx::Error::Database(error)) if error.code().as_deref()==Some("22003")),
        "overflow must be the atomic BIGINT boundary"
    );
    overflow.rollback().await.unwrap();
    let after_overflow: (i64, serde_json::Value) =
        sqlx::query_as("SELECT input_revision,value FROM key WHERE id=$1")
            .bind(plain.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        after_overflow,
        (i64::MAX, json!("collision-value")),
        "overflow fails atomically without changing accepted data"
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

// Corruption fixture on this test's exclusively owned database only. Recompute
// public metadata hashes to require the authenticated boundary to be decisive.
async fn reject_queue_manifest(
    conn: &mut sqlx::PgConnection,
    execution: i64,
    admission: i64,
    item: i64,
    input: i64,
    site: &attune_common::repositories::declared_input::DeclaredInputBinding,
    body: &serde_json::Value,
) {
    use attune_common::repositories::inherited_input::{
        InheritedInputRepository as Inputs, InputSitePurpose,
    };
    let mut tx = sqlx::Connection::begin(conn).await.unwrap();
    sqlx::query(
        "ALTER TABLE runtime_accepted_batch_item DISABLE TRIGGER guard_runtime_accepted_batch_item",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("UPDATE runtime_accepted_batch_item SET proof=$3,proof_digest=$4 WHERE admission=$1 AND item=$2").bind(admission).bind(item).bind(body).bind(fixture_digest(body)).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "ALTER TABLE runtime_accepted_batch_item ENABLE TRIGGER guard_runtime_accepted_batch_item",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    let denied = match Inputs::begin_render(
        &mut tx,
        execution,
        &site.declaration_revision,
        &site.source_path,
        &site.expression,
        InputSitePurpose::Value,
    )
    .await
    {
        Ok(mut session) => {
            Inputs::bind_queue_configuration(&mut tx, &mut session, input, QUEUE_KEY)
                .await
                .is_err()
        }
        Err(_) => true,
    };
    assert!(
        denied,
        "swapped or incomplete Queue attestation cannot authorize fixed inputs"
    );
    tx.rollback().await.unwrap();
}
