//! The collapsed island's status dot and media ring.

use crate::animator::Face;
use crate::draw::{Align, Color, Painter, TEXT_PRIMARY};
use island_core::Rect;
use island_core::github::GhEvent;
use island_core::anim::{BLOCKED_PULSE_PERIOD, pulse};
use island_core::view::Dot;
use windows::core::Result;

const IDLE_DOT: Color = Color::white(0.35);
const DONE: Color = Color::rgba(0x34, 0xc7, 0x59, 1.0);
const BLOCKED: Color = Color::rgba(0xff, 0x95, 0x00, 1.0);
const SPINNER_D: f32 = 14.0;
/// One spinner step (of 8) per tick.
pub const SPINNER_TICK_MS: u32 = 100;

pub struct Scene {
    /// Seconds on the app clock.
    pub t: f64,
    pub playing: bool,
    /// The album-art tint for the orbiting arc (white when there is none).
    pub accent: Color,
    /// Requests waiting on the user; above 0 a badge shows top-right.
    pub badge: u8,
    /// GitHub events not yet seen on the GitHub tab: a purple dot top-left.
    pub gh_dot: bool,
    /// The GitHub event the pop face shows (the last one, while it fades out).
    pub pop: Option<GhEvent>,
}

const BADGE_R: f32 = 9.0;
const GH_DOT_R: f32 = 6.0;
/// GitHub purple.
const GH_DOT: Color = Color::rgba(0x89, 0x57, 0xe5, 1.0);

/// Draws `face` inside `r` at opacity `k` (the caller clips to the island shape).
pub fn draw(p: &mut Painter, face: &Face, r: Rect, k: f32, s: &Scene) -> Result<()> {
    let Face::Dot { dot, media } = face else { return Ok(()) };
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    if *media {
        p.circle_outline(cx, cy, 28.0, Color::white(0.16).fade(k), 1.5);
        if s.playing {
            let angle = (s.t % 2.0) as f32 * std::f32::consts::PI;
            p.arc(cx, cy, 14.0, angle, std::f32::consts::FRAC_PI_2, s.accent.fade(0.9 * k), 1.5)?;
        }
    }
    match dot {
        Dot::Idle => p.circle(cx, cy, 9.0 * (0.8 + 0.2 * k), IDLE_DOT.fade(k)),
        Dot::Working => spinner(p, cx, cy, s.t, k),
        Dot::Blocked => {
            let (o, scale) = pulse(s.t, BLOCKED_PULSE_PERIOD);
            let opacity = 0.6 + (o - 0.25) / 0.35 * 0.4;
            p.circle(cx, cy, 10.0 * scale, BLOCKED.fade(opacity * k));
        }
        Dot::Done => {
            let dot = p.shape(Rect { x: cx - 5.0, y: cy - 5.0, w: 10.0, h: 10.0 }, [5.0; 4])?;
            p.shadow(&dot, 0.0, 0.0, 6.0, DONE.fade(k))?;
            p.fill(&dot, DONE.fade(k));
        }
    }
    if s.gh_dot {
        p.circle(r.x + GH_DOT_R, r.y + GH_DOT_R, 2.0 * GH_DOT_R, GH_DOT.fade(k));
    }
    if s.badge > 0 {
        let (bx, by) = (r.x + r.w - BADGE_R, r.y + BADGE_R);
        p.circle(bx, by, 2.0 * BADGE_R, BLOCKED.fade(k));
        let label = if s.badge > 9 { "9+".to_string() } else { s.badge.to_string() };
        let box_ = Rect { x: bx - BADGE_R, y: by - BADGE_R, w: 2.0 * BADGE_R, h: 2.0 * BADGE_R };
        p.text(&label, box_, 11.0, 700, Color::BLACK.fade(k), Align::Center)?;
    }
    Ok(())
}

/// iOS-style activity indicator: 8 spokes, the brightest one stepping round every tick.
fn spinner(p: &Painter, cx: f32, cy: f32, t: f64, k: f32) {
    let step = ((t * 1000.0) as u64 / SPINNER_TICK_MS as u64 % 8) as usize;
    let (r0, r1) = (SPINNER_D * 0.25, SPINNER_D / 2.0);
    for i in 0..8 {
        let a = i as f32 * std::f32::consts::TAU / 8.0 - std::f32::consts::FRAC_PI_2;
        let age = (step + 8 - i) % 8;
        let alpha = 1.0 - age as f32 * 0.11;
        let (sx, sy) = (a.cos(), a.sin());
        p.line(cx + sx * r0, cy + sy * r0, cx + sx * r1, cy + sy * r1, TEXT_PRIMARY.fade(alpha * k), 1.6);
    }
}
