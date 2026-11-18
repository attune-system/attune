//! Bounded ephemeral raw transport, separate from encrypted durable log custody.
//! Slow readers receive explicit missing-byte evidence, never a complete log.
use std::{
    collections::VecDeque,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::io::AsyncWrite;

#[derive(Clone)]
pub struct Capture {
    inner: Arc<Mutex<State>>,
    capacity: usize,
}
struct State {
    bytes: VecDeque<u8>,
    offset: u64,
    total: u64,
    closed: bool,
    failed: bool,
}
pub struct CapturedChunk {
    pub bytes: Vec<u8>,
    pub next_offset: u64,
    pub truncated_bytes: u64,
    pub eof: bool,
}
impl Capture {
    pub fn new(capacity: usize) -> io::Result<Self> {
        if capacity == 0 || capacity > 64 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid capture ceiling",
            ));
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(State {
                bytes: VecDeque::new(),
                offset: 0,
                total: 0,
                closed: false,
                failed: false,
            })),
            capacity,
        })
    }
    pub fn read(&self, offset: u64, max_bytes: u32) -> io::Result<CapturedChunk> {
        if max_bytes == 0 || max_bytes > 65536 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid finite read",
            ));
        }
        let state = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("capture state unavailable"))?;
        if state.failed || offset > state.total {
            return Err(io::Error::other("capture incomplete or offset invalid"));
        }
        let missing = state.offset.saturating_sub(offset);
        let start = offset.max(state.offset);
        let bytes = state
            .bytes
            .iter()
            .skip((start - state.offset) as usize)
            .take(max_bytes as usize)
            .copied()
            .collect::<Vec<_>>();
        let next_offset = start + bytes.len() as u64;
        Ok(CapturedChunk {
            bytes,
            next_offset,
            truncated_bytes: missing,
            eof: state.closed && next_offset == state.total,
        })
    }
    pub fn fail(&self) {
        if let Ok(mut state) = self.inner.lock() {
            state.failed = true;
            state.closed = true;
        }
    }
}
impl AsyncWrite for Capture {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut state = match self.inner.lock() {
            Ok(state) => state,
            Err(_) => return Poll::Ready(Err(io::Error::other("capture state unavailable"))),
        };
        if state.closed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "capture closed",
            )));
        }
        let Some(total) = state.total.checked_add(bytes.len() as u64) else {
            state.failed = true;
            return Poll::Ready(Err(io::Error::other("capture offset overflow")));
        };
        state.total = total;
        if bytes.len() >= self.capacity {
            state.bytes.clear();
            state.bytes.extend(&bytes[bytes.len() - self.capacity..]);
        } else {
            while state.bytes.len() + bytes.len() > self.capacity {
                state.bytes.pop_front();
            }
            state.bytes.extend(bytes);
        }
        state.offset = state.total - state.bytes.len() as u64;
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.inner.lock() {
            Ok(mut state) => {
                state.closed = true;
                Poll::Ready(Ok(()))
            }
            Err(_) => Poll::Ready(Err(io::Error::other("capture state unavailable"))),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    #[tokio::test]
    async fn wrap_reports_exact_missing_bytes_and_eof() {
        let mut capture = Capture::new(4).unwrap();
        capture.write_all(b"abcdef").await.unwrap();
        let chunk = capture.read(0, 3).unwrap();
        assert_eq!(chunk.bytes, b"cde");
        assert_eq!(chunk.truncated_bytes, 2);
        assert_eq!(chunk.next_offset, 5);
        assert!(!chunk.eof);
        capture.shutdown().await.unwrap();
        let tail = capture.read(5, 3).unwrap();
        assert_eq!(tail.bytes, b"f");
        assert!(tail.eof);
        capture.fail();
        assert!(capture.read(5, 3).is_err());
    }
}
