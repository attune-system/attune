//! Owning canonical issuer/native-effect fixtures. Exact provider receipts are
//! repository doubles; these cases do not contact a provider or claim rollout.
use super::*;
use attune_common::{
    isolated_process::ProcessSpec,
    repositories::{
        execution_input::{ExecutionInputRepository as Input, SelectedExecutionInput},
        execution_process::ExecutionProcessRepository as Effects,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        identity::UpdatePermissionSetInput,
        recorded_input_authority::RecordedInputAuthorityRepository as Captured,
    },
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{
        prepare_secret_values, redaction_marker, SecretValueInput, ENTITY_EXECUTION_CONFIG,
    },
    workload_launch::*,
};

const MASTER: &str = "native-input-frontier-owned-test-master";
const PRIVATE: &str = "native-input-frontier-private-fixture";
struct Source {
    fixture: Fixture,
    spec: ProcessSpec,
    permission: Option<Id>,
    origin: SecretOrigin,
}

async fn seed(
    secret: bool,
    allowed: bool,
    lifetime: Option<Duration>,
) -> (TestDatabase, anyhow::Result<Source>) {
    seed_with_launch(
        secret,
        allowed,
        lifetime,
        json!({"token":{"type":"string","secret":true}}),
        None,
    )
    .await
}

async fn seed_with_launch(
    secret: bool,
    allowed: bool,
    lifetime: Option<Duration>,
    schema: serde_json::Value,
    launch: Option<attune_common::pack_format::LaunchSpec>,
) -> (TestDatabase, anyhow::Result<Source>) {
    let (db, fixture) = fixture_with_launch(Some(schema), None, lifetime, launch).await;
    let source = async {
        let owner = &fixture.policy.policy.owner;
        let key = helpers::KeyFixture::new_pack(owner.pack_id,&owner.pack_ref,"native_input",PRIVATE).create(&db).await?;
        let origin = SecretOrigin::Key(attune_common::key_access::key_origin(&key));
        let permission = if allowed {
            let set = PermissionSetRepository::create(&db,CreatePermissionSetInput {
                r#ref:format!("{}.native_input",owner.pack_ref),pack:Some(owner.pack_id),pack_ref:Some(owner.pack_ref.clone()),
                label:None,description:None,grants:json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[key.r#ref]}}]),
            }).await?;
            PermissionAssignmentRepository::create(&db,CreatePermissionAssignmentInput {identity:fixture.human_id,permset:set.id}).await?;
            Some(set.id)
        } else { None };
        let instance=uuid::Uuid::new_v4();
        let worker=WorkerRepository::create(&db,CreateWorkerInput {
            name:format!("native-input-{}",instance),worker_type:WorkerType::Local,
            runtime:None,host:None,port:None,status:Some(WorkerStatus::Active),
            capabilities:Some(json!({})),meta:Some(json!({"registration_incarnation":instance})),
        }).await?;
        WorkerRepository::heartbeat_incarnation(db.pool(),worker.id,instance).await?;
        let execution=fixture.credential.execution_id;
        let mut tx=db.begin().await?;
        sqlx::query("UPDATE execution SET worker=$2,started_at=clock_timestamp(),timeout_seconds=300,config=$3 WHERE id=$1")
            .bind(execution).bind(worker.id).bind(if secret {json!({"token":redaction_marker()})} else {json!({})})
            .execute(&mut *tx).await?;
        if allowed { Captured::capture_human(&mut tx,"execution",execution,fixture.human_id).await?; }
        if secret { set_secret(&mut tx,execution,&origin).await?; }
        tx.commit().await?;
        let image=Sha256Digest::of_bytes(b"owned-input-image");
        let spec=ProcessSpec {
            intent:uuid::Uuid::new_v4(),purpose:LaunchPurpose::Action,
            target:LaunchTarget::Execution {execution,worker:worker.id,worker_instance:instance,attempt:fixture.credential.attempt},
            image:PinnedImage {reference:format!("sha256:{}",image.as_str()),sha256:image},
            platform:LaunchPlatform::LinuxAmd64,inputs:vec![],pack_access:PackAccess::ReadOnly,
            stage:None,timeout_ms:Some(300_000),invocation:LaunchInvocation {
                argv:vec!["/opt/attune/pack/owned-test-tool".into()],env:Default::default(),working_root:LaunchRoot::Pack,
            },
        };
        Ok(Source {fixture,spec,permission,origin})
    }.await;
    (db, source)
}
async fn set_secret(
    conn: &mut sqlx::PgConnection,
    execution: Id,
    origin: &SecretOrigin,
) -> anyhow::Result<()> {
    let fields = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/token".into(),
            value: json!(PRIVATE),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&SecretProvenance {
                origins: vec![origin.clone()],
                templates: vec![],
            })?),
        }],
        MASTER,
    )?;
    Secrets::upsert_many_with_conn(conn, ENTITY_EXECUTION_CONFIG, execution, &fields).await?;
    Ok(())
}
async fn selected(db: &TestDatabase, source: &Source) -> anyhow::Result<SelectedExecutionInput> {
    let mut tx = db.begin().await?;
    let input = Input::restore(
        &mut tx,
        source.fixture.credential.execution_id,
        Some(MASTER),
        None,
    )
    .await?;
    let (value, _, snapshot, fence) = input.into_parts();
    tx.commit().await?;
    fence.ensure_fresh()?;
    Ok(SelectedExecutionInput::new(value, snapshot)?)
}

