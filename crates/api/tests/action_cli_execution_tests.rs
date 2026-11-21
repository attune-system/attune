//! Real binary execution through immutable admission, worker preparation and completion.
#![cfg(target_os = "linux")]
#[path = "../../common/tests/helpers.rs"]
mod fixtures;
mod helpers;

use attune_common::{
    artifact_transport::VolumeTransport,
    models::{Execution, ExecutionStatus, OutputFormat, WorkerRole, WorkerStatus, WorkerType},
    pack_format::LaunchSpec,
    repositories::{
        action::{ActionRepository, UpdateActionInput},
        component_lifecycle::PackProjectionIds,
        delegated_execution::DelegatedExecutionRepository,
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{
            CreateExecutionInput, ExecutionRepository, UpdateExecutionInput,
            WorkerExecutionAssignment,
        },
        execution_program::ExecutionProgramRepository,
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        runtime::{CreateWorkerInput, WorkerRepository},
        schema_contract::{BindingOwner, SchemaContractRepository},
        Delete, FindById, Patch, Update,
    },
    value_schema::CompiledContract,
};
use attune_worker::{
    artifacts::ArtifactManager,
    runtime::{NativeRuntime, RuntimeRegistry},
    ActionExecutor, SecretManager,
};
use fixtures::{ActionFixture, IdentityFixture, PackFixture, RuntimeFixture};
use serde_json::{json, Value};
use std::sync::Arc;

struct OwnedApiServer {
    url: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl OwnedApiServer {
    async fn start(router: axum::Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (shutdown, signal) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = signal.await;
                })
                .await
                .unwrap();
        });
        Self {
            url,
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }
    async fn stop(mut self) {
        self.shutdown.take().unwrap().send(()).unwrap();
        self.task.take().unwrap().await.unwrap();
    }
}
impl Drop for OwnedApiServer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

enum Case {
    Command,
    Packaged,
    Stdin,
    MissingRoot,
    ProtectedInput,
}

