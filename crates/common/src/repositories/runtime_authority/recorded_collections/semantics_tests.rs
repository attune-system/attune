//! Differential DB tests against the real typed target matcher, not SQL text.
//!
//! Matcher fixtures deliberately include scopes rejected at admission, such as
//! empty vectors. They test defense in depth without exposing a production
//! authority constructor. Real credential/liveness tests remain in the API suite.
use super::*;
use crate::{
    config::Config,
    repositories::{
        runtime_authority::{ApprovalEffectFence, RuntimeAdmissionRecord},
        runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
    },
    runtime_authority::{
        BoundRuntimeTarget, InvocationBinding, PackAuthorityOwner, RequestAttribution,
        SelectedRuntimeAuthority,
    },
    test_database::TestDatabase,
};
use serde_json::{json, Value};
use sqlx::types::Json;
use std::collections::BTreeSet;

fn matcher_authority(
    pack: i64,
    grants: Vec<AdmittedGrant>,
    binding: Option<GrantTargetBindings>,
    approvals: &[i64],
) -> ValidatedRuntimeAuthority {
    let concrete_targets = grants
        .iter()
        .filter_map(|grant| {
            binding
                .clone()
                .map(|binding| (grant_key(grant).unwrap(), binding))
        })
        .collect();
    ValidatedRuntimeAuthority {
        record: RuntimeAdmissionRecord {
            id: 1,
            owner_kind: "execution".into(),
            owner_id: 77,
            policy_revision: 1,
            authority_digest: String::new(),
            revoked_at: None,
            authority: Json(SelectedRuntimeAuthority {
                policy_id: 1,
                binding: InvocationBinding::Action {
                    action_ref: "semantics.run".into(),
                },
                owner: PackAuthorityOwner {
                    pack_id: pack,
                    pack_ref: "semantics".into(),
                    sponsor_identity_id: 7,
                },
                target_release_id: 1,
                target_release_digest: "a".repeat(64),
                attempt: uuid::Uuid::new_v4(),
                selected_refs: vec!["semantics.reader".into()],
                grants,
                attribution: RequestAttribution::ParentExecution { execution_id: 77 },
            }),
        },
        approvals: approvals.iter().copied().collect(),
        current_attempt: uuid::Uuid::new_v4(),
        standard_owners: BTreeSet::new(),
        concrete_targets,
        effect_fence: ApprovalEffectFence {
            earliest_expiry: None,
        },
    }
}