fn resource(source: &Source) -> attune_common::isolated_process::ResourceIdentity {
    attune_common::isolated_process::ResourceIdentity::Kubernetes {
        cluster: "owned-fixture".into(),
        namespace: "owned-fixture".into(),
        pod_name: format!("owned-{}", source.spec.intent),
        pod_uid: uuid::Uuid::new_v4().to_string(),
    }
}
async fn reserve(
    db: &TestDatabase,
    source: &Source,
    input: &SelectedExecutionInput,
) -> anyhow::Result<()> {
    Effects::reserve(
        db.pool(),
        &source.spec,
        source.fixture.credential.sponsor_identity_id,
        input,
        MASTER,
    )
    .await?;
    Ok(())
}
async fn create(
    db: &TestDatabase,
    source: &Source,
) -> anyhow::Result<attune_common::isolated_process::ResourceIdentity> {
    use attune_common::isolated_process::ProcessCreateDisposition;
    anyhow::ensure!(
        Effects::begin_create(db.pool(), source.spec.intent, MASTER).await?
            == ProcessCreateDisposition::CreateOnce
    );
    anyhow::ensure!(
        Effects::begin_create(db.pool(), source.spec.intent, MASTER).await?
            == ProcessCreateDisposition::ReconcileOnly
    );
    let resource = resource(source);
    Effects::record_resource(db.pool(), source.spec.intent, &resource).await?;
    Ok(resource)
}
async fn deny_completion(db: &TestDatabase, source: &Source) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;
    let denied = matches!(
        Effects::lock_completion(&mut tx, source.spec.intent, MASTER).await,
        Err(attune_common::Error::PermissionDenied(_))
    );
    tx.rollback().await?;
    anyhow::ensure!(denied);
    Ok(())
}
async fn no_effect(db: &TestDatabase, intent: uuid::Uuid) -> anyhow::Result<()> {
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM execution_process_effect WHERE intent=$1")
            .bind(intent)
            .fetch_one(db.pool())
            .await?;
    anyhow::ensure!(rows == 0);
    Ok(())
}

#[tokio::test]
async fn native_input_secret_reserve_create_one_shot_consume_and_completion() -> anyhow::Result<()>
{
    let (db, source) = seed(true, true, None).await;
    let result = async {
        let source = source?;
        let input = selected(&db, &source).await?;
        reserve(&db, &source, &input).await?;
        let resource = create(&db, &source).await?;
        let payload =
            Effects::consume_stdin(db.pool(), source.spec.intent, &resource, MASTER).await?;
        anyhow::ensure!(
            serde_json::from_slice::<serde_json::Value>(payload.as_bytes())?
                == json!({"token":PRIVATE})
        );
        anyhow::ensure!(!format!("{payload:?}").contains(PRIVATE));
        anyhow::ensure!(
            Effects::consume_stdin(db.pool(), source.spec.intent, &resource, MASTER)
                .await
                .is_err()
        );
        deny_completion(&db, &source).await?;
        Effects::record_exit(
            db.pool(),
            source.spec.intent,
            &resource,
            LaunchExitOutcome {
                exit_code: 0,
                oom_killed: false,
            },
        )
        .await?;
        let LaunchTarget::Execution {
            execution, worker, ..
        } = source.spec.target
        else {
            unreachable!()
        };
        let mut tx = db.begin().await?;
        anyhow::ensure!(
            Effects::lock_completion(&mut tx, source.spec.intent, MASTER)
                .await?
                .is_none()
        );
        attune_common::repositories::runtime_read::RuntimeReadRepository::close_execution(
            &mut tx, execution,
        )
        .await?;
        ExecutionRepository::transition_worker(
            &mut *tx,
            execution,
            Some(worker),
            ExecutionStatus::Completed,
            Some(json!({"public":"retained-public-output"})),
        )
        .await?;
        tx.commit().await?;
        let mut tx = db.begin().await?;
        let historical = Effects::lock_completion(&mut tx, source.spec.intent, MASTER)
            .await?
            .ok_or_else(|| anyhow::anyhow!("missing committed completion"))?;
        anyhow::ensure!(historical.execution.status == ExecutionStatus::Completed);
        anyhow::ensure!(
            historical.execution.result == Some(json!({"public":"retained-public-output"}))
        );
        tx.commit().await?;
        Ok(())
    }
    .await;
    db.cleanup().await?;
    result
}

