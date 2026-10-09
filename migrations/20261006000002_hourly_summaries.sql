SET search_path TO attune, public;

CREATE TYPE native_summary_kind AS ENUM ('execution_status', 'execution_creation', 'event_volume', 'worker_status');
CREATE TABLE native_summary_state (
    kind native_summary_kind PRIMARY KEY,
    updated TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
INSERT INTO native_summary_state(kind)
VALUES ('execution_status'), ('execution_creation'), ('event_volume'), ('worker_status');

CREATE TABLE native_summary_hour (
    kind native_summary_kind NOT NULL,
    bucket TIMESTAMPTZ NOT NULL CHECK (bucket = date_trunc('hour', bucket, 'UTC')),
    refreshed_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (kind, bucket)
);
-- No FK to the builder state. Producers must not wait on its FOR UPDATE lock.
CREATE TABLE native_summary_invalidation (
    id BIGSERIAL PRIMARY KEY,
    kind native_summary_kind NOT NULL,
    bucket TIMESTAMPTZ NOT NULL CHECK (bucket = date_trunc('hour', bucket, 'UTC')),
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    -- Full top-level transaction ID. Imported origin values are not ownership
    -- evidence: producers additionally verify the marker's actual system xmin.
    transaction_origin xid8 NOT NULL DEFAULT pg_current_xact_id()
);
CREATE INDEX idx_native_summary_invalidation_bucket ON native_summary_invalidation(kind, bucket, id);
CREATE INDEX idx_native_summary_invalidation_created ON native_summary_invalidation(created);
CREATE INDEX idx_native_summary_invalidation_origin ON native_summary_invalidation(kind, bucket, transaction_origin);

-- Keep existing source writer grants usable after installing invoker triggers.
-- Include column-level grants and execution/worker writers whose history triggers
-- append to the history parents. No builder-state privilege is given to producers.
DO $$
DECLARE writer RECORD; role_name TEXT;
BEGIN
    FOR writer IN
        SELECT DISTINCT grantee FROM (
            SELECT x.grantee FROM pg_class c
            CROSS JOIN LATERAL aclexplode(COALESCE(c.relacl, acldefault('r', c.relowner))) x
            WHERE c.oid IN ('event'::regclass, 'execution_history'::regclass, 'worker_history'::regclass, 'execution'::regclass, 'worker'::regclass)
              AND x.privilege_type IN ('INSERT', 'UPDATE', 'DELETE')
            UNION
            SELECT x.grantee FROM pg_attribute a CROSS JOIN LATERAL aclexplode(a.attacl) x
            WHERE a.attrelid IN ('event'::regclass, 'execution_history'::regclass, 'worker_history'::regclass, 'execution'::regclass, 'worker'::regclass)
              AND x.privilege_type IN ('INSERT', 'UPDATE')
        ) writers
    LOOP
        role_name := CASE WHEN writer.grantee = 0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(writer.grantee)) END;
        EXECUTE format('GRANT INSERT ON native_summary_invalidation TO %s', role_name);
        -- PG16/18 permit SELECT(xmin), including the system attribute. Give
        -- only ownership-predicate metadata, not ID/created or builder state.
        -- This is an internal service-DB grant, not an API permission change.
        EXECUTE format('GRANT SELECT (kind, bucket, transaction_origin, xmin) ON native_summary_invalidation TO %s', role_name);
        EXECUTE format('GRANT USAGE ON SEQUENCE native_summary_invalidation_id_seq TO %s', role_name);
    END LOOP;
END $$;

