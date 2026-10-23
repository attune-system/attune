-- Independent-retention links remain plain IDs, never partitioned Event FKs.
-- New accepted enforcements retain the timestamp that qualifies their source.
ALTER TABLE enforcement ADD COLUMN event_created TIMESTAMPTZ;
DROP INDEX uq_enforcement_rule_event;
CREATE UNIQUE INDEX uq_enforcement_rule_event
    ON enforcement (rule, event, event_created) NULLS NOT DISTINCT
    WHERE rule IS NOT NULL AND event IS NOT NULL;
