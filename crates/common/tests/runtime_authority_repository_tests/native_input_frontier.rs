//! Owning canonical issuer fixtures. Native secret provider delivery stays
//! deliberately gated; these cases do not contact a provider or claim rollout.
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
    let (db, fixture) = fixture_with_approval_lifetime(
        Some(json!({"token":{"type":"string","secret":true}})),
        None,
        lifetime,
    )
    .await;
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

#[tokio::test]
async fn native_input_canonical_secret_is_allowed_but_provider_guard_stays_closed(
) -> anyhow::Result<()> {
    let (db, source) = seed(true, true, None).await;
    let result = async {
        let source = source?;
        let selected = selected(&db, &source).await?;
        anyhow::ensure!(matches!(
            Effects::reserve(
                db.pool(),
                &source.spec,
                source.fixture.credential.sponsor_identity_id,
                &selected,
                MASTER
            )
            .await,
            Err(attune_common::Error::PermissionDenied(_))
        ));
        let rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM execution_process_effect WHERE intent=$1")
                .bind(source.spec.intent)
                .fetch_one(db.pool())
                .await?;
        anyhow::ensure!(
            rows == 0,
            "unsupported secret delivery must not reserve a provider effect"
        );
        Ok(())
    }
    .await;
    let cleanup = db.cleanup().await;
    cleanup?;
    result
}

#[tokio::test]
async fn native_input_canonical_secret_without_captured_grants_is_denied() -> anyhow::Result<()> {
    let (db, source) = seed(true, false, None).await;
    let result = async {
        let source = source?;
        let mut tx = db.begin().await?;
        let denied = matches!(
            Input::restore(
                &mut tx,
                source.fixture.credential.execution_id,
                Some(MASTER),
                None
            )
            .await,
            Err(attune_common::Error::PermissionDenied(_))
        );
        tx.rollback().await?;
        anyhow::ensure!(denied);
        Ok(())
    }
    .await;
    let cleanup = db.cleanup().await;
    cleanup?;
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
        let (_, _, snapshot, fence) = input.into_parts();
        tx.commit().await?;
        fence.ensure_fresh()?;
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
    let (db, source) = seed(false, true, None).await;
    let result=async {
        let source=source?;
        let execution=source.fixture.credential.execution_id;
        let frozen=selected(&db,&source).await?;
        Effects::reserve(db.pool(),&source.spec,source.fixture.credential.sponsor_identity_id,&frozen,MASTER).await?;
        let mut tx=db.begin().await?;
        let (body,digest):(serde_json::Value,String)=sqlx::query_as("DELETE FROM recorded_execution_input_authority WHERE owner_kind='execution' AND owner_id=$1 RETURNING authority,authority_digest")
            .bind(execution).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        let denied=matches!(Effects::begin_create(db.pool(),source.spec.intent,MASTER).await,Err(attune_common::Error::PermissionDenied(_)));
        sqlx::query("INSERT INTO recorded_execution_input_authority(owner_kind,owner_id,authority,authority_digest) VALUES('execution',$1,$2,$3)")
            .bind(execution).bind(body).bind(digest).execute(db.pool()).await?;
        let _restored=selected(&db,&source).await?;
        let restored_config=true;
        let still_denied=matches!(Effects::begin_create(db.pool(),source.spec.intent,MASTER).await,Err(attune_common::Error::PermissionDenied(_)));
        anyhow::ensure!(denied && restored_config && still_denied);
        Ok(())
    }.await;
    let cleanup = db.cleanup().await;
    cleanup?;
    result
}

#[tokio::test]
async fn native_input_runtime_sponsor_freeze_restore_cannot_revive_reserved_effect(
) -> anyhow::Result<()> {
    let (db, source) = seed(false, true, None).await;
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
async fn native_input_selected_parent_origin_revoke_restore_advances_durable_epoch(
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
        let (_,_,snapshot,fence)=input.into_parts(); tx.commit().await?; fence.ensure_fresh()?;
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
        let mut tx=db.begin().await?; set_secret(&mut tx,parent.id,&source.origin).await?; tx.commit().await?;
        let mut tx=db.begin().await?;
        let input=Input::restore(&mut tx,execution,Some(MASTER),Some(&snapshot)).await?;
        let restored=input.value().clone(); let (_,_,_,fence)=input.into_parts(); tx.commit().await?; fence.ensure_fresh()?;
        let after:i64=sqlx::query_scalar("SELECT epoch FROM workload_source_epoch WHERE kind='input_secret_buffer' AND subject=$1")
            .bind(buffer).fetch_one(db.pool()).await?;
        anyhow::ensure!(denied && restored==json!({"token":PRIVATE}) && after>before);
        Ok(())
    }.await;
    let cleanup = db.cleanup().await;
    cleanup?;
    result
}

#[tokio::test]
async fn native_input_observed_native_operation_wait_rechecks_approval_expiry() -> anyhow::Result<()>
{
    let (db, source) = seed(false, true, Some(Duration::seconds(30))).await;
    let result=async {
        let source=source?;
        let expiry=source.fixture.approval.expires_at.unwrap();
        let frozen=selected(&db,&source).await?;
        Effects::reserve(db.pool(),&source.spec,source.fixture.credential.sponsor_identity_id,&frozen,MASTER).await?;
        Effects::begin_create(db.pool(),source.spec.intent,MASTER).await?;
        let resource=attune_common::isolated_process::ResourceIdentity::Kubernetes {
            cluster:"owned-fixture".into(),namespace:"owned-fixture".into(),
            pod_name:format!("owned-{}",source.spec.intent),pod_uid:uuid::Uuid::new_v4().to_string(),
        };
        Effects::record_resource(db.pool(),source.spec.intent,&resource).await?;
        let mut blocker=db.begin().await?;
        sqlx::query("SELECT intent FROM execution_process_effect WHERE intent=$1 FOR UPDATE").bind(source.spec.intent).fetch_one(&mut *blocker).await?;
        let pid:i32=sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *blocker).await?;
        let read=Effects::consume_stdin(db.pool(),source.spec.intent,&resource,MASTER);
        let release=async {
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
            let rollback=blocker.rollback().await;
            (observed,rollback)
        };
        let (read,(observed,rollback))=tokio::join!(read,release);
        rollback?;
        anyhow::ensure!(observed??,"must observe the actual unexpired operation wait before expiry");
        anyhow::ensure!(matches!(read,Err(attune_common::Error::PermissionDenied(message)) if message.contains("expired")));
        Ok(())
    }.await;
    let cleanup = db.cleanup().await;
    cleanup?;
    result
}
