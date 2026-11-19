//! Historic accepted-input edits must use the execution selection, not its owner IAM.
use super::*;

#[tokio::test]
async fn payload_edits_and_upserts_retain_selected_source_ceiling_and_attempt() -> Result<()> {
    use attune_common::repositories::{
        execution::UpdateExecutionInput, recorded_input_authority::RecordedInputAuthorityRepository,
    };
    let ctx = TestContext::new().await?.with_auth().await?;
    let login = "payload_editor_owner";
    let access = register_scoped_user(
        &ctx,
        login,
        json!([
            {"resource":"queues","actions":["create","read","update"]},
            {"resource":"queue_items","actions":["create","read","update"]},
            {"resource":"actions","actions":["execute"]},
            {"resource":"keys","actions":["read","decrypt"]}
        ]),
    )
    .await?;
    let owner = IdentityRepository::find_by_login(&ctx.pool, login)
        .await?
        .unwrap();
    for (reference, grants) in [
        (
            "test.payload_edit",
            json!([
                {"resource":"queues","actions":["read"]},
                {"resource":"queue_items","actions":["create","read","update"]}
            ]),
        ),
        (
            "test.payload_keys",
            json!([{"resource":"keys","actions":["read","decrypt"]}]),
        ),
    ] {
        let set = PermissionSetRepository::create(
            &ctx.pool,
            CreatePermissionSetInput {
                r#ref: reference.into(),
                pack: None,
                pack_ref: None,
                label: None,
                description: None,
                grants,
            },
        )
        .await?;
        PermissionAssignmentRepository::create(
            &ctx.pool,
            CreatePermissionAssignmentInput {
                identity: owner.id,
                permset: set.id,
            },
        )
        .await?;
    }
    let (_, action) = create_pack_with_action(&ctx, "payload_edit", "payload_edit.dispatch").await;
    let queue_ref = "adhoc.payload_edit";
    let created = ctx
        .post(
            "/api/v1/queues",
            json!({
                "ref":queue_ref,"label":"Payload editing","dispatch_action_ref":action.r#ref,
                "allow_pending_update":true,"update_strategy":"replace","permission_set_refs":[],
                "item_schema":{"value":{"type":"string"}}
            }),
            Some(&access),
        )
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let enqueued = ctx
        .post(
            &format!("/api/v1/queues/{queue_ref}/items"),
            json!({"item_key":"stable","payload":{"value":"original"}}),
            Some(&access),
        )
        .await?;
    assert_eq!(enqueued.status(), StatusCode::CREATED);
    let item_id = enqueued.json::<serde_json::Value>().await?["data"]["id"]
        .as_i64()
        .unwrap();
    // Model a retained accepted input snapshot, isolating the edit consumer.
    // This fixture is not production managed enqueue/readiness evidence.
    let mut tx = ctx.pool.begin().await?;
    RecordedInputAuthorityRepository::capture_human(&mut tx, "queue_item", item_id, owner.id)
        .await?;
    tx.commit().await?;
    let narrow =
        helpers::delegated_source::token(&ctx, owner.id, &["test.payload_edit".into()]).await?;
    let source = helpers::delegated_source::create(
        &ctx.pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            executor: Some(owner.id),
            permission_set_refs: vec!["test.payload_edit".into(), "test.payload_keys".into()],
            ..Default::default()
        },
    )
    .await?;
    let selected = helpers::delegated_source::execution_token(&ctx, source.id).await?;
    let item_path = format!("/api/v1/queues/{queue_ref}/items/{item_id}");
    let enqueue_path = format!("/api/v1/queues/{queue_ref}/items");
    for upsert in [false, true] {
        let body = json!({"item_key":"stable","payload":{"value":"unauthorized"}});
        let denied = if upsert {
            ctx.post(&enqueue_path, body, Some(&narrow)).await?
        } else {
            ctx.put(
                &item_path,
                json!({"payload":{"value":"unauthorized"}}),
                Some(&narrow),
            )
            .await?
        };
        assert_eq!(
            denied.status(),
            StatusCode::FORBIDDEN,
            "sponsor's unselected Key grants cannot authorize edits"
        );
        assert_eq!(
            WorkQueueItemRepository::find_by_id(&ctx.pool, item_id)
                .await?
                .unwrap()
                .payload,
            json!({"value":"original"})
        );
    }
    // A genuinely selected Key ceiling and the human Access credential both work.
    for bearer in [&selected, &access] {
        assert_eq!(
            ctx.put(
                &item_path,
                json!({"payload":{"value":"authorized"}}),
                Some(bearer)
            )
            .await?
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            ctx.post(
                &enqueue_path,
                json!({"item_key":"stable","payload":{"value":"authorized"}}),
                Some(bearer)
            )
            .await?
            .status(),
            StatusCode::OK
        );
    }
    ExecutionRepository::update(
        &ctx.pool,
        source.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    for upsert in [false, true] {
        let response = if upsert {
            ctx.post(
                &enqueue_path,
                json!({"item_key":"stable","payload":{"value":"stale"}}),
                Some(&selected),
            )
            .await?
        } else {
            ctx.put(
                &item_path,
                json!({"payload":{"value":"stale"}}),
                Some(&selected),
            )
            .await?
        };
        assert!(matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ));
        assert_eq!(
            WorkQueueItemRepository::find_by_id(&ctx.pool, item_id)
                .await?
                .unwrap()
                .payload,
            json!({"value":"authorized"})
        );
    }
    ctx.cleanup().await?;
    Ok(())
}
