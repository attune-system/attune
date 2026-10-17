//! Instance-owned resource admission and joined file-publication tasks.
//! Cancellation stops a local task, never certifies provider settlement.
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};
use tokio_util::{
    sync::CancellationToken,
    task::{task_tracker::TaskTrackerToken, TaskTracker},
};

const UNIT: u64 = 1024 * 1024;
#[derive(Clone)]
pub struct FileTransfers {
    closed: Arc<Mutex<bool>>,
    memory: Arc<Semaphore>,
    slots: Arc<Semaphore>,
    budget: u64,
    tasks: TaskTracker,
    cancel: CancellationToken,
}
pub(crate) struct FileMemoryLease {
    _memory: OwnedSemaphorePermit,
    _slot: OwnedSemaphorePermit,
}
pub(crate) struct FileUploadLease {
    _memory: FileMemoryLease,
    _task: TaskTrackerToken,
}
impl FileTransfers {
    pub fn new(budget: u64, concurrency: usize) -> Self {
        assert!(
            budget >= UNIT
                && budget % UNIT == 0
                && budget / UNIT <= u32::MAX as u64
                && (1..=1024).contains(&concurrency),
            "invalid file transfer resource configuration"
        );
        Self {
            closed: Arc::new(Mutex::new(false)),
            memory: Arc::new(Semaphore::new((budget / UNIT) as usize)),
            slots: Arc::new(Semaphore::new(concurrency)),
            budget,
            tasks: TaskTracker::new(),
            cancel: CancellationToken::new(),
        }
    }
    fn memory_lease(&self, plaintext: Option<u64>) -> Result<FileMemoryLease, String> {
        let bytes = match plaintext {
            Some(size) => attune_common::runtime_file_body::allocation_budget(size),
            None => Some(16 * UNIT),
        }
        .ok_or_else(|| {
            "File codec allocation accounting overflow; no buffer was allocated".to_string()
        })?;
        self.memory_lease_bytes(bytes)
    }
    fn memory_lease_bytes(&self, bytes: u64) -> Result<FileMemoryLease, String> {
        if bytes > self.budget {
            return Err(format!("Protected file needs {bytes} bytes of codec admission, above artifacts.protected_file_memory_budget_bytes={}; provision a larger API memory budget before retrying; the file size/authority was not changed",self.budget));
        }
        let units = u32::try_from(bytes.div_ceil(UNIT))
            .map_err(|_| "File resource accounting overflow".to_string())?;
        let slot = self.slots.clone().try_acquire_owned().map_err(|_| {
            "File transfer concurrency is exhausted or shutting down; retry later".to_string()
        })?;
        let memory = self
            .memory
            .clone()
            .try_acquire_many_owned(units)
            .map_err(|_| "Protected file memory budget is in use; retry later".to_string())?;
        Ok(FileMemoryLease {
            _memory: memory,
            _slot: slot,
        })
    }
    pub(crate) fn upload(&self, plaintext: Option<u64>) -> Result<FileUploadLease, String> {
        let closed = self.closed.lock().expect("file admission lock poisoned");
        if *closed {
            return Err("File publication is shutting down".into());
        }
        let memory = self.memory_lease(plaintext)?;
        // Count preparation as well as native I/O, before shutdown can observe
        // a closed/empty tracker and before any codec buffer is allocated.
        Ok(FileUploadLease {
            _memory: memory,
            _task: self.tasks.token(),
        })
    }
    /// One maintenance step fetches at most 16 persisted 1 MiB metadata items
    /// and one bounded management response at a time. No body bytes are loaded.
    /// Account for JSON decoding/copies as well as transport buffers.
    pub(crate) fn maintenance(&self) -> Result<FileUploadLease, String> {
        let closed = self.closed.lock().expect("file admission lock poisoned");
        if *closed {
            return Err("File maintenance is shutting down".into());
        }
        Ok(FileUploadLease {
            _memory: self.memory_lease_bytes(128 * UNIT)?,
            _task: self.tasks.token(),
        })
    }
    pub(crate) fn reader(&self, plaintext: u64) -> Result<FileMemoryLease, String> {
        let closed = self.closed.lock().expect("file admission lock poisoned");
        if *closed {
            return Err("File delivery is shutting down".into());
        }
        self.memory_lease(Some(plaintext))
    }
    pub(crate) fn spawn<F, T>(&self, lease: FileUploadLease, work: F) -> JoinHandle<Option<T>>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let cancel = self.cancel.clone();
        self.tasks.spawn(async move {
            let _lease = lease;
            tokio::select! {value=work=>Some(value),_=cancel.cancelled()=>None}
        })
    }
    pub fn begin_shutdown(&self) {
        let mut closed = self.closed.lock().expect("file admission lock poisoned");
        *closed = true;
        self.slots.close();
        self.memory.close();
        self.tasks.close();
    }
    /// Grace expiry cancels and JOINS local work; unresolved native operations
    /// stay durable for provider fencing. It does not release evidence pins.
    pub async fn drain(&self, grace: Duration) -> bool {
        self.begin_shutdown();
        if tokio::time::timeout(grace, self.tasks.wait()).await.is_ok() {
            return true;
        }
        self.cancel.cancel();
        self.tasks.wait().await;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn maintenance_uses_shared_resource_slots_and_shutdown_joins_it() {
        let transfers = FileTransfers::new(256 * UNIT, 1);
        let lease = transfers.maintenance().unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let task = transfers.spawn(lease, async move {
            entered_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        entered_rx.await.unwrap();
        drop(task);
        assert!(transfers.maintenance().is_err());
        assert!(transfers.upload(Some(1)).is_err());
        assert!(!transfers.drain(Duration::ZERO).await);
        assert_eq!(transfers.tasks.len(), 0);
        assert!(transfers.maintenance().is_err());
    }
    #[tokio::test]
    async fn socket_handle_drop_is_bounded_and_shutdown_joins_canceled_work() {
        let transfers = FileTransfers::new(128 * UNIT, 1);
        let lease = transfers.upload(Some(1)).unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let handle = transfers.spawn(lease, async move {
            entered_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        entered_rx.await.unwrap();
        drop(handle);
        assert!(
            transfers.upload(Some(1)).is_err(),
            "a disconnected caller cannot create unlimited detached work"
        );
        assert!(!transfers.drain(Duration::ZERO).await);
        assert_eq!(transfers.tasks.len(), 0);
        assert!(transfers.upload(Some(1)).is_err());
    }
    #[tokio::test]
    async fn memory_admission_precedes_buffers_and_successful_jobs_release_ownership() {
        let transfers = FileTransfers::new(64 * UNIT, 4);
        assert!(transfers.upload(Some(64 * UNIT)).is_err());
        let lease = transfers.upload(Some(1)).unwrap();
        assert_eq!(
            transfers.spawn(lease, async { 42 }).await.unwrap(),
            Some(42)
        );
        assert!(transfers.drain(Duration::from_secs(1)).await);
    }
}
