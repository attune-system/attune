-- Prospective deployment-owned generic writers. This is NOT fabricated legacy
-- history: registration precedes the first request and pins the real producer.
ALTER TABLE file_evidence_put DROP CONSTRAINT file_evidence_put_protocol_check;
ALTER TABLE file_evidence_put ADD CONSTRAINT file_evidence_put_protocol_check
    CHECK(protocol IN ('legacy_unknown','atomic_create_v1','controlled_native_v1'));

CREATE TABLE file_native_generation (
    generation UUID PRIMARY KEY,
    run_id TEXT NOT NULL CHECK(length(run_id) BETWEEN 1 AND 20),
    project TEXT NOT NULL CHECK(length(project) BETWEEN 1 AND 128),
    mode TEXT NOT NULL CHECK(mode IN ('blob','shared','blob_and_shared')),
    operator BIGINT NOT NULL,
    native_instance UUID NOT NULL UNIQUE,
    worker_instance UUID NOT NULL UNIQUE,
    execution_attempt UUID NOT NULL UNIQUE,
    scope JSONB NOT NULL,
    origin JSONB NOT NULL,
    origin_digest TEXT NOT NULL,
    action_ref TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE file_native_operation (
    operation UUID PRIMARY KEY,
    generation UUID NOT NULL UNIQUE REFERENCES file_native_generation(generation),
    version BIGINT NOT NULL UNIQUE,
    artifact BIGINT NOT NULL,
    artifact_version INTEGER NOT NULL,
    execution BIGINT NOT NULL,
    admission BIGINT NOT NULL,
    worker BIGINT NOT NULL,
    source_receipt BIGINT NOT NULL CHECK(source_receipt>0),
    member JSONB NOT NULL,
    member_digest TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TRIGGER immutable BEFORE UPDATE OR DELETE ON file_native_generation
FOR EACH ROW EXECUTE FUNCTION guard_worker_delivery_immutable();
CREATE TRIGGER immutable BEFORE UPDATE OR DELETE ON file_native_operation
FOR EACH ROW EXECUTE FUNCTION guard_worker_delivery_immutable();

CREATE FUNCTION guard_original_native_file_operation() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT file_evidence_graph_held(false) THEN RAISE EXCEPTION 'Native operation must enter graph admission first'; END IF;
    IF NEW.protocol='controlled_native_v1' THEN
        IF NOT EXISTS (
            SELECT 1 FROM file_native_operation n JOIN file_native_generation g ON g.generation=n.generation
            JOIN file_evidence_owner o ON o.version=n.version
            JOIN artifact_file_origin f ON f.artifact_version=o.version AND f.origin_read=o.root_read
            JOIN worker_delivery_permit p ON p.artifact_version=o.version
            JOIN worker_delivery_context c ON c.token=p.context AND c.execution=n.execution
            JOIN runtime_authority_execution_fence e ON e.execution=n.execution AND e.admission=n.admission
            WHERE n.operation=NEW.attempt AND n.version=NEW.owner AND n.execution=NEW.execution
              AND NOT o.metadata_gone AND o.cleanup_group IS NULL
              AND o.evidence_state='retained' AND n.artifact=o.artifact
              AND f.admission=n.admission AND c.admission=n.admission AND c.worker=n.worker
              AND c.attempt=NEW.execution_attempt AND e.attempt=NEW.execution_attempt
              AND c.worker_instance=NEW.worker_instance AND NEW.native_instance=g.native_instance
              AND NEW.worker_instance=g.worker_instance AND NEW.execution_attempt=g.execution_attempt
              AND n.member->>'operation'=NEW.attempt::text
              AND (n.member->>'version')::bigint=o.version
              AND (n.member->>'root_read')::bigint=o.root_read
              AND n.member->>'object_key' IS NOT DISTINCT FROM o.object_key
              AND n.member->>'staging_path' IS NOT DISTINCT FROM o.staging_path
              AND n.member->>'provider_version' IS NOT DISTINCT FROM o.provider_version
              AND (n.member->>'execution_attempt')::uuid=NEW.execution_attempt
              AND (n.member->>'worker_instance')::uuid=NEW.worker_instance
              AND (n.member->>'native_instance')::uuid=NEW.native_instance
              AND EXISTS(SELECT 1 FROM file_evidence_pin pin WHERE pin.owner=o.version
                  AND pin.kind='runtime_input_receipt' AND pin.source=n.source_receipt)
              AND ((g.mode='blob' AND o.object_key IS NOT NULL AND o.staging_path IS NULL)
                OR (g.mode='shared' AND o.object_key IS NULL AND o.staging_path IS NOT NULL)
                OR (g.mode='blob_and_shared' AND o.object_key IS NOT NULL AND o.staging_path IS NOT NULL))
        ) THEN RAISE EXCEPTION 'Original native generation/producer binding unavailable'; END IF;
    ELSIF EXISTS(SELECT 1 FROM file_native_operation WHERE version=NEW.owner) THEN
        RAISE EXCEPTION 'A controlled native file cannot acquire an unrelated writer protocol';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER original_native_operation BEFORE INSERT ON file_evidence_put
FOR EACH ROW EXECUTE FUNCTION guard_original_native_file_operation();

CREATE OR REPLACE FUNCTION file_evidence_guard_put_retirement() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT file_evidence_graph_held(false) THEN RAISE EXCEPTION 'PUT retirement must enter graph admission first'; END IF;
    -- A success/HEAD/atomic sentinel never settles the controlled generic writer.
    -- ALL requests and staging children need the original owning native audit.
    IF OLD.protocol='controlled_native_v1' THEN
        IF file_evidence_graph_held(true) AND EXISTS(
            SELECT 1 FROM file_native_operation n JOIN file_native_generation g ON g.generation=n.generation
            JOIN file_legacy_audit_member m ON m.operation=n.operation AND m.version=n.version
            JOIN file_legacy_audit a ON a.id=m.audit AND a.state='completed'
            JOIN file_legacy_audit_evidence e ON e.audit=a.id AND e.kind='settled' AND e.identity=a.plan_digest
            WHERE n.operation=OLD.attempt AND n.version=OLD.owner AND a.plan->'scope'=g.scope
              AND EXISTS(SELECT 1 FROM jsonb_array_elements(a.plan->'members') member WHERE member=n.member)
        ) THEN RETURN OLD; END IF;
        RAISE EXCEPTION 'Original controlled native audit settlement missing';
    END IF;
    -- Preserve the validated atomic and genuinely unknown historical paths.
    IF EXISTS(SELECT 1 FROM file_evidence_put_observation r JOIN file_evidence_owner o ON o.version=OLD.owner
        WHERE r.attempt=OLD.attempt AND r.object_key=o.object_key AND r.provider_version=o.provider_version) THEN RETURN OLD; END IF;
    IF OLD.protocol='atomic_create_v1' AND EXISTS(SELECT 1 FROM file_evidence_write_fence f
        WHERE f.version=OLD.owner AND f.confirmed) AND file_evidence_graph_held(true) THEN RETURN OLD; END IF;
    IF OLD.protocol='legacy_unknown' AND file_evidence_graph_held(true) AND EXISTS(
        SELECT 1 FROM file_legacy_audit_member m JOIN file_legacy_audit a ON a.id=m.audit
        JOIN file_legacy_audit_evidence e ON e.audit=a.id AND e.kind='settled' AND e.identity=a.plan_digest
        WHERE m.operation=OLD.attempt AND m.version=OLD.owner AND a.state='completed'
    ) THEN RETURN OLD; END IF;
    RAISE EXCEPTION 'Native PUT settlement missing; cancellation/age/HEAD absence is not proof';
END $$;

CREATE OR REPLACE VIEW file_evidence_put_diagnostic AS
SELECT p.attempt,p.owner,o.object_key,p.execution,p.execution_attempt,p.worker_instance,p.native_instance,p.protocol,
    CASE WHEN p.protocol='legacy_unknown' THEN
        'Legacy native writer/staging identity missing; stop and audit the original writer and provider multipart staging; HEAD absence and TTL cannot settle this operation'
    WHEN p.protocol='controlled_native_v1' THEN
        'Original controlled native generation registered; require its authenticated writer/provider settlement audit before physical release'
    ELSE COALESCE(p.diagnostic,'Atomic native operation unresolved; exact-provider key fencing required after metadata cleanup') END AS diagnostic
FROM file_evidence_put p JOIN file_evidence_owner o ON o.version=p.owner;
