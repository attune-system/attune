-- Migration: v0.2.1 upgrade compatibility
-- Description: Applies the dashboard default-home behavior that was added to
--              an already-released historical migration.
-- Version: 20250101000027

SET search_path TO attune, public;

CREATE OR REPLACE FUNCTION enforce_dashboard_default_home()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.is_default_home AND (
        TG_OP = 'INSERT'
        OR OLD.is_default_home IS DISTINCT FROM TRUE
        OR OLD.scope_type IS DISTINCT FROM NEW.scope_type
        OR OLD.scope_ref IS DISTINCT FROM NEW.scope_ref
    ) THEN
        IF NOT pg_try_advisory_xact_lock(
            hashtextextended(NEW.scope_type::text || ':' || NEW.scope_ref, 0)
        ) THEN
            RAISE EXCEPTION 'Dashboard default-home scope is busy; retry the transaction'
                USING ERRCODE = '55P03';
        END IF;

        -- The row being moved is locked before this BEFORE trigger runs. Never
        -- wait on a destination default that may be moving in the other direction.
        PERFORM id
        FROM dashboard
        WHERE scope_type = NEW.scope_type
          AND scope_ref = NEW.scope_ref
          AND is_default_home = TRUE
          AND id <> NEW.id
        FOR UPDATE NOWAIT;

        UPDATE dashboard
        SET is_default_home = FALSE,
            revision = revision + 1,
            updated = NOW()
        WHERE scope_type = NEW.scope_type
          AND scope_ref = NEW.scope_ref
          AND is_default_home = TRUE
          AND id <> NEW.id;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS enforce_dashboard_default_home_trigger ON dashboard;
CREATE TRIGGER enforce_dashboard_default_home_trigger
    BEFORE INSERT OR UPDATE ON dashboard
    FOR EACH ROW
    EXECUTE FUNCTION enforce_dashboard_default_home();
