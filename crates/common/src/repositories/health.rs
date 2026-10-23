//! Database-backed API health state.

use sqlx::{Executor, Postgres};

use crate::Result;

const HOST_HEARTBEAT_MAX_AGE_SECONDS: i32 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformHealth {
    pub compatibility_epoch: i32,
    pub catalog_revision: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentHealth {
    pub core_active: bool,
    pub core_admitted: bool,
    pub action_host_available: bool,
    pub sensor_host_available: bool,
}

pub struct HealthRepository;

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct AdmittedCoreRelease {
    pub release_id: i64,
    pub version: String,
    pub digest: String,
    pub policy_revision: i64,
}

impl HealthRepository {
    /// A release file/frozen snapshot alone is not runtime admission evidence.
    pub async fn admitted_core(pool: &sqlx::PgPool) -> Result<Option<AdmittedCoreRelease>> {
        let mut tx = pool.begin().await?;
        super::schema_type::SchemaTypeRepository::lock_selection(&mut tx).await?;
        let core: Option<AdmittedCoreRelease> = sqlx::query_as("SELECT pr.id AS release_id, pr.version, pr.digest, r.id AS policy_revision FROM pack p JOIN pack_release pr ON pr.id = p.active_release AND pr.pack = p.id JOIN pack_runtime_policy_current c ON c.pack = p.id JOIN pack_runtime_policy_revision r ON r.id = c.revision AND r.pack = p.id AND r.release = pr.id JOIN pack_runtime_identity runtime_owner ON runtime_owner.pack = p.id JOIN identity sponsor ON sponsor.id = runtime_owner.identity AND NOT sponsor.frozen WHERE p.ref = 'core' AND r.revoked_at IS NULL AND r.policy->'owner'->>'sponsor_identity_id' = runtime_owner.identity::text")
            .fetch_optional(&mut *tx).await?;
        if let Some(core) = &core {
            // Presence of a revision alone does not prove fixed approvals or
            // retained publisher dependencies remain live.
            super::runtime_authority::RuntimeAuthorityRepository::require_deployment_policy(
                &mut tx,
                core.policy_revision,
            )
            .await?;
        }
        tx.rollback().await?;
        Ok(core)
    }
    pub async fn platform<'e, E>(executor: E) -> Result<PlatformHealth>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let (compatibility_epoch, catalog_revision) = sqlx::query_as(
            "SELECT compatibility_epoch, revision FROM platform_catalog_state WHERE singleton",
        )
        .fetch_one(executor)
        .await?;

        Ok(PlatformHealth {
            compatibility_epoch,
            catalog_revision,
        })
    }

    pub async fn content<'e, E>(executor: E) -> Result<ContentHealth>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let (core_active, core_admitted, action_host_available, sensor_host_available) =
            sqlx::query_as(
                "SELECT
                EXISTS(
                    SELECT 1 FROM pack p
                    JOIN pack_release pr ON pr.id = p.active_release AND pr.pack = p.id
                    WHERE p.ref = 'core'
                ),
                EXISTS(
                    SELECT 1 FROM pack p
                    JOIN pack_release pr ON pr.id = p.active_release AND pr.pack = p.id
                    JOIN pack_runtime_policy_current c ON c.pack = p.id
                    JOIN pack_runtime_policy_revision r ON r.id = c.revision
                        AND r.pack = p.id AND r.release = pr.id
                    JOIN pack_runtime_identity runtime_owner ON runtime_owner.pack = p.id
                    JOIN identity sponsor ON sponsor.id = runtime_owner.identity AND NOT sponsor.frozen
                    WHERE p.ref = 'core' AND r.revoked_at IS NULL
                        AND r.policy->'owner'->>'sponsor_identity_id' = runtime_owner.identity::text
                ),
                EXISTS(
                    SELECT 1 FROM worker
                    WHERE worker_role = 'action'
                      AND status IN ('active', 'busy')
                      AND NOT cordoned
                      AND last_heartbeat > clock_timestamp() - make_interval(secs => $1)
                      AND jsonb_typeof(capabilities->'runtimes') = 'array'
                      AND jsonb_array_length(capabilities->'runtimes') > 0
                ),
                EXISTS(
                    SELECT 1 FROM worker
                    WHERE worker_role = 'sensor'
                      AND status IN ('active', 'busy')
                      AND NOT cordoned
                      AND last_heartbeat > clock_timestamp() - make_interval(secs => $1)
                      AND jsonb_typeof(capabilities->'runtimes') = 'array'
                      AND capabilities->'runtimes' ? 'native'
                )",
            )
            .bind(HOST_HEARTBEAT_MAX_AGE_SECONDS)
            .fetch_one(executor)
            .await?;

        Ok(ContentHealth {
            core_active,
            core_admitted,
            action_host_available,
            sensor_host_available,
        })
    }
}
