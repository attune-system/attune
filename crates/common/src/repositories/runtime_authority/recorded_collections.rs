//! SQL collection authority, compiled only from a live recorded source.
//!
//! The source transaction protects revocation. The retained deadline is checked
//! again after the last query, without reacquiring source locks behind targets.
use super::ValidatedRuntimeAuthority;
use crate::{
    models::{ExecutionStatus, WorkQueue, WorkQueueItem},
    rbac::{Action, AuthorizationContext, Grant, Resource},
    repositories::{
        execution::{
            ExecutionSearchFilters, ExecutionSearchResult, ExecutionWithRefs, SELECT_COLUMNS,
        },
        iam_ownership,
        runtime_target_binding::grant_key,
        work_queue::{
            WorkQueueItemJsonPathSelector, WorkQueueItemRepository, WorkQueueItemSearchFilters,
            WorkQueueItemSearchResult, WorkQueueSearchFilters, WorkQueueSearchResult,
        },
    },
    runtime_authority::{AdmittedGrant, GrantTargetBindings},
    Error, Result,
};
use sqlx::{PgConnection, Postgres, QueryBuilder};

pub mod bulk;

/// No public constructor, mutable fields, deserializer or ordinary IAM input.
pub struct CollectionFilter<'a> {
    authority: &'a ValidatedRuntimeAuthority,
    resource: Resource,
    action: Action,
}

impl ValidatedRuntimeAuthority {
    pub fn execution_collection(&self, action: Action) -> CollectionFilter<'_> {
        CollectionFilter {
            authority: self,
            resource: Resource::Executions,
            action,
        }
    }

    pub fn queue_collection(&self) -> CollectionFilter<'_> {
        CollectionFilter {
            authority: self,
            resource: Resource::Queues,
            action: Action::Read,
        }
    }

    pub fn queue_item_collection(&self) -> CollectionFilter<'_> {
        CollectionFilter {
            authority: self,
            resource: Resource::QueueItems,
            action: Action::Read,
        }
    }
}

impl CollectionFilter<'_> {
    fn has_operation(&self) -> bool {
        self.authority
            .record
            .authority
            .grants
            .iter()
            .any(|admitted| {
                let approved = match admitted {
                    AdmittedGrant::PackOwned { .. } => true,
                    AdmittedGrant::External { approval_id, .. } => {
                        self.authority.approvals.contains(approval_id)
                    }
                };
                approved
                    && static_match(
                        admitted.grant(),
                        self.resource,
                        self.action,
                        self.authority.record.authority.owner.sponsor_identity_id,
                    )
                    && grant_key(admitted)
                        .is_ok_and(|key| self.authority.concrete_targets.contains_key(&key))
            })
    }

    fn require(&self, resource: Resource) -> Result<()> {
        if self.resource != resource {
            return Err(Error::PermissionDenied(
                "Wrong recorded collection filter".into(),
            ));
        }
        self.authority.ensure_fresh()
    }

    fn push(&self, query: &mut QueryBuilder<'_, Postgres>) {
        let (id, reference, kind, raw_pack) = match self.resource {
            Resource::Executions => (
                "e.id",
                "e.action_ref",
                "execution",
                iam_ownership::execution_pack("e"),
            ),
            Resource::Queues => (
                "work_queue.id",
                "work_queue.ref",
                "work_queue",
                iam_ownership::component_pack("work_queue"),
            ),
            Resource::QueueItems => (
                "work_queue_item.id",
                "work_queue_item.queue_ref",
                "work_queue_item",
                format!(
                    "(SELECT {} FROM work_queue q WHERE q.id = work_queue_item.queue)",
                    iam_ownership::component_pack("q")
                ),
            ),
            _ => {
                query.push("FALSE");
                return;
            }
        };
        // A deleted owner cannot be reconstructed from a retained ref or JSON.
        let pack = format!("(SELECT collection_pack.id FROM pack collection_pack WHERE collection_pack.id = {raw_pack})");
        query.push("(FALSE");
        if self.authority.ensure_fresh().is_ok() {
            for admitted in &self.authority.record.authority.grants {
                let owned_pack = match admitted {
                    AdmittedGrant::PackOwned { pack_id, .. } => Some(*pack_id),
                    AdmittedGrant::External { approval_id, .. }
                        if self.authority.approvals.contains(approval_id) =>
                    {
                        None
                    }
                    _ => continue,
                };
                let grant = admitted.grant();
                if !static_match(
                    grant,
                    self.resource,
                    self.action,
                    self.authority.record.authority.owner.sponsor_identity_id,
                ) {
                    continue;
                }
                let Some(binding) = grant_key(admitted)
                    .ok()
                    .and_then(|key| self.authority.concrete_targets.get(&key))
                else {
                    continue;
                };
                query.push(" OR (");
                push_grant(
                    query, grant, binding, owned_pack, id, reference, kind, &pack,
                );
                query.push(")");
            }
        }
        // Standard deliberately has no execution/queue/item operation.
        query.push(")");
    }
}