async fn seed() -> (TestDatabase, i64, i64, i64, i64) {
    let config = Config::load_from_file(&format!(
        "{}/../../config.test.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let db = TestDatabase::create(&config.database)
        .await
        .unwrap()
        .with_cleanup_on_drop();
    let own: i64 = sqlx::query_scalar(
        "INSERT INTO pack(ref,label,version) VALUES('semantics','Own','1.0.0') RETURNING id",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    let foreign: i64 = sqlx::query_scalar("INSERT INTO pack(ref,label,version) VALUES('foreign_semantics','Foreign','1.0.0') RETURNING id")
        .fetch_one(db.pool()).await.unwrap();
    let queue: i64 = sqlx::query_scalar("INSERT INTO work_queue(ref,pack,pack_ref,label,dispatch_action_ref,reference_visibility) VALUES('semantics.queue',$1,'semantics','Own','semantics.run','private') RETURNING id")
        .bind(own).fetch_one(db.pool()).await.unwrap();
    let foreign_queue: i64 = sqlx::query_scalar("INSERT INTO work_queue(ref,pack,pack_ref,label,dispatch_action_ref) VALUES('semantics.foreign',$1,'foreign_semantics','Foreign','semantics.run') RETURNING id")
        .bind(foreign).fetch_one(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO work_queue_item(id,queue,queue_ref,payload,enqueue_source) VALUES(700001,$1,'semantics.previous','{}','matcher'),(700002,$1,'semantics.queue','{}','matcher')")
        .bind(queue).execute(db.pool()).await.unwrap();
    // Numeric collision with a queue ID must not stand in for an item ID.
    sqlx::query("INSERT INTO work_queue_item(id,queue,queue_ref,payload,enqueue_source) VALUES($1,$2,'semantics.foreign','{}','matcher')")
        .bind(queue).bind(foreign_queue).execute(db.pool()).await.unwrap();
    (db, own, foreign, queue, foreign_queue)
}

async fn assert_item_parity(
    conn: &mut PgConnection,
    authority: &ValidatedRuntimeAuthority,
    ids: &[i64],
) -> Vec<i64> {
    let mut expected = Vec::new();
    for id in ids {
        if let Some(target) =
            RuntimeTargetRepository::resolve(conn, RuntimeTarget::QueueItem(*id), 7)
                .await
                .unwrap()
        {
            if authority.allows_target(&target, Action::Read) {
                expected.push(*id);
            }
        }
    }
    expected.sort_unstable();
    let filter = authority.queue_item_collection();
    let mut query = QueryBuilder::new("SELECT work_queue_item.id FROM work_queue_item WHERE ");
    filter.push(&mut query);
    query.push(" ORDER BY id");
    let actual: Vec<i64> = query
        .build_query_scalar()
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    assert_eq!(actual, expected, "SQL and typed matcher disagree");
    let mut count = QueryBuilder::new("SELECT COUNT(*)::BIGINT FROM work_queue_item WHERE ");
    filter.push(&mut count);
    let total: i64 = count
        .build_query_scalar()
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(total as usize, actual.len());
    authority.ensure_fresh_on(conn).await.unwrap();
    actual
}

#[tokio::test]
async fn queue_item_sql_matches_typed_context_for_every_selector_and_empty_vector() {
    let (db, own, foreign, queue, _) = seed().await;
    let mut tx = db.pool().begin().await.unwrap();
    let scopes = vec![
        Value::Null,
        json!({}),
        json!({"ids":[700001]}),
        json!({"ids":[queue]}),
        json!({"ids":[]}),
        json!({"refs":["semantics.previous"]}),
        json!({"refs":["*"]}),
        json!({"refs":[]}),
        json!({"pack_refs":["semantics"]}),
        json!({"pack_refs":["foreign_semantics"]}),
        json!({"pack_refs":[]}),
        json!({"owner":"any"}),
        json!({"owner":"none"}),
        json!({"owner":"self"}),
        json!({"owner_types":["pack"]}),
        json!({"owner_types":[]}),
        json!({"owner_refs":["semantics"]}),
        json!({"owner_refs":[]}),
        json!({"visibility":["private"]}),
        json!({"visibility":["public"]}),
        json!({"visibility":[]}),
        json!({"execution_scope":"any"}),
        json!({"execution_scope":"self"}),
        json!({"execution_scope":"descendants"}),
        json!({"attributes":{}}),
        json!({"attributes":{"role":"admin"}}),
        json!({"encrypted":true}),
        json!({"encrypted":false}),
        json!({"managing_pack_id":own}),
        json!({"managing_pack_id":foreign}),
        json!({"ids":[700001,700002],"refs":["semantics.previous"]}),
        json!({"ids":[700002],"refs":["semantics.previous"]}),
    ];
    for scope in scopes {
        let grant: Grant = serde_json::from_value(
            json!({"resource":"queue_items","actions":["read"],"constraints":scope}),
        )
        .unwrap();
        let authority = matcher_authority(
            own,
            vec![AdmittedGrant::PackOwned {
                pack_id: own,
                grant,
            }],
            Some(GrantTargetBindings::default()),
            &[],
        );
        assert_item_parity(&mut tx, &authority, &[queue, 700001, 700002]).await;
    }
    // Relative owner refs and artifact visibility are absent from this typed
    // context, even though the actual queue is pack-owned and reference-private.
    let target = RuntimeTargetRepository::resolve(&mut tx, RuntimeTarget::QueueItem(700001), 7)
        .await
        .unwrap()
        .unwrap();
    assert!(target.context().owner_ref.is_none());
    assert!(target.context().visibility.is_none());
    assert_eq!(target.context().target_id, Some(700001));
    let collection = RuntimeTargetRepository::resolve(&mut tx, RuntimeTarget::QueueItems(queue), 7)
        .await
        .unwrap()
        .unwrap();
    assert!(collection.context().target_id.is_none());
    let narrow: Grant = serde_json::from_value(
        json!({"resource":"queue_items","actions":["read"],"constraints":{"ids":[700001]}}),
    )
    .unwrap();
    let narrow = matcher_authority(
        own,
        vec![AdmittedGrant::PackOwned {
            pack_id: own,
            grant: narrow,
        }],
        Some(GrantTargetBindings::default()),
        &[],
    );
    assert!(narrow.allows_target(&target, Action::Read));
    assert!(!narrow.allows_target(&collection, Action::Read));
    assert_eq!(
        assert_item_parity(&mut tx, &narrow, &[queue, 700001, 700002]).await,
        vec![700001]
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn concrete_bindings_none_empty_pins_and_selected_approvals_match_typed_targets() {
    let (db, own, foreign, queue, _) = seed().await;
    let mut tx = db.pool().begin().await.unwrap();
    let grant = Grant {
        resource: Resource::QueueItems,
        actions: vec![Action::Read],
        constraints: None,
    };
    let target = BoundRuntimeTarget {
        kind: "work_queue_item".into(),
        id: 700001,
        pack_id: Some(own),
        owner_type: None,
        owner_id: None,
    };
    let bindings = vec![
        None,
        Some(GrantTargetBindings::default()),
        Some(GrantTargetBindings {
            targets: Some(BTreeSet::new()),
            ..Default::default()
        }),
        Some(GrantTargetBindings {
            targets: Some(BTreeSet::from([target.clone()])),
            ..Default::default()
        }),
        Some(GrantTargetBindings {
            targets: Some(BTreeSet::from([BoundRuntimeTarget {
                id: queue,
                ..target.clone()
            }])),
            ..Default::default()
        }),
        Some(GrantTargetBindings {
            targets: Some(BTreeSet::from([BoundRuntimeTarget {
                kind: "work_queue".into(),
                ..target.clone()
            }])),
            ..Default::default()
        }),
        Some(GrantTargetBindings {
            targets: Some(BTreeSet::from([BoundRuntimeTarget {
                pack_id: None,
                ..target.clone()
            }])),
            ..Default::default()
        }),
        Some(GrantTargetBindings {
            packs: Some(BTreeSet::from([own])),
            ..Default::default()
        }),
        Some(GrantTargetBindings {
            packs: Some(BTreeSet::from([foreign])),
            ..Default::default()
        }),
        Some(GrantTargetBindings {
            packs: Some(BTreeSet::new()),
            ..Default::default()
        }),
        Some(GrantTargetBindings {
            owners: Some(BTreeSet::from([("pack".into(), own, Some(own))])),
            ..Default::default()
        }),
        Some(GrantTargetBindings {
            owners: Some(BTreeSet::new()),
            ..Default::default()
        }),
    ];
    for binding in bindings {
        let authority = matcher_authority(
            own,
            vec![AdmittedGrant::PackOwned {
                pack_id: own,
                grant: grant.clone(),
            }],
            binding,
            &[],
        );
        assert_item_parity(&mut tx, &authority, &[queue, 700001, 700002]).await;
    }
    for approvals in [vec![], vec![99], vec![88], vec![88, 99]] {
        let authority = matcher_authority(
            own,
            vec![AdmittedGrant::External {
                approval_id: 88,
                grant: grant.clone(),
            }],
            Some(GrantTargetBindings::default()),
            &approvals,
        );
        let actual = assert_item_parity(&mut tx, &authority, &[queue, 700001, 700002]).await;
        assert_eq!(actual.len(), if approvals.contains(&88) { 3 } else { 0 });
    }
    let external = AdmittedGrant::External {
        approval_id: 88,
        grant: grant.clone(),
    };
    let open = matcher_authority(
        own,
        vec![external.clone()],
        Some(GrantTargetBindings::default()),
        &[88],
    );
    let pinned = matcher_authority(
        own,
        vec![external],
        Some(GrantTargetBindings {
            targets: Some(BTreeSet::from([target])),
            ..Default::default()
        }),
        &[88],
    );
    sqlx::query("INSERT INTO work_queue_item(id,queue,queue_ref,payload,enqueue_source) VALUES(700003,$1,'semantics.queue','{}','matcher')")
        .bind(queue).execute(&mut *tx).await.unwrap();
    assert_eq!(
        assert_item_parity(&mut tx, &open, &[queue, 700001, 700002, 700003])
            .await
            .len(),
        4
    );
    assert_eq!(
        assert_item_parity(&mut tx, &pinned, &[queue, 700001, 700002, 700003]).await,
        vec![700001]
    );
    let mut standard = matcher_authority(own, vec![], Some(GrantTargetBindings::default()), &[88]);
    standard.standard_owners.insert(("pack".into(), own));
    assert!(
        assert_item_parity(&mut tx, &standard, &[queue, 700001, 700002, 700003])
            .await
            .is_empty()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn accepted_item_listing_survives_disabled_retired_and_renamed_queues_not_deletion() {
    let (db, own, _, queue, _) = seed().await;
    sqlx::query("UPDATE work_queue SET enabled = false, retired_at = clock_timestamp(), ref = 'semantics.renamed' WHERE id = $1")
        .bind(queue).execute(db.pool()).await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    let grant = Grant {
        resource: Resource::QueueItems,
        actions: vec![Action::Read],
        constraints: None,
    };
    let authority = matcher_authority(
        own,
        vec![AdmittedGrant::PackOwned {
            pack_id: own,
            grant,
        }],
        Some(GrantTargetBindings::default()),
        &[],
    );
    let rows = queue_items(
        &mut tx,
        &authority.queue_item_collection(),
        &WorkQueueItemSearchFilters {
            queue: Some(queue),
            limit: 100,
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(rows.total, 2);
    assert_eq!(
        assert_item_parity(&mut tx, &authority, &[queue, 700001, 700002]).await,
        vec![700001, 700002]
    );
    let target = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Component(ComponentTarget::Queue, queue),
        7,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!target.accepts_new_effects());
    tx.rollback().await.unwrap();
    sqlx::query("DELETE FROM work_queue WHERE id = $1")
        .bind(queue)
        .execute(db.pool())
        .await
        .unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    assert!(
        assert_item_parity(&mut tx, &authority, &[queue, 700001, 700002])
            .await
            .is_empty()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

async fn assert_queue_parity(
    conn: &mut PgConnection,
    authority: &ValidatedRuntimeAuthority,
    ids: &[i64],
) -> Vec<i64> {
    let mut expected = Vec::new();
    for id in ids {
        if let Some(target) = RuntimeTargetRepository::resolve(
            conn,
            RuntimeTarget::Component(ComponentTarget::Queue, *id),
            7,
        )
        .await
        .unwrap()
        {
            if authority.allows_target(&target, Action::Read) {
                expected.push(*id);
            }
        }
    }
    expected.sort_unstable();
    let mut query = QueryBuilder::new("SELECT work_queue.id FROM work_queue WHERE ");
    authority.queue_collection().push(&mut query);
    query.push(" ORDER BY id");
    let actual: Vec<i64> = query
        .build_query_scalar()
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    assert_eq!(actual, expected);
    actual
}

#[tokio::test]
async fn queue_ref_selector_intersects_id_pin_and_private_reference_is_not_artifact_visibility() {
    let (db, own, _, queue, foreign_queue) = seed().await;
    let grant: Grant = serde_json::from_value(
        json!({"resource":"queues","actions":["read"],"constraints":{"refs":["semantics.queue"]}}),
    )
    .unwrap();
    let target = BoundRuntimeTarget {
        kind: "work_queue".into(),
        id: queue,
        pack_id: Some(own),
        owner_type: None,
        owner_id: None,
    };
    let pinned = matcher_authority(
        own,
        vec![AdmittedGrant::PackOwned {
            pack_id: own,
            grant: grant.clone(),
        }],
        Some(GrantTargetBindings {
            targets: Some(BTreeSet::from([target])),
            ..Default::default()
        }),
        &[],
    );
    let open_ref = matcher_authority(
        own,
        vec![AdmittedGrant::PackOwned {
            pack_id: own,
            grant,
        }],
        Some(GrantTargetBindings::default()),
        &[],
    );
    let external = matcher_authority(
        own,
        vec![AdmittedGrant::External {
            approval_id: 88,
            grant: Grant {
                resource: Resource::Queues,
                actions: vec![Action::Read],
                constraints: None,
            },
        }],
        Some(GrantTargetBindings::default()),
        &[88],
    );
    let mut tx = db.pool().begin().await.unwrap();
    assert_eq!(
        assert_queue_parity(&mut tx, &pinned, &[queue, foreign_queue]).await,
        vec![queue]
    );
    // The explicit private fallback in allows_target is Artifact-only. Queue
    // reference visibility gates targeting, not this metadata Read contract.
    assert_eq!(
        assert_queue_parity(&mut tx, &external, &[queue, foreign_queue])
            .await
            .len(),
        2
    );
    tx.rollback().await.unwrap();
    sqlx::query("DELETE FROM work_queue WHERE id = $1")
        .bind(queue)
        .execute(db.pool())
        .await
        .unwrap();
    let replacement: i64 = sqlx::query_scalar("INSERT INTO work_queue(ref,pack,pack_ref,label,dispatch_action_ref) VALUES('semantics.queue',$1,'semantics','Reused','semantics.run') RETURNING id")
        .bind(own).fetch_one(db.pool()).await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    assert!(
        assert_queue_parity(&mut tx, &pinned, &[replacement, foreign_queue])
            .await
            .is_empty()
    );
    // This unpinned matcher fixture intentionally shows the distinct contract:
    // None is open; capture normally pins concrete authored queue refs.
    assert_eq!(
        assert_queue_parity(&mut tx, &open_ref, &[replacement, foreign_queue]).await,
        vec![replacement]
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn removed_and_recreated_pack_names_do_not_replace_the_structural_owner_pin() {
    let (db, own, _, queue, _) = seed().await;
    let grant = Grant {
        resource: Resource::QueueItems,
        actions: vec![Action::Read],
        constraints: None,
    };
    let local = matcher_authority(
        own,
        vec![AdmittedGrant::PackOwned {
            pack_id: own,
            grant: grant.clone(),
        }],
        Some(GrantTargetBindings::default()),
        &[],
    );
    let pinned_external = matcher_authority(
        own,
        vec![AdmittedGrant::External {
            approval_id: 88,
            grant: grant.clone(),
        }],
        Some(GrantTargetBindings {
            packs: Some(BTreeSet::from([own])),
            ..Default::default()
        }),
        &[88],
    );
    let open_external = matcher_authority(
        own,
        vec![AdmittedGrant::External {
            approval_id: 88,
            grant,
        }],
        Some(GrantTargetBindings::default()),
        &[88],
    );
    sqlx::query("DELETE FROM pack WHERE id = $1")
        .bind(own)
        .execute(db.pool())
        .await
        .unwrap();
    let replacement: i64 = sqlx::query_scalar(
        "INSERT INTO pack(ref,label,version) VALUES('semantics','New owner','1.0.0') RETURNING id",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_ne!(replacement, own);
    sqlx::query("UPDATE work_queue SET pack = $1 WHERE id = $2")
        .bind(replacement)
        .bind(queue)
        .execute(db.pool())
        .await
        .unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    assert!(
        assert_item_parity(&mut tx, &local, &[queue, 700001, 700002])
            .await
            .is_empty()
    );
    assert!(
        assert_item_parity(&mut tx, &pinned_external, &[queue, 700001, 700002])
            .await
            .is_empty()
    );
    assert_eq!(
        assert_item_parity(&mut tx, &open_external, &[queue, 700001, 700002])
            .await
            .len(),
        3
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}
