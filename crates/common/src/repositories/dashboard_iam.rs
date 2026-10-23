//! Raw dashboard reads whose authority cannot be represented by ref-only hourly
//! summaries. Authorize source rows before DISTINCT, grouping, ordering or limits.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Duration, Timelike, Utc};
use serde_json::Value;
use sqlx::{Executor, Postgres, QueryBuilder};

use super::iam_ownership::{self, IamTargetSql};
use crate::{
    models::Id,
    rbac::{Action, Grant, Resource},
    Error, Result,
};

#[derive(Debug, Clone)]
pub struct DashboardIamScope {
    pub identity: Id,
    pub attributes: HashMap<String, Value>,
    pub grants: Vec<Grant>,
    pub resource: Resource,
    pub action: Action,
}

pub struct DashboardIamFilters<'a> {
    pub source: &'a str,
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub pack_refs: Option<&'a BTreeSet<String>>,
    pub primary_refs: Option<&'a BTreeSet<String>>,
    pub statuses: Vec<String>,
    pub assigned_to: Option<Id>,
    pub sla_target_seconds: i64,
    pub limit: usize,
    pub small_cohort_min: Option<i64>,
}

pub struct DashboardIamRepository;

impl DashboardIamRepository {
    pub async fn query<'e, E>(
        executor: E,
        scope: &DashboardIamScope,
        filters: &DashboardIamFilters<'_>,
    ) -> Result<Value>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        build_query(scope, filters)?
            .build_query_scalar()
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

