//! Recorded execution credentials never expand through mutable identity IAM.

use attune_common::{
    auth::Claims,
    rbac::Action,
    repositories::{
        runtime_authority::{
            ApprovalEffectFence, RuntimeAuthorityRepository, RuntimeCredentialBinding,
        },
        runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
        schema_type::SchemaTypeRepository,
    },
};
use sqlx::{pool::PoolConnection, Connection, PgPool, Postgres, Transaction};
use std::time::{Duration, Instant};
use tokio::time::{timeout, timeout_at, Instant as TokioInstant};

use crate::service::Notification;

const VALIDATION_TIMEOUT: Duration = Duration::from_secs(10);
const STATEMENT_TIMEOUT: &str = "5000ms";
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Own the socket before BEGIN, including while BEGIN itself is pending. SQLx's
/// ordinary pool return pings the connection and can wait on an abandoned query.
/// Cancellation instead detaches and drops the socket synchronously. No cleanup
/// task owns it after this guard is dropped.
struct ValidationConnection(Option<PoolConnection<Postgres>>);

impl ValidationConnection {
    async fn acquire(pool: &PgPool, deadline: TokioInstant) -> Option<Self> {
        let mut connection = timeout_at(deadline, pool.acquire()).await.ok()?.ok()?;
        connection.close_on_drop();
        Some(Self(Some(connection)))
    }

    async fn release(mut self) -> bool {
        let mut connection = self.0.take().unwrap();
        // This future takes custody synchronously. Dropping it closes the raw
        // socket, rather than starting SQLx's async return-to-pool fallback.
        let returning = connection.return_to_pool();
        drop(connection); // Empty wrapper, no socket remains in its Drop task.
        timeout(CLEANUP_TIMEOUT, returning).await.is_ok()
    }
}

impl Drop for ValidationConnection {
    fn drop(&mut self) {
        if let Some(connection) = self.0.take() {
            drop(connection.detach());
        }
    }
}

/// One holder record per outer validation, not one per repository reentry.
struct ValidationHolder {
    span: tracing::Span,
    backend: Option<i32>,
    stage: &'static str,
    requested: Option<Instant>,
    acquired: Option<Instant>,
    confirmed: bool,
}

impl ValidationHolder {
    fn new() -> Self {
        Self {
            span: tracing::Span::current(),
            backend: None,
            stage: "begin",
            requested: None,
            acquired: None,
            confirmed: false,
        }
    }

    async fn select(&mut self, tx: &mut Transaction<'_, Postgres>) -> Option<()> {
        self.stage = "configure";
        let (backend, _): (i32, String) =
            sqlx::query_as("SELECT pg_backend_pid(), set_config('statement_timeout', $1, true)")
                .bind(STATEMENT_TIMEOUT)
                .fetch_one(&mut **tx)
                .await
                .ok()?;
        self.backend = Some(backend);
        self.stage = "graph_admission";
        self.requested = Some(Instant::now());
        tracing::debug!(
            backend,
            mode = "shared",
            event = "requested",
            "Notifier graph holder"
        );
        // Establish the first acquisition explicitly. Repository calls may
        // reenter this transaction's lock, but do not create another holder.
        SchemaTypeRepository::lock_selection(tx).await.ok()?;
        self.acquired = Some(Instant::now());
        tracing::debug!(
            backend,
            mode = "shared",
            event = "acquired",
            wait_us = self.requested.unwrap().elapsed().as_micros() as u64,
            "Notifier graph holder"
        );
        self.stage = "source_validation";
        Some(())
    }

    fn confirm_rollback(&mut self) {
        self.confirmed = true;
        if self.requested.is_some() {
            tracing::debug!(
                backend = self.backend,
                stage = self.stage,
                mode = "shared",
                event = "ended",
                outcome = "rollback_confirmed",
                holder_us = self.acquired.map(|at| at.elapsed().as_micros() as u64),
                "Notifier graph holder"
            );
        }
    }
}

impl Drop for ValidationHolder {
    fn drop(&mut self) {
        let _entered = self.span.enter();
        if self.requested.is_some() && !self.confirmed {
            tracing::debug!(
                backend = self.backend,
                stage = self.stage,
                mode = "shared",
                event = "socket_disposed",
                outcome = "unknown",
                holder_observed_us = self.acquired.map(|at| at.elapsed().as_micros() as u64),
                "Notifier graph holder"
            );
        }
    }
}

#[derive(Clone)]
pub(crate) struct RecordedConnection {
    pub(crate) binding: RuntimeCredentialBinding,
    selected_refs: Vec<String>,
}

