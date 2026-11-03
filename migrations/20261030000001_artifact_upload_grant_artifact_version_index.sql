-- Cleanup settlement checks and FK cascades must cover both body and segment
-- grants, regardless of state or segment_sequence.
CREATE INDEX idx_artifact_upload_grant_artifact_version
    ON artifact_upload_grant (artifact_version);
