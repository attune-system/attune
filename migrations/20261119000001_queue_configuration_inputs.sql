-- Fixed queue configuration is admitted separately from item contributors and
-- execution API permissions. Values are encrypted, bound to their owning item,
-- and never reselected from current Keys/configuration during dispatch/retry.
CREATE TABLE work_queue_item_configuration_input (
    item BIGINT PRIMARY KEY REFERENCES work_queue_item(id) ON DELETE CASCADE,
    policy_revision BIGINT NOT NULL REFERENCES pack_runtime_policy_revision(id) ON DELETE RESTRICT,
    configuration_revision BIGINT REFERENCES component_configuration_revision(id) ON DELETE RESTRICT,
    binding JSONB NOT NULL CHECK (octet_length(binding::TEXT) <= 1048576),
    binding_digest TEXT NOT NULL,
    encrypted_context JSONB NOT NULL CHECK (octet_length(encrypted_context::TEXT) <= 2097152),
    encryption_key_hash TEXT NOT NULL
);
CREATE INDEX work_queue_configuration_input_policy ON work_queue_item_configuration_input(policy_revision);
CREATE INDEX work_queue_configuration_input_revision ON work_queue_item_configuration_input(configuration_revision);
CREATE TRIGGER guard_work_queue_configuration_input BEFORE UPDATE ON work_queue_item_configuration_input
    FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