impl RecordedConnection {
    pub(crate) fn selected_refs(&self) -> &[String] {
        &self.selected_refs
    }
    pub(crate) fn from_claims(claims: &Claims) -> anyhow::Result<Option<Self>> {
        claims
            .admitted_execution_credential()?
            .map(|binding| {
                Ok(Self {
                    binding,
                    selected_refs: claims.admitted_permission_set_refs()?,
                })
            })
            .transpose()
    }

    /// No live permit is cached. Database failure closes the connection.
    pub(crate) async fn is_live(&self, pool: &PgPool) -> bool {
        self.source_fence(pool).await.is_some()
    }

    #[tracing::instrument(
        name = "notifier.recorded_authority",
        level = "debug",
        skip_all,
        fields(operation = "source", validation_id = %uuid::Uuid::new_v4())
    )]
    pub(crate) async fn source_fence(&self, pool: &PgPool) -> Option<ApprovalEffectFence> {
        let deadline = TokioInstant::now() + VALIDATION_TIMEOUT;
        let mut holder = ValidationHolder::new();
        let mut owner = ValidationConnection::acquire(pool, deadline).await?;
        let mut tx = timeout_at(deadline, owner.0.as_mut().unwrap().begin())
            .await
            .ok()?
            .ok()?;
        tracing::debug!(stage = "transaction_started");
        let fence = timeout_at(deadline, async {
            holder.select(&mut tx).await?;
            let authority =
                RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &self.binding)
                    .await
                    .ok()?;
            tracing::debug!(stage = "source_validated");
            if authority.authority().selected_refs != self.selected_refs {
                return None;
            }
            holder.stage = "final_clock";
            authority.ensure_fresh_on(&mut tx).await.ok()?;
            tracing::debug!(stage = "final_clock_checked");
            Some(authority.effect_fence().clone())
        })
        .await
        .ok()
        .flatten();
        let fence = rollback_validation(tx, fence, &mut holder).await;
        if holder.confirmed && owner.release().await {
            fence
        } else {
            None
        }
    }

    pub(crate) async fn permits(&self, pool: &PgPool, notification: &Notification) -> bool {
        self.notification_fence(pool, notification).await.is_some()
    }

    #[tracing::instrument(
        name = "notifier.recorded_authority",
        level = "debug",
        skip_all,
        fields(operation = "notification", validation_id = %uuid::Uuid::new_v4())
    )]
    pub(crate) async fn notification_fence(
        &self,
        pool: &PgPool,
        notification: &Notification,
    ) -> Option<ApprovalEffectFence> {
        // Event identity requires its partition key. Unknown notification kinds
        // and ambiguous event IDs cannot establish a structural owner.
        let target = match notification.entity_type.as_str() {
            "event" => {
                let created = notification
                    .payload
                    .get("created")
                    .and_then(|value| value.as_str())
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())?;
                RuntimeTarget::Event {
                    id: notification.entity_id,
                    created: created.with_timezone(&chrono::Utc),
                }
            }
            "execution" => RuntimeTarget::Execution(notification.entity_id),
            "enforcement" => RuntimeTarget::Enforcement(notification.entity_id),
            "artifact" => RuntimeTarget::Artifact(notification.entity_id),
            "work_queue" => {
                RuntimeTarget::Component(ComponentTarget::Queue, notification.entity_id)
            }
            "work_queue_item" => RuntimeTarget::QueueItem(notification.entity_id),
            "rule_lifecycle" => {
                RuntimeTarget::Component(ComponentTarget::Rule, notification.entity_id)
            }
            _ => return None,
        };
        let deadline = TokioInstant::now() + VALIDATION_TIMEOUT;
        let mut holder = ValidationHolder::new();
        let mut owner = ValidationConnection::acquire(pool, deadline).await?;
        let mut tx = timeout_at(deadline, owner.0.as_mut().unwrap().begin())
            .await
            .ok()?
            .ok()?;
        tracing::debug!(stage = "transaction_started");
        let fence = timeout_at(deadline, async {
            holder.select(&mut tx).await?;
            let Ok(authority) =
                RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &self.binding).await
            else {
                return None;
            };
            tracing::debug!(stage = "source_validated");
            if authority.authority().selected_refs != self.selected_refs {
                return None;
            }
            holder.stage = "target_validation";
            let mut expected_links = None;
            if notification.entity_type == "artifact" {
                let Ok(links) = attune_common::repositories::artifact::ArtifactRepository::linked_execution_ids_bounded(
                    &mut *tx, notification.entity_id, 257,
                ).await else { return None; };
                if links.len() > 256 {
                    return None;
                }
                for execution in links.iter().copied() {
                    if execution == self.binding.execution_id {
                        continue;
                    }
                    let Ok(Some(target)) = RuntimeTargetRepository::resolve(
                        &mut tx,
                        RuntimeTarget::Execution(execution),
                        self.binding.sponsor_identity_id,
                    )
                    .await
                    else {
                        return None;
                    };
                    if !authority.allows_target(&target, Action::Read) {
                        return None;
                    }
                }
                expected_links = Some(links);
            }
            // Match worker completion's execution-before-artifact order. No
            // execution locks may be acquired after the artifact lock.
            let Ok(Some(target)) =
                RuntimeTargetRepository::resolve(&mut tx, target, self.binding.sponsor_identity_id)
                    .await
            else {
                return None;
            };
            if !authority.allows_target(&target, Action::Read) {
                return None;
            }
            if let Some(expected) = expected_links {
                let Ok(current) = attune_common::repositories::artifact::ArtifactRepository::linked_execution_ids_bounded(
                    &mut *tx, notification.entity_id, 257,
                ).await else { return None; };
                if current != expected {
                    return None;
                }
            }
            tracing::debug!(stage = "target_validated");
            holder.stage = "final_clock";
            authority.ensure_fresh_on(&mut tx).await.ok()?;
            tracing::debug!(stage = "final_clock_checked");
            Some(authority.effect_fence().clone())
        })
        .await.ok().flatten();
        let fence = rollback_validation(tx, fence, &mut holder).await;
        if holder.confirmed && owner.release().await {
            fence
        } else {
            None
        }
    }
}

