//! Everything that moves: the island's size springs, the content cross-fade and colour fades.

use crate::draw::Color;
use island_core::Size;
use island_core::anim::{Fade, ISLAND_SPRING, Spring, ease};
use island_core::view::Tab;

/// What the island's content shows; a change cross-fades.
#[derive(Clone, Debug, PartialEq)]
pub enum Face {
    Idle,
    Media,
    Working { count: usize },
    Done { label: String },
    Blocked { label: String },
    Expanded(Tab),
}

const CROSS_FADE: f32 = 0.2;
const GLOW_FADE: f32 = 0.8;
const WAVE_FADE: f32 = 0.6;
const HOVER_FADE: f32 = 0.2;

#[derive(Clone, Copy)]
struct ColorFade {
    from: Color,
    to: Color,
    start: f64,
    duration: f32,
}

impl ColorFade {
    fn still(c: Color) -> ColorFade {
        ColorFade { from: c, to: c, start: 0.0, duration: 0.0 }
    }

    fn value_at(&self, t: f64) -> Color {
        if self.done_at(t) {
            return self.to;
        }
        self.from.lerp(self.to, ease(((t - self.start) as f32 / self.duration).clamp(0.0, 1.0)))
    }

    fn done_at(&self, t: f64) -> bool {
        (t - self.start) as f32 >= self.duration
    }

    fn retarget(&mut self, to: Color, t: f64, duration: f32) {
        if to != self.to {
            *self = ColorFade { from: self.value_at(t), to, start: t, duration };
        }
    }
}

pub struct Animator {
    pub w: Spring,
    pub h: Spring,
    pub r: Spring,
    pub face: Face,
    /// The face being faded out, and when the swap started.
    pub old: Option<Face>,
    swap_at: f64,
    glow: Fade,
    glow_color: ColorFade,
    wave_color: ColorFade,
    /// Hover on the collapsed media pill.
    hover: Fade,
    last: Option<f64>,
}

impl Animator {
    pub fn new(size: Size, radius: f32, face: Face) -> Animator {
        Animator {
            w: Spring::new(size.w),
            h: Spring::new(size.h),
            r: Spring::new(radius),
            face,
            old: None,
            swap_at: -1.0,
            glow: Fade { from: 0.0, to: 0.0, start: 0.0, duration: 0.0 },
            glow_color: ColorFade::still(Color::white(1.0)),
            wave_color: ColorFade::still(Color::white(1.0)),
            hover: Fade { from: 0.0, to: 0.0, start: 0.0, duration: 0.0 },
            last: None,
        }
    }

    pub fn set_target(&mut self, size: Size, radius: f32) {
        self.w.target = size.w;
        self.h.target = size.h;
        self.r.target = radius;
    }

    pub fn set_face(&mut self, face: Face, t: f64) {
        if face != self.face {
            self.old = Some(std::mem::replace(&mut self.face, face));
            self.swap_at = t;
        }
    }

    /// Glow on (1) or off (0), and its colour.
    pub fn set_glow(&mut self, on: bool, color: Color, t: f64) {
        let to = if on { 1.0 } else { 0.0 };
        if to != self.glow.to {
            self.glow = Fade { from: self.glow.value_at(t), to, start: t, duration: GLOW_FADE };
        }
        self.glow_color.retarget(color, t, GLOW_FADE);
    }

    pub fn set_wave_color(&mut self, color: Color, t: f64) {
        self.wave_color.retarget(color, t, WAVE_FADE);
    }

    pub fn set_hover(&mut self, on: bool, t: f64) {
        let to = if on { 1.0 } else { 0.0 };
        if to != self.hover.to {
            self.hover = Fade { from: self.hover.value_at(t), to, start: t, duration: HOVER_FADE };
        }
    }

    pub fn hover(&self, t: f64) -> f32 {
        self.hover.value_at(t)
    }

    /// Advances the springs to `t`.
    pub fn step(&mut self, t: f64) {
        let dt = self.last.map_or(0.0, |l| ((t - l) as f32).clamp(0.0, 0.05));
        self.last = Some(t);
        for s in [&mut self.w, &mut self.h, &mut self.r] {
            s.step(ISLAND_SPRING, dt);
        }
        if self.old.is_some() && (t - self.swap_at) as f32 >= CROSS_FADE {
            self.old = None;
        }
    }

    pub fn size(&self) -> Size {
        Size { w: self.w.value, h: self.h.value }
    }

    /// Opacity of the current face (fading in) and of the old one (fading out).
    pub fn face_opacity(&self, t: f64) -> (f32, f32) {
        let k = ease(((t - self.swap_at) as f32 / CROSS_FADE).clamp(0.0, 1.0));
        (k, 1.0 - k)
    }

    pub fn glow(&self, t: f64) -> (f32, Color) {
        (self.glow.value_at(t), self.glow_color.value_at(t))
    }

    pub fn wave_color(&self, t: f64) -> Color {
        self.wave_color.value_at(t)
    }

    /// Whether anything is still in motion at `t`.
    pub fn busy(&self, t: f64) -> bool {
        !(self.w.is_settled() && self.h.is_settled() && self.r.is_settled())
            || self.w.value != self.w.target
            || self.h.value != self.h.target
            || self.r.value != self.r.target
            || self.old.is_some()
            || ((t - self.swap_at) as f32) < CROSS_FADE
            || !self.glow.done_at(t)
            || !self.glow_color.done_at(t)
            || !self.wave_color.done_at(t)
            || !self.hover.done_at(t)
    }

    /// Forgets the last frame time, so the next step doesn't jump after an idle stretch.
    pub fn pause(&mut self) {
        self.last = None;
    }
}
