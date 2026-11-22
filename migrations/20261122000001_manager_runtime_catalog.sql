CREATE TABLE manager_runtime_catalog (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    revision BIGINT NOT NULL DEFAULT 1
);
INSERT INTO manager_runtime_catalog DEFAULT VALUES;

CREATE FUNCTION advance_manager_runtime_catalog() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE manager_runtime_catalog SET revision = revision + 1 WHERE singleton;
    RETURN NULL;
END;
$$;

CREATE TRIGGER manager_runtime_added_removed AFTER INSERT OR DELETE ON runtime
    FOR EACH STATEMENT EXECUTE FUNCTION advance_manager_runtime_catalog();
CREATE TRIGGER manager_runtime_changed AFTER UPDATE ON runtime FOR EACH ROW
    WHEN ((OLD.ref, OLD.name, OLD.aliases, OLD.distributions, OLD.execution_config, OLD.retired_at)
        IS DISTINCT FROM
          (NEW.ref, NEW.name, NEW.aliases, NEW.distributions, NEW.execution_config, NEW.retired_at))
    EXECUTE FUNCTION advance_manager_runtime_catalog();
CREATE TRIGGER manager_runtime_version_added_removed AFTER INSERT OR DELETE ON runtime_version
    FOR EACH STATEMENT EXECUTE FUNCTION advance_manager_runtime_catalog();
CREATE TRIGGER manager_runtime_version_changed AFTER UPDATE ON runtime_version FOR EACH ROW
    WHEN ((OLD.runtime, OLD.runtime_ref, OLD.version, OLD.distributions, OLD.execution_config, OLD.retired_at)
        IS DISTINCT FROM
          (NEW.runtime, NEW.runtime_ref, NEW.version, NEW.distributions, NEW.execution_config, NEW.retired_at))
    EXECUTE FUNCTION advance_manager_runtime_catalog();

CREATE TABLE manager_runtime_report (
    worker BIGINT PRIMARY KEY REFERENCES worker(id) ON DELETE CASCADE,
    manager_session BIGINT NOT NULL UNIQUE REFERENCES manager_session(id),
    revision BIGINT NOT NULL,
    sequence BIGINT NOT NULL CHECK (sequence > 0),
    digest TEXT NOT NULL
);

-- Legacy registrations have no report. A manager-managed worker stays ineligible
-- from catalog publication until its current process publishes new evidence.
CREATE FUNCTION manager_runtime_is_current(worker_id BIGINT) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT NOT EXISTS (
        SELECT 1 FROM manager_runtime_report r
        JOIN manager_session s ON s.id = r.manager_session
        CROSS JOIN manager_runtime_catalog c
        WHERE r.worker = worker_id AND (r.revision <> c.revision OR s.closed_at IS NOT NULL
            OR s.expires_at <= statement_timestamp())
    );
$$;
