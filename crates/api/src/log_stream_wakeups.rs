use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::watch;

#[derive(Debug)]
struct StreamSignal {
    generation: watch::Sender<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct LogStreamWakeups {
    streams: Arc<Mutex<HashMap<i64, Weak<StreamSignal>>>>,
}

#[derive(Debug)]
pub struct LogStreamSubscription {
    _signal: Arc<StreamSignal>,
    receiver: watch::Receiver<u64>,
}

impl LogStreamWakeups {
    pub fn subscribe(&self, stream_id: i64) -> LogStreamSubscription {
        let mut streams = self.streams.lock().unwrap();
        let signal = streams
            .get(&stream_id)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let (generation, _) = watch::channel(0);
                let signal = Arc::new(StreamSignal { generation });
                streams.insert(stream_id, Arc::downgrade(&signal));
                signal
            });
        let receiver = signal.generation.subscribe();
        LogStreamSubscription {
            _signal: signal,
            receiver,
        }
    }

    pub fn wake(&self, stream_id: i64) -> bool {
        let signal = {
            let mut streams = self.streams.lock().unwrap();
            match streams.get(&stream_id).and_then(Weak::upgrade) {
                Some(signal) => signal,
                None => {
                    streams.remove(&stream_id);
                    return false;
                }
            }
        };
        signal
            .generation
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        true
    }

    pub fn wake_all(&self) -> usize {
        let signals = {
            let mut streams = self.streams.lock().unwrap();
            let mut signals = Vec::with_capacity(streams.len());
            streams.retain(|_, signal| {
                if let Some(signal) = signal.upgrade() {
                    signals.push(signal);
                    true
                } else {
                    false
                }
            });
            signals
        };
        for signal in &signals {
            signal
                .generation
                .send_modify(|generation| *generation = generation.wrapping_add(1));
        }
        signals.len()
    }
}

impl LogStreamSubscription {
    pub async fn wait(&mut self, reconciliation_timeout: Duration) -> bool {
        tokio::time::timeout(reconciliation_timeout, self.receiver.changed())
            .await
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn notification_wakes_only_interested_local_readers() {
        let wakeups = LogStreamWakeups::default();
        let mut interested = wakeups.subscribe(7);
        let mut unrelated = wakeups.subscribe(8);

        assert!(wakeups.wake(7));
        assert!(interested.wait(Duration::from_secs(1)).await);

        let unrelated_wait =
            tokio::spawn(async move { unrelated.wait(Duration::from_secs(1)).await });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(!unrelated_wait.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn missed_notification_falls_back_to_bounded_reconciliation() {
        let wakeups = LogStreamWakeups::default();
        let mut subscription = wakeups.subscribe(7);
        let wait = tokio::spawn(async move { subscription.wait(Duration::from_secs(15)).await });
        tokio::task::yield_now().await;

        tokio::time::advance(Duration::from_secs(15)).await;

        assert!(!wait.await.unwrap());
    }

    #[tokio::test]
    async fn subscribe_before_read_closes_the_notification_race() {
        let wakeups = LogStreamWakeups::default();
        let mut subscription = wakeups.subscribe(7);

        assert!(wakeups.wake(7));
        assert!(subscription.wait(Duration::from_secs(1)).await);
    }

    #[tokio::test]
    async fn reconnect_wakes_all_active_readers_and_coalesces_broadcast_lag() {
        let wakeups = LogStreamWakeups::default();
        let mut first = wakeups.subscribe(7);
        let mut second = wakeups.subscribe(8);

        for _ in 0..2_000 {
            assert!(wakeups.wake(7));
        }
        assert_eq!(wakeups.wake_all(), 2);

        assert!(first.wait(Duration::from_secs(1)).await);
        assert!(second.wait(Duration::from_secs(1)).await);
    }

    #[test]
    fn inactive_streams_are_not_fanned_out() {
        let wakeups = LogStreamWakeups::default();
        drop(wakeups.subscribe(7));

        assert!(!wakeups.wake(7));
        assert_eq!(wakeups.wake_all(), 0);
    }
}
