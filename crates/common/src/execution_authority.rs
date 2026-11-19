//! Coordinated readiness for recorded execution authority.

use crate::{Error, Result};

/// Advertised by compatible worker startup; never authored by a pack.
pub const DELEGATED_EXECUTION_CAPABILITY: &str = "delegated_execution_authority_v1";

/// Supplied by coordinated service startup, not pack metadata or environment.
/// Default is deliberately not ready. Do not enable individual consumers by
/// treating a denied operation as a reason to issue an older broad credential.
#[derive(Debug, Clone, Copy, Default)]
pub struct RecordedConsumerReadiness {
    pub execution_admission: bool,
    pub api_consumers: bool,
    pub artifact_disclosure: bool,
    pub runtime_read_provenance: bool,
    pub notifier_delivery: bool,
    pub worker_completion: bool,
    pub native_executor_consumers: bool,
}

impl RecordedConsumerReadiness {
    /// Report every missing prerequisite so an operator can distinguish an
    /// unavailable consumer from an invalid invocation. This is diagnostic only:
    /// observing a blocker never changes the admission decision.
    pub fn missing_consumers(self) -> Vec<&'static str> {
        [
            (self.execution_admission, "execution_admission"),
            (self.api_consumers, "api_consumers"),
            (self.artifact_disclosure, "artifact_disclosure"),
            (self.runtime_read_provenance, "runtime_read_provenance"),
            (self.notifier_delivery, "notifier_delivery"),
            (self.worker_completion, "worker_completion"),
            (self.native_executor_consumers, "native_executor_consumers"),
        ]
        .into_iter()
        .filter_map(|(ready, consumer)| (!ready).then_some(consumer))
        .collect()
    }

    pub fn require_ready(self) -> Result<()> {
        let missing = self.missing_consumers();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(Error::PermissionDenied(format!(
                "Recorded execution credentials require coordinated admission, consumers and completion readiness; missing: {}",
                missing.join(", "),
            )))
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
        for flags in 0u16..128 {
            let readiness = RecordedConsumerReadiness {
                execution_admission: flags & 1 != 0,
                api_consumers: flags & 2 != 0,
                artifact_disclosure: flags & 4 != 0,
                runtime_read_provenance: flags & 8 != 0,
                notifier_delivery: flags & 16 != 0,
                worker_completion: flags & 32 != 0,
                native_executor_consumers: flags & 64 != 0,
            };
            let missing = readiness.missing_consumers();
            assert_eq!(missing.len(), 7 - flags.count_ones() as usize);
            assert_eq!(readiness.require_ready().is_ok(), flags == 127);
            if let Err(error) = readiness.require_ready() {
                for consumer in missing {
                    assert!(error.to_string().contains(consumer));
                }
            }
        }
    }
}
