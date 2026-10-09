SET search_path TO attune, public;

-- The supervisor leader session serializes cycles. Advance reservations before
-- an attempted repair so busy/oversized days cannot monopolize a one-op budget.
CREATE TABLE native_partition_reconcile_state (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    next_parent SMALLINT NOT NULL DEFAULT 0 CHECK (next_parent BETWEEN 0 AND 2)
);
INSERT INTO native_partition_reconcile_state(id) VALUES (TRUE);
CREATE TABLE native_partition_reconcile_cursor (
    parent native_partition_parent PRIMARY KEY,
    last_day TIMESTAMPTZ CHECK (last_day = date_trunc('day', last_day, 'UTC'))
);
INSERT INTO native_partition_reconcile_cursor(parent)
VALUES ('event'), ('execution_history'), ('audit_event');

-- Seek one indexed DEFAULT day and one missing future day on either side of
-- the cursor. No DISTINCT/GROUP BY scan over an oversized oldest DEFAULT day.
CREATE FUNCTION native_partition_next_day(
    managed native_partition_parent, today TIMESTAMPTZ,
    horizon TIMESTAMPTZ, after_day TIMESTAMPTZ
) RETURNS TIMESTAMPTZ LANGUAGE plpgsql AS $$
DECLARE
    oldest TIMESTAMPTZ; later_default TIMESTAMPTZ;
    future TIMESTAMPTZ; later_future TIMESTAMPTZ;
    col TEXT := CASE managed WHEN 'execution_history' THEN 'time' ELSE 'created' END;
BEGIN
    -- Preserve the existing registry/catalog/ownership checks, including the
    -- rejection of unregistered children. A cursor is never ownership evidence.
    oldest := native_partition_backlog_day(managed);
    SELECT min(d.bucket), min(d.bucket) FILTER (WHERE d.bucket > after_day)
      INTO future, later_future
      FROM generate_series(today, horizon - interval '1 day', interval '1 day') d(bucket)
      WHERE NOT EXISTS (SELECT 1 FROM native_partition_registry r
          WHERE r.parent = managed AND r.lower_bound = d.bucket);
    IF after_day IS NOT NULL THEN
        EXECUTE format('SELECT date_trunc(''day'', %I, ''UTC'') FROM ONLY %I
            WHERE %I >= $1 + interval ''1 day'' ORDER BY %I LIMIT 1',
            col, managed::text || '_default', col, col)
            INTO later_default USING after_day;
        IF later_default IS NOT NULL OR later_future IS NOT NULL THEN
            RETURN least(later_default, later_future);
        END IF;
    END IF;
    RETURN least(oldest, future);
END $$;
