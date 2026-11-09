//! Pure policy at the SMTC boundary (all times here use 100 ns ticks).

const TICKS_PER_SECOND: f64 = 10_000_000.0;

/// A thumbnail can arrive after the text for a track. Retry on metadata events,
/// including replacement of a previously decoded placeholder. Timeline wakes do not read art.
#[derive(Default)]
pub struct ArtRetry {
    track: Option<String>,
    // Outer None means nothing has been published for this track yet.
    last_hash: Option<Option<u64>>,
}

impl ArtRetry {
    pub fn should_read(&mut self, track: &str, metadata_event: bool) -> bool {
        let changed = self.track.as_deref() != Some(track);
        if changed {
            self.track = Some(track.to_owned());
            self.last_hash = None;
        }
        changed || metadata_event
    }

    /// `None` represents no visible artwork. Remember only what the UI was sent.
    pub fn should_publish(&mut self, hash: Option<u64>) -> bool {
        let changed = self.last_hash != Some(hash);
        self.last_hash = Some(hash);
        changed
    }
}

/// Fingerprint of decoded PBGRA pixels, with placeholder detection.
pub struct ArtFingerprint {
    pub hash: u64,
    pub opaque_fraction: f64,
    pub visible: bool,
}

impl ArtFingerprint {
    pub fn new(bgra: &[u8]) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bgra.hash(&mut hasher);
        let pixels = bgra.as_chunks::<4>().0.iter();
        let count = pixels.len();
        // Alpha <= 4/255 is effectively invisible, even if RGB bytes vary.
        let opaque = pixels.clone().filter(|p| p[3] > 4).count();
        let uniform = pixels.clone().all(|p| Some(p) == pixels.clone().next());
        Self {
            hash: hasher.finish(),
            opaque_fraction: if count == 0 { 0.0 } else { opaque as f64 / count as f64 },
            visible: opaque > 0 && !uniform,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeline {
    start: i64,
    end: i64,
    position: i64,
}

impl Timeline {
    pub fn new(start: i64, end: i64, position: i64) -> Self {
        Self { start, end, position }
    }

    pub fn seconds(self) -> (f64, f64) {
        let duration = (self.end as i128 - self.start as i128).max(0);
        let position = (self.position as i128 - self.start as i128).max(0);
        let position = if duration > 0 { position.min(duration) } else { position };
        (position as f64 / TICKS_PER_SECOND, duration as f64 / TICKS_PER_SECOND)
    }

    pub fn seek_ticks(self, seconds: f64) -> Option<i64> {
        if !seconds.is_finite() || self.end <= self.start {
            return None;
        }
        let span = self.end as i128 - self.start as i128;
        let relative = ((seconds.max(0.0) * TICKS_PER_SECOND).round() as i128).min(span);
        Some((self.start as i128 + relative) as i64)
    }
}

#[derive(Default)]
pub struct TrackTimeline {
    track: Option<String>,
    previous: Option<Timeline>,
    waiting: bool,
}

impl TrackTimeline {
    /// Metadata may precede the corresponding timeline. Hide the stale bar until
    /// either its values change or a subsequent timeline event confirms them.
    pub fn update(&mut self, track: String, timeline: Timeline, timeline_event: bool) -> (f64, f64) {
        if self.track.as_ref().is_some_and(|old| old != &track) {
            self.waiting = self.previous == Some(timeline);
        } else if timeline_event || self.previous != Some(timeline) {
            self.waiting = false;
        }
        self.track = Some(track);
        self.previous = Some(timeline);
        if self.waiting { (0.0, 0.0) } else { timeline.seconds() }
    }
}

#[derive(Default)]
pub struct SessionSelection {
    selected: Option<u64>,
    had_session: bool,
}

impl SessionSelection {
    /// Prefer the current playing session; otherwise preserve the existing owner.
    /// A different paused player must not restart the UI clock's pause linger.
    pub fn choose(&mut self, sessions: &[(u64, bool)], current: Option<u64>) -> Option<u64> {
        let contains = |id| sessions.iter().any(|&(s, _)| s == id);
        let playing = |id| sessions.iter().any(|&(s, p)| s == id && p);
        self.selected = current
            .filter(|&id| playing(id))
            .or(self.selected.filter(|&id| playing(id)))
            .or_else(|| sessions.iter().find(|(_, p)| *p).map(|(id, _)| *id))
            .or(self.selected.filter(|&id| contains(id)))
            .or_else(|| (!self.had_session).then_some(current).flatten().filter(|&id| contains(id)));
        self.had_session |= self.selected.is_some();
        self.selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{MediaClock, MediaSnapshot, Playback};
    use std::time::{Duration, Instant};

    #[test]
    fn timeline_is_relative_to_start_and_clamped() {
        assert_eq!(Timeline::new(100_000_000, 300_000_000, 150_000_000).seconds(), (5.0, 20.0));
        assert_eq!(Timeline::new(100, 200, 50).seconds(), (0.0, 0.00001));
        assert_eq!(Timeline::new(100, 200, 300).seconds(), (0.00001, 0.00001));
        assert_eq!(Timeline::new(100, 100, 500).seconds(), (0.00004, 0.0));
        assert_eq!(Timeline::new(i64::MIN, i64::MAX, 0).seconds().0, 922337203685.4775);
    }

    #[test]
    fn seek_converts_back_to_absolute_ticks_and_rejects_bad_input() {
        let t = Timeline::new(100_000_000, 300_000_000, 150_000_000);
        assert_eq!(t.seek_ticks(5.0), Some(150_000_000));
        assert_eq!(t.seek_ticks(-3.0), Some(100_000_000));
        assert_eq!(t.seek_ticks(100.0), Some(300_000_000));
        assert_eq!(t.seek_ticks(f64::NAN), None);
        assert_eq!(t.seek_ticks(f64::INFINITY), None);
        assert_eq!(t.seek_ticks(f64::MAX), Some(300_000_000));
        assert_eq!(Timeline::new(0, 0, 0).seek_ticks(1.0), None);
    }

    #[test]
    fn metadata_waits_for_new_timeline() {
        let mut g = TrackTimeline::default();
        let old = Timeline::new(0, 200_000_000, 190_000_000);
        assert_eq!(g.update("old".into(), old, false), (19.0, 20.0));
        assert_eq!(g.update("new".into(), old, false), (0.0, 0.0));
        assert_eq!(g.update("new".into(), old, false), (0.0, 0.0));
        let new = Timeline::new(0, 300_000_000, 0);
        assert_eq!(g.update("new".into(), new, true), (0.0, 30.0));
        // Same timeline values can be valid for a repeat or a new live stream.
        assert_eq!(g.update("next".into(), new, false), (0.0, 0.0));
        assert_eq!(g.update("next".into(), new, true), (0.0, 30.0));
    }

    #[test]
    fn changed_timeline_can_arrive_with_metadata() {
        let mut g = TrackTimeline::default();
        g.update("old".into(), Timeline::new(0, 100, 90), false);
        assert_eq!(g.update("new".into(), Timeline::new(0, 200_000_000, 0), false), (0.0, 20.0));
    }

    #[test]
    fn session_selection_keeps_paused_owner_until_playing_replacement() {
        let mut s = SessionSelection::default();
        assert_eq!(s.choose(&[(1, false), (2, false)], Some(1)), Some(1));
        assert_eq!(s.choose(&[(1, false), (2, false)], Some(2)), Some(1));
        assert_eq!(s.choose(&[(1, false), (2, true)], Some(1)), Some(2));
        assert_eq!(s.choose(&[(1, true), (2, true)], Some(1)), Some(1));
        assert_eq!(s.choose(&[(2, false)], Some(2)), None);
        assert_eq!(s.choose(&[(2, false)], Some(2)), None);
        assert_eq!(s.choose(&[(2, true)], Some(2)), Some(2));
        assert_eq!(s.choose(&[], None), None);
    }

    #[test]
    fn empty_start_does_not_prevent_first_session() {
        let mut s = SessionSelection::default();
        assert_eq!(s.choose(&[], None), None);
        assert_eq!(s.choose(&[(1, false)], Some(1)), Some(1));
    }

    #[test]
    fn other_paused_sessions_do_not_restart_linger() {
        let now = Instant::now();
        let mut s = SessionSelection::default();
        let mut clock = MediaClock::default();
        for (elapsed, current) in [(0, 1), (30, 2), (61, 2)] {
            let selected = s.choose(&[(1, false), (2, false)], Some(current)).unwrap();
            clock.update(
                Some(MediaSnapshot {
                    title: selected.to_string(),
                    artist: String::new(),
                    album: String::new(),
                    source_app: selected.to_string(),
                    playback: Playback::Paused,
                    position_s: 0.0,
                    duration_s: 100.0,
                }),
                now + Duration::from_secs(elapsed),
            );
        }
        assert!(!clock.is_present(now + Duration::from_secs(61)));
    }

    #[test]
    fn art_is_reread_on_metadata_even_after_success() {
        let mut art = ArtRetry::default();
        assert!(art.should_read("track", false)); // first snapshot
        assert!(!art.should_read("track", false)); // timeline/playback wake
        assert!(art.should_read("track", true)); // delayed metadata
        assert!(art.should_read("track", true)); // still retry after error
        assert!(art.should_publish(Some(42))); // successfully decoded art
        assert!(art.should_read("track", true));
        assert!(!art.should_read("track", false));
        assert!(art.should_read("next", false));
        assert!(art.should_read("next", true));
    }

    #[test]
    fn artwork_rejects_transparent_and_uniform_placeholders() {
        for pixels in [&[0, 0, 0, 0, 1, 1, 1, 4][..], &[90, 90, 90, 255, 90, 90, 90, 255][..], &[]] {
            assert!(!ArtFingerprint::new(pixels).visible);
        }
        let art = ArtFingerprint::new(&[0, 0, 0, 255, 0, 0, 200, 255]);
        assert!(art.visible);
        assert_eq!(art.opaque_fraction, 1.0);
        assert_eq!(ArtFingerprint::new(&[0, 0, 0, 0, 0, 0, 200, 255]).opaque_fraction, 0.5);
        assert_ne!(art.hash, ArtFingerprint::new(&[0, 0, 0, 255, 0, 0, 201, 255]).hash);
    }

    #[test]
    fn new_track_with_blank_art_clears_previous_tracks_art() {
        let mut art = ArtRetry::default();
        assert!(art.should_read("previous", false));
        assert!(art.should_publish(Some(42)));
        assert!(art.should_read("new", false));
        assert!(art.should_publish(None));
        assert!(!art.should_publish(None));
    }

    #[test]
    fn artwork_only_publishes_pixel_changes_including_removal() {
        let mut art = ArtRetry::default();
        assert!(art.should_read("track", false));
        assert!(art.should_publish(None));
        assert!(art.should_publish(Some(1)));
        assert!(!art.should_publish(Some(1)));
        assert!(art.should_publish(Some(2)));
        assert!(art.should_publish(None));
        assert!(!art.should_publish(None));
        assert!(art.should_publish(Some(2)));
        assert!(art.should_read("next", false));
        assert!(art.should_publish(Some(2))); // UI was cleared for the new track
    }
}
