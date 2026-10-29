//! Часы по NTP для зеркалирования (AirPlay 1): отправитель ждёт обмена
//! метками времени, прежде чем начать поток. Мы и спрашиваем его время, и
//! отвечаем на его запросы, как UxPlay.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tracing::debug;

/// Секунды между эпохами NTP (1900) и UNIX (1970).
const NTP_UNIX_OFFSET: u64 = 2_208_988_800;
const REQUEST: u8 = 0x52;
const REPLY: u8 = 0x53;

fn ntp_now() -> [u8; 8] {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = (now.as_secs() + NTP_UNIX_OFFSET) as u32;
    let frac = ((now.subsec_nanos() as u64) << 32) / 1_000_000_000;
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&secs.to_be_bytes());
    out[4..].copy_from_slice(&(frac as u32).to_be_bytes());
    out
}

fn request() -> [u8; 32] {
    let mut req = [0u8; 32];
    req[..4].copy_from_slice(&[0x80, 0x80 | REQUEST, 0x00, 0x07]);
    req[24..32].copy_from_slice(&ntp_now());
    req
}

/// Ответ на запрос: его время отправки — в «origin», наше — в «receive» и
/// «transmit».
pub fn reply(req: &[u8]) -> Option<[u8; 32]> {
    if req.len() < 32 || req[1] & 0x7F != REQUEST {
        return None;
    }
    let mut resp = [0u8; 32];
    resp.copy_from_slice(&req[..32]);
    resp[1] = 0x80 | REPLY;
    resp[8..16].copy_from_slice(&req[24..32]);
    let now = ntp_now();
    resp[16..24].copy_from_slice(&now);
    resp[24..32].copy_from_slice(&now);
    Some(resp)
}

/// Обмен временем с отправителем, у которого порт времени `remote`.
/// Возвращает наш порт (его отдаём в `timingPort`) и задачу.
pub fn start(remote: SocketAddr) -> io::Result<(u16, JoinHandle<()>)> {
    let any = match remote.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    let socket = std::net::UdpSocket::bind((any, 0))?;
    socket.set_nonblocking(true)?;
    let socket = UdpSocket::from_std(socket)?;
    let port = socket.local_addr()?.port();
    let task = tokio::spawn(async move {
        let mut buf = [0u8; 128];
        let mut tick = tokio::time::interval(Duration::from_secs(3));
        loop {
            tokio::select! {
                r = socket.recv_from(&mut buf) => match r {
                    Ok((n, from)) => {
                        if let Some(resp) = reply(&buf[..n]) {
                            let _ = socket.send_to(&resp, from).await;
                        }
                    }
                    Err(e) => {
                        debug!("NTP: {e}");
                        return;
                    }
                },
                _ = tick.tick() => {
                    if remote.port() != 0 {
                        let _ = socket.send_to(&request(), remote).await;
                    }
                }
            }
        }
    });
    Ok((port, task))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_echoes_origin() {
        let req = request();
        let resp = reply(&req).unwrap();
        assert_eq!(resp[1] & 0x7F, REPLY);
        assert_eq!(resp[8..16], req[24..32]);
        assert!(reply(&resp).is_none());
    }
}
