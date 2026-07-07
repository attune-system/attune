//! Inquiry repository for database operations

use crate::models::{enums::InquiryStatus, inquiry::*, Id, JsonDict, JsonSchema};
use crate::rbac::{Action, ExecutionScopeConstraint, Grant, OwnerConstraint, Resource};
use crate::Result;
use chrono::{DateTime, Utc};
use sqlx::{Executor, Postgres, QueryBuilder};
use std::collections::HashMap;

use super::{Create, Delete, FindById, List, Repository, Update};

/// Filters for [`InquiryRepository::search`].
///
/// All fields are optional and combinable (AND). Pagination is always applied.
#[derive(Debug, Clone, Default)]
pub struct InquirySearchFilters {
    pub status: Option<InquiryStatus>,
    pub execution: Option<Id>,
    pub assigned_to: Option<Id>,
    pub limit: u32,
    pub offset: u32,
}

/// Result of [`InquiryRepository::search`].
#[derive(Debug)]
pub struct InquirySearchResult {
    pub rows: Vec<Inquiry>,
    pub total: u64,
}

/// Context needed to translate an identity's effective RBAC grants into a
/// SQL-side visibility predicate for [`InquiryRepository::search_visible`].
///
/// Mirrors the fields the API layer's inquiry visibility evaluator assembles
/// once per request: the caller's identity, its attributes (for
/// `constraints.attributes` matching), and its effective grants.
#[derive(Debug, Clone)]
pub struct InquiryVisibilityContext {
    pub identity_id: Id,
    pub identity_attributes: HashMap<String, serde_json::Value>,
    pub grants: Vec<Grant>,
}

impl InquiryVisibilityContext {
    /// True when the identity holds an unconstrained `inquiries:read` grant.
    /// In that case every inquiry is content-visible and the per-row
    /// participant/scope-reader predicate can be skipped entirely.
    pub fn has_global_inquiry_read(&self) -> bool {
        self.grants.iter().any(|grant| {
            grant.resource == Resource::Inquiries
                && grant.actions.contains(&Action::Read)
                && grant.constraints.is_none()
                && grant_attributes_match(grant, &self.identity_attributes)
        })
    }
}

pub struct InquiryRepository;

impl Repository for InquiryRepository {
    type Entity = Inquiry;
    fn table_name() -> &'static str {
        "inquiry"
    }
}

