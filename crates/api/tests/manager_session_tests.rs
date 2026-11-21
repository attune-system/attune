use anyhow::Result;
use attune_api::{server::Server, state::AppState};
use attune_common::{
    config::{BlobStorageConfig, Config},
    manager_control::{
        ManagerControlClient, ManagerEnrollment, ManagerEnrollmentGrant, MANAGER_CONTROL_PATH,
    },
    models::WorkerRole,
    repositories::manager_session::ManagerSessionRepository,
    test_database::TestDatabase,
    Error,
};
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const ACTION_SECRET: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const SENSOR_SECRET: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn grant(role: WorkerRole, pool: &str, secret: &str) -> ManagerEnrollmentGrant {
    ManagerEnrollmentGrant {
        role,
        pool: pool.into(),
        secret_sha256: hex::encode(Sha256::digest(secret.as_bytes())),
    }
}

async fn serve(
    state: AppState,
    stop: CancellationToken,
) -> Result<(String, JoinHandle<std::io::Result<()>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base_url = format!("http://{}", listener.local_addr()?);
    let app = Server::new(Arc::new(state)).router();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
    });
    Ok((base_url, server))
}

#[tokio::test]
async fn manager_sessions_are_scoped_revocable_and_retry_safe() -> Result<()> {
    attune_common::auth::crypto_provider::install();
    let config_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.test.yaml");
    let mut config = Config::load_from_file(config_path.to_str().unwrap())?;
    let scratch = tempfile::tempdir()?;
    config.storage = BlobStorageConfig::Filesystem {
        root: scratch.path().join("blobs"),
    };
    config.security.jwt_secret = Some("isolated-test-replica-signing-key-manager-control".into());
    config.manager_enrollments = vec![
        grant(WorkerRole::Action, "build", ACTION_SECRET),
        grant(WorkerRole::Sensor, "timer", SENSOR_SECRET),
    ];
    let db = TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop();
    let stop = CancellationToken::new();
    let _ensure_shutdown = stop.clone().drop_guard();
    let (first_url, first_task) = serve(
        AppState::new(db.pool().clone(), config.clone()),
        stop.clone(),
    )
    .await?;
    config
        .manager_enrollments
        .retain(|grant| grant.role == WorkerRole::Sensor);
    let (restricted_url, restricted_task) = serve(
        AppState::new(db.pool().clone(), config.clone()),
        stop.clone(),
    )
    .await?;
    config
        .manager_enrollments
        .push(grant(WorkerRole::Action, "build", ACTION_SECRET));
    let (second_url, second_task) =
        serve(AppState::new(db.pool().clone(), config), stop.clone()).await?;

    let result: Result<()> = async {
        let first = ManagerControlClient::new(&first_url, true)?;
        let second = ManagerControlClient::new(&second_url, true)?;
        let restricted = ManagerControlClient::new(&restricted_url, true)?;
        let mut enrollment = ManagerEnrollment {
            role: WorkerRole::Action,
            pool: "build".into(),
            name: "runner-0".into(),
            instance: Uuid::new_v4(),
        };

        assert!(matches!(
            first.enroll(SENSOR_SECRET, &enrollment).await,
            Err(Error::PermissionDenied(_))
        ));
        let (one, two) = tokio::try_join!(
            first.enroll(ACTION_SECRET, &enrollment),
            second.enroll(ACTION_SECRET, &enrollment),
        )?;
        assert_eq!(one.id, two.id);
        assert_eq!(one.expires_at, two.expires_at);
        assert!(one.token == two.token);
        assert!(matches!(
            restricted.renew(&one).await,
            Err(Error::PermissionDenied(_))
        ));
        assert!(matches!(
            restricted.withdraw(&one).await,
            Err(Error::PermissionDenied(_))
        ));

        let impostor = ManagerEnrollment {
            instance: Uuid::new_v4(),
            ..enrollment.clone()
        };
        assert!(matches!(
            second.enroll(ACTION_SECRET, &impostor).await,
            Err(Error::InvalidState(_))
        ));
        let renewed = second.renew(&one).await?;
        assert_eq!(renewed.id, one.id);
        assert!(renewed.expires_at >= one.expires_at);
        assert_eq!(first.renew(&one).await?.id, one.id);
        let mut expired_effect = db.pool().begin().await?;
        assert!(ManagerSessionRepository::renew(
            &mut expired_effect,
            one.id,
            chrono::DateTime::UNIX_EPOCH
        )
        .await?
        .is_none());
        expired_effect.rollback().await?;

        let client = reqwest::Client::new();
        let other_api = client
            .get(format!("{first_url}/auth/me"))
            .bearer_auth(&renewed.token)
            .send()
            .await?;
        assert_eq!(other_api.status(), StatusCode::UNAUTHORIZED);
        let mut bad = renewed.token.clone();
        bad.push('a');
        assert_eq!(
            client
                .post(format!("{first_url}{MANAGER_CONTROL_PATH}/session"))
                .bearer_auth(bad)
                .send()
                .await?
                .status(),
            StatusCode::UNAUTHORIZED
        );

        first.withdraw(&renewed).await?;
        second.withdraw(&renewed).await?;
        assert!(matches!(
            second.renew(&one).await,
            Err(Error::PermissionDenied(_))
        ));
        assert!(matches!(
            first.enroll(ACTION_SECRET, &enrollment).await,
            Err(Error::InvalidState(_))
        ));
        let replacement = second.enroll(ACTION_SECRET, &impostor).await?;
        assert_ne!(replacement.id, one.id);
        first.withdraw(&one).await?;
        assert_eq!(first.renew(&replacement).await?.id, replacement.id);

        // Advance the durable deadline without a wall-clock sleep.
        sqlx::query("UPDATE manager_session SET expires_at = clock_timestamp() WHERE id=$1")
            .bind(replacement.id)
            .execute(db.pool())
            .await?;
        assert!(matches!(
            second.renew(&replacement).await,
            Err(Error::PermissionDenied(_))
        ));
        assert!(matches!(
            second.withdraw(&replacement).await,
            Err(Error::PermissionDenied(_))
        ));
        let fresh = ManagerEnrollment {
            instance: Uuid::new_v4(),
            ..enrollment.clone()
        };
        assert_ne!(
            first.enroll(ACTION_SECRET, &fresh).await?.id,
            replacement.id
        );

        enrollment.role = WorkerRole::Sensor;
        enrollment.pool = "timer".into();
        assert!(matches!(
            first.enroll(ACTION_SECRET, &enrollment).await,
            Err(Error::PermissionDenied(_))
        ));
        let sensor = restricted.enroll(SENSOR_SECRET, &enrollment).await?;
        assert_eq!(first.renew(&sensor).await?.instance, enrollment.instance);
        Ok(())
    }
    .await;

    stop.cancel();
    first_task.await??;
    restricted_task.await??;
    second_task.await??;
    db.cleanup().await?;
    result
}
