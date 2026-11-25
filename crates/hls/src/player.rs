//! HLS VOD playback: demultiplexing, video and audio decoding on one
//! presentation clock, and the playback controls the AirPlay receiver exposes.
//!
//! The fetch side is async; FFmpeg reads synchronously from a bounded segment
//! stream, so demuxing and decoding run on a blocking worker. The async
//! controller opens the playlist, hands the worker one run per seek and keeps
//! the AirPlay lifecycle: stop and seek abandon the current run and its
//! segment producer instead of leaving a decoder running behind an aborted
//! task.
//!
//! Timing: both streams use the same monotonic anchor. Audio writes pace the
//! loop through the sound card, and video frames are shown at the same
//! timestamps, so picture follows sound with an error of at most one audio
//! buffer (the anchor is corrected by the measured buffer depth on the first
//! write). While paused, the audio sink is stopped and the anchor is shifted by
//! the pause duration on resume.
//!
//! Seek granularity is the playlist's `EXTINF` segment boundary: those values
//! are confirmed data, while a frame-accurate index is not available without
//! downloading and demuxing every segment.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use codec::video::VideoDecoder;
use ffmpeg_next as ff;
use ffmpeg_next::media::Type;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::{ContainerHint, Error, Fetcher, SegmentReader, Url, VodSource, open_demux};

/// Frames kept ahead of the audio clock before the worker waits for them.
const MAX_AHEAD_FRAMES: usize = 6;
/// How early a frame may be shown without sleeping for its timestamp.
const PRESENT_WINDOW: Duration = Duration::from_millis(20);
/// How often a paused worker re-checks its controls.
const CONTROL_POLL: Duration = Duration::from_millis(5);
/// Bounds the start-up correction taken from the audio sink's buffer depth.
const MAX_ANCHOR_CORRECTION_NS: i64 = 250_000_000;
/// Stop is noticed between packets; this is how often the controller checks
/// whether it should abandon a stalled run.
const CONTROL_CHECK: Duration = Duration::from_millis(50);

/// Where decoded audio goes. `sink` may open a host-specific output; otherwise
/// `device` is opened with [`audio::open`].
#[derive(Clone)]
pub struct AudioOptions {
    pub device: String,
    pub latency: Duration,
    pub sink: Option<audio::SinkFactory>,
}

pub struct PlaybackOptions {
    pub max_bandwidth: u64,
    pub video: video::VideoOptions,
    pub video_sink: video::FrameSinkFactory,
    /// `None` plays video only.
    pub audio: Option<AudioOptions>,
}

/// A running HLS VOD playback. Dropping it stops the worker.
pub struct Playback {
    control: Arc<Control>,
    position_ms: Arc<AtomicU64>,
    duration_ms: Arc<AtomicU64>,
    finished: Arc<AtomicBool>,
    task: Mutex<Option<JoinHandle<Result<(), Error>>>>,
}

impl Playback {
    /// Start playback of a finite, unencrypted playlist. Returns immediately;
    /// the playlist is fetched and the first run started in the background.
    /// Playback errors are logged and returned by [`Playback::wait`].
    pub fn spawn(fetcher: Fetcher, url: Url, options: PlaybackOptions, start: Duration) -> Self {
        let control = Arc::new(Control::default());
        let position_ms = Arc::new(AtomicU64::new(millis(start)));
        let duration_ms = Arc::new(AtomicU64::new(0));
        let finished = Arc::new(AtomicBool::new(false));
        let (run_tx, run_rx) = mpsc::unbounded_channel();
        let (done_tx, done_rx) = mpsc::unbounded_channel();
        let worker_options = WorkerOptions {
            video: options.video,
            video_sink: options.video_sink,
            audio: options.audio,
        };
        let worker_control = control.clone();
        let worker_position = position_ms.clone();
        tokio::task::spawn_blocking(move || {
            run_worker(
                run_rx,
                done_tx,
                worker_control,
                worker_options,
                worker_position,
            );
        });
        let controller_control = control.clone();
        let controller_position = position_ms.clone();
        let controller_duration = duration_ms.clone();
        let controller_finished = finished.clone();
        let task = tokio::spawn(async move {
            let result = drive(
                fetcher,
                url,
                options.max_bandwidth,
                start,
                run_tx,
                done_rx,
                controller_control,
                controller_position,
                controller_duration,
            )
            .await;
            if let Err(error) = &result {
                tracing::warn!("HLS playback: {error}");
            }
            controller_finished.store(true, Ordering::Relaxed);
            result
        });
        Self {
            control,
            position_ms,
            duration_ms,
            finished,
            task: Mutex::new(Some(task)),
        }
    }

