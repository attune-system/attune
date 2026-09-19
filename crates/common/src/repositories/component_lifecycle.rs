//! Lifecycle reconciliation for pack-managed component projections.

use chrono::{DateTime, Utc};
use sqlx::{Executor, PgConnection, Postgres};

use crate::models::AbsentMetadataPolicy;
use crate::platform_catalog::ManagedComponentKind;
use crate::{Error, Result};

#[derive(Debug, Default, Clone)]
pub struct PackProjectionIds {
    pub runtimes: Vec<i64>,
    pub runtime_versions: Vec<i64>,
    pub permission_sets: Vec<i64>,
    pub triggers: Vec<i64>,
    pub actions: Vec<i64>,
    pub sensors: Vec<i64>,
    pub rules: Vec<i64>,
    pub policies: Vec<i64>,
    pub work_queues: Vec<i64>,
    pub workflows: Vec<i64>,
    pub dashboards: Vec<i64>,
    pub caches: Vec<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RetiredPackComponent {
    pub kind: String,
    pub id: i64,
    pub component_ref: Option<String>,
    pub managed_release: Option<i64>,
    pub retired_at: DateTime<Utc>,
}

pub struct ComponentLifecycleRepository;

impl ComponentLifecycleRepository {
    pub async fn list_retired_by_pack<'e, E>(
        executor: E,
        pack_id: i64,
    ) -> Result<Vec<RetiredPackComponent>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        Ok(sqlx::query_as(
            "SELECT 'runtime' AS kind, id, ref AS component_ref, managed_release, retired_at
             FROM runtime WHERE pack = $1 AND management_origin = 'pack' AND retired_at IS NOT NULL
             UNION ALL
             SELECT 'permission_set', id, ref, managed_release, retired_at
             FROM permission_set WHERE pack = $1 AND management_origin = 'pack' AND retired_at IS NOT NULL
             UNION ALL
             SELECT 'trigger', id, ref, managed_release, retired_at
             FROM trigger WHERE pack = $1 AND management_origin = 'pack' AND retired_at IS NOT NULL
             UNION ALL
             SELECT 'action', id, ref, managed_release, retired_at
             FROM action WHERE pack = $1 AND management_origin = 'pack' AND retired_at IS NOT NULL
             UNION ALL
             SELECT 'sensor', id, ref, managed_release, retired_at
             FROM sensor WHERE pack = $1 AND management_origin = 'pack' AND retired_at IS NOT NULL
             UNION ALL
             SELECT 'rule', id, ref, managed_release, retired_at
             FROM rule WHERE pack = $1 AND management_origin = 'pack' AND retired_at IS NOT NULL
             UNION ALL
             SELECT 'policy', id, ref, managed_release, retired_at
             FROM policy WHERE pack = $1 AND management_origin = 'pack' AND retired_at IS NOT NULL
             UNION ALL
             SELECT 'work_queue', id, ref, managed_release, retired_at
             FROM work_queue WHERE pack = $1 AND management_origin = 'pack' AND retired_at IS NOT NULL
             UNION ALL
             SELECT 'workflow', id, ref, managed_release, retired_at
             FROM workflow_definition WHERE pack = $1 AND management_origin = 'pack' AND retired_at IS NOT NULL
             UNION ALL
             SELECT 'dashboard', id, ref, managed_release, retired_at
             FROM dashboard WHERE pack = $1 AND management_origin = 'pack' AND retired_at IS NOT NULL
             UNION ALL
             SELECT 'cache', id, definition_ref, managed_release, retired_at
             FROM cache_namespace WHERE managing_pack = $1 AND management_origin = 'pack' AND retired_at IS NOT NULL
             ORDER BY retired_at DESC, kind, component_ref NULLS LAST, id",
        )
        .bind(pack_id)
        .fetch_all(executor)
        .await?)
    }

    pub async fn reactivate_pack_component(
        connection: &mut PgConnection,
        kind: ManagedComponentKind,
        id: i64,
    ) -> Result<()> {
        let assignment = match kind {
            ManagedComponentKind::Trigger
            | ManagedComponentKind::Action
            | ManagedComponentKind::Sensor
            | ManagedComponentKind::Rule
            | ManagedComponentKind::Policy
            | ManagedComponentKind::WorkQueue
            | ManagedComponentKind::Dashboard => {
                "retired_at = NULL, omission_disabled = FALSE, updated = NOW()"
            }
            _ => "retired_at = NULL, updated = NOW()",
        };
        let result = sqlx::query(&format!(
            "UPDATE {} SET {assignment} WHERE id = $1 AND management_origin = 'pack'",
            kind.table(),
        ))
        .bind(id)
        .execute(connection)
        .await?;
        if result.rows_affected() != 1 {
            return Err(Error::invalid_state(format!(
                "Pack-managed {} {id} was not found",
                kind.table()
            )));
        }
        Ok(())
    }

    pub async fn set_declared_enabled(
        connection: &mut PgConnection,
        kind: ManagedComponentKind,
        id: i64,
        enabled: bool,
    ) -> Result<()> {
        match kind {
            ManagedComponentKind::Trigger
            | ManagedComponentKind::Action
            | ManagedComponentKind::Sensor
            | ManagedComponentKind::Rule
            | ManagedComponentKind::Policy
            | ManagedComponentKind::WorkQueue
            | ManagedComponentKind::Dashboard => {}
            _ => {
                return Err(Error::validation(format!(
                    "{} does not have declarative enabled state",
                    kind.table()
                )))
            }
        }

        let result = sqlx::query(&format!(
            "UPDATE {} SET enabled = $2, updated = NOW() WHERE id = $1 AND management_origin = 'pack'",
            kind.table()
        ))
        .bind(id)
        .bind(enabled)
        .execute(connection)
        .await?;
        if result.rows_affected() != 1 {
            return Err(Error::invalid_state(format!(
                "Pack-managed {} {id} was not found",
                kind.table()
            )));
        }
        Ok(())
    }

    /// Apply the default removal policy for callers that do not choose one.
    pub async fn reconcile_omissions(
        connection: &mut PgConnection,
        pack_id: i64,
        ids: &PackProjectionIds,
    ) -> Result<u64> {
        Self::reconcile_omissions_with_policy(
            connection,
            pack_id,
            ids,
            AbsentMetadataPolicy::Remove,
        )
        .await
    }

    /// Reactivate incoming IDs and apply the selected policy to omitted
    /// pack-owned projections. Platform and ad-hoc rows cannot match.
    pub async fn reconcile_omissions_with_policy(
        connection: &mut PgConnection,
        pack_id: i64,
        ids: &PackProjectionIds,
        policy: AbsentMetadataPolicy,
    ) -> Result<u64> {
        let mut retired = 0;
        for (table, owner, keep, toggleable) in [
            ("runtime", "pack", ids.runtimes.as_slice(), false),
            (
                "permission_set",
                "pack",
                ids.permission_sets.as_slice(),
                false,
            ),
            ("trigger", "pack", ids.triggers.as_slice(), true),
            ("action", "pack", ids.actions.as_slice(), true),
            ("sensor", "pack", ids.sensors.as_slice(), true),
            ("rule", "pack", ids.rules.as_slice(), true),
            ("policy", "pack", ids.policies.as_slice(), true),
            ("work_queue", "pack", ids.work_queues.as_slice(), true),
            (
                "workflow_definition",
                "pack",
                ids.workflows.as_slice(),
                false,
            ),
            ("dashboard", "pack", ids.dashboards.as_slice(), true),
            (
                "cache_namespace",
                "managing_pack",
                ids.caches.as_slice(),
                false,
            ),
        ] {
            let omitted_update = match (policy, toggleable) {
                (AbsentMetadataPolicy::Remove, true) => Some(
                    "retired_at = COALESCE(retired_at, NOW()), omission_disabled = FALSE, managed_release = NULL",
                ),
                (AbsentMetadataPolicy::Remove, false) => Some(
                    "retired_at = COALESCE(retired_at, NOW()), managed_release = NULL",
                ),
                (AbsentMetadataPolicy::Disable, true) => {
                    Some("retired_at = NULL, omission_disabled = TRUE")
                }
                (AbsentMetadataPolicy::Disable, false) | (AbsentMetadataPolicy::Retain, _) => None,
            };
            let present_update = if toggleable {
                "retired_at = NULL, omission_disabled = FALSE"
            } else {
                "retired_at = NULL"
            };
            let result = sqlx::query(&format!(
                "UPDATE {table} SET {present_update}, updated = NOW() \
                 WHERE {owner} = $1 AND management_origin = 'pack' AND id = ANY($2::BIGINT[])"
            ))
            .bind(pack_id)
            .bind(keep)
            .execute(&mut *connection)
            .await?;
            if result.rows_affected() != keep.len() as u64 {
                return Err(Error::invalid_state(format!(
                    "Pack projection keep set for {table} contains an ID not owned by pack {pack_id}"
                )));
            }
            if let Some(omitted_update) = omitted_update {
                let result = sqlx::query(&format!(
                    "UPDATE {table} SET {omitted_update}, updated = NOW() \
                     WHERE {owner} = $1 AND management_origin = 'pack' AND NOT (id = ANY($2::BIGINT[]))"
                ))
                .bind(pack_id)
                .bind(keep)
                .execute(&mut *connection)
                .await?;
                if policy == AbsentMetadataPolicy::Remove {
                    retired += result.rows_affected();
                }
            }
        }

        // Runtime versions inherit pack ownership from their parent. Keep-set
        // membership and an active incoming parent are both required.
        let result = sqlx::query(
            "UPDATE runtime_version rv SET retired_at = NULL, updated = NOW() \
             FROM runtime r WHERE rv.runtime = r.id AND r.pack = $1 AND r.management_origin = 'pack' \
             AND rv.id = ANY($2::BIGINT[]) AND r.id = ANY($3::BIGINT[])",
        )
        .bind(pack_id)
        .bind(&ids.runtime_versions)
        .bind(&ids.runtimes)
        .execute(&mut *connection)
        .await?;
        if result.rows_affected() != ids.runtime_versions.len() as u64 {
            return Err(Error::invalid_state(format!(
                "Pack projection keep set for runtime_version contains an ID not owned by pack {pack_id}"
            )));
        }
        if policy == AbsentMetadataPolicy::Remove {
            retired += sqlx::query(
                "UPDATE runtime_version rv SET retired_at = COALESCE(rv.retired_at, NOW()), managed_release = NULL, updated = NOW() \
                 FROM runtime r WHERE rv.runtime = r.id AND r.pack = $1 AND r.management_origin = 'pack' \
                 AND NOT (rv.id = ANY($2::BIGINT[]) AND r.id = ANY($3::BIGINT[]))",
            )
            .bind(pack_id)
            .bind(&ids.runtime_versions)
            .bind(&ids.runtimes)
            .execute(&mut *connection)
            .await?
            .rows_affected();
        }

        Self::validate_keep_sets(connection, pack_id, ids).await?;
        Ok(retired)
    }

    pub async fn stamp_release(
        connection: &mut PgConnection,
        pack_id: i64,
        release_id: i64,
        ids: &PackProjectionIds,
    ) -> Result<()> {
        for (table, owner, keep) in [
            ("runtime", "pack", ids.runtimes.as_slice()),
            ("permission_set", "pack", ids.permission_sets.as_slice()),
            ("trigger", "pack", ids.triggers.as_slice()),
            ("action", "pack", ids.actions.as_slice()),
            ("sensor", "pack", ids.sensors.as_slice()),
            ("rule", "pack", ids.rules.as_slice()),
            ("policy", "pack", ids.policies.as_slice()),
            ("work_queue", "pack", ids.work_queues.as_slice()),
            ("workflow_definition", "pack", ids.workflows.as_slice()),
            ("dashboard", "pack", ids.dashboards.as_slice()),
            ("cache_namespace", "managing_pack", ids.caches.as_slice()),
        ] {
            sqlx::query(&format!(
                "UPDATE {table} SET managed_release = $3 WHERE {owner} = $1 AND management_origin = 'pack' \
                 AND retired_at IS NULL AND id = ANY($2::BIGINT[])"
            ))
            .bind(pack_id)
            .bind(keep)
            .bind(release_id)
            .execute(&mut *connection)
            .await?;
        }
        sqlx::query(
            "UPDATE runtime_version rv SET managed_release = $3 \
             FROM runtime r WHERE rv.runtime = r.id AND r.pack = $1 \
             AND r.management_origin = 'pack' AND rv.retired_at IS NULL \
             AND rv.id = ANY($2::BIGINT[])",
        )
        .bind(pack_id)
        .bind(&ids.runtime_versions)
        .bind(release_id)
        .execute(connection)
        .await?;
        Ok(())
    }

    async fn validate_keep_sets(
        connection: &mut PgConnection,
        pack_id: i64,
        ids: &PackProjectionIds,
    ) -> Result<()> {
        for (table, owner, keep) in [
            ("runtime", "pack", ids.runtimes.as_slice()),
            ("permission_set", "pack", ids.permission_sets.as_slice()),
            ("trigger", "pack", ids.triggers.as_slice()),
            ("action", "pack", ids.actions.as_slice()),
            ("sensor", "pack", ids.sensors.as_slice()),
            ("rule", "pack", ids.rules.as_slice()),
            ("policy", "pack", ids.policies.as_slice()),
            ("work_queue", "pack", ids.work_queues.as_slice()),
            ("workflow_definition", "pack", ids.workflows.as_slice()),
            ("dashboard", "pack", ids.dashboards.as_slice()),
            ("cache_namespace", "managing_pack", ids.caches.as_slice()),
        ] {
            let count: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM {table} WHERE {owner} = $1 AND management_origin = 'pack' AND retired_at IS NULL AND id = ANY($2::BIGINT[])"
            ))
            .bind(pack_id)
            .bind(keep)
            .fetch_one(&mut *connection)
            .await?;
            if count != keep.len() as i64 {
                return Err(Error::invalid_state(format!(
                    "Pack projection keep set for {table} contains an ID not owned by pack {pack_id}"
                )));
            }
        }
        Ok(())
    }
}
