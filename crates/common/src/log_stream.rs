use std::sync::Arc;
use std::time::Duration;

use futures::Future;
use rand::Rng;
use tokio::sync::{mpsc, oneshot, watch, OwnedSemaphorePermit, Semaphore};
use tokio::task::AbortHandle;

use crate::artifact_transport::ArtifactFileTransport;
use crate::{Error, Result};

enum Command {
    Write {
        bytes: Vec<u8>,
        _permit: OwnedSemaphorePermit,
    },
    Seal {
        truncated: bool,
        reply: oneshot::Sender<Result<()>>,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct SegmentedLogConfig {
    pub initial_segment_bytes: usize,
    pub max_segment_bytes: usize,
    pub flush_interval_ms: u64,
    pub retry_max_attempts: u32,
    pub retry_attempt_timeout_ms: u64,
    pub retry_initial_backoff_ms: u64,
    pub retry_max_backoff_ms: u64,
    pub finalization_timeout_ms: u64,
}

#[derive(Debug, Clone, Copy)]
enum FlushReason {
    Size,
    Interval,
    Seal,
}

impl FlushReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Interval => "interval",
            Self::Seal => "seal",
        }
    }
}

pub struct SegmentedLogWriter {
    sender: mpsc::Sender<Command>,
    available_bytes: Arc<Semaphore>,
    failure: watch::Receiver<Option<Arc<str>>>,
    config: SegmentedLogConfig,
    task_abort: AbortHandle,
}

#[derive(Debug)]
pub struct SharedFileLogWriter {
    transport: Arc<dyn ArtifactFileTransport>,
    artifact_version: i64,
    file_path: String,
    failure: std::sync::Mutex<Option<Arc<str>>>,
}

impl SharedFileLogWriter {
    pub fn new(
        transport: Arc<dyn ArtifactFileTransport>,
        artifact_version: i64,
        file_path: String,
    ) -> Self {
        Self {
            transport,
            artifact_version,
            file_path,
            failure: std::sync::Mutex::new(None),
        }
    }

    pub async fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        if let Some(message) = self.failure.lock().unwrap().clone() {
            return Err(latched_shared_io_error(message));
        }
        let result = self
            .transport
            .append_log_file(&self.file_path, bytes)
            .await
            .map_err(|error| Arc::<str>::from(error.to_string()));
        match result {
            Ok(()) => Ok(()),
            Err(message) => {
                let mut failure = self.failure.lock().unwrap();
                let original = failure.get_or_insert(message).clone();
                Err(latched_shared_io_error(original))
            }
        }
    }

    pub async fn seal(self, truncated: bool) -> Result<()> {
        if let Some(message) = self.failure.lock().unwrap().clone() {
            return Err(Error::invalid_state(format!(
                "shared log writer failed: {message}"
            )));
        }
        self.transport
            .seal_log_stream(self.artifact_version, truncated)
            .await
    }
}

fn latched_shared_io_error(message: Arc<str>) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        format!("shared log writer failed: {message}"),
    )
}

impl std::fmt::Debug for SegmentedLogWriter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SegmentedLogWriter")
            .field("config", &self.config)
            .finish()
    }
}

