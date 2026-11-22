//! Run managed sensors using only their admitted generation and the control API.
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use attune_common::{
    artifact_transport::ValidatedRelativePath,
    child_process_environment::ChildProcessEnvironment,
    manager_control::{
        LogChannel, ManagerControlClient, ManagerEnrollment, ManagerSession, PackTransferScope,
        SensorLaunch, SensorProcessReport,
    },
    models::{Id, WorkerRole},
    pack_transport::PackFileTransport,
    runtime_cache::{sha256_bytes, RuntimeCacheKey},
    runtime_detection::{normalize_runtime_name, probe_manager_catalog},
    runtime_lifecycle::RuntimeSource,
    Error,
};
use chrono::Utc;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::watch,
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use crate::sensor_manager::{
    apply_notifier_env, apply_runtime_env_vars, build_recipe_sensor_command, build_sensor_command,
    configure_sensor_process, deliver_sensor_bootstrap, parse_sensor_token_expiry,
    sandbox_sensor_command, sensor_restart_backoff_delay, sensor_token_rotation_delay,
    terminate_sensor_child, SensorManager, StartupProcessGuard,
};

type Session = watch::Receiver<Arc<ManagerSession>>;

pub struct SensorManagerConfig {
    pub packs: PathBuf,
    pub runtimes: PathBuf,
    pub artifacts: PathBuf,
    pub environment: ChildProcessEnvironment,
    pub notifier: Option<String>,
}

pub async fn run_from_environment(name: Option<String>, log_level: Option<&str>) -> Result<()> {
    attune_common::observability::init_tracing(log_level, None)?;
    let env = |key: &'static str| std::env::var(key).with_context(|| format!("Missing {key}"));
    let api = ManagerControlClient::new(&env("ATTUNE_API_URL")?, false)?;
    let environment = ChildProcessEnvironment::capture_for_service(&[], true).await?;
    if !environment.uses_workload_sandbox() {
        return Err(anyhow!("API-only sensor managers require Bubblewrap"));
    }
    let local = SensorManagerConfig {
        packs: PathBuf::from(env("ATTUNE_PACKS_BASE_DIR")?),
        runtimes: PathBuf::from(env("ATTUNE_RUNTIME_ENVS_DIR")?),
        artifacts: PathBuf::from(env("ATTUNE_ARTIFACTS_DIR")?),
        environment,
        notifier: std::env::var("ATTUNE_NOTIFIER_WS_URL").ok(),
    };
    let enrollment = ManagerEnrollment {
        role: WorkerRole::Sensor,
        pool: env("ATTUNE_MANAGER_POOL")?,
        name: name.map(Ok).unwrap_or_else(|| env("ATTUNE_MANAGER_NAME"))?,
        instance: Uuid::new_v4(),
    };
    let credential = tokio::fs::read_to_string(env("ATTUNE_MANAGER_ENROLLMENT_FILE")?)
        .await
        .context("Could not read manager enrollment file")?;
    let session = api.enroll(credential.trim(), &enrollment).await?;
    drop(credential);
    let stop = CancellationToken::new();
    let signals = stop.clone();
    let signal_task = tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = signal(SignalKind::terminate())?;
            tokio::select! {
                _ = term.recv() => {},
                result = tokio::signal::ctrl_c() => result?,
            }
        }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c().await?;
        signals.cancel();
        Ok::<_, std::io::Error>(())
    });
    let result = run(api, session, local, stop).await;
    signal_task.abort();
    let _ = signal_task.await;
    result
}

pub async fn run(
    api: ManagerControlClient,
    session: ManagerSession,
    local: SensorManagerConfig,
    stop: CancellationToken,
) -> Result<()> {
    if !local.environment.uses_workload_sandbox() {
        return Err(anyhow!("API-only sensor managers require Bubblewrap"));
    }
    for path in [&local.packs, &local.runtimes, &local.artifacts] {
        tokio::fs::create_dir_all(path).await?;
    }
    let local = Arc::new(local);
    let (sessions, session) = watch::channel(Arc::new(session));
    let renewal = tokio::spawn(renew_session(api.clone(), sessions, stop.clone()));
    let result = reconcile(&api, &session, local, &stop).await;
    stop.cancel();
    renewal.await?;
    let current = session.borrow().clone();
    if let Err(error) = api.withdraw(&current).await {
        warn!(%error, "Sensor manager withdrawal failed");
    }
    result
}

async fn renew_session(
    api: ManagerControlClient,
    sessions: watch::Sender<Arc<ManagerSession>>,
    stop: CancellationToken,
) {
    let mut delay = Duration::from_secs(20);
    loop {
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(delay) => {},
        }
        let current = sessions.borrow().clone();
        let result = tokio::select! {
            _ = stop.cancelled() => return,
            result = api.renew(&current) => result,
        };
        match result {
            Ok(next) => {
                sessions.send_replace(Arc::new(next));
                delay = Duration::from_secs(20);
            }
            Err(Error::RetryableTransport(_)) if current.expires_at > Utc::now() => {
                delay = Duration::from_secs(2);
            }
            Err(error) => {
                warn!(%error, "Sensor manager session lost");
                stop.cancel();
                return;
            }
        }
    }
}

