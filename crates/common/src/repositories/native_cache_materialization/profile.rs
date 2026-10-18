//! Opt-in, task-scoped preparation counters. Fixed buckets, no source values,
//! SQL, paths, global caches, background tasks or locks across an await.
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

tokio::task_local! { static ACTIVE: NativeCachePreparationProfile; }

#[derive(Clone)]
pub struct NativeCachePreparationProfile(Arc<[Counters; 22]>);
impl Default for NativeCachePreparationProfile {
    fn default() -> Self {
        Self(Arc::new(std::array::from_fn(|_| Counters::default())))
    }
}
#[derive(Default)]
struct Counters {
    calls: AtomicU64,
    finished: AtomicU64,
    cancelled: AtomicU64,
    active: AtomicU64,
    micros: AtomicU64,
    max_micros: AtomicU64,
}
#[derive(Debug)]
pub struct NativeCachePreparationMetric {
    pub operation: &'static str,
    pub calls: u64,
    pub finished: u64,
    pub cancelled: u64,
    pub active: u64,
    /// Inclusive wall time, not CPU or SQL time. Nested buckets must not be summed.
    pub micros: u64,
    pub max_micros: u64,
}

#[derive(Clone, Copy)]
#[repr(usize)]
pub(in crate::repositories) enum Operation {
    TransitionPreparation,
    RootSources,
    ReuseSources,
    Expression,
    Literal,
    SelectorProof,
    ControlDestination,
    ControlRecord,
    NativeClosure,
    TaskAdmission,
    CachePage,
    ChildCreate,
    SchemaBind,
    ChildDestination,
    ChildDefaults,
    StagedValue,
    ConditionDecision,
    FrameBinding,
    ReceiptReopen,
    SessionBegin,
    CompletionSource,
    CompletionAccounting,
}
const OPERATIONS: [Operation; 22] = [
    Operation::TransitionPreparation,
    Operation::RootSources,
    Operation::ReuseSources,
    Operation::Expression,
    Operation::Literal,
    Operation::SelectorProof,
    Operation::ControlDestination,
    Operation::ControlRecord,
    Operation::NativeClosure,
    Operation::TaskAdmission,
    Operation::CachePage,
    Operation::ChildCreate,
    Operation::SchemaBind,
    Operation::ChildDestination,
    Operation::ChildDefaults,
    Operation::StagedValue,
    Operation::ConditionDecision,
    Operation::FrameBinding,
    Operation::ReceiptReopen,
    Operation::SessionBegin,
    Operation::CompletionSource,
    Operation::CompletionAccounting,
];
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::TransitionPreparation => "transition.prepare",
            Self::RootSources => "sources.prepare",
            Self::ReuseSources => "sources.reuse_check",
            Self::Expression => "canonical.expression",
            Self::Literal => "canonical.literal",
            Self::SelectorProof => "selector.proof",
            Self::ControlDestination => "control.destination",
            Self::ControlRecord => "control.record",
            Self::NativeClosure => "native.closure",
            Self::TaskAdmission => "task.admission",
            Self::CachePage => "cache.page",
            Self::ChildCreate => "child.create",
            Self::SchemaBind => "child.schema",
            Self::ChildDestination => "child.destination",
            Self::ChildDefaults => "child.defaults",
            Self::StagedValue => "publication.staged_proof",
            Self::ConditionDecision => "condition.decision_proof",
            Self::FrameBinding => "frame.bind",
            Self::ReceiptReopen => "receipt.source_reopen",
            Self::SessionBegin => "canonical.begin",
            Self::CompletionSource => "completion.prepare",
            Self::CompletionAccounting => "completion.accounting",
        }
    }
}
impl NativeCachePreparationProfile {
    pub async fn record<F: Future>(&self, future: F) -> F::Output {
        ACTIVE.scope(self.clone(), future).await
    }
    pub fn snapshot(&self) -> Vec<NativeCachePreparationMetric> {
        OPERATIONS
            .iter()
            .filter_map(|op| {
                let counters = &self.0[*op as usize];
                let calls = counters.calls.load(Ordering::Relaxed);
                (calls != 0).then(|| NativeCachePreparationMetric {
                    operation: op.name(),
                    calls,
                    finished: counters.finished.load(Ordering::Relaxed),
                    cancelled: counters.cancelled.load(Ordering::Relaxed),
                    active: counters.active.load(Ordering::Relaxed),
                    micros: counters.micros.load(Ordering::Relaxed),
                    max_micros: counters.max_micros.load(Ordering::Relaxed),
                })
            })
            .collect()
    }
}

struct Guard {
    active: Option<(NativeCachePreparationProfile, Operation, Instant)>,
    finished: bool,
}
impl Drop for Guard {
    fn drop(&mut self) {
        if let Some((profile, op, started)) = &self.active {
            let counters = &profile.0[*op as usize];
            let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            // Saturate on both the pinned Docker toolchain and current stable.
            let mut previous = counters.micros.load(Ordering::Relaxed);
            while let Err(current) = counters.micros.compare_exchange_weak(
                previous,
                previous.saturating_add(micros),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                previous = current;
            }
            counters.max_micros.fetch_max(micros, Ordering::Relaxed);
            if self.finished {
                counters.finished.fetch_add(1, Ordering::Relaxed);
            } else {
                counters.cancelled.fetch_add(1, Ordering::Relaxed);
            }
            counters.active.fetch_sub(1, Ordering::Relaxed);
        }
    }
}
pub(in crate::repositories) async fn measure<F: Future>(op: Operation, future: F) -> F::Output {
    let active = ACTIVE
        .try_with(|profile| {
            let counters = &profile.0[op as usize];
            counters.calls.fetch_add(1, Ordering::Relaxed);
            counters.active.fetch_add(1, Ordering::Relaxed);
            (profile.clone(), op, Instant::now())
        })
        .ok();
    let mut guard = Guard {
        active,
        finished: false,
    };
    let result = future.await;
    guard.finished = true;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancellation_is_counted_without_an_unbounded_event_log() {
        let profile = NativeCachePreparationProfile::default();
        let mut work = Box::pin(profile.record(measure(
            Operation::RootSources,
            std::future::pending::<()>(),
        )));
        assert!(futures::poll!(work.as_mut()).is_pending());
        drop(work);
        let snapshot = profile.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].calls, 1);
        assert_eq!(snapshot[0].cancelled, 1);
        assert_eq!(snapshot[0].active, 0);
        assert_eq!(snapshot[0].finished, 0);
    }
    #[tokio::test]
    async fn scopes_do_not_share_counters() {
        let first = NativeCachePreparationProfile::default();
        let second = NativeCachePreparationProfile::default();
        first.record(measure(Operation::Literal, async {})).await;
        assert!(second.snapshot().is_empty());
        assert_eq!(first.snapshot()[0].finished, 1);
    }
}
