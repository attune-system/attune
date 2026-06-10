-- Migration: Work Queues
-- Description: Creates first-class business work queue tables and enums
-- Version: 20250101000012

-- Set search_path for schema isolation
SET search_path TO attune, public;

-- ============================================================================
-- ENUM TYPES
-- ============================================================================

DO $$ BEGIN
    CREATE TYPE work_queue_update_strategy_enum AS ENUM (
        'immutable',
        'replace',
        'merge_patch'
    );
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

COMMENT ON TYPE work_queue_update_strategy_enum IS
    'How a queue handles pending item updates keyed by item_key';

DO $$ BEGIN
    CREATE TYPE work_queue_batch_mode_enum AS ENUM (
        'single',
        'batch'
    );
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

COMMENT ON TYPE work_queue_batch_mode_enum IS
    'How queue dispatches deliver work items to the target action';

DO $$ BEGIN
    CREATE TYPE work_queue_item_status_enum AS ENUM (
        'queued',
        'leased',
        'retry',
        'completed',
        'failed',
        'skipped',
        'cancelled'
    );
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

COMMENT ON TYPE work_queue_item_status_enum IS
    'Lifecycle status for a durable work queue item';

DO $$ BEGIN
    CREATE TYPE work_queue_dispatch_status_enum AS ENUM (
        'leased',
        'dispatched',
        'completed',
        'failed',
        'released',
        'cancelled'
    );
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

COMMENT ON TYPE work_queue_dispatch_status_enum IS
    'Lifecycle status for a queue dispatch/execution lineage record';

-- ============================================================================
-- WORK_QUEUE TABLE
-- ============================================================================

CREATE TABLE work_queue (
    id BIGSERIAL PRIMARY KEY,
    ref TEXT NOT NULL UNIQUE,
    pack BIGINT REFERENCES pack(id) ON DELETE SET NULL,
    pack_ref TEXT,
    is_adhoc BOOLEAN NOT NULL DEFAULT FALSE,
    label TEXT NOT NULL,
    description TEXT,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    accepting_new_items BOOLEAN NOT NULL DEFAULT TRUE,
    dispatch_action BIGINT REFERENCES action(id) ON DELETE SET NULL,
    dispatch_action_ref TEXT NOT NULL,
    default_priority INTEGER NOT NULL DEFAULT 0,
    allow_pending_update BOOLEAN NOT NULL DEFAULT FALSE,
    update_strategy work_queue_update_strategy_enum NOT NULL DEFAULT 'replace',
    batch_mode work_queue_batch_mode_enum NOT NULL DEFAULT 'single',
    item_schema JSONB NOT NULL DEFAULT '{}'::jsonb,
    action_params JSONB NOT NULL DEFAULT '{}'::jsonb,
    permission_set_refs TEXT[],
    reference_visibility action_reference_visibility_enum NOT NULL DEFAULT 'public',
    reference_allowed_pack_refs TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    config JSONB NOT NULL DEFAULT '{}'::jsonb,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT work_queue_ref_lowercase CHECK (ref = LOWER(ref)),
    CONSTRAINT work_queue_ref_format CHECK (
        ref ~ '^[a-z][a-z0-9_-]*(\.[a-z0-9_-]+)*$'
    ),
    CONSTRAINT work_queue_reference_allowed_pack_refs_non_null
        CHECK (array_position(reference_allowed_pack_refs, NULL) IS NULL),
    CONSTRAINT work_queue_reference_allowed_pack_refs_restricted_only
        CHECK (reference_visibility = 'restricted' OR cardinality(reference_allowed_pack_refs) = 0)
);

CREATE INDEX idx_work_queue_ref ON work_queue(ref);
CREATE INDEX idx_work_queue_pack ON work_queue(pack);
CREATE INDEX idx_work_queue_pack_ref ON work_queue(pack_ref) WHERE pack_ref IS NOT NULL;
CREATE INDEX idx_work_queue_dispatch_action ON work_queue(dispatch_action) WHERE dispatch_action IS NOT NULL;
CREATE INDEX idx_work_queue_enabled ON work_queue(enabled) WHERE enabled = TRUE;
CREATE INDEX idx_work_queue_is_adhoc ON work_queue(is_adhoc);
CREATE INDEX idx_work_queue_created ON work_queue(created DESC);
CREATE INDEX idx_work_queue_config_gin ON work_queue USING GIN (config);
CREATE INDEX idx_work_queue_permission_set_refs ON work_queue USING GIN (permission_set_refs) WHERE permission_set_refs IS NOT NULL;
CREATE INDEX idx_work_queue_reference_visibility ON work_queue(reference_visibility);
CREATE INDEX idx_work_queue_reference_allowed_pack_refs ON work_queue USING GIN (reference_allowed_pack_refs);