async fn reconcile(
    api: &ManagerControlClient,
    session: &Session,
    local: Arc<SensorManagerConfig>,
    stop: &CancellationToken,
) -> Result<()> {
    let mut tasks = JoinSet::new();
    let mut running = HashSet::new();
    let mut failures = HashMap::<Id, (i32, tokio::time::Instant)>::new();
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let result = reconcile_loop(
        api,
        session,
        &local,
        stop,
        &mut tasks,
        &mut running,
        &mut failures,
        &mut tick,
    )
    .await;
    stop.cancel();
    while let Some(result) = tasks.join_next().await {
        result?;
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn reconcile_loop(
    api: &ManagerControlClient,
    session: &Session,
    local: &Arc<SensorManagerConfig>,
    stop: &CancellationToken,
    tasks: &mut JoinSet<(Id, bool)>,
    running: &mut HashSet<Id>,
    failures: &mut HashMap<Id, (i32, tokio::time::Instant)>,
    tick: &mut tokio::time::Interval,
) -> Result<()> {
    let mut refresh_at = tokio::time::Instant::now();
    let mut sequence = 1_i64;
    let mut registered = false;
    loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => return Ok(()),
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                let (sensor, failed) = result?;
                running.remove(&sensor);
                if failed {
                    let count = failures.get(&sensor).map_or(1, |(count, _)| count.saturating_add(1));
                    failures.insert(sensor, (count, tokio::time::Instant::now() + sensor_restart_backoff_delay(count)));
                } else {
                    failures.remove(&sensor);
                }
            }
            _ = tick.tick() => {
                let current = session.borrow().clone();
                if tokio::time::Instant::now() >= refresh_at {
                    let refresh = async {
                        let catalog = api.runtime_catalog(&current, None).await?
                            .ok_or_else(|| Error::invalid_state("Missing runtime catalog"))?;
                        let report = probe_manager_catalog(&catalog, sequence, &local.environment, &local.runtimes, None, false).await?;
                        let current = session.borrow().clone();
                        api.report_runtime(&current, &report).await
                    };
                    let result = tokio::select! {
                        _ = stop.cancelled() => return Ok(()),
                        result = refresh => result,
                    };
                    match result {
                        Ok(_) => {
                            registered = true;
                            refresh_at = tokio::time::Instant::now() + Duration::from_secs(30);
                        }
                        Err(Error::PermissionDenied(error)) => return Err(Error::PermissionDenied(error).into()),
                        Err(error) => {
                            warn!(%error, "Could not refresh sensor runtime evidence");
                            refresh_at = tokio::time::Instant::now() + Duration::from_secs(2);
                        }
                    }
                    // A report may have committed before its response was lost.
                    sequence += 1;
                }
                let current = session.borrow().clone();
                if !registered {
                    continue;
                }
                let available = match api.available_sensors(&current).await {
                    Ok(available) => available,
                    Err(Error::RetryableTransport(_)) => continue,
                    Err(error) => return Err(error.into()),
                };
                let now = tokio::time::Instant::now();
                for sensor in available {
                    if !failures.get(&sensor).is_some_and(|(_, until)| *until > now) && running.insert(sensor) {
                        let api = api.clone();
                        let session = session.clone();
                        let local = local.clone();
                        let stop = stop.child_token();
                        tasks.spawn(async move {
                            let failed = match run_generation(&api, &session, &local, sensor, stop).await {
                                Ok(()) => false,
                                Err(error) => {
                                    warn!(sensor, %error, "Managed sensor generation failed");
                                    true
                                }
                            };
                            (sensor, failed)
                        });
                    }
                }
            }
        }
    }
}

