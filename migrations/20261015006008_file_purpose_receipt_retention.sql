-- Requires A d59e06e6/a236be88/3196c929 and their 01011/01012 schema.
DO $$ BEGIN
    IF to_regclass('runtime_input_receipt') IS NULL OR to_regclass('runtime_input_receipt_dependency') IS NULL THEN
        RAISE EXCEPTION 'Purpose file retention requires the committed A purpose receipt and native dependency migrations';
    END IF;
END $$;

CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON runtime_input_receipt
FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_delete_entry();
CREATE TRIGGER file_evidence_guard BEFORE DELETE ON runtime_input_receipt
FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_delete('runtime_input_receipt','id');
CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON runtime_declared_input_binding
FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_delete_entry();
CREATE TRIGGER file_evidence_guard BEFORE DELETE ON runtime_declared_input_binding
FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_delete('runtime_declared_input_binding','id');
CREATE TRIGGER file_evidence_input_binding_immutable BEFORE UPDATE ON runtime_declared_input_binding
FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE TRIGGER file_evidence_configuration_reference_immutable BEFORE UPDATE ON runtime_input_configuration_reference
FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();
CREATE TRIGGER file_evidence_admission_dependency_immutable BEFORE UPDATE ON runtime_admission_dependency
FOR EACH ROW EXECUTE FUNCTION guard_runtime_schema_binding();

-- Approval replacements append declaration links. Freeze only the approvals and
-- exact original Key reads selected by a pinned immutable receipt, not the
-- declaration's mutable current approval list or any live Key row.
CREATE FUNCTION file_evidence_input_selection_pinned(d BIGINT,a BIGINT,origin JSONB DEFAULT NULL)
RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
    SELECT EXISTS(SELECT 1 FROM file_evidence_pin p JOIN runtime_input_receipt r
        ON p.kind='runtime_input_receipt' AND p.source=r.id
      WHERE r.declaration=d AND EXISTS(SELECT 1 FROM jsonb_array_elements(r.proof->'selections') selected(value)
        WHERE selected.value->'source'->'input_authority'=to_jsonb(a)
          AND (origin IS NULL OR EXISTS(SELECT 1 FROM jsonb_array_elements(selected.value->'source'->'origins') original(value) WHERE original.value=origin))))
$$;

CREATE FUNCTION file_evidence_guard_input_secondary() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE b JSONB:=to_jsonb(OLD);
BEGIN
    IF NOT file_evidence_graph_held(true) THEN RAISE EXCEPTION 'Input receipt GC must enter exclusive graph admission before source locks'; END IF;
    IF TG_TABLE_NAME IN ('runtime_input_receipt_source','runtime_input_receipt_dependency','runtime_input_configuration_reference') THEN
        IF file_evidence_pinned('runtime_input_receipt',(b->>'receipt')::BIGINT) THEN RETURN NULL; END IF;
    ELSIF TG_TABLE_NAME='runtime_admission_dependency' THEN
        IF file_evidence_pinned('runtime_authority_admission',(b->>'admission')::BIGINT) THEN RETURN NULL; END IF;
    ELSIF TG_TABLE_NAME='runtime_input_only_approval' THEN
        IF file_evidence_pinned('runtime_external_approval',(b->>'approval')::BIGINT) THEN RETURN NULL; END IF;
    ELSIF TG_TABLE_NAME='runtime_declared_input_approval' THEN
        IF file_evidence_input_selection_pinned((b->>'declaration')::BIGINT,(b->>'approval')::BIGINT) THEN RETURN NULL; END IF;
    ELSIF TG_TABLE_NAME='runtime_declared_input_key_read' THEN
        IF file_evidence_input_selection_pinned((b->>'declaration')::BIGINT,(b->>'approval')::BIGINT,b->'origin') THEN RETURN NULL; END IF;
    END IF;
    RETURN OLD;
END;
$$;
DO $$ DECLARE t TEXT; BEGIN
    FOREACH t IN ARRAY ARRAY['runtime_input_receipt_source','runtime_input_receipt_dependency','runtime_input_configuration_reference','runtime_admission_dependency','runtime_input_only_approval','runtime_declared_input_approval','runtime_declared_input_key_read'] LOOP
        EXECUTE format('CREATE TRIGGER aaa_file_evidence_entry BEFORE DELETE ON %I FOR EACH STATEMENT EXECUTE FUNCTION file_evidence_delete_entry()',t);
        EXECUTE format('CREATE TRIGGER file_evidence_guard BEFORE DELETE ON %I FOR EACH ROW EXECUTE FUNCTION file_evidence_guard_input_secondary()',t);
    END LOOP;
END; $$;