/// Validation must not publish incidental bindings. Await release of its locks
/// before returning a fence. A dropped query may leave SQLSTATE 57014 unread.
/// Drain it before sending ROLLBACK, then wait for ROLLBACK's ReadyForQuery.
async fn rollback_validation(
    mut tx: Transaction<'_, Postgres>,
    fence: Option<ApprovalEffectFence>,
    holder: &mut ValidationHolder,
) -> Option<ApprovalEffectFence> {
    holder.stage = "rollback";
    let cleanup = timeout(CLEANUP_TIMEOUT, async {
        loop {
            match Connection::flush(&mut *tx).await {
                Ok(()) => break,
                Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("57014") => {
                    tracing::debug!(stage = "rollback", event = "cancellation_drained");
                }
                Err(error) => return Err(error),
            }
        }
        tx.rollback().await
    })
    .await;
    if !matches!(cleanup, Ok(Ok(()))) {
        tracing::warn!(
            stage = "rollback",
            outcome = "unknown",
            "Recorded validation rollback failed"
        );
        return None;
    }
    holder.confirm_rollback();
    tracing::debug!(
        stage = "rollback",
        outcome = "confirmed",
        allowed = fence.is_some()
    );
    fence
}

#[cfg(test)]
#[path = "recorded_authority_transaction_tests.rs"]
mod transaction_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::auth::{jwt::generate_admitted_execution_token, validate_token, JwtConfig};

    #[test]
    fn recorded_claims_cannot_downgrade_to_identity_iam() {
        attune_common::auth::install_crypto_provider();
        let config = JwtConfig {
            secret: "notifier-recorded-test".into(),
            access_token_expiration: 60,
            refresh_token_expiration: 60,
        };
        let binding = RuntimeCredentialBinding {
            execution_id: 11,
            admission_id: 12,
            sponsor_identity_id: 13,
            action_ref: "fixture.action".into(),
            attempt: uuid::Uuid::new_v4(),
        };
        let token =
            generate_admitted_execution_token(&binding, &["standard".into()], &config, Some(60))
                .unwrap();
        let claims = validate_token(&token, &config).unwrap();
        let recorded = RecordedConnection::from_claims(&claims).unwrap().unwrap();
        assert_eq!(recorded.binding.execution_id, 11);
        let mut malformed = claims.clone();
        malformed.metadata.as_mut().unwrap()["authority_source"] =
            serde_json::json!("human_delegation");
        assert!(RecordedConnection::from_claims(&malformed).is_err());
        malformed = claims.clone();
        malformed.metadata.as_mut().unwrap()["permission_set_refs"] =
            serde_json::json!(["standard", 7]);
        assert!(RecordedConnection::from_claims(&malformed).is_err());
        malformed = claims;
        malformed.sub = "999".into();
        assert!(RecordedConnection::from_claims(&malformed).is_err());
    }
}
