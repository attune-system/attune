-- Durable, encrypted inbox for managed-sensor inquiry callbacks.
-- Sensor, workload, and pack release IDs are immutable provenance and may
-- outlive the operational rows they identify.

CREATE TABLE inquiry_callback_delivery (
    id BIGSERIAL PRIMARY KEY,
    sensor BIGINT NOT NULL,
    integration_identity BIGINT NOT NULL REFERENCES identity(id) ON DELETE RESTRICT,
    workload BIGINT NOT NULL,
    assignment_generation BIGINT NOT NULL,
    pack_release BIGINT NOT NULL,
    pack_release_digest TEXT NOT NULL,
    adapter_ref TEXT NOT NULL,
    provider_delivery_id TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    encrypted_payload JSONB NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    inquiry BIGINT,
    rejection_code TEXT,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT inquiry_callback_delivery_generation_positive CHECK (assignment_generation > 0),
    CONSTRAINT inquiry_callback_delivery_attempt_count_nonnegative CHECK (attempt_count >= 0),
    CONSTRAINT inquiry_callback_delivery_adapter_ref_valid CHECK (
        adapter_ref ~ '^[a-z0-9][a-z0-9._-]{0,63}$'
    ),
    CONSTRAINT inquiry_callback_delivery_provider_id_bounded CHECK (
        provider_delivery_id = BTRIM(provider_delivery_id)
        AND length(provider_delivery_id) BETWEEN 1 AND 255
    ),
    CONSTRAINT inquiry_callback_delivery_digest_format CHECK (
        request_digest ~ '^sha256:[0-9a-f]{64}$'
    ),
    CONSTRAINT inquiry_callback_delivery_release_digest_bounded CHECK (
        length(pack_release_digest) BETWEEN 1 AND 255
    ),
    CONSTRAINT inquiry_callback_delivery_payload_encrypted CHECK (
        jsonb_typeof(encrypted_payload) = 'string'
        AND octet_length(encrypted_payload::TEXT) <= 4096
    ),
    CONSTRAINT inquiry_callback_delivery_state_valid CHECK (
        state IN ('pending', 'accepted', 'rejected')
    ),
    CONSTRAINT inquiry_callback_delivery_terminal_shape CHECK (
        (state = 'pending' AND inquiry IS NULL AND rejection_code IS NULL)
        OR (state = 'accepted' AND inquiry IS NOT NULL AND rejection_code IS NULL)
        OR (state = 'rejected' AND inquiry IS NULL AND rejection_code IS NOT NULL)
    ),
    CONSTRAINT inquiry_callback_delivery_rejection_code_bounded CHECK (
        rejection_code IS NULL OR rejection_code ~ '^[a-z0-9_]{1,64}$'
    ),
    CONSTRAINT uq_inquiry_callback_delivery_sensor_adapter_provider_id UNIQUE (
        sensor,
        adapter_ref,
        provider_delivery_id
    )
);

CREATE INDEX idx_inquiry_callback_delivery_pending
    ON inquiry_callback_delivery(next_attempt_at, created, id)
    WHERE state = 'pending';

CREATE TRIGGER update_inquiry_callback_delivery_updated
    BEFORE UPDATE ON inquiry_callback_delivery
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

COMMENT ON TABLE inquiry_callback_delivery IS 'Encrypted durable inbox for managed-sensor inquiry callback selections';
COMMENT ON COLUMN inquiry_callback_delivery.adapter_ref IS 'Release-pinned sensor metadata adapter used to normalize the callback';
COMMENT ON COLUMN inquiry_callback_delivery.inquiry IS 'Inquiry answered by this delivery when state is accepted';
COMMENT ON COLUMN inquiry_callback_delivery.encrypted_payload IS 'Encrypted normalized provider, actor, response handle, tenant, and external subject';
