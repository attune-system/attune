-- ============================================================================
-- PACK INSTALL STATUS & INSTALL TRACKING
--
-- Tracks pack installation lifecycle (including worker-executed pack tests)
-- so clients can poll install status by pack ref even after a failed new
-- install has been rolled back (pack row deleted).
-- ============================================================================

-- Mark packs as pending while installation activities (including automated
-- tests) are in progress.
ALTER TABLE pack
    ADD COLUMN IF NOT EXISTS install_status TEXT NOT NULL DEFAULT 'installed'
        CONSTRAINT valid_pack_install_status
        CHECK (install_status IN ('pending', 'installed', 'install_failed'));

-- Per-install tracking record. Survives rollback: when a brand-new pack fails
-- installation and is deleted, its pack_install row remains so the failure
-- (including the test result snapshot) can still be queried by pack ref.
CREATE TABLE IF NOT EXISTS pack_install (
    id BIGSERIAL PRIMARY KEY,
    pack_ref TEXT NOT NULL,
    pack_version TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CONSTRAINT valid_pack_install_status_enum
        CHECK (status IN ('pending', 'running', 'succeeded', 'failed', 'rolled_back')),
    trigger_reason TEXT NOT NULL, -- 'install', 'update', 'manual', 'validation'
    pack_id BIGINT,               -- plain BIGINT: pack may be deleted on rollback
    test_execution_id BIGINT,     -- plain BIGINT: pack_test_execution may cascade-delete
    test_result JSONB,            -- snapshot of PackTestResult; survives rollback
    error_message TEXT,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ
);

CREATE INDEX idx_pack_install_pack_ref ON pack_install(pack_ref, id DESC);
CREATE INDEX idx_pack_install_status ON pack_install(status);

COMMENT ON TABLE pack_install IS 'Tracks each pack installation attempt and its worker-executed test results';
COMMENT ON COLUMN pack_install.status IS 'pending, running, succeeded, failed, or rolled_back';
COMMENT ON COLUMN pack_install.test_result IS 'Snapshot of the test result; retained even if the pack is rolled back';