    pub fn pause(&self) {
        self.control.pause();
    }

    pub fn resume(&self) {
        self.control.resume();
    }

    pub fn stop(&self) {
        self.control.stop();
    }

    /// Seek to `position`. Returns `false`, changing nothing, when the playlist
    /// duration is already known and the position is past it.
    pub fn seek(&self, position: Duration) -> bool {
        let total = self.duration_ms.load(Ordering::Relaxed);
        if total > 0 && millis(position) > total {
            return false;
        }
        self.control.seek(position);
        true
    }

    pub fn position(&self) -> Duration {
        Duration::from_millis(self.position_ms.load(Ordering::Relaxed))
    }

    /// `true` once the playback ended, failed or was stopped: controls have
    /// nothing left to act on.
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    /// Confirmed playlist duration, available once the playlist is fetched.
    pub fn duration(&self) -> Option<Duration> {
        match self.duration_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(Duration::from_millis(ms)),
        }
    }

    /// Wait for the playback to finish: end of stream, stop, or an error.
    pub async fn wait(self) -> Result<(), Error> {
        let task = self
            .task
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        match task {
            Some(task) => task
                .await
                .map_err(|error| Error::Worker(error.to_string()))?,
            None => Ok(()),
        }
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.control.stop();
        if let Some(task) = self
            .task
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            task.abort();
        }
    }
}

fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[derive(Default)]
struct Control {
    state: Mutex<ControlState>,
}

#[derive(Default)]
struct ControlState {
    paused: bool,
    stop: bool,
    seek: Option<Duration>,
}

enum Step {
    Play,
    Pause,
    Seek(Duration),
    Stop,
}

impl Control {
    fn state(&self) -> std::sync::MutexGuard<'_, ControlState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn step(&self) -> Step {
        let mut state = self.state();
        if state.stop {
            Step::Stop
        } else if let Some(position) = state.seek.take() {
            Step::Seek(position)
        } else if state.paused {
            Step::Pause
        } else {
            Step::Play
        }
    }

    fn pause(&self) {
        self.state().paused = true;
    }

    fn resume(&self) {
        self.state().paused = false;
    }

    fn stop(&self) {
        self.state().stop = true;
    }

    fn seek(&self, position: Duration) {
        self.state().seek = Some(position);
    }

    fn stopped(&self) -> bool {
        self.state().stop
    }
}

struct WorkerOptions {
    video: video::VideoOptions,
    video_sink: video::FrameSinkFactory,
    audio: Option<AudioOptions>,
}

struct Run {
    reader: SegmentReader,
    start_ms: u64,
}

enum Outcome {
    Eof,
    Stop,
    Seek(Duration),
}

fn run_worker(
    mut runs: mpsc::UnboundedReceiver<Run>,
    done: mpsc::UnboundedSender<Result<Outcome, Error>>,
    control: Arc<Control>,
    options: WorkerOptions,
    reported: Arc<AtomicU64>,
) {
    let mut sink = options.video_sink.open();
    let mut audio_sink: Option<Box<dyn audio::Sink>> = None;
    while let Some(run) = runs.blocking_recv() {
        if control.stopped() {
            break;
        }
        let outcome = play_run(
            run,
            &options,
            &control,
            &reported,
            &mut *sink,
            &mut audio_sink,
        );
        let last = !matches!(outcome, Ok(Outcome::Seek(_)));
        if done.send(outcome).is_err() || last {
            break;
        }
    }
    sink.end();
}

