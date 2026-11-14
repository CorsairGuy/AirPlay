//! Source-session supervision, independent of device routes and shared clocks.
use anyhow::Result;
use sender::continuous_source::End;
use std::future::Future;
use tokio::sync::watch;
struct StopOnDrop(watch::Sender<bool>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
/// Observe media, metadata/source lifetime and command forwarding together.
/// First completion stops this session, then awaits the other branches. The
/// caller must pass the same stop domain used to construct its media group.
/// This does not stop a shared clock daemon or mute/restore hardware routes.
/// Cancelling this future broadcasts stop; async cleanup still requires the
/// outer host to await the task rather than simply aborting it.
pub async fn run_source(
    media: impl Future<Output = Result<End>>,
    observations: impl Future<Output = Result<()>>,
    commands: impl Future<Output = Result<()>>,
    stop: watch::Sender<bool>,
) -> Result<End> {
    let stop = StopOnDrop(stop);
    if *stop.0.borrow() {
        return Ok(End::Stopped);
    }
    tokio::pin!(media, observations, commands);
    tokio::select! {
        biased;
        result = &mut media => {
            stop.0.send_replace(true);
            let (observed, commanded) = tokio::join!(observations, commands);
            let end = result?;
            observed?;
            commanded?;
            Ok(end)
        }
        result = &mut observations => {
            stop.0.send_replace(true);
            let (ended, commanded) = tokio::join!(media, commands);
            result?;
            ended?;
            commanded?;
            Ok(End::Stopped)
        }
        result = &mut commands => {
            stop.0.send_replace(true);
            let (ended, observed) = tokio::join!(media, observations);
            result?;
            ended?;
            observed?;
            Ok(End::Stopped)
        }
    }
}
