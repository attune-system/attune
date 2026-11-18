-- Immutable execution profiles and one enrollment slot per logical worker.
CREATE TABLE workload_host_profile (
    id BIGSERIAL PRIMARY KEY,
    image BIGINT NOT NULL REFERENCES workload_execution_image(id),
    role TEXT NOT NULL CHECK (role IN ('action','sensor')),
    capabilities JSONB NOT NULL CHECK (jsonb_typeof(capabilities)='object'),
    max_concurrent INTEGER NOT NULL CHECK (max_concurrent BETWEEN 1 AND 1024),
    disabled BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE TABLE workload_host_enrollment (
    id BIGSERIAL PRIMARY KEY,
    profile BIGINT NOT NULL REFERENCES workload_host_profile(id),
    identity BIGINT NOT NULL REFERENCES identity(id),
    worker_name TEXT NOT NULL UNIQUE CHECK (length(worker_name) BETWEEN 1 AND 128),
    generation BIGINT NOT NULL DEFAULT 0 CHECK (generation>=0),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    worker BIGINT,
    instance UUID,
    custody_instance UUID,
    registration_generation BIGINT NOT NULL DEFAULT 0 CHECK (registration_generation>=0),
    CHECK ((worker IS NULL)=(instance IS NULL)),
    CHECK ((worker IS NULL)=(custody_instance IS NULL))
);
CREATE FUNCTION guard_workload_host_profile() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR ROW(NEW.id,NEW.image,NEW.role,NEW.capabilities,NEW.max_concurrent)
      IS DISTINCT FROM ROW(OLD.id,OLD.image,OLD.role,OLD.capabilities,OLD.max_concurrent)
    THEN RAISE EXCEPTION 'Execution profile is immutable; admit a new profile'; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER workload_host_profile_guard BEFORE UPDATE OR DELETE ON workload_host_profile
FOR EACH ROW EXECUTE FUNCTION guard_workload_host_profile();
CREATE FUNCTION guard_workload_host_enrollment() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR ROW(NEW.id,NEW.profile,NEW.identity,NEW.worker_name)
      IS DISTINCT FROM ROW(OLD.id,OLD.profile,OLD.identity,OLD.worker_name)
      OR NEW.generation<OLD.generation OR NEW.registration_generation<OLD.registration_generation OR (OLD.worker IS NOT NULL AND NEW.worker IS DISTINCT FROM OLD.worker)
      OR (ROW(NEW.instance,NEW.custody_instance) IS DISTINCT FROM ROW(OLD.instance,OLD.custody_instance) AND NEW.registration_generation<=OLD.registration_generation)
      OR (OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS DISTINCT FROM OLD.revoked_at)
    THEN RAISE EXCEPTION 'Host enrollment binding and withdrawal evidence are immutable'; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER workload_host_enrollment_guard BEFORE UPDATE OR DELETE ON workload_host_enrollment
FOR EACH ROW EXECUTE FUNCTION guard_workload_host_enrollment();
-- Withdrawal closes live authority before launchers reconcile/stop resources.
-- Restoring a profile never restores the affected worker source epochs.
CREATE FUNCTION withdraw_workload_host_profile() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.disabled IS DISTINCT FROM OLD.disabled THEN
        UPDATE worker SET status='inactive',updated=clock_timestamp()
        WHERE id IN (SELECT worker FROM workload_host_enrollment WHERE profile=OLD.id);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER workload_host_profile_withdraw AFTER UPDATE ON workload_host_profile
FOR EACH ROW EXECUTE FUNCTION withdraw_workload_host_profile();
CREATE FUNCTION withdraw_workload_host_enrollment() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(NEW.generation,NEW.revoked_at,NEW.expires_at) IS DISTINCT FROM ROW(OLD.generation,OLD.revoked_at,OLD.expires_at) THEN
        UPDATE worker SET status='inactive',updated=clock_timestamp() WHERE id=OLD.worker;
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER workload_host_enrollment_withdraw AFTER UPDATE ON workload_host_enrollment
FOR EACH ROW EXECUTE FUNCTION withdraw_workload_host_enrollment();
