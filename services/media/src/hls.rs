//! HLS VOD output: how a cast URL is fetched and rendered.

/// Variant bitrate ceiling; if none fits, the fetcher selects the lowest.
#[derive(Clone)]
pub struct HlsOptions {
    pub max_bandwidth: u64,
    /// FFmpeg decoder selection and latency options.
    pub decoder: video::VideoOptions,
    /// Host-provided renderer for decoded frames.
    pub sink: video::FrameSinkFactory,
}
