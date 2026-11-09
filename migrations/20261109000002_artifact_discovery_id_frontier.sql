-- Discovery carries no deletion authority. Restart any in-flight advisory pass
-- when changing traversal so no candidate relies on the former timestamp order.
ALTER TABLE artifact_retention_discovery
    DROP COLUMN last_created,
    DROP COLUMN high_created;
-- Drop the old paired-null checks before resetting a populated cursor.
UPDATE artifact_retention_discovery SET last_id=NULL, high_id=NULL;
ALTER TABLE artifact_retention_discovery
    ADD CHECK (last_id IS NULL OR high_id IS NOT NULL),
    ADD CHECK (last_id IS NULL OR last_id <= high_id);

-- The existing artifact_version primary key supplies both raw traversal bounds.
-- Do not retain the now-unused maintenance-specific composite index.
DROP INDEX idx_artifact_version_retention_cursor;
