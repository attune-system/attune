-- Advisory discovery only; deletion retains current graph/policy/identity checks.
CREATE TABLE artifact_retention_discovery (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    last_created TIMESTAMPTZ,
    last_id BIGINT,
    high_created TIMESTAMPTZ,
    high_id BIGINT,
    CHECK ((last_created IS NULL) = (last_id IS NULL)),
    CHECK ((high_created IS NULL) = (high_id IS NULL)),
    CHECK (last_id IS NULL OR (last_created,last_id) <= (high_created,high_id))
);
INSERT INTO artifact_retention_discovery(singleton) VALUES (TRUE);
-- Existing created DESC index lacks the ID tie-break for bounded keyset pages.
CREATE INDEX idx_artifact_version_retention_cursor ON artifact_version(created,id);
