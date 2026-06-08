-- ============================================================================
-- Action reference visibility
-- ============================================================================

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'action_reference_visibility_enum') THEN
        CREATE TYPE action_reference_visibility_enum AS ENUM (
            'public',
            'private',
            'restricted'
        );
    END IF;
END $$;

COMMENT ON TYPE action_reference_visibility_enum IS
    'Controls which packs may reference an action from rules, workflows, and work queues';

ALTER TABLE action
    ADD COLUMN reference_visibility action_reference_visibility_enum NOT NULL DEFAULT 'public',
    ADD COLUMN reference_allowed_pack_refs TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[];

ALTER TABLE action
    ADD CONSTRAINT action_reference_allowed_pack_refs_non_null
    CHECK (array_position(reference_allowed_pack_refs, NULL) IS NULL);

CREATE INDEX idx_action_reference_visibility ON action(reference_visibility);
CREATE INDEX idx_action_reference_allowed_pack_refs ON action USING GIN (reference_allowed_pack_refs);

COMMENT ON COLUMN action.reference_visibility IS
    'Pack-level reference visibility: public actions may be referenced by any pack; private actions only by their owning pack; restricted actions by their owning pack and reference_allowed_pack_refs.';
COMMENT ON COLUMN action.reference_allowed_pack_refs IS
    'Allow-list of pack refs that may reference this action when reference_visibility is restricted.';
