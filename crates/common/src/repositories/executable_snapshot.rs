use crate::models::{ExecutionExecutableSnapshot, Id};
use crate::{Error, Result};
use serde_json::Value;
use sqlx::{Executor, FromRow, Postgres};

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
                SELECT a.pack, a.workflow_def, p.active_release
                FROM action a
                JOIN pack p ON p.id = a.pack
                WHERE a.id = $1 OR a.ref = $2
            ), action_snapshots AS (
                SELECT
                    a.id,
                    a.ref,
                    jsonb_build_object(
                        'action', to_jsonb(a),
                        'runtime', to_jsonb(r),
                        'runtime_versions', COALESCE((
                            SELECT jsonb_agg(to_jsonb(rv) ORDER BY rv.version, rv.id)
                            FROM runtime_version rv
                            WHERE rv.runtime = r.id
                        ), '[]'::jsonb),
                        'workflow_definition', to_jsonb(wd)
                    ) AS snapshot
                FROM selected s
                JOIN action a ON a.pack = s.pack
                LEFT JOIN runtime r ON r.id = a.runtime
                LEFT JOIN workflow_definition wd ON wd.id = a.workflow_def
            )
            SELECT
                pr.id AS release_id,
                pr.digest AS release_digest,
                pr.content_path,
                current.snapshot AS executable,
                CASE WHEN s.workflow_def IS NULL THEN '{}'::jsonb
                     ELSE COALESCE(jsonb_object_agg(all_actions.ref, all_actions.snapshot), '{}'::jsonb)
                END AS pack_executables
            FROM selected s
            JOIN pack_release pr ON pr.id = s.active_release
            JOIN action_snapshots current ON current.id = $1 OR current.ref = $2
            LEFT JOIN action_snapshots all_actions ON TRUE
            GROUP BY pr.id, pr.digest, pr.content_path, current.snapshot, s.workflow_def
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
