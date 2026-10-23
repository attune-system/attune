//! Bounded metadata effects. Payload editing/admission is deliberately separate.
use super::*;
use crate::repositories::{runtime_read::RuntimeReadRepository, work_queue::WorkQueueRepository};

pub const MAX_METADATA_BULK_ITEMS: usize = 1_000;

#[derive(Clone, Copy)]
pub enum MetadataOperation {
    Cancel,
    Reprioritize(i32),
}

impl MetadataOperation {
    fn action(self) -> Action {
        match self {
            Self::Cancel => Action::Delete,
            Self::Reprioritize(_) => Action::Update,
        }
    }
}

pub struct MetadataBulkResult {
    pub matched_count: u64,
    pub preview: Vec<WorkQueueItem>,
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct Candidate {
    id: i64,
    fingerprint: String,
    operation_allowed: bool,
}

async fn candidates(
    conn: &mut PgConnection,
    read: &CollectionFilter<'_>,
    operation: &CollectionFilter<'_>,
    queue: i64,
    selector: &WorkQueueItemJsonPathSelector,
) -> Result<Vec<Candidate>> {
    let filters = WorkQueueItemSearchFilters {
        queue: Some(queue),
        ..Default::default()
    };
    let mut query = QueryBuilder::new("");
    visible_selector_cte(&mut query, read, &filters, Some(operation));
    query.push("SELECT id, encode(sha256(convert_to(selector_document::text,'UTF8')),'hex') AS fingerprint, COALESCE(operation_allowed,FALSE) AS operation_allowed FROM visible_item WHERE ");
    selector_predicate(&mut query, selector);
    query
        .push(" ORDER BY priority DESC, created ASC, id ASC LIMIT ")
        .push_bind((MAX_METADATA_BULK_ITEMS + 1) as i64);
    let selected: Vec<Candidate> = query.build_query_as().fetch_all(&mut *conn).await?;
    if selected.len() > MAX_METADATA_BULK_ITEMS {
        return Err(Error::validation(
            "Recorded bulk selection exceeds 1000 readable items; narrow the selector",
        ));
    }
    Ok(selected)
}

fn exact_targets(
    query: &mut QueryBuilder<'_, Postgres>,
    queue: i64,
    ids: &[i64],
    read: &CollectionFilter<'_>,
    operation: &CollectionFilter<'_>,
) {
    query
        .push(" WHERE queue = ")
        .push_bind(queue)
        .push(" AND id = ANY(")
        .push_bind(ids.to_vec())
        .push(") AND ");
    read.push(query);
    query.push(" AND ");
    operation.push(query);
}

/// Source validation must already have completed on this same connection.
/// Capture -> queue SHARE -> sorted item UPDATE; never reenter graph/source
/// admission or the point resolver behind these target locks. The SQL compiler
/// matches the same typed ownership/pin context against the protected rows.
pub async fn apply_metadata(
    conn: &mut PgConnection,
    authority: &ValidatedRuntimeAuthority,
    expected: &WorkQueue,
    selector: &WorkQueueItemJsonPathSelector,
    operation: MetadataOperation,
    preview_limit: u32,
) -> Result<MetadataBulkResult> {
    let read = authority.queue_item_collection();
    let effect = CollectionFilter {
        authority,
        resource: Resource::QueueItems,
        action: operation.action(),
    };
    read.require(Resource::QueueItems)?;
    if !read.has_operation() || !effect.has_operation() {
        return Err(Error::PermissionDenied(
            "Bulk items require independent Read and operation authority".into(),
        ));
    }
    let admission = authority.credential_binding().admission_id;
    match operation {
        // A fixed cancellation state is not a data sink. Do not write caller
        // strings, selectors or variable names into audit records on this path.
        MetadataOperation::Cancel => RuntimeReadRepository::lock_open(conn, admission).await?,
        // Priority is caller-controlled persisted data, so it retains C's sink
        // guard until durable field-level provenance exists for that value.
        MetadataOperation::Reprioritize(_) => {
            RuntimeReadRepository::require_untainted(conn, admission).await?
        }
    }
    authority.ensure_fresh_on(conn).await?;
    let current = WorkQueueRepository::find_by_id_for_share(conn, expected.id)
        .await?
        .ok_or_else(|| Error::invalid_state("Bulk queue disappeared; retry"))?;
    if current.pack != expected.pack
        || current.is_adhoc != expected.is_adhoc
        || current.r#ref != expected.r#ref
        || current.pack_ref != expected.pack_ref
        || current.updated != expected.updated
        || current.reference_visibility != expected.reference_visibility
        || current.reference_allowed_pack_refs != expected.reference_allowed_pack_refs
    {
        return Err(Error::invalid_state("Bulk queue changed; retry"));
    }
    let selected = candidates(conn, &read, &effect, current.id, selector).await?;
    authority.ensure_fresh_on(conn).await?;
    if selected
        .iter()
        .any(|candidate| !candidate.operation_allowed)
    {
        return Err(Error::PermissionDenied(
            "Selected item operation denied".into(),
        ));
    }
    let mut ids = selected
        .iter()
        .map(|candidate| candidate.id)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    let locked: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM work_queue_item WHERE queue = $1 AND id = ANY($2) ORDER BY id FOR UPDATE",
    )
    .bind(current.id)
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    authority.ensure_fresh_on(conn).await?;
    if locked != ids {
        return Err(Error::invalid_state("Selected item disappeared; retry"));
    }
    // Repeat the same visible selection after every target wait. Changed public
    // payload/priority/status, new matches, or changed authorization deny the
    // entire batch rather than applying an old selection to fresh row contents.
    let fresh = candidates(conn, &read, &effect, current.id, selector).await?;
    if fresh != selected {
        return Err(Error::invalid_state(
            "Bulk selection changed while locking items; retry",
        ));
    }
    let mut preview = QueryBuilder::new(format!(
        "SELECT {} FROM work_queue_item",
        item_metadata_columns()
    ));
    exact_targets(&mut preview, current.id, &ids, &read, &effect);
    preview
        .push(" ORDER BY priority DESC, created ASC, id ASC LIMIT ")
        .push_bind(i64::from(preview_limit.min(100)));
    let preview = preview.build_query_as().fetch_all(&mut *conn).await?;
    authority.ensure_fresh_on(conn).await?;
    let mut update = QueryBuilder::new("UPDATE work_queue_item SET ");
    match operation {
        MetadataOperation::Cancel => {
            update.push("status = 'cancelled'::work_queue_item_status_enum");
        }
        MetadataOperation::Reprioritize(priority) => {
            update.push("priority = ").push_bind(priority);
        }
    }
    update.push(", updated = NOW()");
    exact_targets(&mut update, current.id, &ids, &read, &effect);
    update
        .push(" AND status = ANY(")
        .push_bind(WorkQueueItemRepository::mutable_pending_statuses())
        .push(") RETURNING id");
    let mut changed: Vec<i64> = update.build_query_scalar().fetch_all(&mut *conn).await?;
    changed.sort_unstable();
    if changed != ids {
        return Err(Error::invalid_state(
            "Bulk effect target set changed; retry",
        ));
    }
    authority.ensure_fresh_on(conn).await?;
    Ok(MetadataBulkResult {
        matched_count: ids.len() as u64,
        preview,
    })
}
