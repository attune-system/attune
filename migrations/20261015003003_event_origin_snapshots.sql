-- Partitioned events are identified by the complete tuple, never event.id.
-- No FK to event: retention and partition maintenance remain independent.
CREATE TABLE event_origin_snapshot (
    id BIGSERIAL PRIMARY KEY,
    event BIGINT NOT NULL,
    event_created TIMESTAMPTZ NOT NULL,
    snapshot JSONB NOT NULL,
    snapshot_digest TEXT NOT NULL,
    evidence JSONB NOT NULL,
    evidence_digest TEXT NOT NULL,
    UNIQUE (event, event_created),
    CHECK (octet_length(snapshot::TEXT) <= 16777216),
    CHECK (octet_length(evidence::TEXT) <= 1048576)
);
CREATE INDEX event_origin_snapshot_retention_idx ON event_origin_snapshot(event_created,id);

CREATE FUNCTION guard_event_origin_snapshot() RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'Event origin snapshots are immutable';
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER event_origin_snapshot_immutable BEFORE UPDATE ON event_origin_snapshot
FOR EACH ROW EXECUTE FUNCTION guard_event_origin_snapshot();
