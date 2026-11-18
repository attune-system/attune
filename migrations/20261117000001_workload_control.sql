-- Monotonic revocation evidence deliberately has no FK to retained sources.
CREATE TABLE workload_source_epoch (
    id BIGSERIAL PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('owner','execution','worker','workload','sensor','admission','install','image','engine')),
    subject BIGINT NOT NULL CHECK (subject > 0),
    epoch BIGINT NOT NULL DEFAULT 0 CHECK (epoch >= 0),
    UNIQUE(kind, subject)
);
CREATE FUNCTION guard_workload_source_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(NEW.id,NEW.kind,NEW.subject) IS DISTINCT FROM ROW(OLD.id,OLD.kind,OLD.subject)
        OR NEW.epoch<=OLD.epoch THEN RAISE EXCEPTION 'Custody revocation epochs are monotonic'; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER workload_source_epoch_guard BEFORE UPDATE ON workload_source_epoch
FOR EACH ROW EXECUTE FUNCTION guard_workload_source_epoch();
CREATE FUNCTION bump_workload_source_epoch(source_kind TEXT, source_id BIGINT) RETURNS VOID LANGUAGE SQL AS $$
    INSERT INTO workload_source_epoch(kind,subject,epoch) VALUES(source_kind,source_id,1)
    ON CONFLICT(kind,subject) DO UPDATE SET epoch=workload_source_epoch.epoch+1
$$;

-- Durable admission for remote custody preparation. Tombstones survive source
-- and artifact retention; absence of a network acknowledgement is never proof
-- that a request cannot still arrive. No foreign keys to runtime parents.
CREATE TABLE log_custody_operation (
    id BIGSERIAL PRIMARY KEY,
    custody UUID NOT NULL UNIQUE CHECK (custody <> '00000000-0000-0000-0000-000000000000'::uuid),
    execution BIGINT NOT NULL CHECK (execution > 0),
    artifact_version BIGINT NOT NULL UNIQUE CHECK (artifact_version > 0),
    stream BIGINT NOT NULL CHECK (stream > 0),
    worker BIGINT NOT NULL CHECK (worker > 0),
    source_instance UUID NOT NULL CHECK (source_instance <> '00000000-0000-0000-0000-000000000000'::uuid),
    attempt UUID NOT NULL CHECK (attempt <> '00000000-0000-0000-0000-000000000000'::uuid),
    owner BIGINT CHECK (owner > 0),
    owner_epoch BIGINT NOT NULL DEFAULT 0 CHECK (owner_epoch >= 0),
    execution_epoch BIGINT NOT NULL DEFAULT 0 CHECK (execution_epoch >= 0),
    worker_epoch BIGINT NOT NULL DEFAULT 0 CHECK (worker_epoch >= 0),
    finalize_by TIMESTAMPTZ NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('reserved','prepared','cancelled')),
    payload_sha256 TEXT CHECK (payload_sha256 ~ '^[0-9a-f]{64}$'),
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (state = 'cancelled' OR owner IS NOT NULL),
    CHECK ((state = 'prepared') = (payload_sha256 IS NOT NULL))
);
CREATE INDEX log_custody_operation_worker_idx ON log_custody_operation(worker, finalize_by);
CREATE FUNCTION guard_log_custody_operation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(NEW.id, NEW.custody, NEW.execution, NEW.artifact_version, NEW.stream,
        NEW.worker, NEW.source_instance, NEW.attempt, NEW.owner, NEW.owner_epoch, NEW.execution_epoch, NEW.worker_epoch, NEW.finalize_by, NEW.created)
       IS DISTINCT FROM
       ROW(OLD.id, OLD.custody, OLD.execution, OLD.artifact_version, OLD.stream,
        OLD.worker, OLD.source_instance, OLD.attempt, OLD.owner, OLD.owner_epoch, OLD.execution_epoch, OLD.worker_epoch, OLD.finalize_by, OLD.created)
       OR NOT (NEW.state = OLD.state OR (OLD.state = 'reserved' AND NEW.state IN ('prepared','cancelled')))
       OR (OLD.payload_sha256 IS NOT NULL AND NEW.payload_sha256 IS DISTINCT FROM OLD.payload_sha256)
    THEN RAISE EXCEPTION 'Custody operation binding and terminal state are immutable'; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER log_custody_operation_guard BEFORE UPDATE ON log_custody_operation
FOR EACH ROW EXECUTE FUNCTION guard_log_custody_operation();

