//! Owner-scoped timing for shared graph transactions. This does not own or
//! complete SQL transactions. Only the owner can acknowledge their completion.

use std::{
    future::Future,
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

tokio::task_local! {
    // Scoped to the owner's future, including nested repository calls. This is
    // not a holder registry and is never inherited by spawned tasks.
    static SHARED_GRAPH_TRANSACTION: Arc<Mutex<State>>;
}

struct State {
    operation: &'static str,
    transaction_id: uuid::Uuid,
    backend: Option<i32>,
    started: Instant,
    last_event: Instant,
    stage: &'static str,
    begun: bool,
    first_requested: Option<Instant>,
    first_acquired: Option<Instant>,
    completion: &'static str,
    completed_at: Option<Instant>,
}

impl State {
    fn emit(&mut self, event: &'static str, result: &'static str) {
        let now = Instant::now();
        let end = self.completed_at.unwrap_or(now);
        tracing::debug!(
            target: "attune::graph_transaction",
            operation = self.operation,
            transaction_id = %self.transaction_id,
            backend = self.backend,
            mode = "shared",
            event,
            stage = self.stage,
            transaction_started = self.begun,
            transaction_us = micros(end.duration_since(self.started)),
            elapsed_since_event_us = micros(now.duration_since(self.last_event)),
            first_request_us = ?self.first_requested.map(|at| micros(at.duration_since(self.started))),
            first_acquired_us = ?self.first_acquired.map(|at| micros(at.duration_since(self.started))),
            admission_wait_us = ?self.first_acquired.zip(self.first_requested).map(|(acquired, requested)| micros(acquired.duration_since(requested))),
            hold_us = self.completed_at.zip(self.first_acquired).map(|(completed, acquired)| micros(completed.duration_since(acquired))),
            observed_hold_us = self.first_acquired.map(|at| micros(end.duration_since(at))),
            completion = self.completion,
            result,
            "Shared graph transaction"
        );
        self.last_event = now;
    }
}

fn micros(duration: std::time::Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Create once at the actual transaction owner, before `begin`, and scope its
/// future with `observe`. Use fixed operation/stage/result labels, never request
/// data. Correlation IDs exist only in debug traces, not metric labels.
/// Observe only work on this owner's transaction, not a separate transaction.
///
/// Call `committed` or `rolled_back` only after PostgreSQL acknowledges that
/// operation. Errors and cancellation leave an unknown outcome on drop; Rust
/// transaction destruction is not evidence of database lock release.
pub struct SharedGraphTransactionTrace {
    state: Arc<Mutex<State>>,
}

impl SharedGraphTransactionTrace {
    pub fn new(operation: &'static str) -> Self {
        let now = Instant::now();
        let mut state = State {
            operation,
            transaction_id: uuid::Uuid::new_v4(),
            backend: None,
            started: now,
            last_event: now,
            stage: "begin",
            begun: false,
            first_requested: None,
            first_acquired: None,
            completion: "pending",
            completed_at: None,
        };
        state.emit("begin_requested", "pending");
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub async fn observe<F: Future>(&self, future: F) -> F::Output {
        SHARED_GRAPH_TRANSACTION
            .scope(self.state.clone(), future)
            .await
    }

    pub fn begun(&self) {
        let mut state = lock(&self.state);
        state.begun = true;
        state.emit("begin_acknowledged", "ok");
    }

    /// Marks an owner continuation. The elapsed interval includes commands and
    /// scheduling/I/O gaps since the previous event; it is not a SQL-only timer.
    pub fn stage(&self, stage: &'static str) {
        let mut state = lock(&self.state);
        state.emit("stage_finished", "pending");
        state.stage = stage;
        state.emit("stage_started", "pending");
    }

    pub fn operation_result(&self, result: &'static str) {
        lock(&self.state).emit("operation_result", result);
    }

    pub fn committed(&self) {
        self.complete("committed");
    }

    pub fn rolled_back(&self) {
        self.complete("rolled_back");
    }

    fn complete(&self, completion: &'static str) {
        let mut state = lock(&self.state);
        state.completed_at = Some(Instant::now());
        state.completion = completion;
        state.emit("transaction_completed", "ok");
    }
}

impl Drop for SharedGraphTransactionTrace {
    fn drop(&mut self) {
        let mut state = lock(&self.state);
        if state.completion == "pending" {
            state.completion = "unknown";
            state.emit("owner_dropped", "unknown");
        }
    }
}

pub(crate) struct SharedGraphAdmissionRequest(Option<Arc<Mutex<State>>>);

/// Called immediately around the existing admission SQL, so nested authority
/// work cannot hide the first request/acquisition behind its total duration.
pub(crate) fn shared_graph_admission_requested() -> SharedGraphAdmissionRequest {
    let state = SHARED_GRAPH_TRANSACTION.try_with(Arc::clone).ok();
    if let Some(state) = &state {
        let mut state = lock(state);
        if state.first_requested.is_none() {
            state.first_requested = Some(Instant::now());
            state.emit("admission_requested", "pending");
        }
    }
    SharedGraphAdmissionRequest(state)
}

impl SharedGraphAdmissionRequest {
    pub(crate) fn finished(self, backend: Option<i32>) {
        if let Some(state) = self.0 {
            let mut state = lock(&state);
            if backend.is_none() {
                state.emit("admission_failed", "error");
            } else if state.first_acquired.is_none() {
                state.backend = backend;
                state.first_acquired = Some(Instant::now());
                state.emit("admission_acquired", "ok");
            } else {
                // PostgreSQL permits reentry, but this is still the same holder.
                state.emit("admission_reentered", "ok");
            }
        }
    }
}

#[cfg(test)]
mod tests;
