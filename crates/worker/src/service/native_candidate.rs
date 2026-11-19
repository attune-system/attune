//! The ordinary pack-test handler retains discovery, policy and install state.
//! Native process effects cannot release roots or publish a terminal install
//! until their own factual settlement and source-fenced transaction succeed.
use super::*;
use attune_common::{
    models::PackInstall,
    repositories::candidate_effect::{CandidateArchiveBinding, CandidateEffectRepository},
    test_executor::{CandidateTestEnvironment, TestExecutor},
};
use std::path::Path;

pub(super) async fn handle(
    pool: &PgPool,
    executor: &ActionExecutor,
    payload: &PackTestRequestedPayload,
    install: &PackInstall,
    packs_base_dir: &Path,
    api_url: &str,
    cancellation: CancellationToken,
) -> Result<()> {
    let denied = || Error::PermissionDenied("Native candidate admission unavailable".into());
    if install.pack_ref != payload.pack_ref
        || install.pack_version != payload.pack_version
        || install.trigger_reason != payload.trigger_reason
        || payload.candidate_path.is_none()
    {
        return Err(denied());
    }
    // Admission precedes the API download. No manager-side Pack sync, Python
    // preparation or lifecycle execution is reachable through this branch.
    let native = tokio::select! {
        value=executor.native_candidate_tests(
            payload.pack_install_id,
            payload.candidate_access_token.as_deref().ok_or_else(denied)?,
        )=>value?,
        _=cancellation.cancelled()=>return Err(Error::invalid_state("Native candidate preparation cancelled")),
    };
    let testing = native.testing();
    if !testing.enabled {
        // Preserve the ordinary handler's no-op policy without fabricating a
        // test result/history or opening an unused candidate API lease.
        native.settled().await?;
        let mut tx = pool.begin().await?;
        let fence = native.lock_result(&mut tx).await?;
        let status = if payload.trigger_reason == "manual" {
            PackInstallStatus::Succeeded
        } else {
            PackInstallStatus::Activating
        };
        PackInstallRepository::finish_running_on(&mut tx, install.id, status, None, None, None)
            .await?
            .ok_or_else(|| {
                Error::invalid_state("Native candidate install changed before publication")
            })?;
        fence.ensure_fresh(&mut tx).await?;
        tx.commit().await?;
        return Ok(());
    }
    let has_python = testing.runners.values().any(|runner| {
        runner.lifecycle.is_none() && matches!(runner.r#type.as_str(), "unittest" | "pytest")
    });
    if has_python
        && testing
            .runners
            .values()
            .any(|runner| runner.lifecycle.is_some())
    {
        // Combined preparation is enabled only after its fixed helper and
        // retained challenge admission are integrated together.
        return Err(Error::validation(
            "Native candidate recipe and Python composition is not yet available",
        ));
    }
    let mut tests =
        native.attach(TestExecutor::new(packs_base_dir.to_owned()).with_cancellation(cancellation));
    let lease = if testing.candidate_api_effects {
        let lease = CandidateEffectRepository::open(
            pool,
            install.id,
            install.assigned_worker_id.ok_or_else(denied)?,
            payload
                .candidate_access_token
                .as_deref()
                .ok_or_else(denied)?,
            &CandidateArchiveBinding {
                key: install.candidate_object_key.as_deref().ok_or_else(denied)?,
                provider_version: install
                    .candidate_provider_version
                    .as_deref()
                    .ok_or_else(denied)?,
                digest: install.candidate_digest.as_deref().ok_or_else(denied)?,
            },
        )
        .await?;
        tests = tests.with_candidate_effects(CandidateTestEnvironment {
            url: format!(
                "{}/api/v1/candidates/{}/effects",
                api_url.trim_end_matches('/'),
                lease.id,
            ),
            token: lease.token.clone(),
        });
        Some(lease)
    } else {
        None
    };
    let outcome = tests
        .execute_pack_tests_at_with_python_interpreter(
            native.pack_dir(),
            &payload.pack_ref,
            &payload.pack_version,
            testing,
            None,
        )
        .await;
    tests.process_owner().shutdown().await;
    native.settled().await?;
    // A known helper/preparation failure may terminate the install only after
    // factual settlement and the same live result fence as a parsed suite.
    // Unknown capture/removal fails above; revoked sources fail below.
    // Keep the existing owner/epoch/opt-in context locks through disclosure,
    // lease closure and terminal publication. Pool wrappers would use another
    // session and break this atomic boundary.
    let mut tx = pool.begin().await?;
    let fence = native.lock_result(&mut tx).await?;
    // Missing global preparation is a hard install failure even when suite
    // policy would accept cancellation or metadata errors without a process.
    // It cannot create history or an accepted result. Source failures remain
    // errors from lock_result above, rather than terminal install writes.
    let result = outcome
        .and_then(|result| {
            fence.require_result()?;
            Ok(result)
        })
        .ok();
    let result_json = result.as_ref().map(serde_json::to_value).transpose()?;
    if let Some(lease) = lease {
        CandidateEffectRepository::close_on(&mut tx, lease.id).await?;
    }
    let history = match (
        result.as_ref(),
        install.pack_id,
        PackRepository::find_by_ref(&mut *tx, &payload.pack_ref).await?,
    ) {
        (Some(result), Some(expected), Some(pack)) if expected == pack.id => Some(
            PackTestRepository::create_on(
                &mut tx,
                pack.id,
                &payload.pack_version,
                &payload.trigger_reason,
                result,
            )
            .await?
            .id,
        ),
        _ => None,
    };
    let (status, error) = match result.as_ref() {
        Some(result) if testing.accepts_result(result) => (
            if payload.trigger_reason == "manual" {
                PackInstallStatus::Succeeded
            } else {
                PackInstallStatus::Activating
            },
            None,
        ),
        Some(result) => (
            PackInstallStatus::Failed,
            Some(format!(
                "Pack tests did not pass ({}/{} passed)",
                result.passed, result.total_tests,
            )),
        ),
        None => (
            PackInstallStatus::Failed,
            Some("Native candidate preparation or test processing failed".into()),
        ),
    };
    PackInstallRepository::finish_running_on(
        &mut tx,
        install.id,
        status,
        history,
        result_json,
        error,
    )
    .await?
    .ok_or_else(|| Error::invalid_state("Native candidate install changed before publication"))?;
    fence.ensure_fresh(&mut tx).await?;
    tx.commit().await?;
    info!(
        pack_install_id = install.id,
        "Native candidate result committed"
    );
    Ok(())
}