#[derive(Debug, Clone)]
pub struct CreateInquiryInput {
    pub execution: Id,
    pub prompt: String,
    pub response_schema: Option<JsonSchema>,
    pub assigned_to: Option<Id>,
    pub status: InquiryStatus,
    pub response: Option<JsonDict>,
    pub timeout_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateInquiryInput {
    pub status: Option<InquiryStatus>,
    pub response: Option<JsonDict>,
    pub responded_at: Option<DateTime<Utc>>,
    pub assigned_to: Option<Id>,
}

#[async_trait::async_trait]
impl FindById for InquiryRepository {
    async fn find_by_id<'e, E>(executor: E, id: i64) -> Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Inquiry>(
            "SELECT id, execution, prompt, response_schema, assigned_to, status, response, timeout_at, responded_at, created, updated FROM inquiry WHERE id = $1"
        ).bind(id).fetch_optional(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl List for InquiryRepository {
    async fn list<'e, E>(executor: E) -> Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Inquiry>(
            "SELECT id, execution, prompt, response_schema, assigned_to, status, response, timeout_at, responded_at, created, updated FROM inquiry ORDER BY created DESC LIMIT 1000"
        ).fetch_all(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Create for InquiryRepository {
    type CreateInput = CreateInquiryInput;
    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Inquiry>(
            "INSERT INTO inquiry (execution, prompt, response_schema, assigned_to, status, response, timeout_at) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id, execution, prompt, response_schema, assigned_to, status, response, timeout_at, responded_at, created, updated"
        ).bind(input.execution).bind(&input.prompt).bind(&input.response_schema).bind(input.assigned_to).bind(input.status).bind(&input.response).bind(input.timeout_at).fetch_one(executor).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Update for InquiryRepository {
    type UpdateInput = UpdateInquiryInput;
    async fn update<'e, E>(executor: E, id: i64, input: Self::UpdateInput) -> Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        // Build update query
        let mut query = QueryBuilder::new("UPDATE inquiry SET ");
        let mut has_updates = false;

        if let Some(status) = input.status {
            query.push("status = ").push_bind(status);
            has_updates = true;
        }
        if let Some(response) = &input.response {
            if has_updates {
                query.push(", ");
            }
            query.push("response = ").push_bind(response);
            has_updates = true;
        }
        if let Some(responded_at) = input.responded_at {
            if has_updates {
                query.push(", ");
            }
            query.push("responded_at = ").push_bind(responded_at);
            has_updates = true;
        }
        if let Some(assigned_to) = input.assigned_to {
            if has_updates {
                query.push(", ");
            }
            query.push("assigned_to = ").push_bind(assigned_to);
            has_updates = true;
        }

        if !has_updates {
            // No updates requested, fetch and return existing entity
            return Self::get_by_id(executor, id).await;
        }

        query.push(", updated = NOW() WHERE id = ").push_bind(id);
        query.push(" RETURNING id, execution, prompt, response_schema, assigned_to, status, response, timeout_at, responded_at, created, updated");

        query
            .build_query_as::<Inquiry>()
            .fetch_one(executor)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl Delete for InquiryRepository {
    async fn delete<'e, E>(executor: E, id: i64) -> Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let result = sqlx::query("DELETE FROM inquiry WHERE id = $1")
            .bind(id)
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl InquiryRepository {
    /// Atomically mark all expired pending inquiries as timed out.
    pub async fn timeout_expired_pending<'e, E>(executor: E) -> Result<Vec<Inquiry>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Inquiry>(
            "UPDATE inquiry \
             SET status = $1, updated = NOW() \
             WHERE status = $2 \
               AND timeout_at IS NOT NULL \
               AND timeout_at <= NOW() \
             RETURNING id, execution, prompt, response_schema, assigned_to, status, response, timeout_at, responded_at, created, updated",
        )
        .bind(InquiryStatus::Timeout)
        .bind(InquiryStatus::Pending)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn find_by_status<'e, E>(executor: E, status: InquiryStatus) -> Result<Vec<Inquiry>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Inquiry>(
            "SELECT id, execution, prompt, response_schema, assigned_to, status, response, timeout_at, responded_at, created, updated FROM inquiry WHERE status = $1 ORDER BY created DESC"
        ).bind(status).fetch_all(executor).await.map_err(Into::into)
    }

    pub async fn find_by_execution<'e, E>(executor: E, execution_id: Id) -> Result<Vec<Inquiry>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        sqlx::query_as::<_, Inquiry>(
            "SELECT id, execution, prompt, response_schema, assigned_to, status, response, timeout_at, responded_at, created, updated FROM inquiry WHERE execution = $1 ORDER BY created DESC"
        ).bind(execution_id).fetch_all(executor).await.map_err(Into::into)
    }

    /// Search inquiries with all filters pushed into SQL.
    ///
    /// All filter fields are combinable (AND). Pagination is server-side.
    pub async fn search<'e, E>(db: E, filters: &InquirySearchFilters) -> Result<InquirySearchResult>
    where
        E: Executor<'e, Database = Postgres> + Copy + 'e,
    {
        let select_cols = "id, execution, prompt, response_schema, assigned_to, status, response, timeout_at, responded_at, created, updated";

        let mut qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new(format!("SELECT {select_cols} FROM inquiry"));
        let mut count_qb: QueryBuilder<'_, Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM inquiry");

        let mut has_where = false;

        macro_rules! push_condition {
            ($cond_prefix:expr, $value:expr) => {{
                if !has_where {
                    qb.push(" WHERE ");
                    count_qb.push(" WHERE ");
                    has_where = true;
                } else {
                    qb.push(" AND ");
                    count_qb.push(" AND ");
                }
                qb.push($cond_prefix);
                qb.push_bind($value.clone());
                count_qb.push($cond_prefix);
                count_qb.push_bind($value);
            }};
        }

        if let Some(status) = &filters.status {
            push_condition!("status = ", *status);
        }
        if let Some(execution_id) = filters.execution {
            push_condition!("execution = ", execution_id);
        }
        if let Some(assigned_to) = filters.assigned_to {
            push_condition!("assigned_to = ", assigned_to);
        }

        // Suppress unused-assignment warning from the macro's last expansion.
        let _ = has_where;

        // Count
        let total: i64 = count_qb.build_query_scalar().fetch_one(db).await?;
        let total = total.max(0) as u64;

        // Data query
        qb.push(" ORDER BY created DESC");
        qb.push(" LIMIT ");
        qb.push_bind(filters.limit as i64);
        qb.push(" OFFSET ");
        qb.push_bind(filters.offset as i64);

        let rows: Vec<Inquiry> = qb.build_query_as().fetch_all(db).await?;

        Ok(InquirySearchResult { rows, total })
    }

    /// Search inquiries with content-visibility filtering applied in SQL.
    ///
    /// Replicates the participant/scope-reader semantics previously
    /// evaluated per-row in the API layer (see `InquiryVisibilityEvaluator`):
    /// a row is included when the caller is the assignee, the executor of
    /// the linked execution, or holds an `inquiries:read` grant whose
    /// constraints match (pack scope, ownership, execution scope, explicit
    /// refs/ids). Rows whose execution link is dangling or otherwise
    /// unreadable are never excluded on that basis alone — only the
    /// `execution` field is redacted later, by the caller, for the returned
    /// page.
    ///
    /// `filters.limit`/`filters.offset` are applied *after* the visibility
    /// predicate, so pagination is consistent with what the caller may
    /// actually see. No per-row database round trip is made: the whole page
    /// (plus one extra row used by callers to detect `has_next`) is fetched
    /// with a single query.
    pub async fn search_visible<'e, E>(
        db: E,
        filters: &InquirySearchFilters,
        ctx: &InquiryVisibilityContext,
    ) -> Result<Vec<Inquiry>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let select_cols = "i.id, i.execution, i.prompt, i.response_schema, i.assigned_to, i.status, i.response, i.timeout_at, i.responded_at, i.created, i.updated";

