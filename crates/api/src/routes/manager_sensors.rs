//! The API owns sensor admission and process generation changes.
use attune_common::{
    manager_control::{ClaimWork, SensorLaunch, SensorLease},
    models::{Id, WorkerRole},
    repositories::{
        manager_runtime::ManagerRuntimeRepository, manager_sensor::ManagerSensorRepository,
        runtime_preparation::RuntimePreparationRepository, schema_type::SchemaTypeRepository,
        sensor_admission::SensorAdmissionRepository, sensor_schema::SensorSchemaRepository,
        SensorRepository, SensorWorkloadRepository,
    },
    runtime_lifecycle::{PreparationScope, PreparationTarget},
};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};

use crate::{
    auth::middleware::RequireAuth,
    middleware::ApiError,
    routes::{
        auth::{issue_sensor_token, CreateSensorTokenRequest},
        managers::ManagerSessionAuthorization,
    },
    state::SharedState,
};

pub(crate) async fn available(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
) -> Result<Json<Vec<Id>>, ApiError> {
    let mut tx = state.db.begin().await?;
    SensorAdmissionRepository::lock_workload_checks(&mut tx).await?;
    SchemaTypeRepository::lock_selection(&mut tx).await?;
    let manager = auth.live_record(&state, &mut tx).await?;
    if manager.role != WorkerRole::Sensor {
        return Err(denied());
    }
    let worker = ManagerRuntimeRepository::lock_worker(&mut tx, &manager).await?;
    let sensors = SensorRepository::find_enabled(&mut *tx).await?;
    let capabilities =
        SensorAdmissionRepository::worker_eligibility_by_sensor(&mut tx, &sensors, worker).await?;
    let candidates = sensors
        .into_iter()
        .filter(|sensor| {
            capabilities.get(&sensor.id).is_some_and(|entry| {
                entry.eligible && (entry.active_rule_count > 0 || sensor.has_transport_demand())
            })
        })
        .map(|sensor| sensor.id)
        .collect();
    ManagerRuntimeRepository::check_deadline(&mut tx, &manager, auth.bearer_expires_at()).await?;
    tx.commit().await?;
    Ok(Json(candidates))
}

pub(crate) async fn start(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path(sensor): Path<Id>,
    Json(request): Json<ClaimWork>,
) -> Result<impl IntoResponse, ApiError> {
    let mut tx = state.db.begin().await?;
    SensorAdmissionRepository::lock_workload_checks(&mut tx).await?;
    SchemaTypeRepository::lock_selection(&mut tx).await?;
    let manager = auth.live_record(&state, &mut tx).await?;
    let key = state.config.security.encryption_key.as_deref();
    let fence =
        ManagerSensorRepository::start(&mut tx, &manager, sensor, request.request, key).await?;
    let (binding_id, binding, public) =
        SensorSchemaRepository::find(&mut *tx, fence.workload_id, fence.generation)
            .await?
            .ok_or_else(denied)?;
    let config =
        SensorSchemaRepository::restore_config(&mut tx, binding_id, &binding, public, key).await?;
    let triggers = serde_json::to_value(
        SensorSchemaRepository::restore_initial_rules(&mut tx, binding_id, &binding, key).await?,
    )
    .map_err(attune_common::Error::from)?;
    let executable = binding.executable;
    let worker = attune_common::repositories::WorkerRepository::find_registration_for_share(
        &mut tx,
        fence.worker_id,
    )
    .await?
    .ok_or_else(denied)?;
    let version = crate::routes::manager_actions::select_version(
        &executable.runtime,
        &executable.runtime_versions,
        executable.sensor.runtime_version_constraint.as_deref(),
        &worker,
    )?;
    let recipe = version
        .as_ref()
        .map(|version| version.parsed_execution_config())
        .unwrap_or_else(|| executable.runtime.parsed_execution_config())
        .lifecycle;
    let prepared = if let Some(target) = recipe
        .as_ref()
        .map(|recipe| {
            PreparationTarget::for_sensor(&executable.sensor, &executable.runtime, recipe)
        })
        .transpose()
        .map_err(|_| denied())?
        .flatten()
    {
        let scope = PreparationScope::for_worker(&worker).ok_or_else(denied)?;
        Some(
            RuntimePreparationRepository::ready_for(
                &mut *tx,
                executable.release.id,
                &executable.release.digest,
                &target,
            )
            .await?
            .into_iter()
            .find(|ready| ready.scope == scope)
            .ok_or_else(denied)?,
        )
    } else {
        None
    };
    let token = issue_sensor_token(
        &state,
        &mut tx,
        CreateSensorTokenRequest {
            sensor_ref: executable.sensor.r#ref.clone(),
            pack_ref: executable.sensor.pack_ref.clone(),
            trigger_types: executable
                .triggers
                .iter()
                .map(|trigger| trigger.r#ref.clone())
                .collect(),
            permission_set_refs: executable.cache_permission_set_refs.clone(),
            ttl_seconds: Some(3600),
        },
        true,
        fence,
    )
    .await?;
    SensorWorkloadRepository::recheck_locked_fence(&mut tx, sensor, fence).await?;
    ManagerRuntimeRepository::check_deadline(&mut tx, &manager, auth.bearer_expires_at()).await?;
    tx.commit().await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(SensorLaunch {
            fence,
            executable,
            config,
            triggers,
            version,
            prepared,
            token: token.token,
            token_expiry: token.expires_at,
        }),
    ))
}

