-- The queue remains ad hoc. Its association is not structural pack ownership.
-- Runtime authority is the accepted target Action invocation, with the actual
-- originating execution retained as an independent parent dependency.
CREATE TABLE work_queue_item_ad_hoc_dispatch (
    item BIGINT PRIMARY KEY REFERENCES work_queue_item(id) ON DELETE CASCADE,
    source_admission BIGINT NOT NULL REFERENCES runtime_authority_admission(id) ON DELETE RESTRICT,
    template JSONB NOT NULL,
    template_digest TEXT NOT NULL,
    schema_binding JSONB NOT NULL,
    schema_digest TEXT NOT NULL,
    CHECK (octet_length(template::TEXT) <= 1048576),
    CHECK (octet_length(schema_binding::TEXT) <= 1048576)
);
CREATE FUNCTION guard_work_queue_item_ad_hoc_dispatch() RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'Accepted ad-hoc dispatch templates are immutable';
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER work_queue_item_ad_hoc_dispatch_immutable
    BEFORE UPDATE ON work_queue_item_ad_hoc_dispatch
    FOR EACH ROW EXECUTE FUNCTION guard_work_queue_item_ad_hoc_dispatch();
