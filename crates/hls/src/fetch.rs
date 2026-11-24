use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::LOCATION;

use crate::{
    Error, MAX_PLAYLIST_BYTES, MAX_REDIRECTS, Playlist, Url, is_disallowed_ip, parse_playlist,
    validate_redirect,
};

pub const MAX_SEGMENT_BYTES: usize = 32 * 1024 * 1024;
const MAX_DNS_ADDRESSES: usize = 32;
const MAX_CONCURRENT_FETCHES: usize = 2;
const MAX_MASTER_DEPTH: usize = 8;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const FETCH_TIMEOUT: Duration = Duration::from_secs(40);

#[derive(Clone)]
pub struct Fetcher {
    client: reqwest::Client,
    in_flight: Arc<tokio::sync::Semaphore>,
}

impl Fetcher {
    /// Create a client with pinned, non-public DNS results rejected.
    pub fn new() -> Result<Self, Error> {
        let client = reqwest::Client::builder()
            .dns_resolver(Arc::new(PublicResolver))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| Error::Fetch)?;
        Ok(Self {
            client,
            in_flight: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_FETCHES)),
        })
    }

    /// Fetch bounded HLS playlist bytes.
    pub async fn fetch_playlist(&self, url: &Url) -> Result<Vec<u8>, Error> {
        self.fetch_limited(url, MAX_PLAYLIST_BYTES).await
    }

    /// Fetch a playlist and resolve each listed URI against its URL.
    pub async fn playlist(&self, url: &Url) -> Result<Playlist, Error> {
        let bytes = self.fetch_playlist(url).await?;
        parse_playlist(url, &bytes)
    }

    /// Follow bounded master-playlist nesting and choose a stream at or below
    /// `max_bandwidth`, falling back to the lowest advertised variant.
    pub async fn media_playlist(
        &self,
        url: &Url,
        max_bandwidth: u64,
    ) -> Result<(Url, Playlist), Error> {
        let mut current = url.clone();
        for _ in 0..MAX_MASTER_DEPTH {
            let playlist = self.playlist(&current).await?;
            if playlist.variants.is_empty() {
                return Ok((current, playlist));
            }
            let variant = playlist
                .variants
                .iter()
                .filter_map(|variant| variant.bandwidth.map(|bandwidth| (bandwidth, variant)))
                .filter(|(bandwidth, _)| *bandwidth <= max_bandwidth)
                .max_by_key(|(bandwidth, _)| *bandwidth)
                .or_else(|| {
                    playlist
                        .variants
                        .iter()
                        .filter_map(|variant| {
                            variant.bandwidth.map(|bandwidth| (bandwidth, variant))
                        })
                        .min_by_key(|(bandwidth, _)| *bandwidth)
                })
                .map(|(_, variant)| variant)
                .ok_or(Error::NoVariant)?;
            current = variant.uri.clone();
        }
        Err(Error::Limit)
    }

    /// Fetch one bounded media segment.
    pub async fn fetch_segment(&self, url: &Url) -> Result<Vec<u8>, Error> {
        self.fetch_limited(url, MAX_SEGMENT_BYTES).await
    }

    async fn fetch_limited(&self, url: &Url, max_bytes: usize) -> Result<Vec<u8>, Error> {
        let _permit = self
            .in_flight
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Fetch)?;
        tokio::time::timeout(FETCH_TIMEOUT, self.fetch_redirects(url, max_bytes))
            .await
            .map_err(|_| Error::Timeout)?
    }

    async fn fetch_redirects(&self, url: &Url, max_bytes: usize) -> Result<Vec<u8>, Error> {
        let mut current = url.clone();
        for redirect_count in 0..=MAX_REDIRECTS {
            let mut response = self
                .client
                .get(current.as_str())
                .send()
                .await
                .map_err(|_| Error::Fetch)?;
            if response.status().is_redirection() {
                if redirect_count == MAX_REDIRECTS {
                    return Err(Error::UnsafeRedirect);
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(Error::UnsafeRedirect)?
                    .to_owned();
                let next = current.join(&location)?;
                validate_redirect(&current, &next)?;
                current = next;
                continue;
            }
            if !response.status().is_success() {
                return Err(Error::HttpStatus(response.status().as_u16()));
            }
            if response
                .content_length()
                .is_some_and(|length| length > max_bytes as u64)
            {
                return Err(Error::Limit);
            }

            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| Error::Fetch)? {
                if chunk.len() > max_bytes.saturating_sub(body.len()) {
                    return Err(Error::Limit);
                }
                body.extend_from_slice(&chunk);
            }
            return Ok(body);
        }
        Err(Error::UnsafeRedirect)
    }
}

struct PublicResolver;

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?
                .collect();
            if addresses.is_empty()
                || addresses.len() > MAX_DNS_ADDRESSES
                || addresses
                    .iter()
                    .any(|address| is_disallowed_ip(address.ip()))
            {
                return Err(Box::new(std::io::Error::other("non-public DNS result"))
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}
