-- A pre-06005 database with reservations uses the explicit, writer-drained
-- preflight script. Restore its exact original table, constraints and triggers
-- before auditing rows. Already-applied 06005 and fresh schemas need no parking.
DO $$ BEGIN
    IF to_regclass('file_evidence_upgrade_reservation') IS NOT NULL THEN
        DROP VIEW artifact_file_origin;
        ALTER TABLE file_evidence_upgrade_reservation RENAME TO artifact_file_origin;
    END IF;
END $$;
ALTER TABLE file_evidence_owner
    ADD COLUMN evidence_state TEXT NOT NULL DEFAULT 'retained'
        CHECK (evidence_state IN ('retained','pending_audit','quarantined')),
    ADD COLUMN diagnostic TEXT;
-- Admission still has a 1 MiB total graph budget. Upgrade quarantine must also
-- preserve an oversized old receipt plus its fixed reservation fields.
ALTER TABLE file_evidence_owner DROP CONSTRAINT file_evidence_owner_reservation_check;
ALTER TABLE file_evidence_owner ADD CONSTRAINT file_evidence_owner_reservation_check
    CHECK (octet_length(reservation::text)<=2097152);

-- Copy exact obligations and the original receipt binding before attempting any
-- graph reconstruction. Missing evidence cannot prevent durable cleanup ownership.
INSERT INTO file_evidence_owner(version,artifact,root_read,reservation,object_key,provider_version,
    object_done,staging_path,staging_done,evidence_state,diagnostic)
SELECT v.id,v.artifact,o.origin_read,to_jsonb(o),v.object_key,v.provider_version,
    v.object_key IS NULL,v.file_path,v.file_path IS NULL,'pending_audit',
    format('file version %s root %s awaits bounded historical evidence audit',v.id,o.origin_read)
FROM artifact_file_origin o JOIN artifact_version v ON v.id=o.artifact_version
ON CONFLICT(version) DO NOTHING;

-- Pending reservations that never reached PUT still need a native absence
-- observation. Do not overwrite an already observed exact provider identity.
INSERT INTO object_maintenance_ledger(object_key,object_kind,provider_version,size_bytes,state)
SELECT v.object_key,'artifact',v.provider_version,v.size_bytes,
    CASE WHEN v.provider_version IS NULL THEN 'uploading' ELSE 'ready' END
FROM artifact_version v JOIN file_evidence_owner o ON o.version=v.id
WHERE v.object_key IS NOT NULL
ON CONFLICT(object_key) DO NOTHING;

-- Old path guards can outlive both version and origin metadata. Keep those
-- exact native cleanup obligations even though no authority can be recovered.
INSERT INTO file_evidence_owner(version,artifact,root_read,reservation,metadata_gone,
    object_done,staging_path,staging_done,evidence_state,diagnostic)
SELECT g.artifact_version,0,0,'{}',true,true,g.file_path,false,'quarantined',
    format('file version %s has orphan staging %s; historical origin is missing; cleanup only',g.artifact_version,g.file_path)
FROM artifact_file_staging_guard g
WHERE NOT EXISTS(SELECT 1 FROM artifact_version v WHERE v.id=g.artifact_version)
ON CONFLICT(version) DO NOTHING;

-- Preserve unknown-origin object ownership separately rather than inventing a
-- runtime origin/version from a display key. Exact cleanup completion retires it.
CREATE TABLE file_evidence_orphan_object (
    object_key TEXT PRIMARY KEY,
    provider_version TEXT,
    diagnostic TEXT NOT NULL
);
INSERT INTO file_evidence_orphan_object(object_key,provider_version,diagnostic)
SELECT l.object_key,l.provider_version,
    'Historical file origin unavailable after metadata deletion; native exact cleanup only'
FROM object_maintenance_ledger l WHERE l.object_kind='artifact'
    AND NOT EXISTS(SELECT 1 FROM artifact_version v WHERE v.object_key=l.object_key)
    AND NOT EXISTS(SELECT 1 FROM file_evidence_owner o WHERE o.object_key=l.object_key);

CREATE FUNCTION file_evidence_require_retained(v BIGINT) RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE s TEXT; d TEXT;
BEGIN
    SELECT evidence_state,diagnostic INTO s,d FROM file_evidence_owner WHERE version=v;
    IF s IS DISTINCT FROM 'retained' THEN
        RAISE EXCEPTION 'File evidence readmission required for version %: %',v,
            COALESCE(d,'durable evidence owner is missing; run bounded historical audit or clean up the file');
    END IF;
END;
$$;
