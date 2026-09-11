use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch, OwnedSemaphorePermit, Semaphore};

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

pub struct SegmentedLogWriter {
    sender: mpsc::Sender<Command>,
    available_bytes: Arc<Semaphore>,
    failure: watch::Receiver<Option<Arc<str>>>,
    max_unflushed_bytes: usize,
    max_unflushed_milliseconds: u64,
}

impl std::fmt::Debug for SegmentedLogWriter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SegmentedLogWriter")
            .field("max_unflushed_bytes", &self.max_unflushed_bytes)
            .field(
                "max_unflushed_milliseconds",
                &self.max_unflushed_milliseconds,
            )
            .finish()
    }
}

impl SegmentedLogWriter {
    pub fn new(
        transport: Arc<dyn ArtifactFileTransport>,
        artifact_version: i64,
        max_unflushed_bytes: usize,
        max_unflushed_milliseconds: u64,
    ) -> Result<Self> {
        if max_unflushed_bytes == 0 || max_unflushed_milliseconds == 0 {
            return Err(Error::validation(
                "log flush limits must be greater than zero",
            ));
        }
        let _: u32 = u32::try_from(max_unflushed_bytes).map_err(|_| {
            Error::validation("log segment byte limit must fit in a 32-bit semaphore")
        })?;
        let available_bytes = Arc::new(Semaphore::new(max_unflushed_bytes));
        let (failure_sender, failure) = watch::channel(None);
        let (sender, mut receiver) = mpsc::channel::<Command>(1);
        tokio::spawn(async move {
            let mut sequence = 0_i64;
            let mut buffer = Vec::with_capacity(max_unflushed_bytes);
            let mut interval =
                tokio::time::interval(Duration::from_millis(max_unflushed_milliseconds));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            let mut failure: Option<Error> = None;
            let mut permits = Vec::new();

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if failure.is_none() && !buffer.is_empty() {
                            match flush(&transport, artifact_version, &mut sequence, &mut buffer).await {
                                Ok(()) => permits.clear(),
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
                            permits.push(permit);
                            while failure.is_none() && !bytes.is_empty() {
                                let available = max_unflushed_bytes.saturating_sub(buffer.len());
                                let take = available.min(bytes.len());
                                buffer.extend(bytes.drain(..take));
                                if buffer.len() == max_unflushed_bytes {
                                    if let Err(error) = flush(&transport, artifact_version, &mut sequence, &mut buffer).await {
                                        failure_sender.send_replace(Some(Arc::from(error.to_string())));
                                        receiver.close();
                                        permits.clear();
                                        failure = Some(error);
                                    } else {
                                        permits.clear();
                                    }
                                }
                            }
                        }
                        Some(Command::Seal { truncated, reply }) => {
                            let result = if let Some(error) = failure.take() {
                                Err(error)
                            } else {
                                async {
                                    if !buffer.is_empty() {
                                        flush(&transport, artifact_version, &mut sequence, &mut buffer).await?;
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
            max_unflushed_bytes,
            max_unflushed_milliseconds,
            "Configured immutable log stream buffer"
        );
        Ok(Self {
            sender,
            available_bytes,
            failure,
            max_unflushed_bytes,
            max_unflushed_milliseconds,
        })
    }

    pub async fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut failure = self.failure.clone();
        for chunk in bytes.chunks(self.max_unflushed_bytes) {
            if let Some(message) = failure.borrow().clone() {
                return Err(latched_io_error(message));
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
        self.max_unflushed_bytes
    }
    pub fn max_unflushed_milliseconds(&self) -> u64 {
        self.max_unflushed_milliseconds
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
) -> Result<()> {
    let bytes = std::mem::take(buffer);
    transport
        .commit_log_segment(artifact_version, *sequence, &bytes)
        .await?;
    tracing::debug!(
        artifact_version,
        sequence = *sequence,
        bytes = bytes.len(),
        "Committed immutable log segment"
    );
    *sequence += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::path::Path;
    use std::sync::Mutex;
    use tokio::sync::Notify;

    #[derive(Debug, Default)]
    struct RecordingTransport {
        segments: Mutex<Vec<(i64, i64, Vec<u8>)>>,
        seals: Mutex<Vec<(i64, bool)>>,
        fail_commits: bool,
    }

    #[derive(Debug, Default)]
    struct GatedTransport {
        commit_started: Notify,
        allow_commit: Notify,
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

    #[tokio::test]
    async fn flushes_ordered_segments_at_the_byte_limit() {
        let transport = Arc::new(RecordingTransport::default());
        let writer = SegmentedLogWriter::new(transport.clone(), 42, 4, 60_000).unwrap();

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
        let writer = SegmentedLogWriter::new(transport.clone(), 7, 1024, 10).unwrap();

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
        let writer = SegmentedLogWriter::new(transport.clone(), 9, 4, 60_000).unwrap();

        writer.write_all(b"fail").await.unwrap();
        tokio::task::yield_now().await;
        let error = writer.write_all(b"later").await.unwrap_err();
        assert!(error.to_string().contains("injected segment failure"));
        assert!(writer.seal(false).await.is_err());
        assert!(transport.seals.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn large_write_backpressures_when_uncommitted_bytes_reach_the_limit() {
        let transport = Arc::new(GatedTransport::default());
        let writer = Arc::new(SegmentedLogWriter::new(transport.clone(), 10, 4, 60_000).unwrap());
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
        let writer = SegmentedLogWriter::new(transport.clone(), 11, 4, 60_000).unwrap();

        writer.write_all(b"abc").await.unwrap();
        drop(writer);
        tokio::task::yield_now().await;

        assert!(transport.segments.lock().unwrap().is_empty());
        assert!(transport.seals.lock().unwrap().is_empty());
    }
}