async fn run_generation(
    api: &ManagerControlClient,
    session: &Session,
    local: &SensorManagerConfig,
    sensor: Id,
    stop: CancellationToken,
) -> Result<()> {
    let request = Uuid::new_v4();
    let current = session.borrow().clone();
    let launch = match api.start_sensor(&current, sensor, request).await {
        Err(Error::RetryableTransport(_)) => {
            let current = session.borrow().clone();
            api.start_sensor(&current, sensor, request).await
        }
        result => result,
    }?;
    let fence = launch.fence;
    let child_stop = stop.child_token();
    let process = launch_sensor(api, session, local, launch, &child_stop);
    let lease = async {
        let mut end = Utc::now() + chrono::Duration::seconds(25);
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let current = session.borrow().clone();
            let renewal = tokio::time::timeout_at(
                tokio::time::Instant::now() + (end - Utc::now()).to_std().unwrap_or_default(),
                api.renew_sensor(&current, fence),
            )
            .await;
            match renewal {
                Ok(Ok(lease)) if lease.expires.is_some() => end = lease.expires.unwrap(),
                Ok(Err(Error::RetryableTransport(_))) if Utc::now() < end => continue,
                _ => return,
            }
        }
    };
    tokio::pin!(process, lease);
    let result = tokio::select! {
        result = &mut process => result,
        _ = &mut lease => {
            child_stop.cancel();
            process.await
        }
    };
    let report = if result.is_ok() {
        SensorProcessReport::Stopped
    } else {
        SensorProcessReport::Failed { exit_code: None }
    };
    if let Err(error) = publish_process(api, session, fence, report).await {
        warn!(sensor, %error, "Managed sensor process settlement was not acknowledged");
        let current = session.borrow().clone();
        if let Err(error) = api.release_sensor(&current, fence).await {
            warn!(sensor, %error, "Managed sensor generation release was not acknowledged");
        }
    }
    result
}

async fn publish_process(
    api: &ManagerControlClient,
    session: &Session,
    fence: attune_common::models::SensorWorkloadFence,
    report: SensorProcessReport,
) -> attune_common::Result<()> {
    let current = session.borrow().clone();
    match api.report_sensor_process(&current, fence, report).await {
        Err(Error::RetryableTransport(_)) => {
            let current = session.borrow().clone();
            api.report_sensor_process(&current, fence, report).await
        }
        result => result,
    }
}

