//! Bounded, execution-owned PostgreSQL protocol cancellation, never SQL signaling.
use crate::{Error, Result};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use std::{
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex as StdMutex, MutexGuard,
    },
    time::Duration,
};
use tokio::{
    sync::{oneshot, watch, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::Instant,
};

const LIVE: u8 = 0;
const COMMIT_PENDING: u8 = 1;
const ACKNOWLEDGED: u8 = 2;
fn cleanup_error() -> Error {
    Error::Internal("Reclamation cancellation cleanup is unavailable or unacknowledged".into())
}

struct State {
    failed: bool,
    closed: bool,
    jobs: Vec<JoinHandle<bool>>,
}
/// One startup-only control transport at a time per API replica. No control
/// PgPool, authenticated extra query session, global map, or detached Drop task.
pub struct CancellationOwner {
    slots: Arc<Semaphore>,
    shutdown: watch::Sender<bool>,
    state: StdMutex<State>,
}
impl CancellationOwner {
    /// Use the configured endpoint's standard PostgreSQL cancellation contract.
    /// The capability retains the actual established connection's options/key
    /// and peer, not a second caller-provided URL or a mandatory opt-in flag.
    pub fn new(_primary: &PgPool) -> Arc<Self> {
        let (shutdown, _) = watch::channel(false);
        Arc::new(Self {
            slots: Arc::new(Semaphore::new(1)),
            shutdown,
            state: StdMutex::new(State {
                failed: false,
                closed: false,
                jobs: Vec::new(),
            }),
        })
    }
    fn state(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.failed = true;
                state
            }
        }
    }
    pub fn begin_shutdown(&self) {
        self.state().closed = true;
        self.shutdown.send_replace(true);
    }
    pub async fn closed(&self) {
        let mut receiver = self.shutdown.subscribe();
        loop {
            if *receiver.borrow_and_update() {
                return;
            }
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
    fn unavailable(&self) -> bool {
        let state = self.state();
        state.closed || state.failed
    }
    async fn join_jobs(self: &Arc<Self>, deadline: Instant) -> Result<()> {
        loop {
            let handle = self.state().jobs.pop();
            let Some(handle) = handle else {
                break;
            };
            let mut pending = PendingJoin {
                owner: self.clone(),
                handle: Some(handle),
            };
            match tokio::time::timeout_at(deadline, pending.handle.as_mut().unwrap()).await {
                Ok(result) => {
                    pending.handle.take();
                    if !matches!(result, Ok(true)) {
                        self.state().failed = true;
                    }
                }
                Err(_) => return Err(cleanup_error()),
            }
        }
        if self.state().failed {
            Err(cleanup_error())
        } else {
            Ok(())
        }
    }
    pub async fn shutdown(self: &Arc<Self>, deadline: Instant) -> Result<()> {
        self.begin_shutdown();
        let permit = tokio::time::timeout_at(deadline, self.slots.clone().acquire_owned())
            .await
            .map_err(|_| cleanup_error())?
            .map_err(|_| cleanup_error())?;
        let result = self.join_jobs(deadline).await;
        drop(permit);
        result
    }
    pub(crate) async fn reserve(self: &Arc<Self>, deadline: Instant) -> Result<Reservation> {
        if self.unavailable() {
            return Err(cleanup_error());
        }
        let permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| cleanup_error())?;
        self.join_jobs(deadline).await?;
        if self.unavailable() {
            return Err(cleanup_error());
        }
        Ok(Reservation {
            owner: self.clone(),
            permit: Some(permit),
        })
    }
}
struct PendingJoin {
    owner: Arc<CancellationOwner>,
    handle: Option<JoinHandle<bool>>,
}
impl Drop for PendingJoin {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.owner.state().jobs.push(handle);
        }
    }
}

