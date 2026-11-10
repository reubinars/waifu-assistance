//! The collapsed island's content (spec §6.2), laid out against the island's current rect.

use crate::animator::Face;
use crate::draw::{Align, Color, Painter, TEXT_PRIMARY, TEXT_SECONDARY};
use island_core::Rect;
use island_core::anim::{BLOCKED_PULSE_PERIOD, SOUND_BAR_BASE, pulse, sound_bar_scale};
use island_core::view::centred_status;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::core::Result;

const PAD: f32 = 10.0;
/// pill-info margin-left.
const GAP: f32 = 8.0;
const IDLE_DOT: Color = Color::white(0.35);
const DONE: Color = Color::rgba(0x34, 0xc7, 0x59, 1.0);
const BLOCKED: Color = Color::rgba(0xff, 0x95, 0x00, 1.0);
const ART_D: f32 = 25.0;
const BAR_W: f32 = 3.5;
const BAR_GAP: f32 = 2.5;
const BAR_H: f32 = 20.0;
const SPINNER_D: f32 = 14.0;
/// One spinner step (of 8) per tick.
pub const SPINNER_TICK_MS: u32 = 100;

pub struct Scene<'a> {
    /// Seconds on the app clock.
    pub t: f64,
    pub art: Option<&'a ID2D1Bitmap1>,
    pub playing: bool,
    pub wave: Color,
    /// Hover on the media pill, 0..=1: the wave gives way to title and artist.
    pub hover: f32,
    pub title: &'a str,
    pub artist: &'a str,
}

/// The hover slide distance.
const SLIDE: f32 = 8.0;

/// Draws `face` inside `r` at opacity `k` (the caller clips to the island shape).
pub fn draw(p: &mut Painter, face: &Face, r: Rect, k: f32, s: &Scene) -> Result<()> {
    let cy = r.y + r.h / 2.0;
    match face {
        Face::Idle => {
            let scale = 0.8 + 0.2 * k;
            p.circle(r.x + r.w / 2.0, cy, 9.0 * scale, IDLE_DOT.fade(k));
        }
        Face::Media => {
            let (ax, ay) = (r.x + PAD, cy - ART_D / 2.0);
            let art = Rect { x: ax, y: ay, w: ART_D, h: ART_D };
            match s.art {
                Some(bmp) => p.image(bmp, art, ART_D / 2.0, k)?,
                None => p.circle(ax + ART_D / 2.0, cy, ART_D, Color::white(0.08).fade(k)),
            }
            p.circle_outline(ax + ART_D / 2.0, cy, ART_D - 1.0, Color::white(0.06).fade(k), 1.0);
            let wave_w = 5.0 * BAR_W + 4.0 * BAR_GAP;
            let wx = r.x + r.w - PAD - wave_w;
            let h = s.hover;
            // the wave slides out to x−8 as the text slides in from x+8
            let kw = k * (1.0 - h);
            if kw > 0.001 {
                let restore = p.translate(-SLIDE * h, 0.0);
                if s.playing {
                    for (i, base) in SOUND_BAR_BASE.iter().enumerate() {
                        let bh = BAR_H * base * sound_bar_scale(i, s.t);
                        let x = wx + i as f32 * (BAR_W + BAR_GAP);
                        let bar = Rect { x, y: cy + BAR_H / 2.0 - bh, w: BAR_W, h: bh };
                        p.round_rect(bar, 1.5, s.wave.fade(kw));
                    }
                } else {
                    p.circle(wx + wave_w / 2.0, cy, 9.0, IDLE_DOT.fade(0.6 * kw));
                }
                p.set_transform(restore);
            }
            let kt = k * h;
            if kt > 0.001 {
                let restore = p.translate(SLIDE * (1.0 - h), 0.0);
                let x = ax + ART_D + GAP;
                let w = (r.x + r.w - PAD - x).max(0.0);
                p.text(s.title, Rect { x, y: cy - 13.0, w, h: 14.0 }, 11.0, 600, TEXT_PRIMARY.fade(kt), Align::Left)?;
                p.text(s.artist, Rect { x, y: cy + 1.0, w, h: 12.0 }, 9.0, 400, TEXT_SECONDARY.fade(kt), Align::Left)?;
                p.set_transform(restore);
            }
        }
        Face::Working { count } => {
            status(p, &format!("{count} working"), r, SPINNER_D, k, |p, x| {
                spinner(p, x + SPINNER_D / 2.0, cy, s.t, k);
                Ok(())
            })?;
        }
        Face::Done { label: text } => {
            status(p, text, r, 6.0, k, |p, x| {
                let dot = p.shape(Rect { x, y: cy - 3.0, w: 6.0, h: 6.0 }, [3.0; 4])?;
                p.shadow(&dot, 0.0, 0.0, 6.0, DONE.fade(k))?;
                p.fill(&dot, DONE.fade(k));
                Ok(())
            })?;
        }
        Face::Blocked { label: text } => {
            let (o, scale) = pulse(s.t, BLOCKED_PULSE_PERIOD);
            status(p, text, r, 6.0, k, |p, x| {
                p.circle(x + 3.0, cy, 6.0 * scale, BLOCKED.fade(o * k));
                Ok(())
            })?;
        }
        Face::Expanded(_) => {}
    }
    Ok(())
}

/// An icon `icon_w` wide and an 11/600 label, centred together in `r`; `icon` draws the icon from its left x.
fn status(
    p: &mut Painter,
    text: &str,
    r: Rect,
    icon_w: f32,
    k: f32,
    icon: impl FnOnce(&mut Painter, f32) -> Result<()>,
) -> Result<()> {
    let room = (r.w - 2.0 * PAD - icon_w - GAP).max(0.0);
    let l = p.layout(text, Rect { x: 0.0, y: 0.0, w: room, h: r.h }, 11.0, 600, Align::Left)?;
    let (icon_x, label_x, _) = centred_status(r, PAD, icon_w, GAP, Painter::width(&l));
    icon(p, icon_x)?;
    p.draw_layout(&l, label_x, r.y, TEXT_PRIMARY.fade(k));
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
