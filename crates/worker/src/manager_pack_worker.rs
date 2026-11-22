//! API-only worker mode for pack tests and install preparation.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use attune_common::{
    child_process_environment::ChildProcessEnvironment,
    manager_control::{
        ClaimedPackJob, FinishPackJob, ManagerControlClient, ManagerEnrollment, ManagerSession,
        PackJobSource, PackTransferScope,
    },
    models::WorkerRole,
    pack_transport::PackFileTransport,
    runtime_detection::probe_manager_catalog,
    runtime_lifecycle::PreparationScope,
    test_executor::{CandidateTestEnvironment, TestConfig, TestExecutor},
    Error,
};
use tokio::sync::watch;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracing::{info, warn};
use uuid::Uuid;

pub struct PackJobWorkerConfig {
    pub packs: PathBuf,
    pub runtimes: PathBuf,
    pub environment: ChildProcessEnvironment,
    pub filter: Option<Vec<String>>,
}

pub struct ActionManagerConfig {
    pub artifacts: PathBuf,
    pub spool: PathBuf,
    pub spool_key_file: PathBuf,
    pub concurrency: usize,
}

type SessionReader = watch::Receiver<Arc<ManagerSession>>;

pub async fn run_from_environment(name: Option<String>, actions: bool) -> Result<()> {
    attune_common::observability::init_tracing(None, None)?;
    let env = |key: &'static str| std::env::var(key).with_context(|| format!("Missing {key}"));
    let api = ManagerControlClient::new(&env("ATTUNE_API_URL")?, false)?;
    let credential_path = env("ATTUNE_MANAGER_ENROLLMENT_FILE")?;
    let enrollment = ManagerEnrollment {
        role: WorkerRole::Action,
        name: name.map(Ok).unwrap_or_else(|| env("ATTUNE_MANAGER_NAME"))?,
        pool: env("ATTUNE_MANAGER_POOL")?,
        instance: Uuid::new_v4(),
    };
    let environment = ChildProcessEnvironment::capture_for_service(&[], true).await?;
    if !environment.uses_workload_sandbox() {
        return Err(anyhow!("API pack jobs require Bubblewrap"));
    }
    let local = PackJobWorkerConfig {
        packs: PathBuf::from(env("ATTUNE_PACKS_BASE_DIR")?),
        runtimes: PathBuf::from(env("ATTUNE_RUNTIME_ENVS_DIR")?),
        environment,
        filter: crate::env_setup::runtime_filter_from_env(),
    };
    let actions = if actions {
        Some(ActionManagerConfig {
            artifacts: PathBuf::from(env("ATTUNE_ARTIFACTS_DIR")?),
            spool: PathBuf::from(env("ATTUNE_MANAGER_LOG_SPOOL_DIR")?),
            spool_key_file: PathBuf::from(env("ATTUNE_MANAGER_LOG_SPOOL_KEY_FILE")?),
            concurrency: std::env::var("ATTUNE_MANAGER_ACTION_CONCURRENCY")
                .ok()
                .map(|value| value.parse::<usize>())
                .transpose()
                .context("Invalid ATTUNE_MANAGER_ACTION_CONCURRENCY")?
                .unwrap_or(10),
        })
    } else {
        None
    };
    let secret = tokio::fs::read_to_string(credential_path)
        .await
        .context("Could not read manager enrollment file")?;
    let session = api.enroll(secret.trim(), &enrollment).await?;
    drop(secret);

    let stop = CancellationToken::new();
    let signals = stop.clone();
    let signal_task = tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut termination = signal(SignalKind::terminate())?;
            tokio::select! {
                _ = termination.recv() => {},
                result = tokio::signal::ctrl_c() => result?,
            }
        }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c().await?;
        signals.cancel();
        Ok::<_, std::io::Error>(())
    });
    let result = run_manager(api, session, local, actions, stop.clone()).await;
    signal_task.abort();
    let _ = signal_task.await;
    result
}