-- Source triggers update only monotonic epoch rows, never operation rows. Both
-- reserve and prepare lock sources, then epochs, then the operation. Restoring
-- a source cannot undo an epoch increment or revive an older reservation.
CREATE FUNCTION revoke_custody_owner_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='identity' THEN
        IF TG_OP='DELETE' OR ROW(OLD.frozen,OLD.attributes) IS DISTINCT FROM ROW(NEW.frozen,NEW.attributes) THEN
            PERFORM bump_workload_source_epoch('owner',OLD.id);
        END IF;
    ELSE
        PERFORM bump_workload_source_epoch('owner',OLD.identity);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER log_custody_owner_epoch AFTER UPDATE OR DELETE ON identity FOR EACH ROW EXECUTE FUNCTION revoke_custody_owner_epoch();
CREATE TRIGGER log_custody_permission_epoch AFTER UPDATE OR DELETE ON permission_assignment FOR EACH ROW EXECUTE FUNCTION revoke_custody_owner_epoch();
CREATE TRIGGER log_custody_role_epoch AFTER UPDATE OR DELETE ON identity_role_assignment FOR EACH ROW EXECUTE FUNCTION revoke_custody_owner_epoch();

CREATE FUNCTION revoke_custody_grant_epochs() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE affected BIGINT;
BEGIN
    IF TG_TABLE_NAME='permission_set' THEN
        FOR affected IN
            SELECT owner FROM execution_authority_selection WHERE OLD.ref=ANY(selected_refs) AND owner IS NOT NULL
            UNION SELECT identity FROM permission_assignment WHERE permset=OLD.id
            UNION SELECT a.identity FROM identity_role_assignment a JOIN permission_set_role_assignment r ON r.role=a.role WHERE r.permset=OLD.id
            ORDER BY 1
        LOOP PERFORM bump_workload_source_epoch('owner',affected); END LOOP;
    ELSE
        FOR affected IN SELECT DISTINCT identity FROM identity_role_assignment WHERE role=OLD.role ORDER BY identity
        LOOP PERFORM bump_workload_source_epoch('owner',affected); END LOOP;
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER log_custody_definition_epoch AFTER UPDATE OR DELETE ON permission_set FOR EACH ROW EXECUTE FUNCTION revoke_custody_grant_epochs();
CREATE TRIGGER log_custody_role_definition_epoch AFTER UPDATE OR DELETE ON permission_set_role_assignment FOR EACH ROW EXECUTE FUNCTION revoke_custody_grant_epochs();

CREATE FUNCTION revoke_custody_execution_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR ROW(OLD.worker,OLD.executor,OLD.action,OLD.executable_snapshot,OLD.permission_set_refs)
        IS DISTINCT FROM ROW(NEW.worker,NEW.executor,NEW.action,NEW.executable_snapshot,NEW.permission_set_refs) THEN
        PERFORM bump_workload_source_epoch('execution',OLD.id);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER log_custody_execution_epoch AFTER UPDATE OR DELETE ON execution FOR EACH ROW EXECUTE FUNCTION revoke_custody_execution_epoch();
CREATE FUNCTION revoke_custody_selection_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM bump_workload_source_epoch('execution',OLD.execution);
    RETURN NULL;
END $$;
CREATE TRIGGER log_custody_selection_epoch AFTER UPDATE OR DELETE ON execution_authority_selection FOR EACH ROW EXECUTE FUNCTION revoke_custody_selection_epoch();
CREATE FUNCTION revoke_custody_attempt_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR ROW(OLD.attempt,OLD.worker,OLD.producer_kind,OLD.closed)
        IS DISTINCT FROM ROW(NEW.attempt,NEW.worker,NEW.producer_kind,NEW.closed) THEN
        PERFORM bump_workload_source_epoch('execution',OLD.execution);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER log_custody_attempt_epoch AFTER UPDATE OR DELETE ON delegated_execution_attempt FOR EACH ROW EXECUTE FUNCTION revoke_custody_attempt_epoch();
