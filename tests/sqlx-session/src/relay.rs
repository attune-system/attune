use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};
// Holds the first transactional ReadyForQuery only after the server has
// accepted BEGIN. No timing sleeps, SQL comments, or driver counters.
pub async fn relay(
    target: String,
    status: u8,
    occurrence: usize,
) -> (
    u16,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (seen_tx, seen) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut client, _) = listener.accept().await.unwrap();
        let mut server = TcpStream::connect(target).await.unwrap();
        let (mut cr, mut cw) = client.split();
        let (mut sr, mut sw) = server.split();
        let up = async {
            let _ = tokio::io::copy(&mut cr, &mut sw).await;
        };
        let down = async {
            let mut gate = Some((seen_tx, release_rx));
            let mut matches = 0;
            while let Ok(kind) = sr.read_u8().await {
                let len = sr.read_u32().await.unwrap();
                let mut body = vec![0; len as usize - 4];
                sr.read_exact(&mut body).await.unwrap();
                if kind == b'Z' && body == [status] {
                    matches += 1;
                    if matches == occurrence {
                        if let Some((seen, release)) = gate.take() {
                            seen.send(()).unwrap();
                            release.await.unwrap();
                        }
                    }
                }
                let mut frame = Vec::with_capacity(len as usize + 1);
                frame.push(kind);
                frame.extend_from_slice(&len.to_be_bytes());
                frame.extend_from_slice(&body);
                if cw.write_all(&frame).await.is_err() {
                    break;
                }
            }
        };
        tokio::select! { _ = up => (), _ = down => () }
    });
    (port, seen, release, task)
}