CREATE TABLE execution_status_hourly_summary (
    bucket TIMESTAMPTZ NOT NULL CHECK (bucket = date_trunc('hour', bucket, 'UTC')),
    action_ref TEXT,
    new_status TEXT,
    transition_count BIGINT NOT NULL CHECK (transition_count >= 0),
    UNIQUE NULLS NOT DISTINCT (bucket, action_ref, new_status)
);
CREATE TABLE execution_creation_hourly_summary (
    bucket TIMESTAMPTZ NOT NULL CHECK (bucket = date_trunc('hour', bucket, 'UTC')),
    action_ref TEXT,
    execution_count BIGINT NOT NULL CHECK (execution_count >= 0),
    UNIQUE NULLS NOT DISTINCT (bucket, action_ref)
);
CREATE TABLE event_volume_hourly_summary (
    bucket TIMESTAMPTZ NOT NULL CHECK (bucket = date_trunc('hour', bucket, 'UTC')),
    trigger_ref TEXT NOT NULL,
    event_count BIGINT NOT NULL CHECK (event_count >= 0),
    UNIQUE (bucket, trigger_ref)
);
CREATE TABLE worker_status_hourly_summary (
    bucket TIMESTAMPTZ NOT NULL CHECK (bucket = date_trunc('hour', bucket, 'UTC')),
    worker_name TEXT,
    new_status TEXT,
    transition_count BIGINT NOT NULL CHECK (transition_count >= 0),
    UNIQUE NULLS NOT DISTINCT (bucket, worker_name, new_status)
);

-- Transition tables deduplicate statement groups. A producer reuses only a
-- marker whose full origin AND actual tuple xmin belong to this transaction.
-- An imported marker can copy an origin but cannot copy its old system xmin.
-- There is no shared unique key or cross-producer lock. Own source writes are
-- serialized; after they commit the producer cannot reuse this transaction ID.
-- A marker created inside a savepoint has a child xmin. Keep it conservatively
-- separate unless an outer-transaction marker already covers the same group.
-- Filtering precedes defaults, so ignored duplicates do not call nextval.
-- Readers/builders still acknowledge only exact visible IDs, never max ID.
CREATE FUNCTION native_summary_notify() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    -- Bind the volatile xid getter once. Cached SPI plans can use this typed
    -- parameter as the third origin-index condition, with xmin a heap check.
    v_origin xid8 := pg_current_xact_id();
    v_xmin xid := v_origin::xid;
    v_status native_summary_kind := CASE TG_TABLE_NAME
        WHEN 'execution_history' THEN 'execution_status'::native_summary_kind
        ELSE 'worker_status'::native_summary_kind END;
    v_creation BOOLEAN := TG_TABLE_NAME = 'execution_history';
    v_group RECORD;
