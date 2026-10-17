-- A 01017's immutable original frontier body and actual normalized binding are
-- different identities. Preserve both without changing validated migrations.
-- Creation history must outlive raw iteration GC while a file owns its page.
ALTER TABLE runtime_cache_iteration_epoch
    DROP CONSTRAINT runtime_cache_iteration_epoch_iteration_fkey;

CREATE FUNCTION file_evidence_cache_epoch_pinned(i BIGINT) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
    SELECT file_evidence_pinned('runtime_cache_iteration_epoch',i)
       OR EXISTS(SELECT 1 FROM runtime_native_cache_frontier f
           WHERE f.bound_iteration=i AND file_evidence_pinned('runtime_native_input_snapshot',f.snapshot))
$$;

CREATE FUNCTION file_evidence_guard_native_frontier_delete() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT file_evidence_graph_held(true) THEN RAISE EXCEPTION 'Native frontier GC requires exclusive graph admission'; END IF;
    IF file_evidence_pinned('runtime_native_input_snapshot',OLD.snapshot) THEN RETURN NULL; END IF;
    RETURN OLD;
END; $$;
CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON runtime_native_cache_frontier
FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_delete_entry();
-- Run before A's immutable association guard. Keep its binding/update checks.
CREATE TRIGGER aaa_file_evidence_native_frontier BEFORE DELETE ON runtime_native_cache_frontier
FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_native_frontier_delete();

CREATE FUNCTION file_evidence_guard_cache_epoch_delete() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NOT file_evidence_graph_held(true) THEN RAISE EXCEPTION 'Native epoch GC requires exclusive graph admission'; END IF;
    IF file_evidence_cache_epoch_pinned(OLD.iteration) THEN RETURN NULL; END IF;
    RETURN OLD;
END; $$;
CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON runtime_cache_iteration_epoch
FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_delete_entry();
CREATE TRIGGER aaa_file_evidence_cache_epoch BEFORE DELETE ON runtime_cache_iteration_epoch
FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_cache_epoch_delete();

-- 06013's candidate selector and pre-DROP function already call this predicate
-- under graph-first admission. Retain exact generation-qualified entry pins and
-- additionally honor actual normalized frontier bindings while raw rows exist.
CREATE OR REPLACE FUNCTION file_evidence_cache_generation_pinned(g BIGINT) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
    SELECT file_evidence_pinned('cache_generation',g)
       OR EXISTS(SELECT 1 FROM file_evidence_pin WHERE kind='cache_entry' AND source_generation=g)
       OR EXISTS(SELECT 1 FROM runtime_native_cache_frontier f
           JOIN workflow_cache_iteration i ON i.id=f.bound_iteration
           WHERE i.generation=g AND file_evidence_pinned('runtime_native_input_snapshot',f.snapshot))
$$;

-- Snapshot predecessor and actual iteration associations are never reconstructed
-- from JSON origin paths. A's predecessor FK and immutable frontier/epoch update
-- guards remain. Bounded F collection removes only orphan history after all
-- frontier associations, raw iteration rows and exact physical file pins clear.
