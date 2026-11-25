//! Приём TCP-соединений отправителя и разбор байтов сессией.
//!
//! Транспорт не знает ни о Bonjour, ни о выводе звука: он принимает
//! соединения, отдаёт байты [`Session`] и пишет ответы обратно.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

use crate::session::{Context, Session};

pub async fn accept_loop(listener: TcpListener, ctx: Arc<Context>) {
    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(a) => a,
            Err(e) => {
                warn!("accept: {e}");
                continue;
            }
        };
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let peer = addr.to_string();
            info!(%peer, "подключение");
            match serve_connection(stream, ctx, addr).await {
                Ok(()) => info!(%peer, "отключение"),
                Err(e) => warn!(%peer, "соединение закрыто: {e:#}"),
            }
        });
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
        let n = stream.read(&mut buf).await?;
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