BEGIN
    -- Static transition-table queries retain per-trigger SPI plans. In the
    -- common execution INSERT case, irrelevant status rows produce no group.
    -- Check existing ownership before running any INSERT/default machinery.
    IF TG_TABLE_NAME = 'event' THEN
        IF TG_OP = 'INSERT' THEN
            FOR v_group IN SELECT DISTINCT date_trunc('hour', created, 'UTC') AS bucket FROM native_new LOOP
                IF NOT EXISTS (SELECT 1 FROM native_summary_invalidation i
                    WHERE i.kind='event_volume' AND i.bucket=v_group.bucket AND i.transaction_origin=v_origin AND i.xmin=v_xmin) THEN
                    INSERT INTO native_summary_invalidation(kind,bucket) VALUES ('event_volume',v_group.bucket);
                END IF;
            END LOOP;
        ELSIF TG_OP = 'DELETE' THEN
            FOR v_group IN SELECT DISTINCT date_trunc('hour', created, 'UTC') AS bucket FROM native_old LOOP
                IF NOT EXISTS (SELECT 1 FROM native_summary_invalidation i
                    WHERE i.kind='event_volume' AND i.bucket=v_group.bucket AND i.transaction_origin=v_origin AND i.xmin=v_xmin) THEN
                    INSERT INTO native_summary_invalidation(kind,bucket) VALUES ('event_volume',v_group.bucket);
                END IF;
            END LOOP;
        ELSE
            FOR v_group IN SELECT DISTINCT date_trunc('hour', created, 'UTC') AS bucket
                FROM (SELECT created FROM native_old UNION ALL SELECT created FROM native_new) changed LOOP
                IF NOT EXISTS (SELECT 1 FROM native_summary_invalidation i
                    WHERE i.kind='event_volume' AND i.bucket=v_group.bucket AND i.transaction_origin=v_origin AND i.xmin=v_xmin) THEN
                    INSERT INTO native_summary_invalidation(kind,bucket) VALUES ('event_volume',v_group.bucket);
                END IF;
            END LOOP;
        END IF;
    ELSE
        IF TG_OP = 'INSERT' THEN
            FOR v_group IN
                SELECT kinds.kind,date_trunc('hour',r.time,'UTC') AS bucket FROM native_new r
                CROSS JOIN LATERAL (VALUES
                    (v_status,'status'=ANY(r.changed_fields)),
                    ('execution_creation'::native_summary_kind,v_creation AND r.operation='INSERT')
                ) kinds(kind,relevant) WHERE kinds.relevant GROUP BY kinds.kind,bucket
            LOOP
                IF NOT EXISTS (SELECT 1 FROM native_summary_invalidation i
                    WHERE i.kind=v_group.kind AND i.bucket=v_group.bucket AND i.transaction_origin=v_origin AND i.xmin=v_xmin) THEN
                    INSERT INTO native_summary_invalidation(kind,bucket) VALUES (v_group.kind,v_group.bucket);
                END IF;
            END LOOP;
        ELSIF TG_OP = 'DELETE' THEN
            FOR v_group IN
                SELECT kinds.kind,date_trunc('hour',r.time,'UTC') AS bucket FROM native_old r
                CROSS JOIN LATERAL (VALUES
                    (v_status,'status'=ANY(r.changed_fields)),
                    ('execution_creation'::native_summary_kind,v_creation AND r.operation='INSERT')
                ) kinds(kind,relevant) WHERE kinds.relevant GROUP BY kinds.kind,bucket
            LOOP
                IF NOT EXISTS (SELECT 1 FROM native_summary_invalidation i
                    WHERE i.kind=v_group.kind AND i.bucket=v_group.bucket AND i.transaction_origin=v_origin AND i.xmin=v_xmin) THEN
                    INSERT INTO native_summary_invalidation(kind,bucket) VALUES (v_group.kind,v_group.bucket);
                END IF;
            END LOOP;
        ELSE
            FOR v_group IN
                SELECT kinds.kind,date_trunc('hour',r.time,'UTC') AS bucket
                FROM (SELECT time,operation,changed_fields FROM native_old UNION ALL SELECT time,operation,changed_fields FROM native_new) r
                CROSS JOIN LATERAL (VALUES
                    (v_status,'status'=ANY(r.changed_fields)),
                    ('execution_creation'::native_summary_kind,v_creation AND r.operation='INSERT')
                ) kinds(kind,relevant) WHERE kinds.relevant GROUP BY kinds.kind,bucket
            LOOP
                IF NOT EXISTS (SELECT 1 FROM native_summary_invalidation i
                    WHERE i.kind=v_group.kind AND i.bucket=v_group.bucket AND i.transaction_origin=v_origin AND i.xmin=v_xmin) THEN
                    INSERT INTO native_summary_invalidation(kind,bucket) VALUES (v_group.kind,v_group.bucket);
                END IF;
            END LOOP;
        END IF;
    END IF;
    RETURN NULL;
END $$;

