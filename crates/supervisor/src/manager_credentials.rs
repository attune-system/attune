use std::{sync::Arc, time::Duration};

use anyhow::{ensure, Context, Result};
use attune_common::{
    config::{Config, ManagerKubernetesDatabaseConfig},
    db::ConnectionIdentity,
    kubernetes_manager_api::KubernetesManagerApi,
    repositories::manager_db_login::ManagerDatabaseLoginRepository,
};
use sqlx::{
    postgres::{PgPoolOptions, PgSslMode},
    ConnectOptions, PgPool,
};
use tokio::{sync::broadcast, task::JoinSet};
use tracing::{info, warn};

const BATCH_SIZE: i32 = 32;
const INTERVAL: Duration = Duration::from_secs(2);

pub async fn connect(config: &Config) -> Result<Option<PgPool>> {
    let Some(path) = &config.maintenance.manager_database_credentials_file else {
        return Ok(None);
    };
    let url = tokio::fs::read_to_string(path)
        .await
        .context("Read supervisor manager database credential file")?;
    let identity = ConnectionIdentity::for_process("supervisor", "manager-credentials");
    let options = identity
        .connect_options(url.trim())
        .map_err(|_| anyhow::anyhow!("Invalid supervisor manager database credential URL"))?;
    ensure!(
        matches!(options.get_ssl_mode(), PgSslMode::VerifyFull),
        "Supervisor manager database credentials require sslmode=verify-full"
    );

    // The login can only execute guarded operations. It has no catalog epoch
    // table access, so it uses its own pool outside the service's normal pool.
    let schema = config.database.schema.as_deref().unwrap_or("attune");
    let options = options
        .options([
            ("search_path", schema),
            ("statement_timeout", "5000"),
            ("lock_timeout", "1000"),
        ])
        .disable_statement_logging();
    Ok(Some(
        PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect_lazy_with(options),
    ))
}

pub async fn run(
    pool: PgPool,
    kubernetes: Option<ManagerKubernetesDatabaseConfig>,
    mut shutdown: broadcast::Receiver<()>,
) {
    let mut tick = tokio::time::interval(INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    info!("Supervisor manager database credential reconciliation started");
    loop {
        tokio::select! {
            biased;
            _ = shutdown.recv() => break,
            _ = tick.tick() => {}
        }
        tokio::select! {
            biased;
            _ = shutdown.recv() => break,
            _ = reconcile_once(&pool, kubernetes.as_ref()) => {}
        }
    }
    pool.close().await;
}

async fn reconcile_once(pool: &PgPool, kubernetes: Option<&ManagerKubernetesDatabaseConfig>) {
    if let Some(config) = kubernetes {
        if reconcile_pods(pool, config).await.is_err() {
            warn!("Manager Pod reconciliation unavailable; will retry");
        }
    }
    match ManagerDatabaseLoginRepository::install_requested(pool, BATCH_SIZE).await {
        Ok(count) if count > 0 => info!(count, "Installed manager database credentials"),
        Ok(_) => {}
        Err(_) => warn!("Manager database credential installation failed; will retry"),
    }
    // Each statement commits independently. Attempt previously committed
    // drains even when another binding blocks the disabling phase.
    match ManagerDatabaseLoginRepository::disable_retired(pool, BATCH_SIZE).await {
        Ok(count) if count > 0 => info!(count, "Disabled retired manager database logins"),
        Ok(_) => {}
        Err(_) => warn!("Manager database login retirement failed; will retry"),
    }
    match ManagerDatabaseLoginRepository::drain_retired(pool, BATCH_SIZE).await {
        Ok(count) if count > 0 => info!(count, "Drained retired manager database connections"),
        Ok(_) => {}
        Err(_) => warn!("Manager database connection drain failed; will retry"),
    }
}

async fn reconcile_pods(pool: &PgPool, config: &ManagerKubernetesDatabaseConfig) -> Result<()> {
    let api = KubernetesManagerApi::in_cluster().await?;
    let caller: Arc<str> = KubernetesManagerApi::access_token().await?.trim().into();
    let mut checks = JoinSet::new();
    for pod in ManagerDatabaseLoginRepository::claim_pod_checks(pool, 16).await? {
        let account = config
            .pools
            .iter()
            .find(|scope| scope.role == pod.manager_role && scope.pool == pod.pool)
            .map(|scope| scope.service_account.clone());
        let namespace = config.namespace.clone();
        let caller = caller.clone();
        checks.spawn(async move {
            // Lookup errors leave the credential on its existing bounded lease.
            let live = api
                .live_pod(&caller, &namespace, &pod.pod_name, pod.pod_uid)
                .await?;
            Ok::<_, attune_common::Error>((pod, live.is_some() && live == account))
        });
    }
    while let Some(result) = checks.join_next().await {
        match result? {
            Ok((pod, false)) => {
                ManagerDatabaseLoginRepository::invalidate_pod(pool, pod.session_id, pod.pod_uid)
                    .await?
            }
            Ok(_) => {}
            Err(_) => warn!("Manager Pod identity check unavailable; will retry"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn credentials_task_is_disabled_without_a_file() {
        let config: Config = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(connect(&config).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn credentials_task_requires_verified_tls() {
        let file = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(
            file.path(),
            "postgresql://fixture@localhost?sslmode=require",
        )
        .await
        .unwrap();
        let config: Config = serde_json::from_value(serde_json::json!({
            "maintenance": {"manager_database_credentials_file": file.path()}
        }))
        .unwrap();
        let error = connect(&config).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Supervisor manager database credentials require sslmode=verify-full"
        );
    }
}
