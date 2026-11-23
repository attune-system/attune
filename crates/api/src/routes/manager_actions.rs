//! Deliver the immutable inputs of a claimed action to its manager.
use attune_common::{
    auth::jwt::{generate_admitted_execution_token, generate_delegated_execution_token},
    manager_control::{ActionLaunch, ClaimWork, LogChannel},
    models::{Id, Runtime, RuntimeVersion, Worker},
    repositories::{
        delegated_execution::DelegatedExecutionRepository,
        event::EnforcementRepository,
        execution_runtime_admission::{ExecutionAdmissionRepository, ExistingExecutionAuthority},
        execution_secret_value::ExecutionSecretValueRepository,
        manager_assignment::ManagerAssignmentRepository,
        runtime_authority::RuntimeAuthorityRepository,
        runtime_preparation::RuntimePreparationRepository,
        schema_contract::{BindingOwner, SchemaContractRepository},
        schema_type::SchemaTypeRepository,
        FindById, PackReleaseRepository, WorkerRepository,
    },
    runtime_detection::normalize_runtime_name,
    runtime_lifecycle::{PreparationScope, PreparationTarget},
    value_schema::CompiledContract,
    version_matching::{runtime_version_matches_worker, select_best_version},
};
use axum::{
    extract::{Path, State},
    http::header,
    response::IntoResponse,
    Json,
};

use crate::{
    auth::middleware::RequireAuth, middleware::ApiError,
    routes::managers::ManagerSessionAuthorization, state::SharedState,
};

pub(crate) async fn launch(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path(assignment): Path<Id>,
    Json(request): Json<ClaimWork>,
) -> Result<impl IntoResponse, ApiError> {
    let mut tx = state.db.begin().await?;
    SchemaTypeRepository::lock_selection(&mut tx).await?;
    let scope = auth.work_scope(&state, &mut tx).await?;
    let (execution, deadline) = ManagerAssignmentRepository::execution_for_launch(
        &mut tx,
        scope,
        assignment,
        request.request,
    )
    .await?;
    let snapshot = execution
        .executable_snapshot
        .as_ref()
        .ok_or_else(unavailable)?;
    let action = &snapshot.executable.action;
    if action.workflow_def.is_some() || snapshot.executable.workflow_definition.is_some() {
        return Err(unavailable());
    }
    let runtime = snapshot
        .executable
        .runtime
        .as_ref()
        .ok_or_else(unavailable)?;
    let worker = WorkerRepository::find_by_id(&mut *tx, execution.worker.ok_or_else(unavailable)?)
        .await?
        .ok_or_else(unavailable)?;
    let version = select_version(
        runtime,
        &snapshot.executable.runtime_versions,
        action.runtime_version_constraint.as_deref(),
        &worker,
    )?;
    let recipe = runtime.parsed_execution_config().lifecycle;
    attune_common::execution_env::validate_runtime_environment(&runtime.execution_config)?;
    attune_common::execution_env::validate_selected_environment(
        Some(snapshot),
        version.as_ref().map(|version| version.version.as_str()),
        execution.env_vars.as_ref(),
    )?;
    let environment = execution
        .env_vars
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| unavailable())?
        .unwrap_or_default();
    let release = &snapshot.release;
    let prepared = if let Some(target) = recipe
        .as_ref()
        .map(|recipe| PreparationTarget::for_action(action, runtime, recipe))
        .transpose()
        .map_err(|_| unavailable())?
        .flatten()
    {
        let scope = PreparationScope::for_worker(&worker).ok_or_else(unavailable)?;
        let config = version
            .as_ref()
            .map(RuntimeVersion::parsed_execution_config)
            .unwrap_or_else(|| runtime.parsed_execution_config());
        if config
            .lifecycle
            .and_then(|recipe| recipe.digest().ok())
            .as_deref()
            != Some(target.recipe_digest.as_str())
        {
            return Err(unavailable());
        }
        Some(
            RuntimePreparationRepository::ready_for(&mut *tx, release.id, &release.digest, &target)
                .await?
                .into_iter()
                .find(|item| item.scope == scope)
                .ok_or_else(unavailable)?,
        )
    } else {
        None
    };
    let manifest = if matches!(
        action.launch,
        Some(attune_common::pack_format::LaunchSpec::Native { .. })
    ) {
        let row = PackReleaseRepository::find_by_id(&mut *tx, release.id)
            .await?
            .filter(|row| row.digest == release.digest && row.pack_ref == action.pack_ref)
            .ok_or_else(unavailable)?;
        Some(serde_json::from_value(row.manifest).map_err(|_| unavailable())?)
    } else {
        None
    };
    let schema = SchemaContractRepository::find(&mut tx, BindingOwner::Execution, execution.id)
        .await?
        .ok_or_else(unavailable)?;
    let (parameters, secret_paths, receipts) =
        ExecutionSecretValueRepository::restore_execution_parameters(
            &mut tx,
            execution.id,
            execution
                .config
                .clone()
                .unwrap_or_else(|| serde_json::json!({})),
            state.config.security.encryption_key.as_deref(),
        )
        .await?;
    CompiledContract::new(&schema.input)
        .and_then(|contract| contract.validate(&parameters))
        .map_err(|_| {
            ApiError::UnprocessableEntity(
                "Execution inputs do not match the admitted schema".into(),
            )
        })?;
    let parameters = parameters.as_object().cloned().ok_or_else(unavailable)?;
    let enforcement = if let Some(enforcement) = execution.enforcement {
        EnforcementRepository::find_by_id(&mut *tx, enforcement).await?
    } else {
        None
    };
    let timeout_seconds = ManagerAssignmentRepository::action_timeout(&execution, action);
    let mode = ExecutionAdmissionRepository::existing_authority(&mut tx, execution.id).await?;
    let mut runtime_fence = None;
    let child_token = if execution.permission_set_refs.is_empty() {
        None
    } else {
        let ttl = i64::try_from(timeout_seconds).map_err(|_| unavailable())?;
        let value = match mode {
            ExistingExecutionAuthority::Delegated => {
                DelegatedExecutionRepository::validate(&mut tx, execution.id).await?;
                let attempt =
                    DelegatedExecutionRepository::current_attempt(&mut tx, execution.id).await?;
                ManagerAssignmentRepository::bind_child_attempt(&mut tx, assignment, attempt)
                    .await?;
                let (actions, _) =
                    DelegatedExecutionRepository::standard_scope(&mut tx, &execution).await?;
                generate_delegated_execution_token(
                    execution.executor.ok_or_else(unavailable)?,
                    execution.id,
                    &execution.action_ref,
                    &state.jwt_config,
                    ttl,
                    &execution.permission_set_refs,
                    &actions,
                    attempt,
                )
                .map_err(|_| unavailable())?
            }
            ExistingExecutionAuthority::Recorded => {
                let binding = RuntimeAuthorityRepository::begin_execution_attempt(
                    &mut tx,
                    execution.id,
                    worker.id,
                    request.request,
                )
                .await?;
                ManagerAssignmentRepository::bind_child_attempt(
                    &mut tx,
                    assignment,
                    binding.attempt,
                )
                .await?;
                let authority =
                    RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &binding)
                        .await?;
                let token = generate_admitted_execution_token(
                    &binding,
                    &execution.permission_set_refs,
                    &state.jwt_config,
                    Some(ttl),
                )
                .map_err(|_| unavailable())?;
                authority.ensure_fresh_on(&mut tx).await?;
                runtime_fence = Some(authority);
                token
            }
        };
        Some(value)
    };
    ManagerAssignmentRepository::check_deadline(&mut tx, deadline).await?;
    tx.commit().await?;
    for receipt in receipts {
        receipt.ensure_fresh()?;
    }
    if let Some(fence) = runtime_fence {
        fence.effect_fence().ensure_fresh()?;
    }
    if chrono::Utc::now() >= deadline {
        return Err(unavailable());
    }
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(ActionLaunch {
            execution: execution.id,
            release: release.id,
            digest: release.digest.clone(),
            action: action.clone(),
            runtime: runtime.clone(),
            version,
            prepared,
            manifest,
            schema,
            parameters,
            secret_paths,
            environment,
            trace_tag: execution.trace_tag,
            rule: enforcement.as_ref().map(|item| item.rule_ref.clone()),
            trigger: enforcement.map(|item| item.trigger_ref),
            timeout_seconds,
            max_stdout_bytes: super::manager_logs::max_plaintext(&state, LogChannel::Stdout),
            max_stderr_bytes: super::manager_logs::max_plaintext(&state, LogChannel::Stderr),
            child_token,
        }),
    ))
}

