-- Migration: Execution pack-ref expression index
-- Description: Adds an expression index to accelerate execution pack-scoped
--              queries and totals that filter by split_part(action_ref, '.', 1).
-- Version: 20250101000019

SET search_path TO attune, public;

CREATE INDEX IF NOT EXISTS idx_execution_pack_ref_created_desc
    ON execution ((split_part(action_ref, '.', 1)), created DESC);
