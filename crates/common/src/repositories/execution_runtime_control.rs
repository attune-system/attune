//! Exact, bounded tree cancellation under an already validated source fence.
use super::{
    execution::ExecutionRepository,
    runtime_authority::{
        recorded_collections::{execution_cancel_ids, execution_metadata_columns},
        ValidatedRuntimeAuthority,
    },
};
use crate::{
    models::{Execution, ExecutionStatus, Id},
    Error, Result,
};
use sqlx::PgConnection;

const MAX_TREE: usize = 256;

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct Node {
    id: Id,
    parent: Option<Id>,
    is_cycle: bool,
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct Workflow {
    id: Id,
    execution: Id,
    status: ExecutionStatus,
}

fn denied() -> Error {
    // Do not expose private descendant IDs, counts, status or missing grants.
    Error::PermissionDenied("Cancellation target unavailable or operation denied".into())
}

async fn tree(conn: &mut PgConnection, root: Id) -> Result<Vec<Node>> {
    // Walk only this root's indexed parent edges, not a global candidate scan.
    // The extra node detects overflow; the path also stops cycles and deep trees.
    let mut nodes: Vec<Node> = sqlx::query_as(
        "WITH RECURSIVE tree(id,parent,path,is_cycle) AS (\
          SELECT id,parent,ARRAY[id],FALSE FROM execution WHERE id = $1 \
          UNION ALL \
          SELECT e.id,e.parent,t.path || e.id,e.id = ANY(t.path) \
          FROM tree t JOIN execution e ON e.parent = t.id \
          WHERE NOT t.is_cycle AND cardinality(t.path) <= 256\
         ) SELECT id,parent,is_cycle FROM tree LIMIT 257",
    )
    .bind(root)
    .fetch_all(conn)
    .await?;
    if nodes.is_empty() || nodes.len() > MAX_TREE || nodes.iter().any(|node| node.is_cycle) {
        return Err(denied());
    }
    nodes.sort_unstable_by_key(|node| node.id);
    Ok(nodes)
}

async fn workflows(conn: &mut PgConnection, ids: &[Id], lock: bool) -> Result<Vec<Workflow>> {
    let suffix = if lock { " FOR UPDATE" } else { "" };
    Ok(sqlx::query_as(&format!(
        "SELECT id,execution,status FROM workflow_execution WHERE execution = ANY($1) ORDER BY id{suffix}"
    )).bind(ids).fetch_all(conn).await?)
}

fn next_status(status: ExecutionStatus) -> Option<ExecutionStatus> {
    match status {
        ExecutionStatus::Requested
        | ExecutionStatus::Scheduling
        | ExecutionStatus::Scheduled
        | ExecutionStatus::Cancelled => Some(ExecutionStatus::Cancelled),
        ExecutionStatus::Running | ExecutionStatus::Canceling => Some(ExecutionStatus::Canceling),
        _ => None,
    }
}

impl ExecutionRepository {
    /// Caller retains A's graph/source authority through final commit. All target
    /// checks use that same opaque authority; no resolver/graph/source reentry.
    /// Workflow-state creators must retain a parent execution SHARE/status fence
    /// through INSERT. The independently retained workflow lineage is not an FK.
    pub async fn cancel_recorded_tree(
        conn: &mut PgConnection,
        root: Id,
        authority: &ValidatedRuntimeAuthority,
    ) -> Result<Vec<(Execution, ExecutionStatus)>> {
        if root <= 0 {
            return Err(denied());
        }
        let initial = tree(conn, root).await?;
        let ids: Vec<Id> = initial.iter().map(|node| node.id).collect();
        // Require independent Cancel rights for EVERY actual descendant, including
        // terminal nodes. A root grant, sponsor equality or contribution is not one.
        if execution_cancel_ids(conn, authority, &ids).await? != ids {
            return Err(denied());
        }
        let initial_workflows = workflows(conn, &ids, false).await?;
        let workflow_ids: Vec<Id> = initial_workflows
            .iter()
            .map(|workflow| workflow.id)
            .collect();
        // Exactly the scheduler's advisory locks, in stable order, in one call.
        sqlx::query("WITH locks AS MATERIALIZED (SELECT id FROM unnest($1::bigint[]) AS w(id) ORDER BY id) SELECT pg_advisory_xact_lock(id) FROM locks ORDER BY id")
            .bind(&workflow_ids).execute(&mut *conn).await?;
        let locked_workflows = workflows(conn, &ids, true).await?;
        if initial_workflows
            .iter()
            .map(|w| (w.id, w.execution))
            .collect::<Vec<_>>()
            != locked_workflows
                .iter()
                .map(|w| (w.id, w.execution))
                .collect::<Vec<_>>()
        {
            return Err(Error::invalid_state("Cancellation workflow changed; retry"));
        }
        let rows: Vec<Execution> = sqlx::query_as(&format!(
            "SELECT {} FROM execution e WHERE e.id = ANY($1) ORDER BY e.id FOR UPDATE",
            execution_metadata_columns(),
        ))
        .bind(&ids)
        .fetch_all(&mut *conn)
        .await?;
        authority.ensure_fresh_on(conn).await?;
        if rows.iter().map(|row| row.id).collect::<Vec<_>>() != ids {
            return Err(denied());
        }
        if execution_cancel_ids(conn, authority, &ids).await? != ids {
            return Err(denied());
        }
        // The actual workflow advisory/row locks prevent scheduler spawning; the
        // execution UPDATE locks fence API parent-source admission. Compare exact
        // edges, not only a stale ID count. Workflow-state creation must also
        // retain a parent execution SHARE lock through INSERT: its lineage ID
        // intentionally has no FK. See the creation-side integration prerequisite.
        if tree(conn, root).await? != initial {
            return Err(Error::invalid_state("Cancellation tree changed; retry"));
        }
        if workflows(conn, &ids, false).await? != locked_workflows {
            return Err(Error::invalid_state("Cancellation workflow changed; retry"));
        }
        let mut changes = Vec::new();
        for execution in rows {
            if let Some(status) = next_status(execution.status) {
                changes.push((execution, status));
            } else if execution.id == root {
                return Err(Error::invalid_state("Execution is not cancellable"));
            }
        }
        let changed_ids: Vec<Id> = changes.iter().map(|(execution, _)| execution.id).collect();
        let active_workflows: Vec<Id> = locked_workflows
            .iter()
            .filter(|w| changed_ids.binary_search(&w.execution).is_ok())
            .map(|w| w.id)
            .collect();
        if locked_workflows.iter().any(|w| {
            active_workflows.binary_search(&w.id).is_ok() && next_status(w.status).is_none()
        }) {
            return Err(Error::invalid_state(
                "Workflow completed before cancellation; retry",
            ));
        }
        authority.ensure_fresh_on(conn).await?;
        // Preserve cancel_with_prerequisites semantics, batched by the exact
        // protected workflow IDs. Never return inquiry/wait/variable payloads.
        sqlx::query("UPDATE inquiry SET status = 'cancelled',updated = NOW() WHERE workflow_execution = ANY($1) AND status = 'pending'")
            .bind(&active_workflows).execute(&mut *conn).await?;
        sqlx::query("UPDATE workflow_task_wait SET state = 'cancelled',result = '{\"reason\":\"workflow cancelled\"}'::jsonb,resolved_at = NOW(),active_work_queue_item = NULL WHERE workflow_execution = ANY($1) AND state = 'waiting'")
            .bind(&active_workflows).execute(&mut *conn).await?;
        sqlx::query("UPDATE workflow_execution SET status = 'cancelled',error_message = 'Cancelled by recorded execution control',current_tasks = ARRAY[]::text[],updated = NOW() WHERE id = ANY($1)")
            .bind(&active_workflows).execute(&mut *conn).await?;
        let mut written: Vec<Id> = sqlx::query_scalar(
            "UPDATE execution SET status = CASE WHEN status IN ('running','canceling') THEN 'canceling'::execution_status_enum ELSE 'cancelled'::execution_status_enum END,updated = NOW() WHERE id = ANY($1) AND status IN ('requested','scheduling','scheduled','running','canceling','cancelled') RETURNING id"
        ).bind(&changed_ids).fetch_all(&mut *conn).await?;
        written.sort_unstable();
        if written != changed_ids {
            return Err(Error::invalid_state(
                "Cancellation effect set changed; retry",
            ));
        }
        authority.ensure_fresh_on(conn).await?;
        Ok(changes)
    }
}
