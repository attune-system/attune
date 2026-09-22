use std::sync::Arc;

use attune_common::{
    auth::jwt::TokenType,
    crypto::encrypt_json,
    inquiry_callback_adapter::sensor_inquiry_callback_adapters,
    models::trigger::SensorExecutableSnapshot,
    repositories::{
        inquiry_callback_delivery::{
            CreateInquiryCallbackDelivery, InquiryCallbackDeliveryRepository,
            InsertInquiryCallbackDelivery,
        },
        sensor_admission::SensorAdmissionRepository,
        sensor_workload::SensorWorkloadRepository,
        trigger::SensorRepository,
        FindById, FindByRef,
    },
};
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    routing::post,
    Json, Router,
};
use serde::Serialize;
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

use crate::{
    auth::middleware::RequireAuth,
    middleware::{ApiError, ApiResult},
    state::AppState,
};

const MAX_CALLBACK_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Serialize)]
struct CallbackAcknowledgement {
    acknowledge: bool,
}

async fn receive_sensor_callback(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(adapter_ref): Path<String>,
    Json(raw): Json<JsonValue>,
) -> ApiResult<Json<CallbackAcknowledgement>> {
    if user.claims.token_type != TokenType::Sensor {
        return Err(ApiError::Forbidden(
            "Managed sensor callbacks require a sensor token".to_string(),
        ));
    }
    let fence = user
        .sensor_workload_fence()
        .map_err(|_| ApiError::Forbidden("Sensor token has no valid workload fence".to_string()))?;
    let identity_id = user
        .claims
        .sub
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| ApiError::Forbidden("Sensor token identity is invalid".to_string()))?;
    let request_digest = canonical_request_digest(&raw)?;

    let mut tx = state.db.begin().await?;
    let sensor = SensorRepository::find_by_ref(&mut *tx, &user.claims.login)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Sensor token identity is invalid".to_string()))?;
    SensorAdmissionRepository::lock_workload_checks(&mut tx).await?;
    let workload = SensorWorkloadRepository::lock_current_fence_workload(&mut tx, sensor.id, fence)
        .await?
        .ok_or_else(|| {
            ApiError::Forbidden("Sensor workload assignment is stale or expired".to_string())
        })?;
    let sensor = SensorRepository::find_by_id(&mut *tx, sensor.id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Sensor callback adapter is unavailable".to_string()))?;
    if !sensor.enabled || sensor.retired_at.is_some() {
        return Err(ApiError::Forbidden(
            "Sensor callback adapter is unavailable".to_string(),
        ));
    }
    let live_adapters = sensor_inquiry_callback_adapters(sensor.config.as_ref())
        .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
    if !live_adapters
        .get(&adapter_ref)
        .is_some_and(|adapter| adapter.enabled)
    {
        return Err(ApiError::Forbidden(
            "Sensor callback adapter is unavailable".to_string(),
        ));
    }

    let identity = attune_common::repositories::identity::IdentityRepository::find_by_id(
        &mut *tx,
        identity_id,
    )
    .await?
    .ok_or_else(|| ApiError::Forbidden("Sensor token identity is invalid".to_string()))?;
    if identity.frozen || identity.login != format!("sensor:{}", sensor.r#ref) {
        return Err(ApiError::Forbidden(
            "Sensor token identity is invalid".to_string(),
        ));
    }
    let pack_release = workload.pack_release.ok_or_else(|| {
        ApiError::Forbidden("Sensor workload has no pinned callback revision".to_string())
    })?;
    let pack_release_digest = workload.pack_release_digest.as_deref().ok_or_else(|| {
        ApiError::Forbidden("Sensor workload has no pinned callback revision".to_string())
    })?;
    let executable_snapshot = workload.executable_snapshot.clone().ok_or_else(|| {
        ApiError::Forbidden("Sensor workload has no pinned callback revision".to_string())
    })?;
    let snapshot: SensorExecutableSnapshot =
        serde_json::from_value(executable_snapshot).map_err(|_| {
            ApiError::InternalServerError("Pinned sensor callback metadata is invalid".to_string())
        })?;
    if snapshot.sensor.id != sensor.id
        || snapshot.release.id != pack_release
        || snapshot.release.digest != pack_release_digest
    {
        return Err(ApiError::Forbidden(
            "Pinned sensor callback metadata does not match the workload".to_string(),
        ));
    }
    let adapters = sensor_inquiry_callback_adapters(snapshot.sensor.config.as_ref())
        .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
    let adapter = adapters
        .get(&adapter_ref)
        .filter(|adapter| adapter.enabled)
        .ok_or_else(|| ApiError::Forbidden("Sensor callback adapter is unavailable".to_string()))?;
    let (provider_delivery_id, selection) = adapter
        .normalize(&raw)
        .map_err(|_| ApiError::BadRequest("Invalid managed sensor inquiry callback".to_string()))?;

    let encryption_key = state
        .config
        .security
        .encryption_key
        .as_deref()
        .ok_or_else(|| {
            ApiError::InternalServerError(
                "Inquiry callback ingress requires security.encryption_key".to_string(),
            )
        })?;
    let encrypted_payload = encrypt_json(
        &serde_json::to_value(&selection).map_err(|_| {
            ApiError::InternalServerError("Failed to encode callback delivery".to_string())
        })?,
        encryption_key,
    )?;

    let delivery = InquiryCallbackDeliveryRepository::insert_or_load(
        &mut tx,
        CreateInquiryCallbackDelivery {
            sensor: sensor.id,
            integration_identity: identity_id,
            workload: workload.id,
            assignment_generation: fence.generation,
            pack_release,
            pack_release_digest,
            adapter_ref: &adapter_ref,
            provider_delivery_id: &provider_delivery_id,
            request_digest: &request_digest,
            encrypted_payload: &encrypted_payload,
        },
    )
    .await?;
    match delivery {
        InsertInquiryCallbackDelivery::DigestConflict => {
            return Err(ApiError::Conflict(
                "Callback delivery identifier was replayed with different content".to_string(),
            ));
        }
        InsertInquiryCallbackDelivery::Inserted(_) | InsertInquiryCallbackDelivery::Existing(_) => {
        }
    }
    tx.commit().await?;

    Ok(Json(CallbackAcknowledgement { acknowledge: true }))
}

