//! The now-playing clock: estimates the position between SMTC updates and decides when media is shown.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Playback {
    Playing,
    Paused,
    Stopped,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaSnapshot {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playback: Playback,
    pub position_s: f64,
    /// 0 = unknown (live streams, some browsers).
    pub duration_s: f64,
    pub source_app: String,
}

impl MediaSnapshot {
    fn same_track(&self, other: &MediaSnapshot) -> bool {
        self.title == other.title && self.artist == other.artist && self.album == other.album
    }
}

/// Seconds media stays on the island after it stops playing.
pub const LINGER_SECS: u64 = 60;

/// A backward step smaller than this within one track is reporting jitter, not a seek.
const JITTER_SECS: f64 = 2.0;

#[derive(Default)]
pub struct MediaClock {
    snap: Option<MediaSnapshot>,
    updated_at: Option<Instant>,
    last_shown: f64,
    paused_at: Option<Instant>,
}

impl MediaClock {
    pub fn update(&mut self, snap: Option<MediaSnapshot>, now: Instant) {
        let Some(snap) = snap else {
            *self = MediaClock::default();
            return;
        };
        let same = self.snap.as_ref().is_some_and(|old| old.same_track(&snap));
        if !same {
            self.last_shown = 0.0;
        }
        self.paused_at = match (snap.playback, self.paused_at) {
            (Playback::Playing, _) => None,
            (_, Some(t)) if same => Some(t),
            _ => Some(now),
        };
        self.snap = Some(snap);
        self.updated_at = Some(now);
    }

    pub fn snapshot(&self) -> Option<&MediaSnapshot> {
        self.snap.as_ref()
    }

    /// Estimated position in seconds: never steps back by mere jitter within a track,
    /// clamped to the duration when it is known.
    pub fn position(&mut self, now: Instant) -> f64 {
        let (Some(s), Some(at)) = (&self.snap, self.updated_at) else { return 0.0 };
        let mut pos = s.position_s;
        if s.playback == Playback::Playing {
            pos += now.saturating_duration_since(at).as_secs_f64();
        }
        if s.duration_s > 0.0 {
            pos = pos.min(s.duration_s);
        }
        pos = pos.max(0.0);
        if pos < self.last_shown && self.last_shown - pos < JITTER_SECS {
            return self.last_shown;
        }
        self.last_shown = pos;
        pos
    }

    pub fn seekable(&self) -> bool {
        self.snap.as_ref().is_some_and(|s| s.duration_s > 0.0)
    }

    pub fn progress(&mut self, now: Instant) -> f32 {
        if !self.seekable() {
            return 0.0;
        }
        let dur = self.snap.as_ref().map_or(1.0, |s| s.duration_s);
        (self.position(now) / dur).clamp(0.0, 1.0) as f32
    }

    pub fn is_present(&self, now: Instant) -> bool {
        if self.snap.is_none() {
            return false;
        }
        match self.paused_at {
            None => true,
            Some(t) => now.saturating_duration_since(t) < Duration::from_secs(LINGER_SECS),
        }
    }

    pub fn is_playing(&self) -> bool {
        self.snap.as_ref().is_some_and(|s| s.playback == Playback::Playing)
    }
}

#[cfg(test)]
mod tests {
    use super::Playback::*;
    use super::*;
    use std::time::Duration;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn snap(pos: f64, dur: f64, p: Playback) -> MediaSnapshot {
        MediaSnapshot {
            title: "T".into(),
            artist: "A".into(),
            album: "L".into(),
            playback: p,
            position_s: pos,
            duration_s: dur,
            source_app: "Spotify.exe".into(),
        }
    }

    #[test]
    fn estimates_while_playing() {
        let t0 = Instant::now();
        let mut c = MediaClock::default();
        c.update(Some(snap(10.0, 200.0, Playing)), t0);
        assert!((c.position(t0 + ms(2500)) - 12.5).abs() < 0.01);
        assert!((c.progress(t0 + ms(2500)) - 12.5 / 200.0).abs() < 1e-4);
    }

    #[test]
    fn holds_while_paused() {
        let t0 = Instant::now();
        let mut c = MediaClock::default();
        c.update(Some(snap(10.0, 200.0, Paused)), t0);
        assert_eq!(c.position(t0 + ms(5000)), 10.0);
        assert!(!c.is_playing());
    }

    #[test]
    fn clamps_to_duration() {
        let t0 = Instant::now();
        let mut c = MediaClock::default();
        c.update(Some(snap(199.0, 200.0, Playing)), t0);
        assert_eq!(c.position(t0 + ms(5000)), 200.0);
        assert_eq!(c.progress(t0 + ms(5000)), 1.0);
    }

    #[test]
    fn never_goes_back_within_track() {
        let t0 = Instant::now();
        let mut c = MediaClock::default();
        c.update(Some(snap(10.0, 200.0, Playing)), t0);
        assert!((c.position(t0 + ms(3000)) - 13.0).abs() < 0.01);
        c.update(Some(snap(12.8, 200.0, Playing)), t0 + ms(3000));
        assert!(c.position(t0 + ms(3000)) >= 13.0);
    }

    #[test]
    fn seek_backwards_is_followed() {
        let t0 = Instant::now();
        let mut c = MediaClock::default();
        c.update(Some(snap(100.0, 200.0, Playing)), t0);
        c.position(t0 + ms(1000));
        c.update(Some(snap(20.0, 200.0, Playing)), t0 + ms(1000));
        assert!((c.position(t0 + ms(1000)) - 20.0).abs() < 0.01);
    }

    #[test]
    fn resets_on_new_track() {
        let t0 = Instant::now();
        let mut c = MediaClock::default();
        c.update(Some(snap(10.0, 200.0, Playing)), t0);
        c.position(t0 + ms(3000));
        let mut next = snap(0.0, 180.0, Playing);
        next.title = "U".into();
        c.update(Some(next), t0 + ms(3000));
        assert_eq!(c.position(t0 + ms(3000)), 0.0);
    }

    #[test]
    fn unknown_duration_is_not_seekable() {
        let t0 = Instant::now();
        let mut c = MediaClock::default();
        c.update(Some(snap(5.0, 0.0, Playing)), t0);
        assert!(!c.seekable());
        assert_eq!(c.progress(t0 + ms(1000)), 0.0);
        assert!((c.position(t0 + ms(1000)) - 6.0).abs() < 0.01);
    }

    #[test]
    fn lingers_60s_after_pause() {
        let t0 = Instant::now();
        let mut c = MediaClock::default();
        c.update(Some(snap(10.0, 200.0, Playing)), t0);
        assert!(c.is_present(t0 + Duration::from_secs(3600)));
        c.update(Some(snap(10.0, 200.0, Paused)), t0);
        // a repeated paused update does not restart the linger
        c.update(Some(snap(10.0, 200.0, Paused)), t0 + Duration::from_secs(30));
        assert!(c.is_present(t0 + Duration::from_secs(59)));
        assert!(!c.is_present(t0 + Duration::from_secs(61)));
        c.update(Some(snap(10.0, 200.0, Playing)), t0 + Duration::from_secs(62));
        assert!(c.is_present(t0 + Duration::from_secs(62)));
        c.update(None, t0 + Duration::from_secs(63));
        assert!(!c.is_present(t0 + Duration::from_secs(63)));
        assert!(c.snapshot().is_none());
    }
}
