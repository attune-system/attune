-- Align pack test pass-rate columns with the Rust f64 model.
--
-- The application stores pass_rate as f64 (FLOAT8) and SQLx cannot decode
-- PostgreSQL NUMERIC into f64. The original DECIMAL(5,4) column caused
-- "mismatched types; Rust type f64 ... not compatible with SQL type NUMERIC"
-- when storing test results during pack registration. The check constraint
-- (0.0..=1.0) and the ratio semantics are unchanged.
--
-- The column is referenced by the pack_test_summary / pack_latest_test views,
-- so those must be dropped before altering the type and recreated afterwards.

DROP VIEW IF EXISTS pack_latest_test;
DROP VIEW IF EXISTS pack_test_summary;

ALTER TABLE pack_test_execution
    ALTER COLUMN pass_rate TYPE DOUBLE PRECISION;

-- Pack test result summary view (all test executions with pack info)
CREATE OR REPLACE VIEW pack_test_summary AS
SELECT
    p.id AS pack_id,
    p.ref AS pack_ref,
    p.label AS pack_label,
    pte.id AS test_execution_id,
    pte.pack_version,
    pte.execution_time AS test_time,
    pte.trigger_reason,
    pte.total_tests,
    pte.passed,
    pte.failed,
    pte.skipped,
    pte.pass_rate,
    pte.duration_ms,
    ROW_NUMBER() OVER (PARTITION BY p.id ORDER BY pte.execution_time DESC) AS rn
FROM pack p
LEFT JOIN pack_test_execution pte ON p.id = pte.pack_id
WHERE pte.id IS NOT NULL;

COMMENT ON VIEW pack_test_summary IS 'Summary of all pack test executions with pack details';

-- Latest test results per pack view
CREATE OR REPLACE VIEW pack_latest_test AS
SELECT
    pack_id,
    pack_ref,
    pack_label,
    test_execution_id,
    pack_version,
    test_time,
    trigger_reason,
    total_tests,
    passed,
    failed,
    skipped,
    pass_rate,
    duration_ms
FROM pack_test_summary
WHERE rn = 1;

COMMENT ON VIEW pack_latest_test IS 'Latest test results for each pack';

-- get_pack_test_stats declared avg_pass_rate DECIMAL; recreate it to return
-- FLOAT8 so the aggregation decodes into the Rust Option<f64> model.
DROP FUNCTION IF EXISTS get_pack_test_stats(BIGINT);

CREATE FUNCTION get_pack_test_stats(p_pack_id BIGINT)
RETURNS TABLE (
    total_executions BIGINT,
    successful_executions BIGINT,
    failed_executions BIGINT,
    avg_pass_rate DOUBLE PRECISION,
    avg_duration_ms BIGINT,
    last_test_time TIMESTAMPTZ,
    last_test_passed BOOLEAN
) AS $$
BEGIN
    RETURN QUERY
    SELECT
        COUNT(*)::BIGINT AS total_executions,
        COUNT(*) FILTER (WHERE passed = total_tests)::BIGINT AS successful_executions,
        COUNT(*) FILTER (WHERE failed > 0)::BIGINT AS failed_executions,
        AVG(pass_rate) AS avg_pass_rate,
        AVG(duration_ms)::BIGINT AS avg_duration_ms,
        MAX(execution_time) AS last_test_time,
        (SELECT failed = 0 FROM pack_test_execution
         WHERE pack_id = p_pack_id
         ORDER BY execution_time DESC
         LIMIT 1) AS last_test_passed
    FROM pack_test_execution
    WHERE pack_id = p_pack_id;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION get_pack_test_stats IS 'Get statistical summary of test executions for a pack';