fn play_run(
    run: Run,
    options: &WorkerOptions,
    control: &Control,
    reported: &AtomicU64,
    sink: &mut dyn video::FrameSink,
    audio_sink: &mut Option<Box<dyn audio::Sink>>,
) -> Result<Outcome, Error> {
    let mut input = open_demux(run.reader, ContainerHint::Auto)?;
    let mut video_track = match input.streams().best(Type::Video) {
        Some(stream) => Some(VideoTrack {
            index: stream.index(),
            time_base: stream.time_base(),
            decoder: VideoDecoder::from_parameters(stream.parameters(), &options.video)
                .map_err(|error| Error::Decode(error.to_string()))?,
        }),
        None => None,
    };
    let mut audio_track = match (input.streams().best(Type::Audio), options.audio.is_some()) {
        (Some(stream), true) => Some(AudioTrack {
            index: stream.index(),
            time_base: stream.time_base(),
            decoder: open_audio_decoder(stream.parameters())?,
            frame: ff::frame::Audio::empty(),
            next_pts: None,
        }),
        _ => None,
    };
    if video_track.is_none() && audio_track.is_none() {
        return Err(Error::Demux);
    }

    let mut clock = Clock::default();
    let mut pacer = video::Pacer::new(true);
    let mut paused_at: Option<Instant> = None;
    let mut audio_anchored = false;

    let mut packets = input.packets();
    loop {
        match control.step() {
            Step::Stop => return Ok(Outcome::Stop),
            Step::Seek(position) => return Ok(Outcome::Seek(position)),
            Step::Pause => {
                if paused_at.is_none() {
                    paused_at = Some(Instant::now());
                    pacer.clear();
                    if let Some(sink) = audio_sink.as_deref_mut() {
                        sink.stop();
                    }
                }
                thread::sleep(CONTROL_POLL);
                continue;
            }
            Step::Play => {
                if let Some(at) = paused_at.take() {
                    clock.shift(at.elapsed());
                }
            }
        }

        let Some((stream, packet)) = packets.next() else {
            break;
        };

        if let Some(track) = audio_track.as_mut()
            && stream.index() == track.index
        {
            let Some(data) = packet.data() else {
                continue;
            };
            let mut pts = to_nanoseconds(packet.pts(), track.time_base).or(track.next_pts);
            track
                .decoder
                .send_packet(&ff::Packet::copy(data))
                .map_err(|error| Error::Audio(error.to_string()))?;
            let mut pcm = Vec::new();
            while track.decoder.receive_frame(&mut track.frame).is_ok() {
                pcm.clear();
                append_pcm(&track.frame, &mut pcm)?;
                if pcm.is_empty() {
                    continue;
                }
                let Some(position) = pts else {
                    continue;
                };
                let rate = track.frame.rate().max(1);
                let now = clock.now_ns();
                let anchored = clock.anchor(position, now);
                let due = clock.due(position);
                if due > now + 3_000_000 {
                    thread::sleep(Duration::from_nanos((due - now) as u64));
                }
                let output = match audio_sink {
                    Some(output) => output,
                    None => audio_sink.insert(open_audio_sink(options, rate)?),
                };
                output
                    .write(&pcm)
                    .map_err(|error| Error::Audio(error.to_string()))?;
                if anchored && !audio_anchored {
                    audio_anchored = true;
                    clock.correct_audio_start(&mut **output, pcm.len() / audio::CHANNELS, rate);
                }
                let chunk_ns =
                    pcm.len() as i64 * 1_000_000_000 / (rate as i64 * audio::CHANNELS as i64);
                pts = Some(position.saturating_add(chunk_ns));
                track.next_pts = pts;
                report(reported, run.start_ms, &clock, position);
            }
        } else if let Some(track) = video_track.as_mut()
            && stream.index() == track.index
        {
            let Some(data) = packet.data() else {
                continue;
            };
            let pts = to_nanoseconds(packet.pts(), track.time_base);
            let dts = to_nanoseconds(packet.dts(), track.time_base);
            let mut pictures = Vec::new();
            track
                .decoder
                .decode_timed(pts, dts, data, &mut |picture| pictures.push(picture))
                .map_err(|error| Error::Decode(error.to_string()))?;
            for picture in pictures {
                present(picture, &mut clock, &mut pacer, reported, run.start_ms);
                if pacer.len() >= MAX_AHEAD_FRAMES {
                    wait_and_show(&mut pacer, sink, PRESENT_WINDOW);
                }
            }
            show_due(&mut pacer, sink);
        }
    }

    if let Some(track) = video_track.as_mut() {
        let mut pictures = Vec::new();
        track.decoder.flush(&mut |picture| pictures.push(picture));
        for picture in pictures {
            present(picture, &mut clock, &mut pacer, reported, run.start_ms);
        }
    }
    drain(&mut pacer, sink);
    Ok(Outcome::Eof)
}

