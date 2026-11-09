//! Animation maths: the island spring, CSS easing, fades, the sound wave and the blocked pulse.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringParams {
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
}

/// framer-motion `{ type: "spring", stiffness: 380, damping: 28 }`.
pub const ISLAND_SPRING: SpringParams = SpringParams { stiffness: 380.0, damping: 28.0, mass: 1.0 };

/// Longest integration substep; keeps the spring identical at any frame rate.
const MAX_SUBSTEP: f32 = 1.0 / 480.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
}

impl Spring {
    pub fn new(v: f32) -> Spring {
        Spring { value: v, velocity: 0.0, target: v }
    }

    /// Advances by `dt` seconds (semi-implicit Euler); snaps to the target once settled.
    pub fn step(&mut self, p: SpringParams, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        let n = (dt / MAX_SUBSTEP).ceil().max(1.0) as u32;
        let h = dt / n as f32;
        for _ in 0..n {
            let force = -p.stiffness * (self.value - self.target) - p.damping * self.velocity;
            self.velocity += force / p.mass * h;
            self.value += self.velocity * h;
        }
        if self.is_settled() {
            self.value = self.target;
            self.velocity = 0.0;
        }
    }

    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < 0.5 && self.velocity.abs() < 1.0
    }
}

/// CSS `cubic-bezier(x1, y1, x2, y2)` evaluated at progress `t` (clamped to 0..=1).
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    // B(u) = 3(1-u)^2 u p1 + 3(1-u) u^2 p2 + u^3
    let curve = |a: f32, b: f32, u: f32| {
        let v = 1.0 - u;
        3.0 * v * v * u * a + 3.0 * v * u * u * b + u * u * u
    };
    let slope = |a: f32, b: f32, u: f32| {
        let v = 1.0 - u;
        3.0 * v * v * a + 6.0 * v * u * (b - a) + 3.0 * u * u * (1.0 - b)
    };
    let close = |u: f32| (curve(x1, x2, u) - t).abs() < 1e-6;
    let mut u = t;
    for _ in 0..8 {
        if close(u) {
            break;
        }
        let d = slope(x1, x2, u);
        if d.abs() < 1e-6 {
            break;
        }
        u = (u - (curve(x1, x2, u) - t) / d).clamp(0.0, 1.0);
    }
    if !close(u) {
        // Newton stalled; x(u) is monotonic on 0..=1, so bisect.
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..40 {
            u = (lo + hi) / 2.0;
            if close(u) {
                break;
            }
            if curve(x1, x2, u) < t { lo = u } else { hi = u }
        }
    }
    curve(y1, y2, u)
}

/// CSS `ease`.
pub fn ease(t: f32) -> f32 {
    cubic_bezier(0.25, 0.1, 0.25, 1.0, t)
}

/// CSS `ease-in-out`.
pub fn ease_in_out(t: f32) -> f32 {
    cubic_bezier(0.42, 0.0, 0.58, 1.0, t)
}

/// A one-shot eased transition between two values. Times are seconds on the app clock, in
/// f64: an f32 clock loses whole frames of precision after a few days of uptime.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    pub from: f32,
    pub to: f32,
    pub start: f64,
    pub duration: f32,
}

impl Fade {
    pub fn value_at(&self, t: f64) -> f32 {
        if self.done_at(t) {
            return self.to;
        }
        let p = ((t - self.start) as f32 / self.duration).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * ease(p)
    }

    pub fn done_at(&self, t: f64) -> bool {
        // elapsed time is small, so f32 is exact enough and matches `duration`'s precision
        (t - self.start) as f32 >= self.duration
    }
}

const SOUND_BAR_DURATION: [f32; 5] = [0.45, 0.55, 0.40, 0.50, 0.42];
const SOUND_BAR_DELAY: [f32; 5] = [0.0, 0.12, 0.24, 0.08, 0.18];

/// Resting heights of the five sound bars, as a fraction of the full bar.
pub const SOUND_BAR_BASE: [f32; 5] = [0.35, 0.65, 0.45, 0.85, 0.55];

/// Vertical scale of sound bar `bar` (0..5) at `t` seconds since the wave started.
pub fn sound_bar_scale(bar: usize, t: f64) -> f32 {
    let (dur, delay) = (SOUND_BAR_DURATION[bar] as f64, SOUND_BAR_DELAY[bar] as f64);
    if t < delay {
        return 1.0;
    }
    let cycles = (t - delay) / dur;
    let p = ease_in_out(cycles.fract() as f32);
    let rising = (cycles.floor() as u64).is_multiple_of(2);
    if rising { 0.25 + 0.75 * p } else { 1.0 - 0.75 * p }
}

/// Seconds per breath of the blocked dot.
pub const BLOCKED_PULSE_PERIOD: f32 = 1.2;

/// (opacity, scale) of a pulsing dot: (0.25, 0.9) at the ends of the period, (0.6, 1.1) halfway.
pub fn pulse(t: f64, period: f32) -> (f32, f32) {
    let phase = (t / period as f64).rem_euclid(1.0) as f32;
    let p = if phase < 0.5 { ease_in_out(phase * 2.0) } else { ease_in_out((1.0 - phase) * 2.0) };
    (0.25 + 0.35 * p, 0.9 + 0.2 * p)
}

