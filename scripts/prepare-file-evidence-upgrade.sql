-- Explicit coordinated upgrade preflight. Stop/drain ALL old application
-- writers first. Use the application's search_path and ON_ERROR_STOP. This
-- script neither drops reservation data nor changes applied migration history.
-- Run the ordinary migrator through 06006 immediately after this commits;
-- never start an application while the reservation table is parked.
BEGIN;
SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':named-type-graph',0));
DO $$
BEGIN
    IF to_regclass('file_evidence_upgrade_reservation') IS NOT NULL THEN RETURN; END IF;
    IF to_regclass('_sqlx_migrations') IS NOT NULL THEN
        IF EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version=20261015006005 AND success) THEN RETURN; END IF;
    END IF;
    IF to_regclass('artifact_file_origin') IS NULL THEN RETURN; END IF;
    LOCK TABLE artifact_file_origin IN ACCESS EXCLUSIVE MODE;
    IF NOT EXISTS(SELECT 1 FROM artifact_file_origin) THEN RETURN; END IF;
    -- Preserve the actual table, including FKs, immutable triggers and indexes.
    -- The guarded empty view satisfies 06005 without pretending old rows were
    -- admitted. Writes through it are rejected by its CHECK OPTION.
    ALTER TABLE artifact_file_origin RENAME TO file_evidence_upgrade_reservation;
    EXECUTE 'CREATE VIEW artifact_file_origin AS SELECT artifact_version,origin_read,admission,execution,attempt,sponsor,max_plaintext_bytes,source_digest,receipts,created FROM file_evidence_upgrade_reservation WHERE false WITH LOCAL CHECK OPTION';
END;
$$;
COMMIT;