#[tokio::test]
async fn native_input_secret_without_captured_grants_cannot_reserve() -> anyhow::Result<()> {
    let (db, source) = seed(true, true, None).await;
    let result=async {
        let source=source?;
        let selected=selected(&db,&source).await?;
        let execution=source.fixture.credential.execution_id;
        sqlx::query("DELETE FROM recorded_execution_input_authority WHERE owner_kind='execution' AND owner_id=$1").bind(execution).execute(db.pool()).await?;
        let mut tx=db.begin().await?;
        let denied=matches!(Input::restore(&mut tx,execution,Some(MASTER),None).await,Err(attune_common::Error::PermissionDenied(_)));
        tx.rollback().await?;
        anyhow::ensure!(denied);
        anyhow::ensure!(matches!(Effects::reserve(db.pool(),&source.spec,source.fixture.credential.sponsor_identity_id,&selected,MASTER).await,Err(attune_common::Error::PermissionDenied(_))));
        no_effect(&db,source.spec.intent).await?;
        Ok(())
    }.await;
    db.cleanup().await?;
    result
}

#[tokio::test]
async fn native_input_captured_grants_survive_human_freeze_and_permission_edits(
) -> anyhow::Result<()> {
    let (db, source) = seed(true, true, None).await;
    let result = async {
        let source = source?;
        IdentityRepository::update(
            &db,
            source.fixture.human_id,
            UpdateIdentityInput {
                frozen: Some(true),
                ..Default::default()
            },
        )
        .await?;
        PermissionSetRepository::update(
            &db,
            source.permission.unwrap(),
            UpdatePermissionSetInput {
                grants: Some(json!([])),
                ..Default::default()
            },
        )
        .await?;
        let mut tx = db.begin().await?;
        let input = Input::restore(
            &mut tx,
            source.fixture.credential.execution_id,
            Some(MASTER),
            None,
        )
        .await?;
        let value = input.value().clone();
        let (_, _, _, fence) = input.into_parts();
        tx.commit().await?;
        fence.ensure_fresh()?;
        anyhow::ensure!(value == json!({"token":PRIVATE}));
        let frozen = selected(&db, &source).await?;
        reserve(&db, &source, &frozen).await?;
        let resource = create(&db, &source).await?;
        let payload =
            Effects::consume_stdin(db.pool(), source.spec.intent, &resource, MASTER).await?;
        anyhow::ensure!(serde_json::from_slice::<serde_json::Value>(payload.as_bytes())? == value);
        Ok(())
    }
    .await;
    let cleanup = db.cleanup().await;
    cleanup?;
    result
}

#[tokio::test]
async fn native_input_equal_plaintext_with_replaced_ciphertext_invalidates_snapshot(
) -> anyhow::Result<()> {
    let (db, source) = seed(true, true, None).await;
    let result = async {
        let source = source?;
        let execution = source.fixture.credential.execution_id;
        let mut tx = db.begin().await?;
        let input = Input::restore(&mut tx, execution, Some(MASTER), None).await?;
        let (value, _, snapshot, fence) = input.into_parts();
        tx.commit().await?;
        fence.ensure_fresh()?;
        let frozen = SelectedExecutionInput::new(value, snapshot.clone())?;
        reserve(&db, &source, &frozen).await?;
        let resource = create(&db, &source).await?;
        let mut tx = db.begin().await?;
        set_secret(&mut tx, execution, &source.origin).await?;
        tx.commit().await?;
        let mut tx = db.begin().await?;
        let denied = matches!(
            Input::restore(&mut tx, execution, Some(MASTER), Some(&snapshot)).await,
            Err(attune_common::Error::PermissionDenied(_))
        );
        tx.rollback().await?;
        let mut tx = db.begin().await?;
        let input = Input::restore(&mut tx, execution, Some(MASTER), None).await?;
        let value = input.value().clone();
        let (_, _, _, fence) = input.into_parts();
        tx.commit().await?;
        fence.ensure_fresh()?;
        anyhow::ensure!(denied && value == json!({"token":PRIVATE}));
        anyhow::ensure!(matches!(
            Effects::consume_stdin(db.pool(), source.spec.intent, &resource, MASTER).await,
            Err(attune_common::Error::PermissionDenied(_))
        ));
        Ok(())
    }
    .await;
    let cleanup = db.cleanup().await;
    cleanup?;
    result
}