pub(crate) async fn renew(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path((workload, generation)): Path<(Id, i64)>,
) -> Result<Json<SensorLease>, ApiError> {
    let mut tx = state.db.begin().await?;
    SensorAdmissionRepository::lock_workload_checks(&mut tx).await?;
    SchemaTypeRepository::lock_selection(&mut tx).await?;
    let manager = auth.live_record(&state, &mut tx).await?;
    let expires = ManagerSensorRepository::renew(&mut tx, &manager, workload, generation).await?;
    ManagerRuntimeRepository::check_deadline(&mut tx, &manager, auth.bearer_expires_at()).await?;
    tx.commit().await?;
    Ok(Json(SensorLease { expires }))
}

pub(crate) async fn release(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path((workload, generation)): Path<(Id, i64)>,
) -> Result<StatusCode, ApiError> {
    let mut tx = state.db.begin().await?;
    let manager = auth.live_record(&state, &mut tx).await?;
    ManagerSensorRepository::release(&mut tx, &manager, workload, generation).await?;
    ManagerRuntimeRepository::check_deadline(&mut tx, &manager, auth.bearer_expires_at()).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn denied() -> ApiError {
    ApiError::Forbidden("Sensor workload is unavailable".into())
}

pub(crate) async fn open_log(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path((workload, generation)): Path<(Id, i64)>,
    Json(request): Json<attune_common::manager_control::OpenLog>,
) -> Result<impl IntoResponse, ApiError> {
    use attune_common::{
        manager_control::SensorLog,
        models::{
            ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType,
            RetentionPolicyType,
        },
        repositories::{
            artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
            file_evidence_retention::FileEvidenceRetentionRepository,
            log_stream::LogStreamRepository,
        },
    };
    let mut tx = state.db.begin().await?;
    FileEvidenceRetentionRepository::lock_capture(&mut tx).await?;
    SensorAdmissionRepository::lock_workload_checks(&mut tx).await?;
    SchemaTypeRepository::lock_selection(&mut tx).await?;
    let manager = auth.live_record(&state, &mut tx).await?;
    let fence =
        ManagerSensorRepository::owned_fence(&mut tx, &manager, workload, generation).await?;
    let sensor = SensorWorkloadRepository::find_by_id(&mut *tx, workload)
        .await?
        .ok_or_else(denied)?
        .sensor;
    SensorWorkloadRepository::lock_current_fence_workload(&mut tx, sensor, fence)
        .await?
        .ok_or_else(denied)?;
    let channel = request.channel.as_str();
    let segments = state.config.artifacts.log_segment_writer_config();
    let version = if let Some(version) =
        ManagerSensorRepository::existing_log(&mut tx, fence, request.request, channel).await?
    {
        version
    } else {
        let (binding_id, binding, _) = SensorSchemaRepository::find(&mut *tx, workload, generation)
            .await?
            .ok_or_else(denied)?;
        let sensor = &binding.executable.sensor;
        let reference = format!("sensor.{}.{channel}", sensor.r#ref);
        let artifact = ArtifactRepository::create_or_get(
            &mut tx,
            CreateArtifactInput {
                r#ref: reference.clone(),
                scope: OwnerType::Sensor,
                owner: sensor.r#ref.clone(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::RuntimeLog,
                retention_policy: sensor
                    .log_retention_policy
                    .unwrap_or(RetentionPolicyType::Versions),
                retention_limit: sensor
                    .log_retention_limit
                    .unwrap_or(state.config.artifacts.sensor_log_max_files as i32),
                name: None,
                description: None,
                content_type: Some("text/plain".into()),
                data: None,
            },
        )
        .await?;
        if artifact.classification != ArtifactClassification::RuntimeLog
            || artifact.visibility != ArtifactVisibility::Private
            || artifact.scope != OwnerType::Sensor
            || artifact.owner != sensor.r#ref
        {
            return Err(denied());
        }
        let version = ArtifactVersionRepository::create_file_backed_with_conn(
            &mut tx,
            artifact.id,
            &reference,
            "text/plain".into(),
            None,
            Some(serde_json::json!({"sensor_ref": sensor.r#ref, "stream": channel})),
            Some("sensor".into()),
        )
        .await?;
        SensorSchemaRepository::attach_log_version(&mut tx, version.id, binding_id).await?;
        LogStreamRepository::create_with_backend_with_conn(
            &mut tx,
            version.id,
            attune_common::models::LogStreamBackend::ObjectSegments,
            segments.max_segment_bytes as u64,
            segments.flush_interval_ms,
        )
        .await?;
        ManagerSensorRepository::bind_log(&mut tx, fence, request.request, channel, version.id)
            .await?;
        version.id
    };
    let stream = LogStreamRepository::find_by_artifact_version_with_conn(&mut tx, version)
        .await?
        .ok_or_else(denied)?;
    SensorWorkloadRepository::recheck_locked_fence(&mut tx, sensor, fence).await?;
    ManagerRuntimeRepository::check_deadline(&mut tx, &manager, auth.bearer_expires_at()).await?;
    tx.commit().await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(SensorLog {
            version,
            rotation_bytes: state.config.artifacts.sensor_log_max_bytes,
            rotates: state.config.artifacts.sensor_log_max_files > 0,
            segment_bytes: usize::try_from(stream.max_unflushed_bytes).map_err(|_| denied())?,
        }),
    ))
}

pub(crate) async fn report_process(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path((workload, generation)): Path<(Id, i64)>,
    Json(report): Json<attune_common::manager_control::SensorProcessReport>,
) -> Result<StatusCode, ApiError> {
    let mut tx = state.db.begin().await?;
    SensorAdmissionRepository::lock_workload_checks(&mut tx).await?;
    SchemaTypeRepository::lock_selection(&mut tx).await?;
    let manager = auth.live_record(&state, &mut tx).await?;
    let failed =
        ManagerSensorRepository::report_process(&mut tx, &manager, workload, generation, report)
            .await?;
    ManagerRuntimeRepository::check_deadline(&mut tx, &manager, auth.bearer_expires_at()).await?;
    tx.commit().await?;
    if let Some(failed) = failed.filter(|p| {
        p.consecutive_failures >= 3 && p.consecutive_failures > p.last_alerted_failure_count
    }) {
        sensor_alert(&state, failed).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn sensor_alert(state: &SharedState, failed: attune_common::models::SensorProcess) {
    use attune_common::{
        repositories::sensor_process::{RecordSensorProcessAlertedInput, SensorProcessRepository},
        system_alert::{emit_core_alert, SystemAlert},
    };
    let alert = SystemAlert {
        severity: "error".into(),
        category: "sensor_process_health".into(),
        failure_type: "sensor_process_repeated_failure".into(),
        component_type: "sensor".into(),
        component_id: Some(failed.sensor),
        component_ref: Some(failed.sensor_ref.clone()),
        worker_role: Some("sensor".into()),
        observed_at: chrono::Utc::now(),
        summary: format!(
            "Sensor '{}' failed {} consecutive times",
            failed.sensor_ref, failed.consecutive_failures
        ),
        details: serde_json::json!({"worker_id": failed.worker, "generation_failures": failed.consecutive_failures}),
        correlation_id: Some(format!(
            "sensor:{}:worker:{}:repeated_failure",
            failed.sensor, failed.worker
        )),
    };
    let publisher = state.get_publisher().await;
    match emit_core_alert(&state.db, publisher.as_deref(), alert).await {
        Ok(Some(_)) => {
            if let Err(error) = SensorProcessRepository::record_alerted(
                &state.db,
                RecordSensorProcessAlertedInput {
                    sensor: failed.sensor,
                    worker: failed.worker,
                    failure_count: failed.consecutive_failures,
                    alerted_at: None,
                },
            )
            .await
            {
                tracing::warn!(%error, sensor = failed.sensor, "Could not record sensor failure notification");
            }
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(%error, sensor = failed.sensor, "Could not publish sensor failure notification")
        }
    }
}
