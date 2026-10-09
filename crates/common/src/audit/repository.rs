//! Read-side queries for audit events.

use chrono::{DateTime, Utc};
use sqlx::{Executor, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::models::Id;
use crate::Result;

use super::{AuditCategory, AuditEvent, AuditOutcome, PendingAuditEvent};

/// Filters for [`AuditRepository::search`].
#[derive(Debug, Clone, Default)]
pub struct AuditEventFilters {
    pub category: Option<AuditCategory>,
    pub event_type: Option<String>,
    /// Substring (case-insensitive) match on the actor login.
    pub actor_login_contains: Option<String>,
    pub outcome: Option<AuditOutcome>,
    pub actor_identity: Option<Id>,
    pub resource_type: Option<String>,
    pub resource_id: Option<Id>,
    pub resource_ref: Option<String>,
    pub request_id: Option<Uuid>,
    pub http_status: Option<i32>,
    pub http_method: Option<String>,
    /// Substring (case-insensitive) match on the HTTP path.
    pub http_path_contains: Option<String>,
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// If true, the repository also runs a `COUNT(*)` to populate
    /// [`AuditSearchResult::total`].
    pub include_total: bool,
}

/// Result of [`AuditRepository::search`].
#[derive(Debug)]
pub struct AuditSearchResult {
    pub rows: Vec<AuditEvent>,
    pub total: Option<u64>,
    pub has_next: bool,
}

pub struct AuditRepository;

const SELECT_COLUMNS: &str = "id, created, category, event_type, outcome, \
    actor_identity, actor_login, actor_token_type, host(actor_ip) as actor_ip, actor_user_agent, \
    request_id, resource_type, resource_id, resource_ref, \
    http_method, http_path, http_status, duration_ms, \
    details, correlation_chain";

impl AuditRepository {
    /// Insert one pending audit event immediately.
    ///
    /// Most request-path emitters should use [`crate::audit::AuditEmitter`] so
    /// writes are batched off the hot path. This direct insert is useful for
    /// central infrastructure such as authorization failure auditing where the
    /// caller may not have an emitter handle.
    pub async fn insert<'e, E>(executor: E, event: PendingAuditEvent) -> Result<()>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query(
            "INSERT INTO audit_event (\
                category, event_type, outcome, \
                actor_identity, actor_login, actor_token_type, actor_ip, actor_user_agent, \
                request_id, \
                resource_type, resource_id, resource_ref, \
                http_method, http_path, http_status, duration_ms, \
                details, correlation_chain\
            ) VALUES (\
                $1, $2, $3, \
                $4, $5, $6, $7::inet, $8, \
                $9, \
                $10, $11, $12, \
                $13, $14, $15, $16, \
                $17, $18\
            )",
        )
        .bind(event.category)
        .bind(event.event_type)
        .bind(event.outcome)
        .bind(event.actor_identity)
        .bind(event.actor_login)
        .bind(event.actor_token_type)
        .bind(event.actor_ip.map(|ip| ip.to_string()))
        .bind(event.actor_user_agent)
        .bind(event.request_id)
        .bind(event.resource_type)
        .bind(event.resource_id)
        .bind(event.resource_ref)
        .bind(event.http_method)
        .bind(event.http_path)
        .bind(event.http_status)
        .bind(event.duration_ms)
        .bind(event.details)
        .bind(event.correlation_chain)
        .execute(executor)
        .await?;

        Ok(())
    }

    /// Look up a single audit event by its primary-key ID.
    pub async fn find_by_id<'e, E>(executor: E, id: Id) -> Result<Option<AuditEvent>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sql = format!("SELECT {SELECT_COLUMNS} FROM audit_event WHERE id = $1 LIMIT 1");
        sqlx::query_as::<_, AuditEvent>(&sql)
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(Into::into)
    }

    /// Return all audit events for a single request_id ordered by created.
    pub async fn find_by_request_id<'e, E>(executor: E, request_id: Uuid) -> Result<Vec<AuditEvent>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM audit_event WHERE request_id = $1 ORDER BY created ASC, id ASC"
        );
        sqlx::query_as::<_, AuditEvent>(&sql)
            .bind(request_id)
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Search audit events with optional filters. Results are ordered by
    /// `created DESC, id DESC` and are paginated by `limit`/`offset`.
    pub async fn search<'e, E>(executor: E, filters: &AuditEventFilters) -> Result<Vec<AuditEvent>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT ");
        qb.push(SELECT_COLUMNS);
        qb.push(" FROM audit_event WHERE 1=1");

        Self::push_filter_clauses(&mut qb, filters);

        qb.push(" ORDER BY created DESC, id DESC");
        let limit = filters.limit.unwrap_or(100).clamp(1, 1000);
        let offset = filters.offset.unwrap_or(0).max(0);
        qb.push(" LIMIT ").push_bind(limit);
        qb.push(" OFFSET ").push_bind(offset);

        qb.build_query_as::<AuditEvent>()
            .fetch_all(executor)
            .await
            .map_err(Into::into)
    }

    /// Search audit events and (optionally) return the total matching count
    /// in a single trip to the database.
    pub async fn search_with_meta<'e, E>(
        executor: E,
        filters: &AuditEventFilters,
    ) -> Result<AuditSearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let limit = filters.limit.unwrap_or(50).clamp(1, 1000);
        let offset = filters.offset.unwrap_or(0).max(0);

        // Optional COUNT(*) — only when include_total is true.
        let total = if filters.include_total {
            let mut count_qb: QueryBuilder<Postgres> =
                QueryBuilder::new("SELECT COUNT(*) FROM audit_event WHERE 1=1");
            Self::push_filter_clauses(&mut count_qb, filters);
            let total: i64 = count_qb.build_query_scalar().fetch_one(executor).await?;
            Some(total.max(0) as u64)
        } else {
            None
        };

        // Data query: fetch `limit + 1` when we don't have an exact total so
        // we can derive `has_next` cheaply.
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT ");
        qb.push(SELECT_COLUMNS);
        qb.push(" FROM audit_event WHERE 1=1");
        Self::push_filter_clauses(&mut qb, filters);
        qb.push(" ORDER BY created DESC, id DESC");
        qb.push(" LIMIT ");
        let query_limit = if total.is_some() { limit } else { limit + 1 };
        qb.push_bind(query_limit);
        qb.push(" OFFSET ").push_bind(offset);

        let mut rows: Vec<AuditEvent> = qb
            .build_query_as::<AuditEvent>()
            .fetch_all(executor)
            .await?;

        let has_next = if let Some(total) = total {
            (offset as u64) + (rows.len() as u64) < total
        } else if rows.len() as i64 > limit {
            rows.truncate(limit as usize);
            true
        } else {
            false
        };

        Ok(AuditSearchResult {
            rows,
            total,
            has_next,
        })
    }

    fn push_filter_clauses<'q>(
        qb: &mut QueryBuilder<'q, Postgres>,
        filters: &'q AuditEventFilters,
    ) {
        if let Some(category) = filters.category {
            qb.push(" AND category = ").push_bind(category);
        }
        if let Some(event_type) = &filters.event_type {
            qb.push(" AND event_type = ").push_bind(event_type.clone());
        }
        if let Some(outcome) = filters.outcome {
            qb.push(" AND outcome = ").push_bind(outcome);
        }
        if let Some(actor) = filters.actor_identity {
            qb.push(" AND actor_identity = ").push_bind(actor);
        }
        if let Some(login) = &filters.actor_login_contains {
            qb.push(" AND LOWER(actor_login) LIKE ")
                .push_bind(format!("%{}%", login.to_lowercase()));
        }
        if let Some(rt) = &filters.resource_type {
            qb.push(" AND resource_type = ").push_bind(rt.clone());
        }
        if let Some(rid) = filters.resource_id {
            qb.push(" AND resource_id = ").push_bind(rid);
        }
        if let Some(rref) = &filters.resource_ref {
            qb.push(" AND resource_ref = ").push_bind(rref.clone());
        }
        if let Some(rid) = filters.request_id {
            qb.push(" AND request_id = ").push_bind(rid);
        }
        if let Some(status) = filters.http_status {
            qb.push(" AND http_status = ").push_bind(status);
        }
        if let Some(method) = &filters.http_method {
            qb.push(" AND http_method = ")
                .push_bind(method.to_uppercase());
        }
        if let Some(path) = &filters.http_path_contains {
            qb.push(" AND LOWER(http_path) LIKE ")
                .push_bind(format!("%{}%", path.to_lowercase()));
        }
        if let Some(after) = filters.created_after {
            qb.push(" AND created >= ").push_bind(after);
        }
        if let Some(before) = filters.created_before {
            qb.push(" AND created < ").push_bind(before);
        }
    }
}
