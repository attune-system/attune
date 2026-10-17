//! Actual router requests with repository-owned source/target barriers.
use super::*;
use crate::helpers::TestResponse;
use sqlx::PgPool;
use tokio::task::JoinHandle;

fn body(operation: &str, path: &str, limit: u32) -> Value {
    let mut body =
        json!({"operation":operation,"selector":{"path":path,"vars":{}},"preview_limit":limit});
    if operation == "reprioritize" {
        body["priority"] = json!(17);
    }
    body
}

async fn seed(fixture: &Fixture, reference: &str) -> Result<attune_common::models::WorkQueue> {
    let queue = managed_queue(fixture, reference).await?;
    sqlx::query("INSERT INTO work_queue_item(id,queue,queue_ref,item_key,payload,metadata,ack_summary,requested_by_execution,requested_by_enforcement,enqueue_source) SELECT n,$1,$2,n::text,'{\"group\":\"visible\",\"version\":\"old\"}'::jsonb,'{\"peer_secret\":\"private_batch_value\"}'::jsonb,'{\"peer\":\"private_batch_value\"}'::jsonb,$3,910007,'bulk-fixture' FROM generate_series(810001,810003) n")
        .bind(queue.id).bind(&queue.r#ref).bind(fixture.credential.execution_id).execute(&fixture.ctx.pool).await?;
    // Retained/stale lease data must stay invisible even on mutable rows.
    sqlx::query("UPDATE work_queue_item SET leased_execution = $2,lease_token = 'b1a61905-1b0a-4b44-8d5c-8ee1fca93025',lease_expires_at = clock_timestamp(),last_error = '{\"peer\":\"private_batch_value\"}' WHERE queue = $1")
        .bind(queue.id).bind(fixture.credential.execution_id).execute(&fixture.ctx.pool).await?;
    Ok(queue)
}

async fn metadata_fixture(
    read_ids: Option<Vec<i64>>,
    operation_ids: Option<Vec<i64>>,
) -> Result<Fixture> {
    let mut read = json!({"resource":"queue_items","actions":["read"]});
    let mut operation = json!({"resource":"queue_items","actions":["update","delete"]});
    if let Some(ids) = read_ids {
        read["constraints"] = json!({"ids":ids});
    }
    if let Some(ids) = operation_ids {
        operation["constraints"] = json!({"ids":ids});
    }
    // Authored definitions reject DB IDs. These are server-side admitted
    // narrowing fixtures, captured through the real policy/admission repos.
    let mut fixture = fixture_with_additional_grants(false, false, &[], &[], &[]).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let mut policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap()
        .policy
        .0;
    for grant in [read, operation] {
        policy.invocations[0]
            .permission_sets
            .get_mut("recorded_emitter.events")
            .unwrap()
            .grants
            .push(attune_common::runtime_authority::AdmittedGrant::PackOwned {
                pack_id: fixture.pack_id,
                grant: serde_json::from_value(grant)?,
            });
    }
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy).await?;
    tx.commit().await?;
    fixture.policy_id = policy.id;
    let (credential, token) = independent_source(&fixture).await?;
    fixture.credential = credential;
    fixture.token = token;
    Ok(fixture)
}

fn spawn_request(fixture: &Fixture, path: String, body: Value) -> JoinHandle<TestResponse> {
    use axum::{
        body::Body,
        http::{header, Request},
    };
    use tower::Service;
    let mut app = fixture.ctx.app.clone();
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {}", fixture.token))
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    tokio::spawn(async move {
        TestResponse::new(
            app.call(request)
                .await
                .expect("metadata bulk router request"),
        )
    })
}

async fn finish<T>(mut task: JoinHandle<T>) -> Result<T> {
    match tokio::time::timeout(std::time::Duration::from_secs(15), &mut task).await {
        Ok(result) => Ok(result?),
        Err(_) => {
            task.abort();
            let _ = task.await;
            Err("metadata bulk operation exceeded deadlock bound".into())
        }
    }
}

async fn stop<T>(task: Option<JoinHandle<T>>) -> Result<()> {
    if let Some(task) = task {
        task.abort();
        if let Err(error) = task.await {
            if !error.is_cancelled() {
                return Err(error.into());
            }
        }
    }
    Ok(())
}