struct VideoTrack {
    index: usize,
    time_base: ff::Rational,
    decoder: VideoDecoder,
}

struct AudioTrack {
    index: usize,
    time_base: ff::Rational,
    decoder: ff::decoder::Audio,
    frame: ff::frame::Audio,
    next_pts: Option<i64>,
}

fn present(
    picture: video::Picture,
    clock: &mut Clock,
    pacer: &mut video::Pacer<video::Picture>,
    reported: &AtomicU64,
    start_ms: u64,
) {
    let pts = picture.pts();
    let now = clock.now_ns();
    clock.anchor(pts, now);
    pacer.push(clock.due(pts), picture);
    report(reported, start_ms, clock, pts);
}

fn show_due(pacer: &mut video::Pacer<video::Picture>, sink: &mut dyn video::FrameSink) {
    while let Some(picture) = pacer.pop_due(clock::now_ns() as i64) {
        sink.show(picture);
    }
}

/// Show the next frame if it is due within `window`; never blocks the audio.
fn wait_and_show(
    pacer: &mut video::Pacer<video::Picture>,
    sink: &mut dyn video::FrameSink,
    window: Duration,
) {
    if let Some(due) = pacer.next_due() {
        let wait = due.saturating_sub(clock::now_ns() as i64);
        if wait > 0 && (wait as u128) <= window.as_nanos() {
            thread::sleep(Duration::from_nanos(wait as u64));
        }
    }
    show_due(pacer, sink);
}

fn drain(pacer: &mut video::Pacer<video::Picture>, sink: &mut dyn video::FrameSink) {
    while let Some(due) = pacer.next_due() {
        let wait = due.saturating_sub(clock::now_ns() as i64);
        if wait > 0 {
            thread::sleep(Duration::from_nanos(wait as u64));
        }
        show_due(pacer, sink);
    }
}

fn report(reported: &AtomicU64, start_ms: u64, clock: &Clock, pts: i64) {
    reported.store(
        start_ms.saturating_add(clock.offset_ms(pts)),
        Ordering::Relaxed,
    );
}

/// Shared presentation clock: every timestamp is played `origin` nanoseconds
/// after its own start, with the first decoded timestamp anchored to `now`.
#[derive(Default)]
struct Clock {
    origin_ns: i64,
    base_ns: Option<i64>,
}

impl Clock {
    fn now_ns(&self) -> i64 {
        clock::now_ns() as i64
    }

    /// Anchor `pts` to `now` if the run has no anchor yet. Returns `true` when
    /// this call set it.
    fn anchor(&mut self, pts: i64, now: i64) -> bool {
        if self.base_ns.is_some() {
            return false;
        }
        self.base_ns = Some(pts);
        self.origin_ns = now;
        true
    }

    fn due(&self, pts: i64) -> i64 {
        self.origin_ns
            .saturating_add(pts.saturating_sub(self.base_ns.unwrap_or(pts)).max(0))
    }

    fn shift(&mut self, by: Duration) {
        self.origin_ns = self
            .origin_ns
            .saturating_add(by.as_nanos().min(i64::MAX as u128) as i64);
    }

    fn offset_ms(&self, pts: i64) -> u64 {
        (pts.saturating_sub(self.base_ns.unwrap_or(pts)).max(0) / 1_000_000) as u64
    }

    /// The first written chunks are still buffered in the sound card; move the
    /// anchor so the picture lines up with what is audible, not with the write.
    fn correct_audio_start(&mut self, sink: &mut dyn audio::Sink, frames: usize, rate: u32) {
        let rate = i64::from(rate.max(1));
        let chunk_ns = frames as i64 * 1_000_000_000 / rate;
        let buffered_ns = i64::from(sink.delay_frames()) * 1_000_000_000 / rate;
        let ahead = (buffered_ns - chunk_ns).clamp(0, MAX_ANCHOR_CORRECTION_NS);
        self.origin_ns = self.origin_ns.saturating_add(ahead);
    }
}

