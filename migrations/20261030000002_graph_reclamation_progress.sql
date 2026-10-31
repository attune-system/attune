-- Collector scheduling/progress only. These rows grant no graph authority.
-- Follow the artifact-upload-grant index migration; migration versions are unique.
-- Successful page cursors commit with metadata deletion. Deferred visits follow
-- acknowledged rollback. Progress/oldest deferral survive leader restart;
-- every completed raw pass wraps to 0.
-- A fixed per-pass high-water mark prevents continuous insertion from delaying
-- wrap forever and hiding previously pinned or late-committing older IDs.
-- A confirmed over-budget DELETE rollback shrinks the next raw probe. A
-- single-leaf timeout records a deferred visit and advances only the cursor.
-- Its age survives cheap successes and dirty/truncated passes until a clean pass.
CREATE TABLE graph_reclamation_progress (
    phase TEXT PRIMARY KEY CHECK (phase IN (
        'receipts', 'accepted_inputs', 'native_snapshots', 'cache_epochs',
        'admissions', 'policy_revisions', 'releases', 'object_deletions', 'file_audits'
    )),
    cursor_id BIGINT NOT NULL DEFAULT 0 CHECK (cursor_id >= 0),
    high_water BIGINT NOT NULL DEFAULT 0 CHECK (high_water >= cursor_id),
    probe_limit BIGINT NOT NULL DEFAULT 256 CHECK (probe_limit BETWEEN 1 AND 256),
    pass_deferred BOOLEAN NOT NULL DEFAULT FALSE,
    oversized_since TIMESTAMPTZ,
    revision BIGINT NOT NULL DEFAULT 0,
    next_attempt TIMESTAMPTZ NOT NULL DEFAULT '-infinity',
    last_attempt TIMESTAMPTZ,
    last_progress TIMESTAMPTZ,
    deferred_since TIMESTAMPTZ
);

INSERT INTO graph_reclamation_progress(phase) VALUES
    ('receipts'), ('accepted_inputs'), ('native_snapshots'), ('cache_epochs'),
    ('admissions'), ('policy_revisions'), ('releases'), ('object_deletions'), ('file_audits');

-- Graph/object continuation must not shorten the independent row-retention job.
ALTER TABLE native_maintenance_schedule
    DROP CONSTRAINT native_maintenance_schedule_job_check;
ALTER TABLE native_maintenance_schedule
    ADD CONSTRAINT native_maintenance_schedule_job_check
    CHECK (job IN ('partition', 'summary', 'retention', 'object_retention'));
ALTER TABLE native_maintenance_schedule
    ADD COLUMN deferred_since TIMESTAMPTZ;
INSERT INTO native_maintenance_schedule(job, next_due)
VALUES ('object_retention', clock_timestamp());
