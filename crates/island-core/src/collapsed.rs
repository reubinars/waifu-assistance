//! The collapsed island's centre piece: three dots whose arrangement shows the state, blending
//! from wherever they are to the next state's targets.

use crate::Point;
use crate::anim::{cubic_bezier, ease, ease_in_out};
use crate::view::Dot;

/// Seconds a change of state takes to blend into the next arrangement.
pub const MORPH: f32 = 0.35;
/// The record's diameter: the 32 px pill inset 1.5 px.
pub const RECORD_D: f32 = 29.0;

/// Seconds the record's sheen takes to turn once while media plays.
pub const SHEEN_PERIOD: f32 = 8.0;

/// Working's rise: one cycle, and the delay between neighbouring dots.
const RISE_PERIOD: f64 = 1.2;
const RISE_STAGGER: f64 = 0.15;
/// How far a rising dot lifts, px, and its opacity at rest.
const RISE_LIFT: f32 = 2.0;
const RISE_REST_ALPHA: f32 = 0.45;
/// Needs-you's ripple: one cycle, how far the ring grows, its starting opacity.
const RIPPLE_PERIOD: f64 = 1.5;
const RIPPLE_GROW: f32 = 1.4;
const RIPPLE_ALPHA: f32 = 0.75;
/// Done's entry, in seconds from arriving: the pop's peak scale and timings, the tick's draw.
const POP_START: f64 = 0.35;
const POP_PEAK_AT: f64 = 0.45;
const POP_END: f64 = 0.65;
const POP_PEAK: f32 = 1.3;
const TICK_START: f64 = 0.45;
const TICK_END: f64 = 0.85;
/// The tick, centre-relative px (board 5's `M3 7.2 L6 10 L11 4.2` in a 14 box, halved).
const TICK: [(f32, f32); 3] = [(-2.0, 0.1), (-0.5, 1.5), (2.0, -1.4)];

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

/// One dot as drawn: offset from the pill centre, diameter, opacity (times the shared colour's).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DotPose {
    pub x: f32,
    pub y: f32,
    pub d: f32,
    pub alpha: f32,
}

/// Needs-you's ripple ring: diameter and opacity; drawn in the dots' colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub d: f32,
    pub alpha: f32,
}

/// Everything the collapsed centre piece draws at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub dots: [DotPose; 3],
    pub color: Rgba,
    /// How much of the Done tick is drawn, 0..1.
    pub tick: f32,
    pub ring: Option<Ring>,
    /// The record's diameter; 0 when there is no record.
    pub record_d: f32,
    pub label_d: f32,
    /// The sheen's angle, radians.
    pub sheen: f32,
}

/// The centre piece's state: what it shows, and the blend from where it was.
#[derive(Clone, Debug)]
pub struct Morph {
    dot: Dot,
    media: bool,
    from: Base,
    start: f64,
    /// When `dot` last changed: the rise, ripple, pop and tick run from here.
    arrived: f64,
    /// The tick when the blend started.
    tick_from: f32,
    /// The pose as drawn when the blend started; the drawn pose blends from it, so a change
    /// never jumps, even mid-rise, mid-pop or mid-ripple.
    from_pose: Pose,
    playing: bool,
    /// The sheen's angle at `sheen_at`.
    sheen_from: f64,
    sheen_at: f64,
}

impl Morph {
    /// Settled on `dot` at `t`: nothing plays.
    pub fn new(dot: Dot, media: bool, t: f64) -> Morph {
        let long_ago = t - 10.0;
        let mut m = Morph {
            dot,
            media,
            from: target(dot, media),
            start: long_ago,
            arrived: long_ago,
            tick_from: if dot == Dot::Done { 1.0 } else { 0.0 },
            from_pose: Pose::default(),
            playing: false,
            sheen_from: 0.0,
            sheen_at: t,
        };
        m.from_pose = m.raw(t);
        m
    }