impl SegmentedLogWriter {
    pub fn new(
        transport: Arc<dyn ArtifactFileTransport>,
        artifact_version: i64,
        config: SegmentedLogConfig,
    ) -> Result<Self> {
        if config.initial_segment_bytes == 0
            || config.max_segment_bytes == 0
            || config.flush_interval_ms == 0
            || config.retry_max_attempts == 0
            || config.retry_attempt_timeout_ms == 0
            || config.retry_initial_backoff_ms == 0
            || config.retry_max_backoff_ms == 0
            || config.finalization_timeout_ms == 0
        {
            return Err(Error::validation(
                "log segment sizes, flush interval, retry attempts, retry timeout, retry delays, and finalization timeout must be greater than zero",
            ));
        }
        if config.initial_segment_bytes > config.max_segment_bytes {
            return Err(Error::validation(
                "initial log segment size cannot exceed maximum segment size",
            ));
        }
        if config.retry_initial_backoff_ms > config.retry_max_backoff_ms {
            return Err(Error::validation(
                "initial log retry delay cannot exceed maximum retry delay",
            ));
        }
        let _: u32 = u32::try_from(config.max_segment_bytes).map_err(|_| {
            Error::validation("log segment byte limit must fit in a 32-bit semaphore")
        })?;
        config.maximum_retry_delay_ms()?;
        let available_bytes = Arc::new(Semaphore::new(config.max_segment_bytes));
        let (failure_sender, failure) = watch::channel(None);
        let (sender, mut receiver) = mpsc::channel::<Command>(1);
        let task = tokio::spawn(async move {
            let mut sequence = 0_i64;
            let mut segment_target_bytes = config.initial_segment_bytes;
            let mut buffer = Vec::with_capacity(config.initial_segment_bytes);
            let mut interval =
                tokio::time::interval(Duration::from_millis(config.flush_interval_ms));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            let mut failure: Option<Error> = None;
            let mut permits = Vec::new();

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if failure.is_none() && !buffer.is_empty() {
                            match flush(&transport, artifact_version, &mut sequence, &mut buffer, config, FlushReason::Interval, segment_target_bytes).await {
                                Ok(()) => {
                                    permits.clear();
                                    segment_target_bytes = config.initial_segment_bytes;
                                },
                                Err(error) => {
                                    failure_sender.send_replace(Some(Arc::from(error.to_string())));
                                    receiver.close();
                                    permits.clear();
                                    failure = Some(error);
                                }
                            }
                        }
                    }
                    command = receiver.recv() => match command {
                        Some(Command::Write { mut bytes, _permit: permit }) => {
                            while failure.is_none() && !bytes.is_empty() {
                                let available = segment_target_bytes.saturating_sub(buffer.len());
                                let take = available.min(bytes.len());
                                buffer.extend(bytes.drain(..take));
                                if buffer.len() == segment_target_bytes {
                                    if let Err(error) = flush(&transport, artifact_version, &mut sequence, &mut buffer, config, FlushReason::Size, segment_target_bytes).await {
                                        failure_sender.send_replace(Some(Arc::from(error.to_string())));
                                        receiver.close();
                                        permits.clear();
                                        failure = Some(error);
                                    } else {
                                        permits.clear();
                                        segment_target_bytes = segment_target_bytes
                                            .saturating_mul(2)
                                            .min(config.max_segment_bytes);
                                    }
                                }
                            }
                            if failure.is_none() && !buffer.is_empty() {
                                permits.push(permit);
                            }
                        }
                        Some(Command::Seal { truncated, reply }) => {
                            let result = if let Some(error) = failure.take() {
                                Err(error)
                            } else {
                                async {
                                    if !buffer.is_empty() {
                                        flush(&transport, artifact_version, &mut sequence, &mut buffer, config, FlushReason::Seal, segment_target_bytes).await?;
                                    }
                                    transport.seal_log_stream(artifact_version, truncated).await
                                }.await
                            };
                            let _ = reply.send(result);
                            break;
                        }
                        None => break,
                    }
                }
            }
        });

        tracing::info!(
            artifact_version,
            initial_segment_bytes = config.initial_segment_bytes,
            max_segment_bytes = config.max_segment_bytes,
            flush_interval_ms = config.flush_interval_ms,
            retry_max_attempts = config.retry_max_attempts,
            retry_attempt_timeout_ms = config.retry_attempt_timeout_ms,
            retry_initial_backoff_ms = config.retry_initial_backoff_ms,
            retry_max_backoff_ms = config.retry_max_backoff_ms,
            finalization_timeout_ms = config.finalization_timeout_ms,
            "Configured immutable log stream buffer"
        );
        Ok(Self {
            sender,
            available_bytes,
            failure,
            config,
            task_abort: task.abort_handle(),
        })
    }

    pub async fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut failure = self.failure.clone();
        for chunk in bytes.chunks(self.config.initial_segment_bytes) {
            if let Some(message) = failure.borrow().clone() {
                return Err(latched_io_error(message));
            }

            if self.available_bytes.available_permits() < chunk.len() {
                tracing::debug!(
                    requested_bytes = chunk.len(),
                    available_bytes = self.available_bytes.available_permits(),
                    max_segment_bytes = self.config.max_segment_bytes,
                    "Applying immutable log stream backpressure"
                );
            }
            let permit = tokio::select! {
                result = self.available_bytes.clone().acquire_many_owned(chunk.len() as u32) => {
                    result.map_err(|_| stopped_io_error())?
                }
                result = failure.changed() => {
                    result.map_err(|_| stopped_io_error())?;
                    return Err(latched_io_error(
                        failure.borrow().clone().expect("failure watch changed without a value"),
                    ));
                }
            };

            if let Some(message) = failure.borrow().clone() {
                return Err(latched_io_error(message));
            }
            self.sender
                .send(Command::Write {
                    bytes: chunk.to_vec(),
                    _permit: permit,
                })
                .await
                .map_err(|_| {
                    failure
                        .borrow()
                        .clone()
                        .map(latched_io_error)
                        .unwrap_or_else(stopped_io_error)
                })?;
        }
        Ok(())
    }

    pub async fn seal(self, truncated: bool) -> Result<()> {
        if let Some(message) = self.failure.borrow().clone() {
            return Err(Error::invalid_state(message.to_string()));
        }
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Command::Seal { truncated, reply })
            .await
            .map_err(|_| Error::invalid_state("log segment writer stopped before sealing"))?;
        response.await.map_err(|_| {
            Error::invalid_state("log segment writer stopped before confirming seal")
        })?
    }

    pub fn max_unflushed_bytes(&self) -> usize {
        self.config.max_segment_bytes
    }
    pub fn max_unflushed_milliseconds(&self) -> u64 {
        self.config.flush_interval_ms
    }

    pub fn finalization_timeout_ms(&self) -> u64 {
        self.config.finalization_timeout_ms
    }
}