fn canonical_request_digest(value: &JsonValue) -> ApiResult<String> {
    fn write(value: &JsonValue, output: &mut Vec<u8>) -> Result<(), serde_json::Error> {
        match value {
            JsonValue::Object(map) => {
                output.push(b'{');
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort_unstable();
                for (index, key) in keys.into_iter().enumerate() {
                    if index > 0 {
                        output.push(b',');
                    }
                    output.extend(serde_json::to_vec(key)?);
                    output.push(b':');
                    write(&map[key], output)?;
                }
                output.push(b'}');
            }
            JsonValue::Array(values) => {
                output.push(b'[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(b',');
                    }
                    write(value, output)?;
                }
                output.push(b']');
            }
            value => output.extend(serde_json::to_vec(value)?),
        }
        Ok(())
    }

    let mut canonical = Vec::new();
    write(value, &mut canonical).map_err(|_| {
        ApiError::InternalServerError("Failed to digest callback request".to_string())
    })?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(canonical))))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/internal/inquiry-callbacks/{adapter_ref}",
        post(receive_sensor_callback).layer(DefaultBodyLimit::max(MAX_CALLBACK_BODY_BYTES)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_digest_is_independent_of_object_key_order() {
        let left = serde_json::from_str(r#"{"a":1,"b":{"x":2,"y":3}}"#).unwrap();
        let right = serde_json::from_str(r#"{"b":{"y":3,"x":2},"a":1}"#).unwrap();
        assert_eq!(
            canonical_request_digest(&left).unwrap(),
            canonical_request_digest(&right).unwrap()
        );
    }

    #[test]
    fn request_digest_changes_with_unconstrained_content() {
        let left = json!({"delivery_id": "one", "ignored": true});
        let right = json!({"delivery_id": "one", "ignored": false});
        assert_ne!(
            canonical_request_digest(&left).unwrap(),
            canonical_request_digest(&right).unwrap()
        );
    }
}
