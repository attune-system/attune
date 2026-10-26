use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::error::Result;
use crate::models::Id;
use crate::repositories::queue_stats::{QueueStatsRepository, UpsertQueueStatsInput};

#[derive(Debug, Clone)]
pub struct AdmissionSlotAcquireOutcome {
    pub acquired: bool,
    pub current_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionEnqueueOutcome {
    Acquired,
    Enqueued,
}

#[derive(Debug, Clone)]
pub struct AdmissionSlotReleaseOutcome {
    pub action_id: Id,
    pub group_key: Option<String>,
    pub next_execution_id: Option<Id>,
}

#[derive(Debug, Clone)]
pub struct AdmissionQueuedRemovalOutcome {
    pub action_id: Id,
    pub group_key: Option<String>,
    pub next_execution_id: Option<Id>,
    pub execution_id: Id,
    pub queue_order: i64,
    pub enqueued_at: DateTime<Utc>,
    pub removed_index: usize,
}

#[derive(Debug, Clone)]
pub struct AdmissionQueueStats {
    pub action_id: Id,
    pub queue_length: usize,
    pub active_count: u32,
    pub max_concurrent: u32,
    pub oldest_enqueued_at: Option<DateTime<Utc>>,
    pub total_enqueued: u64,
    pub total_completed: u64,
}

#[derive(Debug, Clone)]
struct AdmissionState {
    id: Id,
    action_id: Id,
    group_key: Option<String>,
    max_concurrent: i32,
}

#[derive(Debug, Clone)]
struct ExecutionEntry {
    state_id: Id,
    action_id: Id,
    group_key: Option<String>,
    status: String,
    queue_order: i64,
    enqueued_at: DateTime<Utc>,
}

pub struct ExecutionAdmissionRepository;

/// The existing effective action constraint, evaluated alongside a shared pool.
pub struct StandardConcurrencyAdmission {
    pub limit: u32,
    pub method: crate::models::PolicyMethod,
    pub group_key: Option<String>,
}

impl ExecutionAdmissionRepository {
    /// `None` means this action has no shared constraint. The caller then uses
    /// its unchanged standard-only admission path. Both reservations, when
    /// required, are committed in this one short transaction.
    pub async fn admit_shared_resource(
        pool: &PgPool,
        max_queue_length: usize,
        action_id: Id,
        execution_id: Id,
        config: Option<&serde_json::Value>,
        standard: Option<StandardConcurrencyAdmission>,
    ) -> Result<Option<AdmissionEnqueueOutcome>> {
        use crate::{models::PolicyMethod, resource_concurrency::resource_digest};
        let (mut tx, retained, participant) = loop {
            let mut tx = pool.begin().await?;
            // The execution lock also excludes terminal transitions during admission.
            let status: Option<String> =
                sqlx::query_scalar("SELECT status::text FROM execution WHERE id = $1 FOR UPDATE")
                    .bind(execution_id)
                    .fetch_optional(&mut *tx)
                    .await?;
            if !status.as_deref().is_some_and(|status| {
                matches!(status, "requested" | "scheduling" | "scheduled" | "running")
            }) {
                return Err(crate::Error::validation(
                    "Policy violation: execution is not eligible for admission",
                ));
            }
            let retained = sqlx::query("SELECT policy_id, resource_group, active FROM execution_resource_admission WHERE execution_id = $1")
            .bind(execution_id).fetch_optional(&mut *tx).await?;
            let participant = if retained
                .as_ref()
                .is_some_and(|entry| entry.get::<bool, _>("active"))
            {
                None
            } else {
                let candidate: Option<Id> = sqlx::query_scalar(
                "SELECT m.policy_id FROM resource_concurrency_participant m JOIN action a ON a.ref = m.action_ref WHERE a.id = $1"
            ).bind(action_id).fetch_optional(&mut *tx).await?;
                if let Some(policy_id) = candidate {
                    sqlx::query("SELECT id FROM policy WHERE id = $1 FOR UPDATE")
                        .bind(policy_id)
                        .fetch_optional(&mut *tx)
                        .await?;
                }
                // READ COMMITTED locking joins can return a new policy with old
                // membership after waiting for an edit. Resolve membership in a
                // fresh statement, and never use a replacement policy without its
                // lock. Restart rather than accumulating locks in arbitrary order.
                let current = sqlx::query(
                "SELECT p.id, p.ref, p.resource_concurrency, m.resource_parameter FROM policy p JOIN resource_concurrency_participant m ON m.policy_id = p.id JOIN action a ON a.ref = m.action_ref WHERE a.id = $1 AND p.effective_enabled AND p.retired_at IS NULL"
            ).bind(action_id).fetch_optional(&mut *tx).await?;
                if current.as_ref().map(|row| row.get::<Id, _>("id")) != candidate {
                    tx.rollback().await?;
                    continue;
                }
                current
            };
            break (tx, retained, participant);
        };

        // Already admitted executions retain their original identity even after
        // disabling or retiring the policy. Waiting executions use current policy.
        let (policy_id, policy_ref, resource_group, limit, method) = if let Some(entry) = retained
            .as_ref()
            .filter(|entry| entry.get::<bool, _>("active"))
        {
            let policy_id: Id = entry.try_get("policy_id")?;
            let policy = sqlx::query(
                "SELECT ref, resource_concurrency FROM policy WHERE id = $1 FOR UPDATE",
            )
            .bind(policy_id)
            .fetch_one(&mut *tx)
            .await?;
            let spec: crate::resource_concurrency::ResourceConcurrency =
                serde_json::from_value(policy.try_get("resource_concurrency")?)?;
            (
                policy_id,
                policy.get::<String, _>("ref"),
                entry.get::<String, _>("resource_group"),
                spec.limit,
                spec.method,
            )
        } else if let Some(participant) = participant {
            let policy_id: Id = participant.try_get("id")?;
            let policy_ref: String = participant.try_get("ref")?;
            let path: String = participant.try_get("resource_parameter")?;
            let spec: crate::resource_concurrency::ResourceConcurrency =
                serde_json::from_value(participant.try_get("resource_concurrency")?)?;
            let group = resource_digest(config, &path).map_err(|_| crate::Error::validation(format!("Policy violation: shared resource policy {policy_ref} requires a valid scalar resource identifier at {path}")))?;
            (policy_id, policy_ref, group, spec.limit, spec.method)
        } else {
            sqlx::query(
                "DELETE FROM execution_resource_admission WHERE execution_id = $1 AND NOT active",
            )
            .bind(execution_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(None);
        };

        // Policy row locking serializes admissions across all executor instances.
        // Groups still have independent occupancy and FIFO order.
        sqlx::query("INSERT INTO execution_resource_admission (execution_id, policy_id, resource_group) VALUES ($1, $2, $3) ON CONFLICT (execution_id) DO UPDATE SET policy_id = EXCLUDED.policy_id, resource_group = EXCLUDED.resource_group WHERE NOT execution_resource_admission.active")
            .bind(execution_id).bind(policy_id).bind(&resource_group).execute(&mut *tx).await?;
        let active: bool = sqlx::query_scalar(
            "SELECT active FROM execution_resource_admission WHERE execution_id = $1",
        )
        .bind(execution_id)
        .fetch_one(&mut *tx)
        .await?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_resource_admission WHERE policy_id = $1 AND resource_group = $2 AND active")
            .bind(policy_id).bind(&resource_group).fetch_one(&mut *tx).await?;
        let front: Option<Id> = sqlx::query_scalar("SELECT execution_id FROM execution_resource_admission WHERE policy_id = $1 AND resource_group = $2 AND NOT active ORDER BY queue_order LIMIT 1")
            .bind(policy_id).bind(&resource_group).fetch_optional(&mut *tx).await?;
        if !active && (count >= i64::from(limit) || front != Some(execution_id)) {
            tracing::info!(
                policy_ref,
                resource_group,
                execution_id,
                decision = if method == PolicyMethod::Cancel {
                    "rejected"
                } else {
                    "deferred"
                },
                "Shared resource admission exhausted"
            );
            if method == PolicyMethod::Cancel {
                return Err(crate::Error::validation(format!("Policy violation: shared resource policy {policy_ref} group {resource_group} exhausted its concurrency limit {limit}")));
            }
            let waiting: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_resource_admission WHERE policy_id = $1 AND resource_group = $2 AND NOT active")
                .bind(policy_id).bind(&resource_group).fetch_one(&mut *tx).await?;
            if waiting > max_queue_length as i64 {
                return Err(crate::Error::validation(format!(
                    "Policy violation: shared resource policy {policy_ref} queue is full"
                )));
            }
            // A prior scheduling-error compensation may have restored a
            // standard slot. A shared waiter must not retain that capacity.
            if let Some(entry) = Self::find_execution_entry(&mut tx, execution_id).await? {
                if entry.status == "active" {
                    Self::lock_existing_state(&mut tx, entry.action_id, entry.group_key).await?;
                    sqlx::query("UPDATE execution_admission_entry SET status = 'queued', activated_at = NULL WHERE execution_id = $1")
                        .bind(execution_id).execute(&mut *tx).await?;
                    Self::refresh_queue_stats(&mut tx, entry.action_id).await?;
                }
            }
            tx.commit().await?;
            return Ok(Some(AdmissionEnqueueOutcome::Enqueued));
        }

        let outcome = if let Some(standard) = standard {
            let state =
                Self::lock_state(&mut tx, action_id, standard.group_key, standard.limit).await?;
            let outcome = Self::enqueue_in_state(
                &mut tx,
                &state,
                max_queue_length,
                execution_id,
                standard.method == PolicyMethod::Enqueue,
            )
            .await?;
            if outcome == AdmissionEnqueueOutcome::Enqueued
                && standard.method == PolicyMethod::Cancel
            {
                return Err(crate::Error::validation(
                    "Policy violation: concurrency limit exceeded",
                ));
            }
            Self::refresh_queue_stats(&mut tx, action_id).await?;
            outcome
        } else {
            AdmissionEnqueueOutcome::Acquired
        };
        sqlx::query("UPDATE execution_resource_admission SET active = $2 WHERE execution_id = $1")
            .bind(execution_id)
            .bind(outcome == AdmissionEnqueueOutcome::Acquired)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(
            policy_ref,
            resource_group,
            execution_id,
            ?outcome,
            "Shared resource admission evaluated"
        );
        Ok(Some(outcome))
    }

    pub async fn queued_execution_ids(
        pool: &PgPool,
        action_id: Id,
        group_key: Option<&str>,
    ) -> Result<Vec<Id>> {
        Ok(sqlx::query_scalar::<Postgres, Id>(
            r#"
            SELECT e.execution_id
            FROM execution_admission_entry e
            JOIN execution_admission_state s ON s.id = e.state_id
            WHERE s.action_id = $1
              AND s.group_key_normalized = COALESCE($2, '')
              AND e.status = 'queued'
            ORDER BY e.queue_order ASC
            "#,
        )
        .bind(action_id)
        .bind(group_key)
        .fetch_all(pool)
        .await?)
    }

    pub async fn enqueue(
        pool: &PgPool,
        max_queue_length: usize,
        action_id: Id,
        execution_id: Id,
        max_concurrent: u32,
        group_key: Option<String>,
    ) -> Result<AdmissionEnqueueOutcome> {
        let mut tx = pool.begin().await?;
        let state = Self::lock_state(&mut tx, action_id, group_key, max_concurrent).await?;
        let outcome =
            Self::enqueue_in_state(&mut tx, &state, max_queue_length, execution_id, true).await?;
        Self::refresh_queue_stats(&mut tx, action_id).await?;
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn wait_status(pool: &PgPool, execution_id: Id) -> Result<Option<bool>> {
        let row = sqlx::query_scalar::<Postgres, bool>(
            r#"
            SELECT status = 'active'
            FROM execution_admission_entry
            WHERE execution_id = $1
            "#,
        )
        .bind(execution_id)
        .fetch_optional(pool)
        .await?;

        Ok(row)
    }

    pub async fn try_acquire(
        pool: &PgPool,
        action_id: Id,
        execution_id: Id,
        max_concurrent: u32,
        group_key: Option<String>,
    ) -> Result<AdmissionSlotAcquireOutcome> {
        let mut tx = pool.begin().await?;
        let state = Self::lock_state(&mut tx, action_id, group_key, max_concurrent).await?;
        let active_count = Self::active_count(&mut tx, state.id).await? as u32;

        let outcome = match Self::find_execution_entry(&mut tx, execution_id).await? {
            Some(entry) if entry.status == "active" => AdmissionSlotAcquireOutcome {
                acquired: true,
                current_count: active_count,
            },
            Some(entry) if entry.status == "queued" && entry.state_id == state.id => {
                let promoted =
                    Self::maybe_promote_existing_queued(&mut tx, &state, execution_id).await?;
                AdmissionSlotAcquireOutcome {
                    acquired: promoted,
                    current_count: active_count,
                }
            }
            Some(_) => AdmissionSlotAcquireOutcome {
                acquired: false,
                current_count: active_count,
            },
            None => {
                if active_count < max_concurrent
                    && Self::queued_count(&mut tx, state.id).await? == 0
                {
                    let queue_order = Self::allocate_queue_order(&mut tx, state.id).await?;
                    Self::insert_entry(
                        &mut tx,
                        state.id,
                        execution_id,
                        "active",
                        queue_order,
                        Utc::now(),
                    )
                    .await?;
                    Self::increment_total_enqueued(&mut tx, state.id).await?;
                    Self::refresh_queue_stats(&mut tx, action_id).await?;
                    AdmissionSlotAcquireOutcome {
                        acquired: true,
                        current_count: active_count,
                    }
                } else {
                    AdmissionSlotAcquireOutcome {
                        acquired: false,
                        current_count: active_count,
                    }
                }
            }
        };

        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn release_active_slot(
        pool: &PgPool,
        execution_id: Id,
    ) -> Result<Option<AdmissionSlotReleaseOutcome>> {
        let mut tx = pool.begin().await?;
        let outcome = Self::release_active_slot_in_tx(&mut tx, execution_id).await?;
        tx.commit().await?;
        Ok(outcome)
    }

    /// Entry dispatch begins only after the parent leaves Scheduling for Running.
    /// Lock that lifecycle boundary while reverting and releasing both slots.
    pub async fn revert_prestart_workflow(pool: &PgPool, execution_id: Id) -> Result<bool> {
        use crate::models::ExecutionStatus;
        use crate::repositories::execution::{ExecutionRepository, UpdateExecutionInput};
        let mut tx = pool.begin().await?;
        let status: Option<String> =
            sqlx::query_scalar("SELECT status::text FROM execution WHERE id = $1 FOR UPDATE")
                .bind(execution_id)
                .fetch_optional(&mut *tx)
                .await?;
        if status.as_deref() != Some("scheduling") {
            tx.commit().await?;
            return Ok(false);
        }
        ExecutionRepository::update_if_status(
            &mut *tx,
            execution_id,
            ExecutionStatus::Scheduling,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Requested),
                ..Default::default()
            },
        )
        .await?;
        Self::release_active_slot_in_tx(&mut tx, execution_id).await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn release_active_slot_in_tx(
        tx: &mut Transaction<'_, Postgres>,
        execution_id: Id,
    ) -> Result<Option<AdmissionSlotReleaseOutcome>> {
        // Scheduling failures before worker publication may return the execution
        // to Requested. Release both constraints, including shared-only owners.
        // Running/Canceling reservations can only be released by terminal state.
        let shared: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM execution_resource_admission WHERE execution_id = $1)",
        )
        .bind(execution_id)
        .fetch_one(&mut **tx)
        .await?;
        if shared {
            let status: Option<String> =
                sqlx::query_scalar("SELECT status::text FROM execution WHERE id = $1 FOR UPDATE")
                    .bind(execution_id)
                    .fetch_optional(&mut **tx)
                    .await?;
            if matches!(status.as_deref(), Some("requested" | "scheduling")) {
                sqlx::query("UPDATE execution_resource_admission SET active = false WHERE execution_id = $1")
                    .bind(execution_id).execute(&mut **tx).await?;
                tracing::info!(
                    execution_id,
                    "Shared resource pre-dispatch reservation released"
                );
            }
        }
        let Some(entry) = Self::find_execution_entry_for_update(tx, execution_id).await? else {
            return Ok(None);
        };

        if entry.status != "active" {
            return Ok(None);
        }

        let state = Self::lock_existing_state(tx, entry.action_id, entry.group_key.clone())
            .await?
            .ok_or_else(|| {
                crate::Error::internal("missing execution_admission_state for active execution")
            })?;

        sqlx::query("DELETE FROM execution_admission_entry WHERE execution_id = $1")
            .bind(execution_id)
            .execute(&mut **tx)
            .await?;

        Self::increment_total_completed(tx, state.id).await?;

        let next_execution_id = Self::promote_next_queued(tx, &state).await?;
        Self::refresh_queue_stats(tx, state.action_id).await?;

        Ok(Some(AdmissionSlotReleaseOutcome {
            action_id: state.action_id,
            group_key: state.group_key,
            next_execution_id,
        }))
    }

