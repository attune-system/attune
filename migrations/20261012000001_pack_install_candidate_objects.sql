-- Candidate test archives are durable objects, not API-replica-local files.
ALTER TABLE pack_install
    ADD COLUMN candidate_object_key TEXT,
    ADD COLUMN candidate_provider_version TEXT,
    ADD COLUMN candidate_archive_size BIGINT,
    ADD COLUMN candidate_digest TEXT,
    ADD CONSTRAINT pack_install_candidate_object_complete CHECK (
        (candidate_object_key IS NULL AND candidate_provider_version IS NULL
            AND candidate_archive_size IS NULL AND candidate_digest IS NULL)
        OR
        (candidate_object_key IS NOT NULL AND candidate_provider_version IS NOT NULL
            AND candidate_archive_size IS NOT NULL AND candidate_archive_size >= 0
            AND candidate_digest IS NOT NULL AND candidate_digest ~ '^[0-9a-f]{64}$')
    );

CREATE INDEX idx_pack_install_candidate_object ON pack_install(candidate_object_key)
    WHERE candidate_object_key IS NOT NULL;