pub(crate) enum BindingError {
    Unavailable,
}
pub(crate) struct Reservation {
    owner: Arc<CancellationOwner>,
    permit: Option<OwnedSemaphorePermit>,
}
impl Reservation {
    /// Synchronous: caller marks actual PoolConnection close-on-drop first. No
    /// metadata query, BEGIN, or await exists between capability capture and
    /// registered monitor custody. No caller PID/options are accepted here.
    pub(crate) fn bind(
        mut self,
        primary: &PgConnection,
    ) -> std::result::Result<CancellationGuard, BindingError> {
        let token = primary
            .cancel_token()
            .map_err(|_| BindingError::Unavailable)?;
        let permit = self.permit.take().unwrap();
        let phase = Arc::new(AtomicU8::new(LIVE));
        let monitor_phase = phase.clone();
        let (signal, mut receive) = oneshot::channel::<()>();
        let (finished, completion) = oneshot::channel::<bool>();
        let owner = self.owner.clone();
        let mut state = owner.state();
        let monitor_owner = owner.clone();
        let handle = tokio::spawn(async move {
            let (acknowledged, await_owner_release) = tokio::select! {
                biased;
                result = &mut receive => (result.is_ok(), false),
                _ = monitor_owner.closed() => (false, true),
            };
            let mut success = true;
            if !acknowledged && monitor_phase.load(Ordering::Acquire) != ACKNOWLEDGED {
                // This deadline is APPLICATION-owned. Driver creates no task,
                // retry or timer. Dropping its future closes its one transport.
                let deadline = Instant::now() + Duration::from_secs(5);
                let sent = matches!(
                    tokio::time::timeout_at(deadline, token.cancel()).await,
                    Ok(Ok(()))
                );
                // An active handler/wrapper still owns the data socket. Idle
                // CancelRequest does not terminate it. Shutdown cannot pretend
                // its work is released until guard acknowledgement/Drop too.
                let released = !await_owner_release
                    || tokio::time::timeout_at(deadline, &mut receive)
                        .await
                        .is_ok();
                // Transport EOF is NOT backend-gone acknowledgement. The data
                // connection remains close-on-drop and the original exact
                // backend-gone acceptance assertions must still prove disposal.
                success =
                    sent && released && monitor_phase.load(Ordering::Acquire) != COMMIT_PENDING;
                if !success {
                    let reason = if !sent {
                        "cancel_transport_unconfirmed"
                    } else if !released {
                        "owner_not_released"
                    } else {
                        "commit_outcome_unknown"
                    };
                    tracing::error!(
                        component = "reclamation_cancellation",
                        reason,
                        "Reclamation cancellation failed or outcome unknown"
                    );
                }
            }
            if !success {
                monitor_owner.state().failed = true;
            }
            // Keep the only control permit through transport closure/Drop.
            drop(permit);
            let _ = finished.send(success);
            success
        });
        state.jobs.push(handle);
        drop(state);
        Ok(CancellationGuard {
            phase,
            signal: Some(signal),
            completion: Some(completion),
        })
    }
}
pub(crate) struct CancellationGuard {
    phase: Arc<AtomicU8>,
    signal: Option<oneshot::Sender<()>>,
    completion: Option<oneshot::Receiver<bool>>,
}
impl CancellationGuard {
    async fn acknowledged(mut self) -> Result<()> {
        self.phase.store(ACKNOWLEDGED, Ordering::Release);
        if let Some(signal) = self.signal.take() {
            let _ = signal.send(());
        }
        match self.completion.take().unwrap().await {
            Ok(true) => Ok(()),
            _ => Err(cleanup_error()),
        }
    }
}

pub struct ReclamationTransaction {
    transaction: Option<Transaction<'static, Postgres>>,
    guard: Option<CancellationGuard>,
}
#[derive(Debug, thiserror::Error)]
pub enum ReclamationEndError {
    #[error(transparent)]
    Database(#[from] Error),
    #[error("Owned reclamation cleanup was not acknowledged")]
    CleanupUnavailable,
    #[error("Reclamation commit outcome is unknown")]
    CommitUnknown,
}
impl From<ReclamationEndError> for Error {
    fn from(error: ReclamationEndError) -> Self {
        match error {
            ReclamationEndError::Database(error) => error,
            ReclamationEndError::CleanupUnavailable | ReclamationEndError::CommitUnknown => {
                cleanup_error()
            }
        }
    }
}
impl std::fmt::Debug for ReclamationTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReclamationTransaction")
            .finish_non_exhaustive()
    }
}
impl ReclamationTransaction {
    pub(crate) fn new(
        transaction: Transaction<'static, Postgres>,
        guard: CancellationGuard,
    ) -> Self {
        Self {
            transaction: Some(transaction),
            guard: Some(guard),
        }
    }
    pub fn transaction_mut(&mut self) -> &mut Transaction<'static, Postgres> {
        self.transaction.as_mut().unwrap()
    }
    pub async fn commit(mut self) -> std::result::Result<(), ReclamationEndError> {
        self.guard
            .as_ref()
            .unwrap()
            .phase
            .store(COMMIT_PENDING, Ordering::Release);
        if self.transaction.take().unwrap().commit().await.is_err() {
            // Conservative classification, without a second COMMIT/INSERT or
            // an inference that disposing the backend undoes committed work.
            return Err(ReclamationEndError::CommitUnknown);
        }
        self.guard
            .take()
            .unwrap()
            .acknowledged()
            .await
            .map_err(|_| ReclamationEndError::CleanupUnavailable)
    }
    pub async fn rollback(mut self) -> std::result::Result<(), ReclamationEndError> {
        self.transaction
            .take()
            .unwrap()
            .rollback()
            .await
            .map_err(Error::from)?;
        self.guard
            .take()
            .unwrap()
            .acknowledged()
            .await
            .map_err(|_| ReclamationEndError::CleanupUnavailable)
    }
}
impl Deref for ReclamationTransaction {
    type Target = PgConnection;
    fn deref(&self) -> &Self::Target {
        self.transaction.as_ref().unwrap()
    }
}
impl DerefMut for ReclamationTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.transaction.as_mut().unwrap()
    }
}
#[cfg(test)]
mod tests;