impl Drop for SegmentedLogWriter {
    fn drop(&mut self) {
        self.task_abort.abort();
    }
}

fn stopped_io_error() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "log segment writer stopped")
}

fn latched_io_error(message: Arc<str>) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        format!("log segment writer failed: {message}"),
    )
}

async fn flush(
    transport: &Arc<dyn ArtifactFileTransport>,
    artifact_version: i64,
    sequence: &mut i64,
    buffer: &mut Vec<u8>,
    config: SegmentedLogConfig,
    reason: FlushReason,
    segment_target_bytes: usize,
) -> Result<()> {
    let bytes = std::mem::take(buffer);
    let attempts = retry_log_segment_commit(config, artifact_version, *sequence, || {
        transport.commit_log_segment(artifact_version, *sequence, &bytes)
    })
    .await?;
    tracing::debug!(
        artifact_version,
        sequence = *sequence,
        bytes = bytes.len(),
        reason = reason.as_str(),
        segment_target_bytes,
        attempts,
        "Committed immutable log segment"
    );
    *sequence += 1;
    Ok(())
}

impl SegmentedLogConfig {
    pub fn maximum_retry_delay_ms(self) -> Result<u64> {
        let mut total = self
            .retry_attempt_timeout_ms
            .checked_mul(u64::from(self.retry_max_attempts))
            .ok_or_else(|| Error::validation("aggregate log segment retry timeout is too large"))?;
        let mut backoff = self.retry_initial_backoff_ms;
        let mut remaining_backoffs = u64::from(self.retry_max_attempts.saturating_sub(1));
        while remaining_backoffs > 0 && backoff < self.retry_max_backoff_ms {
            total = total.checked_add(backoff).ok_or_else(|| {
                Error::validation("aggregate log segment retry delay is too large")
            })?;
            backoff = backoff.saturating_mul(2).min(self.retry_max_backoff_ms);
            remaining_backoffs -= 1;
        }
        let capped_backoffs = self
            .retry_max_backoff_ms
            .checked_mul(remaining_backoffs)
            .ok_or_else(|| Error::validation("aggregate log segment retry delay is too large"))?;
        total = total
            .checked_add(capped_backoffs)
            .ok_or_else(|| Error::validation("aggregate log segment retry delay is too large"))?;
        Ok(total)
    }
}

