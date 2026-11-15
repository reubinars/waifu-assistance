//! When a GitHub event pops the collapsed island open, and for how long.

use std::time::{Duration, Instant};

use super::{GhEvent, GhState};

/// How long a pop stays up once the pointer is off it.
pub const POP_SECS: Duration = Duration::from_secs(4);
/// While an expired pop is hovered, how often to look again.
pub const POP_HOVER_RECHECK: Duration = Duration::from_millis(250);
/// After a pop closes, the island collapses before the next one shows.
pub const POP_GAP: Duration = Duration::from_millis(400);

/// What the island is doing besides the pop.
#[derive(Clone, Copy, Debug, Default)]
pub struct PopWorld {
    /// Fullscreen or hidden from the tray: pops are dropped (unseen stays).
    pub suppressed: bool,
    /// An approval popup shows or the user expanded the island: the pop waits.
    pub held: bool,
    /// The pointer is over the pop: it stays past `POP_SECS`.
    pub hovered: bool,
}

/// The pop being shown (or held), when it appeared, and the pause after one closes.
#[derive(Default)]
pub struct PopSlot {
    event: Option<GhEvent>,
    shown_at: Option<Instant>,
    gap_until: Option<Instant>,
}

impl PopSlot {
    /// The event on screen; `None` while nothing shows or the pop is held.
    pub fn showing(&self) -> Option<&GhEvent> {
        self.shown_at.and(self.event.as_ref())
    }

    /// Advances the pop for this moment; returns whether what shows changed.
    pub fn tick(&mut self, gh: Option<&mut GhState>, w: PopWorld, now: Instant) -> bool {
        let was = self.showing().is_some();
        match gh {
            None => *self = PopSlot::default(),
            Some(gh) if w.suppressed => {
                gh.drop_pops();
                *self = PopSlot::default();
            }
            Some(_) if w.held => self.shown_at = None,
            Some(gh) => match (self.shown_at, &self.event) {
                (Some(at), _) => {
                    if now.saturating_duration_since(at) >= POP_SECS && !w.hovered {
                        self.close(now);
                    }
                }
                // back from being held: the same event, a fresh 4 s
                (None, Some(_)) => self.shown_at = Some(now),
                (None, None) => {
                    if self.gap_until.is_some_and(|g| now < g) {
                        return was;
                    }
                    self.gap_until = None;
                    if let Some(e) = gh.next_pop() {
                        self.event = Some(e);
                        self.shown_at = Some(now);
                    }
                }
            },
        }
        was != self.showing().is_some()
    }

    /// The pop was clicked: it goes, and the next waits out the gap.
    pub fn dismiss(&mut self, now: Instant) {
        self.close(now);
    }

    /// The event was seen on the GitHub tab: a held pop does not come back. No gap follows.
    pub fn forget(&mut self) {
        self.event = None;
        self.shown_at = None;
    }

    fn close(&mut self, now: Instant) {
        self.event = None;
        self.shown_at = None;
        self.gap_until = Some(now + POP_GAP);
    }

