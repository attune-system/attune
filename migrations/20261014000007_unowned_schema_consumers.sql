-- Ad-hoc triggers, sensors, and queues can have no declaring pack. Their
-- public-type references still protect active declarations from retirement.
ALTER TABLE schema_type_reference DROP CONSTRAINT schema_type_reference_pkey;
ALTER TABLE schema_type_reference ALTER COLUMN source_pack DROP NOT NULL;
CREATE UNIQUE INDEX schema_type_reference_identity
    ON schema_type_reference (source_pack, source_kind, source_ref, schema_field, target_ref)
    NULLS NOT DISTINCT;
