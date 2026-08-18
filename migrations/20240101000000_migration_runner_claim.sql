-- Establish one migration runner before application DDL is applied. SQLx
-- creates _sqlx_migrations before executing source migrations; the Docker
-- runner creates _migrations before entering its file loop.
SELECT pg_advisory_xact_lock(78210015);

CREATE TABLE IF NOT EXISTS public._attune_migration_runner (
    id SMALLINT PRIMARY KEY CHECK (id = 1),
    runner TEXT NOT NULL CHECK (runner IN ('sqlx', 'docker')),
    claimed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

DO $$
DECLARE
    has_attune_sqlx BOOLEAN := to_regclass('attune._sqlx_migrations') IS NOT NULL;
    has_public_sqlx BOOLEAN := to_regclass('public._sqlx_migrations') IS NOT NULL;
    has_attune_docker BOOLEAN := to_regclass('attune._migrations') IS NOT NULL;
    has_public_docker BOOLEAN := to_regclass('public._migrations') IS NOT NULL;
    detected_runner TEXT;
    claimed_runner TEXT;
BEGIN
    IF has_attune_sqlx AND has_public_sqlx THEN
        RAISE EXCEPTION 'Database contains SQLx migration history in both attune and public schemas';
    ELSIF has_attune_docker AND has_public_docker THEN
        RAISE EXCEPTION 'Database contains Docker migration history in both attune and public schemas';
    ELSIF (has_attune_sqlx OR has_public_sqlx)
        AND (has_attune_docker OR has_public_docker) THEN
        RAISE EXCEPTION 'Database contains mixed SQLx and Docker migration history';
    ELSIF has_attune_sqlx OR has_public_sqlx THEN
        detected_runner := 'sqlx';
    ELSIF has_attune_docker OR has_public_docker THEN
        detected_runner := 'docker';
    ELSE
        RAISE EXCEPTION 'No supported migration history found; use attune-api --migrate, sqlx migrate run, or the Docker migration container';
    END IF;

    INSERT INTO public._attune_migration_runner (id, runner)
    VALUES (1, detected_runner)
    ON CONFLICT (id) DO NOTHING;

    SELECT runner
    INTO claimed_runner
    FROM public._attune_migration_runner
    WHERE id = 1;

    IF claimed_runner <> detected_runner THEN
        RAISE EXCEPTION 'Database is claimed by the % migration runner, not %', claimed_runner, detected_runner;
    END IF;
END
$$;
