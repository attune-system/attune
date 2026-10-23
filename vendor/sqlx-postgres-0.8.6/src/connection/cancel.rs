use std::fmt;
use std::sync::Arc;

use crate::connection::stream::PgStream;
use crate::io::ProtocolEncode;
use crate::{Error, PgConnectOptions, PgConnection};

/// An owned capability for this connection's PostgreSQL 3.0 backend key.
/// No PID/key constructor, getters, serialization, or alternate destination.
/// Do not send after allowing the original session to return to a pool.
pub struct PgCancelToken {
    process_id: u32,
    secret_key: u32,
    options: Arc<PgConnectOptions>,
    endpoint: Option<std::net::SocketAddr>,
}

impl fmt::Debug for PgCancelToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PgCancelToken([REDACTED])")
    }
}
impl fmt::Display for PgCancelToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PgCancelToken([REDACTED])")
    }
}

impl PgConnection {
    /// Snapshot the established connection's backend key without I/O or SQL.
    /// Routing is pinned to its TCP peer, or its original UDS path. A proxy must
    /// implement cancellation routing correctly; no wire acknowledgement proves
    /// target cancellation. Relays must honor PostgreSQL cancellation routing;
    /// unsupported routing is not certified by successful transport closure.
    pub fn cancel_token(&self) -> Result<PgCancelToken, Error> {
        if self.inner.process_id == 0 {
            return Err(err_protocol!("backend cancellation capability unavailable"));
        }
        Ok(PgCancelToken {
            process_id: self.inner.process_id,
            secret_key: self.inner.secret_key,
            options: self.inner.cancel_options.clone(),
            endpoint: self.inner.stream.cancel_endpoint,
        })
    }
}

impl PgCancelToken {
    /// Open ONE startup-only channel, negotiate the original TLS policy, send a
    /// CancelRequest and wait for remote close. No authentication/StartupMessage,
    /// query, retry, deadline, or task is created here. Caller owns the deadline
    /// and resource permit. Dropping this future drops its transport.
    ///
    /// Ok means transport completion only. PostgreSQL sends no acknowledgement
    /// of matching a key, canceling a query, ending a session, or rolling back.
    pub async fn cancel(self) -> Result<(), Error> {
        let mut stream = PgStream::connect_cancel(&self.options, self.endpoint).await?;
        stream.write(CancelRequest {
            process_id: self.process_id,
            secret_key: self.secret_key,
        })?;
        stream.flush().await?;
        match stream.read_buffered(1).await {
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(()),
            Err(error) => Err(error),
            Ok(_) => Err(err_protocol!("unexpected response on cancellation channel")),
        }
        // Drop closes the startup-only socket, including on timeout/error.
    }
}

struct CancelRequest {
    process_id: u32,
    secret_key: u32,
}
impl ProtocolEncode<'_> for CancelRequest {
    fn encode_with(&self, buffer: &mut Vec<u8>, _context: ()) -> Result<(), Error> {
        buffer.extend_from_slice(&16_u32.to_be_bytes());
        buffer.extend_from_slice(&80_877_102_u32.to_be_bytes());
        buffer.extend_from_slice(&self.process_id.to_be_bytes());
        buffer.extend_from_slice(&self.secret_key.to_be_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancel_request_is_a_startup_packet_not_a_query_message() {
        let mut packet = Vec::new();
        CancelRequest {
            process_id: 7,
            secret_key: 11,
        }
        .encode_with(&mut packet, ())
        .unwrap();
        assert_eq!(
            packet,
            [0, 0, 0, 16, 4, 210, 22, 46, 0, 0, 0, 7, 0, 0, 0, 11]
        );
    }
    #[test]
    fn capability_formatting_is_fully_redacted() {
        let token = PgCancelToken {
            process_id: 7,
            secret_key: 11,
            options: Arc::new(PgConnectOptions::new_without_pgpass()),
            endpoint: None,
        };
        assert_eq!(format!("{token:?}"), "PgCancelToken([REDACTED])");
        assert_eq!(format!("{token}"), "PgCancelToken([REDACTED])");
    }
}
