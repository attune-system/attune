//! Log Writer Module
//!
//! Provides bounded log writers that limit output size to prevent OOM issues.

use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;

const TRUNCATION_NOTICE_STDOUT: &str = "\n\n[OUTPUT TRUNCATED: stdout exceeded size limit]\n";
const TRUNCATION_NOTICE_STDERR: &str = "\n\n[OUTPUT TRUNCATED: stderr exceeded size limit]\n";

// Reserve space for truncation notice so it can always fit
const NOTICE_RESERVE_BYTES: usize = 128;

/// Result of bounded log writing
#[derive(Debug, Clone)]
pub struct BoundedLogResult {
    /// The captured log content
    pub content: String,

    /// Whether the log was truncated
    pub truncated: bool,

    /// Number of bytes truncated (0 if not truncated)
    pub bytes_truncated: usize,

    /// Total bytes attempted to write
    pub total_bytes_attempted: usize,
}

impl BoundedLogResult {
    /// Create a new result with no truncation
    pub fn new(content: String) -> Self {
        let len = content.len();
        Self {
            content,
            truncated: false,
            bytes_truncated: 0,
            total_bytes_attempted: len,
        }
    }

    /// Create a truncated result
    pub fn truncated(
        content: String,
        bytes_truncated: usize,
        total_bytes_attempted: usize,
    ) -> Self {
        Self {
            content,
            truncated: true,
            bytes_truncated,
            total_bytes_attempted,
        }
    }
}

/// A writer that limits the amount of data captured and adds a truncation notice
pub struct BoundedLogWriter {
    /// Internal buffer for captured data
    buffer: Vec<u8>,

    /// Maximum bytes to capture
    max_bytes: usize,

    /// Whether we've already truncated and added the notice
    truncated: bool,

    /// Total bytes attempted to write (including truncated)
    total_bytes_attempted: usize,

    /// Actual data bytes written to buffer (excluding truncation notice)
    data_bytes_written: usize,

    /// Truncation notice to append when limit is reached
    truncation_notice: &'static str,
}

/// A transport-backed writer that applies the same truncation policy as `BoundedLogWriter`.
/// The writer is opened lazily on first write — if nothing is written, no writer is created.
///
/// When constructed with a path, it opens the file directly (legacy/volume mode).
/// When constructed with a pre-opened `BoxAsyncWriter`, it uses that writer (transport mode).
pub struct BoundedLogFileWriter {
    writer: RuntimeLogWriter,
    mirror_source: Option<attune_common::runtime_log_mirror::RuntimeLogSource>,
    finalization_timeout_ms: u64,
    max_bytes: usize,
    truncated: bool,
    data_bytes_written: usize,
    truncation_notice: &'static str,
}

enum RuntimeLogWriter {
    Segmented(attune_common::log_stream::SegmentedLogWriter),
    SharedFile(attune_common::log_stream::SharedFileLogWriter),
}

impl RuntimeLogWriter {
    async fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Segmented(writer) => writer.write_all(bytes).await,
            Self::SharedFile(writer) => writer.write_all(bytes).await,
        }
    }

    async fn seal(self, truncated: bool) -> attune_common::Result<()> {
        match self {
            Self::Segmented(writer) => writer.seal(truncated).await,
            Self::SharedFile(writer) => writer.seal(truncated).await,
        }
    }
}