fn open_audio_sink(options: &WorkerOptions, rate: u32) -> Result<Box<dyn audio::Sink>, Error> {
    let Some(audio) = options.audio.as_ref() else {
        return Err(Error::Audio("audio output is not configured".into()));
    };
    let opened = match &audio.sink {
        Some(factory) => factory.open(rate, audio.latency),
        None => audio::open(&audio.device, rate, audio.latency),
    };
    opened.map_err(|error| Error::Audio(error.to_string()))
}

fn open_audio_decoder(parameters: ff::codec::Parameters) -> Result<ff::decoder::Audio, Error> {
    let codec = ff::decoder::find(parameters.id())
        .ok_or_else(|| Error::Audio("no FFmpeg decoder for the audio stream".into()))?;
    let context = ff::codec::context::Context::from_parameters(parameters)
        .map_err(|error| Error::Audio(error.to_string()))?;
    context
        .decoder()
        .open_as(codec)
        .map_err(|error| Error::Audio(error.to_string()))?
        .audio()
        .map_err(|error| Error::Audio(error.to_string()))
}

fn to_nanoseconds(timestamp: Option<i64>, time_base: ff::Rational) -> Option<i64> {
    let timestamp = timestamp?;
    let numerator = i128::from(time_base.numerator());
    let denominator = i128::from(time_base.denominator());
    if denominator <= 0 {
        return None;
    }
    i64::try_from(i128::from(timestamp) * numerator * 1_000_000_000 / denominator).ok()
}

/// Interleaved stereo i16 from any decoded sample format. No resampling: the
/// sink is opened at the stream's own rate, so only channel layout is adapted.
fn append_pcm(frame: &ff::frame::Audio, out: &mut Vec<i16>) -> Result<(), Error> {
    use ff::format::Sample;
    match frame.format() {
        Sample::U8(_) => collect::<u8>(frame, out),
        Sample::I16(_) => collect::<i16>(frame, out),
        Sample::I32(_) => collect::<i32>(frame, out),
        Sample::F32(_) => collect::<f32>(frame, out),
        Sample::F64(_) => collect::<f64>(frame, out),
        other => {
            return Err(Error::Audio(format!(
                "unsupported audio sample format {other:?}"
            )));
        }
    }
    Ok(())
}

fn collect<T: ff::frame::audio::Sample + PcmSample>(frame: &ff::frame::Audio, out: &mut Vec<i16>) {
    let channels = frame.channels().max(1) as usize;
    let samples = frame.samples();
    let planar = frame.is_planar();
    let first = frame.plane::<T>(0);
    let second = if planar && channels > 1 {
        frame.plane::<T>(1)
    } else {
        first
    };
    out.reserve(samples * 2);
    for i in 0..samples {
        let (left, right) = if planar {
            (
                first.get(i).copied().unwrap_or_default(),
                second.get(i).copied().unwrap_or_default(),
            )
        } else {
            let base = i * channels;
            (
                first.get(base).copied().unwrap_or_default(),
                first
                    .get(base + usize::from(channels > 1))
                    .copied()
                    .unwrap_or_default(),
            )
        };
        out.push(left.to_i16());
        out.push(right.to_i16());
    }
}

trait PcmSample: Copy + Default {
    fn to_i16(self) -> i16;
}

impl PcmSample for u8 {
    fn to_i16(self) -> i16 {
        (i16::from(self) - 128) << 8
    }
}

impl PcmSample for i16 {
    fn to_i16(self) -> i16 {
        self
    }
}

impl PcmSample for i32 {
    fn to_i16(self) -> i16 {
        (self >> 16) as i16
    }
}

impl PcmSample for f32 {
    fn to_i16(self) -> i16 {
        (self.clamp(-1.0, 1.0) * 32767.0) as i16
    }
}

impl PcmSample for f64 {
    fn to_i16(self) -> i16 {
        (self.clamp(-1.0, 1.0) * 32767.0) as i16
    }
}

