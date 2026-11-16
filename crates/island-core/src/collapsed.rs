//! The collapsed island's centre piece: three dots whose arrangement shows the state, blending
//! from wherever they are to the next state's targets.

use crate::anim::ease;
use crate::view::Dot;

/// Seconds a change of state takes to blend into the next arrangement.
pub const MORPH: f32 = 0.35;
/// The record's diameter: the 32 px pill inset 1.5 px.
pub const RECORD_D: f32 = 29.0;

/// Colour as linear RGBA, 0..1.
pub type Rgba = [f32; 4];

const fn hex(rgb: u32) -> Rgba {
    [((rgb >> 16) & 0xff) as f32 / 255.0, ((rgb >> 8) & 0xff) as f32 / 255.0, (rgb & 0xff) as f32 / 255.0, 1.0]
}

const IDLE: Rgba = [1.0, 1.0, 1.0, 0.35];
const WORKING: Rgba = hex(0xf5f5f7);
const BLOCKED: Rgba = hex(0xff9500);
const DONE: Rgba = hex(0x34c759);

/// The values a state settles on, before any ongoing motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Base {
    /// Per dot: `[x, y, d]`, offset from the pill centre and diameter, px.
    pub dots: [[f32; 3]; 3],
    pub color: Rgba,
    /// Strength of Working's rise, 0..1.
    pub rise: f32,
    /// Strength of Needs-you's ripple, 0..1.
    pub ripple: f32,
    /// How far the record has grown out from behind the label, 0..1.
    pub record: f32,
    /// The record's label diameter, px.
    pub label_d: f32,
}

impl Base {
    fn lerp(&self, to: &Base, k: f32) -> Base {
        let m = |a: f32, b: f32| a + (b - a) * k;
        let mut out = *to;
        for (o, (a, b)) in out.dots.iter_mut().zip(self.dots.iter().zip(to.dots.iter())) {
            for j in 0..3 {
                o[j] = m(a[j], b[j]);
            }
        }
        for j in 0..4 {
            out.color[j] = m(self.color[j], to.color[j]);
        }
        out.rise = m(self.rise, to.rise);
        out.ripple = m(self.ripple, to.ripple);
        out.record = m(self.record, to.record);
        out.label_d = m(self.label_d, to.label_d);
        out
    }
}

/// Where `dot` (with or without media) settles.
pub fn target(dot: Dot, media: bool) -> Base {
    let stacked = |d: f32| [[0.0, 0.0, d]; 3];
    let (dots, color, label_d) = match dot {
        Dot::Idle => (stacked(9.0), IDLE, 14.0),
        Dot::Working => ([[-5.0, 0.0, 3.0], [0.0, 0.0, 3.0], [5.0, 0.0, 3.0]], WORKING, 16.0),
        Dot::Blocked => (stacked(9.0), BLOCKED, 14.0),
        Dot::Done => (stacked(11.0), DONE, 15.0),
    };
    Base {
        dots,
        color,
        rise: if dot == Dot::Working { 1.0 } else { 0.0 },
        ripple: if dot == Dot::Blocked { 1.0 } else { 0.0 },
        record: if media { 1.0 } else { 0.0 },
        label_d,
    }
}

/// The centre piece's state: what it shows, and the blend from where it was.
#[derive(Clone, Debug)]
pub struct Morph {
    dot: Dot,
    media: bool,
    from: Base,
    start: f64,
}

impl Morph {
    /// Settled on `dot` at `t`: nothing plays.
    pub fn new(dot: Dot, media: bool, t: f64) -> Morph {
        Morph { dot, media, from: target(dot, media), start: t - MORPH as f64 }
    }

    /// Retargets to `dot` / `media`, blending from the current values; `playing` says whether
    /// media is playing (it turns the record's sheen).
    pub fn set(&mut self, dot: Dot, media: bool, _playing: bool, t: f64) {
        if (dot, media) != (self.dot, self.media) {
            self.from = self.base(t);
            self.start = t;
            self.dot = dot;
            self.media = media;
        }
    }

