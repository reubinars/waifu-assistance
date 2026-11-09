//! The expanded island (spec §6.3): agents and media tabs, with hover and press feedback.
//! Everything is laid out from `island_core::view::layout`, relative to the expanded island's
//! top-left.

use crate::draw::{Align, Color, Painter, TEXT_PRIMARY, TEXT_SECONDARY, TEXT_TERTIARY};
use island_core::Rect;
use island_core::anim::{Fade, breathe};
use island_core::herdr::AgentStatus;
use island_core::view::layout::{self, MAX_ROWS};
use island_core::view::{ExpandedHit, Tab};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::core::Result;

const DONE: Color = Color::rgba(0x34, 0xc7, 0x59, 1.0);
const BLOCKED: Color = Color::rgba(0xff, 0x95, 0x00, 1.0);
const TAB_FADE: f32 = 0.2;
const BUTTON_FADE: f32 = 0.12;

/// Something under the cursor that reacts to hover or press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hot {
    Tab(Tab),
    Row(usize),
    Prev,
    Play,
    Next,
    Seek,
}

const HOT_COUNT: usize = 6 + MAX_ROWS;

impl Hot {
    pub fn of(hit: ExpandedHit) -> Hot {
        match hit {
            ExpandedHit::Tab(t) => Hot::Tab(t),
            ExpandedHit::Row(i) => Hot::Row(i),
            ExpandedHit::Prev => Hot::Prev,
            ExpandedHit::PlayPause => Hot::Play,
            ExpandedHit::Next => Hot::Next,
            ExpandedHit::Seek(_) => Hot::Seek,
        }
    }

    fn index(self) -> usize {
        match self {
            Hot::Tab(Tab::Agents) => 0,
            Hot::Tab(Tab::Media) => 1,
            Hot::Prev => 2,
            Hot::Play => 3,
            Hot::Next => 4,
            Hot::Seek => 5,
            Hot::Row(i) => 6 + i.min(MAX_ROWS - 1),
        }
    }
}

const STILL: Fade = Fade { from: 0.0, to: 0.0, start: 0.0, duration: 0.0 };

fn fade_to(f: &mut Fade, to: f32, t: f32, duration: f32) {
    if f.to != to {
        *f = Fade { from: f.value_at(t), to, start: t, duration };
    }
}

/// Hover and press amounts (0..=1) for every `Hot`, each easing over its own duration.
pub struct Hover {
    pub hot: Option<Hot>,
    pub pressed: Option<Hot>,
    hover: [Fade; HOT_COUNT],
    press: [Fade; HOT_COUNT],
}

impl Default for Hover {
    fn default() -> Hover {
        Hover { hot: None, pressed: None, hover: [STILL; HOT_COUNT], press: [STILL; HOT_COUNT] }
    }
}

impl Hover {
    pub fn set_hot(&mut self, hot: Option<Hot>, t: f32) {
        self.hot = hot;
        for (i, f) in self.hover.iter_mut().enumerate() {
            let on = hot.is_some_and(|h| h.index() == i);
            // indices 0 and 1 are the tabs
            fade_to(f, if on { 1.0 } else { 0.0 }, t, if i < 2 { TAB_FADE } else { BUTTON_FADE });
        }
    }

    pub fn set_pressed(&mut self, pressed: Option<Hot>, t: f32) {
        self.pressed = pressed;
        for (i, f) in self.press.iter_mut().enumerate() {
            let on = pressed.is_some_and(|h| h.index() == i);
            fade_to(f, if on { 1.0 } else { 0.0 }, t, BUTTON_FADE);
        }
    }

    /// Clears hover and press without animating (the view is going away).
    pub fn reset(&mut self) {
        *self = Hover::default();
    }

    fn hover(&self, h: Hot, t: f32) -> f32 {
        self.hover[h.index()].value_at(t)
    }

    fn press(&self, h: Hot, t: f32) -> f32 {
        self.press[h.index()].value_at(t)
    }

    pub fn busy(&self, t: f32) -> bool {
        self.hover.iter().chain(&self.press).any(|f| !f.done_at(t))
    }
}

pub struct Row {
    pub status: AgentStatus,
    pub name: String,
    pub title: String,
    pub folder: String,
    pub since: String,
}

pub struct Media {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub seekable: bool,
    /// 0..=1, or the drag position while the seekbar is held.
    pub progress: f32,
    pub position: String,
    pub duration: String,
}

pub struct Scene<'a> {
    pub t: f32,
    pub tabs: bool,
    pub rows: Vec<Row>,
    pub media: Option<Media>,
    pub art: Option<&'a ID2D1Bitmap1>,
    pub accent: Option<Color>,
    pub hover: &'a Hover,
}

fn off(r: Rect, ox: f32, oy: f32) -> Rect {
    Rect { x: r.x + ox, y: r.y + oy, ..r }
}

