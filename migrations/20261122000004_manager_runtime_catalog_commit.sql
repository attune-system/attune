-- Pack activation plans preparation before it commits its projected runtimes.
-- Keep the published catalog revision visible during planning and advance it
-- atomically with the definition changes at commit.
DROP TRIGGER manager_runtime_added_removed ON runtime;
DROP TRIGGER manager_runtime_changed ON runtime;
DROP TRIGGER manager_runtime_version_added_removed ON runtime_version;
DROP TRIGGER manager_runtime_version_changed ON runtime_version;

CREATE CONSTRAINT TRIGGER manager_runtime_added_removed
    AFTER INSERT OR DELETE ON runtime DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION advance_manager_runtime_catalog();
CREATE CONSTRAINT TRIGGER manager_runtime_changed
    AFTER UPDATE ON runtime DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
    WHEN ((OLD.ref, OLD.name, OLD.aliases, OLD.distributions, OLD.execution_config, OLD.retired_at)
        IS DISTINCT FROM
          (NEW.ref, NEW.name, NEW.aliases, NEW.distributions, NEW.execution_config, NEW.retired_at))
    EXECUTE FUNCTION advance_manager_runtime_catalog();
CREATE CONSTRAINT TRIGGER manager_runtime_version_added_removed
    AFTER INSERT OR DELETE ON runtime_version DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION advance_manager_runtime_catalog();
CREATE CONSTRAINT TRIGGER manager_runtime_version_changed
    AFTER UPDATE ON runtime_version DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
    WHEN ((OLD.runtime, OLD.runtime_ref, OLD.version, OLD.distributions, OLD.execution_config, OLD.retired_at)
        IS DISTINCT FROM
          (NEW.runtime, NEW.runtime_ref, NEW.version, NEW.distributions, NEW.execution_config, NEW.retired_at))
    EXECUTE FUNCTION advance_manager_runtime_catalog();
