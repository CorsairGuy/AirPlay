//! Provider-neutral model of a playback queue.
//!
//! This is not the PCM packet queue: it holds stable track identifiers and
//! metadata resolved by the application's own source. Providers, catalog
//! access, credentials and DRM stay outside the core. A queue operation is
//! atomic: a failed resolve leaves the previous queue untouched, and a stale
//! selection cannot overwrite a newer one.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

/// Provider-scoped stable identifier. Never derived from array position, so a
/// reordered queue does not change identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TrackId(String);

impl TrackId {
    pub const MAX_BYTES: usize = 512;

    pub fn new(id: impl Into<String>) -> Result<Self, QueueError> {
        let id = id.into();
        if id.is_empty() || id.len() > Self::MAX_BYTES || id.chars().any(char::is_control) {
            return Err(QueueError::InvalidId);
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TrackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the application's source returns for one identifier. The core stores
/// no URLs, tokens or provider state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub id: TrackId,
    pub title: String,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    NotFound,
    Unavailable(String),
    Unsupported,
}

/// Implemented by the application or its adapter, not by the core.
pub trait Source {
    fn resolve(&mut self, id: &TrackId) -> Result<Track, SourceError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueError {
    Empty,
    Unknown,
    Duplicate,
    InvalidId,
    TooManyTracks,
    InvalidMetadata,
    Cancelled,
    Source(SourceError),
}

impl From<SourceError> for QueueError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

/// A selection that has been requested but not yet committed. While it is in
/// flight another caller may select a different track; only the newest
/// generation is allowed to change the current item.
#[derive(Debug)]
pub struct PendingSelect {
    revision: Arc<()>,
    id: TrackId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Next,
    Previous,
}

/// Queue with a current item. Indices are internal; identities are not.
#[derive(Debug, Default)]
pub struct TrackQueue {
    tracks: Vec<Track>,
    current: Option<usize>,
    revision: Arc<()>,
}

impl TrackQueue {
    pub const MAX_TRACKS: usize = 10_000;
    pub const MAX_TITLE_BYTES: usize = 4096;
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn current(&self) -> Option<&Track> {
        self.current.map(|index| &self.tracks[index])
    }

    /// Resolve every identifier before touching state. An unknown or
    /// unavailable id leaves the old queue and current item intact.
    pub fn load(
        &mut self,
        source: &mut impl Source,
        ids: &[TrackId],
    ) -> Result<&[Track], QueueError> {
        if ids.is_empty() {
            return Err(QueueError::Empty);
        }
        if ids.len() > Self::MAX_TRACKS {
            return Err(QueueError::TooManyTracks);
        }
        let mut seen = HashSet::with_capacity(ids.len());
        if ids.iter().any(|id| !seen.insert(id)) {
            return Err(QueueError::Duplicate);
        }
        let mut resolved = Vec::with_capacity(ids.len());
        for id in ids {
            let track = source.resolve(id)?;
            if &track.id != id {
                return Err(QueueError::Source(SourceError::Unavailable(
                    "source returned another id".into(),
                )));
            }
            if track.title.len() > Self::MAX_TITLE_BYTES {
                return Err(QueueError::InvalidMetadata);
            }
            resolved.push(track);
        }
        self.tracks = resolved;
        self.current = Some(0);
        self.revision = Arc::new(());
        Ok(&self.tracks)
    }

    /// Start changing the current item. Validates membership now, so an
    /// unknown id fails without touching the current item.
    pub fn begin_select(&mut self, id: &TrackId) -> Result<PendingSelect, QueueError> {
        if self.tracks.is_empty() {
            return Err(QueueError::Empty);
        }
        if !self.tracks.iter().any(|track| &track.id == id) {
            return Err(QueueError::Unknown);
        }
        self.revision = Arc::new(());
        Ok(PendingSelect {
            revision: self.revision.clone(),
            id: id.clone(),
        })
    }

    /// Commit the selection only if no newer one started or cancelled it.
    pub fn commit_select(&mut self, pending: PendingSelect) -> Result<&Track, QueueError> {
        if !Arc::ptr_eq(&pending.revision, &self.revision) {
            return Err(QueueError::Cancelled);
        }
        let index = self
            .tracks
            .iter()
            .position(|track| track.id == pending.id)
            .ok_or(QueueError::Unknown)?;
        self.current = Some(index);
        Ok(&self.tracks[index])
    }

    /// Cancel whatever selection is in flight. Idempotent.
    pub fn cancel(&mut self) {
        self.revision = Arc::new(());
    }

    /// Move in playlist order. No wrap-around: the queue never silently
    /// restarts, and the last/first item reports `None`.
    pub fn step(&mut self, direction: Direction) -> Option<&Track> {
        let current = self.current?;
        let next = match direction {
            Direction::Next => current
                .checked_add(1)
                .filter(|index| *index < self.tracks.len())?,
            Direction::Previous => current.checked_sub(1)?,
        };
        self.current = Some(next);
        self.revision = Arc::new(());
        self.current()
    }

    /// Remove an item by identity. Removing the current item leaves no
    /// current item instead of guessing the next one.
    pub fn remove(&mut self, id: &TrackId) -> bool {
        let Some(index) = self.tracks.iter().position(|track| &track.id == id) else {
            return false;
        };
        self.tracks.remove(index);
        self.revision = Arc::new(());
        self.current = match self.current {
            Some(current) if current == index => None,
            Some(current) if current > index => Some(current - 1),
            other => other,
        };
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeSource {
        catalog: HashMap<TrackId, Track>,
    }

    impl FakeSource {
        fn with(ids: &[(&str, &str)]) -> Self {
            let mut catalog = HashMap::new();
            for (id, title) in ids {
                let id = TrackId::new(*id).unwrap();
                catalog.insert(
                    id.clone(),
                    Track {
                        id,
                        title: (*title).into(),
                        duration_ms: Some(180_000),
                    },
                );
            }
            Self { catalog }
        }
    }

    impl Source for FakeSource {
        fn resolve(&mut self, id: &TrackId) -> Result<Track, SourceError> {
            self.catalog.get(id).cloned().ok_or(SourceError::NotFound)
        }
    }

    fn ids(names: &[&str]) -> Vec<TrackId> {
        names.iter().map(|id| TrackId::new(*id).unwrap()).collect()
    }

    #[test]
    fn load_keeps_stable_identities_and_current_item() {
        let mut source = FakeSource::with(&[("a", "A"), ("b", "B")]);
        let mut queue = TrackQueue::new();
        queue.load(&mut source, &ids(&["b", "a"])).unwrap();
        assert_eq!(
            queue
                .tracks()
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["b", "a"]
        );
        assert_eq!(queue.current().unwrap().id.as_str(), "b");
    }

    #[test]
    fn failed_load_is_atomic_and_keeps_old_queue() {
        let mut source = FakeSource::with(&[("a", "A"), ("b", "B")]);
        let mut queue = TrackQueue::new();
        queue.load(&mut source, &ids(&["a"])).unwrap();
        assert!(matches!(
            queue.load(&mut source, &ids(&["a", "missing"])),
            Err(QueueError::Source(SourceError::NotFound))
        ));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.current().unwrap().id.as_str(), "a");
        assert!(matches!(
            queue.load(&mut source, &ids(&["a", "a"])),
            Err(QueueError::Duplicate)
        ));
        assert!(matches!(
            queue.load(&mut source, &[]),
            Err(QueueError::Empty)
        ));
        assert_eq!(queue.len(), 1, "отказы не меняют очередь");
    }

    #[test]
    fn stale_selection_cannot_replace_the_current_track() {
        let mut source = FakeSource::with(&[("a", "A"), ("b", "B"), ("c", "C")]);
        let mut queue = TrackQueue::new();
        queue.load(&mut source, &ids(&["a", "b", "c"])).unwrap();
        let first = queue.begin_select(&ids(&["b"])[0]).unwrap();
        let second = queue.begin_select(&ids(&["c"])[0]).unwrap();
        assert!(matches!(
            queue.commit_select(first),
            Err(QueueError::Cancelled)
        ));
        assert_eq!(queue.commit_select(second).unwrap().id.as_str(), "c");
        let pending = queue.begin_select(&ids(&["a"])[0]).unwrap();
        queue.cancel();
        assert!(matches!(
            queue.commit_select(pending),
            Err(QueueError::Cancelled)
        ));
        assert_eq!(queue.current().unwrap().id.as_str(), "c");
    }

    #[test]
    fn selection_cannot_be_committed_to_another_queue() {
        let mut source = FakeSource::with(&[("a", "A"), ("b", "B")]);
        let mut first = TrackQueue::new();
        let mut second = TrackQueue::new();
        first.load(&mut source, &ids(&["a", "b"])).unwrap();
        second.load(&mut source, &ids(&["a", "b"])).unwrap();
        let pending = first.begin_select(&ids(&["b"])[0]).unwrap();
        let _other_pending = second.begin_select(&ids(&["b"])[0]).unwrap();
        assert_eq!(second.commit_select(pending), Err(QueueError::Cancelled));
        assert_eq!(second.current().unwrap().id.as_str(), "a");
    }

    #[test]
    fn oversized_queue_or_metadata_keeps_the_previous_queue() {
        let mut source = FakeSource::with(&[("a", "A"), ("b", "B")]);
        let mut queue = TrackQueue::new();
        queue.load(&mut source, &ids(&["a"])).unwrap();
        let oversized = vec![TrackId::new("b").unwrap(); TrackQueue::MAX_TRACKS + 1];
        assert_eq!(
            queue.load(&mut source, &oversized),
            Err(QueueError::TooManyTracks)
        );
        source
            .catalog
            .get_mut(&TrackId::new("b").unwrap())
            .unwrap()
            .title = "x".repeat(TrackQueue::MAX_TITLE_BYTES + 1);
        assert_eq!(
            queue.load(&mut source, &ids(&["b"])),
            Err(QueueError::InvalidMetadata)
        );
        assert_eq!(queue.current().unwrap().id.as_str(), "a");
    }

    #[test]
    fn unknown_and_empty_selections_do_not_move() {
        let mut queue = TrackQueue::new();
        assert!(matches!(
            queue.begin_select(&ids(&["a"])[0]),
            Err(QueueError::Empty)
        ));
        let mut source = FakeSource::with(&[("a", "A")]);
        queue.load(&mut source, &ids(&["a"])).unwrap();
        assert!(matches!(
            queue.begin_select(&ids(&["missing"])[0]),
            Err(QueueError::Unknown)
        ));
        assert_eq!(queue.current().unwrap().id.as_str(), "a");
    }

    #[test]
    fn step_stops_at_both_ends_without_wrapping() {
        let mut source = FakeSource::with(&[("a", "A"), ("b", "B")]);
        let mut queue = TrackQueue::new();
        queue.load(&mut source, &ids(&["a", "b"])).unwrap();
        assert_eq!(queue.step(Direction::Previous), None);
        assert_eq!(queue.step(Direction::Next).unwrap().id.as_str(), "b");
        assert_eq!(queue.step(Direction::Next), None);
        assert_eq!(queue.current().unwrap().id.as_str(), "b");
        assert_eq!(queue.step(Direction::Previous).unwrap().id.as_str(), "a");
    }

    #[test]
    fn removing_the_current_item_leaves_no_current_item() {
        let mut source = FakeSource::with(&[("a", "A"), ("b", "B"), ("c", "C")]);
        let mut queue = TrackQueue::new();
        queue.load(&mut source, &ids(&["a", "b", "c"])).unwrap();
        assert!(queue.remove(&ids(&["a"])[0]));
        assert!(
            queue.current().is_none(),
            "удаление текущего не выбирает другое"
        );
        assert_eq!(queue.len(), 2);

        let b = queue.begin_select(&ids(&["b"])[0]).unwrap();
        queue.commit_select(b).unwrap();
        assert!(queue.remove(&ids(&["b"])[0]));
        assert!(queue.current().is_none());
        assert!(!queue.remove(&ids(&["b"])[0]));
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn identifiers_reject_empty_control_and_oversized_values() {
        assert!(TrackId::new("").is_err());
        assert!(TrackId::new("a\nb").is_err());
        assert!(TrackId::new("x".repeat(TrackId::MAX_BYTES + 1)).is_err());
        assert!(TrackId::new("x".repeat(TrackId::MAX_BYTES)).is_ok());
    }
}