CREATE TRIGGER update_work_queue_updated
    BEFORE UPDATE ON work_queue
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

COMMENT ON TABLE work_queue IS
    'First-class business queues for durable user-visible work items';
COMMENT ON COLUMN work_queue.pack IS
    'Optional owning pack for declarative queue definitions';
COMMENT ON COLUMN work_queue.pack_ref IS
    'Owning pack reference cached for deleted-pack survivability and filtering';
COMMENT ON COLUMN work_queue.is_adhoc IS
    'True for API/UI-managed queues, false for pack-owned declarative queues';
COMMENT ON COLUMN work_queue.dispatch_action IS
    'Optional action ID for the dispatch target; NULL preserves queue metadata if the action is deleted';
COMMENT ON COLUMN work_queue.dispatch_action_ref IS
    'Stable action ref used as the dispatch target';
COMMENT ON COLUMN work_queue.enabled IS
    'Whether the executor is allowed to process queued items from this queue';
COMMENT ON COLUMN work_queue.accepting_new_items IS
    'Whether new queue items may be inserted into this queue';
COMMENT ON COLUMN work_queue.default_priority IS
    'Fallback priority applied when producers do not provide one';
COMMENT ON COLUMN work_queue.allow_pending_update IS
    'Whether pending items may be updated or replaced by item_key';
COMMENT ON COLUMN work_queue.update_strategy IS
    'How item_key collisions are handled for mutable pending items';
COMMENT ON COLUMN work_queue.batch_mode IS
    'Whether dispatches deliver one item at a time or batches';
COMMENT ON COLUMN work_queue.item_schema IS
    'Flat trigger-style schema describing the payload shape accepted for queue items. Enforced on queue item enqueue/update writes.';
COMMENT ON COLUMN work_queue.action_params IS
    'Declarative action parameter mappings resolved at dispatch time using queue template expressions';
COMMENT ON COLUMN work_queue.permission_set_refs IS
    'Optional override for execution-scoped API token permission sets. NULL inherits the dispatch action default; empty array forces no token.';
COMMENT ON COLUMN work_queue.reference_visibility IS
    'Pack-level reference visibility: public queues may be targeted by any pack; private queues only by their owning pack; restricted queues by their owning pack and reference_allowed_pack_refs.';
COMMENT ON COLUMN work_queue.reference_allowed_pack_refs IS
    'Allow-list of pack refs that may target this queue when reference_visibility is restricted.';
COMMENT ON COLUMN work_queue.config IS
    'Typed JSON configuration for queue tunables and ack contract';

-- ============================================================================
-- WORK_QUEUE_ITEM TABLE
-- ============================================================================

CREATE TABLE work_queue_item (
    id BIGSERIAL PRIMARY KEY,
    queue BIGINT NOT NULL REFERENCES work_queue(id) ON DELETE CASCADE,
    queue_ref TEXT NOT NULL,
    item_key TEXT,
    priority INTEGER NOT NULL DEFAULT 0,
    status work_queue_item_status_enum NOT NULL DEFAULT 'queued',
    payload JSONB NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    enqueue_source TEXT NOT NULL,
    requested_by_identity BIGINT REFERENCES identity(id) ON DELETE SET NULL,
    requested_by_execution BIGINT,
    requested_by_enforcement BIGINT,
    leased_execution BIGINT,
    lease_token UUID,
    lease_expires_at TIMESTAMPTZ,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    last_error JSONB,
    ack_summary JSONB,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT work_queue_item_attempt_count_nonnegative CHECK (attempt_count >= 0)
);

CREATE INDEX idx_work_queue_item_queue_status_order
    ON work_queue_item (queue, status, priority DESC, created ASC, id ASC);
CREATE INDEX idx_work_queue_item_queue_ready_order
    ON work_queue_item (queue, priority DESC, created ASC, id ASC)
    WHERE status IN ('queued', 'retry');
CREATE INDEX idx_work_queue_item_queue_item_key
    ON work_queue_item (queue, item_key)
    WHERE item_key IS NOT NULL;