CREATE FUNCTION revoke_custody_schema_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.entity_kind='execution' THEN PERFORM bump_workload_source_epoch('execution',OLD.entity_id); END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER log_custody_schema_epoch AFTER UPDATE OR DELETE ON runtime_schema_binding FOR EACH ROW EXECUTE FUNCTION revoke_custody_schema_epoch();
CREATE FUNCTION revoke_custody_artifact_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE affected BIGINT;
BEGIN
    IF TG_OP='DELETE' OR NEW IS DISTINCT FROM OLD THEN
        FOR affected IN SELECT DISTINCT execution FROM artifact_version WHERE artifact=OLD.artifact AND execution IS NOT NULL ORDER BY execution
        LOOP PERFORM bump_workload_source_epoch('execution',affected); END LOOP;
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER log_custody_artifact_epoch AFTER UPDATE OR DELETE ON artifact_runtime_owner_binding FOR EACH ROW EXECUTE FUNCTION revoke_custody_artifact_epoch();
CREATE FUNCTION revoke_custody_worker_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR (OLD.capabilities->>'log_delivery_instance') IS DISTINCT FROM (NEW.capabilities->>'log_delivery_instance')
        OR (OLD.meta->>'registration_incarnation') IS DISTINCT FROM (NEW.meta->>'registration_incarnation')
        OR (OLD.status IN ('active','busy')) IS DISTINCT FROM (NEW.status IN ('active','busy')) THEN
        PERFORM bump_workload_source_epoch('worker',OLD.id);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER log_custody_worker_epoch AFTER UPDATE OR DELETE ON worker FOR EACH ROW EXECUTE FUNCTION revoke_custody_worker_epoch();

