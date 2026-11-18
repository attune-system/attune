//! Small bounded buffer for the owned Engine binding probe and its fixtures.
//! Production workload capture goes directly to the trusted worker's streams.
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::io::AsyncWrite;
#[derive(Clone)]
pub(crate) struct ProbeCapture {
    state: Arc<Mutex<(Vec<u8>, bool, bool)>>,
    limit: usize,
}
impl ProbeCapture {
    pub fn new(limit: usize) -> io::Result<Self> {
        if limit == 0 || limit > 65536 {
            return Err(io::Error::other("invalid probe bound"));
        }
        Ok(Self {
            state: Arc::new(Mutex::new((Vec::new(), false, false))),
            limit,
        })
    }
    pub fn bytes(&self) -> io::Result<Vec<u8>> {
        let s = self
            .state
            .lock()
            .map_err(|_| io::Error::other("probe unavailable"))?;
        if !s.1 || s.2 {
            return Err(io::Error::other("probe incomplete"));
        }
        Ok(s.0.clone())
    }
}
impl AsyncWrite for ProbeCapture {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| io::Error::other("probe unavailable"))?;
        if s.1 || s.0.len().saturating_add(bytes.len()) > self.limit {
            s.2 = true;
            return Poll::Ready(Err(io::Error::other("probe exceeds bound")));
        }
        s.0.extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| io::Error::other("probe unavailable"))?;
        s.1 = true;
        Poll::Ready(Ok(()))
    }
}