// These targets have no owner, visibility, encryption, identity attributes or
// execution-relative context. Keep the canonical matcher for those dimensions.
fn static_match(grant: &Grant, resource: Resource, action: Action, identity: i64) -> bool {
    let mut residual = grant.clone();
    if let Some(scope) = &mut residual.constraints {
        scope.ids = None;
        scope.refs = None;
        scope.pack_refs = None;
        scope.managing_pack_id = None;
    }
    residual.allows(resource, action, &AuthorizationContext::new(identity))
}

fn push_grant(
    query: &mut QueryBuilder<'_, Postgres>,
    grant: &Grant,
    binding: &GrantTargetBindings,
    owned_pack: Option<i64>,
    id: &str,
    reference: &str,
    kind: &str,
    pack: &str,
) {
    query.push("TRUE");
    if let Some(owner) = owned_pack {
        push_pack_candidates(query, kind, vec![owner]);
        query.push(format!(" AND {pack} = ")).push_bind(owner);
    }
    if let Some(scope) = &grant.constraints {
        if let Some(owner) = scope.managing_pack_id {
            if owned_pack.is_none() {
                push_pack_candidates(query, kind, vec![owner]);
            }
            query.push(format!(" AND {pack} = ")).push_bind(owner);
        }
        if let Some(ids) = &scope.ids {
            query
                .push(format!(" AND {id} = ANY("))
                .push_bind(ids.clone())
                .push(")");
        }
        if let Some(refs) = &scope.refs {
            query
                .push(format!(" AND {reference} = ANY("))
                .push_bind(refs.clone())
                .push(")");
        }
        if let Some(refs) = &scope.pack_refs {
            query
                .push(format!(
                    " AND (SELECT p.ref FROM pack p WHERE p.id = {pack}) = ANY("
                ))
                .push_bind(refs.clone())
                .push(")");
        }
    }
    if let Some(targets) = &binding.targets {
        query.push(" AND (FALSE");
        for target in targets {
            if target.kind != kind || target.owner_type.is_some() || target.owner_id.is_some() {
                continue;
            }
            query
                .push(format!(" OR ({id} = "))
                .push_bind(target.id)
                .push(" AND ");
            if let Some(owner) = target.pack_id {
                query.push(format!("{pack} = ")).push_bind(owner);
            } else {
                query.push(format!("{pack} IS NULL"));
            }
            query.push(")");
        }
        query.push(")");
    }
    // These resource contexts never expose a data owner. An owner pin cannot
    // match merely because its ID happens to equal a queue or execution ID.
    if binding.owners.is_some() {
        query.push(" AND FALSE");
    }
    if let Some(packs) = &binding.packs {
        if owned_pack.is_none() {
            push_pack_candidates(query, kind, packs.iter().copied().collect());
        }
        query
            .push(format!(" AND {pack} = ANY("))
            .push_bind(packs.iter().copied().collect::<Vec<_>>())
            .push(")");
    }
}

