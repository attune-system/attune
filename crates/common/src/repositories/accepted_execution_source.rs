//! Transaction-bound adapters around the canonical accepted-input verifier.
//! No acceptance body parser, authority reconstruction, or second origin walker.
use super::{
    accepted_input::AcceptedInputRepository,
    execution::ExecutionRepository,
    execution_secret_value::ExecutionSecretValueRepository as Secrets,
    inherited_input::ReceiptContext,
    schema_contract::{ActionSchemaBinding, BindingOwner, SchemaContractRepository},
    FindById,
};
use crate::{
    models::{Execution, Id},
    secret_provenance::SecretProvenance,
    secret_values::{StoredSecretValue, ENTITY_EXECUTION_CONFIG},
    Error, Result,
};
use sqlx::PgConnection;

/// Observe an already accepted source, including a legitimate terminal producer.
/// Every field must already be an authenticated first-delivery receipt. This
/// does not deliver inputs to it or authorize ordinary origins through current IAM.
pub(crate) async fn prepare_observation(
    conn: &mut PgConnection,
    source: &Execution,
    schema: &ActionSchemaBinding,
    key: &str,
) -> Result<(Vec<StoredSecretValue>, Vec<ReceiptContext>)> {
    let fields = validate_accepted(conn, source, schema, key).await?;
    let (ordinary, receipts) = super::immutable_input_origin::prepare_input_receipts(
        conn,
        ENTITY_EXECUTION_CONFIG,
        source.id,
        &fields,
        key,
    )
    .await?;
    if !ordinary.is_empty() {
        return Err(denied());
    }
    super::immutable_input_origin::bind_input_receipt_buffers(
        conn,
        ENTITY_EXECUTION_CONFIG,
        source.id,
        &fields,
    )
    .await?;
    for receipt in &receipts {
        receipt.ensure_fresh_on(conn).await?;
    }
    Ok((fields, receipts))
}

pub(crate) async fn validate_accepted(
    conn: &mut PgConnection,
    source: &Execution,
    schema: &ActionSchemaBinding,
    key: &str,
) -> Result<Vec<StoredSecretValue>> {
    // Retention protection only, not a second interpretation of acceptance.
    let _: Id =
        sqlx::query_scalar("SELECT id FROM runtime_accepted_input WHERE execution=$1 FOR SHARE")
            .bind(source.id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(denied)?;
    let fence = AcceptedInputRepository::revalidate_execution(conn, source.id, key).await?;
    let actual = ExecutionRepository::find_by_id(&mut *conn, source.id)
        .await?
        .ok_or_else(denied)?;
    same_execution(source, &actual)?;
    if SchemaContractRepository::find(conn, BindingOwner::Execution, source.id)
        .await?
        .as_ref()
        != Some(schema)
    {
        return Err(denied());
    }
    let fields =
        Secrets::find_stored_by_entity(&mut *conn, ENTITY_EXECUTION_CONFIG, source.id).await?;
    if fields.len() > 256 {
        return Err(denied());
    }
    for field in &fields {
        if field.source_kind != "provenance" {
            return Err(denied());
        }
        let reference = field.source_ref.as_deref().ok_or_else(denied)?;
        if reference.len() > 1024 * 1024 {
            return Err(denied());
        }
        let provenance: SecretProvenance = serde_json::from_str(reference)?;
        if provenance.origins.is_empty()
            || provenance
                .origins
                .iter()
                .any(|origin| !super::immutable_input_origin::is_input_receipt(origin))
        {
            return Err(denied());
        }
    }
    fence.ensure_fresh_on(conn).await?;
    Ok(fields)
}

pub(crate) fn same_execution(expected: &Execution, actual: &Execution) -> Result<()> {
    if serde_json::to_value(expected)? != serde_json::to_value(actual)?
        || serde_json::to_value(&expected.executable_snapshot)?
            != serde_json::to_value(&actual.executable_snapshot)?
    {
        return Err(denied());
    }
    Ok(())
}
fn denied() -> Error {
    Error::PermissionDenied("Retained accepted execution inputs are unavailable".into())
}
