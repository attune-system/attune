//! Local wire peers exercise the actual SQLx connection and cancellation cleanup.
//! No database, schema, or shared service is involved.

use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
    task::JoinHandle,
};

#[derive(Clone, Copy)]
enum Failure {
    Subscription,
    Probe,
    Receive,
}

async fn wire_peer(
    failure: Failure,
) -> Result<(String, oneshot::Receiver<()>, JoinHandle<Result<()>>)> {
    let socket = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!(
        "postgresql://fixture@{}/fixture?sslmode=disable",
        socket.local_addr()?
    );
    let (started_tx, started_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut stream, _) = socket.accept().await?;
        // Consume StartupMessage, then send AuthenticationOk and ReadyForQuery.
        let length = stream.read_u32().await?;
        let mut startup = vec![0; length as usize - 4];
        stream.read_exact(&mut startup).await?;
        stream.write_all(b"R\0\0\0\x08\0\0\0\0Z\0\0\0\x05I").await?;
        let mut subscribed = false;
        loop {
            let kind = stream.read_u8().await?;
            let length = stream.read_u32().await?;
            let mut payload = vec![0; length as usize - 4];
            stream.read_exact(&mut payload).await?;
            match kind {
                b'Q' => {
                    // Observe the real subscription write, not source text.
                    let expected = NOTIFICATION_CHANNELS
                        .iter()
                        .map(|channel| format!("LISTEN \"{channel}\";"))
                        .collect::<String>()
                        + "\0";
                    assert_eq!(payload, expected.as_bytes());
                    assert!(!subscribed, "subscribe once to the full channel list");
                    if matches!(failure, Failure::Subscription) {
                        break;
                    }
                    subscribed = true;
                    // CommandComplete and ReadyForQuery finish the subscription.
                    stream.write_all(b"C\0\0\0\x0bLISTEN\0Z\0\0\0\x05I").await?;
                    if matches!(failure, Failure::Receive) {
                        // A half-close produces a detected receive failure while
                        // retaining the read side to observe client disposal.
                        stream.shutdown().await?;
                        break;
                    }
                }
                b'S' => {
                    assert!(payload.is_empty(), "ping must use protocol Sync");
                    if subscribed {
                        break; // Leave the same connection's probe unanswered.
                    }
                    // SQLx may ping its private pool connection before LISTEN.
                    stream.write_all(b"Z\0\0\0\x05I").await?;
                }
                _ => anyhow::bail!("unexpected frontend message {kind}"),
            }
        }
        started_tx.send(()).unwrap();
        let mut byte = [0];
        assert_eq!(
            stream.read(&mut byte).await?,
            0,
            "discard must close the socket, not spawn UNLISTEN/Terminate cleanup"
        );
        Ok(())
    });
    Ok((url, started_rx, task))
}

#[tokio::test]
async fn stalled_probe_discards_the_same_sqlx_connection_without_async_cleanup() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (url, started, peer) = wire_peer(Failure::Probe).await?;
        let (tx, _rx) = broadcast::channel(4);
        let notifier = PostgresListener::new(url, tx).await?;
        let mut listener = notifier.create_listener().await?;
        {
            let probe = ListenConnection::ping(&mut listener);
            tokio::pin!(probe);
            tokio::select! {
                result = &mut probe => anyhow::bail!("probe unexpectedly completed: {result:?}"),
                result = started => result?,
            }
            assert!(tokio::time::timeout(Duration::from_millis(1), &mut probe)
                .await
                .is_err());
        }
        drop(listener);
        peer.await??;
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

#[tokio::test]
async fn cancelled_subscription_discards_sqlx_connection_without_async_cleanup() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (url, started, peer) = wire_peer(Failure::Subscription).await?;
        let (tx, _rx) = broadcast::channel(4);
        let notifier = PostgresListener::new(url, tx).await?;
        {
            let connection = notifier.create_listener();
            tokio::pin!(connection);
            tokio::select! {
                result = &mut connection => anyhow::bail!("subscription unexpectedly completed: {result:?}"),
                result = started => result?,
            }
        }
        peer.await??;
        Ok::<_, anyhow::Error>(())
    }).await?
}

#[tokio::test]
async fn detected_io_failure_discards_sqlx_connection_before_listener_drop() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (url, started, peer) = wire_peer(Failure::Receive).await?;
        let (tx, _rx) = broadcast::channel(4);
        let notifier = PostgresListener::new(url, tx).await?;
        let mut listener = notifier.create_listener().await?;
        started.await?;
        assert!(ListenConnection::recv(&mut listener).await.is_err());
        peer.await??;
        drop(listener);
        Ok::<_, anyhow::Error>(())
    })
    .await?
}