/// Draws `tab` with the expanded island's top-left at (ox, oy), at opacity `k`.
pub fn draw(p: &mut Painter, tab: Tab, ox: f32, oy: f32, k: f32, s: &Scene) -> Result<()> {
    if s.tabs {
        for (t, r, label) in [(Tab::Agents, layout::TAB_AGENTS, "Agents"), (Tab::Media, layout::TAB_MEDIA, "Media")] {
            let r = off(r, ox, oy);
            let h = s.hover.hover(Hot::Tab(t), s.t);
            let restore = p.scale_about(1.0 + 0.1 * h, r.x + r.w / 2.0, r.y + r.h / 2.0);
            let active = t == tab;
            p.round_rect(r, r.h / 2.0, Color::white(if active { 0.14 } else { 0.06 }).fade(k));
            let c = if active { TEXT_PRIMARY } else { TEXT_SECONDARY };
            p.text(label, r, 11.0, 600, c.fade(k), Align::Center)?;
            p.set_transform(restore);
        }
    }
    match tab {
        Tab::Agents => draw_rows(p, ox, oy, k, s),
        Tab::Media => draw_media(p, ox, oy, k, s),
    }
}

fn status_color(s: AgentStatus) -> Color {
    match s {
        AgentStatus::Blocked => BLOCKED,
        AgentStatus::Done => DONE,
        AgentStatus::Working => TEXT_PRIMARY,
        AgentStatus::Idle | AgentStatus::Unknown => TEXT_TERTIARY,
    }
}

/// One line per agent: dot, name, title (fills), folder, time in state.
fn draw_rows(p: &mut Painter, ox: f32, oy: f32, k: f32, s: &Scene) -> Result<()> {
    const DOT_X: f32 = 14.0;
    const NAME_X: f32 = 26.0;
    const NAME_MAX: f32 = 110.0;
    const FOLDER_MAX: f32 = 90.0;
    const TIME_W: f32 = 26.0;
    const SPACE: f32 = 8.0;
    const RIGHT: f32 = 10.0;
    for (i, row) in s.rows.iter().take(MAX_ROWS).enumerate() {
        let r = off(layout::row(i), ox, oy);
        let h = s.hover.hover(Hot::Row(i), s.t);
        if h > 0.0 {
            p.round_rect(r, layout::ROW_RADIUS, Color::white(0.10 * h).fade(k));
        }
        let cy = r.y + r.h / 2.0;
        p.circle(r.x + DOT_X, cy, 6.0, status_color(row.status).fade(k));
        let line = |x: f32, w: f32| Rect { x, y: r.y, w: w.max(0.0), h: r.h };

        let time_x = r.x + r.w - RIGHT - TIME_W;
        let time = p.layout_styled(&row.since, line(time_x, TIME_W), 10.0, 500, Align::Right, true, 0.0)?;
        p.draw_layout(&time, time_x, r.y, TEXT_TERTIARY.fade(k));

        let name_x = r.x + NAME_X;
        let name = p.layout(&row.name, line(name_x, NAME_MAX), 12.0, 600, Align::Left)?;
        let name_w = Painter::width(&name);
        p.draw_layout(&name, name_x, r.y, TEXT_PRIMARY.fade(k));

        let folder_end = time_x - SPACE;
        let folder_w = if row.folder.is_empty() {
            0.0
        } else {
            let probe = p.layout(&row.folder, line(0.0, FOLDER_MAX), 11.0, 400, Align::Left)?;
            Painter::width(&probe).min(FOLDER_MAX)
        };
        if folder_w > 0.0 {
            p.text(&row.folder, line(folder_end - folder_w, folder_w + 1.0), 11.0, 400, TEXT_TERTIARY.fade(k), Align::Left)?;
        }

        let title_x = name_x + name_w + SPACE;
        let title_end = if folder_w > 0.0 { folder_end - folder_w - SPACE } else { folder_end };
        if title_end - title_x > 12.0 {
            p.text(&row.title, line(title_x, title_end - title_x), 11.0, 400, TEXT_SECONDARY.fade(k), Align::Left)?;
        }
    }
    Ok(())
}

