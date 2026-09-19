-- Exact source definitions for the bounded bundled-core bootstrap bridge.
-- These are populated by catalog reconciliation, not by the Python loader.
ALTER TABLE platform_catalog_state
    ADD COLUMN bootstrap_definitions JSONB NOT NULL DEFAULT '{}'::jsonb
    CHECK (jsonb_typeof(bootstrap_definitions) = 'object');

CREATE OR REPLACE FUNCTION set_component_managed_release() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE owner_id BIGINT;
BEGIN
    owner_id := (to_jsonb(NEW)->>TG_ARGV[0])::BIGINT;
    IF NEW.catalog_revision IS NOT NULL OR owner_id IS NULL OR
       COALESCE((to_jsonb(NEW)->>'is_adhoc')::BOOLEAN, FALSE) THEN
        NEW.managed_release := NULL;
    ELSIF TG_OP = 'INSERT' OR
          (to_jsonb(NEW)->TG_ARGV[0]) IS DISTINCT FROM (to_jsonb(OLD)->TG_ARGV[0]) THEN
        -- FOR KEY SHARE does not conflict with an active_release update.
        -- SHARE waits for activation and reads its committed release, or makes
        -- activation wait until this insertion is visible to its projection sweep.
        SELECT active_release INTO NEW.managed_release FROM pack
            WHERE id = owner_id FOR SHARE;
    END IF;
    RETURN NEW;
END $$;
