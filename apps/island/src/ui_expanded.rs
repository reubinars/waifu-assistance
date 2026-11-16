//! The expanded island (spec §6.3): agents and media tabs, with hover and press feedback.
//! Everything is laid out from `island_core::view::layout`, relative to the expanded island's
//! top-left.

use crate::draw::{Align, Color, Painter, TEXT_PRIMARY, TEXT_SECONDARY, TEXT_TERTIARY};
use island_core::Rect;
use island_core::anim::{Fade, breathe};
use island_core::approvals::Detail;
use island_core::github::{Ci, GhEvent, GhEventKind, GhRow, text::{chip, ci_glyph, head, pop_text}};
use island_core::herdr::AgentStatus;
use island_core::ticker;
use island_core::usage::{Level, Run, Tone, level};
use island_core::view::layout::{self, MAX_ROWS};
use island_core::view::{Action, CardKind, EXPANDED, ExpandedHit, POP, PopupLayout, Tab, max_scroll, status_word};
use std::cell::Cell;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::Win32::Graphics::DirectWrite::{DWRITE_TEXT_METRICS, IDWriteTextLayout};
use windows::core::Result;

const DONE: Color = Color::rgba(0x34, 0xc7, 0x59, 1.0);
const BLOCKED: Color = Color::rgba(0xff, 0x95, 0x00, 1.0);
/// A removed-lines count in the popup's detail box.
const MINUS: Color = Color::rgba(0xff, 0x69, 0x61, 1.0);
const HIGH: Color = Color::rgba(0xff, 0x3b, 0x30, 1.0);
const TAB_FADE: f32 = 0.2;
const BUTTON_FADE: f32 = 0.12;

/// Something under the cursor that reacts to hover or press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hot {
    Tab(Tab),
    Row(usize),
    /// A GitHub tab row, by list index.
    Gh(usize),
    /// A card's button, by card index.
    Button(usize, Action),
    Prev,
    Play,
    Next,
    Seek,
    /// The usage strip: pauses the ticker while hovered; no hover visuals.
    Usage,
}

const ACTIONS: usize = 5;
const BUTTONS_AT: usize = 10 + 2 * MAX_ROWS;
const HOT_COUNT: usize = BUTTONS_AT + (MAX_ROWS + 1) * ACTIONS;

impl Hot {
    pub fn of(hit: ExpandedHit) -> Hot {
        match hit {
            ExpandedHit::Tab(t) => Hot::Tab(t),
            ExpandedHit::Row(i) => Hot::Row(i),
            ExpandedHit::Prev => Hot::Prev,
            ExpandedHit::PlayPause => Hot::Play,
            ExpandedHit::Next => Hot::Next,
            ExpandedHit::Seek(_) => Hot::Seek,
            ExpandedHit::Usage => Hot::Usage,
            ExpandedHit::GhRow(i) => Hot::Gh(i),
            ExpandedHit::Button(i, a) => Hot::Button(i, a),
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
            Hot::Row(i) => 6 + i % (MAX_ROWS + 1),
            Hot::Usage => 7 + MAX_ROWS,
            Hot::Tab(Tab::GitHub) => 8 + MAX_ROWS,
            Hot::Gh(i) => 9 + MAX_ROWS + i % (MAX_ROWS + 1),
            Hot::Button(i, a) => BUTTONS_AT + i % (MAX_ROWS + 1) * ACTIONS + a as usize,
        }
    }
}

const STILL: Fade = Fade { from: 0.0, to: 0.0, start: 0.0, duration: 0.0 };

