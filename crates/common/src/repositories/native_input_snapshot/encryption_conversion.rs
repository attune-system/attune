//! Typed traversal of ciphertext metadata. User projections, schemas, origins,
//! executable snapshots and arbitrary Value payloads are never recursively edited.
use super::*;
use crate::repositories::encryption_conversion::VerifierConversion;

pub(in crate::repositories) fn convert_encryption_metadata(
    value: &Value,
    conversion: &VerifierConversion<'_>,
) -> Result<Value> {
    let mut body: Body = serde_json::from_value(value.clone())?;
    if serde_json::to_value(&body)? != *value {
        return Err(denied());
    }
    convert_body(&mut body, conversion, 0)?;
    let value = serde_json::to_value(body)?;
    bounded(&value)?;
    Ok(value)
}

pub(super) fn fields(
    fields: &mut [ArchivedField],
    conversion: &VerifierConversion<'_>,
) -> Result<()> {
    for field in fields {
        field.provenance()?;
        conversion.field(&field.encrypted_value, &mut field.encryption_key_hash)?;
    }
    Ok(())
}

pub(super) fn convert_body(
    body: &mut Body,
    conversion: &VerifierConversion<'_>,
    depth: usize,
) -> Result<()> {
    if depth > 32 {
        return Err(denied());
    }
    match &mut body.source {
        Source::Cache(_) | Source::EmptyIteration(_) => {}
        Source::Wait(source) => {
            if source.source_version.get("ciphertext_digest")
                != Some(&json!(hash(&serde_json::to_value(&source.fields)?)?))
            {
                return Err(denied());
            }
            for contributor in &source.contributors {
                contributor.validate()?;
            }
            fields(&mut source.fields, conversion)?;
            source.source_version["ciphertext_digest"] =
                json!(hash(&serde_json::to_value(&source.fields)?)?);
        }
        Source::ResultView(source) => result_view::convert_encryption_metadata(source, conversion)?,
        Source::Completion(source) => {
            completion::validate_storage(source)?;
            for child in &mut source.children {
                result_view::convert_encryption_metadata(child, conversion)?;
            }
            fields(&mut source.fields, conversion)?;
        }
        Source::IterationOutcome(source) => {
            iteration_outcome::convert_encryption_metadata(source, conversion)?
        }
        Source::SuccessMetadata(source) => {
            success_metadata::convert_encryption_metadata(source, conversion, depth + 1)?
        }
        Source::WaitCompletion(source) => {
            wait_completion::convert_encryption_metadata(source, conversion, depth + 1)?
        }
    }
    Ok(())
}