#[tokio::test]
async fn native_input_captured_group_revoke_restore_cannot_revive_reserved_effect(
) -> anyhow::Result<()> {
    let (db, source) = seed(true, true, None).await;
    let result=async {
        let source=source?;
        let execution=source.fixture.credential.execution_id;
        let frozen=selected(&db,&source).await?;
        Effects::reserve(db.pool(),&source.spec,source.fixture.credential.sponsor_identity_id,&frozen,MASTER).await?;
        let resource=create(&db,&source).await?;
        let mut tx=db.begin().await?;
        let (body,digest):(serde_json::Value,String)=sqlx::query_as("DELETE FROM recorded_execution_input_authority WHERE owner_kind='execution' AND owner_id=$1 RETURNING authority,authority_digest")
            .bind(execution).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        let denied=matches!(Effects::consume_stdin(db.pool(),source.spec.intent,&resource,MASTER).await,Err(attune_common::Error::PermissionDenied(_)));
        sqlx::query("INSERT INTO recorded_execution_input_authority(owner_kind,owner_id,authority,authority_digest) VALUES('execution',$1,$2,$3)")
            .bind(execution).bind(body).bind(digest).execute(db.pool()).await?;
        let _restored=selected(&db,&source).await?;
        let still_denied=matches!(Effects::consume_stdin(db.pool(),source.spec.intent,&resource,MASTER).await,Err(attune_common::Error::PermissionDenied(_)));
        anyhow::ensure!(denied && still_denied);
        Ok(())
    }.await;
    let cleanup = db.cleanup().await;
    cleanup?;
    result
}

#[tokio::test]
async fn native_input_runtime_sponsor_freeze_restore_cannot_revive_reserved_effect(
) -> anyhow::Result<()> {
    let (db, source) = seed(true, true, None).await;
    let result = async {
        let source = source?;
        let selected = selected(&db, &source).await?;
        Effects::reserve(
            db.pool(),
            &source.spec,
            source.fixture.credential.sponsor_identity_id,
            &selected,
            MASTER,
        )
        .await?;
        for frozen in [true, false] {
            IdentityRepository::update(
                &db,
                source.fixture.credential.sponsor_identity_id,
                UpdateIdentityInput {
                    frozen: Some(frozen),
                    ..Default::default()
                },
            )
            .await?;
            if !frozen {
                let _freshly_authorized = self::selected(&db, &source).await?;
            }
            anyhow::ensure!(matches!(
                Effects::begin_create(db.pool(), source.spec.intent, MASTER).await,
                Err(attune_common::Error::PermissionDenied(_))
            ));
        }
        Ok(())
    }
    .await;
    let cleanup = db.cleanup().await;
    cleanup?;
    result
}

