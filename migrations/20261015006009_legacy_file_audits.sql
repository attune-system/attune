-- Explicit native/operator audits. These records certify cleanup only, never
-- reconstruct lost read authority or make a process credential live.
CREATE TABLE file_legacy_audit (
    id UUID PRIMARY KEY,
    operator BIGINT NOT NULL,
    plan JSONB NOT NULL CHECK(octet_length(plan::text)<=1048576),
    plan_digest TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'requested' CHECK(state IN ('requested','cataloguing','cleaning','completed','manual_required')),
    cursor TEXT,
    diagnostic TEXT
);
CREATE TABLE file_legacy_audit_member (
    audit UUID NOT NULL REFERENCES file_legacy_audit(id),
    operation UUID NOT NULL,
    version BIGINT NOT NULL,
    PRIMARY KEY(audit,operation)
);
CREATE INDEX file_legacy_audit_operation ON file_legacy_audit_member(operation);
CREATE TABLE file_legacy_audit_evidence (
    audit UUID NOT NULL REFERENCES file_legacy_audit(id),
    kind TEXT NOT NULL,
    identity TEXT NOT NULL,
    proof JSONB NOT NULL CHECK(octet_length(proof::text)<=1048576),
    PRIMARY KEY(audit,kind,identity)
);
CREATE TABLE file_legacy_audit_item (
    audit UUID NOT NULL REFERENCES file_legacy_audit(id),
    identity TEXT NOT NULL,
    item JSONB NOT NULL CHECK(octet_length(item::text)<=1048576),
    ownership TEXT NOT NULL CHECK(ownership IN ('owned','unowned','unknown')),
    PRIMARY KEY(audit,identity)
);
CREATE FUNCTION file_legacy_audit_identity_guard() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Legacy file audits are immutable history'; END IF;
    IF NEW.id<>OLD.id OR NEW.operator<>OLD.operator OR NEW.plan IS DISTINCT FROM OLD.plan
       OR NEW.plan_digest<>OLD.plan_digest OR OLD.state IN ('completed','manual_required') THEN
        RAISE EXCEPTION 'Legacy file audit identity and completed receipts are immutable';
    END IF;
    IF NOT file_evidence_graph_held(true) THEN RAISE EXCEPTION 'Legacy audit progress requires exclusive graph admission'; END IF;
    IF NOT ((OLD.state='requested' AND NEW.state IN ('cataloguing','cleaning','manual_required'))
        OR (OLD.state='cataloguing' AND NEW.state IN ('cataloguing','cleaning','manual_required'))
        OR (OLD.state='cleaning' AND NEW.state IN ('cleaning','completed','manual_required'))) THEN
        RAISE EXCEPTION 'Legacy audit state cannot regress';
    END IF;
    IF NEW.state='completed' AND (
        NOT EXISTS(SELECT 1 FROM file_legacy_audit_evidence WHERE audit=OLD.id AND kind='writers' AND identity=OLD.plan_digest)
        OR NOT EXISTS(SELECT 1 FROM file_legacy_audit_evidence WHERE audit=OLD.id AND kind='catalogue' AND identity=OLD.plan_digest)
        OR NOT EXISTS(SELECT 1 FROM file_legacy_audit_evidence WHERE audit=OLD.id AND kind='settled' AND identity=OLD.plan_digest)
        OR EXISTS(SELECT 1 FROM file_legacy_audit_item i WHERE audit=OLD.id AND (ownership='unknown'
            OR (ownership='owned' AND NOT EXISTS(SELECT 1 FROM file_legacy_audit_evidence e
                WHERE e.audit=i.audit AND e.kind='absence' AND e.identity=i.identity))))
    ) THEN RAISE EXCEPTION 'Legacy audit still owns unconfirmed cleanup'; END IF;
    RETURN NEW;
END; $$;
CREATE TRIGGER file_legacy_audit_identity BEFORE UPDATE OR DELETE ON file_legacy_audit
FOR EACH ROW EXECUTE FUNCTION file_legacy_audit_identity_guard();
DO $$ DECLARE t TEXT; BEGIN
    FOREACH t IN ARRAY ARRAY['file_legacy_audit_member','file_legacy_audit_evidence','file_legacy_audit_item'] LOOP
        EXECUTE format('CREATE TRIGGER immutable BEFORE UPDATE OR DELETE ON %I FOR EACH ROW EXECUTE FUNCTION guard_worker_delivery_immutable()',t);
    END LOOP;
END $$;

-- Preserve the atomic path. Legacy operations need the explicit completed audit
-- and immutable all-writer/catalog/exact-absence evidence, not a TTL exception.
CREATE OR REPLACE FUNCTION file_evidence_guard_put_retirement() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT file_evidence_graph_held(false) THEN RAISE EXCEPTION 'PUT retirement must enter graph admission first'; END IF;
    IF EXISTS(SELECT 1 FROM file_evidence_put_observation r JOIN file_evidence_owner o ON o.version=OLD.owner
        WHERE r.attempt=OLD.attempt AND r.object_key=o.object_key AND r.provider_version=o.provider_version) THEN RETURN OLD; END IF;
    IF OLD.protocol='atomic_create_v1' AND EXISTS(SELECT 1 FROM file_evidence_write_fence f
        WHERE f.version=OLD.owner AND f.confirmed) AND file_evidence_graph_held(true) THEN RETURN OLD; END IF;
    IF OLD.protocol='legacy_unknown' AND file_evidence_graph_held(true) AND EXISTS(
        SELECT 1 FROM file_legacy_audit_member m JOIN file_legacy_audit a ON a.id=m.audit
        JOIN file_legacy_audit_evidence e ON e.audit=a.id AND e.kind='settled' AND e.identity=a.plan_digest
        WHERE m.operation=OLD.attempt AND m.version=OLD.owner AND a.state='completed'
    ) THEN RETURN OLD; END IF;
    RAISE EXCEPTION 'Native PUT settlement missing for operation % owner %; use a stopped-writer/provider audit, not cancellation/age/HEAD absence',OLD.attempt,OLD.owner;
END; $$;
