use std::future::Future;
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

use bytes::BufMut;

pub use buffered::{BufferedSocket, WriteBuffer};

use crate::io::ReadBuf;

mod buffered;

pub trait Socket: Send + Sync + Unpin + 'static {
    fn try_read(&mut self, buf: &mut dyn ReadBuf) -> io::Result<usize>;

    fn try_write(&mut self, buf: &[u8]) -> io::Result<usize>;

    fn poll_read_ready(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>>;

    fn poll_write_ready(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>>;

    fn poll_flush(&mut self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // `flush()` is a no-op for TCP/UDS
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>>;

    fn read<'a, B: ReadBuf>(&'a mut self, buf: &'a mut B) -> Read<'a, Self, B>
    where
        Self: Sized,
    {
        Read { socket: self, buf }
    }

    fn write<'a>(&'a mut self, buf: &'a [u8]) -> Write<'a, Self>
    where
        Self: Sized,
    {
        Write { socket: self, buf }
    }

    fn flush(&mut self) -> Flush<'_, Self>
    where
        Self: Sized,
    {
        Flush { socket: self }
    }

    fn shutdown(&mut self) -> Shutdown<'_, Self>
    where
        Self: Sized,
    {
        Shutdown { socket: self }
    }
}

pub struct Read<'a, S: ?Sized, B> {
    socket: &'a mut S,
    buf: &'a mut B,
}

impl<'a, S: ?Sized, B> Future for Read<'a, S, B>
where
    S: Socket,
    B: ReadBuf,
{
    type Output = io::Result<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;

        while this.buf.has_remaining_mut() {
            match this.socket.try_read(&mut *this.buf) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    ready!(this.socket.poll_read_ready(cx))?;
                }
                ready => return Poll::Ready(ready),
            }
        }

        Poll::Ready(Ok(0))
    }
}

pub struct Write<'a, S: ?Sized> {
    socket: &'a mut S,
    buf: &'a [u8],
}

impl<'a, S: ?Sized> Future for Write<'a, S>
where
    S: Socket,
{
    type Output = io::Result<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;

        while !this.buf.is_empty() {
            match this.socket.try_write(this.buf) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    ready!(this.socket.poll_write_ready(cx))?;
                }
                ready => return Poll::Ready(ready),
            }
        }

        Poll::Ready(Ok(0))
    }
}

pub struct Flush<'a, S: ?Sized> {
    socket: &'a mut S,
}

impl<'a, S: Socket + ?Sized> Future for Flush<'a, S> {
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.socket.poll_flush(cx)
    }
}

pub struct Shutdown<'a, S: ?Sized> {
    socket: &'a mut S,
}

impl<'a, S: ?Sized> Future for Shutdown<'a, S>
where
    S: Socket,
{
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.socket.poll_shutdown(cx)
    }
}

pub trait WithSocket {
    type Output;

    /// A connected TCP peer, captured before TLS wrapping or type erasure.
    /// Existing socket consumers retain their original default behavior.
    fn with_tcp_socket<S: Socket>(
        self,
        socket: S,
        _peer: std::net::SocketAddr,
    ) -> impl std::future::Future<Output = Self::Output> + Send
    where
        Self: Sized,
    {
        self.with_socket(socket)
    }

    fn with_socket<S: Socket>(
        self,
        socket: S,
    ) -> impl std::future::Future<Output = Self::Output> + Send;
}

pub struct SocketIntoBox;

impl WithSocket for SocketIntoBox {
    type Output = Box<dyn Socket>;

    async fn with_socket<S: Socket>(self, socket: S) -> Self::Output {
        Box::new(socket)
    }
}

impl<S: Socket + ?Sized> Socket for Box<S> {
    fn try_read(&mut self, buf: &mut dyn ReadBuf) -> io::Result<usize> {
        (**self).try_read(buf)
    }

    fn try_write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (**self).try_write(buf)
    }

    fn poll_read_ready(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        (**self).poll_read_ready(cx)
    }

    fn poll_write_ready(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        (**self).poll_write_ready(cx)
    }

    fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        (**self).poll_flush(cx)
    }

    fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        (**self).poll_shutdown(cx)
    }
}

pub async fn connect_tcp<Ws: WithSocket>(
    host: &str,
    port: u16,
    with_socket: Ws,
) -> crate::Result<Ws::Output> {
    // Keep the existing hostname API and bracket trimming unchanged.
    let host = host.trim_matches(&['[', ']'][..]);
    connect_tcp_target(TcpConnectTarget::Host(host, port), with_socket).await
}