fn build_query(
    scope: &DashboardIamScope,
    filters: &DashboardIamFilters<'_>,
) -> Result<QueryBuilder<'static, Postgres>> {
    let execution_pack = iam_ownership::execution_pack("e");
    let rule_pack = iam_ownership::component_pack("r");
    let trigger_pack = iam_ownership::component_pack("t");
    let queue_pack = iam_ownership::component_pack("q");
    let ancestors = iam_ownership::execution_ancestor_ids("e");
    let execution = IamTargetSql {
        id: "e.id",
        reference: "e.action_ref",
        pack_ref: "split_part(e.action_ref,'.',1)",
        managing_pack: &execution_pack,
        owner_identity: Some("e.executor"),
        execution_owner: Some("e.executor"),
        ancestor_identities: Some(&ancestors),
    };
    let event = IamTargetSql {
        id: "ev.id",
        reference: "ev.trigger_ref",
        pack_ref: "t.pack_ref",
        managing_pack: &trigger_pack,
        owner_identity: None,
        execution_owner: None,
        ancestor_identities: None,
    };
    let enforcement = IamTargetSql {
        id: "ef.id",
        reference: "ef.rule_ref",
        pack_ref: "r.pack_ref",
        managing_pack: &rule_pack,
        owner_identity: None,
        execution_owner: None,
        ancestor_identities: None,
    };
    let inquiry = IamTargetSql {
        id: "i.id",
        reference: "('inquiry:' || i.id::TEXT)",
        pack_ref: "split_part(e.action_ref,'.',1)",
        managing_pack: &execution_pack,
        owner_identity: Some("e.executor"),
        execution_owner: Some("e.executor"),
        ancestor_identities: None,
    };
    let item = IamTargetSql {
        id: "qi.id",
        reference: "qi.queue_ref",
        pack_ref: "q.pack_ref",
        managing_pack: &queue_pack,
        owner_identity: None,
        execution_owner: None,
        ancestor_identities: None,
    };
    let queue = IamTargetSql {
        id: "q.id",
        reference: "q.ref",
        pack_ref: "q.pack_ref",
        managing_pack: &queue_pack,
        owner_identity: None,
        execution_owner: None,
        ancestor_identities: None,
    };
    let latest = matches!(
        filters.source,
        "latest_action_result" | "action_result_path" | "last_execution"
    );
    let execution_buckets = matches!(
        filters.source,
        "execution_count" | "execution_timeseries" | "execution_status_breakdown"
    );
    let (select, from, time, target, resource, tail) = match filters.source {
        "latest_action_result" | "action_result_path" | "last_execution" => (
            "DISTINCT ON (e.action_ref) e.action_ref, e.id AS execution_id, e.status::TEXT AS status, e.created AS created_at, e.started_at, e.updated AS updated_at, e.trace_tag, e.result",
            "execution e", Some("e.created"), &execution, Resource::Executions, "ORDER BY e.action_ref, e.created DESC, e.id DESC"),
        "execution_count" | "execution_timeseries" => (
            "date_trunc('hour',h.time,'UTC') AS bucket_start, e.action_ref AS series, COUNT(*)::BIGINT AS count",
            "execution_history h JOIN execution e ON e.id = h.entity_id AND e.action_ref = h.entity_ref", Some("h.time"), &execution, Resource::Executions,
            "GROUP BY 1,2 ORDER BY 1,2"),
        "execution_status_breakdown" => (
            "date_trunc('hour',h.time,'UTC') AS bucket_start, h.new_values->>'status' AS status, COUNT(*)::BIGINT AS count",
            "execution_history h JOIN execution e ON e.id = h.entity_id AND e.action_ref = h.entity_ref", Some("h.time"), &execution, Resource::Executions,
            "GROUP BY 1,2 ORDER BY 1,2"),
        "execution_duration_stats" => (
            "date_trunc('hour',e.updated,'UTC') AS bucket_start, e.action_ref AS series, COUNT(*)::BIGINT AS execution_count, \
             COALESCE(AVG(EXTRACT(EPOCH FROM(e.updated-e.started_at))),0)::DOUBLE PRECISION AS avg_duration_seconds, \
             COALESCE(PERCENTILE_CONT(0.5) WITHIN GROUP(ORDER BY EXTRACT(EPOCH FROM(e.updated-e.started_at))),0)::DOUBLE PRECISION AS p50_duration_seconds, \
             COALESCE(PERCENTILE_CONT(0.95) WITHIN GROUP(ORDER BY EXTRACT(EPOCH FROM(e.updated-e.started_at))),0)::DOUBLE PRECISION AS p95_duration_seconds, \
             COALESCE(MAX(EXTRACT(EPOCH FROM(e.updated-e.started_at))),0)::DOUBLE PRECISION AS max_duration_seconds",
            "execution e", Some("e.updated"), &execution, Resource::Executions, "GROUP BY 1,2 ORDER BY 1,2"),
        "event_count" | "event_timeseries" => (
            "date_trunc('hour',ev.created,'UTC') AS bucket_start, ev.trigger_ref AS series, COUNT(*)::BIGINT AS count",
            "event ev LEFT JOIN trigger t ON t.id = ev.trigger AND t.ref = ev.trigger_ref", Some("ev.created"), &event, Resource::Events, "GROUP BY 1,2 ORDER BY 1,2"),
        "last_event" => (
            "DISTINCT ON(ev.trigger_ref) ev.trigger_ref, ev.id AS event_id, ev.created, ev.source_ref, ev.rule_ref, ev.trace_tag",
            "event ev LEFT JOIN trigger t ON t.id = ev.trigger AND t.ref = ev.trigger_ref", Some("ev.created"), &event, Resource::Events,
            "ORDER BY ev.trigger_ref, ev.created DESC, ev.id DESC"),
        "enforcement_count" | "enforcement_timeseries" => (
            "date_trunc('hour',ef.created,'UTC') AS bucket_start, ef.rule_ref AS series, COUNT(*)::BIGINT AS count",
            "enforcement ef LEFT JOIN rule r ON r.id = ef.rule AND r.ref = ef.rule_ref", Some("ef.created"), &enforcement, Resource::Enforcements, "GROUP BY 1,2 ORDER BY 1,2"),
        "last_enforcement" => (
            "DISTINCT ON(ef.rule_ref) ef.rule_ref, ef.id AS enforcement_id, ef.trigger_ref, ef.status::TEXT AS status, ef.created, ef.resolved_at, ef.event AS event_id",
            "enforcement ef LEFT JOIN rule r ON r.id = ef.rule AND r.ref = ef.rule_ref", Some("ef.created"), &enforcement, Resource::Enforcements,
            "ORDER BY ef.rule_ref, ef.created DESC, ef.id DESC"),
        "inquiry_backlog" => (
            "split_part(e.action_ref,'.',1) AS pack_ref, i.assigned_to, COUNT(*)::BIGINT AS pending_count, \
             COUNT(*) FILTER(WHERE i.timeout_at IS NOT NULL AND i.timeout_at < NOW())::BIGINT AS overdue_count",
            "inquiry i LEFT JOIN execution e ON e.id = i.created_by_execution", None, &inquiry, Resource::Inquiries, "GROUP BY 1,2 ORDER BY 1,2"),
        "inquiry_sla" => (
            "date_trunc('hour',i.created,'UTC') AS bucket_start, split_part(e.action_ref,'.',1) AS pack_ref, i.assigned_to, \
             COUNT(*)::BIGINT AS total_inquiries, COUNT(*) FILTER(WHERE elapsed <= target_seconds)::BIGINT AS within_sla_count, \
             COUNT(*) FILTER(WHERE elapsed > target_seconds)::BIGINT AS breached_count, COUNT(*) FILTER(WHERE i.status::TEXT = 'pending')::BIGINT AS open_count, \
             target_seconds::BIGINT AS sla_target_seconds, (COUNT(*) FILTER(WHERE elapsed <= target_seconds))::DOUBLE PRECISION / NULLIF(COUNT(*),0) AS compliance_rate",
            "inquiry i LEFT JOIN execution e ON e.id = i.created_by_execution \
             CROSS JOIN LATERAL (SELECT EXTRACT(EPOCH FROM(COALESCE(i.responded_at, CASE WHEN i.status::TEXT = 'timeout' THEN COALESCE(i.updated,i.timeout_at) END,NOW())-i.created)) AS elapsed) duration \
             CROSS JOIN sla", Some("i.created"), &inquiry, Resource::Inquiries, "GROUP BY 1,2,3,target_seconds ORDER BY 1,2,3"),
        "queue_backlog" => (
            "qi.queue_ref, COUNT(*) FILTER(WHERE qi.status::TEXT='queued')::BIGINT AS queued, COUNT(*) FILTER(WHERE qi.status::TEXT='retry')::BIGINT AS retry, \
             COUNT(*) FILTER(WHERE qi.status::TEXT='leased')::BIGINT AS leased, COUNT(*)::BIGINT AS total_backlog",
            "work_queue_item qi LEFT JOIN work_queue q ON q.id=qi.queue AND q.ref=qi.queue_ref", None, &item, Resource::QueueItems, "GROUP BY 1 ORDER BY 1"),
        "queue_throughput" => (
            "date_trunc('hour',qi.updated,'UTC') AS bucket_start, qi.queue_ref, COUNT(*) FILTER(WHERE qi.status::TEXT='completed')::BIGINT AS completed, \
             COUNT(*) FILTER(WHERE qi.status::TEXT='failed')::BIGINT AS failed, COUNT(*) FILTER(WHERE qi.status::TEXT='skipped')::BIGINT AS skipped, \
             COUNT(*) FILTER(WHERE qi.status::TEXT='cancelled')::BIGINT AS cancelled, COUNT(*)::BIGINT AS total_processed",
            "work_queue_item qi LEFT JOIN work_queue q ON q.id=qi.queue AND q.ref=qi.queue_ref", Some("qi.updated"), &item, Resource::QueueItems, "GROUP BY 1,2 ORDER BY 1,2"),
        "queue_dispatch_stats" => (
            "date_trunc('hour',COALESCE(e.updated,d.updated),'UTC') AS bucket_start, d.queue_ref, COALESCE(e.status::TEXT,d.status::TEXT) AS status, \
             COUNT(*)::BIGINT AS dispatch_count, COALESCE(SUM(d.leased_item_count),0)::BIGINT AS leased_item_count, \
             COALESCE(AVG(EXTRACT(EPOCH FROM(COALESCE(e.updated,d.updated)-COALESCE(e.started_at,e.created,d.created)))),0)::DOUBLE PRECISION AS avg_duration_seconds, \
             COALESCE(MAX(EXTRACT(EPOCH FROM(COALESCE(e.updated,d.updated)-COALESCE(e.started_at,e.created,d.created)))),0)::DOUBLE PRECISION AS max_duration_seconds",
            "work_queue_dispatch d LEFT JOIN execution e ON e.id=d.execution LEFT JOIN work_queue q ON q.id=d.queue AND q.ref=d.queue_ref",
            Some("COALESCE(e.updated,d.updated)"), &queue, Resource::Queues, "GROUP BY 1,2,3 ORDER BY 1,2,3"),
        _ => return Err(Error::validation("Unsupported structurally scoped dashboard source")),
    };
    if scope.resource != resource || scope.action != Action::Read {
        return Err(Error::PermissionDenied(
            "Dashboard source authority mismatch".into(),
        ));
    }
    let mut query = QueryBuilder::new("");
    if filters.source == "inquiry_sla" {
        query
            .push("WITH sla AS (SELECT ")
            .push_bind(filters.sla_target_seconds as f64)
            .push("::DOUBLE PRECISION AS target_seconds) ");
    }
    query
        .push("SELECT COALESCE(jsonb_agg(to_jsonb(data)), '[]'::JSONB) FROM (SELECT ")
        .push(select)
        .push(" FROM ")
        .push(from);
    query.push(" WHERE ");
    iam_ownership::push_iam_grants(
        &mut query,
        &scope.grants,
        resource,
        scope.action,
        scope.identity,
        &scope.attributes,
        target,
    );
    let bucket_source = execution_buckets
        || matches!(
            filters.source,
            "event_count" | "event_timeseries" | "enforcement_count" | "enforcement_timeseries"
        );
    let since = if bucket_source {
        let first_hour = filters
            .since
            .with_minute(0)
            .and_then(|value| value.with_second(0))
            .and_then(|value| value.with_nanosecond(0))
            .unwrap_or(filters.since);
        if first_hour < filters.since {
            first_hour + Duration::hours(1)
        } else {
            first_hour
        }
    } else {
        filters.since
    };
    if let Some(time) = time {
        query
            .push(" AND ")
            .push(time)
            .push(" >= ")
            .push_bind(since)
            .push(" AND ")
            .push(time)
            .push(" < ")
            .push_bind(filters.until);
    }
    if let Some(refs) = filters.pack_refs {
        query
            .push(" AND ")
            .push(target.pack_ref)
            .push(" = ANY(")
            .push_bind(refs.iter().cloned().collect::<Vec<_>>())
            .push(")");
    }
    if let Some(refs) = filters.primary_refs {
        query
            .push(" AND ")
            .push(target.reference)
            .push(" = ANY(")
            .push_bind(refs.iter().cloned().collect::<Vec<_>>())
            .push(")");
    }
    if latest {
        query
            .push(" AND e.status::TEXT = ANY(")
            .push_bind(filters.statuses.clone())
            .push(")");
    }
    if execution_buckets {
        query
            .push(" AND 'status' = ANY(h.changed_fields) AND h.new_values->>'status' = ANY(")
            .push_bind(vec![
                "completed",
                "failed",
                "timeout",
                "cancelled",
                "abandoned",
            ])
            .push(")");
    }
    if filters.source == "execution_duration_stats" {
        query.push(" AND e.started_at IS NOT NULL AND e.status::TEXT IN ('completed','failed','timeout','cancelled','abandoned')");
    }
    if filters.source.starts_with("inquiry_") {
        if let Some(identity) = filters.assigned_to {
            query.push(" AND i.assigned_to = ").push_bind(identity);
        }
        if filters.source == "inquiry_backlog" {
            query.push(" AND i.status::TEXT='pending'");
        }
    }
    if filters.source == "queue_backlog" {
        query.push(" AND qi.status::TEXT IN ('queued','retry','leased')");
    }
    if filters.source == "queue_throughput" {
        query.push(" AND qi.status::TEXT IN ('completed','failed','skipped','cancelled')");
    }
    if filters.source == "queue_dispatch_stats" {
        query.push(" AND (e.status::TEXT IN ('completed','failed','timeout','cancelled','abandoned') OR (e.id IS NULL AND d.status::TEXT IN ('completed','failed','released','cancelled')))");
    }
    query.push(" ");
    if let Some(minimum) = filters.small_cohort_min {
        if let Some((grouping, ordering)) = tail.split_once(" ORDER BY ") {
            query
                .push(grouping)
                .push(" HAVING (COUNT(*) >= ")
                .push_bind(minimum);
            if filters.source == "queue_dispatch_stats" {
                query
                    .push(" OR COALESCE(SUM(d.leased_item_count),0) >= ")
                    .push_bind(minimum);
            }
            query.push(") ORDER BY ").push(ordering);
        } else {
            query.push(tail);
        }
    } else {
        query.push(tail);
    }
    query
        .push(" LIMIT ")
        .push_bind(filters.limit as i64)
        .push(") data");
    Ok(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_local_source_is_one_bounded_raw_query_with_scope_before_grouping_or_limit() {
        for (source, resource) in [
            ("last_execution", Resource::Executions),
            ("execution_count", Resource::Executions),
            ("execution_status_breakdown", Resource::Executions),
            ("execution_duration_stats", Resource::Executions),
            ("event_count", Resource::Events),
            ("last_event", Resource::Events),
            ("enforcement_count", Resource::Enforcements),
            ("last_enforcement", Resource::Enforcements),
            ("inquiry_backlog", Resource::Inquiries),
            ("inquiry_sla", Resource::Inquiries),
            ("queue_backlog", Resource::QueueItems),
            ("queue_throughput", Resource::QueueItems),
            ("queue_dispatch_stats", Resource::Queues),
        ] {
            let scope = DashboardIamScope {
                identity: 42,
                attributes: HashMap::new(),
                resource,
                action: Action::Read,
                grants: vec![Grant {
                    resource,
                    actions: vec![Action::Read],
                    constraints: Some(crate::rbac::GrantConstraints {
                        managing_pack_id: Some(17),
                        ids: Some(vec![8]),
                        ..Default::default()
                    }),
                }],
            };
            let now = Utc::now();
            let filters = DashboardIamFilters {
                source,
                since: now - Duration::hours(2),
                until: now,
                pack_refs: None,
                primary_refs: None,
                statuses: vec!["completed".into()],
                assigned_to: None,
                sla_target_seconds: 900,
                limit: 2001,
                small_cohort_min: Some(3),
            };
            let query = build_query(&scope, &filters).unwrap();
            let sql = query.sql();
            assert_eq!(
                sql.matches("jsonb_agg(to_jsonb(data))").count(),
                1,
                "{source}"
            );
            assert!(!sql.contains("summary"), "{source}");
            assert!(!sql.contains("permission_set"), "{source}");
            assert!(
                sql.find(" WHERE ").unwrap() < sql.rfind(" LIMIT ").unwrap(),
                "{source}"
            );
            assert!(sql.contains(" = ANY("), "{source}");
            assert!(
                sql.contains("management_origin = 'pack'")
                    || sql.contains("p.digest = e.pack_release_digest"),
                "{source}"
            );
        }
    }
}
