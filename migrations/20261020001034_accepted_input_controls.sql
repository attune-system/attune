-- Each root lookup reaches at most its one execution-owned accepted archive.
-- Malformed manifests conservatively retain only that owner's actual roots.
CREATE FUNCTION runtime_accepted_input_holds_control(receipt_id BIGINT)
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1
        FROM runtime_input_receipt receipt
        JOIN runtime_accepted_input accepted ON accepted.execution=receipt.execution
            AND accepted.admission=receipt.destination_admission
        WHERE receipt.id=receipt_id
            AND receipt.entity_kind='execution_config'
            AND receipt.destination_purpose='control'
            AND receipt.destination_path=''
            AND CASE
                WHEN jsonb_typeof(accepted.body) IS DISTINCT FROM 'object' THEN TRUE
                WHEN NOT (accepted.body ? 'controls') THEN FALSE
                WHEN jsonb_typeof(accepted.body->'controls') IS DISTINCT FROM 'array' THEN TRUE
                ELSE CASE WHEN jsonb_array_length(accepted.body->'controls') > 256 THEN TRUE
                    ELSE EXISTS (
                        SELECT 1 FROM jsonb_array_elements(accepted.body->'controls') entry
                        WHERE jsonb_typeof(entry) IS DISTINCT FROM 'object'
                            OR entry IS DISTINCT FROM jsonb_build_object(
                                'receipt',entry->'receipt','site',entry->'site')
                            OR jsonb_typeof(entry->'receipt') IS DISTINCT FROM 'number'
                            OR COALESCE(entry->>'receipt','') !~ '^[1-9][0-9]{0,18}$'
                            OR (length(entry->>'receipt')=19
                                AND entry->>'receipt' > '9223372036854775807')
                            OR entry->'site' IS DISTINCT FROM jsonb_build_object(
                                'execution',receipt.execution,'kind','execution_config',
                                'purpose','control','path','')
                    )
                    OR EXISTS (
                        SELECT 1 FROM jsonb_array_elements(accepted.body->'controls') entry
                        GROUP BY entry->'receipt' HAVING count(*) > 1
                    )
                    OR accepted.body->'controls' @> jsonb_build_array(jsonb_build_object(
                        'receipt',receipt.id,'site',jsonb_build_object(
                            'execution',receipt.execution,'kind','execution_config',
                            'purpose','control','path','')))
                END
            END
    );
$$;

-- Preserve the current deletion policy, adding only the accepted root pin.
CREATE OR REPLACE FUNCTION delete_runtime_authority_admission() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE source_id BIGINT; accepted BOOLEAN := FALSE;
BEGIN
    IF TG_TABLE_NAME = 'execution' THEN
        DELETE FROM runtime_declared_input_receipt WHERE execution = OLD.id;
        DELETE FROM runtime_input_receipt receipt WHERE execution = OLD.id
          AND NOT runtime_accepted_input_holds_control(receipt.id)
          AND NOT file_evidence_pinned('runtime_input_receipt',receipt.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_source edge WHERE edge.source_receipt = receipt.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot_parent parent WHERE parent.receipt=receipt.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_secret_origin_reference origin WHERE origin.source_id = receipt.id AND origin.source_entity LIKE 'declared_input:%');
    END IF;
    SELECT id INTO source_id FROM runtime_authority_admission WHERE owner_kind = TG_TABLE_NAME AND owner_id = OLD.id;
    IF to_regclass('work_queue_item_ad_hoc_dispatch') IS NOT NULL THEN
        EXECUTE 'SELECT EXISTS(SELECT 1 FROM work_queue_item_ad_hoc_dispatch WHERE source_admission = $1)' INTO accepted USING source_id;
    END IF;
    IF NOT accepted THEN
        DELETE FROM runtime_authority_admission admission WHERE id = source_id
          AND NOT file_evidence_pinned('runtime_authority_admission',admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_admission_dependency edge WHERE edge.dependency = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_declared_input_receipt receipt WHERE receipt.source_admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt receipt WHERE receipt.renderer_admission = admission.id OR receipt.destination_admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_input_receipt_dependency proof WHERE proof.admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_accepted_input input WHERE input.admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot input WHERE input.renderer_admission = admission.id)
          AND NOT EXISTS(SELECT 1 FROM runtime_native_input_snapshot_admission input WHERE input.admission=admission.id);
    END IF;
    RETURN OLD;
END; $$;
