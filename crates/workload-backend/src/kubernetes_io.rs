//! Bounded non-TTY Kubernetes channels. Require protocol v5 so closing stdin
//! does not close stdout/stderr. A WebSocket close without terminal Status is
//! transport loss, never successful EOF.
use crate::process_policy::invalid;
use attune_common::{isolated_process::ProcessReader, Result};
use futures::{
    future::{BoxFuture, Shared},
    FutureExt, SinkExt, StreamExt,
};
use k8s_openapi::{api::core::v1::Pod, apimachinery::pkg::apis::meta::v1::Status};
use kube::{api::AttachParams, core::Request, Api, Client};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tungstenite::Message;
const BUFFER: usize = 64 * 1024;
struct TaskOwner(tokio::task::AbortHandle);
impl Drop for TaskOwner {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub(crate) struct Streams {
    pub stdin: Box<dyn AsyncWrite + Unpin + Send>,
    pub stdout: ProcessReader,
    pub stderr: ProcessReader,
    terminal: Shared<BoxFuture<'static, Option<bool>>>,
}
impl Streams {
    pub async fn success(&mut self) -> Result<()> {
        if self.terminal.clone().await == Some(true) {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}
struct CheckedReader {
    inner: DuplexStream,
    terminal: Shared<BoxFuture<'static, Option<bool>>>,
    _owner: Arc<TaskOwner>,
}
impl AsyncRead for CheckedReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) if buf.filled().len() == before && buf.remaining() != 0 => {
                use std::future::Future;
                match Pin::new(&mut self.terminal).poll(cx) {
                    Poll::Ready(Some(_)) => Poll::Ready(Ok(())),
                    Poll::Ready(None) => Poll::Ready(Err(io::Error::other(
                        "isolated process capture transport lost",
                    ))),
                    Poll::Pending => Poll::Pending,
                }
            }
            result => result,
        }
    }
}
struct OwnedWriter {
    inner: DuplexStream,
    _owner: Arc<TaskOwner>,
}
impl AsyncWrite for OwnedWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        b: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, b)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
pub(crate) async fn attach(client: &Client, pods: &Api<Pod>, name: &str) -> Result<Streams> {
    let params = AttachParams {
        container: Some(attune_common::isolated_process::PAYLOAD_CONTAINER.into()),
        stdin: true,
        stdout: true,
        stderr: true,
        tty: false,
        ..Default::default()
    };
    connect(
        client,
        Request::new(pods.resource_url())
            .attach(name, &params)
            .map_err(|_| invalid())?,
    )
    .await
}
pub(crate) async fn exec(
    client: &Client,
    pods: &Api<Pod>,
    name: &str,
    container: &str,
    args: Vec<String>,
) -> Result<Streams> {
    let params = AttachParams {
        container: Some(container.into()),
        stdin: true,
        stdout: true,
        stderr: true,
        tty: false,
        ..Default::default()
    };
    connect(
        client,
        Request::new(pods.resource_url())
            .exec(name, args, &params)
            .map_err(|_| invalid())?,
    )
    .await
}
async fn connect(client: &Client, request: http::Request<Vec<u8>>) -> Result<Streams> {
    let connection = client.connect(request).await.map_err(|_| invalid())?;
    if !connection.supports_stream_close() {
        return Err(invalid());
    }
    let (stdin, mut source) = tokio::io::duplex(BUFFER);
    let (mut stdout, stdout_reader) = tokio::io::duplex(BUFFER);
    let (mut stderr, stderr_reader) = tokio::io::duplex(BUFFER);
    let task = tokio::spawn(async move {
        let (mut send, mut receive) = connection.into_stream().split();
        let mut input_open = true;
        let mut input = [0; BUFFER];
        let mut ping = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tokio::select! {
                bytes=source.read(&mut input),if input_open=>{
                    let count=bytes.ok()?;
                    let frame=if count==0 {input_open=false;vec![255,0]} else {let mut v=Vec::with_capacity(count+1);v.push(0);v.extend_from_slice(&input[..count]);v};
                    send.send(Message::Binary(frame.into())).await.ok()?;
                },
                message=receive.next()=>{
                    match message?.ok()? {
                        Message::Binary(bytes)=>{
                            if bytes.len()>16*1024*1024 {return None;}
                            match bytes.first().copied()? {
                                1=>stdout.write_all(&bytes[1..]).await.ok()?,
                                2=>stderr.write_all(&bytes[1..]).await.ok()?,
                                3=>{let status:Status=serde_json::from_slice(&bytes[1..]).ok()?;
                                    return match status.status.as_deref() {Some("Success")=>Some(true),Some("Failure")=>Some(false),_=>None};},
                                _=>return None,
                            }
                        },
                        Message::Ping(bytes)=>send.send(Message::Pong(bytes)).await.ok()?,
                        Message::Pong(_)=>{},
                        Message::Close(_)=>return None,
                        _=>return None,
                    }
                },
                _=ping.tick()=>send.send(Message::Ping(Vec::new().into())).await.ok()?,
            }
        }
    });
    let owner = Arc::new(TaskOwner(task.abort_handle()));
    let terminal = async move { task.await.ok().flatten() }.boxed().shared();
    Ok(Streams {
        stdin: Box::new(OwnedWriter {
            inner: stdin,
            _owner: owner.clone(),
        }),
        stdout: Box::pin(CheckedReader {
            inner: stdout_reader,
            terminal: terminal.clone(),
            _owner: owner.clone(),
        }),
        stderr: Box::pin(CheckedReader {
            inner: stderr_reader,
            terminal: terminal.clone(),
            _owner: owner,
        }),
        terminal,
    })
}
