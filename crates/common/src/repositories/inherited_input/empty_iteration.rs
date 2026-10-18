//! Empty-list lifecycle selections stay CONDITION/CONTROL, never child Values.
use super::super::native_input_snapshot::PreparedNativeEmptyIterationOutcome;
use super::*;

impl InheritedInputRepository {
    /// Reopen the issued program's immutable receipts, including skipped false
    /// decisions and overwritten publication leaves. Never evaluate an expression
    /// or select a newest receipt at a familiar path during duplicate recovery.
    pub(crate) async fn reopen_empty_program_controls(
        conn: &mut PgConnection,
        renderer: Id,
        snapshot: Id,
        task_path: &str,
        expected_conditions: &[String],
        key: &str,
    ) -> Result<Vec<ReceiptContext>> {
        let candidates: Vec<(Id, Value, String)> = sqlx::query_as("SELECT id,proof,proof_digest FROM runtime_input_receipt WHERE renderer_admission=$1 AND EXISTS(SELECT 1 FROM jsonb_array_elements(proof->'selections') selection WHERE selection#>>'{source,purpose}'='native_snapshot' AND selection#>>'{source,snapshot}'=$2::TEXT) ORDER BY id LIMIT 257")
            .bind(renderer).bind(snapshot).fetch_all(&mut *conn).await?;
        if candidates.len() > 256 {
            return Err(denied());
        }
        let record = RuntimeAuthorityRepository::require_retained_admission(conn, renderer).await?;
        let mut conditions = BTreeSet::new();
        let mut held = Vec::new();
        for (id, body, expected) in candidates {
            if hash(&body)? != expected {
                return Err(denied());
            }
            let proof: RenderProof = serde_json::from_value(body)?;
            if proof.renderer != renderer || !proof.selections.iter().any(|selection| matches!(&selection.source, SelectionSource::NativeSnapshot { snapshot: issued } if *issued == snapshot)) { return Err(denied()); }
            let path = if let Some(declaration) = proof.declaration {
                let (body, expected): (Value, String) = sqlx::query_as("SELECT declaration,declaration_digest FROM runtime_declared_input_binding WHERE id=$1 AND policy_revision=$2")
                    .bind(declaration).bind(record.policy_revision).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
                if hash(&body)? != expected {
                    return Err(denied());
                }
                let declaration: super::super::declared_input::DeclaredInputBinding =
                    serde_json::from_value(body)?;
                DeclaredInputRepository::validate_declaration(conn, &record, &declaration).await?;
                declaration.source_path
            } else {
                proof.literal.as_ref().ok_or_else(denied)?.path.clone()
            };
            if !path.starts_with(&format!("{task_path}/next/")) {
                continue;
            }
            if expected_conditions.contains(&path) && !conditions.insert(path) {
                return Err(denied());
            }
            let origin = SecretOrigin::Entity {
                entity_type: proof.site.kind.origin_kind(proof.site.purpose),
                entity_id: id,
                path: proof.site.path.clone(),
            };
            held.push(Self::receipt_context(conn, &proof.site, &origin, key).await?);
        }
        if conditions != expected_conditions.iter().cloned().collect() {
            return Err(denied());
        }
        Ok(held)
    }
    pub(crate) async fn empty_iteration_receipt(
        conn: &mut PgConnection,
        id: Id,
        site: &InputSite,
        pending: bool,
        key: &str,
    ) -> Result<ReceiptContext> {
        let origin = SecretOrigin::Entity {
            entity_type: site.kind.origin_kind(site.purpose),
            entity_id: id,
            path: site.path.clone(),
        };
        if pending {
            Self::preparation_receipt_context(conn, site, &origin, key).await
        } else {
            Self::receipt_context(conn, site, &origin, key).await
        }
    }
    pub(crate) async fn empty_iteration_control(
        conn: &mut PgConnection,
        control: &PreparedIterationControl,
        key: &str,
    ) -> Result<ReceiptContext> {
        control.ensure_fresh_on(conn).await?;
        let receipt =
            Self::preparation_receipt_context(conn, control.site(), &control.origin(), key).await?;
        if receipt.value != serde_json::json!([])
            || control.value() != &receipt.value
            || receipt.site != *control.site()
        {
            return Err(denied());
        }
        Ok(receipt)
    }

    pub fn bind_empty_iteration_outcome(
        session: &mut RenderSession,
        source: &PreparedNativeEmptyIterationOutcome,
    ) -> Result<()> {
        if session.transaction != source.transaction
            || session.permit.source.id != source.renderer
            || session.purpose != InputSitePurpose::Condition
            || !super::super::native_input_snapshot::wait_outcome_path(
                &source.task_path,
                &session.permit.declaration.source_path,
            )
        {
            return Err(denied());
        }
        source.ensure_fresh()?;
        merge_empty_reads(&mut session.empty_reads, &[source.issued_read()?])?;
        session.fence = session.fence.intersect(&source.fence);
        for function in ["succeeded", "failed", "timed_out", "cancelled"] {
            insert(
                session,
                format!("/$outcome/{function}"),
                Value::Bool(function == "succeeded"),
                SelectionSource::NativeSnapshot {
                    snapshot: source.snapshot,
                },
            )?;
        }
        Ok(())
    }

    pub fn attach_empty_iteration_control(
        rendered: &mut RenderedReceiptContext,
        source: &PreparedNativeEmptyIterationOutcome,
    ) -> Result<()> {
        if rendered.transaction != source.transaction || rendered.renderer.id != source.renderer {
            return Err(denied());
        }
        source.ensure_fresh()?;
        merge_empty_reads(&mut rendered.empty_reads, &[source.issued_read()?])?;
        rendered.fence = rendered.fence.intersect(&source.fence);
        let selection = ReadSelection {
            context_path: "$control".into(),
            source: SelectionSource::NativeSnapshot {
                snapshot: source.snapshot,
            },
        };
        if !rendered.selections.contains(&selection) {
            rendered.selections.push(selection);
        }
        if rendered.selections.len() > 256 {
            return Err(denied());
        }
        Ok(())
    }
}
