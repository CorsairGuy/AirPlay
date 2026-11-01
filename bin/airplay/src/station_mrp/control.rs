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
        match command {
            Command::Play => self.feed.send_modify(|feed| feed.set_playing(true)),
            Command::Pause => self.feed.send_modify(|feed| feed.set_playing(false)),
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[tokio::test]
    async fn accepted_pause_freezes_clock_immediately_and_play_resumes() {
        let remote = Remote::new();
        let mut commands = remote.attach(7);
        let (feed, _) = watch::channel(Feed {
            playing: true, position: 10.0,
            position_sampled: Instant::now() - Duration::from_secs(5),
            ..Default::default()
        });
        let control = Control::new(feed.clone(), remote);
        control.action(2, None).await.unwrap();
        assert_eq!(commands.recv().await, Some(Command::Pause));
        let paused = feed.borrow().clone();
        assert!(!paused.playing);
        assert!((paused.position - 15.0).abs() < 0.2);
        assert_eq!(paused.position_at(Instant::now() + Duration::from_secs(30)), paused.position);
        control.action(1, None).await.unwrap();
        assert_eq!(commands.recv().await, Some(Command::Play));
        assert!(feed.borrow().playing);
    }
    #[tokio::test]
    async fn rejected_command_does_not_change_card() {
        let (feed, _) = watch::channel(Feed::default());
        let control = Control::new(feed.clone(), Remote::new());
        assert!(control.action(1, None).await.is_err());
        assert!(!feed.borrow().playing);
    }
}
