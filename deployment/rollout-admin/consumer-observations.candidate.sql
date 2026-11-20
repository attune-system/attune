-- Candidate transport for the recorded-authority rollout contract. Install only
-- in the trusted application schema, after recorded rollout membership.
-- Reporter credentials are dedicated to one deployment boot and never reused.
DO $trust$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_namespace AS ns
        JOIN pg_roles AS owner ON owner.oid = ns.nspowner
        WHERE ns.nspname = current_schema() AND owner.rolname = current_user
            AND NOT owner.rolcanlogin AND NOT owner.rolsuper AND NOT owner.rolcreaterole
            AND NOT owner.rolcreatedb AND NOT owner.rolreplication AND NOT owner.rolbypassrls
    ) OR EXISTS (
        SELECT 1 FROM pg_namespace AS ns
        CROSS JOIN LATERAL aclexplode(COALESCE(ns.nspacl, acldefault('n', ns.nspowner))) AS acl
        WHERE ns.nspname IN (current_schema(), 'pg_catalog')
            AND acl.privilege_type = 'CREATE' AND acl.grantee <> ns.nspowner
    ) THEN
        RAISE EXCEPTION 'Consumer observations require the trusted installation owner' USING ERRCODE = '42501';
    END IF;
END $trust$;

CREATE TABLE recorded_consumer_binding (
    rollout BIGINT NOT NULL REFERENCES recorded_rollout_generation(id),
    instance UUID NOT NULL,
    boot UUID NOT NULL,
    reporter NAME NOT NULL UNIQUE,
    reporter_oid OID NOT NULL UNIQUE,
    role TEXT NOT NULL,
    required_mask INTEGER NOT NULL,
    PRIMARY KEY (rollout, instance)
);

CREATE TABLE recorded_consumer_observation (
    rollout BIGINT NOT NULL,
    instance UUID NOT NULL,
    process_claim UUID NOT NULL CHECK (process_claim <> '00000000-0000-0000-0000-000000000000'),
    observed TIMESTAMPTZ,
    worker_id BIGINT CHECK (worker_id > 0),
    PRIMARY KEY (rollout, instance),
    FOREIGN KEY (rollout, instance) REFERENCES recorded_consumer_binding (rollout, instance)
);

REVOKE ALL ON recorded_consumer_binding, recorded_consumer_observation FROM PUBLIC;

CREATE UNIQUE INDEX recorded_action_worker_boot ON worker ((meta->>'registration_incarnation'))
WHERE capabilities ? 'recorded_execution_authority_v1';

-- This registration is evidence of a routing destination, not an authority
-- owner. The worker's replacement or loss of heartbeat invalidates it.
CREATE FUNCTION recorded_worker_registration_matches(
    registration worker, snapshot JSONB, member_id UUID, boot_id UUID, cutoff TIMESTAMPTZ
) RETURNS BOOLEAN
LANGUAGE SQL STABLE RETURNS NULL ON NULL INPUT AS $worker$
    SELECT registration.worker_role = 'action' AND registration.status = 'active'
        AND registration.last_heartbeat > cutoff - INTERVAL '30 seconds'
        AND registration.last_heartbeat <= cutoff
        AND registration.meta->>'registration_incarnation' = boot_id::TEXT
        AND registration.capabilities->'recorded_execution_authority_v1' =
            jsonb_build_object('instance', member_id, 'boot', boot_id, 'snapshot', snapshot)
$worker$;

-- Invoked by the DBA that provisions the dedicated PostgreSQL LOGIN. The
-- rollout operator and all runtime services keep their existing privileges.
CREATE FUNCTION bind_recorded_consumer(target UUID, member_id UUID, boot_id UUID, login_name NAME) RETURNS VOID
LANGUAGE plpgsql SECURITY DEFINER AS $bind$
DECLARE
    rollout_id BIGINT;
    member_mask INTEGER;
    member_role TEXT;
    principal OID;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = session_user AND rolsuper) THEN
        RAISE EXCEPTION 'Consumer binding requires the DBA session' USING ERRCODE = '42501';
    END IF;

    SELECT generation.id, member.value->>'role', (member.value->>'required_mask')::INTEGER
        INTO rollout_id, member_role, member_mask
    FROM recorded_rollout_generation AS generation
    CROSS JOIN LATERAL jsonb_array_elements(generation.expected->'members') AS member
    WHERE generation.generation = target AND member.value->>'instance' = member_id::TEXT
        AND member.value->>'boot' = boot_id::TEXT;

    SELECT role.oid INTO principal
    FROM pg_roles AS role
    WHERE role.rolname = login_name AND role.rolcanlogin AND NOT role.rolsuper
        AND NOT role.rolcreaterole AND NOT role.rolcreatedb AND NOT role.rolreplication
        AND NOT role.rolbypassrls
        AND NOT EXISTS (
            SELECT 1 FROM pg_auth_members WHERE member = role.oid OR roleid = role.oid
        );

    IF rollout_id IS NULL OR principal IS NULL THEN
        RAISE EXCEPTION 'Invalid rollout member or reporter login' USING ERRCODE = '42501';
    END IF;

    INSERT INTO recorded_consumer_binding (rollout, instance, boot, reporter, reporter_oid, role, required_mask)
    VALUES (rollout_id, member_id, boot_id, login_name, principal, member_role, member_mask)
    ON CONFLICT (rollout, instance) DO NOTHING;

    IF NOT EXISTS (
        SELECT 1 FROM recorded_consumer_binding
        WHERE rollout = rollout_id AND instance = member_id AND boot = boot_id
            AND reporter = login_name AND reporter_oid = principal
            AND role = member_role AND required_mask = member_mask
    ) THEN
        RAISE EXCEPTION 'Conflicting consumer binding' USING ERRCODE = '42501';
    END IF;