fn draw_media(p: &mut Painter, ox: f32, oy: f32, k: f32, s: &Scene) -> Result<()> {
    let art = off(layout::ART, ox, oy);
    let (acx, acy) = (art.x + art.w / 2.0, art.y + art.h / 2.0);
    let playing = s.media.as_ref().is_some_and(|m| m.playing);
    if let (Some(accent), Some(_)) = (s.accent, &s.media) {
        // paused: the glow rests at its low point
        let (o, scale) = if playing { breathe(s.t) } else { (0.3, 1.0) };
        let restore = p.scale_about(scale, acx, acy);
        let glow = p.shape(Rect { x: art.x - 6.0, y: art.y - 6.0, w: art.w + 12.0, h: art.h + 12.0 }, [16.0; 4])?;
        p.shadow(&glow, 0.0, 0.0, 16.0, accent.with_alpha(0.25 * o * k))?;
        p.set_transform(restore);
    }
    match (s.art, &s.media) {
        (Some(bmp), Some(_)) => p.image(bmp, art, 10.0, k)?,
        _ => p.round_rect(art, 10.0, Color::white(0.08).fade(k)),
    }

    let Some(m) = &s.media else {
        p.text("Not playing", off(layout::TITLE, ox, oy), 14.0, 600, TEXT_SECONDARY.fade(k), Align::Left)?;
        return draw_buttons(p, ox, oy, 0.35 * k, false, s);
    };
    let title = off(layout::TITLE, ox, oy);
    let l = p.layout_styled(&m.title, title, 14.0, 600, Align::Left, false, -0.02)?;
    p.draw_layout(&l, title.x, title.y, TEXT_PRIMARY.fade(k));
    p.text(&m.artist, off(layout::ARTIST, ox, oy), 12.0, 400, TEXT_SECONDARY.fade(k), Align::Left)?;

    if m.seekable {
        let h = s.hover.hover(Hot::Seek, s.t).max(s.hover.press(Hot::Seek, s.t));
        let seek = off(layout::SEEK, ox, oy);
        let cy = seek.y + seek.h / 2.0;
        let th = 4.0 + 2.0 * h;
        let track = Rect { x: seek.x, y: cy - th / 2.0, w: seek.w, h: th };
        p.round_rect(track, 2.0, Color::white(0.15).fade(k));
        let fill_w = seek.w * m.progress.clamp(0.0, 1.0);
        if fill_w > 0.5 {
            p.round_rect(Rect { w: fill_w, ..track }, 2.0, s.accent.unwrap_or(Color::white(1.0)).fade(k));
        }
        let d = 10.0 + 2.0 * h;
        let tx = seek.x + fill_w;
        let thumb = p.shape(Rect { x: tx - d / 2.0, y: cy - d / 2.0, w: d, h: d }, [d / 2.0; 4])?;
        p.shadow(&thumb, 0.0, 0.0, 4.0, Color::BLACK.with_alpha(0.5 * k))?;
        p.fill(&thumb, Color::white(k));

        let times = off(layout::TIMES, ox, oy);
        let a = p.layout_styled(&m.position, times, 10.0, 500, Align::Left, true, 0.0)?;
        p.draw_layout(&a, times.x, times.y, TEXT_SECONDARY.fade(k));
        let b = p.layout_styled(&m.duration, times, 10.0, 500, Align::Right, true, 0.0)?;
        p.draw_layout(&b, times.x, times.y, TEXT_SECONDARY.fade(k));
    }
    draw_buttons(p, ox, oy, k, m.playing, s)
}

fn draw_buttons(p: &mut Painter, ox: f32, oy: f32, k: f32, playing: bool, s: &Scene) -> Result<()> {
    let ink = TEXT_PRIMARY.fade(k);
    for (hot, (cx, cy, r)) in [(Hot::Prev, layout::PREV), (Hot::Play, layout::PLAY), (Hot::Next, layout::NEXT)] {
        let (cx, cy) = (cx + ox, cy + oy);
        let h = s.hover.hover(hot, s.t);
        let pr = s.hover.press(hot, s.t);
        let restore = p.scale_about(1.0 + 0.08 * h - 0.14 * pr, cx, cy);
        let bg = 0.10 * h + 0.08 * pr;
        if bg > 0.0 {
            p.circle(cx, cy, 2.0 * r, Color::white(bg).fade(k));
        }
        match hot {
            Hot::Play if playing => {
                for dx in [-4.5, 1.5] {
                    p.round_rect(Rect { x: cx + dx, y: cy - 8.0, w: 3.5, h: 16.0 }, 1.0, ink);
                }
            }
            Hot::Play => p.polygon(&[(cx - 5.0, cy - 9.0), (cx + 9.0, cy), (cx - 5.0, cy + 9.0)], ink)?,
            _ => {
                // skip: a bar and a triangle, mirrored for previous
                let m = if hot == Hot::Prev { -1.0 } else { 1.0 };
                p.polygon(&[(cx - 6.0 * m, cy - 7.0), (cx + 4.0 * m, cy), (cx - 6.0 * m, cy + 7.0)], ink)?;
                let bx = if m > 0.0 { cx + 4.0 } else { cx - 6.5 };
                p.round_rect(Rect { x: bx, y: cy - 7.0, w: 2.5, h: 14.0 }, 1.0, ink);
            }
        }
        p.set_transform(restore);
    }
    Ok(())
}
