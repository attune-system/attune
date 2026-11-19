//! VF-1 reproduction against the intentional invocation-capability contract.
//! Worker assignment is simulated; creation, Key delivery and provenance use the real API/DB.
use super::*;
use attune_common::{
    auth::jwt::generate_access_token,
    repositories::{
        delegated_execution::DelegatedExecutionRepository as Delegated,
        execution_key_read::ExecutionKeyReadRepository,
    },
};

#[tokio::test]
async fn cross_pack_standard_invocation_delivers_target_key_without_granting_caller_plaintext(
) -> TResult<()> {
    let ctx = TestContext::new().await?;
    let result: TResult<()> =
        async {
            let identity = create_execution_identity(&ctx.pool, "vf1_invoker").await?;
            let caller_pack = create_pack(&ctx.pool, "vf1_business", "Business").await?;
            let caller =
                create_action_with_default_access(&ctx.pool, &caller_pack, "invoke", vec![])
                    .await?;
            let provider_pack = create_pack(&ctx.pool, "vf1_provider", "Provider").await?;
            let provider = create_action_with_default_access(
                &ctx.pool,
                &provider_pack,
                "query",
                vec!["standard".into()],
            )
            .await?;
            let execute = create_permission_set(&ctx.pool, "test.vf1_execute", json!([
            {"resource":"actions","actions":["execute"],"constraints":{"refs":[provider.r#ref]}}
        ])).await?;
            assign_permission_set(&ctx.pool, &identity, &execute).await?;
            helpers::activate_test_pack_release(&ctx.pool, &caller_pack).await?;
            helpers::activate_test_pack_release(&ctx.pool, &provider_pack).await?;
            let protected = KeyRepository::create_encrypted_in_pool(
                &ctx.pool,
                CreateKeyInput {
                    local_ref: "credential".into(),
                    owner_type: OwnerType::Pack,
                    owner_pack: Some(provider_pack.id),
                    owner_pack_ref: Some(provider_pack.r#ref.clone()),
                    owner_identity: None,
                    owner_action: None,
                    owner_action_ref: None,
                    owner_sensor: None,
                    owner_sensor_ref: None,
                    name: "Credential".into(),
                    encrypted: true,
                    encryption_key_hash: None,
                    value: json!("vf1-dummy-secret"),
                },
                ctx.state.config.security.encryption_key.as_deref().unwrap(),
            )
            .await?;
            let key_path = format!("/api/v1/keys/{}", protected.r#ref);
            let access = generate_access_token(identity.id, &identity.login, &jwt_config())?;
            let refs = vec![execute.r#ref.clone(), "standard".into()];
            let parent = create_pinned_ad_hoc_parent(&ctx.pool, &caller, &identity, &refs).await?;
            let parent_token =
                execution_token(&ctx.pool, &identity, &parent, &caller, &refs).await?;

            // Both entry points can invoke the trusted target without a direct Key grant.
            // Exercise both resolved action defaults and an explicit standard selection.
            for (token, expected_parent) in [(&access, None), (&parent_token, Some(parent.id))] {
                let denied = ctx.get(&key_path, Some(token)).await?;
                assert_ne!(denied.status(), StatusCode::OK);
                assert!(!denied.text().await?.contains("vf1-dummy-secret"));
                for selection in [None, Some(vec!["standard".into()])] {
                    let response = ctx
                        .post(
                            "/api/v1/executions/execute",
                            execute_body(&provider, selection),
                            Some(token),
                        )
                        .await?;
                    assert_eq!(
                        response.status(),
                        StatusCode::CREATED,
                        "{}",
                        response.text().await?
                    );
                    let body: Value = response.json().await?;
                    let child = ExecutionRepository::find_by_id(
                        &ctx.pool,
                        body["data"]["id"].as_i64().unwrap(),
                    )
                    .await?
                    .unwrap();
                    assert_eq!(child.parent, expected_parent);
                    assert!(
                        child.workflow_task.is_none(),
                        "API invocations cannot manufacture workflow scope"
                    );
                    assert_eq!(child.permission_set_refs, vec!["standard".to_string()]);
                    let mut tx = ctx.pool.begin().await?;
                    start_delegated_fixture(&mut tx, child.id).await?;
                    tx.commit().await?;
                    let child_token = standard_execution_token(
                        &ctx.pool,
                        &identity,
                        &child,
                        &provider.r#ref,
                        std::slice::from_ref(&provider.r#ref),
                    )
                    .await?;
                    let delivered = ctx.get(&key_path, Some(&child_token)).await?;
                    assert_eq!(delivered.status(), StatusCode::OK);
                    let delivered: Value = delivered.json().await?;
                    assert_eq!(delivered["data"]["value"], json!("vf1-dummy-secret"));
                    let origins =
                        ExecutionKeyReadRepository::origins_for_executions(&ctx.pool, &[child.id])
                            .await?;
                    assert_eq!(origins[&child.id].len(), 1);
                    assert_eq!(origins[&child.id][0].key_id, protected.id);
                    assert_eq!(
                        origins[&child.id][0].owner_ref.as_deref(),
                        Some(provider_pack.r#ref.as_str())
                    );
                    assert!(origins[&child.id][0].encrypted);

                    // Even a correctly signed token cannot add unrelated caller scope.
                    let forged = standard_execution_token(
                        &ctx.pool,
                        &identity,
                        &child,
                        &provider.r#ref,
                        &[provider.r#ref.clone(), caller.r#ref.clone()],
                    )
                    .await?;
                    assert_eq!(
                        ctx.get(&key_path, Some(&forged)).await?.status(),
                        StatusCode::UNAUTHORIZED
                    );
                    let mut tx = ctx.pool.begin().await?;
                    Delegated::close(&mut tx, child.id).await?;
                    tx.commit().await?;
                    assert_eq!(
                        ctx.get(&key_path, Some(&child_token)).await?.status(),
                        StatusCode::UNAUTHORIZED
                    );
                }
            }
            // Execute by itself cannot add standard to a child when the parent did not select it.
            let refs = vec![execute.r#ref.clone()];
            let bounded = create_pinned_ad_hoc_parent(&ctx.pool, &caller, &identity, &refs).await?;
            let bounded_token =
                execution_token(&ctx.pool, &identity, &bounded, &caller, &refs).await?;
            for selection in [None, Some(vec!["standard".into()])] {
                assert_eq!(
                    ctx.post(
                        "/api/v1/executions/execute",
                        execute_body(&provider, selection),
                        Some(&bounded_token)
                    )
                    .await?
                    .status(),
                    StatusCode::FORBIDDEN
                );
            }
            // standard does not grant Execute on an unrelated action.
            assert_eq!(
                ctx.post(
                    "/api/v1/executions/execute",
                    execute_body(&caller, Some(vec!["standard".into()])),
                    Some(&parent_token)
                )
                .await?
                .status(),
                StatusCode::FORBIDDEN
            );
            Ok(())
        }
        .await;
    ctx.cleanup().await?;
    result
}

#[tokio::test]
async fn cross_pack_queue_acceptance_retains_contributor_selection_without_inventing_key_grants(
) -> TResult<()> {
    use attune_common::repositories::work_queue::WorkQueueItemRepository;
    let ctx = TestContext::new().await?;
    let result: TResult<()> = async {
        let identity = create_execution_identity(&ctx.pool, "vf2_invoker").await?;
        let business = create_pack(&ctx.pool, "vf2_business", "Business").await?;
        let caller = create_action_with_default_access(&ctx.pool, &business, "invoke", vec![]).await?;
        let provider_pack = create_pack(&ctx.pool, "vf2_provider", "Provider").await?;
        let provider = create_action_with_default_access(&ctx.pool, &provider_pack, "query", vec!["standard".into()]).await?;
        let producer = create_permission_set(&ctx.pool, "test.vf2_producer", json!([
            {"resource":"queues","actions":["create"]},
            {"resource":"actions","actions":["execute"],"constraints":{"refs":[provider.r#ref]}}
        ])).await?;
        assign_permission_set(&ctx.pool, &identity, &producer).await?;
        helpers::activate_test_pack_release(&ctx.pool, &business).await?;
        helpers::activate_test_pack_release(&ctx.pool, &provider_pack).await?;
        let refs = vec![producer.r#ref.clone(), "standard".into()];
        let parent = create_pinned_ad_hoc_parent(&ctx.pool, &caller, &identity, &refs).await?;
        let token = execution_token(&ctx.pool, &identity, &parent, &caller, &refs).await?;
        let mut accepted_items = Vec::new();
        for (name, selection) in [("explicit", Some(json!(["standard"]))), ("defaults", None)] {
            let queue_ref = format!("adhoc.vf2_{name}");
            let mut body = json!({"ref":queue_ref,"label":"VF2 queue","dispatch_action_ref":provider.r#ref,"item_schema":{}});
            if let Some(selection) = selection { body["permission_set_refs"] = selection; }
            let response = ctx.post("/api/v1/queues", body, Some(&token)).await?;
            assert_eq!(response.status(), StatusCode::CREATED, "{}", response.text().await?);
            let path = format!("/api/v1/queues/{queue_ref}/items");
            // Check source validity separately from queue admission for diagnostic clarity.
            let mut source_tx = ctx.pool.begin().await?;
            Delegated::validate(&mut source_tx, parent.id).await?;
            source_tx.rollback().await?;
            let accepted = ctx.post(&path, json!({"item_key":"accepted","payload":{}}), Some(&token)).await?;
            assert_eq!(accepted.status(), StatusCode::CREATED, "{}", accepted.text().await?);
            let body: Value = accepted.json().await?;
            let item = WorkQueueItemRepository::find_by_id(&ctx.pool, body["data"]["id"].as_i64().unwrap()).await?.unwrap();
            assert_eq!(item.requested_by_identity, Some(identity.id));
            assert_eq!(item.requested_by_execution, Some(parent.id));
            let mut tx = ctx.pool.begin().await?;
            assert_eq!(Delegated::queue_refs(&mut tx, std::slice::from_ref(&item)).await?, vec!["standard".to_string()]);
            tx.rollback().await?;
            accepted_items.push(item);
            let bounded_refs = vec![producer.r#ref.clone()];
            let bounded = create_pinned_ad_hoc_parent(&ctx.pool, &caller, &identity, &bounded_refs).await?;
            let bounded_token = execution_token(&ctx.pool, &identity, &bounded, &caller, &bounded_refs).await?;
            let before: i64 = sqlx::query_scalar("SELECT count(*) FROM work_queue_item").fetch_one(&ctx.pool).await?;
            let rejected = ctx.post(&path, json!({"item_key":"rejected","payload":{}}), Some(&bounded_token)).await?;
            assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
            let after: i64 = sqlx::query_scalar("SELECT count(*) FROM work_queue_item").fetch_one(&ctx.pool).await?;
            assert_eq!(before, after, "failed ceiling check must roll back acceptance");
        }
        // Queue acceptance is durable; closing the producer must stop new
        // submissions without invalidating selections already accepted.
        let mut tx = ctx.pool.begin().await?;
        Delegated::close(&mut tx, parent.id).await?;
        tx.commit().await?;
        let mut tx = ctx.pool.begin().await?;
        assert_eq!(Delegated::queue_refs(&mut tx, &accepted_items).await?, vec!["standard".to_string()]);
        tx.rollback().await?;
        let stale = ctx.post("/api/v1/queues/adhoc.vf2_explicit/items", json!({"item_key":"stale","payload":{}}), Some(&token)).await?;
        assert_eq!(stale.status(), StatusCode::UNAUTHORIZED);
        let missing_source = generate_delegated_execution_token(identity.id, -1, &caller.r#ref, &jwt_config(), 600, &refs, std::slice::from_ref(&caller.r#ref), uuid::Uuid::new_v4())?;
        let missing = ctx.post("/api/v1/queues/adhoc.vf2_explicit/items", json!({"item_key":"missing","payload":{}}), Some(&missing_source)).await?;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
        let active_source = create_pinned_ad_hoc_parent(&ctx.pool, &caller, &identity, &refs).await?;
        let attempt = Delegated::current_attempt(&mut *ctx.pool.acquire().await?, active_source.id).await?;
        let malformed_subject = generate_delegated_execution_token(0, active_source.id, &caller.r#ref, &jwt_config(), 600, &refs, std::slice::from_ref(&caller.r#ref), attempt)?;
        let malformed = ctx.post("/api/v1/queues/adhoc.vf2_explicit/items", json!({"item_key":"malformed","payload":{}}), Some(&malformed_subject)).await?;
        assert_eq!(malformed.status(), StatusCode::UNAUTHORIZED);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM work_queue_item").fetch_one(&ctx.pool).await?;
        assert_eq!(count, 2, "invalid sources must not persist items");
        Ok(())
    }.await;
    ctx.cleanup().await?;
    result
}

/// Ad hoc acceptance sponsors an execution; enqueue rights alone do not invoke
/// its target. Managed queue configuration has a separate admitted contract.
#[tokio::test]
async fn ad_hoc_queue_sponsorship_requires_target_execute_from_actual_credential() -> TResult<()> {
    let ctx = TestContext::new().await?;
    let result: TResult<()> =
        async {
            let owner = create_execution_identity(&ctx.pool, "queue_target_owner").await?;
            let enqueue_only =
                create_execution_identity(&ctx.pool, "queue_target_contributor").await?;
            let pack = create_pack(&ctx.pool, "queue_target", "Queue target").await?;
            let caller =
                create_action_with_default_access(&ctx.pool, &pack, "caller", vec![]).await?;
            let target =
                create_action_with_default_access(&ctx.pool, &pack, "target", vec![]).await?;
            let other =
                create_action_with_default_access(&ctx.pool, &pack, "other", vec![]).await?;
            let queue_grants = create_permission_set(
                &ctx.pool,
                "test.queue_target_enqueue",
                json!([
                    {"resource":"queues","actions":["create","update"]}
                ]),
            )
            .await?;
            let execute = create_permission_set(&ctx.pool, "test.queue_target_execute", json!([
            {"resource":"actions","actions":["execute"],"constraints":{"refs":[target.r#ref]}}
        ])).await?;
            assign_permission_set(&ctx.pool, &owner, &queue_grants).await?;
            assign_permission_set(&ctx.pool, &owner, &execute).await?;
            assign_permission_set(&ctx.pool, &enqueue_only, &queue_grants).await?;
            helpers::activate_test_pack_release(&ctx.pool, &pack).await?;
            let access = generate_access_token(owner.id, &owner.login, &jwt_config())?;
            let contributor =
                generate_access_token(enqueue_only.id, &enqueue_only.login, &jwt_config())?;
            let response = ctx.post("/api/v1/queues", json!({
            "ref":"adhoc.target_execute", "label":"Ad hoc sponsorship",
            "dispatch_action_ref":target.r#ref, "permission_set_refs":[], "item_schema":{}
        }), Some(&access)).await?;
            assert_eq!(
                response.status(),
                StatusCode::CREATED,
                "{}",
                response.text().await?
            );
            let narrow_refs = vec![queue_grants.r#ref.clone()];
            let source =
                create_pinned_ad_hoc_parent(&ctx.pool, &caller, &owner, &narrow_refs).await?;
            let narrow = execution_token(&ctx.pool, &owner, &source, &caller, &narrow_refs).await?;
            let broad_refs = vec![queue_grants.r#ref.clone(), execute.r#ref.clone()];
            let source =
                create_pinned_ad_hoc_parent(&ctx.pool, &caller, &owner, &broad_refs).await?;
            let broad = execution_token(&ctx.pool, &owner, &source, &caller, &broad_refs).await?;
            let path = "/api/v1/queues/adhoc.target_execute/items";
            for (index, token) in [&contributor, &narrow].into_iter().enumerate() {
                for bulk in [false, true] {
                    let item = json!({"item_key":format!("denied{index}-{bulk}"),"payload":{}});
                    let (path, body) = if bulk {
                        (format!("{path}/bulk"), json!({"items":[item]}))
                    } else {
                        (path.into(), item)
                    };
                    let response = ctx.post(&path, body, Some(token)).await?;
                    assert_eq!(
                        response.status(),
                        StatusCode::FORBIDDEN,
                        "{}",
                        response.text().await?
                    );
                }
            }
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM work_queue_item")
                .fetch_one(&ctx.pool)
                .await?;
            assert_eq!(count, 0, "denied sponsorship must not persist items");
            for (index, token) in [&access, &broad].into_iter().enumerate() {
                for bulk in [false, true] {
                    let item = json!({"item_key":format!("allowed{index}-{bulk}"),"payload":{}});
                    let (path, body) = if bulk {
                        (format!("{path}/bulk"), json!({"items":[item]}))
                    } else {
                        (path.into(), item)
                    };
                    let response = ctx.post(&path, body, Some(token)).await?;
                    assert_eq!(
                        response.status(),
                        StatusCode::CREATED,
                        "{}",
                        response.text().await?
                    );
                }
            }
            // Definition edits cannot reinterpret already accepted executable pins.
            let response = ctx
                .put(
                    "/api/v1/queues/adhoc.target_execute",
                    json!({"dispatch_action_ref":other.r#ref}),
                    Some(&access),
                )
                .await?;
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "{}",
                response.text().await?
            );
            let snapshots: Vec<sqlx::types::Json<ExecutionExecutableSnapshot>> =
                sqlx::query_scalar("SELECT executable_snapshot FROM work_queue_item")
                    .fetch_all(&ctx.pool)
                    .await?;
            assert_eq!(snapshots.len(), 4);
            for snapshot in snapshots {
                assert_eq!(snapshot.executable.action.id, target.id);
            }
            // An empty queue can be retargeted. New acceptance must check
            // that target rather than the previously authorized action.
            let response = ctx.post("/api/v1/queues", json!({
                "ref":"adhoc.redirected", "label":"Redirect", "dispatch_action_ref":target.r#ref,
                "permission_set_refs":[], "item_schema":{}
            }), Some(&access)).await?;
            assert_eq!(
                response.status(),
                StatusCode::CREATED,
                "{}",
                response.text().await?
            );
            let response = ctx
                .put(
                    "/api/v1/queues/adhoc.redirected",
                    json!({"dispatch_action_ref":other.r#ref}),
                    Some(&access),
                )
                .await?;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{}",
                response.text().await?
            );
            for token in [&access, &broad] {
                for bulk in [false, true] {
                    let path = if bulk {
                        "/api/v1/queues/adhoc.redirected/items/bulk"
                    } else {
                        "/api/v1/queues/adhoc.redirected/items"
                    };
                    let item = json!({"item_key":"redirect-denied", "payload":{}});
                    let body = if bulk { json!({"items":[item]}) } else { item };
                    let response = ctx.post(path, body, Some(token)).await?;
                    assert_eq!(
                        response.status(),
                        StatusCode::FORBIDDEN,
                        "{}",
                        response.text().await?
                    );
                }
            }
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM work_queue_item")
                .fetch_one(&ctx.pool)
                .await?;
            assert_eq!(count, 4);
            Ok(())
        }
        .await;
    ctx.cleanup().await?;
    result
}