// The controller: opens the playlist, feeds one run at a time and turns a
// seek into a new source at the containing segment.
#[allow(clippy::too_many_arguments)]
async fn drive(
    fetcher: Fetcher,
    url: Url,
    max_bandwidth: u64,
    start: Duration,
    runs: mpsc::UnboundedSender<Run>,
    mut done: mpsc::UnboundedReceiver<Result<Outcome, Error>>,
    control: Arc<Control>,
    reported: Arc<AtomicU64>,
    duration_ms: Arc<AtomicU64>,
) -> Result<(), Error> {
    let mut position = start;
    let mut source = open_source(
        &fetcher,
        &url,
        max_bandwidth,
        position,
        &reported,
        &duration_ms,
    )
    .await?;
    send_run(&mut source, &runs)?;

    loop {
        if control.stopped() {
            return Ok(());
        }
        let outcome = match tokio::time::timeout(CONTROL_CHECK, done.recv()).await {
            Err(_) => {
                // Still playing. A stop request must also unblock a decoder
                // that is waiting for a segment, so abandon the producer.
                if control.stopped() {
                    return Ok(());
                }
                continue;
            }
            Ok(None) => return Err(Error::Worker("HLS worker stopped".into())),
            Ok(Some(outcome)) => outcome?,
        };
        match outcome {
            Outcome::Eof => return source.finish().await,
            Outcome::Stop => return Ok(()),
            Outcome::Seek(to) => {
                drop(source);
                if control.stopped() {
                    return Ok(());
                }
                position = to;
                source = open_source(
                    &fetcher,
                    &url,
                    max_bandwidth,
                    position,
                    &reported,
                    &duration_ms,
                )
                .await?;
                if control.stopped() {
                    return Ok(());
                }
                send_run(&mut source, &runs)?;
            }
        }
    }
}

async fn open_source(
    fetcher: &Fetcher,
    url: &Url,
    max_bandwidth: u64,
    position: Duration,
    reported: &AtomicU64,
    duration_ms: &AtomicU64,
) -> Result<VodSource, Error> {
    let source = fetcher.open_vod_at(url, max_bandwidth, position).await?;
    duration_ms.store(millis(source.duration()), Ordering::Relaxed);
    reported.store(millis(source.start()), Ordering::Relaxed);
    Ok(source)
}

