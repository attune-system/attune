use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::{
    sync::oneshot,
    task::{AbortHandle, JoinSet},
};

#[derive(Clone, Default)]
pub(super) struct OutputTasks(Arc<Mutex<JoinSet<()>>>);

#[derive(Debug)]
pub(super) struct OutputTask {
    handle: AbortHandle,
    done: oneshot::Receiver<()>,
}

impl OutputTasks {
    pub(super) fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) -> OutputTask {
        let (finished, done) = oneshot::channel();
        let mut tasks = self.0.lock().expect("output task group poisoned");
        while tasks.try_join_next().is_some() {}
        let handle = tasks.spawn(async move {
            future.await;
            let _ = finished.send(());
        });
        OutputTask { handle, done }
    }

    pub(super) async fn shutdown(&self) {
        let mut tasks = std::mem::take(&mut *self.0.lock().expect("output task group poisoned"));
        tasks.shutdown().await;
    }
}

impl OutputTask {
    pub(super) fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    pub(super) async fn join(self) {
        let _ = self.done.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_drops_and_joins_pending_output_tasks() {
        struct NotifyDrop(Option<oneshot::Sender<()>>);
        impl Drop for NotifyDrop {
            fn drop(&mut self) {
                let _ = self.0.take().unwrap().send(());
            }
        }
        let tasks = OutputTasks::default();
        let (dropped, drop_received) = oneshot::channel();
        let guard = NotifyDrop(Some(dropped));
        let task = tasks.spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
        tasks.shutdown().await;
        assert!(task.is_finished());
        drop_received.await.unwrap();
        task.join().await;
    }
}
