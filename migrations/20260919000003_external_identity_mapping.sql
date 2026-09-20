-- External subjects asserted by an integration and mapped to Attune identities.

CREATE TABLE external_identity_mapping (
    id BIGSERIAL PRIMARY KEY,
    integration_identity BIGINT NOT NULL REFERENCES identity(id) ON DELETE CASCADE,
    mapped_identity BIGINT NOT NULL REFERENCES identity(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    tenant TEXT NOT NULL,
    external_subject TEXT NOT NULL,
    created_by BIGINT REFERENCES identity(id) ON DELETE SET NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT external_identity_mapping_provider_format CHECK (
        provider ~ '^[a-z0-9][a-z0-9._-]{0,63}$'
    ),
    CONSTRAINT external_identity_mapping_tenant_format CHECK (
        tenant = btrim(tenant)
        AND length(tenant) BETWEEN 1 AND 255
    ),
    CONSTRAINT external_identity_mapping_subject_format CHECK (
        external_subject = btrim(external_subject)
        AND length(external_subject) BETWEEN 1 AND 255
    ),
    CONSTRAINT uq_external_identity_mapping_key UNIQUE (
        integration_identity,
        provider,
        tenant,
        external_subject
    )
);

CREATE INDEX idx_external_identity_mapping_integration
    ON external_identity_mapping(integration_identity, id);
CREATE INDEX idx_external_identity_mapping_mapped
    ON external_identity_mapping(mapped_identity);

CREATE TRIGGER update_external_identity_mapping_updated
    BEFORE UPDATE ON external_identity_mapping
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_column();

COMMENT ON TABLE external_identity_mapping IS 'Maps an integration-scoped external subject to an Attune identity';
COMMENT ON COLUMN external_identity_mapping.integration_identity IS 'Integration identity that owns and asserts this mapping';
COMMENT ON COLUMN external_identity_mapping.mapped_identity IS 'Attune identity selected for the external subject';
COMMENT ON COLUMN external_identity_mapping.provider IS 'Canonical lowercase provider token';
COMMENT ON COLUMN external_identity_mapping.tenant IS 'Case-sensitive provider tenant identifier';
COMMENT ON COLUMN external_identity_mapping.external_subject IS 'Case-sensitive provider subject identifier';

ALTER TABLE inquiry
    ADD CONSTRAINT inquiry_external_actor_size CHECK (
        external_actor IS NULL OR octet_length(external_actor::TEXT) <= 4096
    );

COMMENT ON COLUMN inquiry.external_actor IS 'Non-secret external actor attribution for an integration response, limited to 4096 bytes';
