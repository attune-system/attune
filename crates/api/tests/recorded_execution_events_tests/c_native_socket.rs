//! Registered deferred candidates stay metadata-only; value reads use HTTP.
use super::*;
use super::{
    c_native_human_transport::{Change, HumanReader},
    c_native_transport::{Boundary, Checkpoint},
    connected_recorded_notifier::SocketHarness,
};
use attune_notifier::{service::Notification, websocket_server::dispatch_notification};
use futures::StreamExt;
use std::time::Duration;

async fn native(kind: usize, at: Checkpoint) -> Result<()> {
    match kind {
        0 => super::c_native_execution_wait::execution_wait(Some(at)).await,
        1 => {
            super::c_native_related_wait::related_wait(
                super::c_native_related_wait::Kind::Inquiry,
                true,
                Some(at),
            )
            .await
        }
        2 => {
            super::c_native_related_wait::related_wait(
                super::c_native_related_wait::Kind::Queue,
                true,
                Some(at),
            )
            .await
        }
        3 => super::c_native_cache::cache_http_case(false, 3600, Some(at)).await,
        _ => unreachable!(),
    }
}
#[tokio::test]
async fn native_registered_deferred_socket_checks_original_reader_approval_after_target_wait(
) -> Result<()> {
    for kind in 0..4 {
        native(kind, Checkpoint::Socket).await?;
    }
    Ok(())
}
#[tokio::test]
async fn native_access_socket_rechecks_frozen_identity_and_revoked_read_after_registration(
) -> Result<()> {
    for kind in 0..4 {
        for change in [Change::Freeze, Change::RevokeRead] {
            native(kind, Checkpoint::Human(change, Boundary::Socket)).await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn native_access_socket_checks_original_jwt_after_registered_target_wait() -> Result<()> {
    for kind in 0..4 {
        native(kind, Checkpoint::Human(Change::Jwt, Boundary::Socket)).await?;
    }
    Ok(())
}
#[tokio::test]
async fn native_cache_expired_source_only_allows_deferred_metadata_not_value_disclosure_on_socket(
) -> Result<()> {
    for at in [
        Checkpoint::Socket,
        Checkpoint::Human(Change::SourceDeadline, Boundary::Socket),
    ] {
        super::c_native_cache::cache_http_case(true, 30, Some(at)).await?;
    }
    Ok(())
}

pub(super) async fn exercise(
    fixture: &Fixture,
    execution: Id,
    token: &str,
    deadline: chrono::DateTime<chrono::Utc>,
    source_expiry_status: Option<StatusCode>,
    human: Option<&HumanReader>,
) -> Result<()> {
    let mut socket = SocketHarness::connect_for(fixture, token, execution).await?;
    let notification = Notification {
        notification_type: "execution_status_changed".into(),
        entity_type: "execution".into(),
        entity_id: execution,
        user_id: None,
        payload: json!({"auth_mode":"deferred"}),
        timestamp: chrono::Utc::now(),
    };
    dispatch_notification(&socket.manager, &fixture.ctx.pool, notification.clone()).await;
    let frame = tokio::time::timeout(Duration::from_secs(5), socket.socket.next())
        .await?
        .unwrap()?;
    let body: Value = serde_json::from_str(&frame.into_text()?)?;
    assert!(
        body["type"] == "notification" && body["payload"] == json!({"auth_mode":"deferred"}),
        "the real socket baseline must be a deferred metadata frame"
    );
    let candidate = socket
        .manager
        .collect_delivery_candidates(&notification)
        .pop()
        .unwrap();
    assert!(
        chrono::Utc::now() < deadline,
        "the candidate must be registered before its original deadline"
    );
    if let Some(human) = human.filter(|reader| reader.changes_identity_or_read()) {
        // Mutate after registration/candidate collection, not before a fresh
        // handshake or fanout decision. The writer must reject its old snapshot.
        human.change_after_wait(fixture, execution).await?;
        candidate.tx.send(notification)?;
        socket.expect_revoked_close().await?;
    } else {
        let mut held = fixture.ctx.pool.begin().await?;
        ExecutionRepository::find_by_id_for_update(&mut held, execution)
            .await?
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *held)
            .await?;
        candidate.tx.send(notification)?;
        tokio::time::timeout(Duration::from_secs(5),async {
            loop {
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%execution%' AND query LIKE '%FOR SHARE%')")
                    .bind(pid).fetch_one(&fixture.ctx.pool).await.unwrap();
                if blocked {break;}
                tokio::task::yield_now().await;
            }
        }).await?;
        assert!(
            chrono::Utc::now() < deadline,
            "the actual candidate target wait must precede expiry"
        );
        super::c_native_transport::expires(deadline).await?;
        held.rollback().await?;
        if let Some(expected_status) = source_expiry_status {
            // Notifications do not disclose native values. Do not invent a
            // value-WebSocket exception or deny authorized metadata because a
            // separate value-source deadline expired.
            let frame = tokio::time::timeout(Duration::from_secs(5), socket.socket.next())
                .await?
                .unwrap()?;
            let body: Value = serde_json::from_str(&frame.into_text()?)?;
            assert!(
                body["type"] == "notification"
                    && body["payload"] == json!({"auth_mode":"deferred"})
            );
            let value_read = fixture
                .ctx
                .get(
                    &format!("/api/v1/executions/{execution}?include_secret_values=true"),
                    Some(token),
                )
                .await?;
            assert_eq!(
                value_read.status(),
                expected_status,
                "the deferred hint cannot authorize the expired native value read"
            );
            let denied: Value = value_read.json().await?;
            assert_eq!(
                denied["code"],
                if expected_status == StatusCode::CONFLICT {
                    "CONFLICT"
                } else {
                    "FORBIDDEN"
                }
            );
            assert!(denied["data"].is_null());
            socket.socket.close(None).await?;
        } else {
            socket.expect_revoked_close().await?;
        }
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while socket.manager.subscription_count() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    drop(socket);
    Ok(())
}