pub async fn run(
    api: ManagerControlClient,
    session: ManagerSession,
    local: PackJobWorkerConfig,
    stop: CancellationToken,
) -> Result<()> {
    run_manager(api, session, local, None, stop).await
}

pub async fn run_manager(
    api: ManagerControlClient,
    session: ManagerSession,
    local: PackJobWorkerConfig,
    actions: Option<ActionManagerConfig>,
    stop: CancellationToken,
) -> Result<()> {
    if let Some(actions) = &actions {
        if !(1..=32).contains(&actions.concurrency) {
            return Err(anyhow!(
                "Manager action concurrency must be between 1 and 32"
            ));
        }
        tokio::fs::create_dir_all(&actions.artifacts).await?;
    }
    let pack_jobs_only = actions.is_none();
    if !local.environment.uses_workload_sandbox() {
        return Err(anyhow!("API pack jobs require Bubblewrap"));
    }
    tokio::fs::create_dir_all(&local.packs).await?;
    tokio::fs::create_dir_all(&local.runtimes).await?;
    let (sessions, session) = watch::channel(Arc::new(session));
    let local = Arc::new(local);
    let tasks = TaskTracker::new();
    let renew_stop = CancellationToken::new();
    let failed_session = CancellationToken::new();
    tasks.spawn(renew_session(
        api.clone(),
        sessions,
        renew_stop.clone(),
        failed_session.clone(),
    ));

    let initial = tokio::select! {
        result = refresh_runtime(&api, &session, &local, 1, None, pack_jobs_only) => result,
        _ = stop.cancelled() => Ok(None),
        _ = failed_session.cancelled() => Err(Error::PermissionDenied("Manager session lost".into())),
    };
    let result = match initial {
        Ok(Some((revision, worker))) => {
            info!(pack_jobs_only, "API worker is ready");
            let refresh_stop = stop.child_token();
            tasks.spawn(refresh_loop(
                api.clone(),
                session.clone(),
                local.clone(),
                refresh_stop,
                revision,
                pack_jobs_only,
            ));
            let pack = run_jobs(&api, &session, &local, stop.clone(), failed_session.clone());
            if let Some(actions) = actions {
                let actions = run_actions(
                    &api,
                    &session,
                    &local,
                    actions,
                    worker,
                    &stop,
                    &failed_session,
                );
                let (pack, actions) = tokio::join!(pack, async {
                    let result = actions.await;
                    stop.cancel();
                    result
                });
                pack.and(actions)
            } else {
                pack.await
            }
        }
        Ok(None) => Ok(()),
        Err(error) => Err(error.into()),
    };
    renew_stop.cancel();
    tasks.close();
    tasks.wait().await;
    let current = session.borrow().clone();
    if let Err(error) = api.withdraw(&current).await {
        warn!(%error, "Manager session withdrawal failed");
    }
    result
}

async fn renew_session(
    api: ManagerControlClient,
    sessions: watch::Sender<Arc<ManagerSession>>,
    stop: CancellationToken,
    lost: CancellationToken,
) {
    let mut delay = Duration::from_secs(20);
    loop {
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(delay) => {},
        }
        let session = sessions.borrow().clone();
        let result = tokio::select! {
            _ = stop.cancelled() => return,
            result = api.renew(&session) => result,
        };
        match result {
            Ok(next) => {
                sessions.send_replace(Arc::new(next));
                delay = Duration::from_secs(20);
            }
            Err(Error::RetryableTransport(_)) if session.expires_at > chrono::Utc::now() => {
                delay = Duration::from_secs(2);
            }
            Err(error) => {
                warn!(%error, "API pack worker lost its session");
                lost.cancel();
                return;
            }
        }
    }
}

