//! Trusted controller entrypoint. No general service Config, DB/MQ client or
//! signing/encryption key is loaded on this Engine host.
use attune_common::{workload_launch::PinnedImage, workload_launch_io::LaunchHttpPolicy};
use attune_launch_controller::{
    actuator::Actuator,
    cache::{CacheLimits, CacheStore},
    control_plane::{ControlError, ControlPlane},
    engine::{DockerEngine, EnginePolicy},
    service::{LaunchService, ServiceLimits},
    session_store::SessionStore,
};
use chrono::Utc;
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::Read,
    net::SocketAddr,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControllerConfig {
    api_origin: String,
    /// Loopback-only backend. A colocated TLS proxy owns the external origin
    /// registered with CP; raw remote HTTP is never an implicit frontend.
    listen: SocketAddr,
    docker_socket: PathBuf,
    engine_id: i64,
    cache_volume: String,
    workload_network: String,
    allowed_images: Vec<PinnedImage>,
    probe_image: PinnedImage,
    memory_bytes: u64,
    pids_limit: i64,
    scratch_bytes: u64,
    compressed_bytes: u64,
    expanded_bytes: u64,
    archive_files: u64,
    #[serde(default = "artifact_max_total_bytes")]
    artifact_max_total_bytes: u64,
    #[serde(default = "artifact_max_entries")]
    artifact_max_entries: u64,
    minimum_free_bytes: u64,
    max_active: usize,
    max_retained: usize,
    capture_bytes_per_stream: usize,
    session_file: PathBuf,
    #[serde(default)]
    development_plain_http: bool,
}
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
fn artifact_max_total_bytes() -> u64 {
    attune_common::workload_artifact::ArtifactLimits::default().max_total_bytes
}
fn artifact_max_entries() -> u64 {
    attune_common::workload_artifact::ArtifactLimits::default().max_entries
}

fn read_owned<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.len() > 1024 * 1024
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
    {
        return Err("controller file ownership/mode/size invalid".into());
    }
    let mut body = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut body)?;
    if body.len() > 1024 * 1024 {
        return Err("controller file exceeds bound".into());
    }
    serde_json::from_slice(&body)
        .map_err(|_| "invalid controller configuration/session JSON".into())
}