END $bind$;

-- The first ready publication consumes this assigned boot. Renewals must
-- prove the same process-local claim, even after the observation is withdrawn
-- or expires. A replacement process needs a new staged boot and reporter.
CREATE FUNCTION report_recorded_consumer(
    snapshot JSONB, member_id UUID, boot_id UUID, claim UUID, mask INTEGER, target_worker BIGINT
) RETURNS VOID
LANGUAGE plpgsql SECURITY DEFINER AS $report$
DECLARE
    binding recorded_consumer_binding;
BEGIN
    SELECT b.* INTO binding
    FROM recorded_consumer_binding AS b
    JOIN recorded_rollout_generation AS g ON g.id = b.rollout
    JOIN pg_roles AS principal ON principal.oid = b.reporter_oid AND principal.rolname = b.reporter
    WHERE g.expected->'snapshot' = snapshot AND b.instance = member_id AND b.boot = boot_id
        AND b.reporter = session_user AND principal.rolcanlogin AND NOT principal.rolsuper
        AND NOT principal.rolcreaterole AND NOT principal.rolcreatedb AND NOT principal.rolreplication
        AND NOT principal.rolbypassrls
        AND NOT EXISTS (
            SELECT 1 FROM pg_auth_members WHERE member = principal.oid OR roleid = principal.oid
        );

    IF NOT FOUND OR claim IS NULL OR claim = '00000000-0000-0000-0000-000000000000'
        OR (mask IS NOT NULL AND mask <> binding.required_mask) THEN
        RAISE EXCEPTION 'Consumer observation not authorized' USING ERRCODE = '42501';
    END IF;

    IF (mask IS NOT NULL AND binding.role = 'worker') <> (target_worker IS NOT NULL)
        OR (target_worker IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM worker AS registration
            WHERE registration.id = target_worker
                AND recorded_worker_registration_matches(
                    registration, snapshot, member_id, boot_id, clock_timestamp()
                )
        )) THEN
        RAISE EXCEPTION 'Consumer observation has no compatible worker registration' USING ERRCODE = '42501';
    END IF;

    IF mask IS NULL THEN
        UPDATE recorded_consumer_observation
        SET observed = NULL, worker_id = NULL
        WHERE rollout = binding.rollout AND instance = binding.instance AND process_claim = claim;
        IF NOT FOUND AND EXISTS (
            SELECT 1 FROM recorded_consumer_observation
            WHERE rollout = binding.rollout AND instance = binding.instance
        ) THEN
            RAISE EXCEPTION 'Consumer observation belongs to another process' USING ERRCODE = '42501';
        END IF;
    ELSE
        INSERT INTO recorded_consumer_observation (rollout, instance, process_claim, observed, worker_id)
        VALUES (binding.rollout, binding.instance, claim, clock_timestamp(), target_worker)
        ON CONFLICT (rollout, instance) DO UPDATE
        SET observed = EXCLUDED.observed, worker_id = EXCLUDED.worker_id
        WHERE recorded_consumer_observation.process_claim = EXCLUDED.process_claim;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'Consumer observation belongs to another process' USING ERRCODE = '42501';
        END IF;
    END IF;
END $report$;

-- Admission/dispatch callers use the active pointer and database clock in the
-- same transaction as the effect. Reads serialize with an operator's switch.
CREATE FUNCTION current_recorded_consumer_evidence(claim UUID) RETURNS JSONB
LANGUAGE plpgsql SECURITY DEFINER AS $read$
DECLARE
    active recorded_rollout_active;
    expected JSONB;
    cutoff TIMESTAMPTZ;
    observations JSONB;
    caller JSONB;
    worker_targets JSONB;
