use super::*;
use attune_common::crypto::{decrypt_json, encrypt_json};
use axum::{body::Body, http::Request};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;
use tokio::task::JoinSet;

const ORIGIN: &str = "https://git.example.com";
const OTHER_ORIGIN: &str = "https://archives.example.com";
const OLD: &str = "Bearer race-old-source-token";
const ROTATED: &str = "Bearer race-rotated-source-token";
const OTHER: &str = "Bearer race-other-origin-token";
const INDEX: &str = "Bearer race-index-token";

#[tokio::test]
async fn source_header_rotation_before_preservation_keeps_rotated_secret() -> Result<()> {
    ordered_updates(false, false).await
}

#[tokio::test]
async fn source_header_preservation_before_rotation_keeps_rotated_secret() -> Result<()> {
    ordered_updates(true, false).await
}

#[tokio::test]
async fn source_header_removal_rejects_stale_redacted_preservation() -> Result<()> {
    ordered_updates(false, true).await
}

fn update_request(ctx: &TestContext, id: i64, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/pack-indices/{id}"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", ctx.token().unwrap()))
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

// Wait for actual PostgreSQL lock waiters, not an elapsed-time guess. Match the
// exact lock held by this test's connection, excluding unrelated fixture work.
async fn wait_for_waiters(ctx: &TestContext, holder: i32, expected: usize) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let waiters: Vec<i32> = sqlx::query_scalar(
                "SELECT DISTINCT w.pid FROM pg_locks w JOIN pg_locks h
                 ON w.locktype = h.locktype AND w.database = h.database
                 AND w.classid = h.classid AND w.objid = h.objid
                 AND w.objsubid = h.objsubid
                 WHERE h.pid = $1 AND h.locktype = 'advisory' AND h.granted
                 AND NOT w.granted",
            )
            .bind(holder)
            .fetch_all(&ctx.pool)
            .await?;
            if waiters.len() == expected {
                return Ok::<_, sqlx::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    Ok(())
}

fn assert_no_secrets(text: &str) {
    for secret in [OLD, ROTATED, OTHER, INDEX] {
        assert!(
            !text.contains(secret),
            "response or audit disclosed a fixture credential"
        );
    }
}

async fn ordered_updates(preserve_first: bool, remove: bool) -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let key = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let index = PackRegistryIndexRepository::create(
        &ctx.pool,
        CreatePackRegistryIndexInput {
            name: Some("Before concurrent edit".into()),
            url: "https://registry.example.com/index.json".into(),
            position: None,
            enabled: true,
            headers: encrypt_json(&json!({"Authorization": INDEX}), key)?,
            source_headers: json!({
                ORIGIN: encrypt_json(&json!({"Authorization": OLD}), key)?,
                OTHER_ORIGIN: encrypt_json(&json!({"Authorization": OTHER}), key)?,
            }),
        },
    )
    .await?;
    let preserve = json!({
        "name": "Concurrent preserving edit",
        "source_headers": {
            ORIGIN: {"authorization": "[REDACTED]"},
            OTHER_ORIGIN: {"Authorization": "[REDACTED]"},
        }
    });
    let change = if remove {
        json!({"source_headers": {OTHER_ORIGIN: {"Authorization": "[REDACTED]"}}})
    } else {
        json!({"source_headers": {
            ORIGIN: {"Authorization": ROTATED},
            OTHER_ORIGIN: {"Authorization": "[REDACTED]"},
        }})
    };
    let requests = if preserve_first {
        [preserve, change]
    } else {
        [change, preserve]
    };
    let mut gate = ctx.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind("pack_registry_index_mutation")
        .execute(&mut *gate)
        .await?;
    let holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *gate)
        .await?;
    let mut jobs = JoinSet::new();
    let queued = async {
        for (position, body) in requests.into_iter().enumerate() {
            let app = ctx.app.clone();
            let request = update_request(&ctx, index.id, body);
            jobs.spawn(async move {
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                (position, status, body)
            });
            wait_for_waiters(&ctx, holder, position + 1).await?;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    // Releasing the gate admits the already-observed waiters in queue order.
    // Always release it and join/abort requests, even if synchronization fails.
    gate.rollback().await?;
    let outcome = AssertUnwindSafe(async {
        queued?;
        while let Some(joined) = tokio::time::timeout(Duration::from_secs(30), jobs.join_next()).await? {
            let (position, status, body) = joined?;
            assert_no_secrets(std::str::from_utf8(&body)?);
            let expected = if remove && position == 1 { StatusCode::BAD_REQUEST } else { StatusCode::OK };
            assert_eq!(status, expected);
            let body: serde_json::Value = serde_json::from_slice(&body)?;
            if status == StatusCode::OK {
                for headers in body["data"]["source_headers"].as_object().unwrap().values() {
                    assert!(headers.as_object().unwrap().values().all(|value| value == "[REDACTED]"));
                }
            }
        }
        let stored = PackRegistryIndexRepository::find_by_id(&ctx.pool, index.id).await?.unwrap();
        let encrypted = &stored.source_headers;
        assert!(encrypted[OTHER_ORIGIN].is_string());
        assert_eq!(decrypt_json(&encrypted[OTHER_ORIGIN], key)?["Authorization"], OTHER);
        assert_no_secrets(&encrypted.to_string());
        assert_eq!(decrypt_json(&stored.headers, key)?["Authorization"], INDEX);
        if remove {
            assert!(encrypted.get(ORIGIN).is_none(), "stale placeholder resurrected a removed origin");
        } else {
            assert!(encrypted[ORIGIN].is_string());
            let headers = decrypt_json(&encrypted[ORIGIN], key)?;
            let value = headers.get("Authorization").or_else(|| headers.get("authorization")).unwrap();
            assert_eq!(value, ROTATED, "preservation restored the pre-rotation secret");
            assert_eq!(stored.name.as_deref(), Some("Concurrent preserving edit"));
        }
        ctx.flush_audit().await?;
        let details: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT details FROM audit_event WHERE resource_type = 'pack_registry_index' AND resource_id = $1",
        ).bind(index.id).fetch_all(&ctx.pool).await?;
        assert_eq!(details.len(), if remove { 1 } else { 2 });
        for detail in details { assert_no_secrets(&detail.to_string()); }
        Ok::<_, Box<dyn std::error::Error>>(())
    }).catch_unwind().await;
    jobs.shutdown().await;
    ctx.cleanup().await?;
    match outcome {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
