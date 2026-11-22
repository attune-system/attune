//! Local execution of a descriptor obtained from the API's live claim.
use std::{collections::HashMap, path::Path};

use attune_common::{
    action_cli::{self, ArgumentLimits, ArgumentPlan, InputDelivery},
    child_process_environment::ChildProcessEnvironment,
    manager_control::{
        ActionLaunch, FinishWork, LogChannel, ManagerControlClient, ManagerSession, OpenLog,
        PackTransferScope, WorkOffer,
    },
    models::{ExecutionStatus, ParameterDelivery},
    pack_format::LaunchSpec,
    pack_transport::PackFileTransport,
    runtime_detection::normalize_runtime_name,
    Error, Result,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::runtime::{
    native::NativeRuntime, process::ProcessRuntime, ExecutionContext, NativeInvocation, Runtime,
};

pub struct ManagerActionPaths<'a> {
    pub packs: &'a Path,
    pub runtimes: &'a Path,
    pub artifacts: &'a Path,
    pub logs: &'a Arc<crate::log_delivery::LogDeliveryManager>,
}

pub async fn execute(
    client: &ManagerControlClient,
    session: &watch::Receiver<Arc<ManagerSession>>,
    offer: &WorkOffer,
    request: Uuid,
    paths: ManagerActionPaths<'_>,
    environment: &ChildProcessEnvironment,
    cancel: CancellationToken,
) -> Result<FinishWork> {
    let current = session.borrow().clone();
    let launch = match client.launch_action(&current, offer, request).await {
        Err(Error::RetryableTransport(_)) if !cancel.is_cancelled() => {
            let current = session.borrow().clone();
            client.launch_action(&current, offer, request).await?
        }
        result => result?,
    };
    let packs = paths
        .packs
        .to_str()
        .ok_or_else(|| Error::configuration("Invalid manager pack path"))?;
    let transfer = client.pack_transport(
        session.clone(),
        PackTransferScope::Action {
            assignment: offer.assignment,
        },
        packs,
    );
    let pack = transfer
        .sync_release(&launch.action.pack_ref, launch.release, &launch.digest)
        .await?;
    if let Some(prepared) = &launch.prepared {
        transfer
            .sync_prepared_runtime(launch.release, prepared)
            .await?;
    }

    let environment = environment.clone().with_object_preparation(
        launch
            .prepared
            .as_ref()
            .is_some_and(|ready| ready.scope.is_object_backed()),
    );
    let runtime_config = launch
        .version
        .as_ref()
        .map(|version| version.parsed_execution_config())
        .unwrap_or_else(|| launch.runtime.parsed_execution_config());
    let runtime_name = normalize_runtime_name(&launch.runtime.name);
    let version = launch
        .version
        .as_ref()
        .map(|version| version.version.clone());
    let entry_point = launch.action.entrypoint.clone().unwrap_or_default();
    let code_path = if launch.action.launch.is_none() && !launch.action.is_adhoc {
        let source = pack.join("actions").join(&entry_point);
        source.is_file().then_some(source)
    } else {
        None
    };
    let code = crate::executor::inline_action_source(
        &entry_point,
        launch.action.is_adhoc,
        Some(&runtime_name),
        Some(&runtime_config),
        code_path.is_some(),
    );
    let native_invocation = prepare_native(&launch, &pack, &environment).await?;
    if let Some(native) = &native_invocation {
        let current = session.borrow().clone();
        client
            .record_action_program(&current, offer.assignment, request, &native.selected)
            .await?;
    }
    let mut env = HashMap::from([
        ("ATTUNE_EXEC_ID".into(), launch.execution.to_string()),
        ("ATTUNE_ACTION".into(), launch.action.r#ref.clone()),
        ("ATTUNE_PACK_REF".into(), launch.action.pack_ref.clone()),
        ("ATTUNE_PACK_RELEASE_DIGEST".into(), launch.digest.clone()),
        ("ATTUNE_API_URL".into(), client.api_url().to_owned()),
        (
            "ATTUNE_ARTIFACTS_DIR".into(),
            paths.artifacts.to_string_lossy().into_owned(),
        ),
        (
            "ATTUNE_RUNTIME_ENVS_DIR".into(),
            paths.runtimes.to_string_lossy().into_owned(),
        ),
    ]);
    for (name, value) in [
        ("ATTUNE_TRACE_TAG", launch.trace_tag),
        ("ATTUNE_RULE", launch.rule),
        ("ATTUNE_TRIGGER", launch.trigger),
        ("ATTUNE_API_TOKEN", launch.child_token),
    ] {
        if let Some(value) = value {
            env.insert(name.into(), value);
        }
    }
    if let Some(context) = launch.parameters.get("context").and_then(Value::as_object) {
        for (key, value) in context {
            let path = format!("/context/{}", key.replace('~', "~0").replace('/', "~1"));
            if launch.secret_paths.iter().any(|secret| {
                secret == "/context"
                    || secret == &path
                    || secret
                        .strip_prefix(&path)
                        .is_some_and(|rest| rest.starts_with('/'))
            }) {
                continue;
            }
            env.insert(
                format!("ATTUNE_CONTEXT_{}", key.to_uppercase()),
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string()),
            );
        }
    }
    let stdout_allocated = Arc::new(AtomicBool::new(false));
    let stderr_allocated = Arc::new(AtomicBool::new(false));
    let writer = |channel, allocated: Arc<AtomicBool>, max_bytes| {
        let client = client.clone();
        let sessions = session.clone();
        let logs = paths.logs.clone();
        let execution = offer.execution;
        let assignment = offer.assignment;
        let is_stdout = matches!(channel, LogChannel::Stdout);
        let default = attune_common::config::ArtifactsConfig::default().log_segment_writer_config();
        crate::runtime::BoundedLogFileWriter::lazy(
            move || async move {
                let current = sessions.borrow().clone();
                let body = OpenLog { request, channel };
                let log = match client.open_action_log(&current, assignment, &body).await {
                    Err(Error::RetryableTransport(_)) => {
                        let current = sessions.borrow().clone();
                        client.open_action_log(&current, assignment, &body).await?
                    }
                    result => result?,
                };
                let transport = Arc::new(client.artifact_transport(&log.capture_token, ""));
                let writer = if !log.recoverable {
                    let cfg = attune_common::log_stream::SegmentedLogConfig {
                        initial_segment_bytes: default.initial_segment_bytes.min(log.segment_bytes),
                        max_segment_bytes: log.segment_bytes,
                        ..default
                    };
                    crate::runtime::BoundedLogFileWriter::from_segmented_writer(
                        attune_common::log_stream::SegmentedLogWriter::new(
                            transport,
                            log.version,
                            cfg,
                        )?,
                        log.max_plaintext,
                        is_stdout,
                    )
                } else {
                    crate::runtime::BoundedLogFileWriter::from_spooling_writer(
                        logs.manager_writer(execution, &log, Some(transport))?,
                        log.max_plaintext,
                        is_stdout,
                        default.finalization_timeout_ms,
                    )
                };
                allocated.store(true, Ordering::Relaxed);
                Ok(writer.with_encryption(Some(&log.key), execution, log.version))
            },
            max_bytes,
            is_stdout,
            default.finalization_timeout_ms,
            None,
        )
    };
    let context = ExecutionContext {
        native_invocation,
        installed_preparation: launch.prepared,
        execution_id: launch.execution,
        action_ref: launch.action.r#ref,
        parameters: launch.parameters.into_iter().collect(),
        env,
        execution_env: launch.environment,
        secrets: HashMap::new(),
        timeout: Some(launch.timeout_seconds),
        working_dir: Some(pack),
        entry_point,
        code,
        code_path,
        runtime_name: Some(runtime_name.clone()),
        runtime_config_override: Some(runtime_config.clone()),
        runtime_env_dir_suffix: version.as_ref().map(|v| format!("{runtime_name}-{v}")),
        selected_runtime_version: version,
        max_stdout_bytes: launch.max_stdout_bytes,
        max_stderr_bytes: launch.max_stderr_bytes,
        stdout_log_path: None,
        stderr_log_path: None,
        stdout_log_writer: Some(writer(
            LogChannel::Stdout,
            stdout_allocated.clone(),
            launch.max_stdout_bytes,
        )),
        stderr_log_writer: Some(writer(
            LogChannel::Stderr,
            stderr_allocated.clone(),
            launch.max_stderr_bytes,
        )),
        parameter_delivery: launch.action.parameter_delivery,
        parameter_format: launch.action.parameter_format,
        output_format: launch.schema.output_format,
        output_policy: launch.schema.output_policy,
        out_schema: launch.schema.output,
        cancel_token: Some(cancel.clone()),
    };
    let result = if runtime_name == "native" {
        NativeRuntime::new()
            .with_child_environment(environment.clone())
            .execute(context)
            .await
    } else {
        ProcessRuntime::new(
            runtime_name,
            runtime_config,
            paths.packs.to_owned(),
            paths.runtimes.to_owned(),
        )
        .with_child_environment(environment.clone())
        .execute(context)
        .await
    }
    .map_err(|_| Error::execution("Local action runtime could not complete"))?;
    let status = if cancel.is_cancelled() {
        ExecutionStatus::Cancelled
    } else if result.timed_out {
        ExecutionStatus::Timeout
    } else if result.is_success() {
        ExecutionStatus::Completed
    } else {
        ExecutionStatus::Failed
    };
    let mut summary = json!({
        "exit_code": result.exit_code,
        "duration_ms": result.duration_ms,
        "succeeded": status == ExecutionStatus::Completed,
    });
    if !result.stdout.is_empty() {
        summary["stdout"] = json!(result.stdout);
    }
    if let Some(error) = &result.error {
        summary["error"] = json!(error);
    }
    if let Some(data) = &result.result {
        crate::executor::ActionExecutor::copy_reserved_queue_ack(&mut summary, data);
        summary["data"] = data.clone();
    }
    crate::executor::record_runtime_log_capture(
        &mut summary,
        &result,
        Some((
            stdout_allocated.load(Ordering::Relaxed),
            stderr_allocated.load(Ordering::Relaxed),
        )),
    );
    Ok(FinishWork {
        status,
        result: summary.as_object().cloned().unwrap_or_default(),
    })
}

