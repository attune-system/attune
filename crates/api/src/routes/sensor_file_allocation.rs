//! Native sensor ordinary-file hooks; runtime-log generation bindings stay separate.
use super::*;
use attune_common::repositories::{
    sensor_workload::native_effect::SensorFileAllocation, SensorWorkloadRepository,
};

pub(super) async fn allocation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user: &AuthenticatedUser,
) -> ApiResult<Option<SensorFileAllocation>> {
    if user.claims.token_type != TokenType::Sensor {
        return Ok(None);
    }
    let reference = user
        .claims
        .metadata
        .as_ref()
        .and_then(|m| m.get("sensor_ref"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::Forbidden("Sensor file source unavailable".into()))?;
    let sensor = SensorRepository::find_by_ref(&mut **tx, reference)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Sensor file source unavailable".into()))?;
    let fence = user.sensor_workload_fence()?;
    Ok(SensorWorkloadRepository::lock_native_file_allocation(tx, sensor.id, fence).await?)
}

pub(super) async fn bind(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    allocation: Option<SensorFileAllocation>,
    version: i64,
) -> ApiResult<()> {
    if let Some(allocation) = allocation {
        SensorWorkloadRepository::bind_native_file(tx, allocation, version).await?;
    }
    Ok(())
}