    /// How long until `tick` should run again, if anything is pending on time.
    pub fn wake(&self, hovered: bool, now: Instant) -> Option<Duration> {
        match (self.shown_at, self.event.is_some()) {
            (Some(at), _) => {
                let left = POP_SECS.saturating_sub(now.saturating_duration_since(at));
                Some(if left.is_zero() && hovered { POP_HOVER_RECHECK } else { left })
            }
            // held: whatever ends the hold runs the next tick
            (None, true) => None,
            (None, false) => self.gap_until.map(|g| g.saturating_duration_since(now)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_pr as pr;
    use super::super::{Ci, GhEventKind, Review, Role};
    use super::*;

    const IDLE: PopWorld = PopWorld { suppressed: false, held: false, hovered: false };
    const SUPPRESSED: PopWorld = PopWorld { suppressed: true, ..IDLE };
    const HELD: PopWorld = PopWorld { held: true, ..IDLE };
    const HOVERED: PopWorld = PopWorld { hovered: true, ..IDLE };

    /// A state with `n` CI-failed events queued.
    fn queued(n: u32) -> GhState {
        let mut s = GhState::default();
        s.apply(Ok((1..=n).map(|i| pr("o/a", i, Role::Mine, Ci::Running, Review::None)).collect()));
        s.apply(Ok((1..=n).map(|i| pr("o/a", i, Role::Mine, Ci::Failing, Review::None)).collect()));
        s
    }

    fn shown_number(p: &PopSlot) -> Option<u32> {
        p.showing().map(|e| e.number)
    }

    #[test]
    fn idle_shows_the_next_pop() {
        let (mut gh, mut p, t) = (queued(2), PopSlot::default(), Instant::now());
        assert!(p.tick(Some(&mut gh), IDLE, t));
        assert_eq!(p.showing().map(|e| e.kind), Some(GhEventKind::CiFailed));
        assert_eq!(shown_number(&p), Some(1));
        // a second tick at once changes nothing
        assert!(!p.tick(Some(&mut gh), IDLE, t));
        assert_eq!(shown_number(&p), Some(1));
    }

    #[test]
    fn suppressed_drops_the_queue_but_not_unseen() {
        let (mut gh, mut p, t) = (queued(2), PopSlot::default(), Instant::now());
        assert!(!p.tick(Some(&mut gh), SUPPRESSED, t));
        assert_eq!(p.showing(), None);
        assert_eq!(gh.unseen, 2);
        // nothing left once it is lifted
        assert!(!p.tick(Some(&mut gh), IDLE, t));
        assert_eq!(p.showing(), None);
        assert_eq!(gh.unseen, 2);
    }

    #[test]
    fn suppressed_closes_a_showing_pop_for_good() {
        let (mut gh, mut p, t) = (queued(2), PopSlot::default(), Instant::now());
        p.tick(Some(&mut gh), IDLE, t);
        assert!(p.tick(Some(&mut gh), SUPPRESSED, t + Duration::from_secs(1)));
        assert_eq!(p.showing(), None);
        assert!(!p.tick(Some(&mut gh), IDLE, t + Duration::from_secs(10)));
        assert_eq!(p.showing(), None);
        assert_eq!(gh.unseen, 2);
    }

    #[test]
    fn never_shows_while_held() {
        let (mut gh, mut p, t) = (queued(1), PopSlot::default(), Instant::now());
        for s in 0..10 {
            assert!(!p.tick(Some(&mut gh), HELD, t + Duration::from_secs(s)));
            assert_eq!(p.showing(), None);
        }
        // the queue was left alone
        assert!(p.tick(Some(&mut gh), IDLE, t + Duration::from_secs(10)));
        assert_eq!(shown_number(&p), Some(1));
    }

    #[test]
    fn held_pop_reshows_with_a_fresh_stamp() {
        let (mut gh, mut p, t) = (queued(2), PopSlot::default(), Instant::now());
        p.tick(Some(&mut gh), IDLE, t);
        // an approval takes over 3 s in and stays 10 s
        assert!(p.tick(Some(&mut gh), HELD, t + Duration::from_secs(3)));
        assert_eq!(p.showing(), None);
        assert!(!p.tick(Some(&mut gh), HELD, t + Duration::from_secs(13)));
        // the same event comes back, for a full 4 s
        let back = t + Duration::from_secs(13);
        assert!(p.tick(Some(&mut gh), IDLE, back));
        assert_eq!(shown_number(&p), Some(1));
        assert!(!p.tick(Some(&mut gh), IDLE, back + POP_SECS - Duration::from_millis(1)));
        assert_eq!(shown_number(&p), Some(1));
        assert!(p.tick(Some(&mut gh), IDLE, back + POP_SECS));
        assert_eq!(p.showing(), None);
    }

    #[test]
    fn closes_after_pop_secs_only_when_not_hovered() {
        let (mut gh, mut p, t) = (queued(1), PopSlot::default(), Instant::now());
        p.tick(Some(&mut gh), IDLE, t);
        assert!(!p.tick(Some(&mut gh), IDLE, t + POP_SECS - Duration::from_millis(1)));
        assert!(!p.tick(Some(&mut gh), HOVERED, t + POP_SECS + Duration::from_secs(5)));
        assert_eq!(shown_number(&p), Some(1));
        assert!(p.tick(Some(&mut gh), IDLE, t + POP_SECS + Duration::from_secs(5)));
        assert_eq!(p.showing(), None);
    }

    #[test]
    fn next_pop_shows_after_the_gap() {
        let (mut gh, mut p, t) = (queued(2), PopSlot::default(), Instant::now());
        p.tick(Some(&mut gh), IDLE, t);
        let closed = t + POP_SECS;
        assert!(p.tick(Some(&mut gh), IDLE, closed));
        assert_eq!(p.wake(false, closed), Some(POP_GAP));
        assert!(!p.tick(Some(&mut gh), IDLE, closed + POP_GAP - Duration::from_millis(1)));
        assert!(p.tick(Some(&mut gh), IDLE, closed + POP_GAP));
        assert_eq!(shown_number(&p), Some(2));
    }

    #[test]
    fn dismiss_closes_and_waits_the_gap() {
        let (mut gh, mut p, t) = (queued(2), PopSlot::default(), Instant::now());
        p.tick(Some(&mut gh), IDLE, t);
        p.dismiss(t + Duration::from_secs(1));
        assert_eq!(p.showing(), None);
        assert!(!p.tick(Some(&mut gh), IDLE, t + Duration::from_secs(1)));
        assert!(p.tick(Some(&mut gh), IDLE, t + Duration::from_secs(1) + POP_GAP));
        assert_eq!(shown_number(&p), Some(2));
    }

    #[test]
    fn seen_drops_the_held_pop_and_the_queue() {
        let (mut gh, mut p, t) = (queued(2), PopSlot::default(), Instant::now());
        p.tick(Some(&mut gh), IDLE, t);
        // the user expands onto the GitHub tab: the pop is held, then seen
        p.tick(Some(&mut gh), HELD, t + Duration::from_secs(1));
        gh.seen();
        p.forget();
        assert_eq!(p.wake(false, t + Duration::from_secs(1)), None);
        // collapsing brings nothing back
        assert!(!p.tick(Some(&mut gh), IDLE, t + Duration::from_secs(2)));
        assert_eq!(p.showing(), None);
    }

    #[test]
    fn no_github_clears_the_pop() {
        let (mut gh, mut p, t) = (queued(1), PopSlot::default(), Instant::now());
        p.tick(Some(&mut gh), IDLE, t);
        assert!(p.tick(None, IDLE, t));
        assert_eq!(p.showing(), None);
    }

    #[test]
    fn wake_times() {
        let (mut gh, mut p, t) = (queued(1), PopSlot::default(), Instant::now());
        assert_eq!(p.wake(false, t), None);
        p.tick(Some(&mut gh), IDLE, t);
        assert_eq!(p.wake(false, t + Duration::from_secs(1)), Some(Duration::from_secs(3)));
        assert_eq!(p.wake(true, t + POP_SECS), Some(POP_HOVER_RECHECK));
        assert_eq!(p.wake(false, t + POP_SECS + Duration::from_secs(1)), Some(Duration::ZERO));
        // held: nothing on a clock
        p.tick(Some(&mut gh), HELD, t + Duration::from_secs(1));
        assert_eq!(p.wake(false, t + Duration::from_secs(1)), None);
    }
}
