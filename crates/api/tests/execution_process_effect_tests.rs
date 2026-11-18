//! Real repository boundaries for the trusted in-process callsink. No host
//! enrollment/controller principal or provider service is manufactured here.
mod helpers;
use attune_common::{
    isolated_process::{ProcessSpec, ResourceIdentity},
    repositories::{
        delegated_execution::DelegatedExecutionRepository,
        execution::ExecutionRepository,
        execution_input::{ExecutionInputRepository, SelectedExecutionInput},
        execution_process::ExecutionProcessRepository as Effects,
        FindById,
    },
    workload_launch::{
        LaunchInvocation, LaunchPlatform, LaunchPurpose, LaunchRoot, LaunchTarget, PackAccess,
        PinnedImage, Sha256Digest,
    },
};
use helpers::{Result, TestContext};
use serde_json::json;
use uuid::Uuid;

async fn admitted(
    ctx: &TestContext,
) -> Result<(ProcessSpec, SelectedExecutionInput, ResourceIdentity)> {
    let owner = ctx.user.as_ref().unwrap().id;
    let token = helpers::delegated_source::token(ctx, owner, &["standard".into()]).await?;
    let claims = attune_common::auth::jwt::validate_token(&token, &ctx.state.jwt_config)?;
    let execution = claims.metadata.unwrap()["execution_id"].as_i64().unwrap();
    let row = ExecutionRepository::find_by_id(&ctx.pool, execution)
        .await?
        .unwrap();
    let worker = row.worker.unwrap();
    let instance = Uuid::new_v4();
    sqlx::query("UPDATE worker SET meta=COALESCE(meta,'{}'::jsonb)||jsonb_build_object('registration_incarnation',$2::text) WHERE id=$1")
        .bind(worker).bind(instance.to_string()).execute(&ctx.pool).await?;
    sqlx::query("UPDATE execution SET timeout_seconds=300,config=$2 WHERE id=$1")
        .bind(execution)
        .bind(json!({"value":"owned-private-parameters"}))
        .execute(&ctx.pool)
        .await?;
    let mut tx = ctx.pool.begin().await?;
    let attempt = DelegatedExecutionRepository::current_attempt(&mut tx, execution).await?;
    let input = ExecutionInputRepository::restore(
        &mut tx,
        execution,
        ctx.state.config.security.encryption_key.as_deref(),
        None,
    )
    .await?;
    let (value, _, snapshot, expiry) = input.into_parts();
    tx.commit().await?;
    expiry.ensure_fresh()?;
    let image = Sha256Digest::of_bytes(b"owned-process-image");
    Ok((
        ProcessSpec {
            intent: Uuid::new_v4(),
            purpose: LaunchPurpose::Action,
            target: LaunchTarget::Execution {
                execution,
                worker,
                worker_instance: instance,
                attempt,
            },
            image: PinnedImage {
                reference: format!("sha256:{}", image.as_str()),
                sha256: image,
            },
            platform: LaunchPlatform::LinuxAmd64,
            inputs: Vec::new(),
            pack_access: PackAccess::ReadOnly,
            stage: None,
            timeout_ms: Some(300_000),
            invocation: LaunchInvocation {
                argv: vec!["/opt/attune/pack/owned".into()],
                env: [(
                    "OWNED_PRIVATE_ENV".into(),
                    "owned-private-environment".into(),
                )]
                .into(),
                working_root: LaunchRoot::Pack,
            },
        },
        SelectedExecutionInput::new(value, snapshot)?,
        ResourceIdentity::Kubernetes {
            cluster: "owned-cluster".into(),
            namespace: "owned-namespace".into(),
            pod_name: format!("owned-{}", Uuid::new_v4().simple()),
            pod_uid: Uuid::new_v4().to_string(),
        },
    ))
}

#[tokio::test]
async fn exact_process_input_is_encrypted_and_consumed_once_across_recovery() -> Result<()> {
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let (spec, selected, resource) = admitted(&ctx).await?;
    let owner = ctx.user.as_ref().unwrap().id;
    let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
    Effects::reserve(&ctx.pool, &spec, owner, &selected, master).await?;
    Effects::record_resource(&ctx.pool, spec.intent, &resource).await?;
    let ciphertext: String = sqlx::query_scalar(
        "SELECT encrypted_context FROM execution_process_effect WHERE intent=$1",
    )
    .bind(spec.intent)
    .fetch_one(&ctx.pool)
    .await?;
    assert!(!ciphertext.contains("owned-private-parameters"));
    assert!(!ciphertext.contains("owned-private-environment"));
    let input = Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master).await?;
    assert_eq!(input.as_bytes(), br#"{"value":"owned-private-parameters"}"#);
    // A different connection/recovered caller cannot interpret a lost ACK as a
    // fresh write, even though the original immutable plan remains available.
    let retained = Effects::retained_spec(&ctx.pool, spec.intent, master).await?;
    assert_eq!(retained.intent, spec.intent);
    assert!(
        Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master)
            .await
            .is_err()
    );
    assert!(Effects::reserve(&ctx.pool, &spec, owner, &selected, master)
        .await
        .is_err());
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn process_gate_rejects_changed_pod_uid_stop_and_revoke_restore() -> Result<()> {
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let owner = ctx.user.as_ref().unwrap().id;
    let (spec, selected, resource) = admitted(&ctx).await?;
    Effects::reserve(&ctx.pool, &spec, owner, &selected, master).await?;
    Effects::record_resource(&ctx.pool, spec.intent, &resource).await?;
    let mut changed = resource.clone();
    if let ResourceIdentity::Kubernetes { pod_uid, .. } = &mut changed {
        *pod_uid = Uuid::new_v4().to_string();
    }
    assert!(Effects::record_resource(&ctx.pool, spec.intent, &changed)
        .await
        .is_err());
    assert!(
        Effects::consume_stdin(&ctx.pool, spec.intent, &changed, master)
            .await
            .is_err()
    );
    sqlx::query("UPDATE identity SET frozen=TRUE WHERE id=$1")
        .bind(owner)
        .execute(&ctx.pool)
        .await?;
    sqlx::query("UPDATE identity SET frozen=FALSE WHERE id=$1")
        .bind(owner)
        .execute(&ctx.pool)
        .await?;
    assert!(
        Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master)
            .await
            .is_err()
    );
    // Restoration permits a separate newly admitted operation, never revival.
    let (new, selected, new_resource) = admitted(&ctx).await?;
    Effects::reserve(&ctx.pool, &new, owner, &selected, master).await?;
    Effects::record_resource(&ctx.pool, new.intent, &new_resource).await?;
    Effects::request_stop(&ctx.pool, new.intent).await?;
    assert!(
        Effects::consume_stdin(&ctx.pool, new.intent, &new_resource, master)
            .await
            .is_err()
    );
    let (fresh, selected, fresh_resource) = admitted(&ctx).await?;
    Effects::reserve(&ctx.pool, &fresh, owner, &selected, master).await?;
    Effects::record_resource(&ctx.pool, fresh.intent, &fresh_resource).await?;
    assert!(
        Effects::consume_stdin(&ctx.pool, fresh.intent, &fresh_resource, master)
            .await
            .is_ok()
    );
    ctx.cleanup().await?;
    Ok(())
}