#[tokio::test]
async fn native_input_selected_parent_origin_revoke_restore_denies_native_completion(
) -> anyhow::Result<()> {
    let (db, source) = seed(true, true, None).await;
    let result=async {
        let source=source?;
        let execution=source.fixture.credential.execution_id;
        let child=ExecutionRepository::find_by_id(&db,execution).await?.unwrap();
        let mut tx=db.begin().await?;
        let parent=ExecutionRepository::create_pinned(&mut *tx,CreateExecutionInput {
            action:child.action,action_ref:child.action_ref.clone(),executor:Some(source.fixture.human_id),
            status:ExecutionStatus::Completed,config:Some(json!({"token":redaction_marker()})),..Default::default()
        },child.executable_snapshot.as_ref().unwrap()).await?;
        set_secret(&mut tx,parent.id,&source.origin).await?;
        set_secret(&mut tx,execution,&SecretOrigin::Entity {entity_type:ENTITY_EXECUTION_CONFIG.into(),entity_id:parent.id,path:"/token".into()}).await?;
        tx.commit().await?;
        let mut tx=db.begin().await?;
        let input=Input::restore(&mut tx,execution,Some(MASTER),None).await?;
        let (value,_,snapshot,fence)=input.into_parts(); tx.commit().await?; fence.ensure_fresh()?;
        let frozen=SelectedExecutionInput::new(value,snapshot.clone())?;
        reserve(&db,&source,&frozen).await?;
        let resource=create(&db,&source).await?;
        Effects::consume_stdin(db.pool(),source.spec.intent,&resource,MASTER).await?;
        Effects::record_exit(db.pool(),source.spec.intent,&resource,LaunchExitOutcome {exit_code:0,oom_killed:false}).await?;
        let buffer:Id=sqlx::query_scalar("SELECT id FROM execution_secret_value WHERE entity_type=$1 AND entity_id=$2 AND json_path='/token'")
            .bind(ENTITY_EXECUTION_CONFIG).bind(parent.id).fetch_one(db.pool()).await?;
        let before:i64=sqlx::query_scalar("SELECT COALESCE((SELECT epoch FROM workload_source_epoch WHERE kind='input_secret_buffer' AND subject=$1),0)")
            .bind(buffer).fetch_one(db.pool()).await?;
        let foreign=helpers::KeyFixture::new_system_unique("native_input_denied",PRIVATE).create(&db).await?;
        let mut tx=db.begin().await?;
        set_secret(&mut tx,parent.id,&SecretOrigin::Key(attune_common::key_access::key_origin(&foreign))).await?;
        tx.commit().await?;
        let mut tx=db.begin().await?;
        let denied=matches!(Input::restore(&mut tx,execution,Some(MASTER),Some(&snapshot)).await,Err(attune_common::Error::PermissionDenied(_)));
        tx.rollback().await?;
        deny_completion(&db,&source).await?;
        let mut tx=db.begin().await?; set_secret(&mut tx,parent.id,&source.origin).await?; tx.commit().await?;
        let mut tx=db.begin().await?;
        let input=Input::restore(&mut tx,execution,Some(MASTER),Some(&snapshot)).await?;
        let restored=input.value().clone(); let (_,_,_,fence)=input.into_parts(); tx.commit().await?; fence.ensure_fresh()?;
        let after:i64=sqlx::query_scalar("SELECT epoch FROM workload_source_epoch WHERE kind='input_secret_buffer' AND subject=$1")
            .bind(buffer).fetch_one(db.pool()).await?;
        anyhow::ensure!(denied && restored==json!({"token":PRIVATE}) && after>before);
        deny_completion(&db,&source).await?;
        Ok(())
    }.await;
    let cleanup = db.cleanup().await;
    cleanup?;
    result
}