CREATE INDEX idx_work_queue_item_leased_execution
    ON work_queue_item (leased_execution)
    WHERE leased_execution IS NOT NULL;
CREATE INDEX idx_work_queue_item_lease_token
    ON work_queue_item (lease_token)
    WHERE lease_token IS NOT NULL;
CREATE INDEX idx_work_queue_item_queue_lease_expires
    ON work_queue_item (queue, lease_expires_at)
    WHERE lease_expires_at IS NOT NULL;
CREATE INDEX idx_work_queue_item_requested_by_identity
    ON work_queue_item (requested_by_identity)
    WHERE requested_by_identity IS NOT NULL;
CREATE INDEX idx_work_queue_item_created ON work_queue_item(created DESC);
CREATE INDEX idx_work_queue_item_payload_gin ON work_queue_item USING GIN (payload);
CREATE INDEX idx_work_queue_item_metadata_gin ON work_queue_item USING GIN (metadata);

CREATE TRIGGER update_work_queue_item_updated
    BEFORE UPDATE ON work_queue_item
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

COMMENT ON TABLE work_queue_item IS
    'Durable business records waiting to be dispatched from a work queue';
COMMENT ON COLUMN work_queue_item.queue_ref IS
    'Cached queue ref for filtering and lineage even if the queue ref changes later';
COMMENT ON COLUMN work_queue_item.item_key IS
    'Logical item identifier for update aggregation or deduplication';
COMMENT ON COLUMN work_queue_item.enqueue_source IS
    'Producer metadata such as api, execution, workflow, rule, or system';
COMMENT ON COLUMN work_queue_item.requested_by_execution IS
    'Initiating execution ID when a run enqueues work (no FK because execution is a hypertable)';
COMMENT ON COLUMN work_queue_item.requested_by_enforcement IS
    'Initiating enforcement ID when a rule enqueues work (no FK because enforcement is a hypertable)';
COMMENT ON COLUMN work_queue_item.leased_execution IS
    'Execution currently assigned to process this item, if known';
COMMENT ON COLUMN work_queue_item.lease_token IS
    'Opaque lease token for reliable batch leasing and release';
COMMENT ON COLUMN work_queue_item.lease_expires_at IS
    'Lease expiration timestamp used for crash recovery and reconciliation';
COMMENT ON COLUMN work_queue_item.last_error IS
    'Most recent retry/failure details recorded for the item';
COMMENT ON COLUMN work_queue_item.ack_summary IS
    'Final acknowledgement payload applied after processing completes';

-- ============================================================================
-- WORK_QUEUE_DISPATCH TABLE
-- ============================================================================

CREATE TABLE work_queue_dispatch (
    id BIGSERIAL PRIMARY KEY,
    queue BIGINT NOT NULL REFERENCES work_queue(id) ON DELETE CASCADE,
    queue_ref TEXT NOT NULL,
    execution BIGINT NOT NULL UNIQUE,
    status work_queue_dispatch_status_enum NOT NULL DEFAULT 'leased',
    leased_item_count INTEGER NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT work_queue_dispatch_leased_item_count_nonnegative CHECK (leased_item_count >= 0)
);

CREATE INDEX idx_work_queue_dispatch_queue_status
    ON work_queue_dispatch (queue, status, created DESC);
CREATE INDEX idx_work_queue_dispatch_queue_ref
    ON work_queue_dispatch (queue_ref);
CREATE INDEX idx_work_queue_dispatch_created
    ON work_queue_dispatch (created DESC);

CREATE TRIGGER update_work_queue_dispatch_updated
    BEFORE UPDATE ON work_queue_dispatch
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

COMMENT ON TABLE work_queue_dispatch IS
    'Lineage record linking a work queue release to the execution processing it';
COMMENT ON COLUMN work_queue_dispatch.execution IS
    'Execution ID consuming the leased queue items (no FK because execution is a hypertable)';
COMMENT ON COLUMN work_queue_dispatch.leased_item_count IS
    'Number of queue items leased into the dispatch batch';

-- ============================================================================
-- WORK QUEUE NOTIFICATIONS
-- ============================================================================

