use crate::{Error, Fetcher, SegmentReader, Url};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ContainerHint {
    #[default]
    Auto,
    MpegTs,
    FragmentedMp4,
}

/// Open the fetched byte stream as a demuxer. Run this from a blocking thread:
/// FFmpeg reads synchronously while the async fetcher supplies segments.
pub fn open_demux(
    reader: SegmentReader,
    hint: ContainerHint,
) -> Result<ffmpeg_next::format::context::Input, Error> {
    ffmpeg_next::init().map_err(|_| Error::Demux)?;
    let io = ffmpeg_next::format::context::stream_io::StreamIo::from_read(reader)
        .map_err(|_| Error::Demux)?;
    let filename = match hint {
        ContainerHint::Auto => None,
        ContainerHint::MpegTs => Some("hls.ts"),
        ContainerHint::FragmentedMp4 => Some("hls.mp4"),
    };
    ffmpeg_next::format::input_from_stream(io, filename, None).map_err(|_| Error::Demux)
}

impl Fetcher {
    /// Fetch, demux, decode, and present a finite HLS VOD stream on a blocking
    /// worker. The caller retains the AirPlay playback lifecycle and controls.
    pub async fn play_vod(
        &self,
        url: &Url,
        max_bandwidth: u64,
        options: video::VideoOptions,
        sink: Box<dyn video::FrameSink>,
    ) -> Result<(), Error> {
        let mut source = self.open_vod(url, max_bandwidth).await?;
        let reader = source.take_reader().ok_or(Error::Fetch)?;
        let worker = tokio::task::spawn_blocking(move || {
            let input = open_demux(reader, ContainerHint::Auto)?;
            video::decode_input(input, &options, sink)
                .map_err(|error| Error::Decode(error.to_string()))
        });
        let playback = worker
            .await
            .map_err(|error| Error::Worker(error.to_string()))?;
        let fetch = source.finish().await;
        playback?;
        fetch
    }
}
