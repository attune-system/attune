-- Publication already owns source/capture/target locks. It is not reclamation:
-- even DELETE matching zero rows fires the exclusive graph admission trigger.
-- The bounded supervisor collector rechecks retention in its own graph-first
-- transaction and owns exact physical cleanup. Never upgrade publication here.
CREATE OR REPLACE FUNCTION enforce_artifact_retention()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.body_state = 'pending' OR NEW.body_state = 'deleting' THEN
        RETURN NEW;
    END IF;
    UPDATE artifact
    SET size_bytes = NEW.size_bytes,
        content_type = COALESCE(NEW.content_type, content_type)
    WHERE id = NEW.artifact
      AND NEW.version = (
          SELECT MAX(latest.version) FROM artifact_version latest
          WHERE latest.artifact = NEW.artifact
            AND (latest.body_state IS NULL OR latest.body_state = 'ready')
      );
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
COMMENT ON FUNCTION enforce_artifact_retention IS
    'Publication metadata projection only. Version-count reclamation belongs to bounded, graph-first supervisor cleanup.';