    /// Retargets to `dot` / `media`, blending from the current values; `playing` says whether
    /// media is playing (it turns the record's sheen).
    pub fn set(&mut self, dot: Dot, media: bool, playing: bool, t: f64) {
        if playing != self.playing {
            self.sheen_from = self.sheen(t);
            self.sheen_at = t;
            self.playing = playing;
        }
        if (dot, media) == (self.dot, self.media) {
            return;
        }
        let drawn = self.pose(t);
        // within Done the tick runs on its own clock, which a media change leaves alone
        if !(dot == Dot::Done && self.dot == Dot::Done) {
            self.tick_from = drawn.tick;
        }
        if dot != self.dot {
            self.arrived = t;
        }
        self.from = self.base(t);
        self.from_pose = drawn;
        self.start = t;
        self.dot = dot;
        self.media = media;
    }

    /// Jumps to `dot` / `media` as if it had been there all along: no blend, pop or tick draw.
    /// For the app's start-up, before it knows the real state. The sheen keeps its angle.
    pub fn settle(&mut self, dot: Dot, media: bool, playing: bool, t: f64) {
        let sheen = self.sheen(t);
        *self = Morph::new(dot, media, t);
        self.sheen_from = sheen;
        self.playing = playing;
    }

    /// The blend's progress at `t`, 0..1, eased.
    fn k(&self, t: f64) -> f32 {
        ease(((t - self.start) / MORPH as f64).clamp(0.0, 1.0) as f32)
    }

    /// The blended values at `t`, without ongoing motion.
    pub fn base(&self, t: f64) -> Base {
        self.from.lerp(&target(self.dot, self.media), self.k(t))
    }

    pub fn dot(&self) -> Dot {
        self.dot
    }

    /// Seconds since `dot` last changed.
    fn since(&self, t: f64) -> f64 {
        t - self.arrived
    }

    fn tick(&self, t: f64) -> f32 {
        if self.dot == Dot::Done {
            let e = ease(((self.since(t) - TICK_START) / (TICK_END - TICK_START)).clamp(0.0, 1.0) as f32);
            mix(self.tick_from, 1.0, e)
        } else {
            mix(self.tick_from, 0.0, self.k(t))
        }
    }

    fn sheen(&self, t: f64) -> f64 {
        if !self.playing {
            return self.sheen_from;
        }
        let turned = std::f64::consts::TAU * (t - self.sheen_at) / SHEEN_PERIOD as f64;
        (self.sheen_from + turned).rem_euclid(std::f64::consts::TAU)
    }

    /// Done's pop: a scale on the dots' diameter.
    fn pop(&self, t: f64) -> f32 {
        if self.dot != Dot::Done {
            return 1.0;
        }
        let since = self.since(t);
        let up = |a: f64, b: f64| ease(((since - a) / (b - a)).clamp(0.0, 1.0) as f32);
        if since <= POP_START || since >= POP_END {
            1.0
        } else if since <= POP_PEAK_AT {
            mix(1.0, POP_PEAK, up(POP_START, POP_PEAK_AT))
        } else {
            mix(POP_PEAK, 1.0, up(POP_PEAK_AT, POP_END))
        }
    }

    /// Everything to draw at `t`.
    pub fn pose(&self, t: f64) -> Pose {
        let raw = self.raw(t);
        let k = self.k(t);
        if k >= 1.0 {
            return raw;
        }
        let f = &self.from_pose;
        let ring = match (f.ring, raw.ring) {
            (Some(a), Some(b)) => Some(Ring { d: mix(a.d, b.d, k), alpha: mix(a.alpha, b.alpha, k) }),
            (Some(a), None) => Some(Ring { d: a.d, alpha: mix(a.alpha, 0.0, k) }),
            (None, r) => r,
        };
        Pose {
            dots: std::array::from_fn(|i| {
                let (a, b) = (f.dots[i], raw.dots[i]);
                DotPose { x: mix(a.x, b.x, k), y: mix(a.y, b.y, k), d: mix(a.d, b.d, k), alpha: mix(a.alpha, b.alpha, k) }
            }),
            color: std::array::from_fn(|j| mix(f.color[j], raw.color[j], k)),
            ring,
            record_d: mix(f.record_d, raw.record_d, k),
            label_d: mix(f.label_d, raw.label_d, k),
            ..raw
        }
    }