async fn refresh_runtime(
    api: &ManagerControlClient,
    session: &SessionReader,
    local: &PackJobWorkerConfig,
    sequence: i64,
    since: Option<i64>,
    pack_jobs_only: bool,
) -> attune_common::Result<Option<(i64, i64)>> {
    let current = session.borrow().clone();
    let Some(catalog) = api.runtime_catalog(&current, since).await? else {
        return Ok(None);
    };
    let report = probe_manager_catalog(
        &catalog,
        sequence,
        &local.environment,
        &local.runtimes,
        local.filter.as_deref(),
        pack_jobs_only,
    )
    .await?;
    let result = retry_transport(|| {
        let current = session.borrow().clone();
        let report = &report;
        async move { api.report_runtime(&current, report).await }
    })
    .await?;
    if result.sequence != sequence || result.revision != catalog.revision {
        return Err(Error::RetryableTransport(
            "Unexpected manager runtime report receipt".into(),
        ));
    }
    Ok(Some((catalog.revision, result.worker)))
}

async fn refresh_loop(
    api: ManagerControlClient,
    session: SessionReader,
    local: Arc<PackJobWorkerConfig>,
    stop: CancellationToken,
    mut revision: i64,
    pack_jobs_only: bool,
) {
    let mut sequence = 2_i64;
    let mut last_probe = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(12)) => {},
        }
        let since = (last_probe.elapsed() < Duration::from_secs(60)).then_some(revision);
        let result = tokio::select! {
            _ = stop.cancelled() => return,
            result = refresh_runtime(&api, &session, &local, sequence, since, pack_jobs_only) => result,
        };
        sequence += 1;
        match result {
            Ok(Some((applied, _))) => {
                revision = applied;
                last_probe = tokio::time::Instant::now();
            }
            Ok(None) => {}
            Err(error) => warn!(%error, "Could not refresh pack worker runtime evidence"),
        }
    }
}

#[cfg(not(unix))]
async fn run_actions(
    _api: &ManagerControlClient,
    _session: &SessionReader,
    _local: &Arc<PackJobWorkerConfig>,
    _actions: ActionManagerConfig,
    _worker: i64,
    _stop: &CancellationToken,
    _lost: &CancellationToken,
) -> Result<()> {
    Err(anyhow!(
        "API-only action managers require Unix log custody and Bubblewrap"
    ))
}

