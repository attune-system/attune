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
    writer: attune_common::log_stream::SegmentedLogWriter,
    max_bytes: usize,
    truncated: bool,
    data_bytes_written: usize,
    truncation_notice: &'static str,
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
        Self {
            writer,
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
    use tokio::io::AsyncWriteExt;

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
}