async fn observed_wait_expiry(
    db: &TestDatabase,
    source: &Source,
    expiry: chrono::DateTime<Utc>,
) -> anyhow::Result<()> {
    let frozen = selected(db, source).await?;
    reserve(db, source, &frozen).await?;
    let resource = create(db, source).await?;
    let mut blocker = db.begin().await?;
    sqlx::query("SELECT intent FROM execution_process_effect WHERE intent=$1 FOR UPDATE")
        .bind(source.spec.intent)
        .fetch_one(&mut *blocker)
        .await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let read = Effects::consume_stdin(db.pool(), source.spec.intent, &resource, MASTER);
    let release = async {
        let observed=tokio::time::timeout(std::time::Duration::from_secs(40),async {
            let mut before_expiry=false;
            loop {
                let (blocked,expired):(bool,bool)=sqlx::query_as("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid))),clock_timestamp()>=$2")
                    .bind(pid).bind(expiry).fetch_one(db.pool()).await?;
                if blocked && !expired { before_expiry=true; }
                if before_expiry && blocked && expired { return Ok::<_,sqlx::Error>(true); }
                if !before_expiry && expired { return Ok(false); }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await;
        let rollback = blocker.rollback().await;
        (observed, rollback)
    };
    let (read, (observed, rollback)) = tokio::join!(read, release);
    rollback?;
    anyhow::ensure!(
        observed??,
        "must observe the actual unexpired operation wait before expiry"
    );
    anyhow::ensure!(
        matches!(read,Err(attune_common::Error::PermissionDenied(message)) if message.contains("expired"))
    );
    Ok(())
}

#[tokio::test]
async fn native_input_observed_native_operation_wait_rechecks_approval_expiry() -> anyhow::Result<()>
{
    let (db, source) = seed(true, true, Some(Duration::seconds(30))).await;
    let result = async {
        let source = source?;
        observed_wait_expiry(&db, &source, source.fixture.approval.expires_at.unwrap()).await
    }
    .await;
    db.cleanup().await?;
    result
}

#[tokio::test]
async fn native_input_command_inspection_gets_empty_stdin_no_token_and_exact_capture(
) -> anyhow::Result<()> {
    use attune_common::{action_cli::SelectedProgram, pack_format::LaunchSpec};
    let (db, source) = seed_with_launch(
        true,
        true,
        None,
        json!({"token":{"type":"string","secret":true}}),
        Some(LaunchSpec::Command {
            program: "/owned-tool".into(),
            arguments: vec![],
        }),
    )
    .await;
    let result = async {
        let mut source = source?;
        anyhow::ensure!(source.fixture.approval.expires_at.is_none());
        let action = source.spec.clone();
        source.spec.intent = uuid::Uuid::new_v4();
        source.spec.purpose = LaunchPurpose::RuntimeProbe;
        source.spec.invocation.argv = vec![
            format!("{RUNNER_ROOT}/linux-amd64"),
            "inspect-command-program".into(),
            "/owned-tool".into(),
        ];
        let input = selected(&db, &source).await?;
        let mut token_probe = source.spec.clone();
        token_probe.intent = uuid::Uuid::new_v4();
        token_probe.invocation.env.insert(
            "ATTUNE_API_TOKEN".into(),
            "forbidden-owned-fixture-token".into(),
        );
        anyhow::ensure!(matches!(
            Effects::reserve(
                db.pool(),
                &token_probe,
                source.fixture.credential.sponsor_identity_id,
                &input,
                MASTER
            )
            .await,
            Err(attune_common::Error::PermissionDenied(_))
        ));
        no_effect(&db, token_probe.intent).await?;
        reserve(&db, &source, &input).await?;
        let resource = create(&db, &source).await?;
        let empty =
            Effects::consume_stdin(db.pool(), source.spec.intent, &resource, MASTER).await?;
        anyhow::ensure!(empty.as_bytes().is_empty());
        anyhow::ensure!(
            Effects::consume_stdin(db.pool(), source.spec.intent, &resource, MASTER)
                .await
                .is_err()
        );
        let program = SelectedProgram {
            path: "/owned-tool".into(),
            sha256: Sha256Digest::of_bytes(b"same-image-owned-tool")
                .as_str()
                .into(),
            invocation_name: "/owned-tool".into(),
            artifact: None,
            variant: None,
        };
        anyhow::ensure!(Effects::complete_command_inspection(
            db.pool(),
            source.spec.intent,
            &resource,
            &program,
            MASTER
        )
        .await
        .is_err());
        Effects::record_exit(
            db.pool(),
            source.spec.intent,
            &resource,
            LaunchExitOutcome {
                exit_code: 0,
                oom_killed: false,
            },
        )
        .await?;
        Effects::complete_command_inspection(
            db.pool(),
            source.spec.intent,
            &resource,
            &program,
            MASTER,
        )
        .await?;
        anyhow::ensure!(
            Effects::inspected_program(db.pool(), &action, MASTER).await? == Some(program.clone())
        );
        deny_completion(&db, &source).await?;
        source.spec = action;
        source.spec.invocation.argv = vec![
            format!("{RUNNER_ROOT}/linux-amd64"),
            "exec".into(),
            program.sha256,
            program.path.to_str().unwrap().into(),
            program.invocation_name,
        ];
        reserve(&db, &source, &input).await?;
        let resource = create(&db, &source).await?;
        let payload =
            Effects::consume_stdin(db.pool(), source.spec.intent, &resource, MASTER).await?;
        anyhow::ensure!(
            serde_json::from_slice::<serde_json::Value>(payload.as_bytes())?
                == json!({"token":PRIVATE})
        );
        Ok(())
    }
    .await;
    db.cleanup().await?;
    result
}

#[tokio::test]
async fn native_input_secret_origin_path_cannot_be_bound_into_argv() -> anyhow::Result<()> {
    use attune_common::{
        action_cli::{ArgumentLimits, ArgumentPlan, InputDelivery, SelectedProgram},
        pack_format::LaunchSpec,
        repositories::execution_program::ExecutionProgramRepository,
        repositories::schema_contract::{BindingOwner, SchemaContractRepository},
    };
    let (db, source) = seed_with_launch(
        true,
        true,
        None,
        json!({"token":{"type":"string","cli":{"kind":"option","name":"--token","order":1}}}),
        Some(LaunchSpec::Native {
            artifact: "owned".into(),
            arguments: vec![],
        }),
    )
    .await;
    let result = async {
        let mut source = source?;
        let LaunchTarget::Execution {
            execution, worker, ..
        } = source.spec.target
        else {
            unreachable!()
        };
        let row = ExecutionRepository::find_by_id(&db, execution)
            .await?
            .unwrap();
        let program = SelectedProgram {
            path: "/opt/attune/pack/bin/owned".into(),
            sha256: Sha256Digest::of_bytes(b"owned-cli-fixture").as_str().into(),
            invocation_name: "/opt/attune/pack/bin/owned".into(),
            artifact: Some("owned".into()),
            variant: Some("linux-amd64-static".into()),
        };
        ExecutionProgramRepository::record(
            db.pool(),
            execution,
            worker,
            row.pack_release.unwrap(),
            &program,
        )
        .await?;
        let mut tx = db.begin().await?;
        let restored = Input::restore(&mut tx, execution, Some(MASTER), None).await?;
        let (parameters, paths, snapshot, expiry) = restored.into_parts();
        anyhow::ensure!(paths == vec!["/token"]);
        let binding = SchemaContractRepository::find(&mut tx, BindingOwner::Execution, execution)
            .await?
            .ok_or_else(|| anyhow::anyhow!("native CLI fixture has no bound input contract"))?;
        let action = &row.executable_snapshot.as_ref().unwrap().executable.action;
        let launch = action.launch.as_ref().unwrap();
        let plan = ArgumentPlan::from_bound_contract(
            action.param_schema.as_ref().unwrap(),
            launch.action_arguments()?,
            binding.input,
        )?;
        // The admitted schema/CLI metadata is public. It genuinely materializes
        // this argv until the canonical recorded secret paths are supplied.
        let public = plan.materialize(
            program.argument_budget_program()?,
            &parameters,
            &[],
            InputDelivery::Stdin,
            ArgumentLimits::default(),
        )?;
        anyhow::ensure!(public.argv() == ["--token", PRIVATE]);
        anyhow::ensure!(plan
            .materialize(
                program.argument_budget_program()?,
                &parameters,
                &paths,
                InputDelivery::Stdin,
                ArgumentLimits::default()
            )
            .is_err());
        source.spec.invocation.argv = vec![
            format!("{RUNNER_ROOT}/linux-amd64"),
            "exec".into(),
            program.sha256.clone(),
            program.path.to_str().unwrap().into(),
            program.invocation_name.clone(),
        ];
        source
            .spec
            .invocation
            .argv
            .extend(public.argv().iter().cloned());
        expiry.ensure_fresh_on(&mut tx).await?;
        tx.commit().await?;
        expiry.ensure_fresh()?;
        let input = SelectedExecutionInput::new(parameters, snapshot)?;
        anyhow::ensure!(matches!(
            Effects::reserve(
                db.pool(),
                &source.spec,
                source.fixture.credential.sponsor_identity_id,
                &input,
                MASTER
            )
            .await,
            Err(attune_common::Error::PermissionDenied(_))
        ));
        no_effect(&db, source.spec.intent).await?;
        Ok(())
    }
    .await;
    db.cleanup().await?;
    result
}

async fn seed_input_only_receipt() -> (
    TestDatabase,
    anyhow::Result<(Source, chrono::DateTime<Utc>)>,
) {
    use attune_common::repositories::{
        accepted_input::AcceptedInputRepository as Accepted,
        declared_input::DeclaredInputRepository,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        schema_contract::{BindingOwner, SchemaContractRepository},
    };
    let expression = "{{ keystore[parameters.selector] }}";
    let (db, source) = seed_with_launch(
        false,
        false,
        None,
        json!({
            "selector":{"type":"string"},"selected":{"type":"string","default":expression},
            "token":{"type":"string","secret":true}
        }),
        None,
    )
    .await;
    let result=async {
        let mut source=source?;
        let foreign=helpers::KeyFixture::new_system_unique("native_input_only",PRIVATE).create(&db).await?;
        let mut tx=db.begin().await?;
        let original=ExecutionRepository::find_by_id(&mut *tx,source.fixture.credential.execution_id).await?.unwrap();
        let snapshot=original.executable_snapshot.as_ref().unwrap();
        let binding=&source.fixture.policy.policy.invocations[0].binding;
        let authority=source.fixture.policy.policy.select(source.fixture.policy.id,binding,&[],source.fixture.admission.authority.attribution.clone())?;
        anyhow::ensure!(!authority.has_api_access());
        let LaunchTarget::Execution {worker,worker_instance,..}=source.spec.target else {unreachable!()};
        let execution=ExecutionRepository::create_pinned(&mut *tx,CreateExecutionInput {
            action:original.action,action_ref:original.action_ref.clone(),executor:Some(source.fixture.human_id),
            status:ExecutionStatus::Running,worker:Some(worker),timeout_seconds:Some(300),
            config:Some(json!({"selector":foreign.r#ref})),..Default::default()
        },snapshot).await?;
        let admission=RuntimeAuthorityRepository::record_admission(&mut tx,RuntimeOwner::Execution(execution.id),&authority).await?;
        let schema=SchemaContractRepository::materialize_action_pair(&mut tx,snapshot).await?;
        SchemaContractRepository::attach(&mut tx,BindingOwner::Execution,execution.id,&schema).await?;
        Captured::capture_human(&mut tx,"execution",execution.id,source.fixture.human_id).await?;
        let declarations=DeclaredInputRepository::bindings(&mut tx,execution.id).await?;
        let declaration=declarations.iter().find(|item|item.expression==expression).ok_or_else(||anyhow::anyhow!("missing genuine Key declaration"))?;
        let expiry=Utc::now()+Duration::seconds(30);
        let approval=Inputs::approve_external_input(&mut tx,execution.id,&declaration.declaration_revision,&declaration.source_path,expression,source.fixture.human_id,
            serde_json::from_value(json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[foreign.r#ref]}}]))?,Some(expiry)).await?;
        let input_only:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_input_only_approval WHERE approval=$1)").bind(approval).fetch_one(&mut *tx).await?;
        anyhow::ensure!(input_only);
        Accepted::seal_execution(&mut tx,execution.id,MASTER).await?;
        let mut session=Inputs::begin_render(&mut tx,execution.id,&declaration.declaration_revision,&declaration.source_path,expression,InputSitePurpose::Value).await?;
        Inputs::bind_public_parameter(&mut tx,&mut session,"/selector").await?;
        let rendered=Inputs::evaluate_with_keys(&mut tx,session,MASTER).await?;
        let site=InputSite {execution:execution.id,kind:InputEntityKind::ExecutionConfig,purpose:InputSitePurpose::Value,path:"/token".into()};
        let prepared=Inputs::prepare_destination(&mut tx,&rendered,&site,MASTER).await?;
        let receipt=Inputs::record_prepared(&mut tx,&rendered,&prepared,rendered.value(),rendered.selections(),MASTER).await?;
        set_secret(&mut tx,execution.id,&receipt.origin()).await?;
        sqlx::query("UPDATE execution SET config=$2,started_at=clock_timestamp() WHERE id=$1")
            .bind(execution.id).bind(json!({"selector":foreign.r#ref,"token":redaction_marker()})).execute(&mut *tx).await?;
        receipt.ensure_fresh_on(&mut tx).await?;
        tx.commit().await?;
        receipt.ensure_fresh()?;
        source.fixture.credential=RuntimeCredentialBinding {execution_id:execution.id,admission_id:admission.id,sponsor_identity_id:authority.owner.sponsor_identity_id,action_ref:original.action_ref,attempt:authority.attempt};
        source.fixture.admission=admission;
        source.spec.target=LaunchTarget::Execution {execution:execution.id,worker,worker_instance,attempt:authority.attempt};
        source.origin=receipt.origin();
        Ok((source,expiry))
    }.await;
    (db, result)
}

#[tokio::test]
async fn native_input_observed_operation_wait_rechecks_input_only_approval_expiry(
) -> anyhow::Result<()> {
    let (db, source) = seed_input_only_receipt().await;
    let result = async {
        let (source, expiry) = source?;
        // This is a genuine current credential binding except that API access
        // was never selected. Input-only consent must not turn either public
        // credential path into an API grant.
        let credential = &source.fixture.credential;
        let admission = &source.fixture.admission;
        anyhow::ensure!(!admission.authority.has_api_access());
        anyhow::ensure!(admission.id == credential.admission_id);
        anyhow::ensure!(admission.authority.owner.sponsor_identity_id == credential.sponsor_identity_id);
        anyhow::ensure!(admission.authority.binding.action_ref() == credential.action_ref);
        anyhow::ensure!(admission.authority.attempt == credential.attempt);
        let LaunchTarget::Execution {execution,worker,attempt,..} = source.spec.target else {unreachable!()};
        anyhow::ensure!(execution == credential.execution_id && attempt == credential.attempt);
        let actual = RuntimeAuthorityRepository::current_worker_attempt(db.pool().acquire().await?.as_mut(),execution,worker).await?;
        anyhow::ensure!(actual == credential.attempt);
        let mut tx = db.begin().await?;
        let denied = matches!(RuntimeAuthorityRepository::validate_execution_credential(&mut tx,credential).await,
            Err(attune_common::Error::PermissionDenied(message)) if message == "Execution credential does not match admitted authority");
        tx.rollback().await?;
        anyhow::ensure!(denied);
        let mut tx = db.begin().await?;
        let denied = matches!(RuntimeAuthorityRepository::begin_execution_attempt(&mut tx,execution,worker,attempt).await,
            Err(attune_common::Error::PermissionDenied(message)) if message == "Execution has no active runtime API admission");
        tx.rollback().await?;
        anyhow::ensure!(denied);
        observed_wait_expiry(&db, &source, expiry).await
    }
    .await;
    db.cleanup().await?;
    result
}