    /// The pose at `t` before blending from the drawn pose: base values plus ongoing motion.
    fn raw(&self, t: f64) -> Pose {
        let b = self.base(t);
        let pop = self.pop(t);
        let dots = std::array::from_fn(|i| {
            let [x, y, d] = b.dots[i];
            let f = if b.rise > 0.0 { rise_at(self.since(t) - RISE_STAGGER * i as f64) } else { 0.0 };
            DotPose {
                x,
                y: y - RISE_LIFT * f * b.rise,
                d: d * pop,
                alpha: 1.0 - b.rise * (1.0 - mix(RISE_REST_ALPHA, 1.0, f)),
            }
        });
        let ring = (b.ripple > 0.0).then(|| {
            let phase = (self.since(t) / RIPPLE_PERIOD).rem_euclid(1.0) as f32;
            let e = cubic_bezier(0.0, 0.0, 0.58, 1.0, phase);
            Ring { d: b.dots[0][2] * (1.0 + RIPPLE_GROW * e), alpha: RIPPLE_ALPHA * (1.0 - e) * b.ripple }
        });
        Pose {
            dots,
            color: b.color,
            tick: self.tick(t),
            ring,
            record_d: if b.record > 0.0 { mix(b.label_d, RECORD_D, b.record) } else { 0.0 },
            label_d: b.label_d,
            sheen: self.sheen(t) as f32,
        }
    }

    /// Whether the pose still changes after `t`.
    pub fn busy(&self, t: f64) -> bool {
        t - self.start < MORPH as f64
            || matches!(self.dot, Dot::Working | Dot::Blocked)
            || (self.dot == Dot::Done && self.since(t) < TICK_END)
            || (self.media && self.playing)
    }
}

fn mix(a: f32, b: f32, k: f32) -> f32 {
    a * (1.0 - k) + b * k
}

/// How far a rising dot is lifted, 0..1, `local` seconds into its cycles (before 0 it waits):
/// up over the first 30 %, down by 60 %, then rests.
fn rise_at(local: f64) -> f32 {
    if local < 0.0 {
        return 0.0;
    }
    let ph = (local / RISE_PERIOD).fract() as f32;
    if ph < 0.3 {
        ease_in_out(ph / 0.3)
    } else if ph < 0.6 {
        1.0 - ease_in_out((ph - 0.3) / 0.3)
    } else {
        0.0
    }
}

