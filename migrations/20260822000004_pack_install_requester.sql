ALTER TABLE pack_install
    ADD COLUMN requested_by BIGINT;

CREATE INDEX idx_pack_install_requested_by
    ON pack_install(requested_by)
    WHERE requested_by IS NOT NULL;

COMMENT ON COLUMN pack_install.requested_by IS
    'Identity that requested the pack test; retained when a failed new pack is rolled back';