fn send_run(source: &mut VodSource, runs: &mpsc::UnboundedSender<Run>) -> Result<(), Error> {
    let reader = source.take_reader().ok_or(Error::Fetch)?;
    runs.send(Run {
        reader,
        start_ms: millis(source.start()),
    })
    .map_err(|_| Error::Worker("HLS worker stopped".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::Server;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct Counters {
        frames: AtomicUsize,
        samples: AtomicUsize,
        ended: AtomicBool,
    }

    struct VideoCounter(Arc<Counters>);

    impl video::FrameSink for VideoCounter {
        fn show(&mut self, _picture: video::Picture) {
            self.0.frames.fetch_add(1, Ordering::Relaxed);
        }

        fn end(&mut self) {
            self.0.ended.store(true, Ordering::Relaxed);
        }
    }

    struct AudioCounter(Arc<Counters>);

    impl audio::Sink for AudioCounter {
        fn sample_rate(&self) -> u32 {
            48_000
        }

        fn write(&mut self, samples: &[i16]) -> std::io::Result<()> {
            self.0
                .samples
                .fetch_add(samples.len() / audio::CHANNELS, Ordering::Relaxed);
            Ok(())
        }

        fn delay_frames(&mut self) -> u32 {
            0
        }

        fn stop(&mut self) {}
    }

    fn playback_options(counters: &Arc<Counters>) -> PlaybackOptions {
        let video_counters = counters.clone();
        let audio_counters = counters.clone();
        PlaybackOptions {
            max_bandwidth: u64::MAX,
            video: video::VideoOptions::default(),
            video_sink: video::FrameSinkFactory::new(move || {
                Box::new(VideoCounter(video_counters.clone()))
            }),
            audio: Some(AudioOptions {
                device: "null".into(),
                latency: Duration::from_millis(10),
                sink: Some(audio::SinkFactory::new(move |_, _| {
                    Ok(Box::new(AudioCounter(audio_counters.clone())) as Box<dyn audio::Sink>)
                })),
            }),
        }
    }

    fn playlist_url(server: &Server) -> Url {
        Url::parse(&server.url("/av/playlist.m3u8")).expect("playlist URL")
    }

    async fn wait_until(playback: &Playback, mut ready: impl FnMut() -> bool) {
        let started = Instant::now();
        while !ready() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "playback did not reach the expected state"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            assert!(!playback.position().is_zero() || started.elapsed() < Duration::from_secs(1));
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn plays_audio_and_video_over_http() {
        let server = Server::start();
        let counters = Arc::new(Counters::default());
        let playback = Playback::spawn(
            Fetcher::for_tests(),
            playlist_url(&server),
            playback_options(&counters),
            Duration::ZERO,
        );
        tokio::time::timeout(Duration::from_secs(20), playback.wait())
            .await
            .expect("playback timed out")
            .expect("playback failed");
        let frames = counters.frames.load(Ordering::Relaxed);
        let samples = counters.samples.load(Ordering::Relaxed);
        assert!(frames >= 24, "decoded video frames: {frames}");
        assert!(samples >= 60_000, "pushed audio frames: {samples}");
        assert!(counters.ended.load(Ordering::Relaxed));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pause_holds_output_until_resumed() {
        let server = Server::start();
        let counters = Arc::new(Counters::default());
        let playback = Playback::spawn(
            Fetcher::for_tests(),
            playlist_url(&server),
            playback_options(&counters),
            Duration::ZERO,
        );
        wait_until(&playback, || counters.frames.load(Ordering::Relaxed) >= 6).await;
        playback.pause();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let frames = counters.frames.load(Ordering::Relaxed);
        let samples = counters.samples.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            counters.frames.load(Ordering::Relaxed) <= frames + 1,
            "video advanced while paused"
        );
        assert!(
            counters.samples.load(Ordering::Relaxed) <= samples + 48_000 / 10,
            "audio advanced while paused"
        );
        playback.resume();
        tokio::time::timeout(Duration::from_secs(20), playback.wait())
            .await
            .expect("playback timed out after resume")
            .expect("playback failed after resume");
        assert!(counters.frames.load(Ordering::Relaxed) > frames);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn seek_restarts_at_the_containing_segment() {
        let server = Server::start();
        let counters = Arc::new(Counters::default());
        let playback = Playback::spawn(
            Fetcher::for_tests(),
            playlist_url(&server),
            playback_options(&counters),
            Duration::ZERO,
        );
        wait_until(&playback, || playback.duration().is_some()).await;
        assert_eq!(playback.duration(), Some(Duration::from_secs(3)));
        assert!(!playback.seek(Duration::from_secs(30)), "past the end");
        assert!(playback.seek(Duration::from_millis(2300)));
        tokio::time::timeout(Duration::from_secs(10), async {
            while playback.position() < Duration::from_millis(2000) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("seek did not restart playback at the segment");
        tokio::time::timeout(Duration::from_secs(20), playback.wait())
            .await
            .expect("playback timed out after seek")
            .expect("playback failed after seek");
        let frames = counters.frames.load(Ordering::Relaxed);
        assert!(frames > 0, "no frames after seek");
        assert!(
            frames < 72,
            "seek did not skip the first segments: {frames}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_ends_the_worker() {
        let server = Server::start();
        let counters = Arc::new(Counters::default());
        let playback = Playback::spawn(
            Fetcher::for_tests(),
            playlist_url(&server),
            playback_options(&counters),
            Duration::ZERO,
        );
        wait_until(&playback, || counters.frames.load(Ordering::Relaxed) > 0).await;
        playback.stop();
        tokio::time::timeout(Duration::from_secs(5), playback.wait())
            .await
            .expect("stop did not end playback")
            .expect("stop reported an error");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn interrupted_segment_fails_and_a_new_playback_succeeds() {
        let server = Server::start();
        server.fail_next("/av/seg1.ts");
        let counters = Arc::new(Counters::default());
        let playback = Playback::spawn(
            Fetcher::for_tests(),
            playlist_url(&server),
            playback_options(&counters),
            Duration::ZERO,
        );
        let failed = tokio::time::timeout(Duration::from_secs(20), playback.wait())
            .await
            .expect("truncated segment did not end playback");
        assert!(
            failed.is_err(),
            "a truncated segment was reported as success"
        );

        let reconnected = Arc::new(Counters::default());
        let playback = Playback::spawn(
            Fetcher::for_tests(),
            playlist_url(&server),
            playback_options(&reconnected),
            Duration::ZERO,
        );
        tokio::time::timeout(Duration::from_secs(20), playback.wait())
            .await
            .expect("reconnect timed out")
            .expect("reconnect failed");
        assert!(reconnected.frames.load(Ordering::Relaxed) > 0);
    }
}
