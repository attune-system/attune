-- JSONB timestamp rendering must not depend on the selecting session. Preserve
-- the internal v1 projection and every previously selected immutable identity.
-- Historical non-UTC selections remain fail-closed; never rewrite their hash.
ALTER FUNCTION object_log_close_snapshot(BIGINT) SET TimeZone = 'UTC';
