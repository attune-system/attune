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
    'Controls which packs may reference an action, trigger, or queue from pack-owned components';

ALTER TABLE action
    ADD COLUMN reference_visibility action_reference_visibility_enum NOT NULL DEFAULT 'public',
    ADD COLUMN reference_allowed_pack_refs TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[];

ALTER TABLE action
    ADD CONSTRAINT action_reference_allowed_pack_refs_non_null
    CHECK (array_position(reference_allowed_pack_refs, NULL) IS NULL);
ALTER TABLE action
    ADD CONSTRAINT action_reference_allowed_pack_refs_restricted_only
    CHECK (reference_visibility = 'restricted' OR cardinality(reference_allowed_pack_refs) = 0);

CREATE INDEX idx_action_reference_visibility ON action(reference_visibility);
CREATE INDEX idx_action_reference_allowed_pack_refs ON action USING GIN (reference_allowed_pack_refs);

COMMENT ON COLUMN action.reference_visibility IS
    'Pack-level reference visibility: public actions may be referenced by any pack; private actions only by their owning pack; restricted actions by their owning pack and reference_allowed_pack_refs.';
COMMENT ON COLUMN action.reference_allowed_pack_refs IS
    'Allow-list of pack refs that may reference this action when reference_visibility is restricted.';

ALTER TABLE trigger
    ADD COLUMN reference_visibility action_reference_visibility_enum NOT NULL DEFAULT 'public',
    ADD COLUMN reference_allowed_pack_refs TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[];

ALTER TABLE trigger
    ADD CONSTRAINT trigger_reference_allowed_pack_refs_non_null
    CHECK (array_position(reference_allowed_pack_refs, NULL) IS NULL);
ALTER TABLE trigger
    ADD CONSTRAINT trigger_reference_allowed_pack_refs_restricted_only
    CHECK (reference_visibility = 'restricted' OR cardinality(reference_allowed_pack_refs) = 0);

CREATE INDEX idx_trigger_reference_visibility ON trigger(reference_visibility);
CREATE INDEX idx_trigger_reference_allowed_pack_refs ON trigger USING GIN (reference_allowed_pack_refs);

COMMENT ON COLUMN trigger.reference_visibility IS
    'Pack-level reference visibility: public triggers may be subscribed to by rules from any pack; private triggers only by rules in their owning pack; restricted triggers by their owning pack and reference_allowed_pack_refs.';
COMMENT ON COLUMN trigger.reference_allowed_pack_refs IS
    'Allow-list of pack refs that may subscribe to this trigger when reference_visibility is restricted.';

ALTER TABLE work_queue
    ADD COLUMN reference_visibility action_reference_visibility_enum NOT NULL DEFAULT 'public',
    ADD COLUMN reference_allowed_pack_refs TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[];

ALTER TABLE work_queue
    ADD CONSTRAINT work_queue_reference_allowed_pack_refs_non_null
    CHECK (array_position(reference_allowed_pack_refs, NULL) IS NULL);
ALTER TABLE work_queue
    ADD CONSTRAINT work_queue_reference_allowed_pack_refs_restricted_only
    CHECK (reference_visibility = 'restricted' OR cardinality(reference_allowed_pack_refs) = 0);

CREATE INDEX idx_work_queue_reference_visibility ON work_queue(reference_visibility);
CREATE INDEX idx_work_queue_reference_allowed_pack_refs ON work_queue USING GIN (reference_allowed_pack_refs);

COMMENT ON COLUMN work_queue.reference_visibility IS
    'Pack-level reference visibility: public queues may be targeted by any pack; private queues only by their owning pack; restricted queues by their owning pack and reference_allowed_pack_refs.';
COMMENT ON COLUMN work_queue.reference_allowed_pack_refs IS
    'Allow-list of pack refs that may target this queue when reference_visibility is restricted.';
