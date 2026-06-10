-- Migration: Supporting Systems
-- Description: Creates keys, artifacts, artifact_version, queue_stats,
--              execution_admission, pack_environment, pack_testing,
--              and webhook function tables.
--              Consolidates former keys/artifacts, webhook helper,
--              pack environment, pack testing, artifact content,
--              and key JSONB migrations.
-- Version: 20250101000007

-- Set search_path for schema isolation
SET search_path TO attune, public;

-- ============================================================================
-- KEY TABLE
-- ============================================================================

CREATE TABLE key (
    id BIGSERIAL PRIMARY KEY,
    ref TEXT NOT NULL UNIQUE,
    owner_type owner_type_enum NOT NULL,
    owner TEXT,
    owner_identity BIGINT REFERENCES identity(id),
    owner_pack BIGINT REFERENCES pack(id),
    owner_pack_ref TEXT,
    owner_action BIGINT, -- Forward reference to action table
    owner_action_ref TEXT,
    owner_sensor BIGINT, -- Forward reference to sensor table
    owner_sensor_ref TEXT,
    name TEXT NOT NULL,
    encrypted BOOLEAN NOT NULL,
    encryption_key_hash TEXT,
    value JSONB NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Constraints
    CONSTRAINT key_ref_lowercase CHECK (ref = LOWER(ref)),
    CONSTRAINT key_ref_format CHECK (ref ~ '^[^.]+(\.[^.]+)*$')
);

-- Unique index on owner_type, owner, name
CREATE UNIQUE INDEX idx_key_unique ON key(owner_type, owner, name);

-- Indexes
CREATE INDEX idx_key_ref ON key(ref);
CREATE INDEX idx_key_owner_type ON key(owner_type);
CREATE INDEX idx_key_owner_identity ON key(owner_identity);
CREATE INDEX idx_key_owner_pack ON key(owner_pack);
CREATE INDEX idx_key_owner_action ON key(owner_action);
CREATE INDEX idx_key_owner_sensor ON key(owner_sensor);
CREATE INDEX idx_key_created ON key(created DESC);
CREATE INDEX idx_key_owner_type_owner ON key(owner_type, owner);
CREATE INDEX idx_key_owner_identity_name ON key(owner_identity, name);
CREATE INDEX idx_key_owner_pack_name ON key(owner_pack, name);

-- Function to validate and set owner fields
CREATE OR REPLACE FUNCTION validate_key_owner()
RETURNS TRIGGER AS $$
DECLARE
    owner_count INTEGER := 0;
BEGIN
    -- Count how many owner fields are set
    IF NEW.owner_identity IS NOT NULL THEN owner_count := owner_count + 1; END IF;
    IF NEW.owner_pack IS NOT NULL THEN owner_count := owner_count + 1; END IF;
    IF NEW.owner_action IS NOT NULL THEN owner_count := owner_count + 1; END IF;
    IF NEW.owner_sensor IS NOT NULL THEN owner_count := owner_count + 1; END IF;

    -- System owner should have no owner fields set
    IF NEW.owner_type = 'system' THEN
        IF owner_count > 0 THEN
            RAISE EXCEPTION 'System owner cannot have specific owner fields set';
        END IF;
        NEW.owner := 'system';
    -- All other types must have exactly one owner field set
    ELSIF owner_count != 1 THEN
        RAISE EXCEPTION 'Exactly one owner field must be set for owner_type %', NEW.owner_type;
    -- Validate owner_type matches the populated field and set owner
    ELSIF NEW.owner_type = 'identity' THEN
        IF NEW.owner_identity IS NULL THEN
            RAISE EXCEPTION 'owner_identity must be set for owner_type identity';
        END IF;
        NEW.owner := NEW.owner_identity::TEXT;
    ELSIF NEW.owner_type = 'pack' THEN
        IF NEW.owner_pack IS NULL THEN
            RAISE EXCEPTION 'owner_pack must be set for owner_type pack';
        END IF;
        NEW.owner := NEW.owner_pack::TEXT;
    ELSIF NEW.owner_type = 'action' THEN
        IF NEW.owner_action IS NULL THEN
            RAISE EXCEPTION 'owner_action must be set for owner_type action';
        END IF;
        NEW.owner := NEW.owner_action::TEXT;
    ELSIF NEW.owner_type = 'sensor' THEN
        IF NEW.owner_sensor IS NULL THEN
            RAISE EXCEPTION 'owner_sensor must be set for owner_type sensor';
        END IF;
        NEW.owner := NEW.owner_sensor::TEXT;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Trigger to validate owner fields
CREATE TRIGGER validate_key_owner_trigger
    BEFORE INSERT OR UPDATE ON key
    FOR EACH ROW
    EXECUTE FUNCTION validate_key_owner();

-- Trigger for updated timestamp
CREATE TRIGGER update_key_updated
    BEFORE UPDATE ON key
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

-- Comments
COMMENT ON TABLE key IS 'Keys store configuration values and secrets with ownership scoping';
COMMENT ON COLUMN key.ref IS 'Unique key reference (format: [owner.]name)';
COMMENT ON COLUMN key.owner_type IS 'Type of owner (system, identity, pack, action, sensor)';
COMMENT ON COLUMN key.owner IS 'Owner identifier (auto-populated by trigger)';
COMMENT ON COLUMN key.owner_identity IS 'Identity owner (if owner_type=identity)';
COMMENT ON COLUMN key.owner_pack IS 'Pack owner (if owner_type=pack)';
COMMENT ON COLUMN key.owner_pack_ref IS 'Pack reference for owner_pack';
COMMENT ON COLUMN key.owner_action IS 'Action owner (if owner_type=action)';
COMMENT ON COLUMN key.owner_sensor IS 'Sensor owner (if owner_type=sensor)';
COMMENT ON COLUMN key.name IS 'Key name within owner scope';
COMMENT ON COLUMN key.encrypted IS 'Whether the value is encrypted';
COMMENT ON COLUMN key.encryption_key_hash IS 'Hash of encryption key used';
COMMENT ON COLUMN key.value IS 'The actual JSON value (encrypted if encrypted=true); supports strings, objects, arrays, numbers, and booleans';


