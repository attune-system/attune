ALTER TABLE pack_install
    DROP CONSTRAINT valid_pack_install_status_enum,
    ADD CONSTRAINT valid_pack_install_status_enum
        CHECK (status IN ('pending', 'running', 'activating', 'succeeded', 'failed', 'rolled_back'));

COMMENT ON COLUMN pack_install.status IS
    'pending, running, activating, succeeded, failed, or rolled_back';