// Indexable narrowing only. The structural owner/digest and stable-pin clauses
// above remain mandatory. A local query need not run a scalar ownership lookup
// for every unrelated execution before PostgreSQL can use its release-pin index.
fn push_pack_candidates(query: &mut QueryBuilder<'_, Postgres>, kind: &str, packs: Vec<i64>) {
    match kind {
        "execution" => {
            query.push(" AND e.pack_release IN (SELECT candidate_release.id FROM pack_release candidate_release WHERE candidate_release.pack = ANY(")
            .push_bind(packs).push("))");
        }
        "work_queue" => {
            query
                .push(" AND work_queue.pack = ANY(")
                .push_bind(packs)
                .push(")");
        }
        "work_queue_item" => {
            query.push(" AND work_queue_item.queue IN (SELECT candidate_queue.id FROM work_queue candidate_queue WHERE candidate_queue.management_origin = 'pack' AND candidate_queue.pack = ANY(")
            .push_bind(packs).push("))");
        }
        _ => {
            query.push(" AND FALSE");
        }
    }
}

pub async fn execution_status_counts(
    conn: &mut PgConnection,
    filter: &CollectionFilter<'_>,
) -> Result<Vec<(ExecutionStatus, i64)>> {
    filter.require(Resource::Executions)?;
    let mut query = QueryBuilder::new("SELECT e.status, COUNT(*)::BIGINT FROM execution e WHERE ");
    filter.push(&mut query);
    query.push(" GROUP BY e.status");
    let rows = query.build_query_as().fetch_all(&mut *conn).await?;
    filter.authority.ensure_fresh_on(conn).await?;
    Ok(rows)
}

fn execution_where(
    q: &mut QueryBuilder<'_, Postgres>,
    filter: &CollectionFilter<'_>,
    f: &ExecutionSearchFilters,
) {
    q.push(" WHERE ");
    filter.push(q);
    macro_rules! eq {
        ($column:expr, $value:expr) => {
            if let Some(value) = $value {
                q.push(concat!(" AND ", $column, " = "))
                    .push_bind(value.clone());
            }
        };
    }
    macro_rules! reference {
        ($column:expr, $value:expr) => {
            if let Some(value) = $value {
                if let Some(pattern) = crate::repositories::ref_filter_like_pattern(value) {
                    q.push(concat!(" AND ", $column, " LIKE "))
                        .push_bind(pattern)
                        .push(r" ESCAPE '\'");
                } else {
                    q.push(concat!(" AND ", $column, " = "))
                        .push_bind(value.clone());
                }
            }
        };
    }
    eq!("e.status", f.status);
    reference!("e.action_ref", &f.action_ref);
    if let Some(pack) = &f.pack_name {
        let pattern = crate::repositories::ref_filter_like_pattern(&format!("{pack}.*"))
            .expect("pack filter includes wildcard");
        q.push(" AND e.action_ref LIKE ")
            .push_bind(pattern)
            .push(r" ESCAPE '\'");
    }
    eq!("e.enforcement", f.enforcement);
    eq!("e.parent", f.parent);
    eq!("e.executor", f.executor);
    eq!("e.trace_tag", &f.trace_tag);
    reference!("enf.rule_ref", &f.rule_ref);
    reference!("enf.trigger_ref", &f.trigger_ref);
    if f.top_level_only {
        q.push(" AND e.parent IS NULL");
    }
}

