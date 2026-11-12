//! Source control for opt-in continuous relay. No processes/devices or clocks.
use crate::{mrp_service::model::Feed, remote::Command};
use anyhow::{Result, bail};
use sender::continuous_source::State;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;

const BETWEEN_TRACKS: Duration = Duration::from_millis(1500);
#[derive(Default)]
struct Policy {
    state: State,
    content: Option<String>,
    not_playing: Option<Instant>,
    commands: u64,
}
#[derive(Clone)]
pub struct SourceControl {
    policy: Arc<Mutex<Policy>>,
    states: watch::Sender<State>,
}
impl Default for SourceControl {
    fn default() -> Self {
        Self {
            policy: Default::default(),
            states: watch::channel(State::default()).0,
        }
    }
}
impl SourceControl {
    pub fn subscribe(&self) -> watch::Receiver<State> {
        self.states.subscribe()
    }
    fn change(&self, operation: impl FnOnce(&mut Policy) -> Result<()>) -> Result<()> {
        let mut policy = self
            .policy
            .lock()
            .map_err(|_| anyhow::anyhow!("source control lock failed"))?;
        operation(&mut policy)?;
        let next = policy.state;
        self.states.send_if_modified(|state| {
            if *state == next {
                false
            } else {
                *state = next;
                true
            }
        });
        Ok(())
    }
    /// Called only AFTER local command-queue acceptance. This is not an ack
    /// from the remote player; later source observations may override it.
    pub fn accepted(&self, command: Command) -> Result<()> {
        if matches!(command, Command::Seek(position) if !position.is_finite() || position < 0.0) {
            bail!("invalid accepted seek position");
        }
        self.change(|p| {
            p.commands = p
                .commands
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("source command revision exhausted"))?;
            match command {
                Command::Play => {
                    p.state.paused = false;
                    p.not_playing = None;
                }
                Command::Pause | Command::Stop => {
                    p.state.paused = true;
                    p.not_playing = None;
                }
                Command::PlayPause => {
                    p.state.paused = !p.state.paused;
                    p.not_playing = None;
                }
                Command::Seek(_) | Command::Next | Command::Previous => {
                    p.state.generation = p
                        .state
                        .generation
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("source generation exhausted"))?;
                }
                _ => {}
            }
            Ok(())
        })
    }
    pub fn observe(&self, playing: bool, content: &str, now: Instant) -> Result<()> {
        self.observe_revision(None, playing, content, now)
    }
    fn observe_revision(
        &self,
        expected: Option<u64>,
        playing: bool,
        content: &str,
        now: Instant,
    ) -> Result<()> {
        if content.len() > 1024 {
            bail!("source content identifier too long");
        }
        self.change(|p| {
            if expected.is_some_and(|revision| revision != p.commands) {
                return Ok(());
            }
            if !content.is_empty() {
                if p.content.as_deref().is_some_and(|old| old != content) {
                    p.state.generation = p
                        .state
                        .generation
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("source generation exhausted"))?;
                }
                p.content = Some(content.to_owned());
            }
            if playing {
                p.state.paused = false;
                p.not_playing = None;
            } else if !p.state.paused {
                p.not_playing.get_or_insert(now);
            }
            Ok(())
        })?;
        self.tick(now)
    }
    pub fn tick(&self, now: Instant) -> Result<()> {
        self.change(|p| {
            if p.not_playing.is_some_and(|since| {
                now.checked_duration_since(since)
                    .is_some_and(|age| age >= BETWEEN_TRACKS)
            }) {
                p.state.paused = true;
                p.not_playing = None;
            }
            Ok(())
        })
    }
    /// Explicitly awaited by the application alongside its group runtime; no
    /// hidden task. Share the group stop watch. Timer advances the grace period
    /// even if metadata goes quiet. Duplicate state does not invalidate PCM.
    pub async fn follow(
        &self,
        mut feed: watch::Receiver<Feed>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<()> {
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if *stop.borrow() || stop.has_changed().is_err() {
                return Ok(());
            }
            let revision = self
                .policy
                .lock()
                .map_err(|_| anyhow::anyhow!("source control lock failed"))?
                .commands;
            let (playing, content) = {
                let value = feed.borrow_and_update();
                (value.playing, value.content_id.clone())
            };
            self.observe_revision(Some(revision), playing, &content, Instant::now())?;
            tokio::select! {
                biased;
                changed = stop.changed() => { if changed.is_err() { return Ok(()); } },
                changed = feed.changed() => { if changed.is_err() { return Ok(()); } },
                _ = tick.tick() => self.tick(Instant::now())?,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transient_gap_and_explicit_pause_have_different_semantics() {
        let control = SourceControl::default();
        let mut states = control.subscribe();
        let now = Instant::now();
        control.observe(true, "a", now).unwrap();
        states.borrow_and_update();
        control.observe(false, "a", now).unwrap();
        control.tick(now + Duration::from_millis(1499)).unwrap();
        assert!(!states.borrow().paused);
        assert!(!states.has_changed().unwrap());
        control
            .observe(true, "b", now + Duration::from_millis(1500))
            .unwrap();
        assert_eq!(states.borrow().generation, 1);
        assert!(!states.borrow().paused);
        control.accepted(Command::Pause).unwrap();
        assert!(states.borrow().paused);
        control
            .observe_revision(Some(0), true, "old snapshot", now)
            .unwrap();
        assert!(
            states.borrow().paused,
            "observation captured before the command must not undo it"
        );
        // External Play is never locked out by an earlier controller command.
        control
            .observe(true, "b", now + Duration::from_millis(1501))
            .unwrap();
        assert!(!states.borrow().paused);
    }
    #[test]
    fn sustained_pause_duplicates_and_seek_do_not_reset_timeline_policy() {
        let control = SourceControl::default();
        let mut states = control.subscribe();
        let now = Instant::now();
        control.observe(false, "a", now).unwrap();
        control.tick(now + BETWEEN_TRACKS).unwrap();
        assert!(states.borrow().paused);
        states.borrow_and_update();
        control
            .observe(false, "a", now + Duration::from_secs(10))
            .unwrap();
        assert!(!states.has_changed().unwrap());
        control.accepted(Command::Seek(30.0)).unwrap();
        assert_eq!(states.borrow().generation, 1);
        assert!(states.borrow().paused);
        control.accepted(Command::Play).unwrap();
        assert!(!states.borrow().paused);
        assert_eq!(states.borrow().generation, 1);
    }
    #[tokio::test]
    async fn feed_follower_observes_external_play_and_stops_explicitly() {
        let control = SourceControl::default();
        control.accepted(Command::Pause).unwrap();
        let (feed, observed) = watch::channel(Feed::default());
        let (stop, stopped) = watch::channel(false);
        let mut states = control.subscribe();
        states.borrow_and_update();
        let follower = control.clone();
        let task = tokio::spawn(async move { follower.follow(observed, stopped).await });
        feed.send_modify(|feed| {
            feed.playing = true;
            feed.content_id = "track".into();
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while states.borrow_and_update().paused {
                states.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(control.accepted(Command::Seek(f64::NAN)).is_err());
    }
}
