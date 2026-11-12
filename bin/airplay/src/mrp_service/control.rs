//! `Control` станции с тем же API, что видит `server.rs`. На станции команды
//! уходят в `airplay_control` по сокету; здесь — отправителю AirPlay через
//! [`Remote`]. Номера команд и проверки — как в `control.rs` станции.
use crate::mrp_service::model::Feed;
use crate::remote::{Command, Remote};
use anyhow::{Result, bail};
use tokio::sync::watch;

#[derive(Clone)]
pub struct Control {
    pub feed: watch::Sender<Feed>,
    remote: Remote,
    source: Option<crate::continuous_control::SourceControl>,
    pub(crate) outputs: Option<crate::output_control::OutputControl>,
    pub(crate) groups: std::sync::Arc<GroupControlState>,
}

#[derive(Default)]
pub(crate) struct GroupControlState {
    policy: std::sync::Mutex<sender::group_volume::GroupVolume>,
}

impl Control {
    pub fn new(feed: watch::Sender<Feed>, remote: Remote) -> Self {
        Self {
            feed,
            remote,
            source: None,
            outputs: None,
            groups: Default::default(),
        }
    }

    pub fn with_outputs(mut self, outputs: crate::output_control::OutputControl) -> Self {
        self.outputs = Some(outputs);
        self
    }
    pub fn with_continuous_source(
        mut self,
        source: crate::continuous_control::SourceControl,
    ) -> Self {
        self.source = Some(source);
        self
    }

    pub fn group_level(&self, feed: &Feed) -> f32 {
        feed.outputs
            .iter()
            .filter_map(|output| output.volume)
            .fold(feed.volume, f32::max)
    }

