-- Path quarantine outlives version metadata until native staging cleanup.
-- This table retains no runtime authority and grants no file permission.
CREATE TABLE artifact_file_staging_guard (
    file_path TEXT PRIMARY KEY,
    artifact_version BIGINT NOT NULL
);
CREATE TRIGGER artifact_file_staging_guard_immutable BEFORE UPDATE ON artifact_file_staging_guard
FOR EACH ROW EXECUTE FUNCTION guard_worker_delivery_immutable();
INSERT INTO artifact_file_staging_guard(file_path,artifact_version)
SELECT v.file_path,v.id FROM artifact_version v JOIN artifact_file_origin o ON o.artifact_version=v.id
WHERE v.file_path IS NOT NULL;

-- A legacy ref/version path must not be adopted by a new log or file while
-- native cleanup still owns its quarantine. New generic files use unique IDs.
CREATE FUNCTION guard_file_staging_binding() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.file_path IS NOT NULL AND EXISTS (
        SELECT 1 FROM artifact_file_staging_guard
        WHERE file_path=NEW.file_path AND artifact_version<>NEW.id
    ) THEN
        RAISE EXCEPTION 'File staging path is quarantined for another version';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER file_staging_binding_guard BEFORE INSERT OR UPDATE OF file_path ON artifact_version
FOR EACH ROW EXECUTE FUNCTION guard_file_staging_binding();
