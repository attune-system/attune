CREATE TABLE manager_session (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    role worker_role_enum NOT NULL,
    pool TEXT NOT NULL CHECK (pool ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    name TEXT NOT NULL CHECK (name ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    instance UUID NOT NULL,
    enrollment_digest TEXT NOT NULL CHECK (enrollment_digest ~ '^[0-9a-f]{64}$'),
    expires_at TIMESTAMPTZ NOT NULL,
    closed_at TIMESTAMPTZ,
    UNIQUE (role, pool, name, instance)
);

CREATE UNIQUE INDEX manager_session_open_slot
    ON manager_session (role, pool, name) WHERE closed_at IS NULL;
