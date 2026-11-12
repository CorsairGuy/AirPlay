//! Bounded asynchronous source pump; it does not open sessions or stamp clocks.
use crate::{
    PACKET_BYTES,
    continuous::{Input, Step, Timeline},
};
use anyhow::{Result, bail};
use std::time::Instant;
use tokio::sync::{mpsc, watch};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub generation: u64,
    pub paused: bool,
}
pub struct SourceFrame {
    pub generation: u64,
    pub pcm: Box<[u8; PACKET_BYTES]>,
}
/// Stamp once with the existing MediaClock/PTP producer, then fan out. Keep
/// generation when integrating downstream queues; it is not a new RTP epoch.
pub struct ScheduledFrame {
    pub frame_index: u64,
    pub generation: u64,
    pub pcm: Box<[u8; PACKET_BYTES]>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum End {
    Eof,
    Stopped,
    PauseExpired,
}

fn stopped(stop: &watch::Receiver<bool>) -> bool {
    *stop.borrow() || stop.has_changed().is_err()
}
async fn wait_stop(stop: &mut watch::Receiver<bool>) {
    loop {
        if stopped(stop) {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}
fn state(timeline: &mut Timeline, controls: &mut watch::Receiver<State>) -> Result<State> {
    let next = *controls.borrow_and_update();
    timeline.apply_source_state(next.generation, next.paused, Instant::now())?;
    Ok(next)
}

/// Retain PCM while waiting for its slot; controls use a separate priority watch
/// so a decoder backlog cannot delay Pause. Keep input Sender alive between
/// tracks; closing it means final source EOF. Cancellation bypasses drain.
/// No unbounded channels, detached tasks or input reads hidden behind timeouts.
pub async fn run(
    mut timeline: Timeline,
    mut input: mpsc::Receiver<SourceFrame>,
    mut controls: watch::Receiver<State>,
    output: mpsc::Sender<ScheduledFrame>,
    mut stop: watch::Receiver<bool>,
) -> Result<End> {
    if input.max_capacity() > 512 || output.max_capacity() > 512 {
        bail!("continuous source queue capacity exceeds512");
    }
    let mut pending: Option<SourceFrame> = None;
    loop {
        if stopped(&stop) || controls.has_changed().is_err() {
            return Ok(End::Stopped);
        }
        state(&mut timeline, &mut controls)?;
        if pending.is_none() {
            match input.try_recv() {
                Ok(frame) => pending = Some(frame),
                Err(mpsc::error::TryRecvError::Disconnected) => return Ok(End::Eof),
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
        }
        let next = timeline.poll(
            Instant::now(),
            pending.as_ref().map(|frame| Input {
                generation: frame.generation,
                pcm: &*frame.pcm,
            }),
        )?;
        match next {
            Step::PauseExpired => return Ok(End::PauseExpired),
            Step::DiscardStale | Step::Skip { .. } => {
                pending = None;
                // Bounded cooperative catch-up after a long scheduler stall.
                tokio::task::yield_now().await;
            }
            Step::Frame {
                frame_index,
                generation,
                pcm,
            } => {
                pending = None;
                let mut frame = ScheduledFrame {
                    frame_index,
                    generation,
                    pcm,
                };
                let deadline = tokio::time::Instant::now() + timeline.output_deadline();
                loop {
                    if stopped(&stop) || controls.has_changed().is_err() {
                        return Ok(End::Stopped);
                    }
                    let latest = state(&mut timeline, &mut controls)?;
                    if latest.paused || latest.generation != frame.generation {
                        frame.pcm.fill(0);
                        frame.generation = latest.generation;
                    }
                    tokio::select! {
                        biased;
                        _ = wait_stop(&mut stop) => return Ok(End::Stopped),
                        changed = controls.changed() => {
                            if changed.is_err() { return Ok(End::Stopped); }
                        },
                        permit = tokio::time::timeout_at(deadline, output.reserve()) => {
                            permit.map_err(|_| anyhow::anyhow!("continuous output stalled"))?
                                .map_err(|_| anyhow::anyhow!("continuous output closed"))?.send(frame);
                            break;
                        },
                    }
                }
            }
            Step::Wait(until) => {
                tokio::select! {
                    biased;
                    _ = wait_stop(&mut stop) => return Ok(End::Stopped),
                    changed = controls.changed() => {
                        if changed.is_err() { return Ok(End::Stopped); }
                    },
                    frame = input.recv(), if pending.is_none() => {
                        match frame { Some(frame) => pending = Some(frame), None => return Ok(End::Eof) }
                    },
                    _ = tokio::time::sleep_until(until.into()) => {},
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn timeline() -> Timeline {
        Timeline::new(Instant::now(), Duration::from_millis(250)).unwrap()
    }
    async fn received(rx: &mut mpsc::Receiver<ScheduledFrame>) -> ScheduledFrame {
        tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap()
    }
    #[tokio::test]
    async fn gap_silence_then_content_share_frame_timeline_and_eof() {
        let (source, input) = mpsc::channel(4);
        let (_state, controls) = watch::channel(State::default());
        let (_stop, stop) = watch::channel(false);
        let (output, mut frames) = mpsc::channel(16);
        let task = tokio::spawn(run(timeline(), input, controls, output, stop));
        let silence = received(&mut frames).await;
        assert!(silence.pcm.iter().all(|byte| *byte == 0));
        source
            .send(SourceFrame {
                generation: 0,
                pcm: Box::new([9; PACKET_BYTES]),
            })
            .await
            .unwrap();
        let content = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let frame = frames.recv().await.unwrap();
                if frame.pcm[0] == 9 {
                    break frame;
                }
            }
        })
        .await
        .unwrap();
        assert!(content.frame_index > silence.frame_index);
        assert_eq!(content.frame_index % crate::FRAMES_PER_PACKET as u64, 0);
        drop(source);
        assert_eq!(task.await.unwrap().unwrap(), End::Eof);
    }
    #[tokio::test]
    async fn pause_and_generation_control_are_not_queued_behind_pcm() {
        let (source, input) = mpsc::channel(4);
        source
            .send(SourceFrame {
                generation: 0,
                pcm: Box::new([8; PACKET_BYTES]),
            })
            .await
            .unwrap();
        let (state, controls) = watch::channel(State {
            generation: 1,
            paused: true,
        });
        let (_stop, stop) = watch::channel(false);
        let (output, mut frames) = mpsc::channel(16);
        let task = tokio::spawn(run(timeline(), input, controls, output, stop));
        let silence = received(&mut frames).await;
        assert_eq!(silence.generation, 1);
        assert!(silence.pcm.iter().all(|byte| *byte == 0));
        state.send_replace(State {
            generation: 2,
            paused: false,
        });
        source
            .send(SourceFrame {
                generation: 2,
                pcm: Box::new([7; PACKET_BYTES]),
            })
            .await
            .unwrap();
        let content = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let frame = frames.recv().await.unwrap();
                if frame.pcm[0] != 0 {
                    break frame;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(content.generation, 2);
        assert_eq!(content.pcm[0], 7);
        assert!(content.frame_index > silence.frame_index);
        drop(source);
        assert_eq!(task.await.unwrap().unwrap(), End::Eof);
    }
    #[tokio::test]
    async fn stopped_or_stalled_output_cannot_leave_pump_hanging() {
        for cancel in [true, false] {
            let (_source, input) = mpsc::channel(1);
            let (_state, controls) = watch::channel(State {
                generation: 0,
                paused: true,
            });
            let (stop, stopped) = watch::channel(false);
            let (output, mut frames) = mpsc::channel(1);
            let task = tokio::spawn(run(timeline(), input, controls, output, stopped));
            let _first = received(&mut frames).await;
            // Leave the next slots unconsumed: output fills then blocks.
            tokio::time::sleep(Duration::from_millis(30)).await;
            if cancel {
                stop.send_replace(true);
            }
            let result = tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap();
            if cancel {
                assert_eq!(result.unwrap(), End::Stopped);
            } else {
                assert!(result.unwrap_err().to_string().contains("output stalled"));
            }
        }
    }
    #[tokio::test]
    async fn control_changes_mute_the_frame_waiting_for_output_capacity() {
        let (source, input) = mpsc::channel(4);
        for _ in 0..2 {
            source
                .send(SourceFrame {
                    generation: 0,
                    pcm: Box::new([7; PACKET_BYTES]),
                })
                .await
                .unwrap();
        }
        let (state, controls) = watch::channel(State::default());
        let (_stop, stopped) = watch::channel(false);
        let (output, mut frames) = mpsc::channel(1);
        let queued = output.clone();
        let task = tokio::spawn(run(timeline(), input, controls, output, stopped));
        tokio::time::timeout(Duration::from_secs(1), async {
            while queued.capacity() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        state.send_replace(State {
            generation: 1,
            paused: true,
        });
        let old = received(&mut frames).await;
        assert_eq!(old.generation, 0); // Already delivered queue needs its own generation guard.
        let blocked = received(&mut frames).await;
        assert_eq!(blocked.generation, 1);
        assert!(blocked.pcm.iter().all(|byte| *byte == 0));
        drop(source);
        assert_eq!(task.await.unwrap().unwrap(), End::Eof);
    }
    #[tokio::test]
    async fn closed_control_owner_stops_source_and_backwards_generation_fails() {
        for backwards in [false, true] {
            let (_source, input) = mpsc::channel(1);
            let (state, controls) = watch::channel(State {
                generation: 2,
                paused: true,
            });
            let (_stop, stopped) = watch::channel(false);
            let (output, mut frames) = mpsc::channel(4);
            let task = tokio::spawn(run(timeline(), input, controls, output, stopped));
            received(&mut frames).await;
            if backwards {
                state.send_replace(State::default());
            } else {
                drop(state);
            }
            let result = tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap();
            if backwards {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap(), End::Stopped);
            }
        }
    }
}
