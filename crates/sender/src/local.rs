//! Platform-neutral local renderer contract. No ALSA, subprocess or device paths.
use crate::AudioPacket;
use anyhow::{Context, Result, bail};
use std::{
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{broadcast, watch},
    time::timeout,
};

/// A platform adapter consumes the same immutable PCM as remote sessions.
/// `play_at` is the presentation time, not the time to enqueue a device write.
/// Implementations account for their own output latency, never modify remote PCM,
/// and release device/process resources on Drop (including future cancellation).
pub trait LocalSink: Send + 'static {
    fn write(
        &mut self,
        packet: Arc<AudioPacket>,
        play_at: Instant,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Drain accepted samples on EOF. Cancellation/stop instead drops the sink.
    fn finish(&mut self) -> impl Future<Output = Result<()>> + Send;
}

/// A failed or slow local renderer fails the group; the platform restores its
/// direct route. This function itself never mutes a device or changes routing.
pub async fn run_local<S: LocalSink>(
    mut sink: S,
    mut packets: broadcast::Receiver<Arc<AudioPacket>>,
    latency: Duration,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    if latency.is_zero() || latency > Duration::from_secs(10) {
        bail!("invalid local latency");
    }
    let deadline = latency + Duration::from_millis(250);
    let mut last_end = None;
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        let packet = tokio::select! {
            value = packets.recv() => match value {
                Ok(packet) => packet,
                Err(broadcast::error::RecvError::Lagged(n)) => bail!("local PCM queue overran by {n} packets"),
                Err(broadcast::error::RecvError::Closed) => {
                    tokio::select! {
                        result = timeout(deadline, sink.finish()) => result.context("local drain deadline exceeded")??,
                        _ = stopped(&mut stop) => return Ok(()),
                    }
                    if let Some(end) = last_end {
                        tokio::select! {
                            _ = tokio::time::sleep_until(end) => {},
                            _ = stopped(&mut stop) => {},
                        }
                    }
                    return Ok(());
                }
            },
            _ = stopped(&mut stop) => return Ok(()),
        };
        let stale = !packet.source_is_current();
        let play_at = packet
            .play_at(latency)
            .context("local presentation time overflow")?;
        if play_at <= Instant::now() {
            if stale {
                continue;
            }
            bail!("local PCM expired before rendering");
        }
        last_end = play_at
            .checked_add(crate::packet::frames_duration(
                crate::FRAMES_PER_PACKET as u64,
            ))
            .map(Into::into);
        let packet = if stale {
            Arc::new(packet.silence())
        } else {
            packet
        };
        tokio::select! {
            result = timeout(deadline, sink.write(packet, play_at)) => result.context("local renderer stalled")??,
            _ = stopped(&mut stop) => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MediaClock, PACKET_BYTES};
    use std::sync::Mutex;
    struct Capture(Arc<Mutex<Vec<(Arc<AudioPacket>, Instant)>>>);
    impl LocalSink for Capture {
        async fn write(&mut self, packet: Arc<AudioPacket>, at: Instant) -> Result<()> {
            self.0.lock().unwrap().push((packet, at));
            Ok(())
        }
        async fn finish(&mut self) -> Result<()> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn local_receives_original_pcm_and_the_remote_presentation_time() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = broadcast::channel(4);
        let (_stop, stopped) = watch::channel(false);
        let packet = Arc::new(MediaClock::now().packet(0, &[0x35; PACKET_BYTES]).unwrap());
        tx.send(packet.clone()).unwrap();
        drop(tx);
        run_local(
            Capture(seen.clone()),
            rx,
            Duration::from_millis(50),
            stopped,
        )
        .await
        .unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(Arc::ptr_eq(&seen[0].0, &packet));
        assert_eq!(
            seen[0].1,
            packet.play_at(Duration::from_millis(50)).unwrap()
        );
        assert!(packet.pcm().iter().all(|b| *b == 0x35));
    }
    #[tokio::test]
    async fn stale_source_renders_silence_at_original_deadline_without_mutating_remote_pcm() {
        let (state, mut revision) = watch::channel(crate::continuous_source::State::default());
        revision.borrow_and_update();
        let packet = Arc::new(
            MediaClock::now()
                .packet(0, &[7; PACKET_BYTES])
                .unwrap()
                .guard_source(revision),
        );
        state.send_replace(crate::continuous_source::State {
            generation: 1,
            paused: false,
        });
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = broadcast::channel(4);
        tx.send(packet.clone()).unwrap();
        drop(tx);
        let (_stop, stopped) = watch::channel(false);
        run_local(
            Capture(seen.clone()),
            rx,
            Duration::from_millis(100),
            stopped,
        )
        .await
        .unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0].1,
            packet.play_at(Duration::from_millis(100)).unwrap()
        );
        assert!(seen[0].0.pcm().iter().all(|byte| *byte == 0));
        assert_eq!(packet.pcm(), &[7; PACKET_BYTES]);
    }
    #[tokio::test]
    async fn queue_overrun_and_expired_packets_fail_instead_of_skipping() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let packet = Arc::new(
            MediaClock::at(Instant::now() - Duration::from_secs(1), 0)
                .packet(0, &[0; PACKET_BYTES])
                .unwrap(),
        );
        let (_stop, stopped) = watch::channel(false);
        let (tx, rx) = broadcast::channel(1);
        tx.send(packet.clone()).unwrap();
        tx.send(packet.clone()).unwrap();
        assert!(
            run_local(
                Capture(seen.clone()),
                rx,
                Duration::from_millis(50),
                stopped.clone()
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("overran")
        );
        let (tx, rx) = broadcast::channel(1);
        tx.send(packet).unwrap();
        assert!(
            run_local(
                Capture(seen.clone()),
                rx,
                Duration::from_millis(50),
                stopped
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("expired")
        );
        assert!(seen.lock().unwrap().is_empty());
    }
    struct Blocked;
    impl LocalSink for Blocked {
        async fn write(&mut self, _packet: Arc<AudioPacket>, _at: Instant) -> Result<()> {
            std::future::pending().await
        }
        async fn finish(&mut self) -> Result<()> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn blocked_sink_has_a_deadline_and_stop_cancels_it() {
        let (tx, rx) = broadcast::channel(1);
        let (stop, stopped) = watch::channel(false);
        tx.send(Arc::new(
            MediaClock::now().packet(0, &[0; PACKET_BYTES]).unwrap(),
        ))
        .unwrap();
        let task = tokio::spawn(run_local(Blocked, rx, Duration::from_millis(50), stopped));
        tokio::time::sleep(Duration::from_millis(10)).await;
        stop.send_replace(true);
        timeout(Duration::from_millis(100), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let (tx, rx) = broadcast::channel(1);
        let (_stop, stopped) = watch::channel(false);
        tx.send(Arc::new(
            MediaClock::now().packet(0, &[0; PACKET_BYTES]).unwrap(),
        ))
        .unwrap();
        assert!(
            timeout(
                Duration::from_secs(1),
                run_local(Blocked, rx, Duration::from_millis(50), stopped)
            )
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("stalled")
        );
    }
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow_and_update() || stop.changed().await.is_err() {
            return;
        }
    }
}