pub async fn executions(
    conn: &mut PgConnection,
    filter: &CollectionFilter<'_>,
    f: &ExecutionSearchFilters,
) -> Result<ExecutionSearchResult> {
    filter.require(Resource::Executions)?;
    if f.result_contains.is_some() {
        return Err(Error::PermissionDenied(
            "Recorded metadata cannot inspect plaintext".into(),
        ));
    }
    if f.parent.is_some()
        || f.enforcement.is_some()
        || f.rule_ref.is_some()
        || f.trigger_ref.is_some()
    {
        return Err(Error::PermissionDenied(
            "Recorded metadata predicates cannot inspect unproven execution links".into(),
        ));
    }
    const FROM: &str = " FROM execution e LEFT JOIN enforcement enf ON enf.id = e.enforcement";
    let total = if f.include_total {
        let mut q = QueryBuilder::new(format!("SELECT COUNT(*)::BIGINT{FROM}"));
        execution_where(&mut q, filter, f);
        let total: i64 = q.build_query_scalar().fetch_one(&mut *conn).await?;
        Some(total.max(0) as u64)
    } else {
        None
    };
    let columns = execution_metadata_columns();
    let mut q = QueryBuilder::new(format!(
        "SELECT {columns}, NULL::text AS rule_ref, NULL::text AS trigger_ref{FROM}"
    ));
    execution_where(&mut q, filter, f);
    q.push(" ORDER BY e.created DESC, e.id DESC LIMIT ")
        .push_bind(i64::from(f.limit.saturating_add(1)))
        .push(" OFFSET ")
        .push_bind(i64::from(f.offset));
    let mut rows: Vec<ExecutionWithRefs> = q.build_query_as().fetch_all(&mut *conn).await?;
    let has_next = rows.len() > f.limit as usize;
    rows.truncate(f.limit as usize);
    filter.authority.ensure_fresh_on(conn).await?;
    Ok(ExecutionSearchResult {
        rows,
        total,
        has_next,
    })
}