/// Connect to the complete captured address without DNS or text reconstruction.
pub async fn connect_tcp_addr<Ws: WithSocket>(
    endpoint: std::net::SocketAddr,
    with_socket: Ws,
) -> crate::Result<Ws::Output> {
    connect_tcp_target(TcpConnectTarget::exact(endpoint), with_socket).await
}

enum TcpConnectTarget<'a> {
    Host(&'a str, u16),
    Address(std::net::SocketAddr),
}
impl TcpConnectTarget<'_> {
    fn exact(endpoint: std::net::SocketAddr) -> Self {
        Self::Address(endpoint)
    }
}

async fn connect_tcp_target<Ws: WithSocket>(
    target: TcpConnectTarget<'_>,
    with_socket: Ws,
) -> crate::Result<Ws::Output> {
    #[cfg(feature = "_rt-tokio")]
    if crate::rt::rt_tokio::available() {
        use tokio::net::TcpStream;
        let stream = match target {
            TcpConnectTarget::Host(host, port) => TcpStream::connect((host, port)).await?,
            // Pass the full value directly, including IPv6 flowinfo/scope_id.
            TcpConnectTarget::Address(endpoint) => TcpStream::connect(endpoint).await?,
        };
        stream.set_nodelay(true)?;
        let peer = stream.peer_addr()?;
        return Ok(with_socket.with_tcp_socket(stream, peer).await);
    }

    #[cfg(feature = "_rt-async-std")]
    {
        use async_io::Async;
        use async_std::net::ToSocketAddrs;
        use std::net::TcpStream;
        let addresses = match target {
            TcpConnectTarget::Host(host, port) => {
                (host, port).to_socket_addrs().await?.collect::<Vec<_>>()
            }
            // No string conversion, DNS lookup or SocketAddrV6 reconstruction.
            TcpConnectTarget::Address(endpoint) => vec![endpoint],
        };
        let mut last_err = None;
        for endpoint in addresses {
            let stream = Async::<TcpStream>::connect(endpoint)
                .await
                .and_then(|stream| {
                    stream.get_ref().set_nodelay(true)?;
                    Ok(stream)
                });
            match stream {
                Ok(stream) => {
                    let peer = stream.get_ref().peer_addr()?;
                    return Ok(with_socket.with_tcp_socket(stream, peer).await);
                }
                Err(error) => last_err = Some(error),
            }
        }
        match last_err {
            Some(error) => Err(error.into()),
            None => Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "Hostname did not resolve to any addresses",
            )
            .into()),
        }
    }
    #[cfg(not(feature = "_rt-async-std"))]
    {
        crate::rt::missing_rt((target, with_socket))
    }
}

#[test]
fn exact_tcp_target_preserves_scoped_ipv6_socket_addr() {
    use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
    let captured = SocketAddr::V6(SocketAddrV6::new(
        Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1),
        5432,
        0x12345,
        17,
    ));
    // Exercise the same adapter used by connect_tcp_addr before both runtime
    // branches consume the address. This test opens no socket or interface.
    let TcpConnectTarget::Address(for_runtime) = TcpConnectTarget::exact(captured) else {
        panic!("exact endpoint was converted to a hostname");
    };
    assert_eq!(for_runtime, captured);
    let SocketAddr::V6(address) = for_runtime else {
        panic!("lost IPv6 endpoint");
    };
    assert_eq!(address.scope_id(), 17);
    assert_eq!(address.flowinfo(), 0x12345);
}

/// Connect a Unix Domain Socket at the given path.
///
/// Returns an error if Unix Domain Sockets are not supported on this platform.
pub async fn connect_uds<P: AsRef<Path>, Ws: WithSocket>(
    path: P,
    with_socket: Ws,
) -> crate::Result<Ws::Output> {
    #[cfg(unix)]
    {
        #[cfg(feature = "_rt-tokio")]
        if crate::rt::rt_tokio::available() {
            use tokio::net::UnixStream;

            let stream = UnixStream::connect(path).await?;

            return Ok(with_socket.with_socket(stream).await);
        }

        #[cfg(feature = "_rt-async-std")]
        {
            use async_io::Async;
            use std::os::unix::net::UnixStream;

            let stream = Async::<UnixStream>::connect(path).await?;

            Ok(with_socket.with_socket(stream).await)
        }

        #[cfg(not(feature = "_rt-async-std"))]
        {
            crate::rt::missing_rt((path, with_socket))
        }
    }

    #[cfg(not(unix))]
    {
        drop((path, with_socket));

        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Unix domain sockets are not supported on this platform",
        )
        .into())
    }
}