async fn cleanup_barrier(
    blocker: Option<sqlx::Transaction<'static, sqlx::Postgres>>,
    effect: Option<JoinHandle<TestResponse>>,
    writer: Option<JoinHandle<()>>,
) -> Result<()> {
    if let Some(task) = &effect {
        task.abort();
    }
    if let Some(task) = &writer {
        task.abort();
    }
    let rollback = match blocker {
        Some(blocker) => blocker.rollback().await,
        None => Ok(()),
    };
    let effect = stop(effect).await;
    let writer = stop(writer).await;
    rollback?;
    effect?;
    writer?;
    Ok(())
}

async fn blocked_effect(pool: &PgPool, blocker: i32) -> Result<i32> {
    Ok(tokio::time::timeout(std::time::Duration::from_secs(10),async {
        loop {
            let pid: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) AND query LIKE '%work_queue_item%' AND query LIKE '%FOR UPDATE%' LIMIT 1")
                .bind(blocker).fetch_optional(pool).await?;
            if let Some(pid) = pid { break Ok::<_,sqlx::Error>(pid); }
            tokio::task::yield_now().await;
        }
    }).await??)
}

#[tokio::test]
async fn metadata_bulk_preview_and_effect_share_visible_selection_and_independent_item_rights(
) -> Result<()> {
    for operation in ["cancel", "reprioritize"] {
        let fixture = metadata_fixture(Some(vec![810001, 810002]), None).await?;
        let queue = seed(&fixture, "recorded_emitter.bulk_visible").await?;
        let base = format!("/api/v1/queues/{}/items/query", queue.r#ref);
        for hidden in [
            "$ ? (@.metadata.peer_secret == \"private_batch_value\")".to_owned(),
            "$ ? (@.ack_summary.peer == \"private_batch_value\")".to_owned(),
            "$ ? (@.last_error.peer == \"private_batch_value\")".to_owned(),
            format!(
                "$ ? (@.requested_by_execution == {})",
                fixture.credential.execution_id
            ),
            "$ ? (@.requested_by_enforcement == 910007)".to_owned(),
            "$ ? (exists(@.leased_execution) || exists(@.lease_token))".to_owned(),
        ] {
            let preview = fixture
                .ctx
                .post(
                    &format!("{base}/preview"),
                    json!({"selector":{"path":hidden,"vars":{}},"limit":1}),
                    Some(&fixture.token),
                )
                .await?;
            assert_eq!(preview.status(), StatusCode::OK);
            let preview: Value = preview.json().await?;
            let effect = fixture
                .ctx
                .post(
                    &format!("{base}/apply"),
                    body(operation, &hidden, 1),
                    Some(&fixture.token),
                )
                .await?;
            assert_eq!(effect.status(), StatusCode::OK);
            let effect: Value = effect.json().await?;
            assert_eq!(preview["data"]["matched_count"], 0);
            assert_eq!(effect["data"]["matched_count"], 0);
            assert_eq!(effect["data"]["affected_count"], 0);
        }
        let preview = fixture
            .ctx
            .post(
                &format!("{base}/preview"),
                json!({"selector":{"path":"$","vars":{}},"limit":1}),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(preview.status(), StatusCode::OK);
        let preview: Value = preview.json().await?;
        let effect = fixture
            .ctx
            .post(
                &format!("{base}/apply"),
                body(operation, "$", 1),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(effect.status(), StatusCode::OK);
        let effect: Value = effect.json().await?;
        assert_eq!(effect["data"]["matched_count"], 2);
        assert_eq!(effect["data"]["affected_count"], 2);
        assert_eq!(effect["data"]["preview_count"], 1);
        assert_eq!(
            effect["data"]["items"][0]["id"],
            preview["data"]["items"][0]["id"]
        );
        assert_eq!(
            preview["data"]["matched_count"],
            effect["data"]["matched_count"]
        );
        assert!(!effect.to_string().contains("private_batch_value"));
        let rows: Vec<(i64, String, i32)> = sqlx::query_as(
            "SELECT id,status::text,priority FROM work_queue_item WHERE queue = $1 ORDER BY id",
        )
        .bind(queue.id)
        .fetch_all(&fixture.ctx.pool)
        .await?;
        assert_eq!(rows[2], (810003, "queued".into(), 0));
        for row in &rows[..2] {
            assert_eq!(
                row.1,
                if operation == "cancel" {
                    "cancelled"
                } else {
                    "queued"
                }
            );
            assert_eq!(row.2, if operation == "reprioritize" { 17 } else { 0 });
        }
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn metadata_bulk_operation_scope_cannot_turn_read_into_blind_collection_write() -> Result<()>
{
    let fixture = metadata_fixture(None, Some(vec![810001])).await?;
    let queue = seed(&fixture, "recorded_emitter.bulk_operation_scope").await?;
    for operation in ["cancel", "reprioritize"] {
        let response = fixture
            .ctx
            .post(
                &format!("/api/v1/queues/{}/items/query/apply", queue.r#ref),
                body(operation, "$", 100),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let unchanged: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_queue_item WHERE queue = $1 AND status = 'queued' AND priority = 0")
        .bind(queue.id).fetch_one(&fixture.ctx.pool).await?;
    assert_eq!(unchanged, 3);
    let response = fixture
        .ctx
        .post(
            &format!("/api/v1/queues/{}/items/query/apply", queue.r#ref),
            body("reprioritize", "$ ? (@.item_key == \"810001\")", 100),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await?;
    assert_eq!(response["data"]["affected_count"], 1);
    assert_eq!(response["data"]["items"][0]["id"], 810001);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn metadata_bulk_rechecks_changed_payload_after_initial_selection_and_item_lock_wait(
) -> Result<()> {
    for (operation, selector) in [
        ("cancel", "$"),
        ("reprioritize", "$"),
        ("cancel", "$ ? (@.payload.group == \"visible\")"),
        ("reprioritize", "$ ? (@.payload.group == \"visible\")"),
    ] {
        let fixture = metadata_fixture(None, None).await?;
        let queue = seed(&fixture, "recorded_emitter.bulk_drift").await?;
        let mut blocker = Some(fixture.ctx.pool.begin().await?);
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut **blocker.as_mut().unwrap())
            .await?;
        sqlx::query("SELECT id FROM work_queue_item WHERE id = 810001 FOR UPDATE")
            .fetch_one(&mut **blocker.as_mut().unwrap())
            .await?;
        let mut effect = Some(spawn_request(
            &fixture,
            format!("/api/v1/queues/{}/items/query/apply", queue.r#ref),
            body(operation, selector, 100),
        ));
        let outcome: Result<()> = async {
            blocked_effect(&fixture.ctx.pool,blocker_pid).await?;
            // '$' retains its ID set, while the narrowed selector loses a match.
            // Either change must reject the complete initial selection.
            sqlx::query("UPDATE work_queue_item SET payload = '{\"group\":\"changed\",\"version\":\"fresh\"}' WHERE id = 810001")
                .execute(&mut **blocker.as_mut().unwrap()).await?;
            blocker.take().unwrap().commit().await?;
            let response = finish(effect.take().unwrap()).await?;
            if response.status() != StatusCode::CONFLICT { return Err("selection drift did not reject the entire metadata operation".into()); }
            let unchanged: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_queue_item WHERE queue = $1 AND status = 'queued' AND priority = 0")
                .bind(queue.id).fetch_one(&fixture.ctx.pool).await?;
            if unchanged != 3 { return Err("selection drift changed a queued target".into()); }
            Ok(())
        }.await;
        let cleanup = cleanup_barrier(blocker, effect, None).await;
        let database_cleanup = fixture.ctx.cleanup().await;
        cleanup?;
        database_cleanup?;
        outcome?;
    }
    Ok(())
}

#[tokio::test]
async fn metadata_bulk_does_not_reenter_graph_admission_after_item_wait_with_queued_writer(
) -> Result<()> {
    let fixture = metadata_fixture(None, None).await?;
    let queue = seed(&fixture, "recorded_emitter.bulk_graph_wait").await?;
    let mut blocker = Some(fixture.ctx.pool.begin().await?);
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **blocker.as_mut().unwrap())
        .await?;
    sqlx::query("SELECT id FROM work_queue_item WHERE id = 810001 FOR UPDATE")
        .fetch_one(&mut **blocker.as_mut().unwrap())
        .await?;
    let mut effect = Some(spawn_request(
        &fixture,
        format!("/api/v1/queues/{}/items/query/apply", queue.r#ref),
        body("reprioritize", "$", 100),
    ));
    let mut writer = None;
    let outcome: Result<()> = async {
        let effect_pid = blocked_effect(&fixture.ctx.pool, blocker_pid).await?;
        let pool = fixture.ctx.pool.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        writer = Some(tokio::spawn(async move {
            let mut tx = pool.begin().await.unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            let _ = send.send(pid);
            attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(
                &mut tx,
            )
            .await
            .unwrap();
            tx.rollback().await.unwrap();
        }));
        let writer_pid =
            tokio::time::timeout(std::time::Duration::from_secs(10), receive).await??;
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT $1 = ANY(pg_blocking_pids($2))")
                    .bind(effect_pid)
                    .bind(writer_pid)
                    .fetch_one(&fixture.ctx.pool)
                    .await?;
                if blocked {
                    break Ok::<_, sqlx::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        blocker.take().unwrap().commit().await?;
        let response = finish(effect.take().unwrap()).await?;
        finish(writer.take().unwrap()).await?;
        if response.status() != StatusCode::OK {
            return Err("queued graph writer changed the metadata operation outcome".into());
        }
        let affected: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_queue_item WHERE queue = $1 AND priority = 17",
        )
        .bind(queue.id)
        .fetch_one(&fixture.ctx.pool)
        .await?;
        if affected != 3 {
            return Err("queued graph writer prevented the exact metadata batch".into());
        }
        Ok(())
    }
    .await;
    let cleanup = cleanup_barrier(blocker, effect, writer).await;
    let database_cleanup = fixture.ctx.cleanup().await;
    cleanup?;
    database_cleanup?;
    outcome
}

#[tokio::test]
async fn metadata_bulk_locked_selection_detects_new_readable_matches_not_private_row_churn(
) -> Result<()> {
    for new_match in [true, false] {
        let fixture = metadata_fixture(None, None).await?;
        let queue = seed(&fixture, "recorded_emitter.bulk_membership").await?;
        let mut blocker = Some(fixture.ctx.pool.begin().await?);
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut **blocker.as_mut().unwrap())
            .await?;
        sqlx::query("SELECT id FROM work_queue_item WHERE id = 810001 FOR UPDATE")
            .fetch_one(&mut **blocker.as_mut().unwrap())
            .await?;
        let mut effect = Some(spawn_request(
            &fixture,
            format!("/api/v1/queues/{}/items/query/apply", queue.r#ref),
            body("reprioritize", "$", 100),
        ));
        let outcome: Result<()> = async {
            blocked_effect(&fixture.ctx.pool, blocker_pid).await?;
            if new_match {
                sqlx::query("INSERT INTO work_queue_item(queue,queue_ref,payload,enqueue_source) VALUES($1,$2,'{}','new-match')")
                    .bind(queue.id).bind(&queue.r#ref).execute(&mut **blocker.as_mut().unwrap()).await?;
            } else {
                // A private-only change cannot become a conflict/count oracle.
                sqlx::query("UPDATE work_queue_item SET metadata = '{\"peer_secret\":\"changed_private_value\"}',ack_summary = '{\"peer\":\"changed_private_value\"}',requested_by_execution = 910009 WHERE id = 810001")
                    .execute(&mut **blocker.as_mut().unwrap()).await?;
            }
            blocker.take().unwrap().commit().await?;
            let response = finish(effect.take().unwrap()).await?;
            let expected = if new_match { StatusCode::CONFLICT } else { StatusCode::OK };
            if response.status() != expected { return Err(format!("membership/private churn response was {}, expected {expected}",response.status()).into()); }
            let changed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_queue_item WHERE queue = $1 AND priority = 17")
                .bind(queue.id).fetch_one(&fixture.ctx.pool).await?;
            if changed != if new_match { 0 } else { 3 } { return Err("membership/private churn changed the wrong target set".into()); }
            if !new_match {
                let response: Value = response.json().await?;
                if response["data"]["affected_count"] != 3 || response.to_string().contains("changed_private_value") {
                    return Err("private row churn affected metadata visibility".into());
                }
            }
            Ok(())
        }.await;
        let cleanup = cleanup_barrier(blocker, effect, None).await;
        let database_cleanup = fixture.ctx.cleanup().await;
        cleanup?;
        database_cleanup?;
        outcome?;
    }
    Ok(())
}

#[tokio::test]
async fn metadata_bulk_fixed_cancel_allows_taint_but_priority_is_a_guarded_sink() -> Result<()> {
    use attune_common::repositories::runtime_read::{
        RuntimeReadOrigin, RuntimeReadRepository, RuntimeReadTarget,
    };
    let fixture = metadata_fixture(None, None).await?;
    let queue = seed(&fixture, "recorded_emitter.bulk_taint").await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let authority =
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
            .await?;
    RuntimeReadRepository::record(
        &mut tx,
        &authority,
        RuntimeReadOrigin {
            target: RuntimeReadTarget::Execution {
                execution: fixture.credential.execution_id,
            },
            provenance: vec![],
            parents: vec![],
        },
    )
    .await?;
    tx.commit().await?;
    let path = format!("/api/v1/queues/{}/items/query/apply", queue.r#ref);
    let response = fixture
        .ctx
        .post(&path, body("reprioritize", "$", 100), Some(&fixture.token))
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let mut cancel = body(
        "cancel",
        "$ ? (@.payload.group == $tainted_selector_needle)",
        100,
    );
    cancel["selector"]["vars"] = json!({"tainted_selector_needle":"visible"});
    let response = fixture
        .ctx
        .post(&path, cancel, Some(&fixture.token))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await?;
    assert_eq!(response["data"]["affected_count"], 3);
    fixture.ctx.flush_audit().await?;
    let details: Vec<Value> = sqlx::query_scalar("SELECT details FROM audit_event WHERE event_type = 'queue_items.bulk_operation.applied' AND resource_id = $1")
        .bind(queue.id).fetch_all(&fixture.ctx.pool).await?;
    assert!(!details.is_empty());
    assert!(details.iter().all(
        |details| !details.to_string().contains("tainted_selector_needle")
            && details.get("selector_path").is_none()
            && details.get("selector_var_keys").is_none()
    ));
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn metadata_bulk_revoked_execution_does_not_revoke_independent_same_sponsor_invocation(
) -> Result<()> {
    let fixture = metadata_fixture(None, None).await?;
    let queue = seed(&fixture, "recorded_emitter.bulk_revocation").await?;
    let (credential, token) = independent_source(&fixture).await?;
    assert_eq!(
        credential.sponsor_identity_id,
        fixture.credential.sponsor_identity_id
    );
    let mut tx = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_admission(&mut tx, fixture.credential.admission_id).await?;
    tx.commit().await?;
    let path = format!("/api/v1/queues/{}/items/query/apply", queue.r#ref);
    assert_eq!(
        fixture
            .ctx
            .post(&path, body("cancel", "$", 100), Some(&fixture.token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = fixture
        .ctx
        .post(&path, body("reprioritize", "$", 100), Some(&token))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await?;
    assert_eq!(response["data"]["affected_count"], 3);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn metadata_bulk_never_borrows_broader_item_rights_from_an_independent_same_sponsor_invocation(
) -> Result<()> {
    let mut fixture = metadata_fixture(Some(vec![810001]), Some(vec![810001])).await?;
    let queue = seed(&fixture, "recorded_emitter.bulk_sponsor_scope").await?;
    let narrow = fixture.credential.clone();
    let narrow_token = fixture.token.clone();
    append_local_reader(
        &mut fixture,
        json!({"resource":"queue_items","actions":["read","update","delete"]}),
    )
    .await?;
    let (broad, broad_token) = independent_source(&fixture).await?;
    assert_eq!(narrow.sponsor_identity_id, broad.sponsor_identity_id);
    assert_ne!(narrow.execution_id, broad.execution_id);
    let path = format!("/api/v1/queues/{}/items/query/apply", queue.r#ref);
    let response = fixture
        .ctx
        .post(&path, body("reprioritize", "$", 100), Some(&narrow_token))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await?;
    assert_eq!(response["data"]["matched_count"], 1);
    assert_eq!(response["data"]["affected_count"], 1);
    assert_eq!(response["data"]["items"][0]["id"], 810001);
    let unchanged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_queue_item WHERE id = ANY($1) AND priority = 0",
    )
    .bind(vec![810002_i64, 810003])
    .fetch_one(&fixture.ctx.pool)
    .await?;
    assert_eq!(unchanged, 2);
    let response = fixture
        .ctx
        .post(&path, body("cancel", "$", 100), Some(&broad_token))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await?;
    assert_eq!(response["data"]["affected_count"], 3);
    fixture.ctx.cleanup().await?;
    Ok(())
}

async fn select_external_metadata(
    fixture: &mut Fixture,
    expires: Option<chrono::DateTime<chrono::Utc>>,
    actions: &[&str],
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let grant: attune_common::rbac::Grant =
        serde_json::from_value(json!({"resource":"queue_items","actions":actions}))?;
    let issuer = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.bulk_external_issuer".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: serde_json::to_value(vec![grant.clone()])?,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: issuer.id,
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        fixture.pack_id,
        fixture.requester,
        vec![grant.clone()],
        expires,
    )
    .await?;
    let mut policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap()
        .policy
        .0;
    policy.invocations[0]
        .permission_sets
        .get_mut("recorded_emitter.events")
        .unwrap()
        .grants
        .push(attune_common::runtime_authority::AdmittedGrant::External {
            approval_id: approval.id,
            grant,
        });
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy).await?;
    tx.commit().await?;
    fixture.policy_id = policy.id;
    let (credential, token) = independent_source(fixture).await?;
    fixture.credential = credential;
    fixture.token = token;
    Ok(approval.expires_at)
}

#[tokio::test]
async fn metadata_bulk_expiry_during_actual_item_wait_rolls_back_every_target() -> Result<()> {
    let mut fixture = metadata_fixture(None, None).await?;
    let queue = seed(&fixture, "recorded_emitter.bulk_expiry").await?;
    let deadline = select_external_metadata(
        &mut fixture,
        Some(chrono::Utc::now() + chrono::Duration::seconds(20)),
        &["read"],
    )
    .await?
    .unwrap();
    let mut blocker = Some(fixture.ctx.pool.begin().await?);
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **blocker.as_mut().unwrap())
        .await?;
    sqlx::query("SELECT id FROM work_queue_item WHERE id = 810001 FOR UPDATE")
        .fetch_one(&mut **blocker.as_mut().unwrap())
        .await?;
    let mut effect = Some(spawn_request(
        &fixture,
        format!("/api/v1/queues/{}/items/query/apply", queue.r#ref),
        body("cancel", "$", 100),
    ));
    let outcome: Result<()> = async {
        blocked_effect(&fixture.ctx.pool,blocker_pid).await?;
        tokio::time::timeout(std::time::Duration::from_secs(30),async {
            loop {
                let expired: bool = sqlx::query_scalar("SELECT clock_timestamp() >= $1").bind(deadline).fetch_one(&fixture.ctx.pool).await?;
                if expired { break Ok::<_,sqlx::Error>(()); }
                tokio::task::yield_now().await;
            }
        }).await??;
        blocker.take().unwrap().commit().await?;
        let response = finish(effect.take().unwrap()).await?;
        if response.status() != StatusCode::FORBIDDEN { return Err("expired metadata operation was not denied".into()); }
        let unchanged: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_queue_item WHERE queue = $1 AND status = 'queued' AND priority = 0")
            .bind(queue.id).fetch_one(&fixture.ctx.pool).await?;
        if unchanged != 3 { return Err("expired metadata operation changed a target".into()); }
        Ok(())
    }.await;
    let cleanup = cleanup_barrier(blocker, effect, None).await;
    let database_cleanup = fixture.ctx.cleanup().await;
    cleanup?;
    database_cleanup?;
    outcome
}

#[tokio::test]
async fn metadata_bulk_external_item_rights_allow_existing_ad_hoc_items_without_readmission(
) -> Result<()> {
    let mut fixture = metadata_fixture(None, None).await?;
    let queue: i64 = sqlx::query_scalar("INSERT INTO work_queue(ref,pack,pack_ref,is_adhoc,label,dispatch_action_ref) VALUES('recorded_emitter.bulk_external',$1,'recorded_emitter',true,'Existing ad hoc queue','recorded_emitter.emit') RETURNING id")
        .bind(fixture.pack_id).fetch_one(&fixture.ctx.pool).await?;
    sqlx::query("INSERT INTO work_queue_item(queue,queue_ref,payload,enqueue_source) VALUES($1,'recorded_emitter.bulk_external','{\"accepted\":true}','human-fixture')")
        .bind(queue).execute(&fixture.ctx.pool).await?;
    let path = "/api/v1/queues/recorded_emitter.bulk_external/items/query/apply";
    // Pack association is not management. Local grants alone cannot select or
    // change the ad hoc item, regardless of queue or item numeric IDs.
    let response = fixture
        .ctx
        .post(path, body("reprioritize", "$", 100), Some(&fixture.token))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await?;
    assert_eq!(response["data"]["affected_count"], 0);
    select_external_metadata(&mut fixture, None, &["read", "update", "delete"]).await?;
    for operation in ["reprioritize", "cancel"] {
        let response = fixture
            .ctx
            .post(path, body(operation, "$", 100), Some(&fixture.token))
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let response: Value = response.json().await?;
        assert_eq!(response["data"]["matched_count"], 1);
        assert_eq!(response["data"]["affected_count"], 1);
    }
    let payload: Value = sqlx::query_scalar("SELECT payload FROM work_queue_item WHERE queue = $1")
        .bind(queue)
        .fetch_one(&fixture.ctx.pool)
        .await?;
    assert_eq!(payload, json!({"accepted":true}));
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn metadata_bulk_bound_applies_after_read_authorization_and_never_silently_truncates(
) -> Result<()> {
    for narrow in [true, false] {
        let fixture = metadata_fixture(narrow.then_some(vec![810001, 810002]), None).await?;
        let queue = seed(&fixture, "recorded_emitter.bulk_bound").await?;
        let extra = if narrow { 10001 } else { 1000 };
        sqlx::query("INSERT INTO work_queue_item(queue,queue_ref,payload,enqueue_source) SELECT $1,$2,'{}'::jsonb,'bulk-bound' FROM generate_series(1,$3::integer)")
            .bind(queue.id).bind(&queue.r#ref).bind(extra).execute(&fixture.ctx.pool).await?;
        let response = fixture
            .ctx
            .post(
                &format!("/api/v1/queues/{}/items/query/apply", queue.r#ref),
                body("cancel", "$", 1),
                Some(&fixture.token),
            )
            .await?;
        assert_eq!(
            response.status(),
            if narrow {
                StatusCode::OK
            } else {
                StatusCode::BAD_REQUEST
            }
        );
        let cancelled: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_queue_item WHERE queue = $1 AND status = 'cancelled'",
        )
        .bind(queue.id)
        .fetch_one(&fixture.ctx.pool)
        .await?;
        assert_eq!(cancelled, if narrow { 2 } else { 0 });
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn metadata_selector_errors_cannot_probe_payloads_outside_the_read_scope() -> Result<()> {
    let fixture = metadata_fixture(Some(vec![810001, 810002]), None).await?;
    let queue = seed(&fixture, "recorded_emitter.bulk_selector_scope").await?;
    sqlx::query("UPDATE work_queue_item SET payload = '{\"probe\":true}' WHERE id = ANY($1)")
        .bind(vec![810001_i64, 810002])
        .execute(&fixture.ctx.pool)
        .await?;
    // This strict path would error on unreadable item 810003's missing key if
    // evaluated before authorization. Both readers must use the materialized
    // Read-scoped document, not rely on SQL AND evaluation order.
    let selector = "strict $.payload.probe";
    let base = format!("/api/v1/queues/{}/items/query", queue.r#ref);
    let response = fixture
        .ctx
        .post(
            &format!("{base}/preview"),
            json!({"selector":{"path":selector,"vars":{}},"limit":100}),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await?;
    assert_eq!(response["data"]["matched_count"], 2);
    let response = fixture
        .ctx
        .post(
            &format!("{base}/apply"),
            body("reprioritize", selector, 100),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await?;
    assert_eq!(response["data"]["affected_count"], 2);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn metadata_bulk_can_control_accepted_items_after_ordinary_queue_retirement() -> Result<()> {
    let fixture = metadata_fixture(None, None).await?;
    let queue = seed(&fixture, "recorded_emitter.bulk_retired").await?;
    sqlx::query("UPDATE work_queue SET enabled = false,accepting_new_items = false,retired_at = clock_timestamp() WHERE id = $1")
        .bind(queue.id).execute(&fixture.ctx.pool).await?;
    let response = fixture
        .ctx
        .post(
            &format!("/api/v1/queues/{}/items/query/apply", queue.r#ref),
            body("cancel", "$", 100),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await?;
    assert_eq!(response["data"]["affected_count"], 3);
    fixture.ctx.cleanup().await?;
    Ok(())
}