BEGIN
    SELECT * INTO STRICT active FROM recorded_rollout_active WHERE singleton FOR SHARE;
    SELECT g.expected INTO expected
    FROM recorded_rollout_generation AS g
    JOIN platform_catalog_state AS catalog ON catalog.singleton
    WHERE g.id = active.generation
        AND (g.expected->'snapshot'->>'catalog_epoch')::INTEGER = catalog.compatibility_epoch
        AND (g.expected->'snapshot'->>'catalog_revision')::INTEGER = catalog.revision
        AND g.membership_digest = recorded_membership_digest(g.expected)
    FOR SHARE OF catalog;

    IF expected IS NULL THEN RETURN NULL; END IF;

    cutoff := clock_timestamp();
    SELECT COALESCE(jsonb_agg(jsonb_build_object(
        'instance', b.instance, 'boot', b.boot,
        'role', b.role, 'snapshot', expected->'snapshot',
        'gate_mask', b.required_mask, 'observed', o.observed,
        'expires', LEAST(o.observed, COALESCE(registration.last_heartbeat, o.observed)) + INTERVAL '30 seconds'
    ) ORDER BY b.instance), '[]'::JSONB), COALESCE(jsonb_agg(jsonb_build_object(
        'worker_id', registration.id, 'instance', b.instance, 'boot', b.boot
    ) ORDER BY b.instance) FILTER (WHERE registration.id IS NOT NULL), '[]'::JSONB)
    INTO observations, worker_targets
    FROM recorded_consumer_observation AS o
    JOIN recorded_consumer_binding AS b USING (rollout, instance)
    JOIN pg_roles AS principal ON principal.oid = b.reporter_oid AND principal.rolname = b.reporter
    LEFT JOIN worker AS registration ON registration.id = o.worker_id
    WHERE o.rollout = active.generation AND o.observed > cutoff - INTERVAL '30 seconds'
        AND principal.rolcanlogin AND NOT principal.rolsuper AND NOT principal.rolcreaterole
        AND NOT principal.rolcreatedb AND NOT principal.rolreplication AND NOT principal.rolbypassrls
        AND NOT EXISTS (
            SELECT 1 FROM pg_auth_members WHERE member = principal.oid OR roleid = principal.oid
        )
        AND CASE WHEN b.role = 'worker' THEN
            recorded_worker_registration_matches(
                registration, expected->'snapshot', b.instance, b.boot, cutoff
            )
        ELSE o.worker_id IS NULL END;

    SELECT jsonb_build_object('instance', b.instance, 'boot', b.boot, 'role', b.role,
        'required_mask', b.required_mask) INTO caller
    FROM recorded_consumer_binding AS b
    JOIN pg_roles AS principal ON principal.oid = b.reporter_oid AND principal.rolname = b.reporter
    JOIN recorded_consumer_observation AS o USING (rollout, instance)
    WHERE b.rollout = active.generation AND b.reporter = session_user
        AND o.process_claim = claim AND o.observed > cutoff - INTERVAL '30 seconds';

    RETURN jsonb_build_object('expected', expected, 'observations', observations, 'now', cutoff,
        'revision', active.revision, 'caller', caller, 'worker_targets', worker_targets);
END $read$;

REVOKE ALL ON FUNCTION bind_recorded_consumer(UUID, UUID, UUID, NAME),
    report_recorded_consumer(JSONB, UUID, UUID, UUID, INTEGER, BIGINT), current_recorded_consumer_evidence(UUID),
    recorded_worker_registration_matches(worker, JSONB, UUID, UUID, TIMESTAMPTZ) FROM PUBLIC;

DO $path$
DECLARE signature TEXT;
BEGIN
    FOREACH signature IN ARRAY ARRAY[
        'bind_recorded_consumer(UUID, UUID, UUID, NAME)',
        'report_recorded_consumer(JSONB, UUID, UUID, UUID, INTEGER, BIGINT)',
        'recorded_worker_registration_matches(worker, JSONB, UUID, UUID, TIMESTAMPTZ)',
        'current_recorded_consumer_evidence(UUID)'
    ] LOOP
        EXECUTE format('ALTER FUNCTION %s SET search_path TO pg_catalog, %I, pg_temp',
            signature, current_schema());
    END LOOP;
END $path$;

-- Reject inherited default grants that would let a login bypass the functions
-- or read a rollout without a deliberate provisioning grant.
DO $privileges$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_class AS relation
        CROSS JOIN LATERAL aclexplode(COALESCE(relation.relacl, acldefault('r', relation.relowner))) AS acl
        WHERE relation.oid IN (
            'recorded_consumer_binding'::REGCLASS, 'recorded_consumer_observation'::REGCLASS
        ) AND acl.grantee <> relation.relowner
    ) OR EXISTS (
        SELECT 1 FROM pg_proc AS procedure
        CROSS JOIN LATERAL aclexplode(COALESCE(procedure.proacl, acldefault('f', procedure.proowner))) AS acl
        WHERE procedure.oid IN (
            'bind_recorded_consumer(UUID, UUID, UUID, NAME)'::REGPROCEDURE,
            'report_recorded_consumer(JSONB, UUID, UUID, UUID, INTEGER, BIGINT)'::REGPROCEDURE,
            'recorded_worker_registration_matches(worker, JSONB, UUID, UUID, TIMESTAMPTZ)'::REGPROCEDURE,
            'current_recorded_consumer_evidence(UUID)'::REGPROCEDURE
        ) AND acl.grantee <> procedure.proowner
    ) THEN
        RAISE EXCEPTION 'Consumer transport has unexpected default grants' USING ERRCODE = '42501';
    END IF;
END $privileges$;