pub async fn retry_log_segment_commit<F, Fut>(
    config: SegmentedLogConfig,
    artifact_version: i64,
    sequence: i64,
    mut commit: F,
) -> Result<u32>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let mut attempt = 1_u32;
    loop {
        let result = log_segment_commit_attempt(config, commit()).await;
        match result {
            Ok(()) => return Ok(attempt),
            Err(error) if error.is_retryable_transport() && attempt < config.retry_max_attempts => {
                let delay_ms = log_segment_retry_delay_ms(config, attempt);
                tracing::warn!(
                    artifact_version,
                    sequence,
                    attempt,
                    next_attempt = attempt + 1,
                    delay_ms,
                    error = %error,
                    "Retrying immutable log segment commit"
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                attempt += 1;
            }
            Err(error) => {
                tracing::error!(
                    artifact_version,
                    sequence,
                    attempts = attempt,
                    retryable = error.is_retryable_transport(),
                    error = %error,
                    "Immutable log segment commit failed"
                );
                return Err(error);
            }
        }
    }
}

pub async fn log_segment_commit_attempt<Fut>(config: SegmentedLogConfig, commit: Fut) -> Result<()>
where
    Fut: Future<Output = Result<()>>,
{
    tokio::time::timeout(
        Duration::from_millis(config.retry_attempt_timeout_ms),
        commit,
    )
    .await
    .unwrap_or_else(|_| {
        Err(Error::retryable_transport(format!(
            "log segment commit attempt timed out after {}ms",
            config.retry_attempt_timeout_ms
        )))
    })
}