impl BoundedLogWriter {
    /// Create a new bounded log writer for stdout
    pub fn new_stdout(max_bytes: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(std::cmp::min(max_bytes, 1024 * 1024)),
            max_bytes,
            truncated: false,
            total_bytes_attempted: 0,
            data_bytes_written: 0,
            truncation_notice: TRUNCATION_NOTICE_STDOUT,
        }
    }

    /// Create a new bounded log writer for stderr
    pub fn new_stderr(max_bytes: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(std::cmp::min(max_bytes, 1024 * 1024)),
            max_bytes,
            truncated: false,
            total_bytes_attempted: 0,
            data_bytes_written: 0,
            truncation_notice: TRUNCATION_NOTICE_STDERR,
        }
    }

    /// Get the result with truncation information
    pub fn into_result(self) -> BoundedLogResult {
        let content = String::from_utf8_lossy(&self.buffer).to_string();

        if self.truncated {
            BoundedLogResult::truncated(
                content,
                self.total_bytes_attempted
                    .saturating_sub(self.data_bytes_written),
                self.total_bytes_attempted,
            )
        } else {
            BoundedLogResult::new(content)
        }
    }

    /// Write data to the buffer, respecting size limits
    fn write_bounded(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.total_bytes_attempted = self.total_bytes_attempted.saturating_add(buf.len());

        // If already truncated, discard all further writes
        if self.truncated {
            return Ok(buf.len()); // Pretend we wrote it all
        }

        let current_size = self.buffer.len();
        // Reserve space for truncation notice
        let effective_limit = self.max_bytes.saturating_sub(NOTICE_RESERVE_BYTES);
        let remaining_space = effective_limit.saturating_sub(current_size);

        if remaining_space == 0 {
            // Already at limit, add truncation notice if not already added
            if !self.truncated {
                self.add_truncation_notice();
            }
            return Ok(buf.len()); // Pretend we wrote it all
        }

        // Calculate how much we can actually write
        let bytes_to_write = std::cmp::min(buf.len(), remaining_space);

        if bytes_to_write < buf.len() {
            // We're about to hit the limit
            self.buffer.extend_from_slice(&buf[..bytes_to_write]);
            self.data_bytes_written += bytes_to_write;
            self.add_truncation_notice();
        } else {
            // We can write everything
            self.buffer.extend_from_slice(&buf[..bytes_to_write]);
            self.data_bytes_written += bytes_to_write;
        }

        Ok(buf.len()) // Always report full write to avoid backpressure issues
    }

    /// Add truncation notice to the buffer
    fn add_truncation_notice(&mut self) {
        self.truncated = true;

        let notice_bytes = self.truncation_notice.as_bytes();
        // We reserved space, so the notice should always fit
        self.buffer.extend_from_slice(notice_bytes);
    }
}

impl BoundedLogFileWriter {
    pub fn from_segmented_writer(
        writer: attune_common::log_stream::SegmentedLogWriter,
        max_bytes: usize,
        is_stdout: bool,
    ) -> Self {
        let finalization_timeout_ms = writer.finalization_timeout_ms();
        Self {
            writer: RuntimeLogWriter::Segmented(writer),
            mirror_source: None,
            finalization_timeout_ms,
            max_bytes,
            truncated: false,
            data_bytes_written: 0,
            truncation_notice: if is_stdout {
                TRUNCATION_NOTICE_STDOUT
            } else {
                TRUNCATION_NOTICE_STDERR
            },
        }
    }

    pub fn from_shared_file_writer(
        writer: attune_common::log_stream::SharedFileLogWriter,
        max_bytes: usize,
        is_stdout: bool,
        finalization_timeout_ms: u64,
    ) -> Self {
        Self {
            writer: RuntimeLogWriter::SharedFile(writer),
            mirror_source: None,
            finalization_timeout_ms,
            max_bytes,
            truncated: false,
            data_bytes_written: 0,
            truncation_notice: if is_stdout {
                TRUNCATION_NOTICE_STDOUT
            } else {
                TRUNCATION_NOTICE_STDERR
            },
        }
    }

    pub async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        if buf.is_empty() || self.truncated {
            return Ok(());
        }

        let effective_limit = self.max_bytes.saturating_sub(NOTICE_RESERVE_BYTES);
        let remaining_space = effective_limit.saturating_sub(self.data_bytes_written);

        if remaining_space == 0 {
            self.add_truncation_notice().await?;
            return Ok(());
        }

        let bytes_to_write = std::cmp::min(buf.len(), remaining_space);
        if bytes_to_write > 0 {
            self.writer.write_all(&buf[..bytes_to_write]).await?;
            self.data_bytes_written += bytes_to_write;
        }

        if bytes_to_write < buf.len() {
            self.add_truncation_notice().await?;
        }

        Ok(())
    }

    pub fn with_mirror_source(
        mut self,
        source: Option<attune_common::runtime_log_mirror::RuntimeLogSource>,
    ) -> Self {
        self.mirror_source = source;
        self
    }

    pub fn mirror_source(&self) -> Option<&attune_common::runtime_log_mirror::RuntimeLogSource> {
        self.mirror_source.as_ref()
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    async fn add_truncation_notice(&mut self) -> std::io::Result<()> {
        if self.truncated {
            return Ok(());
        }

        self.truncated = true;
        let notice = self.truncation_notice;
        self.writer.write_all(notice.as_bytes()).await
    }

    pub async fn seal(self) -> attune_common::Result<()> {
        self.writer.seal(self.truncated).await
    }

    pub fn finalization_timeout_ms(&self) -> u64 {
        self.finalization_timeout_ms
    }
}