-- These invoker functions keep each parent-lock hold in one server statement.
-- statement_timeout therefore bounds the entire operation, not each DDL step.
-- No SECURITY DEFINER: the supervisor must belong to the schema/table owner role.
CREATE FUNCTION native_partition_check(
    managed native_partition_parent, leaf_name TEXT,
    lo TIMESTAMPTZ, hi TIMESTAMPTZ, is_default BOOLEAN DEFAULT false
) RETURNS void LANGUAGE plpgsql AS $$
DECLARE p pg_class; c pg_class; key_name TEXT; bound TEXT; parsed TEXT[];
BEGIN
    SELECT * INTO p FROM pg_class WHERE oid = to_regclass(managed::text);
    SELECT * INTO c FROM pg_class WHERE oid = to_regclass(leaf_name);
    SELECT a.attname INTO key_name FROM pg_partitioned_table pt
    JOIN pg_attribute a ON a.attrelid = pt.partrelid AND a.attnum = pt.partattrs[0]
    WHERE pt.partrelid = p.oid AND pt.partstrat = 'r' AND pt.partnatts = 1;
    IF p.relkind IS DISTINCT FROM 'p' OR c.relkind IS DISTINCT FROM 'r'
       OR c.relowner IS DISTINCT FROM p.relowner OR c.relnamespace IS DISTINCT FROM p.relnamespace
       OR key_name IS DISTINCT FROM (CASE managed WHEN 'execution_history' THEN 'time' ELSE 'created' END)
       OR NOT EXISTS (SELECT 1 FROM pg_inherits WHERE inhparent = p.oid AND inhrelid = c.oid) THEN
        RAISE EXCEPTION 'Incompatible native partition identity: % / %', managed, leaf_name USING ERRCODE = '55000';
    END IF;
    bound := pg_get_expr(c.relpartbound, c.oid);
    IF is_default THEN
        IF bound <> 'DEFAULT' THEN RAISE EXCEPTION 'Expected DEFAULT: %', leaf_name USING ERRCODE = '55000'; END IF;
    ELSE
        parsed := regexp_match(bound, '^FOR VALUES FROM \(''([^'']+)''\) TO \(''([^'']+)''\)$');
        IF parsed IS NULL OR parsed[1]::timestamptz IS DISTINCT FROM lo OR parsed[2]::timestamptz IS DISTINCT FROM hi THEN
            RAISE EXCEPTION 'Incompatible native partition bounds: %', leaf_name USING ERRCODE = '55000';
        END IF;
    END IF;
    -- Every valid parent index must have a valid attached leaf index. Merely
    -- finding a similarly named index is not enough.
    IF EXISTS (
        SELECT 1 FROM pg_index pi WHERE pi.indrelid = p.oid AND (
            NOT pi.indisvalid OR NOT EXISTS (
                SELECT 1 FROM pg_inherits ii JOIN pg_index ci ON ci.indexrelid = ii.inhrelid
                WHERE ii.inhparent = pi.indexrelid AND ci.indrelid = c.oid AND ci.indisvalid
            )
        )
    ) THEN RAISE EXCEPTION 'Missing or invalid native leaf indexes: %', leaf_name USING ERRCODE = '55000'; END IF;
END $$;

CREATE FUNCTION native_partition_ensure_day(managed native_partition_parent, day TIMESTAMPTZ, row_cap BIGINT)
RETURNS TABLE(outcome TEXT, rows_moved BIGINT) LANGUAGE plpgsql AS $$
DECLARE parent_name TEXT := managed::text; fallback TEXT := managed::text || '_default';
    col TEXT := CASE managed WHEN 'execution_history' THEN 'time' ELSE 'created' END;
    leaf TEXT; hi TIMESTAMPTZ; found_rows BIGINT; owner_name TEXT; registered RECORD;
    v_origin xid8 := pg_current_xact_id(); v_xmin xid := v_origin::xid;
