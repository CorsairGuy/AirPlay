//! `Control` станции с тем же API, что видит `server.rs`. На станции команды
//! уходят в `airplay_control` по сокету; здесь — отправителю AirPlay через
//! [`Remote`]. Номера команд и проверки — как в `control.rs` станции.
use crate::remote::{Command, Remote};
use crate::station_mrp::model::Feed;
use anyhow::{Result, bail};
use tokio::sync::watch;

#[derive(Clone)]
pub struct Control {
    pub feed: watch::Sender<Feed>,
    remote: Remote,
}

impl Control {
    pub fn new(feed: watch::Sender<Feed>, remote: Remote) -> Self {
        Self { feed, remote }
    }

    pub async fn volume(&self, value: f32) -> Result<()> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            bail!("invalid volume")
        }
        self.feed.send_modify(|feed| feed.volume = value);
        self.remote.send(Command::Volume(value));
        Ok(())
    }

    pub async fn action(&self, command: u64, position: Option<f64>) -> Result<()> {
        if command == 45 {
            let position = position
                .filter(|v| v.is_finite() && *v >= 0.0)
                .ok_or_else(|| anyhow::anyhow!("seek position"))?;
            if !self.remote.send(Command::Seek(position)) {
                bail!("seek not accepted")
            }
            return Ok(());
        }
        let feed = self.feed.borrow().clone();
        let command = match command {
            1 => Command::Play,
            2 => Command::Pause,
            3 => {
                if feed.playing {
                    Command::Pause
                } else {
                    Command::Play
                }
            }
            // AirPlay-источник станции превращает stop в pause.
            4 => Command::Pause,
            5 => Command::Next,
            6 => Command::Previous,
            _ => bail!("unsupported command"),
        };
        if !self.remote.send(command) {
            bail!("AirPlay source rejected command")
        }
        Ok(())
    }
}
