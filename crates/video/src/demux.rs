use std::thread;
use std::time::Duration;

use codec::video::VideoDecoder;
use ffmpeg_next::{Rational, format::context::Input, media::Type};

use crate::{FrameSink, Pacer, VideoOptions};

const MAX_AHEAD_FRAMES: usize = 60;

/// Demuxes and decodes the video stream from an already-open FFmpeg input.
/// Run this on a blocking thread; frames are presented against the stream PTS.
pub fn decode_input(
    input: Input,
    options: &VideoOptions,
    mut sink: Box<dyn FrameSink>,
) -> Result<(), codec::Error> {
    let result = decode_frames(input, options, sink.as_mut());
    sink.end();
    result
}

fn decode_frames(
    mut input: Input,
    options: &VideoOptions,
    sink: &mut dyn FrameSink,
) -> Result<(), codec::Error> {
    let stream = input
        .streams()
        .best(Type::Video)
        .ok_or_else(|| codec::Error::Init("HLS содержит поток без видео".into()))?;
    let stream_index = stream.index();
    let time_base = stream.time_base();
    let parameters = stream.parameters();
    let mut decoder = VideoDecoder::from_parameters(parameters, options)?;
    let mut pacer = Pacer::new(true);
    let mut first_pts = None;
    let mut start_ns = None;

    for (stream, packet) in input.packets() {
        if stream.index() != stream_index {
            continue;
        }
        let Some(data) = packet.data() else {
            continue;
        };
        let pts = to_nanoseconds(packet.pts(), time_base);
        let dts = to_nanoseconds(packet.dts(), time_base);
        let mut frames = Vec::new();
        decoder.decode_timed(pts, dts, data, &mut |picture| frames.push(picture))?;
        for picture in frames {
            let pts = picture.pts();
            let origin = *first_pts.get_or_insert(pts);
            let start = *start_ns.get_or_insert_with(|| clock::now_ns() as i64);
            let due = start.saturating_add(pts.saturating_sub(origin).max(0));
            pacer.push(due, picture);
            while pacer.len() >= MAX_AHEAD_FRAMES {
                wait_and_present(&mut pacer, sink);
            }
        }
        present_due(&mut pacer, sink);
    }

    let mut frames = Vec::new();
    decoder.flush(&mut |picture| frames.push(picture));
    for picture in frames {
        let pts = picture.pts();
        let origin = *first_pts.get_or_insert(pts);
        let start = *start_ns.get_or_insert_with(|| clock::now_ns() as i64);
        let due = start.saturating_add(pts.saturating_sub(origin).max(0));
        pacer.push(due, picture);
    }
    drain(&mut pacer, sink);
    Ok(())
}

fn to_nanoseconds(timestamp: Option<i64>, time_base: Rational) -> Option<i64> {
    let timestamp = timestamp?;
    let numerator = i128::from(time_base.numerator());
    let denominator = i128::from(time_base.denominator());
    if denominator <= 0 {
        return None;
    }
    i64::try_from(i128::from(timestamp) * numerator * 1_000_000_000 / denominator).ok()
}

fn present_due(pacer: &mut Pacer<crate::Picture>, sink: &mut dyn FrameSink) {
    while let Some(picture) = pacer.pop_due(clock::now_ns() as i64) {
        sink.show(picture);
    }
}

fn wait_and_present(pacer: &mut Pacer<crate::Picture>, sink: &mut dyn FrameSink) {
    if let Some(due) = pacer.next_due() {
        let wait = due.saturating_sub(clock::now_ns() as i64).max(0) as u64;
        if wait > 0 {
            thread::sleep(Duration::from_nanos(wait));
        }
    }
    present_due(pacer, sink);
}

fn drain(pacer: &mut Pacer<crate::Picture>, sink: &mut dyn FrameSink) {
    while !pacer.is_empty() {
        wait_and_present(pacer, sink);
    }
}
