//! Startup protection for secret-bearing service parents, not workload isolation.

/// Disable Linux core dumps and unprivileged ptrace/procfs inspection of this
/// process. Call only from production entrypoints, before loading credentials or
/// starting runtimes/pack code. Never call from shared service constructors or
/// an in-process test harness: this changes process-wide state.
///
/// Linux startup fails closed if setting or verifying protection fails. Other
/// platforms currently have no equivalent protection here. Privileged tracers
/// and other same-UID powers (including signals and shared files) remain outside
/// this mitigation. A subsequent credential change can reset Linux dumpability;
/// protected services must not change OS identity after this call.
pub fn protect_service_parent() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use anyhow::{ensure, Context};
        use rustix::process::{dumpable_behavior, set_dumpable_behavior, DumpableBehavior};

        set_dumpable_behavior(DumpableBehavior::NotDumpable)
            .context("required Linux service-parent protection could not be applied")?;
        ensure!(
            dumpable_behavior().context("could not verify Linux service-parent protection")?
                == DumpableBehavior::NotDumpable,
            "required Linux service-parent protection is not active"
        );
    }
    Ok(())
}
