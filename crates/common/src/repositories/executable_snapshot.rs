use crate::models::{ExecutionExecutableSnapshot, Id};
use crate::{Error, Result};
use serde_json::Value;
use sqlx::{Executor, FromRow, PgConnection, Postgres};
use std::collections::BTreeMap;

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
    /// Load the complete immutable release declaration set for candidate policy
    /// compilation. Disabled declarations remain present: lifecycle checks for
    /// new invocation are separate from the contents of the immutable release.
    /// This method does not prove foreign reference visibility to its caller.
    pub async fn resolve_release_actions(
        connection: &mut PgConnection,
        release_id: Id,
    ) -> Result<BTreeMap<String, ExecutionExecutableSnapshot>> {
        super::schema_type::SchemaTypeRepository::lock_selection(&mut *connection).await?;
        let release: Option<(Id, String, String)> = sqlx::query_as(
            "SELECT pack, digest, content_path FROM pack_release WHERE id = $1 FOR SHARE",
        )
        .bind(release_id)
        .fetch_optional(&mut *connection)
        .await?;
        let (pack, digest, content_path) =
            release.ok_or_else(|| crate::Error::validation("Immutable release is unavailable"))?;
        let release = crate::models::PackReleasePin {
            id: release_id,
            digest,
            content_path,
        };
        let rows: Vec<(String, Value)> = sqlx::query_as(
            "SELECT component_ref, snapshot FROM pack_release_executable \
             WHERE release = $1 AND component_kind = 'action' ORDER BY component_ref FOR SHARE",
        )
        .bind(release_id)
        .fetch_all(connection)
        .await?;
        let mut actions = BTreeMap::new();
        for (reference, body) in rows {
            let executable: crate::models::ActionExecutableSnapshot = serde_json::from_value(body)?;
            if executable.action.r#ref != reference || executable.action.pack != pack {
                return Err(crate::Error::validation(
                    "Immutable executable ownership mismatch",
                ));
            }
            if actions
                .insert(
                    reference,
                    crate::models::ReleasedActionExecutableSnapshot {
                        release: release.clone(),
                        executable,
                    },
                )
                .is_some()
            {
                return Err(crate::Error::validation(
                    "Duplicate immutable executable reference",
                ));
            }
        }
        Ok(actions
            .iter()
            .map(|(reference, action)| {
                (
                    reference.clone(),
                    ExecutionExecutableSnapshot {
                        release: action.release.clone(),
                        executable: action.executable.clone(),
                        pack_executables: if action.executable.workflow_definition.is_some() {
                            actions.clone()
                        } else {
                            BTreeMap::new()
                        },
                    },
                )
            })
            .collect())
    }

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
                'sensor', to_jsonb(s) - 'config',
                'runtime', to_jsonb(r),
                'runtime_versions', COALESCE((
                    SELECT jsonb_agg(to_jsonb(rv) ORDER BY rv.version, rv.id)
                    FROM runtime_version rv
                    WHERE rv.runtime = r.id AND rv.retired_at IS NULL
                ), '[]'::jsonb),
                'triggers', COALESCE((
                    SELECT jsonb_agg(to_jsonb(t) - 'webhook_key' - 'webhook_config' ORDER BY t.ref, t.id)
                    FROM trigger t WHERE t.sensor = s.id AND t.retired_at IS NULL
                ), '[]'::jsonb),
                'cache_permission_set_refs', COALESCE(s.config->'cache_permission_set_refs', '[]'::jsonb),
                'cache_grants', COALESCE((
                    SELECT jsonb_agg(grant_value ORDER BY p.ref, ordinal)
                    FROM permission_set p
                    CROSS JOIN LATERAL jsonb_array_elements(p.grants) WITH ORDINALITY AS g(grant_value, ordinal)
                    WHERE p.ref IN (SELECT btrim(value) FROM jsonb_array_elements_text(COALESCE(s.config->'cache_permission_set_refs', '[]'::jsonb)) AS refs(value))
                        AND p.retired_at IS NULL AND grant_value->>'resource' = 'caches'
                ), '[]'::jsonb),
                'signing_permission_set_refs', COALESCE(s.config->'signing_permission_set_refs', '[]'::jsonb),
                'signing_grants', COALESCE((
                    SELECT jsonb_agg(jsonb_set(grant_value, '{actions}', '["use"]'::jsonb) ORDER BY p.ref, ordinal)
                    FROM permission_set p
                    CROSS JOIN LATERAL jsonb_array_elements(p.grants) WITH ORDINALITY AS g(grant_value, ordinal)
                    WHERE p.ref IN (SELECT btrim(value) FROM jsonb_array_elements_text(COALESCE(s.config->'signing_permission_set_refs', '[]'::jsonb)) AS refs(value))
                        AND p.retired_at IS NULL AND grant_value->>'resource' = 'keys' AND grant_value->'actions' ? 'use'
                ), '[]'::jsonb),
                'input_permission_set_refs', COALESCE(s.config->'input_permission_set_refs', '[]'::jsonb),
                'input_grants', COALESCE((
                    SELECT jsonb_agg(jsonb_set(grant_value, '{actions}', selected.actions) ORDER BY p.ref, ordinal)
                    FROM permission_set p
                    CROSS JOIN LATERAL jsonb_array_elements(p.grants) WITH ORDINALITY AS g(grant_value, ordinal)
                    CROSS JOIN LATERAL (SELECT jsonb_agg(action) AS actions FROM jsonb_array_elements(grant_value->'actions') action
                        WHERE action IN ('"read"'::jsonb, '"decrypt"'::jsonb)) selected
                    WHERE p.ref IN (SELECT btrim(value) FROM jsonb_array_elements_text(COALESCE(s.config->'input_permission_set_refs', '[]'::jsonb)) AS refs(value))
                        AND p.retired_at IS NULL AND grant_value->>'resource' IN ('keys', 'packs') AND selected.actions IS NOT NULL
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
                UNION ALL
                SELECT
                    a.id,
                    a.ref,
                    jsonb_build_object(
                        'action', to_jsonb(a),
                        'runtime', to_jsonb(r),
                        'runtime_versions', COALESCE((
                            SELECT jsonb_agg(to_jsonb(rv) ORDER BY rv.version, rv.id)
                            FROM runtime_version rv
                            WHERE rv.runtime = r.id AND rv.retired_at IS NULL
                        ), '[]'::jsonb),
                        'workflow_definition', to_jsonb(wd)
                    ) AS snapshot,
                    pr.id AS release_id,
                    pr.digest AS release_digest,
                    pr.content_path
                FROM selected s
                JOIN action a ON a.pack = s.pack
                JOIN pack p ON p.id = a.pack
                JOIN pack_release pr ON pr.id = p.active_release
                LEFT JOIN runtime r ON r.id = a.runtime AND r.retired_at IS NULL
                LEFT JOIN workflow_definition wd ON wd.id = a.workflow_def AND wd.retired_at IS NULL
                WHERE a.is_adhoc
                  AND a.retired_at IS NULL
                  AND a.effective_enabled
                  AND (a.runtime IS NULL OR r.id IS NOT NULL)
                  AND (a.workflow_def IS NULL OR wd.id IS NOT NULL)
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
