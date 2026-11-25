use std::io::{self, Read};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::{Error, Fetcher, Playlist, Url};

const SEGMENT_QUEUE: usize = 2;

pub struct VodSource {
    media_url: Url,
    playlist: Playlist,
    duration: Duration,
    start: Duration,
    reader: Option<SegmentReader>,
    producer: Option<JoinHandle<Result<(), Error>>>,
}

impl VodSource {
    pub fn media_url(&self) -> &Url {
        &self.media_url
    }

    pub fn playlist(&self) -> &Playlist {
        &self.playlist
    }

    /// Full length of the finite playlist: the sum of its `EXTINF` durations.
    /// This is the only duration the receiver can confirm before playback.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Offset of the first delivered segment in the playlist timeline. It is a
    /// segment boundary, never an arbitrary position.
    pub fn start(&self) -> Duration {
        self.start
    }

    /// Take the blocking reader to pass to a demuxer running off the async
    /// runtime. Call `finish` after the reader reaches EOF or is dropped.
    pub fn take_reader(&mut self) -> Option<SegmentReader> {
        self.reader.take()
    }

    /// Wait for all requested segments and surface any download failure.
    pub async fn finish(mut self) -> Result<(), Error> {
        self.reader.take();
        let producer = self.producer.take().ok_or(Error::Fetch)?;
        producer.await.map_err(|_| Error::Fetch)?
    }
}

impl Drop for VodSource {
    fn drop(&mut self) {
        if let Some(producer) = self.producer.take() {
            producer.abort();
        }
    }
}

/// Blocking reader over a bounded queue of asynchronously fetched segments.
/// Read it only from a blocking thread.
pub struct SegmentReader {
    receiver: mpsc::Receiver<Result<Vec<u8>, Error>>,
    current: Vec<u8>,
    offset: usize,
    failed: bool,
}

impl SegmentReader {
    fn new(receiver: mpsc::Receiver<Result<Vec<u8>, Error>>) -> Self {
        Self {
            receiver,
            current: Vec::new(),
            offset: 0,
            failed: false,
        }
    }
}

impl Read for SegmentReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.failed {
            return Err(io::Error::other("HLS segment source failed"));
        }

        loop {
            if self.offset < self.current.len() {
                let count = output.len().min(self.current.len() - self.offset);
                output[..count].copy_from_slice(&self.current[self.offset..self.offset + count]);
                self.offset += count;
                return Ok(count);
            }

            match self.receiver.blocking_recv() {
                Some(Ok(bytes)) => {
                    self.current = bytes;
                    self.offset = 0;
                }
                Some(Err(error)) => {
                    self.failed = true;
                    return Err(io::Error::other(error));
                }
                None => return Ok(0),
            }
        }
    }
}

impl Fetcher {
    /// Fetch a finite media playlist into a bounded blocking byte stream.
    pub async fn open_vod(&self, url: &Url, max_bandwidth: u64) -> Result<VodSource, Error> {
        self.open_vod_at(url, max_bandwidth, Duration::ZERO).await
    }

    /// Open a finite playlist at the segment that contains `position`.
    ///
    /// The playlist's `EXTINF` values are confirmed data, so a start or seek is
    /// mapped to the beginning of the containing segment: the first media byte
    /// served is a segment boundary, not the requested timestamp.
    pub async fn open_vod_at(
        &self,
        url: &Url,
        max_bandwidth: u64,
        position: Duration,
    ) -> Result<VodSource, Error> {
        let (media_url, playlist) = self.media_playlist(url, max_bandwidth).await?;
        if playlist.live {
            return Err(Error::UnsupportedFeature("live playlist refresh"));
        }

        let total_ms: u64 = playlist.segments.iter().map(|s| s.duration_ms).sum();
        let (first, start) = segment_at(&playlist, position)?;
        let duration = Duration::from_millis(total_ms);

        let (sender, receiver) = mpsc::channel(SEGMENT_QUEUE);
        let fetcher = self.clone();
        let segments = playlist.segments[first..].to_vec();
        let producer = tokio::spawn(async move {
            let mut last_init = None;
            for segment in segments {
                if let Some(init_url) = segment.initialization_segment
                    && last_init.as_ref() != Some(&init_url)
                {
                    fetch_and_send(&fetcher, &sender, &init_url).await?;
                    last_init = Some(init_url);
                }
                fetch_and_send(&fetcher, &sender, &segment.uri).await?;
            }
            Ok(())
        });

        Ok(VodSource {
            media_url,
            playlist,
            duration,
            start,
            reader: Some(SegmentReader::new(receiver)),
            producer: Some(producer),
        })
    }
}

/// The segment that contains `position` and that segment's start offset.
fn segment_at(playlist: &Playlist, position: Duration) -> Result<(usize, Duration), Error> {
    if playlist.segments.is_empty() {
        return Err(Error::Playlist("media playlist has no segments".into()));
    }
    let mut start_ms = 0u64;
    for (index, segment) in playlist.segments.iter().enumerate() {
        let end_ms = start_ms.saturating_add(segment.duration_ms);
        if position.as_millis() < u128::from(end_ms) {
            return Ok((index, Duration::from_millis(start_ms)));
        }
        start_ms = end_ms;
    }
    // Past the end: clamp to the last segment rather than reading past it.
    let index = playlist.segments.len() - 1;
    let start_ms: u64 = playlist.segments[..index]
        .iter()
        .map(|s| s.duration_ms)
        .sum();
    Ok((index, Duration::from_millis(start_ms)))
}

async fn fetch_and_send(
    fetcher: &Fetcher,
    sender: &mpsc::Sender<Result<Vec<u8>, Error>>,
    url: &Url,
) -> Result<(), Error> {
    let bytes = match fetcher.fetch_segment(url).await {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = sender.send(Err(error.clone())).await;
            return Err(error);
        }
    };
    // Nobody is reading any more (stop or seek): end quietly instead of
    // reporting a fetch failure for segments the caller no longer wants.
    let _ = sender.send(Ok(bytes)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Segment, Variant};

    fn playlist(durations: &[u64]) -> Playlist {
        Playlist {
            variants: Vec::<Variant>::new(),
            segments: durations
                .iter()
                .enumerate()
                .map(|(i, ms)| Segment {
                    uri: Url::parse(&format!("https://example.com/{i}.ts")).unwrap(),
                    duration_ms: *ms,
                    initialization_segment: None,
                })
                .collect(),
            live: false,
        }
    }

    #[test]
    fn position_maps_to_containing_segment() {
        let p = playlist(&[1000, 1000, 1000]);
        assert_eq!(segment_at(&p, Duration::ZERO).unwrap().0, 0);
        assert_eq!(segment_at(&p, Duration::from_millis(999)).unwrap().0, 0);
        assert_eq!(
            segment_at(&p, Duration::from_millis(1000)).unwrap(),
            (1, Duration::from_millis(1000))
        );
        assert_eq!(
            segment_at(&p, Duration::from_millis(2100)).unwrap(),
            (2, Duration::from_millis(2000))
        );
        assert_eq!(
            segment_at(&p, Duration::from_secs(30)).unwrap(),
            (2, Duration::from_millis(2000))
        );
    }

    #[test]
    fn empty_playlist_is_rejected() {
        assert!(matches!(
            segment_at(&playlist(&[]), Duration::ZERO),
            Err(Error::Playlist(_))
        ));
    }
}
