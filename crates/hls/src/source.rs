use std::io::{self, Read};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::{Error, Fetcher, Playlist, Url};

const SEGMENT_QUEUE: usize = 2;

pub struct VodSource {
    media_url: Url,
    playlist: Playlist,
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
        let (media_url, playlist) = self.media_playlist(url, max_bandwidth).await?;
        if playlist.live {
            return Err(Error::UnsupportedFeature("live playlist refresh"));
        }

        let (sender, receiver) = mpsc::channel(SEGMENT_QUEUE);
        let fetcher = self.clone();
        let segments = playlist.segments.clone();
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
            reader: Some(SegmentReader::new(receiver)),
            producer: Some(producer),
        })
    }
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
    sender.send(Ok(bytes)).await.map_err(|_| Error::Fetch)
}
