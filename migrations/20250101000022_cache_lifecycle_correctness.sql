-- Migration: Cache lifecycle correctness metadata and limits
-- Description: Persists refresh failure streaks and reserves one prior readable generation.
-- Version: 20250101000022

SET search_path TO attune, public;

ALTER TABLE cache_namespace
    ADD COLUMN consecutive_refresh_failures INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN last_refresh_failure_at TIMESTAMPTZ;

ALTER TABLE cache_namespace
    ADD CONSTRAINT cache_namespace_consecutive_refresh_failures_nonnegative
        CHECK (consecutive_refresh_failures >= 0);

-- Preserve failure streaks that predate these namespace-level counters. A
-- successful activation resets the streak, so only later failures contribute.
UPDATE cache_namespace AS n
   SET consecutive_refresh_failures = backfill.failure_count,
       last_refresh_failure_at = backfill.last_failure_at
  FROM (
      SELECT n2.id AS namespace_id,
             LEAST(COUNT(f.id), 2147483647)::INTEGER AS failure_count,
             MAX(COALESCE(f.failed, f.created)) AS last_failure_at
        FROM cache_namespace AS n2
        LEFT JOIN LATERAL (
            SELECT MAX(g.activated) AS latest_activation
              FROM cache_generation AS g
             WHERE g.namespace = n2.id
               AND g.state IN ('active', 'retired')
        ) AS success ON TRUE
        JOIN cache_generation AS f
          ON f.namespace = n2.id
         AND f.state = 'failed'
         AND COALESCE(f.failed, f.created) > COALESCE(success.latest_activation, '-infinity'::TIMESTAMPTZ)
       GROUP BY n2.id
  ) AS backfill
 WHERE n.id = backfill.namespace_id;

UPDATE cache_namespace
   SET max_retained_generations = 2
 WHERE max_retained_generations < 2;

ALTER TABLE cache_namespace
    DROP CONSTRAINT cache_namespace_retained_generations_positive,
    ADD CONSTRAINT cache_namespace_retained_generations_minimum
        CHECK (max_retained_generations >= 2);
