-- Fresh-schema native maintenance setup. The original table migrations already
-- create RANGE parents and DEFAULT partitions. This migration never replaces
-- parents or copies rows; populated development databases require an explicit
-- reset when their recorded canonical migration checksums differ.
SET search_path TO attune, public;
SET LOCAL TIME ZONE 'UTC';

-- Match ManagedTable::ALL and the execution writer's history-before-audit order.
LOCK TABLE ONLY event, ONLY execution_history, ONLY audit_event IN ACCESS EXCLUSIVE MODE;

CREATE TYPE native_partition_parent AS ENUM ('event', 'execution_history', 'audit_event');
CREATE TABLE native_partition_registry (
    id BIGSERIAL PRIMARY KEY,
    parent native_partition_parent NOT NULL,
    partition_name TEXT NOT NULL UNIQUE,
    lower_bound TIMESTAMPTZ NOT NULL,
    upper_bound TIMESTAMPTZ NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (parent, lower_bound),
    CHECK (lower_bound = date_trunc('day', lower_bound, 'UTC')),
    CHECK (upper_bound = lower_bound + interval '24 hours')
);

DO $$
DECLARE
    parent_name TEXT; col TEXT; leaf TEXT; day TIMESTAMPTZ;
    today TIMESTAMPTZ := date_trunc('day', now(), 'UTC');
    has_rows BOOLEAN;
BEGIN
    FOREACH parent_name IN ARRAY ARRAY['event', 'execution_history', 'audit_event'] LOOP
        col := CASE parent_name WHEN 'execution_history' THEN 'time' ELSE 'created' END;
        IF NOT EXISTS (
            SELECT 1 FROM pg_class c
            WHERE c.oid = to_regclass(parent_name) AND c.relkind = 'p'
                AND pg_get_partkeydef(c.oid) = format('RANGE (%I)', col)
        ) OR NOT EXISTS (
            SELECT 1 FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
            WHERE i.inhparent = to_regclass(parent_name)
                AND c.oid = to_regclass(parent_name || '_default')
                AND pg_get_expr(c.relpartbound, c.oid) = 'DEFAULT'
        ) THEN
            RAISE EXCEPTION 'Native partition setup requires the canonical RANGE parent and DEFAULT partition: %', parent_name
                USING ERRCODE = '55000';
        END IF;
        -- Earlier history/audit writes outside this window can stay in DEFAULT.
        -- Do not copy them or invent historical leaves during installation.
        EXECUTE format('SELECT EXISTS (SELECT 1 FROM ONLY %I WHERE %I >= $1 AND %I < $2)',
            parent_name || '_default', col, col)
            INTO has_rows USING today, today + interval '8 days';
        IF has_rows THEN
            RAISE EXCEPTION 'Native partition setup requires an empty initial UTC window in DEFAULT: %', parent_name
                USING ERRCODE = '55000';
        END IF;
        FOR day IN SELECT today + n * interval '1 day' FROM generate_series(0, 7) n LOOP
            leaf := parent_name || '_p' || to_char(day AT TIME ZONE 'UTC', 'YYYYMMDD');
            EXECUTE format('CREATE TABLE %I PARTITION OF %I FOR VALUES FROM (%L) TO (%L)',
                leaf, parent_name, day, day + interval '1 day');
            INSERT INTO native_partition_registry(parent, partition_name, lower_bound, upper_bound)
            VALUES (parent_name::native_partition_parent, leaf, day, day + interval '1 day');
        END LOOP;
    END LOOP;
END $$;

ALTER TABLE runtime_retention_config ADD COLUMN native_maintenance JSONB NOT NULL DEFAULT '{}';
CREATE TABLE native_maintenance_schedule (
    job TEXT PRIMARY KEY CHECK (job IN ('partition', 'summary', 'retention')),
    next_due TIMESTAMPTZ NOT NULL,
    last_success TIMESTAMPTZ
);
INSERT INTO native_maintenance_schedule(job, next_due)
VALUES ('partition', now()), ('summary', now()), ('retention', now());