/// The expanded art glow (§6.3): opacity 0.3↔0.6 and scale 0.95↔1.05, ease-in-out,
/// 3 s each way.
pub fn breathe(t: f64) -> (f32, f32) {
    let phase = (t / 6.0).rem_euclid(1.0) as f32;
    let p = if phase < 0.5 { ease_in_out(phase * 2.0) } else { ease_in_out((1.0 - phase) * 2.0) };
    (0.3 + 0.3 * p, 0.95 + 0.1 * p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six days of uptime: an island that starts with Windows easily runs this long.
    const UPTIME: f64 = 6.0 * 86_400.0;

    #[test]
    fn fades_advance_every_frame_after_days_of_uptime() {
        let f = Fade { from: 0.0, to: 1.0, start: UPTIME, duration: 0.2 };
        let mut last = 0.0;
        for i in 1..12 {
            let v = f.value_at(UPTIME + i as f64 / 60.0);
            assert!(v > last, "frame {i}: {v} <= {last}");
            last = v;
        }
    }

    #[test]
    fn sound_bars_move_every_frame_after_days_of_uptime() {
        let mut last = sound_bar_scale(0, UPTIME);
        for i in 1..12 {
            let v = sound_bar_scale(0, UPTIME + i as f64 / 60.0);
            assert!(v != last, "frame {i}: bar stuck at {v}");
            last = v;
        }
    }

    /// Runs the island spring from 40 to 360 at `fps` for a whole number of frames.
    fn run(fps: f32, secs: f32) -> (Spring, f32) {
        let mut s = Spring::new(40.0);
        s.target = 360.0;
        let mut max = 40.0f32;
        let frames = (secs * fps).round() as usize;
        for _ in 0..frames {
            s.step(ISLAND_SPRING, 1.0 / fps);
            max = max.max(s.value);
        }
        (s, max)
    }

    #[test]
    fn spring_overshoots_slightly_like_framer() {
        let (_, max) = run(60.0, 1.0);
        assert!(max > 361.0 && max < 376.0, "max {max}");
    }

    #[test]
    fn spring_settles_by_0_8s_not_by_0_2s() {
        assert!(run(60.0, 0.8).0.is_settled());
        assert!(!run(60.0, 0.2).0.is_settled());
    }

    #[test]
    fn spring_is_frame_rate_independent() {
        // 1/3 s is a whole number of frames at both 60 and 144 fps.
        let a = run(60.0, 1.0 / 3.0).0.value;
        let b = run(144.0, 1.0 / 3.0).0.value;
        assert!((a - b).abs() < 0.5, "{a} vs {b}");
    }

    #[test]
    fn settled_spring_snaps_to_target() {
        let (s, _) = run(60.0, 1.5);
        assert_eq!(s.value, 360.0);
        assert_eq!(s.velocity, 0.0);
    }

    #[test]
    fn ease_matches_css() {
        assert!((ease(0.5) - 0.8024).abs() < 1e-3, "{}", ease(0.5));
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn fade_runs_for_its_duration() {
        let f = Fade { from: 0.0, to: 1.0, start: 1.0, duration: 0.2 };
        assert_eq!(f.value_at(0.5), 0.0);
        assert_eq!(f.value_at(1.2), 1.0);
        assert!(f.done_at(1.2));
        assert!(!f.done_at(1.1));
    }

    #[test]
    fn sound_bars() {
        assert!((sound_bar_scale(0, 0.0) - 0.25).abs() < 1e-3);
        assert!((sound_bar_scale(0, 0.45) - 1.0).abs() < 1e-3);
        assert!((sound_bar_scale(0, 0.90) - 0.25).abs() < 1e-3);
        assert!((sound_bar_scale(0, 0.225) - 0.625).abs() < 1e-3);
        assert_eq!(sound_bar_scale(1, 0.05), 1.0); // before its 0.12 s delay
    }

    #[test]
    fn blocked_pulse() {
        let (o, s) = pulse(0.0, BLOCKED_PULSE_PERIOD);
        assert!((o - 0.25).abs() < 1e-3 && (s - 0.9).abs() < 1e-3);
        let (o, s) = pulse(0.6, BLOCKED_PULSE_PERIOD);
        assert!((o - 0.6).abs() < 1e-3 && (s - 1.1).abs() < 1e-3);
        let (o, _) = pulse(1.2, BLOCKED_PULSE_PERIOD);
        assert!((o - 0.25).abs() < 1e-3);
    }

    #[test]
    fn art_glow_breathes_over_3s_alternating() {
        let (o, s) = breathe(0.0);
        assert!((o - 0.3).abs() < 1e-3 && (s - 0.95).abs() < 1e-3);
        let (o, s) = breathe(3.0);
        assert!((o - 0.6).abs() < 1e-3 && (s - 1.05).abs() < 1e-3);
        let (o, _) = breathe(6.0);
        assert!((o - 0.3).abs() < 1e-3);
        let (o, _) = breathe(1.5);
        assert!(o > 0.3 && o < 0.6);
    }
}