async fn launch_sensor(
    api: &ManagerControlClient,
    session: &Session,
    local: &SensorManagerConfig,
    launch: SensorLaunch,
    stop: &CancellationToken,
) -> Result<()> {
    let executable = launch.executable;
    let sensor = executable.sensor;
    let release = executable.release;
    let pack_ref = sensor
        .pack_ref
        .as_deref()
        .ok_or_else(|| anyhow!("Missing sensor pack"))?;
    let packs = local
        .packs
        .to_str()
        .ok_or_else(|| anyhow!("Invalid manager pack directory"))?;
    let transfer = api.pack_transport(
        session.clone(),
        PackTransferScope::Sensor {
            workload: launch.fence.workload_id,
            generation: launch.fence.generation,
        },
        packs,
    );
    let pack_result = async {
        let pack = transfer
            .sync_release(pack_ref, release.id, &release.digest)
            .await?;
        if let Some(prepared) = &launch.prepared {
            transfer.sync_prepared_runtime(release.id, prepared).await?;
        }
        Ok::<_, Error>(pack)
    };
    let pack = tokio::select! {
        _ = stop.cancelled() => return Ok(()),
        result = pack_result => result?,
    };
    let script = pack
        .join("sensors")
        .join(ValidatedRelativePath::new(&sensor.entrypoint)?.as_path());
    let runtime = executable.runtime;
    let config = launch
        .version
        .as_ref()
        .map(|version| version.parsed_execution_config())
        .unwrap_or_else(|| runtime.parsed_execution_config());
    let environment = local.environment.clone().with_object_preparation(
        launch
            .prepared
            .as_ref()
            .is_some_and(|ready| ready.scope.is_object_backed()),
    );
    let target = if let Some(recipe) = &config.lifecycle {
        Some(match &launch.prepared {
            Some(ready) => {
                recipe
                    .open_installed(ready, &local.runtimes, &environment)
                    .await?
            }
            None => {
                recipe
                    .prepare(
                        &pack,
                        RuntimeSource::File(&script),
                        &local.runtimes.join("lifecycle-generations"),
                        &environment,
                    )
                    .await?
            }
        })
    } else {
        None
    };
    let result = async {
        if let (Some(target), Some(version)) = (&target, &launch.version) {
            target.validate_selected_version(&version.version)?;
        }
        let runtime_name = normalize_runtime_name(&runtime.name);
        let env_dir = if let Some(target) = &target {
            target.generation.clone()
        } else {
            let dependency_digest = match config.dependencies.as_ref() {
                Some(dep) => sha256_bytes(&tokio::fs::read(pack.join(&dep.manifest_file)).await.unwrap_or_default()),
                None => sha256_bytes(&[]),
            };
            let cache = RuntimeCacheKey::new(&release.digest, &dependency_digest, &runtime_name,
                launch.version.as_ref().map_or("default", |version| &version.version), env!("CARGO_PKG_VERSION"))?;
            let dir = cache.path(&local.runtimes);
            tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                result = SensorManager::ensure_runtime_environment(&environment, &config, &pack, &dir, &cache) => result?,
            };
            if cache.is_ready(&dir) {
                cache.write_pack_ref_marker(&dir, pack_ref)?;
            }
            dir
        };
        let env_dir = env_dir.is_dir().then_some(env_dir.as_path());
        let mut command = match &target {
            Some(target) => build_recipe_sensor_command(target, &environment),
            None => build_sensor_command(&script.to_string_lossy(), runtime_name == "native", &config,
                &pack, env_dir, &environment).1,
        };
        if stop.is_cancelled() {
            return Ok(());
        }
        let trigger_types: Vec<_> = executable.triggers.iter().map(|trigger| &trigger.r#ref).collect();
        command
            .current_dir(&pack)
            .env("ATTUNE_API_URL", api.api_url())
            .env("ATTUNE_API_TOKEN", &launch.token)
            .env("ATTUNE_PACK_REF", pack_ref)
            .env("ATTUNE_SENSOR_ID", sensor.id.to_string())
            .env("ATTUNE_SENSOR_REF", &sensor.r#ref)
            .env("ATTUNE_SENSOR_WORKLOAD_ID", launch.fence.workload_id.to_string())
            .env("ATTUNE_SENSOR_ASSIGNMENT_GENERATION", launch.fence.generation.to_string())
            .env("ATTUNE_SENSOR_TRIGGER_TYPES", serde_json::to_string(&trigger_types)?)
            .env("ATTUNE_ARTIFACTS_DIR", &local.artifacts);
        if let Some(notifier) = &local.notifier {
            apply_notifier_env(&mut command, notifier, false);
        }
        apply_runtime_env_vars(&mut command, &config, &pack, env_dir);
        let mut command = match &target {
            Some(target) => target.sandbox_invocation(command.into_std(), &environment)?.into(),
            None => sandbox_sensor_command(command, &environment, &pack, env_dir)?,
        };
        configure_sensor_process(&mut command)?;
        let bootstrap = serde_json::to_vec(&serde_json::json!({
            "config": launch.config, "triggers": launch.triggers,
        }))?;
        if bootstrap.len() > 32 * 1024 * 1024 {
            return Err(anyhow!("Sensor bootstrap exceeds its 32 MiB delivery limit"));
        }
        let mut child = command.kill_on_drop(true).stdin(Stdio::piped())
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let _group = StartupProcessGuard::new(child.id());
        let mut output = JoinSet::new();
        let writer = |channel| crate::sensor_log::RotatingLogWriter::new_managed(
            api.clone(), session.clone(), &launch.token, launch.fence, channel,
        );
        output.spawn(forward_log(
            child.stdout.take().ok_or_else(|| anyhow!("Missing sensor stdout"))?,
            writer(LogChannel::Stdout),
        ));
        output.spawn(forward_log(
            child.stderr.take().ok_or_else(|| anyhow!("Missing sensor stderr"))?,
            writer(LogChannel::Stderr),
        ));
        let result = tokio::select! {
            _ = stop.cancelled() => Ok(()),
            result = deliver_sensor_bootstrap(&mut child, &bootstrap, Duration::from_secs(10)) => result,
        };
        let result = if result.is_ok() && !stop.is_cancelled() {
            let pid = child.id().and_then(|pid| i32::try_from(pid).ok());
            if let Some(pid) = pid {
                if let Err(error) = publish_process(api, session, launch.fence, SensorProcessReport::Running { pid }).await {
                    warn!(%error, sensor = sensor.id, "Managed sensor startup was not acknowledged");
                }
            }
            let rotate = sensor_token_rotation_delay(Utc::now(), parse_sensor_token_expiry(&launch.token_expiry)?);
            tokio::select! {
                _ = stop.cancelled() => Ok(()),
                _ = tokio::time::sleep(rotate) => Ok(()),
                exit = child.wait() => Err(anyhow!("Sensor process exited ({})", exit?)),
            }
        } else {
            result
        };
        if child.id().is_some() {
            terminate_sensor_child(&mut child, &sensor.r#ref).await;
        }
        let joined = async {
            while let Some(result) = output.join_next().await {
                if let Err(error) = result? {
                    warn!(%error, sensor = sensor.id, "Managed sensor log could not be closed");
                }
            }
            Ok::<(), tokio::task::JoinError>(())
        };
        if tokio::time::timeout(Duration::from_secs(10), joined).await.is_err() {
            output.shutdown().await;
        }
        result
    }.await;
    if let Some(target) = target {
        target.cleanup().await?;
    }
    result
}

async fn forward_log(
    mut output: impl AsyncRead + Unpin + Send + 'static,
    mut writer: crate::sensor_log::RotatingLogWriter,
) -> Result<()> {
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut warned = false;
    loop {
        let n = output.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        if let Err(error) = writer.write_bytes(&buffer[..n]).await {
            if !warned {
                warn!(%error, "Managed sensor log append failed");
                warned = true;
            }
        }
    }
    writer.close().await
}
