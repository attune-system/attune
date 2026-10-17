CREATE FUNCTION guard_admitted_runtime_executable() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.executable_snapshot IS DISTINCT FROM NEW.executable_snapshot
        OR OLD.pack_release IS DISTINCT FROM NEW.pack_release
        OR OLD.pack_release_digest IS DISTINCT FROM NEW.pack_release_digest THEN
        IF EXISTS (SELECT 1 FROM runtime_authority_admission
            WHERE owner_kind = TG_ARGV[0] AND owner_id = OLD.id) THEN
            RAISE EXCEPTION 'Admitted runtime executable metadata is immutable';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_execution_admitted_executable BEFORE UPDATE ON execution
    FOR EACH ROW EXECUTE FUNCTION guard_admitted_runtime_executable('execution');
CREATE TRIGGER guard_enforcement_admitted_executable BEFORE UPDATE ON enforcement
    FOR EACH ROW EXECUTE FUNCTION guard_admitted_runtime_executable('enforcement');
