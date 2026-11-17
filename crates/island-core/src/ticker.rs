//! Scroll maths for the usage strip's auto-scrolling one-line ticker.

/// Scroll speed in px/s.
pub const SPEED: f32 = 30.0;
/// px between the end of the text and its repeat.
pub const GAP: f32 = 32.0;
/// px per wheel notch.
pub const NOTCH: f32 = 40.0;

/// Horizontal scroll state for one ticker line.
///
/// The offset is kept in `0..period()` while the content overflows the strip;
/// otherwise the text is static and `offset()` reports 0.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ticker {
    offset: f32,
    content_w: f32,
    strip_w: f32,
}

impl Ticker {
    /// Updates the measured widths. While the content scrolls the offset is kept, wrapped into
    /// the new period; once it fits the offset is cleared, so scrolling restarts from the start.
    pub fn set_widths(&mut self, content_w: f32, strip_w: f32) {
        self.content_w = content_w;
        self.strip_w = strip_w;
        self.offset = if self.scrolls() { wrap(self.offset, self.period()) } else { 0.0 };
    }

    /// True when the content is wider than the strip and so scrolls.
    pub fn scrolls(&self) -> bool {
        self.content_w > self.strip_w
    }

    /// Length of one full cycle: the content plus the gap before its repeat.
    pub fn period(&self) -> f32 {
        self.content_w + GAP
    }

    /// Advances the scroll by `SPEED * dt` px. No-op unless `scrolls()`.
    pub fn advance(&mut self, dt: f32) {
        if !self.scrolls() {
            return;
        }
        self.offset = wrap(self.offset + SPEED * dt, self.period());
    }

    /// Scrolls by `notches` wheel notches. Positive notches move the text right,
    /// toward its start, matching `scroll_by`. No-op unless `scrolls()`.
    pub fn wheel(&mut self, notches: i32) {
        if !self.scrolls() {
            return;
        }
        self.offset = wrap(self.offset - notches as f32 * NOTCH, self.period());
    }

    /// Current offset in `0..period()`, or 0 when the content does not scroll.
    pub fn offset(&self) -> f32 {
        if self.scrolls() { self.offset } else { 0.0 }
    }

    /// Returns the offset to zero. The widths are kept.
    pub fn reset(&mut self) {
        self.offset = 0.0;
    }
}

/// Wraps `x` into `0..period`. A non-finite `x`, or a period that is not positive (NaN
/// included), yields 0.
fn wrap(x: f32, period: f32) -> f32 {
    if !x.is_finite() || !(period > 0.0) {
        return 0.0;
    }
    let r = x.rem_euclid(period);
    // rem_euclid can return `period` itself for tiny negative inputs due to rounding.
    if r >= period { 0.0 } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(content: f32, strip: f32) -> Ticker {
        let mut t = Ticker::default();
        t.set_widths(content, strip);
        t
    }
    #[test]
    fn static_when_it_fits() {
        let mut k = t(300.0, 344.0);
        k.advance(10.0);
        k.wheel(-3);
        assert!(!k.scrolls());
        assert_eq!(k.offset(), 0.0);
    }
    #[test]
    fn advances_and_wraps() {
        let mut k = t(500.0, 344.0); // period 532
        k.advance(2.0);
        assert_eq!(k.offset(), 60.0);
        k.advance(17.0);
        assert!((k.offset() - (570.0 - 532.0)).abs() < 1e-3);
    }
    #[test]
    fn wheel_both_ways_wraps() {
        let mut k = t(500.0, 344.0);
        k.wheel(-1);
        assert_eq!(k.offset(), 40.0);
        k.wheel(2);
        assert!((k.offset() - (532.0 - 40.0)).abs() < 1e-3);
    }
    #[test]
    fn width_change_keeps_offset_in_range() {
        let mut k = t(1000.0, 344.0);
        k.advance(30.0); // 900
        k.set_widths(400.0, 344.0); // period 432
        assert!((k.offset() - (900.0 % 432.0)).abs() < 1e-3);
        k.set_widths(200.0, 344.0);
        assert_eq!(k.offset(), 0.0);
    }
    #[test]
    fn non_finite_dt_zeroes_and_recovers() {
        let mut k = t(500.0, 344.0);
        k.advance(1.0);
        k.advance(f32::INFINITY);
        assert_eq!(k.offset(), 0.0);
        k.advance(1.0);
        k.advance(f32::NAN);
        assert_eq!(k.offset(), 0.0);
        k.advance(2.0);
        assert_eq!(k.offset(), 60.0);
    }
    #[test]
    fn fitting_clears_the_offset() {
        let mut k = t(500.0, 344.0);
        k.advance(2.0); // 60
        k.set_widths(300.0, 344.0);
        assert_eq!(k.offset(), 0.0);
        k.set_widths(500.0, 344.0);
        assert_eq!(k.offset(), 0.0);
    }
    #[test]
    fn reset_zeroes() {
        let mut k = t(500.0, 344.0);
        k.advance(1.0);
        k.reset();
        assert_eq!(k.offset(), 0.0);
        assert!(k.scrolls());
    }
}