async fn assert_assignment_fences(
    db: &sqlx::PgPool,
    scheduled: &Execution,
    input: CreateWorkerInput,
    first: uuid::Uuid,
) {
    let worker = scheduled.worker.unwrap();
    let assignment = WorkerExecutionAssignment {
        instance: first,
        scheduled_at: scheduled.updated,
        pack_release: scheduled.pack_release,
        pack_release_digest: scheduled.pack_release_digest.clone(),
        recorded: false,
    };
    let mut tx = db.begin().await.unwrap();
    let claimed = ExecutionRepository::claim_worker_execution(
        &mut tx,
        scheduled.id,
        worker,
        Some(&assignment),
    )
    .await
    .unwrap();
    assert_eq!(claimed.execution.status, ExecutionStatus::Running);
    assert!(ExecutionRepository::claim_worker_execution(
        &mut tx,
        scheduled.id,
        worker,
        Some(&assignment)
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();

    let second = uuid::Uuid::new_v4();
    let replacement = WorkerRepository::register_incarnation(db, input, WorkerRole::Action, second)
        .await
        .unwrap();
    assert_eq!(replacement.id, worker);
    let mut tx = db.begin().await.unwrap();
    assert!(ExecutionRepository::claim_worker_execution(
        &mut tx,
        scheduled.id,
        worker,
        Some(&assignment)
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();

    let stale = WorkerExecutionAssignment {
        instance: second,
        scheduled_at: scheduled.updated - chrono::TimeDelta::microseconds(1),
        ..assignment
    };
    let mut tx = db.begin().await.unwrap();
    assert!(ExecutionRepository::claim_worker_execution(
        &mut tx,
        scheduled.id,
        worker,
        Some(&stale)
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let canceling = ExecutionRepository::update(
        &mut *tx,
        scheduled.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Canceling),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let current = WorkerExecutionAssignment {
        scheduled_at: canceling.updated,
        ..stale
    };
    assert!(ExecutionRepository::claim_worker_execution(
        &mut tx,
        scheduled.id,
        worker,
        Some(&current)
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
}

async fn execute_case(case: Case) {
    let packaged = matches!(case, Case::Packaged);
    let stdin = matches!(case, Case::Stdin);
    let missing_root = matches!(case, Case::MissingRoot);
    let protected = matches!(case, Case::ProtectedInput);
    attune_common::auth::crypto_provider::install();
    let ctx = helpers::TestContext::new()
        .await
        .unwrap()
        .with_admin_auth()
        .await
        .unwrap();
    let db = ctx.pool.clone();
    let encryption_key = ctx.state.config.security.encryption_key.clone().unwrap();
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let release_root = attune_common::pack_transport::release_cache_path(
        root.path().join("packs"),
        "cli",
        &"d".repeat(64),
    )
    .unwrap();
    std::fs::create_dir_all(&release_root).unwrap();
    let tools = root.path().join("tools");
    std::fs::create_dir(&tools).unwrap();
    let probe = tools.join("cli-probe");
    // Docker's lean runtime receives this companion from its Rust builder.
    // Direct Cargo runs compile the same dependency-free source locally.
    let executable = std::env::current_exe().unwrap();
    let companion = executable
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures/action_cli_probe");
    if companion.is_file() {
        std::fs::copy(companion, &probe).unwrap();
    } else {
        let compiled =
            std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
                .arg("--edition=2021")
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/support/action_cli_probe.rs"
                ))
                .arg("-o")
                .arg(&probe)
                .output()
                .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
    }
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let pack = PackFixture::new_unique("cli_worker")
        .create(&db)
        .await
        .unwrap();
    let mut runtime = RuntimeFixture::new(None, None, "native");
    runtime.execution_config = json!({});
    let runtime = runtime.create(&db).await.unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "run")
        .with_runtime(runtime.id)
        .create(&db)
        .await
        .unwrap();
    let launch: LaunchSpec = if packaged {
        serde_json::from_value(
            json!({"type":"native","artifact":"probe","arguments":[{"literal":"generate","order":0}]}),
        )
        .unwrap()
    } else {
        serde_json::from_value(if stdin {
            json!({"type":"command","program":probe})
        } else {
            json!({"type":"command","program":probe,"arguments":[{"literal":"generate","order":0}]})
        })
        .unwrap()
    };
    let mut parameters = if stdin {
        json!({"message":{"type":"string","default":"stdin value"}})
    } else {
        json!({"message":{"type":"string","default":"a b; $(false) 世界", "cli":{"kind":"positional","order":10}}})
    };
    if protected {
        parameters["credential"] = json!({"type":"string","secret":true});
    }
    let action = ActionRepository::update(
        &db,
        action.id,
        UpdateActionInput {
            entrypoint: Some(Patch::Clear),
            launch: Some(Patch::Set(launch.clone())),
            param_schema: Some(parameters),
            parameter_delivery: Some(if stdin { "stdin" } else { "none" }.into()),
            output_format: Some(OutputFormat::Text.to_string()),
            output_policy: Some(
                serde_json::from_value(
                    json!({"stdout":"public","stderr":"public","error":"public"}),
                )
                .unwrap(),
            ),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let manifest = if packaged {
        use sha2::{Digest, Sha256};
        let path = release_root.join("probe");
        std::fs::copy(&probe, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        json!({"schema":"attune.pack.release/v1", "pack":{"ref":pack.r#ref,"version":pack.version},
            "requires":{"attune":"*","platform_catalog":"v1","runtimes":["native"]},"dependencies":{},
            "files":[{"path":"probe","size":bytes.len(),"sha256":hash,"mode":"0755"}],
            "artifacts":[{"kind":"native","id":"probe","component_refs":[action.r#ref],"variants":[{"id":"linux","path":"probe","target":{"os":"linux","libc":"static","arch":if std::env::consts::ARCH=="x86_64" {"amd64"}else{"arm64"}}}]}],
            "evidence":{"sboms":[],"provenance":[]}})
    } else {
        json!({})
    };
    let owner = IdentityFixture::new_unique("cli_owner")
        .create(&db)
        .await
        .unwrap();
    let incarnation = uuid::Uuid::new_v4();
    let worker_input = CreateWorkerInput {
        name: "cli-worker".into(),
        worker_type: WorkerType::Local,
        runtime: None,
        host: None,
        port: None,
        status: Some(WorkerStatus::Active),
        capabilities: Some(
            json!({"role":"action","runtimes":["native"],"action_cli_arguments_v1":true}),
        ),
        meta: None,
    };
    let worker = WorkerRepository::register_incarnation(
        &db,
        worker_input.clone(),
        WorkerRole::Action,
        incarnation,
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version.clone(),
            digest: "d".repeat(64),
            object_key: "owned/cli-worker".into(),
            provider_version: "fixture".into(),
            content_path: release_root.to_str().unwrap().into(),
            archive_size: 1,
            manifest,
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        release.id,
        &PackProjectionIds {
            actions: vec![action.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id)
        .await
        .unwrap();
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot)
        .await
        .unwrap();
    let config = CompiledContract::new(&binding.input)
        .unwrap()
        .apply_defaults(&if protected {
            json!({"credential":"must-stay-private"})
        } else {
            json!({})
        })
        .unwrap();
    let (stored_config, secrets) = attune_common::secret_values::redact_secret_parameters(
        config.clone(),
        action.param_schema.as_ref(),
    );
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            executor: Some(owner.id),
            config: Some(stored_config),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
        .await
        .unwrap();
    attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx, attune_common::secret_values::ENTITY_EXECUTION_CONFIG, execution.id,
        &attune_common::secret_values::prepare_secret_values(secrets, &encryption_key).unwrap()
    ).await.unwrap();
    DelegatedExecutionRepository::select(&mut tx, execution.id, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let scheduled = ExecutionRepository::update(
        &db,
        execution.id,
        UpdateExecutionInput {
            worker: Some(worker.id),
            status: Some(ExecutionStatus::Scheduled),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    if matches!(case, Case::Command) {
        assert_assignment_fences(&db, &scheduled, worker_input, incarnation).await;
    }
    // Mutable current metadata must never change an admitted execution.
    ActionRepository::update(
        &db,
        action.id,
        UpdateActionInput {
            launch: Some(Patch::Set(
                serde_json::from_value(
                    json!({"type":"command","program":"unavailable-new-program"}),
                )
                .unwrap(),
            )),
            param_schema: Some(json!({})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    if missing_root {
        std::fs::remove_dir_all(&release_root).unwrap();
    }
    let artifacts = std::path::PathBuf::from(&ctx.state.config.artifacts_dir);
    let server = OwnedApiServer::start(ctx.app.clone()).await;
    let token_provider = Arc::new(attune_common::auth::worker_token::WorkerTokenProvider::new(
        ctx.user.as_ref().unwrap().id,
        worker.id.to_string(),
        ctx.state.jwt_config.as_ref().clone(),
    ));
    let transport = VolumeTransport::new_with_completion_api(
        artifacts.to_str().unwrap(),
        &server.url,
        token_provider.clone(),
    )
    .unwrap();
    let mut runtimes = RuntimeRegistry::new();
    runtimes.register(Box::new(NativeRuntime::new()));
    let executor = ActionExecutor::new(
        db.clone(),
        runtimes,
        ArtifactManager::new(artifacts.clone()),
        SecretManager::new(db.clone(), Some(encryption_key.clone())).unwrap(),
        4096,
        4096,
        false,
        false,
        "cli-worker".into(),
        token_provider.instance_id(),
        attune_common::config::ArtifactsConfig::default().log_segment_writer_config(),
        None,
        None,
        root.path().join("packs"),
        artifacts.clone(),
        root.path().join("runtimes"),
        server.url.clone(),
        ctx.state.jwt_config.as_ref().clone(),
        Arc::new(transport),
    );
    executor.set_worker_id(worker.id);
    let result = executor.execute(execution.id).await;
    let stored = ExecutionRepository::get_by_id(&db, execution.id)
        .await
        .unwrap();
    let selections: Vec<(Value,)> =
        sqlx::query_as("SELECT selection FROM execution_program_selection WHERE execution=$1")
            .bind(execution.id)
            .fetch_all(&db)
            .await
            .unwrap();
    if missing_root || protected {
        assert!(result.is_err());
        assert_eq!(stored.status, ExecutionStatus::Failed);
        assert!(selections.is_empty());
        if protected {
            let error = stored.result.as_ref().unwrap().to_string();
            assert!(error.contains("action.cli.no_secure_delivery"), "{error}");
            assert!(!error.contains("must-stay-private"));
        }
    } else {
        assert!(result.unwrap().is_success(), "{:?}", stored.result);
        assert_eq!(stored.status, ExecutionStatus::Completed);
        assert!(
            stored
                .result
                .as_ref()
                .unwrap()
                .get("logs_incomplete")
                .is_none(),
            "{:?}",
            stored.result
        );
        let message = "a b; $(false) 世界";
        let input = if stdin {
            format!("{}\n", serde_json::to_string(&config).unwrap())
        } else {
            String::new()
        };
        let arguments = if stdin {
            String::new()
        } else {
            format!("arg:8:generate\narg:{}:{message}\n", message.len())
        };
        let expected = format!(
            "{arguments}stdin:{}:{input}\ncwd:{}\ntoken:false\nparent:false\n",
            input.len(),
            release_root.display()
        );
        assert_eq!(stored.result.as_ref().unwrap()["stdout"], expected);
        assert_eq!(selections.len(), 1);
        assert_eq!(
            selections[0].0["artifact"],
            if packaged {
                json!("probe")
            } else {
                Value::Null
            }
        );
        assert!(!selections[0].0.to_string().contains("$(false)"));
        // Terminal attempts cannot write new launch metadata.
        let selected = serde_json::from_value(selections[0].0.clone()).unwrap();
        assert!(ExecutionProgramRepository::record(
            &db,
            execution.id,
            worker.id,
            release.id,
            &selected
        )
        .await
        .is_err());
    }
    ExecutionRepository::delete(&db, execution.id)
        .await
        .unwrap();
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM execution_program_selection WHERE execution=$1")
            .bind(execution.id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(remaining, 0);
    drop(executor);
    server.stop().await;
    root.close().unwrap();
    drop(db);
    ctx.cleanup().await.unwrap();
}

#[tokio::test]
async fn command_preserves_arguments_and_immutable_admission() {
    execute_case(Case::Command).await;
}
#[tokio::test]
async fn packaged_native_uses_verified_artifact_and_arguments() {
    execute_case(Case::Packaged).await;
}
#[tokio::test]
async fn command_stdin_json_reaches_real_binary() {
    execute_case(Case::Stdin).await;
}
#[tokio::test]
async fn missing_release_root_fails_before_selection_or_spawn() {
    execute_case(Case::MissingRoot).await;
}

#[tokio::test]
async fn protected_unbound_input_with_no_stdin_fails_before_spawn() {
    execute_case(Case::ProtectedInput).await;
}
