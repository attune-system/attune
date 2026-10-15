// Included only by websocket_server's nested test module.
use super::*;
use std::sync::Mutex;
use tracing::{instrument::WithSubscriber, Subscriber};
use tracing_subscriber::{layer::Context, prelude::*, Layer};

#[derive(Clone, Default)]
struct QueryCount(Arc<AtomicUsize>);

impl<S: Subscriber> Layer<S> for QueryCount {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[tokio::test]
#[ignore = "explicit database-backed per-identity notifier authorization load profile"]
async fn artifact_notification_authorization_load_profile() -> Result<()> {
    use attune_common::{
        models::{
            ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType,
            RetentionPolicyType,
        },
        repositories::{artifact::CreateArtifactInput, Create},
        test_database::TestDatabase,
    };
    let config = Config::load_from_file(&format!(
        "{}/../../config.test.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))?;
    let db = TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop();
    let artifact = ArtifactRepository::create(
        db.pool(),
        CreateArtifactInput {
            r#ref: "notifier.load.progress".into(),
            scope: OwnerType::Action,
            owner: "notifier.load".into(),
            r#type: ArtifactType::Progress,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::General,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 10,
            name: None,
            description: None,
            content_type: None,
            data: None,
        },
    )
    .await?;
    let mut cases = Vec::new();
    for identities in [1, 16, 64] {
        for tabs in [1, 4] {
            let manager = SubscriberManager::new();
            let mut receivers = Vec::new();
            for identity in 1..=identities {
                let auth = Arc::new(WebSocketAuthContext::test_context(
                    identity,
                    TokenType::Access,
                    vec![Grant {
                        constraints: Some(GrantConstraints {
                            ids: Some(vec![artifact.id]),
                            ..Default::default()
                        }),
                        ..read_grant(Resource::Artifacts)
                    }],
                ));
                for _ in 0..tabs {
                    receivers.push(register_all(&manager, auth.clone()));
                }
            }
            let mut denied = register_all(&manager, test_auth_ctx(999, TokenType::Access));
            let statements = QueryCount::default();
            let evaluations = AtomicUsize::new(0);
            let auth_seconds = Mutex::new(0.0f64);
            let started = std::time::Instant::now();
            for _ in 0..20 {
                let mut notification = make_notification("artifact");
                notification.entity_id = artifact.id;
                let statements = statements.clone();
                let subscriber = tracing_subscriber::registry().with(statements);
                dispatch_with_authorizer(&manager, notification.clone(), |auth| {
                    let notification = notification.clone();
                    let evaluations = &evaluations;
                    let auth_seconds = &auth_seconds;
                    let pool = db.pool();
                    async move {
                        evaluations.fetch_add(1, Ordering::Relaxed);
                        let start = std::time::Instant::now();
                        let allowed =
                            notification_allowed_for_identity(&notification, &auth, pool).await;
                        *auth_seconds.lock().unwrap() += start.elapsed().as_secs_f64();
                        allowed
                    }
                })
                .with_subscriber(subscriber)
                .await;
                for receiver in &mut receivers {
                    assert_eq!(receiver.try_recv()?.entity_id, artifact.id);
                    assert!(matches!(
                        receiver.try_recv(),
                        Err(mpsc::error::TryRecvError::Empty)
                    ));
                }
                assert!(matches!(
                    denied.try_recv(),
                    Err(mpsc::error::TryRecvError::Empty)
                ));
            }
            let evaluated = evaluations.load(Ordering::Relaxed);
            assert_eq!(evaluated, 20 * (identities as usize + 1));
            assert!(
                statements.0.load(Ordering::Relaxed) >= evaluated,
                "real visibility SQL must be observed for every identity evaluation"
            );
            cases.push(
                serde_json::json!({"allowed_identities":identities,"tabs_per_identity":tabs,
                "notifications":20,"authorization_evaluations":evaluated,
                "delivered_frames":20 * identities * tabs,"denied_identity_frames":0,
                "dispatch_wall_seconds":started.elapsed().as_secs_f64(),
                "authorization_wall_seconds_including_database":*auth_seconds.lock().unwrap(),
                "request_scoped_sqlx_statements":statements.0.load(Ordering::Relaxed)}),
            );
            drop(receivers);
            drop(denied);
            drop(manager);
        }
    }
    db.cleanup().await?;
    let report = serde_json::json!({"cases":cases,"scope":"real artifact visibility and per-fingerprint delivery authorizer",
        "limitations":["in-process fan-out, not WebSocket wire latency","authorization wall time includes database waits",
            "connection-time auth contexts supplied by fixture; no change to per-identity delivery policy"]});
    println!("{}", serde_json::to_string(&report)?);
    if let Some(root) = std::env::var_os("ATTUNE_ARTIFACT_BENCHMARK_OUTPUT") {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(std::path::PathBuf::from(root).join("notifier-authorization-profile.json"))?;
        writeln!(file, "{}", serde_json::to_string_pretty(&report)?)?;
    }
    Ok(())
}