impl AsyncWrite for BoundedLogWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(self.write_bounded(buf))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};
    use tokio::io::AsyncWriteExt;

    #[derive(Debug, Default)]
    struct RecordingSharedTransport {
        appends: Mutex<Vec<u8>>,
        segments: Mutex<Vec<(i64, i64, Vec<u8>)>>,
        seals: Mutex<Vec<(i64, bool)>>,
    }

    #[async_trait]
    impl attune_common::artifact_transport::ArtifactFileTransport for RecordingSharedTransport {
        async fn write_file(
            &self,
            _: &str,
            _: &[u8],
            _: Option<&str>,
        ) -> attune_common::Result<()> {
            unreachable!()
        }
        async fn file_exists(&self, _: &str) -> attune_common::Result<bool> {
            unreachable!()
        }
        async fn file_size(&self, _: &str) -> attune_common::Result<Option<u64>> {
            unreachable!()
        }
        async fn delete_file(&self, _: &str) -> attune_common::Result<()> {
            unreachable!()
        }
        async fn append_log_file(&self, _: &str, content: &[u8]) -> attune_common::Result<()> {
            self.appends.lock().unwrap().extend_from_slice(content);
            Ok(())
        }
        async fn commit_log_segment(
            &self,
            artifact_version: i64,
            sequence: i64,
            content: &[u8],
        ) -> attune_common::Result<()> {
            self.segments
                .lock()
                .unwrap()
                .push((artifact_version, sequence, content.to_vec()));
            Ok(())
        }
        async fn seal_log_stream(
            &self,
            artifact_version: i64,
            truncated: bool,
        ) -> attune_common::Result<()> {
            self.seals
                .lock()
                .unwrap()
                .push((artifact_version, truncated));
            Ok(())
        }
        async fn open_reader(
            &self,
            _: &str,
            _: u64,
        ) -> attune_common::Result<attune_common::artifact_transport::BoxAsyncReader> {
            unreachable!()
        }
        fn transport_mode(&self) -> &'static str {
            "volume"
        }
        fn base_dir(&self) -> &str {
            "/unused"
        }
    }

    #[tokio::test]
    async fn test_bounded_writer_under_limit() {
        let mut writer = BoundedLogWriter::new_stdout(1024);
        let data = b"Hello, world!";

        writer.write_all(data).await.unwrap();

        let result = writer.into_result();
        assert_eq!(result.content, "Hello, world!");
        assert!(!result.truncated);
        assert_eq!(result.bytes_truncated, 0);
        assert_eq!(result.total_bytes_attempted, 13);
    }

    #[tokio::test]
    async fn test_bounded_writer_at_limit() {
        // With 178 bytes, we can fit 50 bytes (178 - 128 reserve = 50)
        let mut writer = BoundedLogWriter::new_stdout(178);
        let data = b"12345678901234567890123456789012345678901234567890"; // 50 bytes

        writer.write_all(data).await.unwrap();

        let result = writer.into_result();
        assert_eq!(result.content.len(), 50);
        assert!(!result.truncated);
        assert_eq!(result.bytes_truncated, 0);
    }

    #[tokio::test]
    async fn test_bounded_writer_exceeds_limit() {
        // 148 bytes means effective limit is 20 (148 - 128 = 20)
        let mut writer = BoundedLogWriter::new_stdout(148);
        let data = b"This is a long message that exceeds the limit";

        writer.write_all(data).await.unwrap();

        let result = writer.into_result();
        assert!(result.truncated);
        assert!(result.content.contains("[OUTPUT TRUNCATED"));
        assert!(result.bytes_truncated > 0);
        assert_eq!(result.total_bytes_attempted, 45);
    }

    #[tokio::test]
    async fn test_bounded_writer_multiple_writes() {
        // 148 bytes means effective limit is 20 (148 - 128 = 20)
        let mut writer = BoundedLogWriter::new_stdout(148);

        writer.write_all(b"First ").await.unwrap(); // 6 bytes
        writer.write_all(b"Second ").await.unwrap(); // 7 bytes = 13 total
        writer.write_all(b"Third ").await.unwrap(); // 6 bytes = 19 total
        writer.write_all(b"Fourth ").await.unwrap(); // 7 bytes = 26 total, exceeds 20 limit

        let result = writer.into_result();
        assert!(result.truncated);
        assert!(result.content.contains("[OUTPUT TRUNCATED"));
        assert_eq!(result.total_bytes_attempted, 26);
    }

    #[tokio::test]
    async fn test_bounded_writer_stderr_notice() {
        // 143 bytes means effective limit is 15 (143 - 128 = 15)
        let mut writer = BoundedLogWriter::new_stderr(143);
        let data = b"Error message that is too long";

        writer.write_all(data).await.unwrap();

        let result = writer.into_result();
        assert!(result.truncated);
        assert!(result.content.contains("stderr exceeded size limit"));
    }

    #[tokio::test]
    async fn test_bounded_writer_empty() {
        let writer = BoundedLogWriter::new_stdout(1024);

        let result = writer.into_result();
        assert_eq!(result.content, "");
        assert!(!result.truncated);
        assert_eq!(result.bytes_truncated, 0);
        assert_eq!(result.total_bytes_attempted, 0);
    }

    #[tokio::test]
    async fn test_bounded_writer_exact_limit_no_truncation_notice() {
        // 138 bytes means effective limit is 10 (138 - 128 = 10)
        let mut writer = BoundedLogWriter::new_stdout(138);
        let data = b"1234567890"; // Exactly 10 bytes

        writer.write_all(data).await.unwrap();

        let result = writer.into_result();
        assert_eq!(result.content, "1234567890");
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn test_bounded_writer_one_byte_over() {
        // 138 bytes means effective limit is 10 (138 - 128 = 10)
        let mut writer = BoundedLogWriter::new_stdout(138);
        let data = b"12345678901"; // 11 bytes

        writer.write_all(data).await.unwrap();

        let result = writer.into_result();
        assert!(result.truncated);
        assert_eq!(result.bytes_truncated, 1);
    }

    #[tokio::test]
    async fn shared_file_writer_appends_bounded_bytes_and_seals_without_segments() {
        let transport = Arc::new(RecordingSharedTransport::default());
        let writer = attune_common::log_stream::SharedFileLogWriter::new(
            transport.clone(),
            42,
            "core/echo/stdout/log/v1.txt".to_string(),
        );
        let mut writer = BoundedLogFileWriter::from_shared_file_writer(writer, 138, true, 100);

        writer.write_all(b"12345678901").await.unwrap();
        writer.seal().await.unwrap();

        let content = transport.appends.lock().unwrap().clone();
        assert_eq!(&content[..10], b"1234567890");
        assert!(String::from_utf8(content)
            .unwrap()
            .contains("stdout exceeded size limit"));
        assert_eq!(*transport.seals.lock().unwrap(), vec![(42, true)]);
        assert!(transport.segments.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn segmented_writer_commits_the_truncation_notice_and_seals_truncated() {
        let transport = Arc::new(RecordingSharedTransport::default());
        let writer = attune_common::log_stream::SegmentedLogWriter::new(
            transport.clone(),
            43,
            attune_common::log_stream::SegmentedLogConfig {
                initial_segment_bytes: 1024,
                max_segment_bytes: 1024,
                flush_interval_ms: 60_000,
                retry_max_attempts: 1,
                retry_attempt_timeout_ms: 100,
                retry_initial_backoff_ms: 1,
                retry_max_backoff_ms: 1,
                finalization_timeout_ms: 100,
            },
        )
        .unwrap();
        let mut writer = BoundedLogFileWriter::from_segmented_writer(writer, 138, true);

        writer.write_all(b"12345678901").await.unwrap();
        writer.seal().await.unwrap();

        let segments = transport.segments.lock().unwrap();
        assert_eq!(segments.len(), 1);
        assert!(String::from_utf8(segments[0].2.clone())
            .unwrap()
            .contains("stdout exceeded size limit"));
        assert_eq!(*transport.seals.lock().unwrap(), vec![(43, true)]);
    }
}
