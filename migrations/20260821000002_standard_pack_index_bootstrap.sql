ALTER TABLE pack_registry_index
    ADD COLUMN IF NOT EXISTS is_standard BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE pack_registry_index
SET is_standard = TRUE
WHERE url = 'https://raw.githubusercontent.com/attune-system/index/c9e48439677847797d056efb94ba1c855e188df9/index.json';

CREATE UNIQUE INDEX IF NOT EXISTS pack_registry_index_one_standard
    ON pack_registry_index ((is_standard))
    WHERE is_standard;

CREATE TABLE IF NOT EXISTS standard_pack_index_seed_state (
    id SMALLINT PRIMARY KEY CHECK (id = 1),
    seeded_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO standard_pack_index_seed_state (id)
SELECT 1
WHERE EXISTS (
    SELECT 1
    FROM pack_registry_index
    WHERE is_standard
)
ON CONFLICT (id) DO NOTHING;