#[tokio::main]
async fn main() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 || unsafe { libc::getegid() } != 0 {
        return Err("trusted controller requires root ownership with only its documented custodian capabilities".into());
    }
    attune_common::parent_process_protection::protect_service_parent()
        .map_err(|_| "could not protect controller parent")?;
    let path =
        std::env::var_os("ATTUNE_LAUNCH_CONFIG").ok_or("ATTUNE_LAUNCH_CONFIG is required")?;
    let config: ControllerConfig = read_owned(Path::new(&path))?;
    if !config.listen.ip().is_loopback() || !config.docker_socket.is_absolute() {
        return Err(
            "controller backend must bind literal loopback and use a fixed Unix socket".into(),
        );
    }
    for image in config
        .allowed_images
        .iter()
        .chain(std::iter::once(&config.probe_image))
    {
        if !image
            .reference
            .ends_with(&format!("sha256:{}", image.sha256.as_str()))
        {
            return Err("controller images must be immutable matching digests".into());
        }
    }
    // Exclusive ownership precedes loading either retained or initial state.
    let state = SessionStore::open("/var/lib/attune/controller".into())?;
    let session = match state.retained()? {
        Some(session) => session,
        None => {
            let session = SessionStore::initial(&config.session_file)?;
            state.persist(&session)?;
            session
        }
    };
    if session.expires_at <= Utc::now()
        || session.expires_at > Utc::now() + chrono::Duration::seconds(300)
    {
        return Err(
            "initial CP-issued controller session is expired or exceeds lifetime ceiling".into(),
        );
    }
    let policy = if config.development_plain_http {
        LaunchHttpPolicy::DevelopmentPlainHttp
    } else {
        LaunchHttpPolicy::TlsOrLoopback
    };
    let cp = ControlPlane::with_policy(config.api_origin.parse()?, session.token, policy)?;
    let engine = DockerEngine::connect(&config.docker_socket)?;
    let engine_policy = EnginePolicy {
        engine_id: config.engine_id,
        cache_volume: config.cache_volume,
        network: config.workload_network,
        allowed_images: config
            .allowed_images
            .into_iter()
            .map(|image| image.reference)
            .collect::<BTreeSet<_>>(),
        memory_bytes: config.memory_bytes,
        pids_limit: config.pids_limit,
        scratch_bytes: config.scratch_bytes,
    };
    let cache = Arc::new(
        CacheStore::open(
            "/var/lib/attune/launch".into(),
            CacheLimits {
                compressed_bytes: config.compressed_bytes,
                expanded_bytes: config.expanded_bytes,
                files: config.archive_files,
            },
        )?
        .with_artifact_limits(config.artifact_max_total_bytes, config.artifact_max_entries)?,
    );
    let actuator = Arc::new(
        Actuator::initialize(
            cp.clone(),
            engine,
            engine_policy,
            cache,
            config.probe_image,
            config.compressed_bytes,
        )
        .await?,
    );
    let service = LaunchService::new(
        actuator,
        ServiceLimits {
            max_active: config.max_active,
            max_retained: config.max_retained,
            capture_bytes_per_stream: config.capture_bytes_per_stream,
            minimum_free_bytes: config.minimum_free_bytes,
        },
    )?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let (closing, close) = tokio::sync::watch::channel(false);
    let mut reconcile_close = close.clone();
    let recovery = service.clone();
    let reconcile = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = reconcile_close.changed() => break,
                result = recovery.reconcile_round() => {
                    if result.is_err() { eprintln!("controller discovery unavailable; no cleanup settlement inferred"); }
                }
            }
            tokio::select! { _ = reconcile_close.changed() => break, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
        }
    });
    let mut renew_close = close.clone();
    let renew = tokio::spawn(async move {
        let mut expiry = session.expires_at;
        loop {
            let remaining = (expiry - Utc::now())
                .to_std()
                .map_err(|_| "controller CP session expired")?;
            if remaining.is_zero() {
                return Err("controller CP session expired");
            }
            if remaining > Duration::from_secs(60) {
                tokio::select! { _ = renew_close.changed() => return Ok(()), _ = tokio::time::sleep(remaining - Duration::from_secs(60)) => {} }
            }
            let renewed = tokio::select! { _ = renew_close.changed() => return Ok(()), result = cp.renew_session(expiry, |session| state.persist(session)) => result };
            match renewed {
                Ok(session) => expiry = session.expires_at,
                Err(ControlError::Rejected(401 | 403)) => {
                    return Err("controller session authority withdrawn; stopping admission")
                }
                Err(ControlError::Io(_)) => {
                    return Err("controller session persistence uncertain; stopping admission")
                }
                Err(_) => {
                    eprintln!("controller CP session renewal unavailable; old deadline remains authoritative");
                    let remaining = (expiry - Utc::now())
                        .to_std()
                        .map_err(|_| "controller CP session expired")?;
                    tokio::select! { _ = renew_close.changed() => return Ok(()), _ = tokio::time::sleep(remaining.min(Duration::from_secs(5))) => {} }
                }
            }
        }
    });
    let mut server_close = close;
    let server =
        axum::serve(listener, service.clone().router()).with_graceful_shutdown(async move {
            let _ = server_close.changed().await;
        });
    let mut server = Box::pin(std::future::IntoFuture::into_future(server));
    let mut renew = renew;
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut server_done = false;
    let mut renew_done = false;
    let outcome: Result<()> = tokio::select! {
        result = &mut server => { server_done = true; result.map_err(Into::into) },
        result = &mut renew => { renew_done = true; match result { Ok(Ok(())) => Ok(()), _ => Err("controller session renewal ended; stopping admission".into()) } },
        result = tokio::signal::ctrl_c() => result.map_err(Into::into),
        _ = term.recv() => Ok(()),
    };
    closing
        .send(true)
        .map_err(|_| "controller shutdown signal unavailable")?;
    service.shutdown().await;
    reconcile.await?;
    if !renew_done {
        renew
            .await?
            .map_err(|_| "controller session expired during shutdown")?;
    }
    if !server_done {
        server.await?;
    }
    outcome
}