CREATE OR REPLACE FUNCTION notify_work_queue_created()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
BEGIN
    payload := json_build_object(
        'entity_type', 'work_queue',
        'entity_id', NEW.id,
        'id', NEW.id,
        'ref', NEW.ref,
        'pack_ref', NEW.pack_ref,
        'is_adhoc', NEW.is_adhoc,
        'label', NEW.label,
        'enabled', NEW.enabled,
        'dispatch_action_ref', NEW.dispatch_action_ref,
        'default_priority', NEW.default_priority,
        'allow_pending_update', NEW.allow_pending_update,
        'update_strategy', NEW.update_strategy,
        'batch_mode', NEW.batch_mode,
        'created', NEW.created,
        'updated', NEW.updated
    );

    PERFORM pg_notify('work_queue_created', payload::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_work_queue_updated()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        payload := json_build_object(
            'entity_type', 'work_queue',
            'entity_id', NEW.id,
            'id', NEW.id,
            'ref', NEW.ref,
            'pack_ref', NEW.pack_ref,
            'is_adhoc', NEW.is_adhoc,
            'label', NEW.label,
            'enabled', NEW.enabled,
            'dispatch_action_ref', NEW.dispatch_action_ref,
            'default_priority', NEW.default_priority,
            'allow_pending_update', NEW.allow_pending_update,
            'update_strategy', NEW.update_strategy,
            'batch_mode', NEW.batch_mode,
            'created', NEW.created,
            'updated', NEW.updated
        );

        PERFORM pg_notify('work_queue_updated', payload::text);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER work_queue_created_notify
    AFTER INSERT ON work_queue
    FOR EACH ROW
    EXECUTE FUNCTION notify_work_queue_created();

CREATE TRIGGER work_queue_updated_notify
    AFTER UPDATE ON work_queue
    FOR EACH ROW
    EXECUTE FUNCTION notify_work_queue_updated();

COMMENT ON FUNCTION notify_work_queue_created() IS 'Sends work queue creation notifications via PostgreSQL LISTEN/NOTIFY';
COMMENT ON FUNCTION notify_work_queue_updated() IS 'Sends work queue update notifications via PostgreSQL LISTEN/NOTIFY';

-- ============================================================================
-- WORK QUEUE ITEM NOTIFICATIONS
-- ============================================================================

CREATE OR REPLACE FUNCTION notify_work_queue_item_created()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
BEGIN
    payload := json_build_object(
        'entity_type', 'work_queue_item',
        'entity_id', NEW.id,
        'id', NEW.id,
        'queue', NEW.queue,
        'queue_ref', NEW.queue_ref,
        'item_key', NEW.item_key,
        'priority', NEW.priority,
        'status', NEW.status,
        'enqueue_source', NEW.enqueue_source,
        'requested_by_identity', NEW.requested_by_identity,
        'requested_by_execution', NEW.requested_by_execution,
        'requested_by_enforcement', NEW.requested_by_enforcement,
        'leased_execution', NEW.leased_execution,
        'lease_expires_at', NEW.lease_expires_at,
        'attempt_count', NEW.attempt_count,
        'created', NEW.created,
        'updated', NEW.updated
    );

    PERFORM pg_notify('work_queue_item_created', payload::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION notify_work_queue_item_updated()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        payload := json_build_object(
            'entity_type', 'work_queue_item',
            'entity_id', NEW.id,
            'id', NEW.id,
            'queue', NEW.queue,
            'queue_ref', NEW.queue_ref,
            'item_key', NEW.item_key,
            'priority', NEW.priority,
            'status', NEW.status,
            'old_status', OLD.status,
            'enqueue_source', NEW.enqueue_source,
            'requested_by_identity', NEW.requested_by_identity,
            'requested_by_execution', NEW.requested_by_execution,
            'requested_by_enforcement', NEW.requested_by_enforcement,
            'leased_execution', NEW.leased_execution,
            'lease_expires_at', NEW.lease_expires_at,
            'attempt_count', NEW.attempt_count,
            'created', NEW.created,
            'updated', NEW.updated
        );

        PERFORM pg_notify('work_queue_item_updated', payload::text);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER work_queue_item_created_notify
    AFTER INSERT ON work_queue_item
    FOR EACH ROW
    EXECUTE FUNCTION notify_work_queue_item_created();

CREATE TRIGGER work_queue_item_updated_notify
    AFTER UPDATE ON work_queue_item
    FOR EACH ROW
    EXECUTE FUNCTION notify_work_queue_item_updated();

COMMENT ON FUNCTION notify_work_queue_item_created() IS 'Sends work queue item creation notifications via PostgreSQL LISTEN/NOTIFY';
COMMENT ON FUNCTION notify_work_queue_item_updated() IS 'Sends work queue item update notifications via PostgreSQL LISTEN/NOTIFY';
