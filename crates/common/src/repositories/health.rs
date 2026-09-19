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
    pub action_host_available: bool,
    pub sensor_host_available: bool,
}

pub struct HealthRepository;

impl HealthRepository {
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
        let (core_active, action_host_available, sensor_host_available) = sqlx::query_as(
            "SELECT
                EXISTS(
                    SELECT 1 FROM pack p
                    JOIN pack_release pr ON pr.id = p.active_release AND pr.pack = p.id
                    WHERE p.ref = 'core'
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
            action_host_available,
            sensor_host_available,
        })
    }
}
