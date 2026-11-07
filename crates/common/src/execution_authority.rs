//! Coordinated readiness for recorded execution authority.

use crate::{Error, Result};

/// Advertised by compatible worker startup; never authored by a pack.
pub const DELEGATED_EXECUTION_CAPABILITY: &str = "delegated_execution_authority_v1";

/// Supplied by coordinated service startup, not pack metadata or environment.
/// Default is deliberately not ready. Do not enable individual consumers by
/// treating a denied operation as a reason to issue an older broad credential.
#[derive(Clone, Copy, Default)]
pub struct RecordedConsumerReadiness {
    pub execution_admission: bool,
    pub api_consumers: bool,
    pub artifact_disclosure: bool,
    pub runtime_read_provenance: bool,
    pub notifier_delivery: bool,
    pub worker_completion: bool,
    pub native_executor_consumers: bool,
    pub candidate_isolation: bool,
}

impl RecordedConsumerReadiness {
    pub fn require_ready(self) -> Result<()> {
        if self.execution_admission
            && self.api_consumers
            && self.artifact_disclosure
            && self.runtime_read_provenance
            && self.notifier_delivery
            && self.worker_completion
            && self.native_executor_consumers
            && self.candidate_isolation
        {
            Ok(())
        } else {
            Err(Error::PermissionDenied(
                "Recorded execution credentials require coordinated admission, consumers and completion readiness".into(),
            ))
        }
    }
}

#[cfg(test)]
mod authority_consistency_tests {
    use super::RecordedConsumerReadiness;

    #[test]
    fn recorded_readiness_requires_every_coordinated_consumer() {
        assert!(RecordedConsumerReadiness::default()
            .require_ready()
            .is_err());
        for flags in 0u16..256 {
            let readiness = RecordedConsumerReadiness {
                execution_admission: flags & 1 != 0,
                api_consumers: flags & 2 != 0,
                artifact_disclosure: flags & 4 != 0,
                runtime_read_provenance: flags & 8 != 0,
                notifier_delivery: flags & 16 != 0,
                worker_completion: flags & 32 != 0,
                native_executor_consumers: flags & 64 != 0,
                candidate_isolation: flags & 128 != 0,
            };
            assert_eq!(readiness.require_ready().is_ok(), flags == 255);
        }
    }
}