    /// The blend's progress at `t`, 0..1, eased.
    fn k(&self, t: f64) -> f32 {
        ease(((t - self.start) as f32 / MORPH).clamp(0.0, 1.0))
    }

    /// The blended values at `t`, without ongoing motion.
    pub fn base(&self, t: f64) -> Base {
        self.from.lerp(&target(self.dot, self.media), self.k(t))
    }

    pub fn dot(&self) -> Dot {
        self.dot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Dot;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn base_approx(a: &Base, b: &Base) -> bool {
        let dots = a.dots.iter().flatten().zip(b.dots.iter().flatten()).all(|(x, y)| approx(*x, *y));
        let color = a.color.iter().zip(b.color.iter()).all(|(x, y)| approx(*x, *y));
        dots && color
            && approx(a.rise, b.rise)
            && approx(a.ripple, b.ripple)
            && approx(a.record, b.record)
            && approx(a.label_d, b.label_d)
    }

    #[test]
    fn targets() {
        let i = target(Dot::Idle, false);
        assert_eq!(i.dots, [[0.0, 0.0, 9.0]; 3]);
        assert_eq!(i.color, [1.0, 1.0, 1.0, 0.35]);
        assert_eq!((i.rise, i.ripple, i.record), (0.0, 0.0, 0.0));
        let w = target(Dot::Working, true);
        assert_eq!(w.dots, [[-5.0, 0.0, 3.0], [0.0, 0.0, 3.0], [5.0, 0.0, 3.0]]);
        assert_eq!((w.rise, w.record, w.label_d), (1.0, 1.0, 16.0));
        assert_eq!(target(Dot::Blocked, false).ripple, 1.0);
        assert_eq!(target(Dot::Blocked, true).label_d, 14.0);
        assert_eq!(target(Dot::Done, false).dots, [[0.0, 0.0, 11.0]; 3]);
        assert_eq!(target(Dot::Done, true).label_d, 15.0);
    }

    #[test]
    fn morph_blends_over_morph_time() {
        let mut m = Morph::new(Dot::Idle, false, 0.0);
        m.set(Dot::Working, false, false, 1.0);
        assert!(base_approx(&m.base(1.0), &target(Dot::Idle, false)));
        let mid = m.base(1.0 + MORPH as f64 / 2.0);
        assert!(mid.dots[0][0] < 0.0 && mid.dots[0][0] > -5.0);
        assert!(mid.dots[0][2] < 9.0 && mid.dots[0][2] > 3.0);
        assert!(base_approx(&m.base(1.0 + MORPH as f64), &target(Dot::Working, false)));
    }

    #[test]
    fn retarget_is_continuous() {
        let mut m = Morph::new(Dot::Idle, false, 0.0);
        m.set(Dot::Working, false, false, 1.0);
        let before = m.base(1.1);
        m.set(Dot::Done, false, false, 1.1);
        assert!(base_approx(&m.base(1.1), &before));
        assert!(base_approx(&m.base(1.1 + MORPH as f64), &target(Dot::Done, false)));
    }

    #[test]
    fn same_target_does_not_restart() {
        let mut m = Morph::new(Dot::Idle, false, 0.0);
        m.set(Dot::Working, false, false, 1.0);
        m.set(Dot::Working, false, true, 1.2); // only `playing` changed
        assert!(base_approx(&m.base(1.0 + MORPH as f64), &target(Dot::Working, false)));
    }

    #[test]
    fn media_toggle_morphs_record() {
        let mut m = Morph::new(Dot::Idle, false, 0.0);
        m.set(Dot::Idle, true, false, 1.0);
        let r = m.base(1.0 + MORPH as f64 / 2.0).record;
        assert!(r > 0.0 && r < 1.0);
    }
}