pub(super) fn select_version(
    runtime: &Runtime,
    versions: &[RuntimeVersion],
    constraint: Option<&str>,
    worker: &Worker,
) -> Result<Option<RuntimeVersion>, ApiError> {
    let advertised = if runtime.execution_config.get("lifecycle").is_some() {
        attune_common::runtime_lifecycle::available_versions(worker.capabilities.as_ref(), runtime)
    } else {
        worker
            .capabilities
            .as_ref()
            .and_then(|caps| caps.get("runtime_versions"))
            .and_then(|versions| versions.get(normalize_runtime_name(&runtime.name)))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|version| version.as_str().map(str::to_owned))
            .collect()
    };
    let versions: Vec<_> = versions
        .iter()
        .filter(|version| {
            attune_common::runtime_lifecycle::validate_version_mode(
                &runtime.execution_config,
                &version.execution_config,
            )
            .is_ok()
                && advertised
                    .iter()
                    .any(|entry| runtime_version_matches_worker(version, entry))
        })
        .cloned()
        .map(|mut version| {
            version.available = true;
            version
        })
        .collect();
    let version = select_best_version(&versions, constraint).cloned();
    if constraint.is_some() && version.is_none() {
        return Err(ApiError::UnprocessableEntity(
            "No verified runtime version matches the workload".into(),
        ));
    }
    Ok(version)
}

fn unavailable() -> ApiError {
    ApiError::Forbidden("Action launch scope is unavailable".into())
}

pub(crate) async fn record_program(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path(assignment): Path<Id>,
    Json(program): Json<attune_common::manager_control::ActionProgram>,
) -> Result<axum::http::StatusCode, ApiError> {
    let mut tx = state.db.begin().await?;
    SchemaTypeRepository::lock_selection(&mut tx).await?;
    let scope = auth.work_scope(&state, &mut tx).await?;
    let (execution, deadline) = ManagerAssignmentRepository::execution_for_launch(
        &mut tx,
        scope,
        assignment,
        program.request,
    )
    .await?;
    if !execution
        .executable_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.executable.action.launch.is_some())
    {
        return Err(unavailable());
    }
    attune_common::repositories::execution_program::ExecutionProgramRepository::record_with(
        &mut tx,
        execution.id,
        execution.worker.ok_or_else(unavailable)?,
        execution.pack_release.ok_or_else(unavailable)?,
        serde_json::to_value(program.selected).map_err(|_| unavailable())?,
    )
    .await?;
    ManagerAssignmentRepository::check_deadline(&mut tx, deadline).await?;
    tx.commit().await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