fn fade_to(f: &mut Fade, to: f32, t: f64, duration: f32) {
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
    pub fn set_hot(&mut self, hot: Option<Hot>, t: f64) {
        self.hot = hot;
        for (i, f) in self.hover.iter_mut().enumerate() {
            let on = hot.is_some_and(|h| h.index() == i);
            // indices 0 and 1 are the tabs
            fade_to(f, if on { 1.0 } else { 0.0 }, t, if i < 2 { TAB_FADE } else { BUTTON_FADE });
        }
    }

    pub fn set_pressed(&mut self, pressed: Option<Hot>, t: f64) {
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

    fn hover(&self, h: Hot, t: f64) -> f32 {
        self.hover[h.index()].value_at(t)
    }

    fn press(&self, h: Hot, t: f64) -> f32 {
        self.press[h.index()].value_at(t)
    }

    pub fn busy(&self, t: f64) -> bool {
        self.hover.iter().chain(&self.press).any(|f| !f.done_at(t))
    }
}

pub struct Card {
    pub status: AgentStatus,
    pub name: String,
    pub folder: String,
    /// The agent's title, or None when there is nothing to show beyond the name.
    pub subtitle: Option<String>,
    pub kind: CardKind,
    /// A request card's text: the approval's summary, the question, or the missing-hook hint.
    pub summary: Option<String>,
    /// An approval's compact detail: the first file or command line.
    pub line: Option<String>,
}

/// The one-request popup (spec 6.3), drawn in place of the tabs.
pub struct Popup {
    pub name: String,
    pub folder: String,
    /// Pending approvals; the popup shows the first ("1 of N").
    pub count: usize,
    pub summary: String,
    pub detail: Option<Detail>,
    pub layout: PopupLayout,
    pub kind: CardKind,
    /// Countdown bar fill, 0..=1.
    pub fraction: f32,
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
    pub t: f64,
    pub media_present: bool,
    pub github_present: bool,
    pub github: Vec<GhRow>,
    pub cards: Vec<Card>,
    pub scroll: f32,
    pub media: Option<Media>,
    /// Replaces the tabs while set.
    pub popup: Option<Popup>,
    pub art: Option<&'a ID2D1Bitmap1>,
    pub accent: Option<Color>,
    pub hover: &'a Hover,
    /// The usage strip's text; empty hides the strip.
    pub usage: Vec<Run>,
    /// How far the strip's text has scrolled left (px).
    pub ticker_offset: f32,
    /// The strip's measured text width, written while drawing (0 when the strip isn't drawn).
    pub usage_width: Cell<f32>,
}

fn off(r: Rect, ox: f32, oy: f32) -> Rect {
    Rect { x: r.x + ox, y: r.y + oy, ..r }
}

/// Draws `tab` with the expanded island's top-left at (ox, oy), at opacity `k`.
pub fn draw(p: &mut Painter, tab: Tab, ox: f32, oy: f32, k: f32, s: &Scene) -> Result<()> {
    if let Some(pop) = &s.popup {
        return draw_popup(p, ox, oy, k, s, pop);
    }
    for (t, r) in layout::tabs(s.media_present, s.github_present) {
        let label = match t { Tab::Agents => "Agents", Tab::Media => "Media", Tab::GitHub => "GitHub" };
        let r = off(r, ox, oy);
        let h = s.hover.hover(Hot::Tab(t), s.t);
        let restore = p.scale_about(1.0 + 0.1 * h, r.x + r.w / 2.0, r.y + r.h / 2.0);
        let active = t == tab;
        p.round_rect(r, r.h / 2.0, Color::white(if active { 0.14 } else { 0.06 }).fade(k));
        let c = if active { TEXT_PRIMARY } else { TEXT_SECONDARY };
        p.text(label, r, 11.0, 600, c.fade(k), Align::Center)?;
        p.set_transform(restore);
    }
    match tab {
        Tab::Agents => {
            draw_rows(p, ox, oy, k, s)?;
            if s.usage.is_empty() { Ok(()) } else { draw_usage(p, ox, oy, k, s) }
        }
        Tab::Media => draw_media(p, ox, oy, k, s),
        Tab::GitHub => draw_github(p, ox, oy, k, s),
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

const ORANGE: Color = BLOCKED;

fn button_label(kind: &CardKind, a: Action) -> &'static str {
    match (kind, a) {
        (CardKind::Question, Action::OpenTerminal) => "Reply in terminal",
        (_, Action::OpenTerminal) => "Open terminal",
        (_, Action::Allow) => "Allow",
        (_, Action::Deny) => "Deny",
        (_, Action::Always) => "Always allow",
        (_, Action::Later) => "Later",
    }
}

fn header_label(kind: &CardKind) -> &'static str {
    match kind {
        CardKind::Approval { .. } => "approval",
        CardKind::Question => "asks",
        CardKind::NeedsYou { .. } => "needs you",
        CardKind::Agent => "",
    }
}

const BUTTON_R: f32 = 14.0;

/// A card or popup button; `(hv, pr)` are its hover and press amounts.
fn draw_button(p: &mut Painter, kind: &CardKind, a: Action, b: Rect, (hv, pr): (f32, f32), k: f32) -> Result<()> {
    let primary = a == Action::Allow || (a == Action::OpenTerminal && *kind == CardKind::Question);
    let restore = p.scale_about(1.0 - 0.04 * pr, b.x + b.w / 2.0, b.y + b.h / 2.0);
    let (bg, ink) = if primary {
        (TEXT_PRIMARY.lerp(Color::white(1.0), hv), Color::BLACK)
    } else {
        (Color::white(0.12 + 0.10 * hv + 0.08 * pr), TEXT_PRIMARY)
    };
    p.round_rect(b, BUTTON_R, bg.fade(k));
    p.text(button_label(kind, a), b, 11.0, if primary { 700 } else { 600 }, ink.fade(k), Align::Center)?;
    p.set_transform(restore);
    Ok(())
}

/// The popup: header (agent, folder, "1 of N"), summary, detail box, buttons and the countdown bar.
fn draw_popup(p: &mut Painter, ox: f32, oy: f32, k: f32, s: &Scene, pop: &Popup) -> Result<()> {
    const X: f32 = 36.0;
    const RIGHT: f32 = 340.0;
    const GAP: f32 = 8.0;
    let l = &pop.layout;
    let head_y = oy + 14.0;
    p.circle(ox + 22.0, head_y + 9.0, 6.0, ORANGE.fade(k));
    let head = |x: f32, w: f32| Rect { x, y: head_y, w: w.max(0.0), h: 18.0 };
    let count = format!("{} of {}", 1, pop.count);
    let counter = p.layout(&count, head(ox + RIGHT - 60.0, 60.0), 11.0, 600, Align::Right)?;
    let counter_left = ox + RIGHT - Painter::width(&counter);
    p.draw_layout(&counter, ox + RIGHT - 60.0, head_y, TEXT_SECONDARY.fade(k));
    let name_x = ox + X;
    let name = p.layout(&pop.name, head(name_x, 110.0), 12.0, 600, Align::Left)?;
    p.draw_layout(&name, name_x, head_y, TEXT_PRIMARY.fade(k));
    let folder_x = name_x + Painter::width(&name) + GAP;
    let folder_w = counter_left - GAP - folder_x;
    if !pop.folder.is_empty() && folder_w >= 12.0 {
        p.text(&pop.folder, head(folder_x, folder_w), 11.0, 400, TEXT_TERTIARY.fade(k), Align::Left)?;
    }
    p.text(&pop.summary, Rect { x: ox + X, y: oy + 41.0, w: RIGHT - X, h: 18.0 }, 13.0, 600, TEXT_PRIMARY.fade(k), Align::Left)?;

    if let (Some(box_), Some(detail)) = (l.detail, &pop.detail) {
        let b = off(box_, ox, oy);
        p.round_rect(b, 8.0, Color::white(0.07).fade(k));
        let (x, w) = (b.x + 9.0, b.w - 18.0);
        let line = |i: usize| Rect { x, y: b.y + 7.0 + 16.0 * i as f32, w, h: 16.0 };
        match detail {
            Detail::Files(files) => {
                for (i, f) in files.iter().take(3).enumerate() {
                    let r = line(i);
                    let minus = p.layout_mono(&format!("\u{2212}{}", f.removed), r, 11.0, 400, Align::Right)?;
                    let minus_w = Painter::width(&minus);
                    p.draw_layout(&minus, r.x, r.y, MINUS.fade(k));
                    let plus = p.layout_mono(&format!("+{}", f.added), Rect { w: r.w - minus_w - GAP, ..r }, 11.0, 400, Align::Right)?;
                    let plus_w = Painter::width(&plus);
                    p.draw_layout(&plus, r.x, r.y, DONE.fade(k));
                    let path_w = r.w - minus_w - plus_w - 2.0 * GAP;
                    p.layout_mono(&f.path, Rect { w: path_w.max(0.0), ..r }, 11.0, 400, Align::Left).map(|l| p.draw_layout(&l, r.x, r.y, TEXT_PRIMARY.fade(k)))?;
                }
                if files.len() > 3 {
                    let more = p.layout_mono(&format!("+{} more", files.len() - 3), line(3), 11.0, 400, Align::Left)?;
                    p.draw_layout(&more, x, line(3).y, TEXT_TERTIARY.fade(k));
                }
            }
            Detail::Command(c) => {
                for (i, text) in c.lines().take(2).enumerate() {
                    let r = line(i);
                    let t = p.layout_mono(text, r, 11.0, 400, Align::Left)?;
                    p.draw_layout(&t, r.x, r.y, TEXT_PRIMARY.fade(k));
                }
            }
        }
    }

    for (a, b) in layout::buttons(&pop.kind, off(l.band, ox, oy)) {
        let hot = Hot::Button(0, a);
        draw_button(p, &pop.kind, a, b, (s.hover.hover(hot, s.t), s.hover.press(hot, s.t)), k)?;
    }

    let bar = off(l.bar, ox, oy);
    p.round_rect(bar, 1.5, Color::white(0.10).fade(k));
    let fill = bar.w * pop.fraction.clamp(0.0, 1.0);
    if fill > 0.0 {
        p.round_rect(Rect { w: fill, ..bar }, 1.5_f32.min(fill / 2.0), BLOCKED.with_alpha(0.8).fade(k));
    }
    Ok(())
}

/// One card per agent or request. Agent: dot, name, folder and status word, then the title.
/// Request cards are tinted orange and carry buttons; approvals take two slots (header, summary,
/// detail, buttons), the one-slot kinds put name and text between their buttons.
fn draw_rows(p: &mut Painter, ox: f32, oy: f32, k: f32, s: &Scene) -> Result<()> {
    const DOT_X: f32 = 14.0;
    const NAME_X: f32 = 26.0;
    const NAME_MAX: f32 = 110.0;
    const FOLDER_MAX: f32 = 90.0;
    const STATUS_BOX: f32 = 80.0;
    const SPACE: f32 = 8.0;
    const RIGHT: f32 = 10.0;
    const MIN_FOLDER: f32 = 12.0;
    const LINE1_Y: f32 = 6.0;
    const LINE1_H: f32 = 18.0;
    const LINE2_Y: f32 = 22.0;
    const LINE2_H: f32 = 16.0;
    if s.cards.is_empty() {
        let row = off(layout::slot_rect(0), ox, oy);
        return p.text("No agents", Rect { x: row.x + NAME_X, w: row.w - NAME_X, ..row }, 14.0, 600, TEXT_SECONDARY.fade(k), Align::Left);
    }
    let list = off(layout::LIST, ox, oy);
    let mask = p.rect_geometry(list)?;
    p.push_layer(Some(&mask), 1.0);
    let drawn = (|| -> Result<()> {
        let mut top = 0;
        for (i, row) in s.cards.iter().enumerate() {
            let n = row.kind.slots();
            let base = layout::card_rect(0, n);
            let card = Rect { y: base.y + layout::ROW_STEP * (top as f32 - s.scroll), ..base };
            top += n;
            let r = off(card, ox, oy);
            if r.y + r.h <= list.y || r.y >= list.y + list.h {
                continue;
            }
            let request = row.kind != CardKind::Agent;
            if request {
                p.round_rect(r, layout::ROW_RADIUS, ORANGE.with_alpha(0.08).fade(k));
                p.round_rect_outline(r, layout::ROW_RADIUS, ORANGE.with_alpha(0.35).fade(k), 1.0);
            } else {
                p.round_rect(r, layout::ROW_RADIUS, Color::white(0.06).fade(k));
            }
            let h = s.hover.hover(Hot::Row(i), s.t);
            if h > 0.0 {
                p.round_rect(r, layout::ROW_RADIUS, Color::white(0.10 * h).fade(k));
            }
            let color = if request { ORANGE } else { status_color(row.status) };
            let buttons = layout::buttons(&row.kind, layout::card_band(r));
            let one_slot = request && n == 1;
            // the area for text on a one-slot request card: between its two button groups
            let (mid_l, mid_r) = if one_slot {
                let centre = r.x + r.w / 2.0;
                let l = buttons.iter().filter(|(_, b)| b.x + b.w / 2.0 < centre).map(|(_, b)| b.x + b.w).fold(r.x + NAME_X, f32::max) + SPACE;
                let rt = buttons.iter().filter(|(_, b)| b.x + b.w / 2.0 >= centre).map(|(_, b)| b.x).fold(r.x + r.w - RIGHT + SPACE, f32::min) - SPACE;
                (l, rt)
            } else {
                (r.x + NAME_X, r.x + r.w - RIGHT)
            };
            let (y1, h1) = match (&row.subtitle, request) {
                (_, true) => (r.y + LINE1_Y, LINE1_H),
                (Some(_), false) => (r.y + LINE1_Y, LINE1_H),
                (None, false) => (r.y + (r.h - LINE1_H) / 2.0, LINE1_H),
            };
            let band = |x: f32, w: f32| Rect { x, y: y1, w: w.max(0.0), h: h1 };
            p.circle(r.x + DOT_X, if one_slot { r.y + r.h / 2.0 } else { y1 + h1 / 2.0 }, 6.0, color.fade(k));

            let right = if one_slot { mid_r } else { r.x + r.w - RIGHT };
            let status_text = if request { header_label(&row.kind) } else { status_word(row.status) };
            let status_w = if one_slot && matches!(row.kind, CardKind::Question) { 36.0 } else { STATUS_BOX };
            let status = p.layout(status_text, band(right - status_w, status_w), 11.0, 600, Align::Right)?;
            let status_left = right - Painter::width(&status);
            p.draw_layout(&status, right - status_w, y1, color.fade(k));

            let name_x = if one_slot { mid_l } else { r.x + NAME_X };
            let name_max = if one_slot { (status_left - SPACE - name_x).clamp(0.0, NAME_MAX) } else { NAME_MAX };
            let name = p.layout(&row.name, band(name_x, name_max), 12.0, 600, Align::Left)?;
            let name_w = Painter::width(&name);
            p.draw_layout(&name, name_x, y1, TEXT_PRIMARY.fade(k));

            let folder_x = name_x + name_w + SPACE;
            let folder_w = (status_left - SPACE - folder_x).min(FOLDER_MAX);
            if !row.folder.is_empty() && folder_w >= MIN_FOLDER {
                p.text(&row.folder, band(folder_x, folder_w), 11.0, 400, TEXT_TERTIARY.fade(k), Align::Left)?;
            }

            if one_slot {
                if let Some(text) = &row.summary {
                    let hint = matches!(row.kind, CardKind::NeedsYou { .. });
                    let rect = Rect { x: name_x, y: r.y + LINE2_Y, w: (mid_r - name_x).max(0.0), h: LINE2_H };
                    let (size, c) = if hint { (10.0, TEXT_TERTIARY) } else { (11.0, TEXT_SECONDARY) };
                    p.text(text, rect, size, 400, c.fade(k), Align::Left)?;
                }
            } else if request {
                let x = r.x + NAME_X;
                let w = r.x + r.w - RIGHT - x;
                if let Some(text) = &row.summary {
                    p.text(text, Rect { x, y: r.y + 24.0, w, h: LINE2_H }, 11.0, 400, TEXT_SECONDARY.fade(k), Align::Left)?;
                }
                if let Some(line) = &row.line {
                    p.text(line, Rect { x, y: r.y + 40.0, w, h: LINE2_H }, 11.0, 400, TEXT_TERTIARY.fade(k), Align::Left)?;
                }
            } else if let Some(title) = &row.subtitle {
                let x = r.x + NAME_X;
                let rect = Rect { x, y: r.y + LINE2_Y, w: r.x + r.w - RIGHT - x, h: LINE2_H };
                p.text(title, rect, 11.0, 400, TEXT_SECONDARY.fade(k), Align::Left)?;
            }

            for (a, b) in buttons {
                draw_button(p, &row.kind, a, b, (s.hover.hover(Hot::Button(i, a), s.t), s.hover.press(Hot::Button(i, a), s.t)), k)?;
            }
        }
        Ok(())
    })();
    p.pop_layer();
    drawn?;
    let total: usize = s.cards.iter().map(|c| c.kind.slots()).sum();
    let max = max_scroll(total);
    if max > 0 {
        let h = list.h * MAX_ROWS as f32 / total as f32;
        let y = list.y + (list.h - h) * (s.scroll / max as f32).clamp(0.0, 1.0);
        p.round_rect(Rect { x: ox + EXPANDED.w - 5.0, y, w: 2.5, h }, 1.25, Color::white(0.25).fade(k));
    }
    Ok(())
}

fn ci_color(c: Ci) -> Color {
    match c {
        Ci::Passing => tone_color(Tone::Percent(Level::Low)),
        Ci::Failing => tone_color(Tone::Percent(Level::High)),
        Ci::Running => tone_color(Tone::Percent(Level::Mid)),
        Ci::None => TEXT_TERTIARY,
    }
}

/// The GitHub tab: a message row, or one card per PR in the agent rows' geometry, scrolled like them.
fn draw_github(p: &mut Painter, ox: f32, oy: f32, k: f32, s: &Scene) -> Result<()> {
    const GLYPH_X: f32 = 14.0;
    const TEXT_X: f32 = 30.0;
    const RIGHT: f32 = 10.0;
    const LINE1_Y: f32 = 6.0;
    const LINE1_H: f32 = 18.0;
    const LINE2_Y: f32 = 22.0;
    const LINE2_H: f32 = 16.0;
    const DIM: f32 = 0.55;
    let list = off(layout::LIST, ox, oy);
    let mask = p.rect_geometry(list)?;
    p.push_layer(Some(&mask), 1.0);
    let drawn = (|| -> Result<()> {
        for (i, row) in s.github.iter().enumerate() {
            let top = layout::slot_rect(0).y + layout::ROW_STEP * (i as f32 - s.scroll);
            let r = off(Rect { y: top, ..layout::slot_rect(0) }, ox, oy);
            if r.y + r.h <= list.y || r.y >= list.y + list.h {
                continue;
            }
            match row {
                GhRow::Message(text) => {
                    p.round_rect(r, layout::ROW_RADIUS, Color::white(0.06).fade(k));
                    let rect = Rect { x: r.x + GLYPH_X, w: r.w - GLYPH_X - RIGHT, ..r };
                    p.text(text, rect, 12.0, 400, TEXT_SECONDARY.fade(k), Align::Left)?;
                }
                GhRow::Pr { pr, dim } => {
                    let k = if *dim { k * DIM } else { k };
                    p.round_rect(r, layout::ROW_RADIUS, Color::white(0.06).fade(k));
                    let h = s.hover.hover(Hot::Gh(i), s.t);
                    if h > 0.0 {
                        p.round_rect(r, layout::ROW_RADIUS, Color::white(0.10 * h).fade(k));
                    }
                    let line1 = Rect { x: r.x + GLYPH_X, y: r.y + LINE1_Y, w: TEXT_X - GLYPH_X, h: LINE1_H };
                    p.text(ci_glyph(pr.ci), line1, 12.0, 600, ci_color(pr.ci).fade(k), Align::Left)?;
                    let w = r.x + r.w - RIGHT - (r.x + TEXT_X);
                    let title = Rect { x: r.x + TEXT_X, y: r.y + LINE1_Y, w, h: LINE1_H };
                    p.text(&head(pr), title, 12.0, 600, TEXT_PRIMARY.fade(k), Align::Left)?;
                    let chip = chip(pr);
                    if !chip.is_empty() {
                        let line2 = Rect { x: r.x + TEXT_X, y: r.y + LINE2_Y, w, h: LINE2_H };
                        p.text(&chip, line2, 11.0, 400, TEXT_TERTIARY.fade(k), Align::Left)?;
                    }
                }
            }
        }
        Ok(())
    })();
    p.pop_layer();
    drawn?;
    let total = s.github.len();
    let max = max_scroll(total);
    if max > 0 {
        let h = list.h * MAX_ROWS as f32 / total as f32;
        let y = list.y + (list.h - h) * (s.scroll / max as f32).clamp(0.0, 1.0);
        p.round_rect(Rect { x: ox + EXPANDED.w - 5.0, y, w: 2.5, h }, 1.25, Color::white(0.25).fade(k));
    }
    Ok(())
}

/// A GitHub event on the collapsed island: one line at the `POP` size with its top-left at
/// (ox, oy), the glyph in the GitHub tab's CI colours.
pub fn draw_pop(p: &mut Painter, ox: f32, oy: f32, k: f32, e: &GhEvent) -> Result<()> {
    const PAD_X: f32 = 14.0;
    const GLYPH_W: f32 = 22.0;
    let text = pop_text(e);
    let (glyph, rest) = text.split_once(' ').unwrap_or(("", &text));
    let color = match e.kind {
        GhEventKind::CiFailed => ci_color(Ci::Failing),
        GhEventKind::CiPassed | GhEventKind::Approved => ci_color(Ci::Passing),
        GhEventKind::ReviewRequested | GhEventKind::ChangesRequested => TEXT_PRIMARY,
    };
    let glyph_r = Rect { x: ox + PAD_X, y: oy, w: GLYPH_W, h: POP.h };
    p.text(glyph, glyph_r, 12.0, 600, color.fade(k), Align::Left)?;
    let x = glyph_r.x + GLYPH_W;
    let rest_r = Rect { x, y: oy, w: ox + POP.w - PAD_X - x, h: POP.h };
    p.text(rest, rest_r, 12.0, 600, TEXT_PRIMARY.fade(k), Align::Left)
}

fn tone_color(t: Tone) -> Color {
    match t {
        Tone::Name => TEXT_PRIMARY,
        Tone::Dim => TEXT_TERTIARY,
        Tone::Percent(Level::Low) => DONE,
        Tone::Percent(Level::Mid) => BLOCKED,
        Tone::Percent(Level::High) => HIGH,
        Tone::Bar(pct) => tone_color(Tone::Percent(level(pct as f32))),
    }
}

/// A strip run, laid out: text, or a bar filled to a fraction.
enum Piece {
    Text(IDWriteTextLayout),
    Bar(f32),
}

/// The laid-out text's advance, trailing spaces included (`Painter::width` drops them, and the
/// strip's runs end in the space before the next run).
fn advance(l: &IDWriteTextLayout) -> f32 {
    let mut m = DWRITE_TEXT_METRICS::default();
    unsafe {
        let _ = l.GetMetrics(&mut m);
    }
    m.widthIncludingTrailingWhitespace
}

/// The plan-limit strip under the cards: one line of runs, scrolled left by `ticker_offset` and
/// repeated after `ticker::GAP` when wider than the strip.
fn draw_usage(p: &mut Painter, ox: f32, oy: f32, k: f32, s: &Scene) -> Result<()> {
    const FADE_BANDS: [f32; 4] = [0.8, 0.6, 0.4, 0.2];
    const BAND_W: f32 = 3.0;
    let strip = off(layout::USAGE, ox, oy);
    const BAR_W: f32 = 26.0;
    const BAR_H: f32 = 4.0;
    const BAR_GAP: f32 = 5.0;
    let mut runs = Vec::with_capacity(s.usage.len());
    let mut total = 0.0;
    for run in &s.usage {
        let (piece, w, c) = match run.tone {
            Tone::Bar(pct) => (Piece::Bar(pct.min(100) as f32 / 100.0), BAR_W + BAR_GAP, tone_color(run.tone)),
            tone => {
                let weight = if tone == Tone::Name { 600 } else { 400 };
                let l = p.layout(&run.text, Rect { w: 4000.0, ..strip }, 11.0, weight, Align::Left)?;
                let w = advance(&l);
                (Piece::Text(l), w, tone_color(tone))
            }
        };
        runs.push((piece, w, c.fade(k)));
        total += w;
    }
    s.usage_width.set(total);
    let scrolls = total > strip.w;
    // the offset was wrapped to last frame's width; wrap it to this one's so a width change
    // can't draw a frame with a stale offset
    let offset = if scrolls { s.ticker_offset.rem_euclid(total + ticker::GAP) } else { 0.0 };
    let mask = p.rect_geometry(strip)?;
    p.push_layer(Some(&mask), 1.0);
    let starts: &[f32] = if scrolls { &[0.0, total + ticker::GAP] } else { &[0.0] };
    for start in starts {
        let mut x = strip.x - offset + start;
        for (piece, w, c) in &runs {
            if x < strip.x + strip.w && x + w > strip.x {
                match piece {
                    Piece::Text(l) => p.draw_layout(l, x, strip.y, *c),
                    Piece::Bar(fill) => {
                        let track = Rect { x, y: strip.y + (strip.h - BAR_H) / 2.0, w: BAR_W, h: BAR_H };
                        p.round_rect(track, BAR_H / 2.0, Color::white(0.14).fade(k));
                        if *fill > 0.0 {
                            // at least a dot, so a sliver of use still shows
                            let w = (BAR_W * fill).max(BAR_H);
                            p.round_rect(Rect { w, ..track }, BAR_H / 2.0, *c);
                        }
                    }
                }
            }
            x += w;
        }
    }
    if scrolls {
        // no gradient brush: stepped black bands fade the text out at both edges
        for (i, a) in FADE_BANDS.into_iter().enumerate() {
            let c = Color::BLACK.with_alpha(a).fade(k);
            let dx = BAND_W * i as f32;
            p.round_rect(Rect { x: strip.x + dx, w: BAND_W, ..strip }, 0.0, c);
            p.round_rect(Rect { x: strip.x + strip.w - dx - BAND_W, w: BAND_W, ..strip }, 0.0, c);
        }
    }
    p.pop_layer();
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
    let l = p.layout_tracked(&m.title, title, 14.0, 600, Align::Left, -0.02)?;
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
        let a = p.layout_tabular(&m.position, times, 10.0, 500, Align::Left)?;
        p.draw_layout(&a, times.x, times.y, TEXT_SECONDARY.fade(k));
        let b = p.layout_tabular(&m.duration, times, 10.0, 500, Align::Right)?;
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