    pub async fn restore_active_slot(
        pool: &PgPool,
        execution_id: Id,
        outcome: &AdmissionSlotReleaseOutcome,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        // Shared pre-dispatch releases are recovered by full transactional
        // admission. Never restore just the standard slot, or revoke a successor
        // that may already have observed the released capacity.
        let shared: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM execution_resource_admission WHERE execution_id = $1)",
        )
        .bind(execution_id)
        .fetch_one(&mut *tx)
        .await?;
        if shared {
            tx.commit().await?;
            return Ok(());
        }
        let state =
            Self::lock_existing_state(&mut tx, outcome.action_id, outcome.group_key.clone())
                .await?
                .ok_or_else(|| {
                    crate::Error::internal("missing execution_admission_state on restore")
                })?;

        if let Some(next_execution_id) = outcome.next_execution_id {
            sqlx::query(
                r#"
                UPDATE execution_admission_entry
                SET status = 'queued', activated_at = NULL
                WHERE execution_id = $1
                  AND state_id = $2
                  AND status = 'active'
                "#,
            )
            .bind(next_execution_id)
            .bind(state.id)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            r#"
            INSERT INTO execution_admission_entry (
                state_id, execution_id, status, queue_order, enqueued_at, activated_at
            ) VALUES ($1, $2, 'active', $3, NOW(), NOW())
            ON CONFLICT (execution_id) DO UPDATE
            SET state_id = EXCLUDED.state_id,
                status = 'active',
                activated_at = EXCLUDED.activated_at
            "#,
        )
        .bind(state.id)
        .bind(execution_id)
        .bind(Self::allocate_queue_order(&mut tx, state.id).await?)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            UPDATE execution_admission_state
            SET total_completed = GREATEST(total_completed - 1, 0)
            WHERE id = $1
            "#,
        )
        .bind(state.id)
        .execute(&mut *tx)
        .await?;

        Self::refresh_queue_stats(&mut tx, state.action_id).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn remove_queued_execution(
        pool: &PgPool,
        execution_id: Id,
    ) -> Result<Option<AdmissionQueuedRemovalOutcome>> {
        let mut tx = pool.begin().await?;
        let Some(entry) = Self::find_execution_entry_for_update(&mut tx, execution_id).await?
        else {
            tx.commit().await?;
            return Ok(None);
        };

        if entry.status != "queued" {
            tx.commit().await?;
            return Ok(None);
        }

        let state = Self::lock_existing_state(&mut tx, entry.action_id, entry.group_key.clone())
            .await?
            .ok_or_else(|| {
                crate::Error::internal("missing execution_admission_state for queued execution")
            })?;

        let removed_index = sqlx::query_scalar::<Postgres, i64>(
            r#"
            SELECT COUNT(*)
            FROM execution_admission_entry
            WHERE state_id = $1
              AND status = 'queued'
              AND (enqueued_at, id) < (
                    SELECT enqueued_at, id
                    FROM execution_admission_entry
                    WHERE execution_id = $2
                )
            "#,
        )
        .bind(state.id)
        .bind(execution_id)
        .fetch_one(&mut *tx)
        .await? as usize;

        sqlx::query("DELETE FROM execution_admission_entry WHERE execution_id = $1")
            .bind(execution_id)
            .execute(&mut *tx)
            .await?;

        let next_execution_id =
            if Self::active_count(&mut tx, state.id).await? < state.max_concurrent as i64 {
                Self::promote_next_queued(&mut tx, &state).await?
            } else {
                None
            };

        Self::refresh_queue_stats(&mut tx, state.action_id).await?;
        tx.commit().await?;

        Ok(Some(AdmissionQueuedRemovalOutcome {
            action_id: state.action_id,
            group_key: state.group_key,
            next_execution_id,
            execution_id,
            queue_order: entry.queue_order,
            enqueued_at: entry.enqueued_at,
            removed_index,
        }))
    }

    pub async fn restore_queued_execution(
        pool: &PgPool,
        outcome: &AdmissionQueuedRemovalOutcome,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let state =
            Self::lock_existing_state(&mut tx, outcome.action_id, outcome.group_key.clone())
                .await?
                .ok_or_else(|| {
                    crate::Error::internal("missing execution_admission_state on queued restore")
                })?;

        if let Some(next_execution_id) = outcome.next_execution_id {
            sqlx::query(
                r#"
                UPDATE execution_admission_entry
                SET status = 'queued', activated_at = NULL
                WHERE execution_id = $1
                  AND state_id = $2
                  AND status = 'active'
                "#,
            )
            .bind(next_execution_id)
            .bind(state.id)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            r#"
            INSERT INTO execution_admission_entry (
                state_id, execution_id, status, queue_order, enqueued_at, activated_at
            ) VALUES ($1, $2, 'queued', $3, $4, NULL)
            ON CONFLICT (execution_id) DO NOTHING
            "#,
        )
        .bind(state.id)
        .bind(outcome.execution_id)
        .bind(outcome.queue_order)
        .bind(outcome.enqueued_at)
        .execute(&mut *tx)
        .await?;

        Self::refresh_queue_stats(&mut tx, state.action_id).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_queue_stats(
        pool: &PgPool,
        action_id: Id,
    ) -> Result<Option<AdmissionQueueStats>> {
        let row = sqlx::query(
            r#"
            WITH state_rows AS (
                SELECT
                    COUNT(*) AS state_count,
                    COALESCE(SUM(max_concurrent), 0) AS max_concurrent,
                    COALESCE(SUM(total_enqueued), 0)::BIGINT AS total_enqueued,
                    COALESCE(SUM(total_completed), 0)::BIGINT AS total_completed
                FROM execution_admission_state
                WHERE action_id = $1
            ),
            entry_rows AS (
                SELECT
                    COUNT(*) FILTER (WHERE e.status = 'queued') AS queue_length,
                    COUNT(*) FILTER (WHERE e.status = 'active') AS active_count,
                    MIN(e.enqueued_at) FILTER (WHERE e.status = 'queued') AS oldest_enqueued_at
                FROM execution_admission_state s
                LEFT JOIN execution_admission_entry e ON e.state_id = s.id
                WHERE s.action_id = $1
            )
            SELECT
                sr.state_count,
                er.queue_length,
                er.active_count,
                sr.max_concurrent,
                er.oldest_enqueued_at,
                sr.total_enqueued,
                sr.total_completed
            FROM state_rows sr
            CROSS JOIN entry_rows er
            "#,
        )
        .bind(action_id)
        .fetch_one(pool)
        .await?;

        let state_count: i64 = row.try_get("state_count")?;
        if state_count == 0 {
            return Ok(None);
        }

        Ok(Some(AdmissionQueueStats {
            action_id,
            queue_length: row.try_get::<i64, _>("queue_length")? as usize,
            active_count: row.try_get::<i64, _>("active_count")? as u32,
            max_concurrent: row.try_get::<i64, _>("max_concurrent")? as u32,
            oldest_enqueued_at: row.try_get("oldest_enqueued_at")?,
            total_enqueued: row.try_get::<i64, _>("total_enqueued")? as u64,
            total_completed: row.try_get::<i64, _>("total_completed")? as u64,
        }))
    }

    async fn enqueue_in_state(
        tx: &mut Transaction<'_, Postgres>,
        state: &AdmissionState,
        max_queue_length: usize,
        execution_id: Id,
        allow_queue: bool,
    ) -> Result<AdmissionEnqueueOutcome> {
        if let Some(entry) = Self::find_execution_entry(tx, execution_id).await? {
            if entry.status == "active" {
                return Ok(AdmissionEnqueueOutcome::Acquired);
            }

            if entry.status == "queued" && entry.state_id == state.id {
                if Self::maybe_promote_existing_queued(tx, state, execution_id).await? {
                    return Ok(AdmissionEnqueueOutcome::Acquired);
                }
                return Ok(AdmissionEnqueueOutcome::Enqueued);
            }

            return Ok(AdmissionEnqueueOutcome::Enqueued);
        }

        let active_count = Self::active_count(tx, state.id).await?;
        let queued_count = Self::queued_count(tx, state.id).await?;

        if active_count < state.max_concurrent as i64 && queued_count == 0 {
            let queue_order = Self::allocate_queue_order(tx, state.id).await?;
            Self::insert_entry(
                tx,
                state.id,
                execution_id,
                "active",
                queue_order,
                Utc::now(),
            )
            .await?;
            Self::increment_total_enqueued(tx, state.id).await?;
            return Ok(AdmissionEnqueueOutcome::Acquired);
        }

        if !allow_queue {
            return Ok(AdmissionEnqueueOutcome::Enqueued);
        }

        if queued_count >= max_queue_length as i64 {
            return Err(anyhow::anyhow!(
                "Queue full for action {}: maximum {} entries",
                state.action_id,
                max_queue_length
            )
            .into());
        }

        let queue_order = Self::allocate_queue_order(tx, state.id).await?;
        Self::insert_entry(
            tx,
            state.id,
            execution_id,
            "queued",
            queue_order,
            Utc::now(),
        )
        .await?;
        Self::increment_total_enqueued(tx, state.id).await?;
        Ok(AdmissionEnqueueOutcome::Enqueued)
    }

    async fn maybe_promote_existing_queued(
        tx: &mut Transaction<'_, Postgres>,
        state: &AdmissionState,
        execution_id: Id,
    ) -> Result<bool> {
        let active_count = Self::active_count(tx, state.id).await?;
        if active_count >= state.max_concurrent as i64 {
            return Ok(false);
        }

        let front_execution_id = sqlx::query_scalar::<Postgres, Id>(
            r#"
            SELECT execution_id
            FROM execution_admission_entry
            WHERE state_id = $1
              AND status = 'queued'
            ORDER BY queue_order ASC
            LIMIT 1
            "#,
        )
        .bind(state.id)
        .fetch_optional(&mut **tx)
        .await?;

        if front_execution_id != Some(execution_id) {
            return Ok(false);
        }

        sqlx::query(
            r#"
            UPDATE execution_admission_entry
            SET status = 'active',
                activated_at = NOW()
            WHERE execution_id = $1
              AND state_id = $2
              AND status = 'queued'
            "#,
        )
        .bind(execution_id)
        .bind(state.id)
        .execute(&mut **tx)
        .await?;

        Ok(true)
    }

    async fn promote_next_queued(
        tx: &mut Transaction<'_, Postgres>,
        state: &AdmissionState,
    ) -> Result<Option<Id>> {
        let next_execution_id = sqlx::query_scalar::<Postgres, Id>(
            r#"
            SELECT execution_id
            FROM execution_admission_entry
            WHERE state_id = $1
              AND status = 'queued'
            ORDER BY queue_order ASC
            LIMIT 1
            "#,
        )
        .bind(state.id)
        .fetch_optional(&mut **tx)
        .await?;

        if let Some(next_execution_id) = next_execution_id {
            // Shared waiters must acquire both constraints through the scheduler,
            // never independently through standard completion-time promotion.
            let shared_waiter: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM execution_resource_admission WHERE execution_id = $1 AND NOT active)")
                .bind(next_execution_id).fetch_one(&mut **tx).await?;
            if shared_waiter {
                return Ok(None);
            }
            sqlx::query(
                r#"
                UPDATE execution_admission_entry
                SET status = 'active',
                    activated_at = NOW()
                WHERE execution_id = $1
                  AND state_id = $2
                  AND status = 'queued'
                "#,
            )
            .bind(next_execution_id)
            .bind(state.id)
            .execute(&mut **tx)
            .await?;
        }

        Ok(next_execution_id)
    }

    async fn lock_state(
        tx: &mut Transaction<'_, Postgres>,
        action_id: Id,
        group_key: Option<String>,
        max_concurrent: u32,
    ) -> Result<AdmissionState> {
        sqlx::query(
            r#"
            INSERT INTO execution_admission_state (action_id, group_key, max_concurrent)
            VALUES ($1, $2, $3)
            ON CONFLICT (action_id, group_key_normalized)
            DO UPDATE SET max_concurrent = EXCLUDED.max_concurrent
            "#,
        )
        .bind(action_id)
        .bind(group_key.clone())
        .bind(max_concurrent as i32)
        .execute(&mut **tx)
        .await?;

        let state = sqlx::query(
            r#"
            SELECT id, action_id, group_key, max_concurrent
            FROM execution_admission_state
            WHERE action_id = $1
              AND group_key_normalized = COALESCE($2, '')
            FOR UPDATE
            "#,
        )
        .bind(action_id)
        .bind(group_key)
        .fetch_one(&mut **tx)
        .await?;

        Ok(AdmissionState {
            id: state.try_get("id")?,
            action_id: state.try_get("action_id")?,
            group_key: state.try_get("group_key")?,
            max_concurrent: state.try_get("max_concurrent")?,
        })
    }

    async fn lock_existing_state(
        tx: &mut Transaction<'_, Postgres>,
        action_id: Id,
        group_key: Option<String>,
    ) -> Result<Option<AdmissionState>> {
        let row = sqlx::query(
            r#"
            SELECT id, action_id, group_key, max_concurrent
            FROM execution_admission_state
            WHERE action_id = $1
              AND group_key_normalized = COALESCE($2, '')
            FOR UPDATE
            "#,
        )
        .bind(action_id)
        .bind(group_key)
        .fetch_optional(&mut **tx)
        .await?;

        Ok(row.map(|state| AdmissionState {
            id: state.try_get("id").expect("state.id"),
            action_id: state.try_get("action_id").expect("state.action_id"),
            group_key: state.try_get("group_key").expect("state.group_key"),
            max_concurrent: state
                .try_get("max_concurrent")
                .expect("state.max_concurrent"),
        }))
    }

    async fn find_execution_entry(
        tx: &mut Transaction<'_, Postgres>,
        execution_id: Id,
    ) -> Result<Option<ExecutionEntry>> {
        let row = sqlx::query(
            r#"
            SELECT
                e.state_id,
                s.action_id,
                s.group_key,
                e.execution_id,
                e.status,
                e.queue_order,
                e.enqueued_at
            FROM execution_admission_entry e
            JOIN execution_admission_state s ON s.id = e.state_id
            WHERE e.execution_id = $1
            "#,
        )
        .bind(execution_id)
        .fetch_optional(&mut **tx)
        .await?;

        Ok(row.map(|entry| ExecutionEntry {
            state_id: entry.try_get("state_id").expect("entry.state_id"),
            action_id: entry.try_get("action_id").expect("entry.action_id"),
            group_key: entry.try_get("group_key").expect("entry.group_key"),
            status: entry.try_get("status").expect("entry.status"),
            queue_order: entry.try_get("queue_order").expect("entry.queue_order"),
            enqueued_at: entry.try_get("enqueued_at").expect("entry.enqueued_at"),
        }))
    }

    async fn find_execution_entry_for_update(
        tx: &mut Transaction<'_, Postgres>,
        execution_id: Id,
    ) -> Result<Option<ExecutionEntry>> {
        let row = sqlx::query(
            r#"
            SELECT
                e.state_id,
                s.action_id,
                s.group_key,
                e.execution_id,
                e.status,
                e.queue_order,
                e.enqueued_at
            FROM execution_admission_entry e
            JOIN execution_admission_state s ON s.id = e.state_id
            WHERE e.execution_id = $1
            FOR UPDATE OF e, s
            "#,
        )
        .bind(execution_id)
        .fetch_optional(&mut **tx)
        .await?;

        Ok(row.map(|entry| ExecutionEntry {
            state_id: entry.try_get("state_id").expect("entry.state_id"),
            action_id: entry.try_get("action_id").expect("entry.action_id"),
            group_key: entry.try_get("group_key").expect("entry.group_key"),
            status: entry.try_get("status").expect("entry.status"),
            queue_order: entry.try_get("queue_order").expect("entry.queue_order"),
            enqueued_at: entry.try_get("enqueued_at").expect("entry.enqueued_at"),
        }))
    }

    async fn active_count(tx: &mut Transaction<'_, Postgres>, state_id: Id) -> Result<i64> {
        Ok(sqlx::query_scalar::<Postgres, i64>(
            r#"
            SELECT COUNT(*)
            FROM execution_admission_entry
            WHERE state_id = $1
              AND status = 'active'
            "#,
        )
        .bind(state_id)
        .fetch_one(&mut **tx)
        .await?)
    }

    async fn queued_count(tx: &mut Transaction<'_, Postgres>, state_id: Id) -> Result<i64> {
        Ok(sqlx::query_scalar::<Postgres, i64>(
            r#"
            SELECT COUNT(*)
            FROM execution_admission_entry
            WHERE state_id = $1
              AND status = 'queued'
            "#,
        )
        .bind(state_id)
        .fetch_one(&mut **tx)
        .await?)
    }

    async fn insert_entry(
        tx: &mut Transaction<'_, Postgres>,
        state_id: Id,
        execution_id: Id,
        status: &str,
        queue_order: i64,
        enqueued_at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO execution_admission_entry (
                state_id, execution_id, status, queue_order, enqueued_at, activated_at
            ) VALUES (
                $1, $2, $3, $4, $5,
                CASE WHEN $3 = 'active' THEN NOW() ELSE NULL END
            )
            "#,
        )
        .bind(state_id)
        .bind(execution_id)
        .bind(status)
        .bind(queue_order)
        .bind(enqueued_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn allocate_queue_order(tx: &mut Transaction<'_, Postgres>, state_id: Id) -> Result<i64> {
        let queue_order = sqlx::query_scalar::<Postgres, i64>(
            r#"
            UPDATE execution_admission_state
            SET next_queue_order = next_queue_order + 1
            WHERE id = $1
            RETURNING next_queue_order - 1
            "#,
        )
        .bind(state_id)
        .fetch_one(&mut **tx)
        .await?;

        Ok(queue_order)
    }

    async fn increment_total_enqueued(
        tx: &mut Transaction<'_, Postgres>,
        state_id: Id,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE execution_admission_state
            SET total_enqueued = total_enqueued + 1
            WHERE id = $1
            "#,
        )
        .bind(state_id)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    async fn increment_total_completed(
        tx: &mut Transaction<'_, Postgres>,
        state_id: Id,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE execution_admission_state
            SET total_completed = total_completed + 1
            WHERE id = $1
            "#,
        )
        .bind(state_id)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    async fn refresh_queue_stats(tx: &mut Transaction<'_, Postgres>, action_id: Id) -> Result<()> {
        let Some(stats) = Self::get_queue_stats_from_tx(tx, action_id).await? else {
            QueueStatsRepository::delete(&mut **tx, action_id).await?;
            return Ok(());
        };

        QueueStatsRepository::upsert(
            &mut **tx,
            UpsertQueueStatsInput {
                action_id,
                queue_length: stats.queue_length as i32,
                active_count: stats.active_count as i32,
                max_concurrent: stats.max_concurrent as i32,
                oldest_enqueued_at: stats.oldest_enqueued_at,
                total_enqueued: stats.total_enqueued as i64,
                total_completed: stats.total_completed as i64,
            },
        )
        .await?;

        Ok(())
    }

    async fn get_queue_stats_from_tx(
        tx: &mut Transaction<'_, Postgres>,
        action_id: Id,
    ) -> Result<Option<AdmissionQueueStats>> {
        let row = sqlx::query(
            r#"
            WITH state_rows AS (
                SELECT
                    COUNT(*) AS state_count,
                    COALESCE(SUM(max_concurrent), 0) AS max_concurrent,
                    COALESCE(SUM(total_enqueued), 0)::BIGINT AS total_enqueued,
                    COALESCE(SUM(total_completed), 0)::BIGINT AS total_completed
                FROM execution_admission_state
                WHERE action_id = $1
            ),
            entry_rows AS (
                SELECT
                    COUNT(*) FILTER (WHERE e.status = 'queued') AS queue_length,
                    COUNT(*) FILTER (WHERE e.status = 'active') AS active_count,
                    MIN(e.enqueued_at) FILTER (WHERE e.status = 'queued') AS oldest_enqueued_at
                FROM execution_admission_state s
                LEFT JOIN execution_admission_entry e ON e.state_id = s.id
                WHERE s.action_id = $1
            )
            SELECT
                sr.state_count,
                er.queue_length,
                er.active_count,
                sr.max_concurrent,
                er.oldest_enqueued_at,
                sr.total_enqueued,
                sr.total_completed
            FROM state_rows sr
            CROSS JOIN entry_rows er
            "#,
        )
        .bind(action_id)
        .fetch_one(&mut **tx)
        .await?;

        let state_count: i64 = row.try_get("state_count")?;
        if state_count == 0 {
            return Ok(None);
        }

        Ok(Some(AdmissionQueueStats {
            action_id,
            queue_length: row.try_get::<i64, _>("queue_length")? as usize,
            active_count: row.try_get::<i64, _>("active_count")? as u32,
            max_concurrent: row.try_get::<i64, _>("max_concurrent")? as u32,
            oldest_enqueued_at: row.try_get("oldest_enqueued_at")?,
            total_enqueued: row.try_get::<i64, _>("total_enqueued")? as u64,
            total_completed: row.try_get::<i64, _>("total_completed")? as u64,
        }))
    }
}