/// The Done tick, centre-relative, drawn `progress` (0..1) of its length; fewer than two points
/// means nothing to draw.
pub fn tick_points(progress: f32) -> Vec<Point> {
    let pt = |(x, y): (f32, f32)| Point { x, y };
    if progress >= 1.0 {
        return TICK.iter().copied().map(pt).collect();
    }
    let legs: Vec<f32> = TICK.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).collect();
    let mut left = progress.max(0.0) * legs.iter().sum::<f32>();
    if left <= 0.0 {
        return Vec::new();
    }
    let mut out = vec![pt(TICK[0])];
    for (w, len) in TICK.windows(2).zip(legs) {
        if left >= len {
            out.push(pt(w[1]));
            left -= len;
        } else {
            let k = left / len;
            out.push(pt((w[0].0 + (w[1].0 - w[0].0) * k, w[0].1 + (w[1].1 - w[0].1) * k)));
            break;
        }
    }
    out
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

    #[test]
    fn new_starts_settled() {
        let m = Morph::new(Dot::Done, false, 100.0);
        let p = m.pose(100.0);
        assert_eq!(p.tick, 1.0);
        assert!(p.dots.iter().all(|d| approx(d.d, 11.0)));
        assert!(!m.busy(100.0));
    }

    #[test]
    fn done_pop_and_tick() {
        let mut m = Morph::new(Dot::Working, false, 0.0);
        m.set(Dot::Done, false, false, 10.0);
        assert!(approx(m.pose(10.35).dots[0].d, 11.0));
        assert!(approx(m.pose(10.45).dots[0].d, 11.0 * 1.3));
        assert!(approx(m.pose(10.65).dots[0].d, 11.0));
        assert_eq!(m.pose(10.45).tick, 0.0);
        assert_eq!(m.pose(10.85).tick, 1.0);
        assert!(m.busy(10.8) && !m.busy(10.9));
    }

    #[test]
    fn leaving_done_undraws_tick() {
        let mut m = Morph::new(Dot::Done, false, 0.0);
        m.set(Dot::Idle, false, false, 1.0);
        let half = m.pose(1.0 + MORPH as f64 / 2.0).tick;
        assert!(half > 0.0 && half < 1.0);
        assert_eq!(m.pose(1.0 + MORPH as f64).tick, 0.0);
    }

    #[test]
    fn done_reentry_redraws_from_current_tick() {
        let mut m = Morph::new(Dot::Done, false, 0.0);
        m.set(Dot::Idle, false, false, 1.0);
        let cur = m.pose(1.1).tick;
        m.set(Dot::Done, false, false, 1.1);
        assert!(approx(m.pose(1.1).tick, cur));
        assert!(approx(m.pose(1.1 + 0.45).tick, cur)); // waits for its 0.45 s start
        assert_eq!(m.pose(1.1 + 0.85).tick, 1.0);
    }

    #[test]
    fn working_rise_staggers_from_arrival() {
        let mut m = Morph::new(Dot::Idle, false, 0.0);
        m.set(Dot::Working, false, false, 1.0);
        // settled, dot 0 at phase 0.3 of its second cycle: top of the rise
        let p = m.pose(1.0 + 1.2 + 0.36);
        assert!(approx(p.dots[0].y, -2.0) && approx(p.dots[0].alpha, 1.0));
        // dot 2 has not started 0.2 s after arrival
        assert!(approx(m.pose(1.2).dots[2].y, 0.0));
        // resting part of the cycle
        let r = m.pose(1.0 + 1.2 + 0.9);
        assert!(approx(r.dots[0].y, 0.0) && approx(r.dots[0].alpha, 0.45));
        assert!(m.busy(50.0));
    }

    #[test]
    fn ripple_ring() {
        let mut m = Morph::new(Dot::Idle, false, 0.0);
        assert!(m.pose(0.0).ring.is_none());
        m.set(Dot::Blocked, false, false, 1.0);
        let start = m.pose(1.0 + 1.5 * 2.0).ring.unwrap();
        assert!(approx(start.d, 9.0) && approx(start.alpha, 0.75));
        let end = m.pose(1.0 + 1.5 * 3.0 - 1e-4).ring.unwrap();
        assert!((end.d - 21.6).abs() < 0.01 && end.alpha < 0.01);
        assert!(m.busy(100.0));
    }

    #[test]
    fn record_and_sheen() {
        let mut m = Morph::new(Dot::Idle, true, 0.0);
        assert!(approx(m.pose(0.0).record_d, RECORD_D) && approx(m.pose(0.0).label_d, 14.0));
        assert!(!m.busy(0.0));
        m.set(Dot::Idle, true, true, 1.0);
        assert!(m.busy(1.0));
        let a = m.pose(3.0).sheen; // 2 s playing = quarter turn
        assert!(approx(a, std::f32::consts::FRAC_PI_2));
        m.set(Dot::Idle, true, false, 3.0);
        assert!(approx(m.pose(10.0).sheen, a)); // frozen while paused
        m.set(Dot::Idle, true, true, 10.0);
        assert!(approx(m.pose(12.0).sheen, a + std::f32::consts::FRAC_PI_2)); // continues
        assert_eq!(Morph::new(Dot::Idle, false, 0.0).pose(0.0).record_d, 0.0);
    }

    #[test]
    fn idle_settled_is_static() {
        let mut m = Morph::new(Dot::Working, false, 0.0);
        m.set(Dot::Idle, false, false, 1.0);
        assert!(m.busy(1.2) && !m.busy(1.0 + MORPH as f64 + 1e-3));
    }

    #[test]
    fn tick_points_truncate() {
        assert!(tick_points(0.0).len() < 2);
        let full = tick_points(1.0);
        assert_eq!(full.len(), 3);
        assert_eq!((full[0].x, full[0].y), (-2.0, 0.1));
        assert_eq!((full[2].x, full[2].y), (2.0, -1.4));
        let half = tick_points(0.5); // first leg ≈ 2.05 of total ≈ 5.88, so half ends on leg 2
        assert_eq!(half.len(), 3);
    }

    #[test]
    fn long_uptime_is_finite() {
        let t = 6.0 * 86_400.0;
        let mut m = Morph::new(Dot::Idle, true, t);
        m.set(Dot::Working, true, true, t + 1.0);
        let p = m.pose(t + 2.0);
        assert!(p.dots.iter().all(|d| d.y.is_finite() && d.alpha.is_finite()) && p.sheen.is_finite());
    }

    fn assert_pose_close(a: &Pose, b: &Pose, what: &str) {
        let near = |x: f32, y: f32| (x - y).abs() < 1e-3;
        for (da, db) in a.dots.iter().zip(&b.dots) {
            assert!(near(da.x, db.x) && near(da.y, db.y) && near(da.d, db.d) && near(da.alpha, db.alpha), "{what}: dots {a:?} vs {b:?}");
        }
        assert!(a.color.iter().zip(&b.color).all(|(x, y)| near(*x, *y)), "{what}: color");
        assert!(near(a.tick, b.tick), "{what}: tick {} vs {}", a.tick, b.tick);
        assert!(near(a.record_d, b.record_d) && near(a.label_d, b.label_d), "{what}: record");
        let ring = |p: &Pose| p.ring.map_or((0.0, 0.0), |r| (r.d, r.alpha));
        let (ra, rb) = (ring(a), ring(b));
        // a ring at zero opacity is the same picture as no ring
        assert!(near(ra.1, rb.1) && (ra.1 < 1e-3 || near(ra.0, rb.0)), "{what}: ring {ra:?} vs {rb:?}");
    }

    #[test]
    fn any_change_leaves_the_drawn_pose_unchanged() {
        let states: Vec<(Dot, bool)> =
            [Dot::Idle, Dot::Working, Dot::Blocked, Dot::Done].into_iter().flat_map(|d| [(d, false), (d, true)]).collect();
        for &a in &states {
            for &b in &states {
                for &c in &states {
                    for dt in [0.1, 0.2, 0.36, 0.5, 0.6, 0.8, 1.3, 2.0] {
                        let mut m = Morph::new(a.0, a.1, 0.0);
                        m.set(b.0, b.1, false, 1.0);
                        let before = m.pose(1.0 + dt);
                        m.set(c.0, c.1, false, 1.0 + dt);
                        assert_pose_close(&before, &m.pose(1.0 + dt), &format!("{a:?} -> {b:?} -> {c:?} at +{dt}"));
                    }
                }
            }
        }
    }

    #[test]
    fn settle_jumps_to_the_state_without_playing_it() {
        let mut m = Morph::new(Dot::Idle, false, 0.0);
        m.settle(Dot::Done, true, false, 1.0);
        let p = m.pose(1.0);
        assert_eq!(p.tick, 1.0);
        assert!(p.dots.iter().all(|d| d.d == 11.0));
        assert_eq!(p.record_d, RECORD_D);
        assert!(!m.busy(1.0));
    }
}
