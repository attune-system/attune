use crate::models::{ExecutionExecutableSnapshot, Id};
use crate::{Error, Result};
use serde_json::Value;
use sqlx::{Executor, FromRow, PgConnection, Postgres};

#[derive(FromRow)]
struct SnapshotRow {
    release_id: Id,
    release_digest: String,
    content_path: String,
    executable: Value,
    pack_executables: Value,
}

pub struct ExecutableSnapshotRepository;

impl ExecutableSnapshotRepository {
    pub async fn capture_present(
        connection: &mut PgConnection,
        release_id: Id,
        action_ids: &[Id],
        sensor_ids: &[Id],
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO pack_release_executable
                (release, component_kind, component_id, component_ref, snapshot)
            SELECT $1, 'action', a.id, a.ref, jsonb_build_object(
                'action', to_jsonb(a),
                'runtime', to_jsonb(r),
                'runtime_versions', COALESCE((
                    SELECT jsonb_agg(to_jsonb(rv) ORDER BY rv.version, rv.id)
                    FROM runtime_version rv
                    WHERE rv.runtime = r.id AND rv.retired_at IS NULL
                ), '[]'::jsonb),
                'workflow_definition', to_jsonb(wd)
            )
            FROM action a
            LEFT JOIN runtime r ON r.id = a.runtime AND r.retired_at IS NULL
            LEFT JOIN workflow_definition wd ON wd.id = a.workflow_def AND wd.retired_at IS NULL
            WHERE a.id = ANY($2::BIGINT[]) AND a.retired_at IS NULL
            ON CONFLICT (release, component_kind, component_id) DO NOTHING
            "#,
        )
        .bind(release_id)
        .bind(action_ids)
        .execute(&mut *connection)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO pack_release_executable
                (release, component_kind, component_id, component_ref, snapshot)
            SELECT $1, 'sensor', s.id, s.ref, jsonb_build_object(
                'sensor', to_jsonb(s),
                'runtime', to_jsonb(r),
                'runtime_versions', COALESCE((
                    SELECT jsonb_agg(to_jsonb(rv) ORDER BY rv.version, rv.id)
                    FROM runtime_version rv
                    WHERE rv.runtime = r.id AND rv.retired_at IS NULL
                ), '[]'::jsonb)
            )
            FROM sensor s
            JOIN runtime r ON r.id = s.runtime AND r.retired_at IS NULL
            WHERE s.id = ANY($2::BIGINT[]) AND s.retired_at IS NULL
            ON CONFLICT (release, component_kind, component_id) DO NOTHING
            "#,
        )
        .bind(release_id)
        .bind(sensor_ids)
        .execute(connection)
        .await?;
        Ok(())
    }

    pub async fn resolve_for_action<'e, E>(
        executor: E,
        action_id: Id,
    ) -> Result<ExecutionExecutableSnapshot>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        Self::resolve(executor, Some(action_id), None).await
    }

    pub async fn resolve_for_action_ref<'e, E>(
        executor: E,
        action_ref: &str,
    ) -> Result<ExecutionExecutableSnapshot>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        Self::resolve(executor, None, Some(action_ref)).await
    }

    async fn resolve<'e, E>(
        executor: E,
        action_id: Option<Id>,
        action_ref: Option<&str>,
    ) -> Result<ExecutionExecutableSnapshot>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let row = sqlx::query_as::<_, SnapshotRow>(
            r#"
            WITH selected AS (
                SELECT a.pack, a.workflow_def, a.managed_release
                FROM action a
                WHERE (a.id = $1 OR a.ref = $2)
                  AND a.retired_at IS NULL
                  AND a.effective_enabled
                  AND (a.runtime IS NULL OR EXISTS (
                      SELECT 1 FROM runtime active_runtime
                      WHERE active_runtime.id = a.runtime AND active_runtime.retired_at IS NULL
                  ))
                  AND (a.workflow_def IS NULL OR EXISTS (
                      SELECT 1 FROM workflow_definition active_workflow
                      WHERE active_workflow.id = a.workflow_def AND active_workflow.retired_at IS NULL
                  ))
            ), action_snapshots AS (
                SELECT
                    a.id,
                    a.ref,
                    executable.snapshot,
                    pr.id AS release_id,
                    pr.digest AS release_digest,
                    pr.content_path
                FROM selected s
                JOIN action a ON a.pack = s.pack
                LEFT JOIN runtime r ON r.id = a.runtime AND r.retired_at IS NULL
                LEFT JOIN workflow_definition wd ON wd.id = a.workflow_def AND wd.retired_at IS NULL
                JOIN pack_release_executable executable
                  ON executable.release = a.managed_release
                 AND executable.component_kind = 'action'
                 AND executable.component_id = a.id
                JOIN pack_release pr ON pr.id = executable.release
                WHERE a.retired_at IS NULL
                  AND a.effective_enabled
                  AND (a.runtime IS NULL OR r.id IS NOT NULL)
            )
            SELECT
                current.release_id,
                current.release_digest,
                current.content_path,
                current.snapshot AS executable,
                CASE WHEN s.workflow_def IS NULL THEN '{}'::jsonb
                     ELSE COALESCE(jsonb_object_agg(
                         all_actions.ref,
                         jsonb_build_object(
                             'release', jsonb_build_object(
                                 'id', all_actions.release_id,
                                 'digest', all_actions.release_digest,
                                 'content_path', all_actions.content_path
                             ),
                             'executable', all_actions.snapshot
                         )
                     ), '{}'::jsonb)
                END AS pack_executables
            FROM selected s
            JOIN action_snapshots current ON current.id = $1 OR current.ref = $2
            LEFT JOIN action_snapshots all_actions ON TRUE
            GROUP BY current.release_id, current.release_digest, current.content_path,
                     current.snapshot, s.workflow_def
            "#,
        )
        .bind(action_id)
        .bind(action_ref)
        .fetch_optional(executor)
        .await?
        .ok_or_else(|| {
            Error::validation(format!(
                "Action {} does not belong to a pack with an active immutable release",
                action_ref.map(str::to_string).unwrap_or_else(|| action_id.unwrap_or_default().to_string())
            ))
        })?;

        Ok(ExecutionExecutableSnapshot {
            release: crate::models::PackReleasePin {
                id: row.release_id,
                digest: row.release_digest,
                content_path: row.content_path,
            },
            executable: serde_json::from_value(row.executable)?,
            pack_executables: serde_json::from_value(row.pack_executables)?,
        })
    }
}