pub(crate) fn execution_metadata_columns() -> String {
    SELECT_COLUMNS
        .split(", ")
        .map(|column| match column {
            "config"
            | "result"
            | "workflow_task"
            | "executable_snapshot"
            | "env_vars"
            | "worker_selector"
            | "worker_tolerations"
            | "worker_affinity"
            | "parent"
            | "enforcement"
            | "original_execution" => format!("NULL AS {column}"),
            _ => format!("e.{column}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// An exact bounded control set, using the same typed target SQL as metadata
/// collections. Does not acquire graph/source/target locks behind its caller.
pub(crate) async fn execution_cancel_ids(
    conn: &mut PgConnection,
    authority: &ValidatedRuntimeAuthority,
    ids: &[i64],
) -> Result<Vec<i64>> {
    if ids.is_empty() || ids.len() > 256 {
        return Err(Error::PermissionDenied(
            "Cancellation target unavailable or operation denied".into(),
        ));
    }
    let filter = authority.execution_collection(Action::Cancel);
    filter.require(Resource::Executions)?;
    let mut query = QueryBuilder::new("SELECT e.id FROM execution e WHERE e.id = ANY(");
    query.push_bind(ids.to_vec()).push(") AND ");
    filter.push(&mut query);
    query.push(" ORDER BY e.id");
    let allowed = query.build_query_scalar().fetch_all(&mut *conn).await?;
    authority.ensure_fresh_on(conn).await?;
    Ok(allowed)
}

fn queue_where(
    q: &mut QueryBuilder<'_, Postgres>,
    filter: &CollectionFilter<'_>,
    f: &WorkQueueSearchFilters,
) {
    q.push(" WHERE retired_at IS NULL AND ");
    filter.push(q);
    macro_rules! eq {
        ($column:expr, $value:expr) => {
            if let Some(value) = $value {
                q.push(concat!(" AND ", $column, " = "))
                    .push_bind(value.clone());
            }
        };
    }
    eq!("pack", f.pack);
    eq!("pack_ref", &f.pack_ref);
    eq!("dispatch_action", f.dispatch_action);
    eq!("effective_enabled", f.enabled);
    eq!("is_adhoc", f.is_adhoc);
    if let Some(search) = &f.search {
        let search = format!("%{}%", search.trim());
        q.push(" AND (ref ILIKE ")
            .push_bind(search.clone())
            .push(" OR label ILIKE ")
            .push_bind(search.clone())
            .push(" OR COALESCE(description, '') ILIKE ")
            .push_bind(search)
            .push(")");
    }
}

pub async fn queues(
    conn: &mut PgConnection,
    filter: &CollectionFilter<'_>,
    f: &WorkQueueSearchFilters,
) -> Result<WorkQueueSearchResult> {
    filter.require(Resource::Queues)?;
    let mut count = QueryBuilder::new("SELECT COUNT(*)::BIGINT FROM work_queue");
    queue_where(&mut count, filter, f);
    let total: i64 = count.build_query_scalar().fetch_one(&mut *conn).await?;
    let mut q = QueryBuilder::new("SELECT ");
    for (index, column) in crate::models::WORK_QUEUE_SELECT_COLUMNS
        .split(", ")
        .enumerate()
    {
        if index > 0 {
            q.push(", ");
        }
        match column {
            "config" | "action_params" => {
                q.push_bind(crate::secret_values::redaction_marker())
                    .push(format!("::jsonb AS {column}"));
            }
            "item_schema" => {
                q.push("'{}'::jsonb AS item_schema");
            }
            _ => {
                q.push(column);
            }
        }
    }
    q.push(" FROM work_queue");
    queue_where(&mut q, filter, f);
    q.push(" ORDER BY ref ASC LIMIT ")
        .push_bind(i64::from(f.limit))
        .push(" OFFSET ")
        .push_bind(i64::from(f.offset));
    let rows: Vec<WorkQueue> = q.build_query_as().fetch_all(&mut *conn).await?;
    filter.authority.ensure_fresh_on(conn).await?;
    Ok(WorkQueueSearchResult {
        rows,
        total: total.max(0) as u64,
    })
}

fn item_where(
    q: &mut QueryBuilder<'_, Postgres>,
    filter: &CollectionFilter<'_>,
    f: &WorkQueueItemSearchFilters,
) {
    q.push(" WHERE ");
    filter.push(q);
    macro_rules! eq {
        ($column:expr, $value:expr) => {
            if let Some(value) = $value {
                q.push(concat!(" AND ", $column, " = "))
                    .push_bind(value.clone());
            }
        };
    }
    eq!("queue", f.queue);
    eq!("queue_ref", &f.queue_ref);
    eq!("item_key", &f.item_key);
    eq!("enqueue_source", &f.enqueue_source);
    eq!("requested_by_identity", f.requested_by_identity);
    eq!("requested_by_execution", f.requested_by_execution);
    eq!("requested_by_enforcement", f.requested_by_enforcement);
    eq!("leased_execution", f.leased_execution);
    eq!("lease_token", f.lease_token);
    if let Some(statuses) = &f.statuses {
        q.push(" AND status = ANY(")
            .push_bind(statuses.clone())
            .push(")");
    }
    if f.only_expired_leases {
        q.push(" AND status = 'leased' AND lease_expires_at IS NOT NULL AND lease_expires_at <= ")
            .push_bind(f.now.unwrap_or_else(chrono::Utc::now));
    }
}

pub async fn queue_items(
    conn: &mut PgConnection,
    filter: &CollectionFilter<'_>,
    f: &WorkQueueItemSearchFilters,
    selector: Option<&WorkQueueItemJsonPathSelector>,
) -> Result<WorkQueueItemSearchResult> {
    filter.require(Resource::QueueItems)?;
    if f.requested_by_execution.is_some()
        || f.requested_by_enforcement.is_some()
        || f.leased_execution.is_some()
        || f.lease_token.is_some()
        || f.only_expired_leases
    {
        return Err(Error::PermissionDenied(
            "Recorded metadata predicates cannot inspect unproven item links or leases".into(),
        ));
    }
    if !filter.has_operation() {
        return Err(Error::PermissionDenied(
            "Recorded queue item read denied".into(),
        ));
    }
    if let Some(selector) = selector {
        return selected_metadata_page(conn, filter, f, selector).await;
    }
    let mut count = QueryBuilder::new("SELECT COUNT(*)::BIGINT FROM work_queue_item");
    item_where(&mut count, filter, f);
    let total: i64 = count.build_query_scalar().fetch_one(&mut *conn).await?;
    let columns = item_metadata_columns();
    let mut q = QueryBuilder::new(format!("SELECT {columns} FROM work_queue_item"));
    item_where(&mut q, filter, f);
    q.push(" ORDER BY priority DESC, created ASC, id ASC LIMIT ")
        .push_bind(i64::from(f.limit))
        .push(" OFFSET ")
        .push_bind(i64::from(f.offset));
    let rows: Vec<WorkQueueItem> = q.build_query_as().fetch_all(&mut *conn).await?;
    filter.authority.ensure_fresh_on(conn).await?;
    Ok(WorkQueueItemSearchResult {
        rows,
        total: total.max(0) as u64,
    })
}

// Materialization is an authorization boundary, not merely an optimizer hint.
// A JSONPath error must not depend on a payload outside the selected Read scope.
fn visible_selector_cte(
    query: &mut QueryBuilder<'_, Postgres>,
    read: &CollectionFilter<'_>,
    f: &WorkQueueItemSearchFilters,
    operation: Option<&CollectionFilter<'_>>,
) {
    query.push(format!(
        "WITH visible_item AS MATERIALIZED (SELECT {}, {} AS selector_document, ",
        item_metadata_columns(),
        recorded_item_document_sql()
    ));
    if let Some(operation) = operation {
        operation.push(query);
    } else {
        query.push("TRUE");
    }
    query.push(" AS operation_allowed FROM work_queue_item");
    item_where(query, read, f);
    query
        .push(" AND status = ANY(")
        .push_bind(WorkQueueItemRepository::mutable_pending_statuses())
        .push(")) ");
}

fn selector_predicate(
    query: &mut QueryBuilder<'_, Postgres>,
    selector: &WorkQueueItemJsonPathSelector,
) {
    query
        .push("jsonb_path_exists(selector_document, ")
        .push_bind(selector.path.clone())
        .push("::jsonpath, ")
        .push_bind(selector.vars.clone())
        .push("::jsonb)");
}

async fn selected_metadata_page(
    conn: &mut PgConnection,
    filter: &CollectionFilter<'_>,
    f: &WorkQueueItemSearchFilters,
    selector: &WorkQueueItemJsonPathSelector,
) -> Result<WorkQueueItemSearchResult> {
    let mut count = QueryBuilder::new("");
    visible_selector_cte(&mut count, filter, f, None);
    count.push("SELECT COUNT(*)::BIGINT FROM visible_item WHERE ");
    selector_predicate(&mut count, selector);
    let total: i64 = count.build_query_scalar().fetch_one(&mut *conn).await?;
    let mut query = QueryBuilder::new("");
    visible_selector_cte(&mut query, filter, f, None);
    query.push(format!(
        "SELECT {} FROM visible_item WHERE ",
        crate::models::WORK_QUEUE_ITEM_SELECT_COLUMNS
    ));
    selector_predicate(&mut query, selector);
    query
        .push(" ORDER BY priority DESC, created ASC, id ASC LIMIT ")
        .push_bind(i64::from(f.limit))
        .push(" OFFSET ")
        .push_bind(i64::from(f.offset));
    let rows = query.build_query_as().fetch_all(&mut *conn).await?;
    filter.authority.ensure_fresh_on(conn).await?;
    Ok(WorkQueueItemSearchResult {
        rows,
        total: total.max(0) as u64,
    })
}

fn item_metadata_columns() -> String {
    crate::models::WORK_QUEUE_ITEM_SELECT_COLUMNS
        .split(", ")
        .map(|column| match column {
            "metadata" => "'null'::jsonb AS metadata".to_owned(),
            "requested_by_execution"
            | "requested_by_enforcement"
            | "leased_execution"
            | "lease_token"
            | "lease_expires_at"
            | "last_error"
            | "ack_summary"
            | "schema_binding"
            | "executable_snapshot" => format!("NULL AS {column}"),
            _ => column.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// Preview predicates must see no more than the recorded metadata projection.
// In particular, arbitrary metadata can contain copied private batch results.
// Redacting only the returned rows would still expose those values via COUNT.
fn recorded_item_document_sql() -> &'static str {
    "jsonb_strip_nulls(jsonb_build_object(\
        'payload', payload, 'item_key', item_key, 'priority', priority, \
        'status', status, 'enqueue_source', enqueue_source, 'attempt_count', attempt_count))"
}

#[cfg(test)]
mod semantics_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{rbac::GrantConstraints, runtime_authority::BoundRuntimeTarget};

    #[test]
    fn absent_context_dimensions_do_not_become_collection_powers() {
        for constraints in [
            serde_json::json!({"owner_types":["pack"]}),
            serde_json::json!({"visibility":["private"]}),
            serde_json::json!({"execution_scope":"self"}),
            serde_json::json!({"encrypted":false}),
            serde_json::json!({"attributes":{"role":"admin"}}),
        ] {
            let grant = Grant {
                resource: Resource::Executions,
                actions: vec![Action::Read],
                constraints: Some(serde_json::from_value(constraints).unwrap()),
            };
            assert!(!static_match(&grant, Resource::Executions, Action::Read, 7));
        }
    }

    #[test]
    fn ownership_selectors_and_stable_pins_are_conjoined_even_when_empty() {
        let grant = Grant {
            resource: Resource::Queues,
            actions: vec![Action::Read],
            constraints: Some(GrantConstraints {
                ids: Some(vec![]),
                refs: Some(vec!["tenant.queue".into()]),
                pack_refs: Some(vec!["tenant".into()]),
                ..Default::default()
            }),
        };
        let binding = GrantTargetBindings {
            targets: Some(std::collections::BTreeSet::from([BoundRuntimeTarget {
                kind: "work_queue".into(),
                id: 12,
                pack_id: Some(5),
                owner_type: None,
                owner_id: None,
            }])),
            packs: Some(std::collections::BTreeSet::from([5])),
            ..Default::default()
        };
        let mut q = QueryBuilder::new("");
        push_grant(
            &mut q,
            &grant,
            &binding,
            Some(5),
            "q.id",
            "q.ref",
            "work_queue",
            "actual_pack",
        );
        let sql = q.sql();
        assert!(sql.contains("AND actual_pack ="));
        assert!(sql.contains("AND q.id = ANY("));
        assert!(sql.contains("AND q.ref = ANY("));
        assert!(sql.contains("p.ref FROM pack"));
        assert!(sql.contains("OR (q.id ="));
        assert!(sql.contains("AND actual_pack = ANY("));
        assert!(!sql.contains("tenant.queue"));
    }

    #[test]
    fn foreign_kind_and_owner_pins_cannot_match_numeric_id_collisions() {
        let grant = Grant {
            resource: Resource::QueueItems,
            actions: vec![Action::Read],
            constraints: None,
        };
        let binding = GrantTargetBindings {
            targets: Some(std::collections::BTreeSet::from([BoundRuntimeTarget {
                kind: "work_queue".into(),
                id: 12,
                pack_id: Some(5),
                owner_type: None,
                owner_id: None,
            }])),
            owners: Some(std::collections::BTreeSet::new()),
            ..Default::default()
        };
        let mut q = QueryBuilder::new("");
        push_grant(
            &mut q,
            &grant,
            &binding,
            None,
            "i.id",
            "i.queue_ref",
            "work_queue_item",
            "actual_pack",
        );
        assert_eq!(q.sql(), "TRUE AND (FALSE) AND FALSE");
    }
}