async fn prepare_native(
    launch: &ActionLaunch,
    path: &Path,
    environment: &ChildProcessEnvironment,
) -> Result<Option<NativeInvocation>> {
    let Some(spec) = &launch.action.launch else {
        return Ok(None);
    };
    spec.validate_action_source(
        launch.action.entrypoint.as_deref(),
        launch.action.parameter_delivery,
        launch.action.parameter_format,
    )?;
    let manifest = launch.manifest.clone();
    let reference = launch.action.r#ref.clone();
    let spec = spec.clone();
    let root = path.to_owned();
    let environment = environment.clone();
    let program_spec = spec.clone();
    let selected = tokio::task::spawn_blocking(move || {
        action_cli::select_program(
            &program_spec,
            &root,
            manifest.as_ref(),
            &reference,
            &environment,
        )
    })
    .await
    .map_err(|_| Error::execution("Local program selection failed"))??;
    let empty = json!({});
    let schema = launch.action.param_schema.as_ref().unwrap_or(&empty);
    let arguments = spec.action_arguments()?;
    let cli_enabled = matches!(spec, LaunchSpec::Command { .. })
        || !arguments.is_empty()
        || action_cli::schema_for_binding(schema).map_err(|e| Error::validation(e.to_string()))?
            != *schema
        || launch.action.parameter_delivery == ParameterDelivery::None;
    let input = if cli_enabled {
        let delivery = match launch.action.parameter_delivery {
            ParameterDelivery::None => InputDelivery::None,
            ParameterDelivery::Stdin => InputDelivery::Stdin,
            _ => return Err(Error::validation("Unsupported command input delivery")),
        };
        let plan =
            ArgumentPlan::from_bound_contract(schema, arguments, launch.schema.input.clone())
                .map_err(|error| Error::validation(error.to_string()))?;
        Some(
            plan.materialize(
                selected.argument_budget_program()?,
                &Value::Object(launch.parameters.clone()),
                &launch.secret_paths,
                delivery,
                ArgumentLimits::default(),
            )
            .map_err(|error| Error::validation(error.to_string()))?,
        )
    } else {
        None
    };
    Ok(Some(NativeInvocation { selected, input }))
}