        let mut qb: QueryBuilder<'_, Postgres> = QueryBuilder::new(format!(
            "SELECT {select_cols} FROM inquiry i LEFT JOIN execution e ON e.id = i.execution"
        ));

        let mut has_where = false;
        push_inquiry_base_filters(&mut qb, filters, &mut has_where);

        push_where_prefix(&mut qb, &mut has_where);
        if ctx.has_global_inquiry_read() {
            qb.push("TRUE");
        } else {
            push_inquiry_content_visible_predicate(&mut qb, ctx);
        }

        qb.push(" ORDER BY i.created DESC");
        qb.push(" LIMIT ");
        qb.push_bind(filters.limit as i64);
        qb.push(" OFFSET ");
        qb.push_bind(filters.offset as i64);

        qb.build_query_as::<Inquiry>()
            .fetch_all(db)
            .await
            .map_err(Into::into)
    }
}

fn grant_attributes_match(
    grant: &Grant,
    identity_attributes: &HashMap<String, serde_json::Value>,
) -> bool {
    let Some(constraints) = &grant.constraints else {
        return true;
    };
    let Some(attributes) = &constraints.attributes else {
        return true;
    };
    attributes
        .iter()
        .all(|(key, expected)| identity_attributes.get(key) == Some(expected))
}

fn push_where_prefix<'a>(query: &mut QueryBuilder<'a, Postgres>, has_where: &mut bool) {
    if *has_where {
        query.push(" AND ");
    } else {
        query.push(" WHERE ");
        *has_where = true;
    }
}

fn push_inquiry_base_filters<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    filters: &InquirySearchFilters,
    has_where: &mut bool,
) {
    if let Some(status) = filters.status {
        push_where_prefix(query, has_where);
        query.push("i.status = ").push_bind(status);
    }
    if let Some(execution_id) = filters.execution {
        push_where_prefix(query, has_where);
        query.push("i.execution = ").push_bind(execution_id);
    }
    if let Some(assigned_to) = filters.assigned_to {
        push_where_prefix(query, has_where);
        query.push("i.assigned_to = ").push_bind(assigned_to);
    }
}

/// A grant can only ever match the inquiry scope-reader context if it does
/// not depend on fields that context never populates (`owner_types`,
/// `owner_refs`, `visibility`, `encrypted` all mirror
/// `AuthorizationContext` fields that `inquiry_readable_with_scope` leaves
/// `None`, so any such constraint is permanently unsatisfiable there).
fn can_apply_inquiry_grant(grant: &Grant) -> bool {
    let Some(constraints) = &grant.constraints else {
        return true;
    };
    constraints.owner_types.is_none()
        && constraints.owner_refs.is_none()
        && constraints.visibility.is_none()
        && constraints.encrypted.is_none()
}