BEGIN
    IF row_cap <= 0 OR row_cap = 9223372036854775807 OR NOT isfinite(day)
       OR day <> date_trunc('day', day, 'UTC') THEN
        RAISE EXCEPTION 'Expected finite UTC day and positive bounded repair cap' USING ERRCODE = '22023';
    END IF;
    hi := day + interval '24 hours';
    leaf := parent_name || '_p' || to_char(day AT TIME ZONE 'UTC', 'YYYYMMDD');
    EXECUTE format('LOCK TABLE ONLY %I IN ACCESS EXCLUSIVE MODE', parent_name);
    EXECUTE format('LOCK TABLE ONLY %I IN ACCESS EXCLUSIVE MODE', fallback);
    PERFORM native_partition_check(managed, fallback, NULL, NULL, true);
    SELECT * INTO registered FROM native_partition_registry WHERE parent = managed AND lower_bound = day;
    IF FOUND THEN
        IF registered.partition_name <> leaf OR registered.upper_bound <> hi THEN
            RAISE EXCEPTION 'Incompatible native registry entry: %', leaf USING ERRCODE = '55000';
        END IF;
        PERFORM native_partition_check(managed, leaf, day, hi);
        RETURN QUERY SELECT 'already_present'::text, 0::bigint;
        RETURN;
    END IF;
    IF to_regclass(leaf) IS NOT NULL THEN
        RAISE EXCEPTION 'Refusing unregistered native relation: %', leaf USING ERRCODE = '55000';
    END IF;
    EXECUTE format('SELECT count(*) FROM (SELECT 1 FROM ONLY %I WHERE %I >= $1 AND %I < $2 LIMIT $3) bounded', fallback, col, col)
        INTO found_rows USING day, hi, row_cap + 1;
    IF found_rows > row_cap THEN
        RETURN QUERY SELECT 'deferred_over_budget'::text, found_rows;
        RETURN;
    END IF;
    EXECUTE format('CREATE TABLE %I (LIKE %I INCLUDING ALL)', leaf, parent_name);
    SELECT pg_get_userbyid(relowner) INTO owner_name FROM pg_class WHERE oid = to_regclass(parent_name);
    EXECUTE format('ALTER TABLE %I OWNER TO %I', leaf, owner_name);
    EXECUTE format('ALTER TABLE %I ADD CONSTRAINT native_day_bound CHECK (%I >= %L::timestamptz AND %I < %L::timestamptz)', leaf, col, day, col, hi);
    EXECUTE format('WITH moved AS (DELETE FROM ONLY %I WHERE %I >= $1 AND %I < $2 RETURNING *) INSERT INTO %I SELECT * FROM moved', fallback, col, col, leaf)
        USING day, hi;
    GET DIAGNOSTICS rows_moved = ROW_COUNT;
    -- Whole remaining DEFAULT validation is still deadline-bound. Any timeout
    -- rolls back the move and the newly created destination together.
    EXECUTE format('ALTER TABLE %I ADD CONSTRAINT native_default_exclusion CHECK (NOT (%I >= %L::timestamptz AND %I < %L::timestamptz)) NOT VALID', fallback, col, day, col, hi);
    EXECUTE format('ALTER TABLE %I VALIDATE CONSTRAINT native_default_exclusion', fallback);
    EXECUTE format('ALTER TABLE %I ATTACH PARTITION %I FOR VALUES FROM (%L) TO (%L)', parent_name, leaf, day, hi);
    EXECUTE format('ALTER TABLE %I DROP CONSTRAINT native_default_exclusion', fallback);
    INSERT INTO native_partition_registry(parent, partition_name, lower_bound, upper_bound) VALUES (managed, leaf, day, hi);
    PERFORM native_partition_check(managed, leaf, day, hi);
    -- Physical moves bypass the parent statement triggers. Invalidate only
    -- affected source predicates, once per kind/hour, in this same transaction.
    IF rows_moved > 0 AND managed = 'event' THEN
        EXECUTE format('INSERT INTO native_summary_invalidation(kind,bucket)
            SELECT ''event_volume'',bucket FROM (SELECT DISTINCT date_trunc(''hour'',created,''UTC'') AS bucket FROM %I) affected
            WHERE NOT EXISTS (SELECT 1 FROM native_summary_invalidation existing
                WHERE existing.kind=''event_volume'' AND existing.bucket=affected.bucket
                AND existing.transaction_origin=$1 AND existing.xmin=$2)', leaf) USING v_origin,v_xmin;
    ELSIF rows_moved > 0 AND managed = 'execution_history' THEN
        EXECUTE format('INSERT INTO native_summary_invalidation(kind,bucket)
            SELECT ''execution_status'',bucket FROM (SELECT DISTINCT date_trunc(''hour'',time,''UTC'') AS bucket FROM %I WHERE ''status''=ANY(changed_fields)) affected
            WHERE NOT EXISTS (SELECT 1 FROM native_summary_invalidation existing
                WHERE existing.kind=''execution_status'' AND existing.bucket=affected.bucket
                AND existing.transaction_origin=$1 AND existing.xmin=$2)', leaf) USING v_origin,v_xmin;
        EXECUTE format('INSERT INTO native_summary_invalidation(kind,bucket)
            SELECT ''execution_creation'',bucket FROM (SELECT DISTINCT date_trunc(''hour'',time,''UTC'') AS bucket FROM %I WHERE operation=''INSERT'') affected
            WHERE NOT EXISTS (SELECT 1 FROM native_summary_invalidation existing
                WHERE existing.kind=''execution_creation'' AND existing.bucket=affected.bucket
                AND existing.transaction_origin=$1 AND existing.xmin=$2)', leaf) USING v_origin,v_xmin;
    END IF;
    RETURN QUERY SELECT 'applied'::text, rows_moved;
END $$;

CREATE FUNCTION native_partition_expire(managed native_partition_parent, registry_id BIGINT, cutoff TIMESTAMPTZ)
RETURNS BOOLEAN LANGUAGE plpgsql AS $$
DECLARE r native_partition_registry; summary_kind native_summary_kind; summary_name TEXT;
BEGIN
    EXECUTE format('LOCK TABLE ONLY %I IN ACCESS EXCLUSIVE MODE', managed::text);
    SELECT * INTO r FROM native_partition_registry WHERE id = registry_id AND parent = managed FOR UPDATE;
    IF NOT FOUND THEN RETURN false; END IF;
    IF r.upper_bound > cutoff THEN RETURN false; END IF;
    IF r.partition_name <> managed::text || '_p' || to_char(r.lower_bound AT TIME ZONE 'UTC', 'YYYYMMDD') THEN
        RAISE EXCEPTION 'Incompatible native expiry name: %', r.partition_name USING ERRCODE = '55000';
    END IF;
    PERFORM native_partition_check(managed, r.partition_name, r.lower_bound, r.upper_bound);
    FOR summary_kind IN SELECT k FROM unnest(CASE managed
        WHEN 'event' THEN ARRAY['event_volume'::native_summary_kind]
        WHEN 'execution_history' THEN ARRAY['execution_status'::native_summary_kind, 'execution_creation'::native_summary_kind]
        ELSE ARRAY[]::native_summary_kind[] END) k ORDER BY k LOOP
        PERFORM 1 FROM native_summary_state WHERE native_summary_state.kind = summary_kind FOR UPDATE;
        summary_name := CASE summary_kind WHEN 'event_volume' THEN 'event_volume_hourly_summary'
            WHEN 'execution_status' THEN 'execution_status_hourly_summary' ELSE 'execution_creation_hourly_summary' END;
        EXECUTE format('DELETE FROM %I WHERE bucket >= $1 AND bucket < $2', summary_name) USING r.lower_bound, r.upper_bound;
        DELETE FROM native_summary_hour h WHERE h.kind = summary_kind AND h.bucket >= r.lower_bound AND h.bucket < r.upper_bound;
        DELETE FROM native_summary_invalidation n WHERE n.kind = summary_kind AND n.bucket >= r.lower_bound AND n.bucket < r.upper_bound;
    END LOOP;
    EXECUTE format('DROP TABLE %I', r.partition_name);
    DELETE FROM native_partition_registry WHERE id = r.id;
    RETURN true;
END $$;

CREATE FUNCTION native_partition_backlog_day(managed native_partition_parent)
RETURNS TIMESTAMPTZ LANGUAGE plpgsql AS $$
DECLARE r native_partition_registry; result TIMESTAMPTZ;
    parent_name TEXT := managed::text; fallback TEXT := managed::text || '_default';
    col TEXT := CASE managed WHEN 'execution_history' THEN 'time' ELSE 'created' END;
BEGIN
    EXECUTE format('LOCK TABLE ONLY %I IN ACCESS SHARE MODE', parent_name);
    PERFORM native_partition_check(managed, fallback, NULL, NULL, true);
    FOR r IN SELECT * FROM native_partition_registry WHERE parent = managed LOOP
        IF r.partition_name <> parent_name || '_p' || to_char(r.lower_bound AT TIME ZONE 'UTC', 'YYYYMMDD') THEN
            RAISE EXCEPTION 'Incompatible native registry name: %', r.partition_name USING ERRCODE = '55000';
        END IF;
        PERFORM native_partition_check(managed, r.partition_name, r.lower_bound, r.upper_bound);
    END LOOP;
    IF EXISTS (SELECT 1 FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
        WHERE i.inhparent = to_regclass(parent_name) AND c.relname <> fallback
        AND NOT EXISTS (SELECT 1 FROM native_partition_registry n WHERE n.parent = managed AND n.partition_name = c.relname)) THEN
        RAISE EXCEPTION 'Unregistered native child: %', parent_name USING ERRCODE = '55000';
    END IF;
    EXECUTE format('SELECT date_trunc(''day'', %I, ''UTC'') FROM ONLY %I ORDER BY %I LIMIT 1', col, fallback, col) INTO result;
    RETURN result;
END $$;

CREATE FUNCTION native_partition_status(managed native_partition_parent, today TIMESTAMPTZ, lookahead BIGINT, row_cap BIGINT)
RETURNS TABLE(registered_partitions BIGINT, future_partitions BIGINT, missing_future_partitions BIGINT,
              default_rows_at_least BIGINT, default_count_exact BOOLEAN, oldest_default_day TIMESTAMPTZ)
LANGUAGE plpgsql AS $$
DECLARE horizon TIMESTAMPTZ;
BEGIN
    IF lookahead < 1 OR row_cap < 1 OR row_cap = 9223372036854775807
       OR NOT isfinite(today) OR today <> date_trunc('day', today, 'UTC') THEN
        RAISE EXCEPTION 'Invalid native status budget' USING ERRCODE = '22023';
    END IF;
    oldest_default_day := native_partition_backlog_day(managed);
    horizon := today + (lookahead + 1) * interval '24 hours';
    SELECT count(*), count(*) FILTER (WHERE lower_bound >= today AND upper_bound <= horizon)
        INTO registered_partitions, future_partitions FROM native_partition_registry WHERE parent = managed;
    missing_future_partitions := lookahead + 1 - future_partitions;
    EXECUTE format('SELECT count(*) FROM (SELECT 1 FROM ONLY %I LIMIT $1) bounded', managed::text || '_default')
        INTO default_rows_at_least USING row_cap + 1;
    default_count_exact := default_rows_at_least <= row_cap;
    RETURN NEXT;
END $$;

DO $$
DECLARE source TEXT;
BEGIN
    FOREACH source IN ARRAY ARRAY['event', 'execution_history', 'worker_history'] LOOP
        EXECUTE format('CREATE TRIGGER native_summary_insert AFTER INSERT ON %I REFERENCING NEW TABLE AS native_new FOR EACH STATEMENT EXECUTE FUNCTION native_summary_notify()', source);
        EXECUTE format('CREATE TRIGGER native_summary_update AFTER UPDATE ON %I REFERENCING OLD TABLE AS native_old NEW TABLE AS native_new FOR EACH STATEMENT EXECUTE FUNCTION native_summary_notify()', source);
        EXECUTE format('CREATE TRIGGER native_summary_delete AFTER DELETE ON %I REFERENCING OLD TABLE AS native_old FOR EACH STATEMENT EXECUTE FUNCTION native_summary_notify()', source);
    END LOOP;
END $$;
