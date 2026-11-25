//! Приём TCP-соединений отправителя и разбор байтов сессией.
//!
//! Транспорт не знает ни о Bonjour, ни о выводе звука: он принимает
//! соединения, отдаёт байты [`Session`] и пишет ответы обратно.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tracing::{info, warn};

use crate::session::{Context, Session};

/// Принимает соединения, пока `shutdown` не станет `true`.
///
/// Соединения принадлежат этому вызову (`JoinSet`): при остановке они
/// обрываются вместе с ним и не задевают другие экземпляры приёмника в том
/// же процессе. `Context` уже свой у каждого экземпляра.
pub async fn accept_loop(
    listener: TcpListener,
    ctx: Arc<Context>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut sessions = JoinSet::new();
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow_and_update() {
                    break;
                }
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, addr)) => {
                    sessions.spawn(serve_peer(stream, ctx.clone(), addr));
                }
                Err(e) => warn!("accept: {e}"),
            },
            Some(_) = sessions.join_next(), if !sessions.is_empty() => {}
        }
    }
    sessions.shutdown().await;
}

async fn serve_peer(stream: TcpStream, ctx: Arc<Context>, addr: SocketAddr) {
    let peer = addr.to_string();
    info!(%peer, "подключение");
    match serve_connection(stream, ctx, addr).await {
        Ok(()) => info!(%peer, "отключение"),
        Err(e) => warn!(%peer, "соединение закрыто: {e:#}"),
    }
}

pub async fn serve_connection(
    mut stream: TcpStream,
    ctx: Arc<Context>,
    peer: SocketAddr,
) -> Result<()> {
    stream.set_nodelay(true)?;
    let local = stream.local_addr()?.ip();
    let mut session = Session::new(ctx, peer, local);
    let mut buf = vec![0u8; 16 * 1024];
    let mut out = Vec::new();
    loop {
        let n = read_more(
            &mut stream,
            &mut buf,
            session.pending_request(),
            Duration::from_secs(60),
        )
        .await?;
        if n == 0 {
            return Ok(());
        }
        session.on_data(&buf[..n], &mut out)?;
        if !out.is_empty() {
            stream.write_all(&out).await?;
            out.clear();
        }
    }
}

async fn read_more<S: tokio::io::AsyncRead + Unpin>(
    stream: &mut S,
    buf: &mut [u8],
    partial: bool,
    deadline: Duration,
) -> std::io::Result<usize> {
    if partial {
        tokio::time::timeout(deadline, stream.read(buf)).await?
    } else {
        stream.read(buf).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_partial_requests_are_timed_out() {
        let (mut client, mut server) = tokio::io::duplex(64);
        let mut buf = [0u8; 64];
        let error = read_more(&mut server, &mut buf, true, Duration::from_millis(20))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        client.write_all(b"next").await.unwrap();
        assert_eq!(
            read_more(&mut server, &mut buf, false, Duration::from_millis(20))
                .await
                .unwrap(),
            4
        );
    }
}
