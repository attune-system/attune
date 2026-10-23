-- Sensor SDK reads retain immutable Key ownership, not credential values.
-- These origins share the producing generation's retention lifetime.
CREATE TABLE sensor_generation_key_read_origin (
    generation_binding BIGINT NOT NULL REFERENCES sensor_generation_schema_binding(id) ON DELETE CASCADE,
    origin_digest TEXT NOT NULL CHECK (length(origin_digest) = 64),
    origin JSONB NOT NULL CHECK (jsonb_typeof(origin) = 'object' AND octet_length(origin::text) <= 16384),
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (generation_binding, origin_digest)
);

CREATE FUNCTION reject_sensor_key_read_origin_update() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Sensor runtime Key read origins are immutable' USING ERRCODE = '23514';
END $$;
CREATE TRIGGER immutable_sensor_key_read_origin BEFORE UPDATE ON sensor_generation_key_read_origin
    FOR EACH ROW EXECUTE FUNCTION reject_sensor_key_read_origin_update();