pub fn log_segment_retry_delay_ms(config: SegmentedLogConfig, attempt: u32) -> u64 {
    let exponent = attempt.saturating_sub(1).min(63);
    let uncapped = config
        .retry_initial_backoff_ms
        .saturating_mul(1_u64 << exponent);
    let capped = uncapped.min(config.retry_max_backoff_ms);
    rand::thread_rng().gen_range(capped.div_ceil(2)..=capped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tokio::sync::Notify;

    #[derive(Debug, Default)]
    struct RecordingTransport {
        segments: Mutex<Vec<(i64, i64, Vec<u8>)>>,
        seals: Mutex<Vec<(i64, bool)>>,
        fail_commits: bool,
        fail_appends: bool,
        commit_errors: Mutex<VecDeque<Error>>,
        attempts: Mutex<Vec<(i64, i64, Vec<u8>)>>,
    }

    #[derive(Debug, Default)]
    struct GatedTransport {
        commit_started: Notify,
        allow_commit: Notify,
    }

    #[derive(Debug, Default)]
    struct HangingTransport {
        attempts: AtomicUsize,
        active: AtomicUsize,
        started: Notify,
        bytes: Mutex<Vec<Vec<u8>>>,
    }

    struct ActiveAttempt<'a>(&'a AtomicUsize);

    impl Drop for ActiveAttempt<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl ArtifactFileTransport for HangingTransport {
        async fn write_file(&self, _: &str, _: &[u8], _: Option<&str>) -> Result<()> {
            unreachable!()
        }
        async fn file_exists(&self, _: &str) -> Result<bool> {
            unreachable!()
        }
        async fn file_size(&self, _: &str) -> Result<Option<u64>> {
            unreachable!()
        }
        async fn delete_file(&self, _: &str) -> Result<()> {
            unreachable!()
        }
        async fn commit_log_segment(&self, _: i64, _: i64, content: &[u8]) -> Result<()> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            self.active.fetch_add(1, Ordering::SeqCst);
            self.bytes.lock().unwrap().push(content.to_vec());
            let _active = ActiveAttempt(&self.active);
            self.started.notify_one();
            std::future::pending().await
        }
        async fn seal_log_stream(&self, _: i64, _: bool) -> Result<()> {
            Ok(())
        }
        async fn open_reader(
            &self,
            _: &str,
            _: u64,
        ) -> Result<crate::artifact_transport::BoxAsyncReader> {
            unreachable!()
        }
        fn transport_mode(&self) -> &'static str {
            "test"
        }
        fn base_dir(&self) -> &str {
            ""
        }
    }

    #[async_trait]
    impl ArtifactFileTransport for GatedTransport {
        async fn write_file(&self, _: &str, _: &[u8], _: Option<&str>) -> Result<()> {
            unreachable!()
        }
        async fn file_exists(&self, _: &str) -> Result<bool> {
            unreachable!()
        }
        async fn file_size(&self, _: &str) -> Result<Option<u64>> {
            unreachable!()
        }
        async fn delete_file(&self, _: &str) -> Result<()> {
            unreachable!()
        }
        async fn commit_log_segment(&self, _: i64, _: i64, _: &[u8]) -> Result<()> {
            self.commit_started.notify_one();
            self.allow_commit.notified().await;
            Ok(())
        }
        async fn seal_log_stream(&self, _: i64, _: bool) -> Result<()> {
            Ok(())
        }
        async fn open_reader(
            &self,
            _: &str,
            _: u64,
        ) -> Result<crate::artifact_transport::BoxAsyncReader> {
            unreachable!()
        }
        fn transport_mode(&self) -> &'static str {
            "test"
        }
        fn base_dir(&self) -> &str {
            ""
        }
    }

    #[async_trait]
    impl ArtifactFileTransport for RecordingTransport {
        async fn write_file(
            &self,
            _file_path: &str,
            _content: &[u8],
            _content_type: Option<&str>,
        ) -> Result<()> {
            unreachable!()
        }

        async fn append_log_file(&self, _: &str, _: &[u8]) -> Result<()> {
            if self.fail_appends {
                Err(Error::invalid_state("injected shared append failure"))
            } else {
                Ok(())
            }
        }

        async fn file_exists(&self, _file_path: &str) -> Result<bool> {
            unreachable!()
        }

        async fn file_size(&self, _file_path: &str) -> Result<Option<u64>> {
            unreachable!()
        }

        async fn delete_file(&self, _file_path: &str) -> Result<()> {
            unreachable!()
        }

        async fn commit_log_segment(
            &self,
            artifact_version: i64,
            sequence: i64,
            content: &[u8],
        ) -> Result<()> {
            self.attempts
                .lock()
                .unwrap()
                .push((artifact_version, sequence, content.to_vec()));
            if let Some(error) = self.commit_errors.lock().unwrap().pop_front() {
                return Err(error);
            }
            if self.fail_commits {
                return Err(Error::invalid_state("injected segment failure"));
            }
            self.segments
                .lock()
                .unwrap()
                .push((artifact_version, sequence, content.to_vec()));
            Ok(())
        }

        async fn seal_log_stream(&self, artifact_version: i64, truncated: bool) -> Result<()> {
            self.seals
                .lock()
                .unwrap()
                .push((artifact_version, truncated));
            Ok(())
        }

        async fn open_reader(
            &self,
            _file_path: &str,
            _offset: u64,
        ) -> Result<crate::artifact_transport::BoxAsyncReader> {
            Ok(Box::pin(tokio::io::empty()))
        }

        fn transport_mode(&self) -> &'static str {
            "test"
        }

        fn base_dir(&self) -> &str {
            Path::new("").to_str().unwrap()
        }
    }

    fn writer_config(initial: usize, max: usize, flush_interval_ms: u64) -> SegmentedLogConfig {
        SegmentedLogConfig {
            initial_segment_bytes: initial,
            max_segment_bytes: max,
            flush_interval_ms,
            retry_max_attempts: 3,
            retry_attempt_timeout_ms: 100,
            retry_initial_backoff_ms: 1,
            retry_max_backoff_ms: 2,
            finalization_timeout_ms: 100,
        }
    }

    #[tokio::test]
    async fn flushes_ordered_segments_at_the_byte_limit() {
        let transport = Arc::new(RecordingTransport::default());
        let writer =
            SegmentedLogWriter::new(transport.clone(), 42, writer_config(4, 4, 60_000)).unwrap();

        writer.write_all(b"abcdef").await.unwrap();
        writer.seal(true).await.unwrap();

        assert_eq!(
            *transport.segments.lock().unwrap(),
            vec![(42, 0, b"abcd".to_vec()), (42, 1, b"ef".to_vec())]
        );
        assert_eq!(*transport.seals.lock().unwrap(), vec![(42, true)]);
    }

    #[tokio::test]
    async fn flushes_partial_segments_at_the_time_limit() {
        let transport = Arc::new(RecordingTransport::default());
        let writer =
            SegmentedLogWriter::new(transport.clone(), 7, writer_config(1024, 1024, 10)).unwrap();

        writer.write_all(b"partial").await.unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;

        assert_eq!(
            *transport.segments.lock().unwrap(),
            vec![(7, 0, b"partial".to_vec())]
        );
        writer.seal(false).await.unwrap();
    }

    #[tokio::test]
    async fn reports_commit_failures_and_does_not_seal() {
        let transport = Arc::new(RecordingTransport {
            fail_commits: true,
            ..Default::default()
        });
        let writer =
            SegmentedLogWriter::new(transport.clone(), 9, writer_config(4, 4, 60_000)).unwrap();

        writer.write_all(b"fail").await.unwrap();
        tokio::task::yield_now().await;
        let error = writer.write_all(b"later").await.unwrap_err();
        assert!(error.to_string().contains("injected segment failure"));
        assert!(writer.seal(false).await.is_err());
        assert!(transport.seals.lock().unwrap().is_empty());
        assert_eq!(transport.attempts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn retries_the_same_sequence_and_bytes_before_continuing_in_order() {
        let transport = Arc::new(RecordingTransport {
            commit_errors: Mutex::new(VecDeque::from([
                Error::retryable_transport("temporary one"),
                Error::retryable_transport("temporary two"),
            ])),
            ..Default::default()
        });
        let writer =
            SegmentedLogWriter::new(transport.clone(), 15, writer_config(4, 4, 60_000)).unwrap();

        writer.write_all(b"abcdefgh").await.unwrap();
        writer.seal(false).await.unwrap();

        assert_eq!(
            *transport.attempts.lock().unwrap(),
            vec![
                (15, 0, b"abcd".to_vec()),
                (15, 0, b"abcd".to_vec()),
                (15, 0, b"abcd".to_vec()),
                (15, 1, b"efgh".to_vec()),
            ]
        );
        assert_eq!(
            *transport.segments.lock().unwrap(),
            vec![(15, 0, b"abcd".to_vec()), (15, 1, b"efgh".to_vec())]
        );
    }

    #[tokio::test]
    async fn grows_segments_for_sustained_output_up_to_the_configured_maximum() {
        let transport = Arc::new(RecordingTransport::default());
        let writer =
            SegmentedLogWriter::new(transport.clone(), 16, writer_config(2, 8, 60_000)).unwrap();

        writer.write_all(b"abcdefghijklmn").await.unwrap();
        writer.seal(false).await.unwrap();

        let sizes = transport
            .segments
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, bytes)| bytes.len())
            .collect::<Vec<_>>();
        assert_eq!(sizes, vec![2, 4, 8]);
    }

    #[tokio::test]
    async fn stops_after_the_configured_retry_attempt_bound() {
        let transport = Arc::new(RecordingTransport {
            commit_errors: Mutex::new(VecDeque::from([
                Error::retryable_transport("temporary one"),
                Error::retryable_transport("temporary two"),
                Error::retryable_transport("temporary three"),
            ])),
            ..Default::default()
        });
        let writer =
            SegmentedLogWriter::new(transport.clone(), 17, writer_config(4, 4, 60_000)).unwrap();

        writer.write_all(b"abcd").await.unwrap();
        tokio::task::yield_now().await;

        assert!(writer.seal(false).await.is_err());
        assert_eq!(transport.attempts.lock().unwrap().len(), 3);
        assert!(transport.seals.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn times_out_each_attempt_and_replays_identical_bytes() {
        let transport = Arc::new(HangingTransport::default());
        let mut config = writer_config(4, 4, 60_000);
        config.retry_max_attempts = 2;
        config.retry_attempt_timeout_ms = 10;
        let writer = SegmentedLogWriter::new(transport.clone(), 18, config).unwrap();

        writer.write_all(b"same").await.unwrap();
        let result = tokio::time::timeout(Duration::from_millis(100), writer.seal(false)).await;

        assert!(result.expect("bounded retry policy").is_err());
        assert_eq!(transport.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(transport.active.load(Ordering::SeqCst), 0);
        assert_eq!(*transport.bytes.lock().unwrap(), vec![b"same", b"same"]);
    }

    #[tokio::test]
    async fn dropping_writer_aborts_in_flight_commit() {
        let transport = Arc::new(HangingTransport::default());
        let mut config = writer_config(4, 4, 60_000);
        config.retry_attempt_timeout_ms = 60_000;
        let writer = SegmentedLogWriter::new(transport.clone(), 19, config).unwrap();

        writer.write_all(b"stop").await.unwrap();
        transport.started.notified().await;
        assert_eq!(transport.active.load(Ordering::SeqCst), 1);
        drop(writer);
        tokio::task::yield_now().await;

        assert_eq!(transport.active.load(Ordering::SeqCst), 0);
        assert_eq!(transport.attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn shared_writer_latches_append_failure_and_refuses_to_seal() {
        let transport = Arc::new(RecordingTransport {
            fail_appends: true,
            ..Default::default()
        });
        let writer = SharedFileLogWriter::new(transport.clone(), 12, "log.txt".to_string());

        let write_error = writer.write_all(b"partial").await.unwrap_err();
        assert!(write_error
            .to_string()
            .contains("injected shared append failure"));
        let seal_error = writer.seal(false).await.unwrap_err();
        assert!(seal_error
            .to_string()
            .contains("injected shared append failure"));
        assert!(transport.seals.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn large_write_backpressures_when_uncommitted_bytes_reach_the_limit() {
        let transport = Arc::new(GatedTransport::default());
        let writer = Arc::new(
            SegmentedLogWriter::new(transport.clone(), 10, writer_config(4, 4, 60_000)).unwrap(),
        );
        let commit_started = transport.commit_started.notified();
        let mut blocked_write = tokio::spawn({
            let writer = writer.clone();
            async move { writer.write_all(b"fullx").await }
        });

        commit_started.await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut blocked_write)
                .await
                .is_err()
        );
        transport.allow_commit.notify_one();
        blocked_write.await.unwrap().unwrap();

        transport.allow_commit.notify_one();
        Arc::try_unwrap(writer).unwrap().seal(false).await.unwrap();
    }

    #[tokio::test]
    async fn abrupt_drop_loses_only_the_uncommitted_window() {
        let transport = Arc::new(RecordingTransport::default());
        let writer =
            SegmentedLogWriter::new(transport.clone(), 11, writer_config(4, 4, 60_000)).unwrap();

        writer.write_all(b"abc").await.unwrap();
        drop(writer);
        tokio::task::yield_now().await;

        assert!(transport.segments.lock().unwrap().is_empty());
        assert!(transport.seals.lock().unwrap().is_empty());
    }
}