/// Builds `(participant OR scope_reader)` — the SQL equivalent of
/// `InquiryVisibilityEvaluator::evaluate`'s `content_visible` computation —
/// against the `inquiry i LEFT JOIN execution e` aliases established by
/// `search_visible`.
fn push_inquiry_content_visible_predicate<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    ctx: &InquiryVisibilityContext,
) {
    query.push("(");

    // Participant: assignee, or the executor of the linked execution. A
    // dangling/missing execution link simply drops the executor clause
    // rather than denying the assignee.
    query.push("(i.assigned_to = ");
    query.push_bind(ctx.identity_id);
    query.push(" OR e.executor = ");
    query.push_bind(ctx.identity_id);
    query.push(")");

    query.push(" OR ");

    // Scope reader: any applicable `inquiries:read` grant.
    let grants: Vec<&Grant> = ctx
        .grants
        .iter()
        .filter(|grant| {
            grant.resource == Resource::Inquiries
                && grant.actions.contains(&Action::Read)
                && grant_attributes_match(grant, &ctx.identity_attributes)
                && can_apply_inquiry_grant(grant)
        })
        .collect();

    query.push("(");
    let mut wrote_any = false;
    for grant in grants {
        if wrote_any {
            query.push(" OR ");
        }
        push_single_inquiry_grant_predicate(query, grant, ctx);
        wrote_any = true;
    }
    if !wrote_any {
        query.push("FALSE");
    }
    query.push(")");

    query.push(")");
}

fn push_single_inquiry_grant_predicate<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    grant: &Grant,
    ctx: &InquiryVisibilityContext,
) {
    let Some(constraints) = &grant.constraints else {
        query.push("TRUE");
        return;
    };

    query.push("(");
    let mut has_term = false;
    let push_and = |query: &mut QueryBuilder<'a, Postgres>, has_term: &mut bool| {
        if *has_term {
            query.push(" AND ");
        } else {
            *has_term = true;
        }
    };

    if let Some(pack_refs) = &constraints.pack_refs {
        if pack_refs.is_empty() {
            query.push("FALSE)");
            return;
        }
        push_and(query, &mut has_term);
        query.push("(e.id IS NOT NULL AND split_part(e.action_ref, '.', 1) = ANY(");
        query.push_bind(pack_refs.clone());
        query.push("))");
    }

    if let Some(owner) = constraints.owner {
        push_and(query, &mut has_term);
        match owner {
            OwnerConstraint::SelfOnly => {
                query.push("(e.executor = ");
                query.push_bind(ctx.identity_id);
                query.push(")");
            }
            OwnerConstraint::Any => {
                query.push("TRUE");
            }
            OwnerConstraint::None => {
                query.push("(e.id IS NULL OR e.executor IS NULL)");
            }
        }
    }

    if let Some(scope) = constraints.execution_scope {
        push_and(query, &mut has_term);
        match scope {
            // Note: unlike execution-visibility checks, the inquiry
            // scope-reader context never populates ancestor identities, so
            // `Descendants` collapses to the same predicate as `SelfOnly`
            // here — matching `inquiry_readable_with_scope`'s behavior.
            ExecutionScopeConstraint::SelfOnly | ExecutionScopeConstraint::Descendants => {
                query.push("(e.executor = ");
                query.push_bind(ctx.identity_id);
                query.push(")");
            }
            ExecutionScopeConstraint::Any => {
                query.push("TRUE");
            }
        }
    }

    if let Some(refs) = &constraints.refs {
        if refs.is_empty() {
            query.push("FALSE)");
            return;
        }
        push_and(query, &mut has_term);
        query.push("(('inquiry:' || i.id::text) = ANY(");
        query.push_bind(refs.clone());
        query.push("))");
    }

    if let Some(ids) = &constraints.ids {
        if ids.is_empty() {
            query.push("FALSE)");
            return;
        }
        push_and(query, &mut has_term);
        query.push("(i.id = ANY(");
        query.push_bind(ids.clone());
        query.push("))");
    }

    if !has_term {
        query.push("TRUE");
    }
    query.push(")");
}