-- Add foreign key constraints for action and sensor references
ALTER TABLE key
    ADD CONSTRAINT key_owner_action_fkey
    FOREIGN KEY (owner_action) REFERENCES action(id) ON DELETE CASCADE;

ALTER TABLE key
    ADD CONSTRAINT key_owner_sensor_fkey
    FOREIGN KEY (owner_sensor) REFERENCES sensor(id) ON DELETE CASCADE;

-- ============================================================================
-- ARTIFACT TABLE
-- ============================================================================

CREATE TABLE artifact (
    id BIGSERIAL PRIMARY KEY,
    ref TEXT NOT NULL,
    scope owner_type_enum NOT NULL DEFAULT 'system',
    owner TEXT NOT NULL DEFAULT '',
    type artifact_type_enum NOT NULL,
    visibility artifact_visibility_enum NOT NULL DEFAULT 'private',
    name TEXT,
    description TEXT,
    content_type TEXT,
    size_bytes BIGINT,
    data JSONB,
    retention_policy artifact_retention_enum NOT NULL DEFAULT 'versions',
    retention_limit INTEGER NOT NULL DEFAULT 1,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes
CREATE INDEX idx_artifact_ref ON artifact(ref);
CREATE INDEX idx_artifact_scope ON artifact(scope);
CREATE INDEX idx_artifact_owner ON artifact(owner);
CREATE INDEX idx_artifact_type ON artifact(type);
CREATE INDEX idx_artifact_created ON artifact(created DESC);
CREATE INDEX idx_artifact_scope_owner ON artifact(scope, owner);
CREATE INDEX idx_artifact_type_created ON artifact(type, created DESC);
CREATE INDEX idx_artifact_visibility ON artifact(visibility);
CREATE INDEX idx_artifact_visibility_scope ON artifact(visibility, scope, owner);

-- Trigger
CREATE TRIGGER update_artifact_updated
    BEFORE UPDATE ON artifact
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

-- Comments
COMMENT ON TABLE artifact IS 'Artifacts track files, logs, and outputs from executions';
COMMENT ON COLUMN artifact.ref IS 'Artifact reference/path';
COMMENT ON COLUMN artifact.scope IS 'Owner type (system, identity, pack, action, sensor)';
COMMENT ON COLUMN artifact.owner IS 'Owner identifier';
COMMENT ON COLUMN artifact.type IS 'Artifact type (file, url, progress, etc.)';
COMMENT ON COLUMN artifact.visibility IS 'Visibility level: public (all users) or private (scoped by scope/owner)';
COMMENT ON COLUMN artifact.name IS 'Human-readable artifact name';
COMMENT ON COLUMN artifact.description IS 'Optional description of the artifact';
COMMENT ON COLUMN artifact.content_type IS 'MIME content type (e.g. application/json, text/plain)';
COMMENT ON COLUMN artifact.size_bytes IS 'Size of latest version content in bytes';
COMMENT ON COLUMN artifact.data IS 'Structured JSONB data for progress artifacts or metadata';
COMMENT ON COLUMN artifact.retention_policy IS 'How to retain artifacts (versions, days, hours, minutes)';
COMMENT ON COLUMN artifact.retention_limit IS 'Numeric limit for retention policy';

-- ============================================================================
-- ARTIFACT_VERSION TABLE
-- ============================================================================

CREATE TABLE artifact_version (
    id BIGSERIAL PRIMARY KEY,
    artifact BIGINT NOT NULL REFERENCES artifact(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    execution BIGINT,
    content_type TEXT,
    size_bytes BIGINT,
    content BYTEA,
    content_json JSONB,
    file_path TEXT,
    meta JSONB,
    created_by TEXT,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

ALTER TABLE artifact_version
    ADD CONSTRAINT uq_artifact_version_artifact_version UNIQUE (artifact, version);

CREATE INDEX idx_artifact_name ON artifact(name);
CREATE INDEX idx_artifact_version_artifact ON artifact_version(artifact);
CREATE INDEX idx_artifact_version_artifact_version ON artifact_version(artifact, version DESC);
CREATE INDEX idx_artifact_version_created ON artifact_version(created DESC);
CREATE INDEX idx_artifact_version_file_path ON artifact_version(file_path) WHERE file_path IS NOT NULL;
CREATE INDEX idx_artifact_version_execution ON artifact_version(execution) WHERE execution IS NOT NULL;
CREATE INDEX idx_artifact_version_artifact_execution ON artifact_version(artifact, execution) WHERE execution IS NOT NULL;

COMMENT ON TABLE artifact_version IS 'Immutable content snapshots for artifacts (file uploads, structured data)';
COMMENT ON COLUMN artifact_version.artifact IS 'Parent artifact this version belongs to';
COMMENT ON COLUMN artifact_version.version IS 'Version number (1-based, monotonically increasing per artifact)';
COMMENT ON COLUMN artifact_version.content_type IS 'MIME content type for this version';
COMMENT ON COLUMN artifact_version.size_bytes IS 'Size of content in bytes';
COMMENT ON COLUMN artifact_version.content IS 'Binary content (file data)';
COMMENT ON COLUMN artifact_version.content_json IS 'Structured JSON content';
COMMENT ON COLUMN artifact_version.meta IS 'Free-form metadata about this version';
COMMENT ON COLUMN artifact_version.created_by IS 'Who created this version (identity ref, action ref, system)';
COMMENT ON COLUMN artifact_version.file_path IS 'Relative path from artifacts_dir root for disk-stored content. When set, content BYTEA is NULL.';

CREATE OR REPLACE FUNCTION next_artifact_version(p_artifact_id BIGINT)
RETURNS INTEGER AS $$
DECLARE
    v_next INTEGER;
BEGIN
    SELECT COALESCE(MAX(version), 0) + 1
    INTO v_next
    FROM artifact_version
    WHERE artifact = p_artifact_id;

    RETURN v_next;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION next_artifact_version IS 'Returns the next version number for the given artifact';

CREATE OR REPLACE FUNCTION enforce_artifact_retention()
RETURNS TRIGGER AS $$
DECLARE
    v_policy artifact_retention_enum;
    v_limit INTEGER;
    v_count INTEGER;
BEGIN
    SELECT retention_policy, retention_limit
    INTO v_policy, v_limit
    FROM artifact
    WHERE id = NEW.artifact;

    IF v_policy = 'versions' AND v_limit > 0 THEN
        SELECT COUNT(*) INTO v_count
        FROM artifact_version
        WHERE artifact = NEW.artifact;

        IF v_count > v_limit THEN
            DELETE FROM artifact_version
            WHERE id IN (
                SELECT id
                FROM artifact_version
                WHERE artifact = NEW.artifact
                ORDER BY version ASC
                LIMIT (v_count - v_limit)
            );
        END IF;
    END IF;

    UPDATE artifact
    SET size_bytes = NEW.size_bytes,
        content_type = COALESCE(NEW.content_type, content_type)
    WHERE id = NEW.artifact;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_enforce_artifact_retention
    AFTER INSERT ON artifact_version
    FOR EACH ROW
    EXECUTE FUNCTION enforce_artifact_retention();

COMMENT ON FUNCTION enforce_artifact_retention IS 'Enforces version-count retention policy and syncs size to parent artifact';

CREATE OR REPLACE FUNCTION notify_artifact_created()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
BEGIN
    payload := json_build_object(
        'entity_type', 'artifact',
        'entity_id', NEW.id,
        'id', NEW.id,
        'ref', NEW.ref,
        'type', NEW.type,
        'visibility', NEW.visibility,
        'name', NEW.name,
        'scope', NEW.scope,
        'owner', NEW.owner,
        'content_type', NEW.content_type,
        'size_bytes', NEW.size_bytes,
        'created', NEW.created
    );

    PERFORM pg_notify('artifact_created', payload::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER artifact_created_notify
    AFTER INSERT ON artifact
    FOR EACH ROW
    EXECUTE FUNCTION notify_artifact_created();

COMMENT ON FUNCTION notify_artifact_created() IS 'Sends artifact creation notifications via PostgreSQL LISTEN/NOTIFY';

CREATE OR REPLACE FUNCTION notify_artifact_updated()
RETURNS TRIGGER AS $$
DECLARE
    payload JSON;
    latest_percent DOUBLE PRECISION;
    latest_message TEXT;
    entry_count INTEGER;
    latest_execution BIGINT;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        SELECT av.execution
        INTO latest_execution
        FROM artifact_version av
        WHERE av.artifact = NEW.id
        ORDER BY av.version DESC
        LIMIT 1;

        IF NEW.type = 'progress' AND NEW.data IS NOT NULL AND jsonb_typeof(NEW.data) = 'array' THEN
            entry_count := jsonb_array_length(NEW.data);
            IF entry_count > 0 THEN
                latest_percent := (NEW.data -> (entry_count - 1) ->> 'percent')::DOUBLE PRECISION;
                latest_message := NEW.data -> (entry_count - 1) ->> 'message';
            END IF;
        END IF;

        payload := json_build_object(
            'entity_type', 'artifact',
            'entity_id', NEW.id,
            'id', NEW.id,
            'ref', NEW.ref,
            'type', NEW.type,
            'visibility', NEW.visibility,
            'name', NEW.name,
            'scope', NEW.scope,
            'owner', NEW.owner,
            'content_type', NEW.content_type,
            'size_bytes', NEW.size_bytes,
            'execution', latest_execution,
            'progress_percent', latest_percent,
            'progress_message', latest_message,
            'progress_entries', entry_count,
            'created', NEW.created,
            'updated', NEW.updated
        );

        PERFORM pg_notify('artifact_updated', payload::text);
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER artifact_updated_notify
    AFTER UPDATE ON artifact
    FOR EACH ROW
    EXECUTE FUNCTION notify_artifact_updated();

COMMENT ON FUNCTION notify_artifact_updated() IS 'Sends artifact update notifications via PostgreSQL LISTEN/NOTIFY (includes progress summary for progress-type artifacts)';

CREATE OR REPLACE FUNCTION notify_artifact_version_changed()
RETURNS TRIGGER AS $$
DECLARE
    artifact_row artifact%ROWTYPE;
    payload JSON;
    latest_percent DOUBLE PRECISION;
    latest_message TEXT;
    entry_count INTEGER;
BEGIN
    SELECT *
    INTO artifact_row
    FROM artifact
    WHERE id = NEW.artifact;

    IF NOT FOUND THEN
        RETURN NEW;
    END IF;

    IF artifact_row.type = 'progress'
        AND artifact_row.data IS NOT NULL
        AND jsonb_typeof(artifact_row.data) = 'array'
    THEN
        entry_count := jsonb_array_length(artifact_row.data);
        IF entry_count > 0 THEN
            latest_percent := (artifact_row.data -> (entry_count - 1) ->> 'percent')::DOUBLE PRECISION;
            latest_message := artifact_row.data -> (entry_count - 1) ->> 'message';
        END IF;
    END IF;

    payload := json_build_object(
        'entity_type', 'artifact',
        'entity_id', artifact_row.id,
        'id', artifact_row.id,
        'ref', artifact_row.ref,
        'type', artifact_row.type,
        'visibility', artifact_row.visibility,
        'name', artifact_row.name,
        'scope', artifact_row.scope,
        'owner', artifact_row.owner,
        'content_type', COALESCE(NEW.content_type, artifact_row.content_type),
        'size_bytes', COALESCE(NEW.size_bytes, artifact_row.size_bytes),
        'execution', NEW.execution,
        'artifact_version_id', NEW.id,
        'version', NEW.version,
        'progress_percent', latest_percent,
        'progress_message', latest_message,
        'progress_entries', entry_count,
        'created', artifact_row.created,
        'updated', NOW()
    );

    PERFORM pg_notify('artifact_updated', payload::text);

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER artifact_version_notify
    AFTER INSERT OR UPDATE OF size_bytes, file_path, content, content_json ON artifact_version
    FOR EACH ROW
    EXECUTE FUNCTION notify_artifact_version_changed();

COMMENT ON FUNCTION notify_artifact_version_changed() IS 'Sends artifact update notifications when a version is created or finalized, including the producing execution id';

-- ============================================================================
-- QUEUE_STATS TABLE
-- ============================================================================

CREATE TABLE queue_stats (
    action_id BIGINT PRIMARY KEY REFERENCES action(id) ON DELETE CASCADE,
    queue_length INTEGER NOT NULL DEFAULT 0,
    active_count INTEGER NOT NULL DEFAULT 0,
    max_concurrent INTEGER NOT NULL DEFAULT 1,
    oldest_enqueued_at TIMESTAMPTZ,
    total_enqueued BIGINT NOT NULL DEFAULT 0,
    total_completed BIGINT NOT NULL DEFAULT 0,
    last_updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes
CREATE INDEX idx_queue_stats_last_updated ON queue_stats(last_updated);

-- Comments
COMMENT ON TABLE queue_stats IS 'Real-time queue statistics for action execution ordering';
COMMENT ON COLUMN queue_stats.action_id IS 'Foreign key to action table';
COMMENT ON COLUMN queue_stats.queue_length IS 'Number of executions waiting in queue';
COMMENT ON COLUMN queue_stats.active_count IS 'Number of currently running executions';
COMMENT ON COLUMN queue_stats.max_concurrent IS 'Maximum concurrent executions allowed';
COMMENT ON COLUMN queue_stats.oldest_enqueued_at IS 'Timestamp of oldest queued execution (NULL if queue empty)';
COMMENT ON COLUMN queue_stats.total_enqueued IS 'Total executions enqueued since queue creation';
COMMENT ON COLUMN queue_stats.total_completed IS 'Total executions completed since queue creation';
COMMENT ON COLUMN queue_stats.last_updated IS 'Timestamp of last statistics update';

-- ============================================================================
-- EXECUTION ADMISSION TABLES
-- ============================================================================

CREATE TABLE execution_admission_state (
    id BIGSERIAL PRIMARY KEY,
    action_id BIGINT NOT NULL REFERENCES action(id) ON DELETE CASCADE,
    group_key TEXT,
    group_key_normalized TEXT GENERATED ALWAYS AS (COALESCE(group_key, '')) STORED,
    max_concurrent INTEGER NOT NULL,
    next_queue_order BIGINT NOT NULL DEFAULT 1,
    total_enqueued BIGINT NOT NULL DEFAULT 0,
    total_completed BIGINT NOT NULL DEFAULT 0,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_execution_admission_state_identity
        UNIQUE (action_id, group_key_normalized)
);

CREATE TABLE execution_admission_entry (
    id BIGSERIAL PRIMARY KEY,
    state_id BIGINT NOT NULL REFERENCES execution_admission_state(id) ON DELETE CASCADE,
    execution_id BIGINT NOT NULL UNIQUE REFERENCES execution(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('active', 'queued')),
    queue_order BIGINT NOT NULL,
    enqueued_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    activated_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_execution_admission_state_action
    ON execution_admission_state (action_id);

CREATE INDEX idx_execution_admission_entry_state_status_queue
    ON execution_admission_entry (state_id, status, queue_order);

CREATE INDEX idx_execution_admission_entry_execution
    ON execution_admission_entry (execution_id);

CREATE TRIGGER update_execution_admission_state_updated
    BEFORE UPDATE ON execution_admission_state
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

CREATE TRIGGER update_execution_admission_entry_updated
    BEFORE UPDATE ON execution_admission_entry
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

COMMENT ON TABLE execution_admission_state IS
    'Shared admission state per action/group for executor concurrency and FIFO coordination';
COMMENT ON COLUMN execution_admission_state.group_key IS
    'Optional parameter-derived concurrency grouping key';
COMMENT ON COLUMN execution_admission_state.max_concurrent IS
    'Current concurrency limit for this action/group queue';
COMMENT ON COLUMN execution_admission_state.next_queue_order IS
    'Monotonic sequence used to preserve exact FIFO order for queued executions';
COMMENT ON COLUMN execution_admission_state.total_enqueued IS
    'Cumulative number of executions admitted into this queue';
COMMENT ON COLUMN execution_admission_state.total_completed IS
    'Cumulative number of active executions released from this queue';

COMMENT ON TABLE execution_admission_entry IS
    'Active slot ownership and queued executions for shared admission control';
COMMENT ON COLUMN execution_admission_entry.status IS
    'active rows own a concurrency slot; queued rows wait in FIFO order';
COMMENT ON COLUMN execution_admission_entry.queue_order IS
    'Durable FIFO position within an action/group queue';

-- ============================================================================
-- PACK ENVIRONMENT TABLE
-- ============================================================================

CREATE TABLE IF NOT EXISTS pack_environment (
    id BIGSERIAL PRIMARY KEY,
    pack BIGINT NOT NULL REFERENCES pack(id) ON DELETE CASCADE,
    pack_ref TEXT NOT NULL,
    runtime BIGINT NOT NULL REFERENCES runtime(id) ON DELETE CASCADE,
    runtime_ref TEXT NOT NULL,
    runtime_version BIGINT REFERENCES runtime_version(id) ON DELETE CASCADE,
    runtime_version_text TEXT,
    env_key TEXT GENERATED ALWAYS AS (
        CASE
            WHEN runtime_version IS NULL THEN 'base:' || pack::text || ':' || runtime::text
            ELSE 'version:' || pack::text || ':' || runtime::text || ':' || runtime_version::text
        END
    ) STORED,
    env_path TEXT NOT NULL,
    status pack_environment_status_enum NOT NULL DEFAULT 'pending',
    manifest_checksum TEXT,
    claimed_by_worker BIGINT REFERENCES worker(id) ON DELETE SET NULL,
    claim_expires_at TIMESTAMPTZ,
    installed_at TIMESTAMPTZ,
    last_verified TIMESTAMPTZ,
    install_log TEXT,
    install_error TEXT,
    metadata JSONB DEFAULT '{}'::jsonb,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(env_key)
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_pack_environment_pack ON pack_environment(pack);
CREATE INDEX IF NOT EXISTS idx_pack_environment_runtime ON pack_environment(runtime);
CREATE INDEX IF NOT EXISTS idx_pack_environment_runtime_version ON pack_environment(runtime_version);
CREATE INDEX IF NOT EXISTS idx_pack_environment_status ON pack_environment(status);
CREATE INDEX IF NOT EXISTS idx_pack_environment_pack_ref ON pack_environment(pack_ref);
CREATE INDEX IF NOT EXISTS idx_pack_environment_runtime_ref ON pack_environment(runtime_ref);
CREATE INDEX IF NOT EXISTS idx_pack_environment_pack_runtime ON pack_environment(pack, runtime);
CREATE INDEX IF NOT EXISTS idx_pack_environment_claim_expires_at ON pack_environment(claim_expires_at);
CREATE INDEX IF NOT EXISTS idx_pack_environment_claimed_by_worker ON pack_environment(claimed_by_worker);

-- Trigger for updated timestamp
CREATE TRIGGER update_pack_environment_updated
    BEFORE UPDATE ON pack_environment
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

-- Comments
COMMENT ON TABLE pack_environment IS 'Tracks pack-specific runtime environments for dependency isolation';
COMMENT ON COLUMN pack_environment.pack IS 'Pack that owns this environment';
COMMENT ON COLUMN pack_environment.pack_ref IS 'Pack reference for quick lookup';
COMMENT ON COLUMN pack_environment.runtime IS 'Runtime used for this environment';
COMMENT ON COLUMN pack_environment.runtime_ref IS 'Runtime reference for quick lookup';
COMMENT ON COLUMN pack_environment.runtime_version IS 'Optional runtime_version row for version-specific environments; NULL for the base runtime environment';
COMMENT ON COLUMN pack_environment.runtime_version_text IS 'Display/runtime version string for version-specific environments';
COMMENT ON COLUMN pack_environment.env_key IS 'Generated unique coordination key for the shared environment target';
COMMENT ON COLUMN pack_environment.env_path IS 'Filesystem path to the environment directory (e.g., /opt/attune/packenvs/mypack/python)';
COMMENT ON COLUMN pack_environment.status IS 'Current installation status';
COMMENT ON COLUMN pack_environment.manifest_checksum IS 'Checksum of the dependency manifest used to produce the installed environment';
COMMENT ON COLUMN pack_environment.claimed_by_worker IS 'Worker currently holding the install lease for this environment target';
COMMENT ON COLUMN pack_environment.claim_expires_at IS 'Lease expiry for the current install claim; expired claims may be reclaimed by another worker';
COMMENT ON COLUMN pack_environment.installed_at IS 'When the environment was successfully installed';
COMMENT ON COLUMN pack_environment.last_verified IS 'Last time the environment was verified as working';
COMMENT ON COLUMN pack_environment.install_log IS 'Installation output logs';
COMMENT ON COLUMN pack_environment.install_error IS 'Error message if installation failed';
COMMENT ON COLUMN pack_environment.metadata IS 'Additional metadata (installed packages, versions, etc.)';

-- ============================================================================
-- PACK ENVIRONMENT: Update existing runtimes with installer metadata
-- ============================================================================

-- Python runtime installers
UPDATE runtime
SET installers = jsonb_build_object(
    'base_path_template', '/opt/attune/packenvs/{pack_ref}/{runtime_name_lower}',
    'installers', jsonb_build_array(
        jsonb_build_object(
            'name', 'create_venv',
            'description', 'Create Python virtual environment',
            'command', 'python3',
            'args', jsonb_build_array('-m', 'venv', '{env_path}'),
            'cwd', '{pack_path}',
            'env', jsonb_build_object(),
            'order', 1,
            'optional', false
        ),
        jsonb_build_object(
            'name', 'upgrade_pip',
            'description', 'Upgrade pip to latest version',
            'command', '{env_path}/bin/pip',
            'args', jsonb_build_array('install', '--upgrade', 'pip'),
            'cwd', '{pack_path}',
            'env', jsonb_build_object(),
            'order', 2,
            'optional', true
        ),
        jsonb_build_object(
            'name', 'install_requirements',
            'description', 'Install pack Python dependencies',
            'command', '{env_path}/bin/pip',
            'args', jsonb_build_array('install', '-r', '{pack_path}/requirements.txt'),
            'cwd', '{pack_path}',
            'env', jsonb_build_object(),
            'order', 3,
            'optional', false,
            'condition', jsonb_build_object(
                'file_exists', '{pack_path}/requirements.txt'
            )
        )
    ),
    'executable_templates', jsonb_build_object(
        'python', '{env_path}/bin/python',
        'pip', '{env_path}/bin/pip'
    )
)
WHERE ref = 'core.python';

-- Node.js runtime installers
UPDATE runtime
SET installers = jsonb_build_object(
    'base_path_template', '/opt/attune/packenvs/{pack_ref}/{runtime_name_lower}',
    'installers', jsonb_build_array(
        jsonb_build_object(
            'name', 'npm_install',
            'description', 'Install Node.js dependencies',
            'command', 'npm',
            'args', jsonb_build_array('install', '--prefix', '{env_path}'),
            'cwd', '{pack_path}',
            'env', jsonb_build_object(
                'NODE_PATH', '{env_path}/node_modules'
            ),
            'order', 1,
            'optional', false,
            'condition', jsonb_build_object(
                'file_exists', '{pack_path}/package.json'
            )
        )
    ),
    'executable_templates', jsonb_build_object(
        'node', 'node',
        'npm', 'npm'
    ),
    'env_vars', jsonb_build_object(
        'NODE_PATH', '{env_path}/node_modules'
    )
)
WHERE ref = 'core.nodejs';

-- Shell runtime (no environment needed, uses system shell)
UPDATE runtime
SET installers = jsonb_build_object(
    'base_path_template', '/opt/attune/packenvs/{pack_ref}/{runtime_name_lower}',
    'installers', jsonb_build_array(),
    'executable_templates', jsonb_build_object(
        'sh', 'sh',
        'bash', 'bash'
    ),
    'requires_environment', false
)
WHERE ref = 'core.shell';

-- Native runtime (no environment needed, binaries are standalone)
UPDATE runtime
SET installers = jsonb_build_object(
    'base_path_template', '/opt/attune/packenvs/{pack_ref}/{runtime_name_lower}',
    'installers', jsonb_build_array(),
    'executable_templates', jsonb_build_object(),
    'requires_environment', false
)
WHERE ref = 'core.native';

-- Built-in sensor runtime (internal, no environment)
UPDATE runtime
SET installers = jsonb_build_object(
    'installers', jsonb_build_array(),
    'requires_environment', false
)
WHERE ref = 'core.sensor.builtin';

-- ============================================================================
-- PACK ENVIRONMENT: Helper functions
-- ============================================================================

-- Function to get environment path for a pack/runtime combination
CREATE OR REPLACE FUNCTION get_pack_environment_path(p_pack_ref TEXT, p_runtime_ref TEXT)
RETURNS TEXT AS $$
DECLARE
    v_runtime_name TEXT;
    v_base_template TEXT;
    v_result TEXT;
BEGIN
    -- Get runtime name and base path template
    SELECT
        LOWER(name),
        installers->>'base_path_template'
    INTO v_runtime_name, v_base_template
    FROM runtime
    WHERE ref = p_runtime_ref;

    IF v_base_template IS NULL THEN
        v_base_template := '/opt/attune/packenvs/{pack_ref}/{runtime_name_lower}';
    END IF;

    -- Replace template variables
    v_result := v_base_template;
    v_result := REPLACE(v_result, '{pack_ref}', p_pack_ref);
    v_result := REPLACE(v_result, '{runtime_ref}', p_runtime_ref);
    v_result := REPLACE(v_result, '{runtime_name_lower}', v_runtime_name);

    RETURN v_result;
END;
$$ LANGUAGE plpgsql IMMUTABLE;

COMMENT ON FUNCTION get_pack_environment_path IS 'Calculate the filesystem path for a pack runtime environment';

-- Function to check if a runtime requires an environment
CREATE OR REPLACE FUNCTION runtime_requires_environment(p_runtime_ref TEXT)
RETURNS BOOLEAN AS $$
DECLARE
    v_requires BOOLEAN;
BEGIN
    SELECT COALESCE((installers->>'requires_environment')::boolean, true)
    INTO v_requires
    FROM runtime
    WHERE ref = p_runtime_ref;

    RETURN COALESCE(v_requires, false);
END;
$$ LANGUAGE plpgsql STABLE;

COMMENT ON FUNCTION runtime_requires_environment IS 'Check if a runtime needs a pack-specific environment';

-- ============================================================================
-- PACK ENVIRONMENT: Status view
-- ============================================================================

CREATE OR REPLACE VIEW v_pack_environment_status AS
SELECT
    pe.id,
    pe.pack,
    p.ref AS pack_ref,
    p.label AS pack_name,
    pe.runtime,
    r.ref AS runtime_ref,
    r.name AS runtime_name,
    pe.env_path,
    pe.status,
    pe.installed_at,
    pe.last_verified,
    CASE
        WHEN pe.status = 'ready' AND pe.last_verified < NOW() - INTERVAL '7 days' THEN true
        ELSE false
    END AS needs_verification,
    CASE
        WHEN pe.status = 'ready' THEN 'healthy'
        WHEN pe.status = 'failed' THEN 'unhealthy'
        WHEN pe.status IN ('pending', 'installing') THEN 'provisioning'
        WHEN pe.status = 'outdated' THEN 'needs_update'
        ELSE 'unknown'
    END AS health_status,
    pe.install_error,
    pe.created,
    pe.updated
FROM pack_environment pe
JOIN pack p ON pe.pack = p.id
JOIN runtime r ON pe.runtime = r.id;

COMMENT ON VIEW v_pack_environment_status IS 'Consolidated view of pack environment status with health indicators';

-- ============================================================================
-- PACK TEST EXECUTION TABLE
-- ============================================================================

CREATE TABLE IF NOT EXISTS pack_test_execution (
    id BIGSERIAL PRIMARY KEY,
    pack_id BIGINT NOT NULL REFERENCES pack(id) ON DELETE CASCADE,
    pack_version VARCHAR(50) NOT NULL,
    execution_time TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    trigger_reason VARCHAR(50) NOT NULL, -- 'install', 'update', 'manual', 'validation'
    total_tests INT NOT NULL,
    passed INT NOT NULL,
    failed INT NOT NULL,
    skipped INT NOT NULL,
    pass_rate DECIMAL(5,4) NOT NULL, -- 0.0000 to 1.0000
    duration_ms BIGINT NOT NULL,
    result JSONB NOT NULL, -- Full test result structure
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT valid_test_counts CHECK (total_tests >= 0 AND passed >= 0 AND failed >= 0 AND skipped >= 0),
    CONSTRAINT valid_pass_rate CHECK (pass_rate >= 0.0 AND pass_rate <= 1.0),
    CONSTRAINT valid_trigger_reason CHECK (trigger_reason IN ('install', 'update', 'manual', 'validation'))
);

-- Indexes for efficient queries
CREATE INDEX idx_pack_test_execution_pack_id ON pack_test_execution(pack_id);
CREATE INDEX idx_pack_test_execution_time ON pack_test_execution(execution_time DESC);
CREATE INDEX idx_pack_test_execution_pass_rate ON pack_test_execution(pass_rate);
CREATE INDEX idx_pack_test_execution_trigger ON pack_test_execution(trigger_reason);

-- Comments for documentation
COMMENT ON TABLE pack_test_execution IS 'Tracks pack test execution results for validation and auditing';
COMMENT ON COLUMN pack_test_execution.pack_id IS 'Reference to the pack being tested';
COMMENT ON COLUMN pack_test_execution.pack_version IS 'Version of the pack at test time';
COMMENT ON COLUMN pack_test_execution.trigger_reason IS 'What triggered the test: install, update, manual, validation';
COMMENT ON COLUMN pack_test_execution.pass_rate IS 'Percentage of tests passed (0.0 to 1.0)';
COMMENT ON COLUMN pack_test_execution.result IS 'Full JSON structure with detailed test results';

-- Pack test result summary view (all test executions with pack info)
CREATE OR REPLACE VIEW pack_test_summary AS
SELECT
    p.id AS pack_id,
    p.ref AS pack_ref,
    p.label AS pack_label,
    pte.id AS test_execution_id,
    pte.pack_version,
    pte.execution_time AS test_time,
    pte.trigger_reason,
    pte.total_tests,
    pte.passed,
    pte.failed,
    pte.skipped,
    pte.pass_rate,
    pte.duration_ms,
    ROW_NUMBER() OVER (PARTITION BY p.id ORDER BY pte.execution_time DESC) AS rn
FROM pack p
LEFT JOIN pack_test_execution pte ON p.id = pte.pack_id
WHERE pte.id IS NOT NULL;

COMMENT ON VIEW pack_test_summary IS 'Summary of all pack test executions with pack details';

-- Latest test results per pack view
CREATE OR REPLACE VIEW pack_latest_test AS
SELECT
    pack_id,
    pack_ref,
    pack_label,
    test_execution_id,
    pack_version,
    test_time,
    trigger_reason,
    total_tests,
    passed,
    failed,
    skipped,
    pass_rate,
    duration_ms
FROM pack_test_summary
WHERE rn = 1;

COMMENT ON VIEW pack_latest_test IS 'Latest test results for each pack';

-- Function to get pack test statistics
CREATE OR REPLACE FUNCTION get_pack_test_stats(p_pack_id BIGINT)
RETURNS TABLE (
    total_executions BIGINT,
    successful_executions BIGINT,
    failed_executions BIGINT,
    avg_pass_rate DECIMAL,
    avg_duration_ms BIGINT,
    last_test_time TIMESTAMPTZ,
    last_test_passed BOOLEAN
) AS $$
BEGIN
    RETURN QUERY
    SELECT
        COUNT(*)::BIGINT AS total_executions,
        COUNT(*) FILTER (WHERE passed = total_tests)::BIGINT AS successful_executions,
        COUNT(*) FILTER (WHERE failed > 0)::BIGINT AS failed_executions,
        AVG(pass_rate) AS avg_pass_rate,
        AVG(duration_ms)::BIGINT AS avg_duration_ms,
        MAX(execution_time) AS last_test_time,
        (SELECT failed = 0 FROM pack_test_execution
         WHERE pack_id = p_pack_id
         ORDER BY execution_time DESC
         LIMIT 1) AS last_test_passed
    FROM pack_test_execution
    WHERE pack_id = p_pack_id;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION get_pack_test_stats IS 'Get statistical summary of test executions for a pack';

-- Function to check if pack has recent passing tests
CREATE OR REPLACE FUNCTION pack_has_passing_tests(
    p_pack_id BIGINT,
    p_hours_ago INT DEFAULT 24
)
RETURNS BOOLEAN AS $$
DECLARE
    v_has_passing_tests BOOLEAN;
BEGIN
    SELECT EXISTS(
        SELECT 1
        FROM pack_test_execution
        WHERE pack_id = p_pack_id
        AND execution_time > NOW() - (p_hours_ago || ' hours')::INTERVAL
        AND failed = 0
        AND total_tests > 0
    ) INTO v_has_passing_tests;

    RETURN v_has_passing_tests;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION pack_has_passing_tests IS 'Check if pack has recent passing test executions';

-- Add trigger to update pack metadata on test execution
CREATE OR REPLACE FUNCTION update_pack_test_metadata()
RETURNS TRIGGER AS $$
BEGIN
    -- Could update pack table with last_tested timestamp if we add that column
    -- For now, just a placeholder for future functionality
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trigger_update_pack_test_metadata
    AFTER INSERT ON pack_test_execution
    FOR EACH ROW
    EXECUTE FUNCTION update_pack_test_metadata();

COMMENT ON TRIGGER trigger_update_pack_test_metadata ON pack_test_execution IS 'Updates pack metadata when tests are executed';

-- ============================================================================
-- WEBHOOK FUNCTIONS
-- ============================================================================

-- Drop existing functions to avoid signature conflicts
DROP FUNCTION IF EXISTS enable_trigger_webhook(BIGINT, JSONB);
DROP FUNCTION IF EXISTS enable_trigger_webhook(BIGINT);
DROP FUNCTION IF EXISTS disable_trigger_webhook(BIGINT);
DROP FUNCTION IF EXISTS regenerate_trigger_webhook_key(BIGINT);

-- Function to enable webhooks for a trigger
CREATE OR REPLACE FUNCTION enable_trigger_webhook(
    p_trigger_id BIGINT,
    p_config JSONB DEFAULT '{}'::jsonb
)
RETURNS TABLE(
    webhook_enabled BOOLEAN,
    webhook_key VARCHAR(255),
    webhook_url TEXT
) AS $$
DECLARE
    v_webhook_key VARCHAR(255);
    v_api_base_url TEXT := 'http://localhost:8080'; -- Default, should be configured
BEGIN
    -- Check if trigger exists
    IF NOT EXISTS (SELECT 1 FROM trigger WHERE id = p_trigger_id) THEN
        RAISE EXCEPTION 'Trigger with id % does not exist', p_trigger_id;
    END IF;

    -- Generate webhook key if one doesn't exist
    SELECT t.webhook_key INTO v_webhook_key
    FROM trigger t
    WHERE t.id = p_trigger_id;

    IF v_webhook_key IS NULL THEN
        v_webhook_key := generate_webhook_key();
    END IF;

    -- Update trigger to enable webhooks
    UPDATE trigger
    SET
        webhook_enabled = TRUE,
        webhook_key = v_webhook_key,
        webhook_config = p_config,
        updated = NOW()
    WHERE id = p_trigger_id;

    -- Return webhook details
    RETURN QUERY SELECT
        TRUE,
        v_webhook_key,
        v_api_base_url || '/api/v1/webhooks/' || v_webhook_key;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION enable_trigger_webhook(BIGINT, JSONB) IS
    'Enables webhooks for a trigger with optional configuration. Generates a new webhook key if one does not exist. Returns webhook details.';

-- Function to disable webhooks for a trigger
CREATE OR REPLACE FUNCTION disable_trigger_webhook(
    p_trigger_id BIGINT
)
RETURNS BOOLEAN AS $$
BEGIN
    -- Check if trigger exists
    IF NOT EXISTS (SELECT 1 FROM trigger WHERE id = p_trigger_id) THEN
        RAISE EXCEPTION 'Trigger with id % does not exist', p_trigger_id;
    END IF;

    -- Update trigger to disable webhooks
    -- Set webhook_key to NULL when disabling to remove it from API responses
    UPDATE trigger
    SET
        webhook_enabled = FALSE,
        webhook_key = NULL,
        updated = NOW()
    WHERE id = p_trigger_id;

    RETURN TRUE;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION disable_trigger_webhook(BIGINT) IS
    'Disables webhooks for a trigger. Webhook key is removed when disabled.';

-- Function to regenerate webhook key for a trigger
CREATE OR REPLACE FUNCTION regenerate_trigger_webhook_key(
    p_trigger_id BIGINT
)
RETURNS TABLE(
    webhook_key VARCHAR(255),
    previous_key_revoked BOOLEAN
) AS $$
DECLARE
    v_new_key VARCHAR(255);
    v_old_key VARCHAR(255);
    v_webhook_enabled BOOLEAN;
BEGIN
    -- Check if trigger exists
    IF NOT EXISTS (SELECT 1 FROM trigger WHERE id = p_trigger_id) THEN
        RAISE EXCEPTION 'Trigger with id % does not exist', p_trigger_id;
    END IF;

    -- Get current webhook state
    SELECT t.webhook_key, t.webhook_enabled INTO v_old_key, v_webhook_enabled
    FROM trigger t
    WHERE t.id = p_trigger_id;

    -- Check if webhooks are enabled
    IF NOT v_webhook_enabled THEN
        RAISE EXCEPTION 'Webhooks are not enabled for trigger %', p_trigger_id;
    END IF;

    -- Generate new key
    v_new_key := generate_webhook_key();

    -- Update trigger with new key
    UPDATE trigger
    SET
        webhook_key = v_new_key,
        updated = NOW()
    WHERE id = p_trigger_id;

    -- Return new key and whether old key was present
    RETURN QUERY SELECT
        v_new_key,
        (v_old_key IS NOT NULL);
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION regenerate_trigger_webhook_key(BIGINT) IS
    'Regenerates webhook key for a trigger. Returns new key and whether a previous key was revoked.';

-- Verify all webhook functions exist
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_proc p
        JOIN pg_namespace n ON p.pronamespace = n.oid
        WHERE n.nspname = current_schema()
        AND p.proname = 'enable_trigger_webhook'
    ) THEN
        RAISE EXCEPTION 'enable_trigger_webhook function not found after migration';
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_proc p
        JOIN pg_namespace n ON p.pronamespace = n.oid
        WHERE n.nspname = current_schema()
        AND p.proname = 'disable_trigger_webhook'
    ) THEN
        RAISE EXCEPTION 'disable_trigger_webhook function not found after migration';
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_proc p
        JOIN pg_namespace n ON p.pronamespace = n.oid
        WHERE n.nspname = current_schema()
        AND p.proname = 'regenerate_trigger_webhook_key'
    ) THEN
        RAISE EXCEPTION 'regenerate_trigger_webhook_key function not found after migration';
    END IF;

    RAISE NOTICE 'All webhook functions successfully created';
END $$;
