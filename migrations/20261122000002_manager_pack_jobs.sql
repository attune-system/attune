CREATE TABLE manager_pack_job (
    install BIGINT PRIMARY KEY REFERENCES pack_install(id) ON DELETE CASCADE,
    payload JSONB NOT NULL,
    worker BIGINT,
    manager_session BIGINT REFERENCES manager_session(id),
    instance UUID,
    attempt INTEGER NOT NULL CHECK (attempt > 0),
    release BIGINT,
    release_digest TEXT,
    deadline TIMESTAMPTZ NOT NULL,
    claim UUID,
    finish_digest TEXT,
    python JSONB,
    CHECK ((worker IS NULL AND manager_session IS NULL AND instance IS NULL)
        OR (worker IS NOT NULL AND manager_session IS NOT NULL AND instance IS NOT NULL)),
    CHECK ((release IS NULL) = (release_digest IS NULL)),
    CHECK (finish_digest IS NULL OR claim IS NOT NULL)
);
CREATE INDEX manager_pack_job_offers ON manager_pack_job (manager_session, install)
    WHERE claim IS NULL;
CREATE INDEX manager_pack_job_recovery ON manager_pack_job (deadline, install)
    WHERE finish_digest IS NULL;