-- Provider credentials live only at the trusted controller. Engine/pool identity
-- is operator-managed CP state, never a workload-selected socket or volume.
CREATE TABLE workload_engine (
    id BIGSERIAL PRIMARY KEY,
    pool TEXT NOT NULL UNIQUE CHECK (length(pool) BETWEEN 1 AND 128),
    controller_url TEXT NOT NULL,
    daemon_id TEXT NOT NULL CHECK (length(daemon_id) BETWEEN 1 AND 256 AND daemon_id ~ '^[!-~]+$'),
    platform TEXT NOT NULL CHECK (platform IN ('linux/amd64','linux/arm64')),
    disabled BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE FUNCTION guard_workload_engine_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR ROW(NEW.id,NEW.pool,NEW.daemon_id,NEW.platform)
      IS DISTINCT FROM ROW(OLD.id,OLD.pool,OLD.daemon_id,OLD.platform)
    THEN RAISE EXCEPTION 'Workload engine identity and native platform are immutable'; END IF;
    IF ROW(NEW.disabled,NEW.controller_url) IS DISTINCT FROM ROW(OLD.disabled,OLD.controller_url) THEN
        PERFORM bump_workload_source_epoch('engine',OLD.id);
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER workload_engine_identity BEFORE UPDATE OR DELETE ON workload_engine FOR EACH ROW EXECUTE FUNCTION guard_workload_engine_identity();
CREATE TABLE workload_controller (
    id BIGSERIAL PRIMARY KEY,
    engine BIGINT NOT NULL REFERENCES workload_engine(id),
    identity BIGINT NOT NULL REFERENCES identity(id),
    incarnation UUID NOT NULL UNIQUE,
    revoked_at TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE workload_execution_image (
    id BIGSERIAL PRIMARY KEY,
    engine BIGINT NOT NULL REFERENCES workload_engine(id),
    reference TEXT NOT NULL,
    sha256 TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    platform TEXT NOT NULL CHECK (platform IN ('linux/amd64','linux/arm64')),
    revoked_at TIMESTAMPTZ,
    UNIQUE(engine,reference,platform)
);

-- No FK from runtime launch evidence to its execution/sensor/pack/admission.
-- Cleanup and cancellation evidence must survive independent source retention.
CREATE TABLE workload_launch (
    id BIGSERIAL PRIMARY KEY,
    engine BIGINT NOT NULL CHECK (engine > 0),
    image BIGINT NOT NULL CHECK (image > 0),
    owner BIGINT NOT NULL CHECK (owner > 0),
    manifest JSONB NOT NULL CHECK (jsonb_typeof(manifest)='object'),
    admitted_digest TEXT NOT NULL CHECK (admitted_digest ~ '^[0-9a-f]{64}$'),
    encrypted_context TEXT NOT NULL,
    source_epochs JSONB NOT NULL CHECK (jsonb_typeof(source_epochs)='array' AND jsonb_array_length(source_epochs)>0),
    operation JSONB NOT NULL CHECK (jsonb_typeof(operation)='object'),
    encrypted_dispatch TEXT NOT NULL,
    dispatch_hash TEXT NOT NULL CHECK (dispatch_hash ~ '^[0-9a-f]{64}$'),
    lease_hash TEXT,
    controller BIGINT,
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK ((manifest->>'launch_id')::BIGINT=id),
    CHECK ((manifest->>'engine_id')::BIGINT=engine),
    CHECK ((manifest->>'owner_identity_id')::BIGINT=owner),
    CHECK (operation#>>'{status,request,admitted_digest}'=admitted_digest),
    CHECK ((operation#>>'{status,request,launch_id}')::BIGINT=id)
);
CREATE INDEX workload_launch_cleanup ON workload_launch(engine,updated)
WHERE operation#>>'{status,state}'<>'stopped' OR operation#>>'{status,publication}' IN ('pending','uploading') OR operation#>>'{status,capture}'='pending';
CREATE FUNCTION guard_workload_launch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(NEW.id,NEW.engine,NEW.image,NEW.owner,NEW.manifest,NEW.admitted_digest,
        NEW.encrypted_context,NEW.source_epochs,NEW.encrypted_dispatch,NEW.dispatch_hash,NEW.created)
       IS DISTINCT FROM ROW(OLD.id,OLD.engine,OLD.image,OLD.owner,OLD.manifest,OLD.admitted_digest,
        OLD.encrypted_context,OLD.source_epochs,OLD.encrypted_dispatch,OLD.dispatch_hash,OLD.created)
        OR (NEW.operation->>'lease_generation')::BIGINT < (OLD.operation->>'lease_generation')::BIGINT
        OR (OLD.operation->>'create_nonce' IS NOT NULL AND NEW.operation->>'create_nonce' IS DISTINCT FROM OLD.operation->>'create_nonce')
        OR (COALESCE((OLD.operation->>'stdin_authorized')::BOOLEAN,false) AND NOT COALESCE((NEW.operation->>'stdin_authorized')::BOOLEAN,false))
        OR (OLD.operation->>'start_nonce' IS NOT NULL AND NEW.operation->>'start_nonce' IS DISTINCT FROM OLD.operation->>'start_nonce')
        OR (OLD.operation#>>'{status,state}'='stopped' AND NEW.operation#>>'{status,state}'<>'stopped')
        OR (OLD.operation#>>'{status,capture}' IN ('complete','incomplete') AND NEW.operation#>>'{status,capture}' IS DISTINCT FROM OLD.operation#>>'{status,capture}')
        OR (OLD.operation#>'{status,capture_deadline}'<>'null'::jsonb AND NEW.operation#>'{status,capture_deadline}' IS DISTINCT FROM OLD.operation#>'{status,capture_deadline}')
        OR (OLD.operation#>>'{status,publication}' IN ('published','rejected','not_required') AND NEW.operation#>>'{status,publication}' IS DISTINCT FROM OLD.operation#>>'{status,publication}')
        OR (OLD.operation#>>'{status,stop_reason}' IS NOT NULL AND NEW.operation#>>'{status,stop_reason}' IS DISTINCT FROM OLD.operation#>>'{status,stop_reason}')
        OR (OLD.operation#>'{status,exit_outcome}'<>'null'::jsonb AND NEW.operation#>'{status,exit_outcome}' IS DISTINCT FROM OLD.operation#>'{status,exit_outcome}')
        OR (OLD.operation#>'{status,resource}'<>'null'::jsonb AND NEW.operation#>'{status,resource}' IS DISTINCT FROM OLD.operation#>'{status,resource}')
    THEN RAISE EXCEPTION 'Launch binding, fences and terminal evidence are immutable'; END IF;
    NEW.updated=clock_timestamp();
    RETURN NEW;
END $$;
CREATE TRIGGER workload_launch_guard BEFORE UPDATE ON workload_launch FOR EACH ROW EXECUTE FUNCTION guard_workload_launch();

CREATE FUNCTION revoke_workload_assignment_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR ROW(OLD.worker,OLD.worker_instance,OLD.generation) IS DISTINCT FROM ROW(NEW.worker,NEW.worker_instance,NEW.generation)
       OR OLD.lease_expires_at<=clock_timestamp() THEN
        PERFORM bump_workload_source_epoch('workload',OLD.workload);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER workload_assignment_epoch AFTER UPDATE OR DELETE ON sensor_workload_assignment FOR EACH ROW EXECUTE FUNCTION revoke_workload_assignment_epoch();
CREATE FUNCTION revoke_workload_runtime_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='runtime_authority_admission' THEN
        IF OLD.owner_kind='execution' THEN PERFORM bump_workload_source_epoch('execution',OLD.owner_id); END IF;
    ELSE
        PERFORM bump_workload_source_epoch('execution',OLD.execution);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER workload_runtime_admission_epoch AFTER UPDATE OR DELETE ON runtime_authority_admission FOR EACH ROW EXECUTE FUNCTION revoke_workload_runtime_epoch();
CREATE TRIGGER workload_runtime_attempt_epoch AFTER UPDATE OR DELETE ON runtime_authority_execution_fence FOR EACH ROW EXECUTE FUNCTION revoke_workload_runtime_epoch();
CREATE FUNCTION revoke_workload_image_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM bump_workload_source_epoch('image',OLD.id);
    RETURN NULL;
END $$;
CREATE TRIGGER workload_image_epoch AFTER UPDATE OR DELETE ON workload_execution_image FOR EACH ROW EXECUTE FUNCTION revoke_workload_image_epoch();

-- Deletion is intentionally denied. A future bounded reclamation protocol must
-- prove source/version absence AND expired deadlines before discarding evidence.
CREATE FUNCTION retain_workload_control_tombstone() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'Workload control tombstone reclamation is not admitted'; END $$;
CREATE TRIGGER workload_epoch_retain BEFORE DELETE ON workload_source_epoch FOR EACH ROW EXECUTE FUNCTION retain_workload_control_tombstone();
CREATE TRIGGER workload_launch_retain BEFORE DELETE ON workload_launch FOR EACH ROW EXECUTE FUNCTION retain_workload_control_tombstone();
CREATE TRIGGER log_custody_operation_retain BEFORE DELETE ON log_custody_operation FOR EACH ROW EXECUTE FUNCTION retain_workload_control_tombstone();

CREATE TABLE workload_launch_prerequisite (
    id BIGSERIAL PRIMARY KEY,
    purpose TEXT NOT NULL CHECK (purpose IN ('setup','runtime_probe')),
    attempt UUID NOT NULL,
    source_target JSONB NOT NULL CHECK (source_target->>'kind' IN ('execution','sensor','candidate_test')),
    stage_binding JSONB,
    image BIGINT NOT NULL CHECK (image>0),
    source_epochs JSONB NOT NULL CHECK (jsonb_typeof(source_epochs)='array' AND jsonb_array_length(source_epochs)>0),
    revoked_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE FUNCTION revoke_workload_prerequisite_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM bump_workload_source_epoch('admission',OLD.id);
    RETURN NULL;
END $$;
CREATE TRIGGER workload_prerequisite_epoch AFTER UPDATE OR DELETE ON workload_launch_prerequisite FOR EACH ROW EXECUTE FUNCTION revoke_workload_prerequisite_epoch();
CREATE TRIGGER workload_prerequisite_guard BEFORE UPDATE ON workload_launch_prerequisite FOR EACH ROW EXECUTE FUNCTION guard_runtime_authority_record();

-- Launch ZIP bodies are immutable, independently pinned objects. Creation is a
-- trusted materializer operation, not a worker upload or arbitrary URL proxy.
CREATE TABLE workload_launch_object (
    id BIGSERIAL PRIMARY KEY,
    source_target JSONB NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('pack','runtime','runner')),
    object_key TEXT NOT NULL UNIQUE,
    provider_version TEXT NOT NULL CHECK (length(provider_version)>0),
    size_bytes BIGINT NOT NULL CHECK (size_bytes>0),
    sha256 TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    max_expanded_bytes BIGINT NOT NULL CHECK (max_expanded_bytes>0),
    max_files BIGINT NOT NULL CHECK (max_files>0),
    retired_at TIMESTAMPTZ,
    built_image_sha256 TEXT CHECK (built_image_sha256 ~ '^[0-9a-f]{64}$'),
    build_launch BIGINT,
    stage_binding JSONB,
    validation_receipt BIGINT,
    CHECK ((kind='runtime')=(built_image_sha256 IS NOT NULL AND build_launch IS NOT NULL AND stage_binding IS NOT NULL AND validation_receipt IS NOT NULL))
);
CREATE FUNCTION guard_workload_launch_object() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (to_jsonb(NEW)-'retired_at') IS DISTINCT FROM (to_jsonb(OLD)-'retired_at')
      OR (OLD.retired_at IS NOT NULL AND NEW.retired_at IS DISTINCT FROM OLD.retired_at)
    THEN RAISE EXCEPTION 'Launch object pins are immutable'; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER workload_launch_object_guard BEFORE UPDATE ON workload_launch_object FOR EACH ROW EXECUTE FUNCTION guard_workload_launch_object();
CREATE TABLE workload_launch_io_grant (
    id BIGSERIAL PRIMARY KEY,
    launch BIGINT NOT NULL,
    channel TEXT NOT NULL CHECK (channel IN ('stdin','stdout','stderr')),
    token_hash TEXT NOT NULL,
    max_bytes BIGINT NOT NULL CHECK (max_bytes>0),
    expires_at TIMESTAMPTZ NOT NULL,
    UNIQUE(launch,channel)
);
CREATE TABLE workload_launch_output (
    id BIGSERIAL PRIMARY KEY,
    launch BIGINT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('stdout','stderr','runtime_result','candidate_result','stage_archive')),
    size_bytes BIGINT NOT NULL CHECK (size_bytes>0),
    sha256 TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    object_key TEXT NOT NULL UNIQUE,
    token_hash TEXT NOT NULL,
    encrypted_token TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('reserved','uploaded','published','rejected')),
    provider_version TEXT,
    validation_receipt BIGINT,
    object_id BIGINT,
    cleanup_token UUID NOT NULL,
    cleanup_required BOOLEAN NOT NULL DEFAULT FALSE,
    cleanup_settled BOOLEAN NOT NULL DEFAULT FALSE,
    cleanup_provider_version TEXT,
    expires_at TIMESTAMPTZ NOT NULL,
    lease_generation BIGINT NOT NULL CHECK (lease_generation>0),
    created TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(launch,kind),
    CHECK ((state='published')=(validation_receipt IS NOT NULL)),
    CHECK ((state='published')=(object_id IS NOT NULL)),
    CHECK (NOT cleanup_settled OR (cleanup_required AND cleanup_provider_version IS NOT NULL))
);
CREATE TRIGGER workload_output_retain BEFORE DELETE ON workload_launch_output FOR EACH ROW EXECUTE FUNCTION retain_workload_control_tombstone();
CREATE FUNCTION guard_workload_launch_output() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(NEW.id,NEW.launch,NEW.kind,NEW.size_bytes,NEW.sha256,NEW.object_key,NEW.cleanup_token,NEW.created)
      IS DISTINCT FROM ROW(OLD.id,OLD.launch,OLD.kind,OLD.size_bytes,OLD.sha256,OLD.object_key,OLD.cleanup_token,OLD.created)
      OR (OLD.state IN ('published','rejected') AND NEW.state IS DISTINCT FROM OLD.state)
      OR NEW.lease_generation<OLD.lease_generation
      OR (NEW.lease_generation=OLD.lease_generation AND ROW(NEW.token_hash,NEW.encrypted_token) IS DISTINCT FROM ROW(OLD.token_hash,OLD.encrypted_token))
      OR (OLD.cleanup_required AND NOT NEW.cleanup_required)
      OR (OLD.cleanup_settled AND NOT NEW.cleanup_settled)
      OR (OLD.cleanup_provider_version IS NOT NULL AND NEW.cleanup_provider_version IS DISTINCT FROM OLD.cleanup_provider_version)
      OR (OLD.provider_version IS NOT NULL AND NEW.provider_version IS DISTINCT FROM OLD.provider_version)
    THEN RAISE EXCEPTION 'Launch output payload and terminal evidence are immutable'; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER workload_output_guard BEFORE UPDATE ON workload_launch_output FOR EACH ROW EXECUTE FUNCTION guard_workload_launch_output();

CREATE INDEX workload_output_reconcile ON workload_launch_output(id) WHERE NOT cleanup_required;
CREATE INDEX workload_output_cleanup ON workload_launch_output(id) WHERE cleanup_required AND NOT cleanup_settled;
CREATE TABLE workload_launch_maintenance (
    id BIGINT PRIMARY KEY CHECK (id IN (1,2)),
    after_id BIGINT NOT NULL DEFAULT 0,
    high_water BIGINT NOT NULL DEFAULT 0,
    CHECK (after_id>=0 AND high_water>=after_id)
);
INSERT INTO workload_launch_maintenance(id) VALUES(1),(2);

CREATE TRIGGER workload_object_retain BEFORE DELETE ON workload_launch_object FOR EACH ROW EXECUTE FUNCTION retain_workload_control_tombstone();

CREATE FUNCTION revoke_workload_pack_owner_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR ROW(OLD.pack,OLD.identity) IS DISTINCT FROM ROW(NEW.pack,NEW.identity) THEN
        PERFORM bump_workload_source_epoch('owner',OLD.identity);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER workload_pack_owner_epoch AFTER UPDATE OR DELETE ON pack_runtime_identity FOR EACH ROW EXECUTE FUNCTION revoke_workload_pack_owner_epoch();
