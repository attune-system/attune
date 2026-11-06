-- Discovery progress is not deletion authority. Final graph/pin/identity checks
-- remain in the existing metadata claim transactions.
CREATE TABLE object_body_discovery (
    kind TEXT PRIMARY KEY CHECK (kind IN ('abandoned', 'deleting')),
    last_id BIGINT NOT NULL DEFAULT 0 CHECK (last_id >= 0),
    high_water BIGINT NOT NULL DEFAULT 0 CHECK (high_water >= 0)
);
INSERT INTO object_body_discovery(kind) VALUES ('abandoned'), ('deleting');