#[cfg(unix)]
async fn run_actions(
    api: &ManagerControlClient,
    session: &SessionReader,
    local: &Arc<PackJobWorkerConfig>,
    actions: ActionManagerConfig,
    worker: i64,
    stop: &CancellationToken,
    lost: &CancellationToken,
) -> Result<()> {
    let key = crate::log_delivery_spool::load_spool_key(&actions.spool_key_file).await?;
    let logs = crate::log_delivery::LogDeliveryManager::open_remote(
        api.clone(),
        session.clone(),
        actions.spool,
        key,
        worker,
    )
    .await?;
    let mut running = tokio::task::JoinSet::new();
    let result: Result<()> = async {
        loop {
            let current = session.borrow().clone();
            let offer = tokio::select! {
                biased;
                _ = stop.cancelled() => {
                    let _ = api.drain_work(&current).await;
                    return Ok(());
                }
                _ = lost.cancelled() => return Err(anyhow!("Manager session lost")),
                Some(result) = running.join_next(), if !running.is_empty() => {
                    result?;
                    continue;
                }
                result = api.wait_work(&current), if running.len() < actions.concurrency => result,
            };
            let offer = match offer {
                Ok(Some(offer)) => offer,
                Ok(None) => continue,
                Err(Error::RetryableTransport(_)) => {
                    tokio::select! {
                        _ = stop.cancelled() => continue,
                        _ = tokio::time::sleep(Duration::from_secs(1)) => continue,
                    }
                }
                Err(error) => return Err(error.into()),
            };
            let request = Uuid::new_v4();
            let offer = match retry_transport(|| {
                let current = session.borrow().clone();
                let offer = &offer;
                async move { api.claim_work(&current, offer, request).await }
            })
            .await
            {
                Ok(offer) => offer,
                Err(error) => {
                    warn!(%error, assignment = offer.assignment, "Could not claim action");
                    continue;
                }
            };
            let api = api.clone();
            let session = session.clone();
            let local = local.clone();
            let artifacts = actions.artifacts.clone();
            let logs = logs.clone();
            let stop = stop.clone();
            let lost = lost.clone();
            running.spawn(async move {
                let paths = crate::manager_action::ManagerActionPaths {
                    packs: &local.packs,
                    runtimes: &local.runtimes,
                    artifacts: &artifacts,
                    logs: &logs,
                };
                run_action(
                    &api,
                    &session,
                    &local.environment,
                    paths,
                    offer,
                    request,
                    &stop,
                    &lost,
                )
                .await;
            });
        }
    }
    .await;
    stop.cancel();
    let mut joined = Ok(());
    while let Some(task) = running.join_next().await {
        if let Err(error) = task {
            joined = Err(error.into());
        }
    }
    logs.drain().await;
    result.and(joined)
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
async fn run_action(
    api: &ManagerControlClient,
    session: &SessionReader,
    environment: &ChildProcessEnvironment,
    paths: crate::manager_action::ManagerActionPaths<'_>,
    offer: attune_common::manager_control::WorkOffer,
    request: Uuid,
    stop: &CancellationToken,
    lost: &CancellationToken,
) {
    use attune_common::manager_control::{FinishWork, WorkLease};
    use attune_common::models::ExecutionStatus;
    let cancel = lost.child_token();
    let action = crate::manager_action::execute(
        api,
        session,
        &offer,
        request,
        paths,
        environment,
        cancel.clone(),
    );
    let observed_deadline = std::sync::atomic::AtomicI64::new(offer.deadline.timestamp_millis());
    let lease = async {
        let mut deadline = offer.deadline;
        loop {
            let span = (deadline - chrono::Utc::now()).to_std().unwrap_or_default();
            let current = session.borrow().clone();
            let result =
                tokio::time::timeout(span, api.watch_work(&current, offer.assignment)).await;
            match result {
                Ok(Ok(WorkLease::Running { deadline: next })) => {
                    deadline = next;
                    observed_deadline.store(
                        next.timestamp_millis(),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
                Ok(Ok(WorkLease::Cancel)) => return true,
                Ok(Err(Error::RetryableTransport(_))) if chrono::Utc::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                _ => return false,
            }
        }
    };
    tokio::pin!(action, lease);
    let mut server_cancel = false;
    let drained = async {
        stop.cancelled().await;
        tokio::time::sleep(Duration::from_secs(
            attune_common::config::DEFAULT_WORKER_SHUTDOWN_TIMEOUT_SECS,
        ))
        .await;
    };
    let result = tokio::select! {
        result = &mut action => result,
        cancelled = &mut lease => {
            server_cancel = cancelled;
            cancel.cancel();
            action.await
        }
        _ = lost.cancelled() => {
            cancel.cancel();
            action.await
        }
        _ = drained => {
            cancel.cancel();
            action.await
        }
    };
    if cancel.is_cancelled() && !server_cancel {
        return;
    }
    let mut completion = result.unwrap_or_else(|_| FinishWork {
        status: ExecutionStatus::Failed,
        result: Default::default(),
    });
    if server_cancel {
        completion.status = ExecutionStatus::Cancelled;
    }
    // Hold the exact completion body through disconnections. After a
    // committed finish, a lost response can still replay under the session.
    let mut deadline = observed_deadline.load(std::sync::atomic::Ordering::Relaxed);
    loop {
        let current = session.borrow().clone();
        match api
            .finish_work(&current, offer.assignment, &completion)
            .await
        {
            Ok(_) => break,
            Err(Error::RetryableTransport(_))
                if !lost.is_cancelled() && chrono::Utc::now().timestamp_millis() < deadline =>
            {
                let current = session.borrow().clone();
                if let Ok(WorkLease::Running { deadline: next }) =
                    api.renew_work(&current, offer.assignment).await
                {
                    deadline = next.timestamp_millis();
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(error) => {
                warn!(%error, assignment = offer.assignment, "Action completion was not acknowledged");
                break;
            }
        }
    }
}

async fn run_jobs(
    api: &ManagerControlClient,
    session: &SessionReader,
    local: &PackJobWorkerConfig,
    stop: CancellationToken,
    lost: CancellationToken,
) -> Result<()> {
    let result: Result<()> = async {
        loop {
            let current = session.borrow().clone();
            let offer = tokio::select! {
                biased;
                _ = stop.cancelled() => return Ok(()),
                _ = lost.cancelled() => return Err(anyhow!("Manager session lost")),
                result = api.wait_pack_job(&current) => result,
            };
            let offer = match offer {
                Ok(Some(offer)) => offer,
                Ok(None) => continue,
                Err(Error::RetryableTransport(_)) => {
                    tokio::select! {
                        _ = stop.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_secs(1)) => continue,
                    }
                }
                Err(error) => return Err(error.into()),
            };
            let claim = Uuid::new_v4();
            let job = retry_transport(|| {
                let current = session.borrow().clone();
                let offer = &offer;
                async move { api.claim_pack_job(&current, offer, claim).await }
            }).await;
            let job = match job {
                Ok(job) => job,
                Err(error) => {
                    warn!(%error, install=offer.install, "Could not claim pack job");
                    continue;
                }
            };
            let install = job.install;
            let cancelled = lost.child_token();
            let execution = execute_job(api, session, local, claim, job, cancelled.clone());
            tokio::pin!(execution);
            let lease = watch_lease(api, session, install, claim);
            tokio::pin!(lease);
            let result = tokio::select! {
                result = &mut execution => result,
                () = &mut lease => {
                    cancelled.cancel();
                    execution.await
                }
                _ = stop.cancelled() => {
                    let current = session.borrow().clone();
                    let _ = api.drain_work(&current).await;
                    tokio::select! {
                        result = &mut execution => result,
                        () = &mut lease => { cancelled.cancel(); execution.await },
                        _ = tokio::time::sleep(Duration::from_secs(attune_common::config::DEFAULT_WORKER_SHUTDOWN_TIMEOUT_SECS)) => { cancelled.cancel(); execution.await },
                    }
                }
            };
            if let Err(error) = result {
                warn!(%error, install, "Pack job ended without completion");
            }
        }
    }.await;
    // Runtime publishing must stop even when a failure ends the dispatcher.
    stop.cancel();
    result
}

async fn watch_lease(
    api: &ManagerControlClient,
    session: &SessionReader,
    install: i64,
    claim: Uuid,
) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let result = retry_transport(|| {
            let current = session.borrow().clone();
            async move { api.renew_pack_job(&current, install, claim).await }
        })
        .await;
        if !matches!(result, Ok(Some(_))) {
            return;
        }
    }
}

async fn execute_job(
    api: &ManagerControlClient,
    session: &SessionReader,
    local: &PackJobWorkerConfig,
    claim: Uuid,
    job: ClaimedPackJob,
    cancelled: CancellationToken,
) -> Result<()> {
    let install = job.install;
    let completion = run_job(api, session, local, claim, job, &cancelled).await;
    if cancelled.is_cancelled() {
        return completion.map(|_| ());
    }
    let completion = match completion {
        Ok(Some(completion)) => completion,
        Ok(None) => return Ok(()),
        Err(error) => {
            warn!(%error, install, "Pack test or runtime preparation failed");
            FinishPackJob::Failed
        }
    };
    retry_transport(|| {
        let current = session.borrow().clone();
        let completion = &completion;
        async move {
            api.finish_pack_job(&current, install, claim, completion)
                .await
        }
    })
    .await?;
    Ok(())
}

async fn run_job(
    api: &ManagerControlClient,
    session: &SessionReader,
    local: &PackJobWorkerConfig,
    claim: Uuid,
    job: ClaimedPackJob,
    cancelled: &CancellationToken,
) -> Result<Option<FinishPackJob>> {
    let attempt = tempfile::Builder::new()
        .prefix("attune-pack-job-")
        .tempdir_in(&local.runtimes)?;
    let paths = |path: &Path| {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("Invalid manager pack path"))
    };
    let packs = paths(&local.packs)?;
    let candidate_dir = paths(attempt.path())?;
    let transfer = api
        .pack_transport(
            session.clone(),
            PackTransferScope::PackJob {
                install: job.install,
                claim,
            },
            &packs,
        )
        .with_candidate_base_dir(&candidate_dir);
    let pack = match &job.source {
        PackJobSource::Candidate { digest } => {
            transfer
                .sync_claimed_candidate(&job.pack_ref, job.install, digest)
                .await?
        }
        PackJobSource::Release { id, digest } => {
            transfer.sync_release(&job.pack_ref, *id, digest).await?
        }
    };
    if cancelled.is_cancelled() {
        return Err(anyhow!("Pack job was cancelled"));
    }
    if let Some(preparation) = job.preparation {
        let current = session.borrow().clone();
        let live = api
            .runtime_catalog(&current, None)
            .await?
            .context("Runtime catalog unavailable for preparation")?;
        let environment = local
            .environment
            .clone()
            .with_object_preparation(live.object_preparation);
        let scope = PreparationScope::open(&local.runtimes, &environment)?
            .filter(|scope| preparation.scopes.contains(scope))
            .context("Preparation storage scope changed")?;
        let release_digest = match &job.source {
            PackJobSource::Candidate { digest } => digest,
            _ => return Err(anyhow!("Preparation requires a candidate archive")),
        };
        let receipt = preparation
            .recipe
            .prepare_for_install(
                &pack,
                preparation.target,
                release_digest,
                &scope,
                &environment,
                cancelled,
            )
            .await?;
        if cancelled.is_cancelled() {
            return Err(anyhow!("Pack job was cancelled"));
        }
        if scope.is_object_backed() {
            tokio::select! {
                _ = cancelled.cancelled() => return Err(anyhow!("Pack job was cancelled")),
                result = transfer.publish_prepared_runtime(job.install, "", &receipt) => result?,
            }
            return Ok(None);
        }
        return Ok(Some(FinishPackJob::Prepared { receipt }));
    }

    let metadata: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&tokio::fs::read_to_string(pack.join("pack.yaml")).await?)?;
    let tests: TestConfig = serde_yaml_ng::from_value(
        metadata
            .get("testing")
            .context("Pinned pack has no testing configuration")?
            .clone(),
    )?;
    tests.validate_policy()?;
    if !tests.enabled || tests.candidate_api_effects != job.effects.is_some() {
        return Err(anyhow!("Pinned test policy differs from the claim"));
    }
    let python = if tests
        .runners
        .values()
        .any(|runner| matches!(runner.r#type.as_str(), "unittest" | "pytest"))
    {
        let runtime = job
            .python
            .context("Claim carries no Python configuration")?;
        Some(
            crate::env_setup::prepare_python_for_pack_test(
                &runtime,
                &pack,
                &local.runtimes,
                attempt.path(),
                &local.environment,
            )
            .await?,
        )
    } else {
        None
    };
    let mut executor = TestExecutor::new(local.packs.clone())
        .with_cancellation(cancelled.clone())
        .with_runtime_lifecycle(
            local.environment.clone(),
            local.runtimes.join("lifecycle-generations"),
        );
    if let Some(effects) = job.effects {
        executor = executor.with_candidate_effects(CandidateTestEnvironment {
            url: format!("{}/api/v1/candidates/{}/effects", api.api_url(), effects.id),
            token: effects.token,
        });
    }
    let result = executor
        .execute_pack_tests_at_with_python_interpreter(
            &pack,
            &job.pack_ref,
            &job.version,
            &tests,
            python.as_deref(),
        )
        .await;
    executor.process_owner().shutdown().await;
    Ok(Some(FinishPackJob::Tests { summary: result? }))
}

async fn retry_transport<T, F, Fut>(mut op: F) -> attune_common::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = attune_common::Result<T>>,
{
    match op().await {
        Err(Error::RetryableTransport(_)) => op().await,
        result => result,
    }
}