    /// Enqueue synchronously in MRP reader order. Compute against levels left
    /// by preceding operations, then commit gain/cache before acknowledging.
    pub fn group_volume(
        &self,
        local_uid: String,
        value: f32,
    ) -> impl std::future::Future<Output = Result<()>> + Send + 'static {
        let control = self.clone();
        let queued =
            self.outputs
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("output control unavailable"))
                .and_then(|queue| {
                    queue
                        .submit_operation(Box::new(move |backend, catalog| {
                            let operation = (|| -> Result<()> {
                                if !control.remote.connected() {
                                    bail!("local source control unavailable");
                                }
                                let mut policy =
                                    control.groups.policy.lock().map_err(|_| {
                                        anyhow::anyhow!("group balance lock failed")
                                    })?;
                                let feed = control.feed.borrow().clone();
                                let mut levels = vec![(local_uid, feed.volume)];
                                for output in &feed.outputs {
                                    let uid = output
                                        .descriptor
                                        .string(crate::mrp_service::model::f(
                                            "AVOutputDeviceDescriptor",
                                            "uniqueIdentifier",
                                        ))
                                        .ok_or_else(|| anyhow::anyhow!("output UID missing"))?;
                                    levels.push((
                                        uid.into(),
                                        output.volume.ok_or_else(|| {
                                            anyhow::anyhow!("observed output level unavailable")
                                        })?,
                                    ));
                                }
                                let mut candidate = policy.clone();
                                let planned = candidate.plan(&levels, value)?;
                                let gains: Result<Vec<_>> = planned
                                    .iter()
                                    .skip(1)
                                    .map(|(uid, value)| {
                                        sender::Volume::normalized(*value)
                                            .map(|gain| (uid.clone(), gain))
                                    })
                                    .collect();
                                let gains = gains?;
                                if !gains.is_empty() {
                                    backend.set_routes(catalog, &gains)?;
                                }
                                if !control.remote.send(Command::Volume(planned[0].1)) {
                                    bail!("local source disconnected during group change");
                                }
                                control.feed.send_if_modified(|current| {
                                    let before = current.clone();
                                    current.volume = planned[0].1;
                                    for output in &mut current.outputs {
                                        if let Some(uid) =
                                            output.descriptor.string(crate::mrp_service::model::f(
                                                "AVOutputDeviceDescriptor",
                                                "uniqueIdentifier",
                                            ))
                                        {
                                            if let Some((_, value)) =
                                                planned.iter().skip(1).find(|(id, _)| {
                                                    crate::mrp_service::model::same_uid(id, uid)
                                                })
                                            {
                                                output.volume = Some(*value);
                                            }
                                        }
                                    }
                                    *current != before
                                });
                                *policy = candidate;
                                Ok(())
                            })();
                            if operation.is_ok() { 0 } else { 2 }
                        }))
                        .map_err(|_| anyhow::anyhow!("group volume queue full"))
                });
        async move {
            let code = tokio::time::timeout(std::time::Duration::from_secs(10), queued?).await??;
            if code != 0 {
                bail!("group volume rejected");
            }
            Ok(())
        }
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
            if let Some(source) = &self.source {
                source.accepted(Command::Seek(position))?;
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
        if let Some(source) = &self.source {
            source.accepted(command)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[tokio::test]
    async fn continuous_source_changes_only_after_command_queue_acceptance() {
        let remote = Remote::new();
        let mut commands = remote.attach(1);
        let (feed, _) = watch::channel(Feed::default());
        let source = crate::continuous_control::SourceControl::default();
        let states = source.subscribe();
        let control = Control::new(feed, remote).with_continuous_source(source);
        control.action(2, None).await.unwrap();
        assert!(states.borrow().paused);
        assert_eq!(commands.recv().await, Some(Command::Pause));
        drop(commands); // closed remote queue must reject Play without a state change
        assert!(control.action(1, None).await.is_err());
        assert!(states.borrow().paused);
        let mut commands = control.remote.attach(2);
        control.action(1, None).await.unwrap();
        assert!(!states.borrow().paused);
        assert_eq!(commands.recv().await, Some(Command::Play));
        control.action(45, Some(30.0)).await.unwrap();
        assert_eq!(states.borrow().generation, 1);
        assert_eq!(commands.recv().await, Some(Command::Seek(30.0)));
        assert!(control.action(45, Some(f64::NAN)).await.is_err());
        assert_eq!(states.borrow().generation, 1);
    }
    #[tokio::test]
    async fn group_individual_group_preserves_reader_order_and_fresh_levels() {
        use crate::{
            mrp_service::{model::RemoteOutput, wire::Node},
            output_control::{OutputControl, testing},
        };
        let remote = Remote::new();
        let mut commands = remote.attach(1);
        let (feed, _) = watch::channel(Feed {
            volume: 0.1,
            outputs: vec![RemoteOutput {
                descriptor: Node::default().text(2, "pod").clone(),
                volume: Some(0.3),
            }],
            ..Default::default()
        });
        let (queue, mut requests) = OutputControl::channel(4);
        let control = Control::new(feed.clone(), remote).with_outputs(queue.clone());
        let first = control.group_volume("local".into(), 0.2);
        let individual = queue
            .submit_volume_after(
                "pod".into(),
                0.5,
                Box::new({
                    let feed = feed.clone();
                    move |code| {
                        if code == 0 {
                            feed.send_modify(|s| s.outputs[0].volume = Some(0.5));
                        }
                    }
                }),
            )
            .unwrap();
        let last = control.group_volume("local".into(), 0.3);
        let backend = testing::Backend::default();
        testing::execute(requests.recv().await.unwrap(), &backend);
        let request = requests.recv().await.unwrap();
        assert_eq!(request.volume, Some(("pod".into(), 0.5)));
        request.finish(0);
        testing::execute(requests.recv().await.unwrap(), &backend);
        first.await.unwrap();
        assert_eq!(individual.await.unwrap(), 0);
        last.await.unwrap();
        let Command::Volume(first) = commands.recv().await.unwrap() else {
            panic!("wrong local command")
        };
        let Command::Volume(last) = commands.recv().await.unwrap() else {
            panic!("wrong local command")
        };
        assert!((first - 0.2 / 3.0).abs() < 1e-6);
        assert!((last - 0.04).abs() < 1e-6);
        assert!((feed.borrow().volume - 0.04).abs() < 1e-6);
        assert_eq!(feed.borrow().outputs[0].volume, Some(0.3));
    }
    #[tokio::test]
    async fn accepted_pause_freezes_clock_immediately_and_play_resumes() {
        let remote = Remote::new();
        let mut commands = remote.attach(7);
        let (feed, _) = watch::channel(Feed {
            playing: true,
            position: 10.0,
            position_sampled: Instant::now() - Duration::from_secs(5),
            ..Default::default()
        });
        let control = Control::new(feed.clone(), remote);
        control.action(2, None).await.unwrap();
        assert_eq!(commands.recv().await, Some(Command::Pause));
        let paused = feed.borrow().clone();
        assert!(!paused.playing);
        assert!((paused.position - 15.0).abs() < 0.2);
        assert_eq!(
            paused.position_at(Instant::now() + Duration::from_secs(30)),
            paused.position
        );
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

    // Scenarios informed by AppleTVControlLibrary/pyatv player-state tests:
    // paused elapsed time and independent metadata/state updates. This is a
    // server-side regression, not a port of their multi-player client model.
    #[tokio::test]
    async fn remote_pause_then_source_play_keeps_metadata_and_resumes_timeline() {
        let remote = Remote::new();
        let mut commands = remote.attach(8);
        let (feed, _) = watch::channel(Feed {
            title: "Current track".into(),
            content_id: "track-1".into(),
            playing: true,
            position: 20.0,
            duration: Some(180.0),
            ..Default::default()
        });
        let control = Control::new(feed.clone(), remote);
        control.action(2, None).await.unwrap();
        assert_eq!(commands.recv().await, Some(Command::Pause));
        let paused = feed.borrow().clone();
        assert_eq!(
            paused.position_at(paused.position_sampled + Duration::from_secs(20)),
            paused.position
        );
        // A subsequent sender notification must override optimistic pause;
        // there must be no pending-command lockout of the other controller.
        feed.send_modify(|state| state.set_playing(true));
        let resumed = feed.borrow().clone();
        assert_eq!(resumed.title, "Current track");
        assert_eq!(resumed.content_id, "track-1");
        assert_eq!(resumed.duration, Some(180.0));
        assert_eq!(
            resumed.position_at(resumed.position_sampled + Duration::from_secs(2)),
            resumed.position + 2.0
        );
        control.action(3, None).await.unwrap();
        assert_eq!(commands.recv().await, Some(Command::Pause));
        control.action(3, None).await.unwrap();
        assert_eq!(commands.recv().await, Some(Command::Play));
    }
}
