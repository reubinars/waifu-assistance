//! What the island shows, where it sits, and what a click hit.

use crate::agents::AgentTable;
use crate::herdr::AgentStatus;
use crate::{Point, Rect, Size};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    #[default]
    Pill,
    Attached,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dot { Idle, Working, Done, Blocked }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Collapsed { pub dot: Dot, pub media: bool }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Agents,
    Media,
}

pub const EXPANDED: Size = Size { w: 360.0, h: 200.0 };
pub const PET: Size = Size { w: 64.0, h: 64.0 };
pub const SECOND_PET: Size = Size { w: 128.0, h: 64.0 };
/// Occupied part of the pet cell; its bottom four rows are transparent.
pub const PET_BOX: Size = Size { w: 64.0, h: 60.0 };
pub const PET_GAP: f32 = 4.0;
/// How far the second pet's cell advances left of the first: the cells overlap by 16 DIPs of
/// transparent margin, so the two stand close; they only touch when both sleep.
pub const PET_STEP: f32 = SECOND_PET.w - 16.0;
/// Room around the island inside its window for the shadow and glow to fade out in, so the
/// window's edge never cuts them off. Where the island sits at a screen edge, this room hangs
/// past that edge (off screen, or over the taskbar strip; the window is click-through).
const SHADOW_ROOM: f32 = 50.0;
/// The fixed transparent window (DIPs): the expanded island, two pet cells and shadow room.
pub const WINDOW: Size = Size {
    w: EXPANDED.w + PET_BOX.w + PET_GAP + PET_STEP + 2.0 * SHADOW_ROOM,
    h: EXPANDED.h + 2.0 * SHADOW_ROOM,
};
pub const COLLAPSED: Size = Size { w: 32.0, h: 32.0 };
/// How far the pill keeps off the edges of the screen (the work area).
const PILL_MARGIN: f32 = 4.0;

/// Priority: blocked > done > working > idle; media is independent.
pub fn choose_collapsed(agents: &AgentTable, media_present: bool) -> Collapsed {
    let dot = if agents.count(AgentStatus::Blocked) > 0 {
        Dot::Blocked
    } else if agents.count(AgentStatus::Done) > 0 {
        Dot::Done
    } else if agents.count(AgentStatus::Working) > 0 {
        Dot::Working
    } else {
        Dot::Idle
    };
    Collapsed { dot, media: media_present }
}

/// How much of the island is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Hidden,
    /// Over a fullscreen window: the collapsed blocked pill, which doesn't expand.
    CollapsedOnly,
    All,
}

/// §7: hidden from the tray, or over a fullscreen window unless an agent is blocked.
pub fn shown(user_hidden: bool, fullscreen: bool, c: &Collapsed) -> Shown {
    match (user_hidden, fullscreen, c.dot) {
        (true, _, _) => Shown::Hidden,
        (false, false, _) => Shown::All,
        (false, true, Dot::Blocked) => Shown::CollapsedOnly,
        (false, true, _) => Shown::Hidden,
    }
}

/// Where the island sits on the main monitor. Each axis runs from 0 (left / top edge) through 0.5
/// (centre) to 1 (right / bottom edge), and the island grows away from the edges in the same
/// proportion: at the top it grows down, at the bottom up, in the middle both ways.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub x: f32,
    pub y: f32,
}

impl Spot {
    pub const TOP_CENTRE: Spot = Spot { x: 0.5, y: 0.0 };

    /// Clamped to 0..=1; NaN centres `x` and puts `y` at the top. Attached is a notch, so it is
    /// always on the top edge.
    pub fn new(mode: DisplayMode, x: f32, y: f32) -> Spot {
        let clamp = |v: f32, nan: f32| if v.is_nan() { nan } else { v.clamp(0.0, 1.0) };
        let y = match mode {
            DisplayMode::Pill => clamp(y, 0.0),
            DisplayMode::Attached => 0.0,
        };
        Spot { x: clamp(x, 0.5), y }
    }
}

fn margin(mode: DisplayMode) -> f32 {
    match mode {
        DisplayMode::Pill => PILL_MARGIN,
        DisplayMode::Attached => 0.0,
    }
}

/// Width of the pet column left of the island for `pets` (0, 1 or 2) cells, gap included.
fn pets_w(pets: u8) -> f32 {
    if pets == 0 { 0.0 } else { PET_BOX.w + PET_GAP + PET_STEP * (pets - 1) as f32 }
}

/// The island and its pets (0, 1 or 2) move together as one block.
pub fn block_size(size: Size, pets: u8) -> Size {
    if pets > 0 { Size { w: size.w + pets_w(pets), h: size.h.max(PET_BOX.h) } } else { size }
}

/// The block's rect inside the window, in DIPs: anywhere inside the shadow room, by `spot`.
pub fn block_rect(spot: Spot, size: Size, pets: u8) -> Rect {
    let size = block_size(size, pets);
    let r = SHADOW_ROOM;
    let x = r + (WINDOW.w - 2.0 * r - size.w) * spot.x;
    let y = r + (WINDOW.h - 2.0 * r - size.h) * spot.y;
    Rect { x, y, w: size.w, h: size.h }
}

/// The pet cell's top relative to the island: centred near the top, feet aligned at the bottom.
fn pet_rel(mode: DisplayMode, spot_y: f32, h: f32) -> f32 {
    let centred = (h - PET_BOX.h) / 2.0;
    match mode {
        DisplayMode::Attached => 0.0,
        DisplayMode::Pill if spot_y <= 0.5 => centred,
        DisplayMode::Pill => centred + (h - PET_BOX.h - centred) * (2.0 * spot_y - 1.0),
    }
}

/// The island sits at the block's right edge, lowered when the pet extends above it.
pub fn island_rect(mode: DisplayMode, spot: Spot, size: Size, pets: u8) -> Rect {
    let b = block_rect(spot, size, pets);
    let dy = if pets > 0 { (-pet_rel(mode, spot.y, size.h)).max(0.0) } else { 0.0 };
    Rect { x: b.x + b.w - size.w, y: b.y + dy, w: size.w, h: size.h }
}

/// Top-left of the first pet's cell (next to the island), aligning its occupied height with the island.
pub fn pet_cell(mode: DisplayMode, spot: Spot, size: Size, pets: u8) -> Point {
    let r = island_rect(mode, spot, size, pets);
    Point { x: r.x - PET_GAP - PET.w, y: r.y + pet_rel(mode, spot.y, size.h) }
}

/// Top-left of the second pet's cell: one step left of the first, same height.
pub fn second_pet_cell(mode: DisplayMode, spot: Spot, size: Size) -> Point {
    let c = pet_cell(mode, spot, size, 2);
    Point { x: c.x - PET_STEP, y: c.y }
}

/// [top-left, top-right, bottom-right, bottom-left].
pub fn corner_radii(mode: DisplayMode, radius: f32) -> [f32; 4] {
    match mode {
        DisplayMode::Pill => [radius; 4],
        DisplayMode::Attached => [0.0, 0.0, radius, radius],
    }
}

/// Where the window may go: the work area (pill) or the whole monitor (attached).
fn area(mode: DisplayMode, work_area: Rect, bounds: Rect) -> Rect {
    match mode {
        DisplayMode::Pill => work_area,
        DisplayMode::Attached => bounds,
    }
}

fn pet_shift(p: f32, pets: u8) -> f32 {
    pets_w(pets) / 2.0 * (1.0 - (2.0 * p - 1.0).abs())
}

/// The window's top-left in physical pixels. It moves across the work area (pill) or the whole
/// monitor (attached), grown by the shadow room less the margin, so that at an edge the island
/// is `margin` from it and the room hangs past it. A monitor narrower or shorter than the window
/// centres it on that axis.
pub fn window_origin(mode: DisplayMode, work_area: Rect, bounds: Rect, scale: f32, spot: Spot, pets: u8) -> Point {
    let a = area(mode, work_area, bounds);
    let hang = (SHADOW_ROOM - margin(mode)) * scale;
    let along = |start: f32, len: f32, win: f32, p: f32, pets: u8| {
        let room = len + 2.0 * hang - win * scale;
        let p = if room > 0.0 { p } else { 0.5 };
        let shift = pet_shift(p, pets) * scale;
        start - hang + (room * p - shift).round()
    };
    Point { x: along(a.x, a.w, WINDOW.w, spot.x, pets), y: along(a.y, a.h, WINDOW.h, spot.y, 0) }
}

/// The spot that puts a block of `size` (DIPs) with its top-left at `at` (physical
/// pixels); past an edge it stays at that edge. The inverse of `window_origin` + `block_rect`.
pub fn spot_at(mode: DisplayMode, work_area: Rect, bounds: Rect, scale: f32, size: Size, pets: u8, at: Point) -> Spot {
    let a = area(mode, work_area, bounds);
    let m = margin(mode) * scale;
    // On x, invert the tent-shaped pet shift on either side of the centre.
    let along = |start: f32, len: f32, size: f32, pos: f32, pets: u8| {
        let span = len - 2.0 * m - size * scale;
        let c = pets_w(pets) * scale;
        if span - c <= 0.0 {
            return 0.5;
        }
        let s = (pos - start - m) / (span - c);
        if s > 0.5 { (pos - start - m + c) / (span + c) } else { s }
    };
    Spot::new(mode, along(a.x, a.w, size.w, at.x, pets), along(a.y, a.h, size.h, at.y, 0))
}

/// A press on the collapsed island becomes a drag once the pointer has moved this far.
const DRAG_START: f32 = 4.0;

/// Whether a move of `(dx, dy)` physical pixels is a drag rather than a click.
pub fn is_drag(dx: i32, dy: i32, scale: f32) -> bool {
    (dx as f32).hypot(dy as f32) / scale > DRAG_START
}

/// Whether `p` lies inside `rect` with the given corner radii.
pub fn hit_rounded(rect: Rect, radii: [f32; 4], p: Point) -> bool {
    let (l, t, r, b) = (rect.x, rect.y, rect.x + rect.w, rect.y + rect.h);
    if p.x < l || p.x > r || p.y < t || p.y > b {
        return false;
    }
    // each corner: its radius and the centre of its circle
    let corners = [
        (radii[0], l + radii[0], t + radii[0]),
        (radii[1], r - radii[1], t + radii[1]),
        (radii[2], r - radii[2], b - radii[2]),
        (radii[3], l + radii[3], b - radii[3]),
    ];
    for (i, (rad, cx, cy)) in corners.into_iter().enumerate() {
        if rad <= 0.0 {
            continue;
        }
        let in_x = if i == 0 || i == 3 { p.x < cx } else { p.x > cx };
        let in_y = if i < 2 { p.y < cy } else { p.y > cy };
        if in_x && in_y && (p.x - cx).powi(2) + (p.y - cy).powi(2) > rad * rad {
            return false;
        }
    }
    true
}

/// The pane to jump to when the collapsed island is clicked: the one agent that is blocked or
/// done, if exactly one is.
pub fn jump_target(agents: &AgentTable) -> Option<String> {
    if agents.count(AgentStatus::Blocked) + agents.count(AgentStatus::Done) != 1 {
        return None;
    }
    agents
        .first(AgentStatus::Blocked)
        .or_else(|| agents.first(AgentStatus::Done))
        .map(|a| a.info.pane_id.clone())
}

/// A click on the blocked pill over fullscreen can't expand, so with several agents needing
/// attention it jumps to the longest-blocked one.
pub fn fullscreen_jump_target(agents: &AgentTable) -> Option<String> {
    jump_target(agents).or_else(|| agents.first(AgentStatus::Blocked).map(|a| a.info.pane_id.clone()))
}

/// Time in state for an agent row: `12s`, `4m`, `1h` (whole units, rounded down).
pub fn time_in_state(d: std::time::Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        _ => format!("{}h", s / 3600),
    }
}

/// A media time: `m:ss`, or `h:mm:ss` from an hour.
pub fn clock_label(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Expanded-island layout, relative to the expanded island's top-left (DIPs).
pub mod layout {
    use crate::Rect;
    use super::Tab;

    pub const PAD_Y: f32 = 14.0;
    pub const PAD_X: f32 = 16.0;
    pub const TAB_AGENTS: Rect = Rect { x: 232.0, y: 10.0, w: 60.0, h: 24.0 };
    pub const TAB_MEDIA: Rect = Rect { x: 296.0, y: 10.0, w: 54.0, h: 24.0 };

    /// Agents is always shown; alone it aligns with the pair's right edge.
    pub fn tabs(media: bool) -> impl Iterator<Item = (Tab, Rect)> {
        let agents = if media { TAB_AGENTS } else { Rect { x: TAB_MEDIA.x + TAB_MEDIA.w - TAB_AGENTS.w, ..TAB_AGENTS } };
        std::iter::once((Tab::Agents, agents)).chain(media.then_some((Tab::Media, TAB_MEDIA)))
    }

    pub const MAX_ROWS: usize = 4;
    pub const ROW_X: f32 = 8.0;
    pub const ROW_Y: f32 = 44.0;
    pub const ROW_STEP: f32 = 34.0;
    pub const ROW_W: f32 = 344.0;
    pub const ROW_H: f32 = 32.0;
    pub const ROW_RADIUS: f32 = 10.0;
    pub const LIST: Rect = Rect {
        x: 0.0, y: ROW_Y, w: super::EXPANDED.w,
        h: ROW_STEP * (MAX_ROWS - 1) as f32 + ROW_H,
    };
    pub const ART: Rect = Rect { x: 16.0, y: 16.0, w: 70.0, h: 70.0 };
    pub const TITLE: Rect = Rect { x: 98.0, y: 34.0, w: 200.0, h: 18.0 };
    pub const ARTIST: Rect = Rect { x: 98.0, y: 54.0, w: 246.0, h: 16.0 };
    pub const SEEK: Rect = Rect { x: 16.0, y: 108.0, w: 328.0, h: 4.0 };
    pub const SEEK_HIT_H: f32 = 12.0;
    pub const TIMES: Rect = Rect { x: 16.0, y: 118.0, w: 328.0, h: 12.0 };
    /// Circles: (centre x, centre y, radius).
    pub const PREV: (f32, f32, f32) = (130.0, 160.0, 18.0);
    pub const PLAY: (f32, f32, f32) = (180.0, 160.0, 22.0);
    pub const NEXT: (f32, f32, f32) = (230.0, 160.0, 18.0);

    pub fn row(i: usize) -> Rect {
        Rect { x: ROW_X, y: ROW_Y + ROW_STEP * i as f32, w: ROW_W, h: ROW_H }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExpandedHit {
    Tab(Tab),
    Row(usize),
    Prev,
    PlayPause,
    Next,
    /// Fraction 0..=1 along the track.
    Seek(f32),
}

fn inside(r: Rect, p: Point) -> bool {
    p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h
}

fn in_circle((cx, cy, r): (f32, f32, f32), p: Point) -> bool {
    (p.x - cx).powi(2) + (p.y - cy).powi(2) <= r * r
}

pub fn max_scroll(rows: usize) -> usize {
    rows.saturating_sub(layout::MAX_ROWS)
}

/// Positive wheel notches move toward the first row.
pub fn scroll_by(offset: usize, notches: i32, rows: usize) -> usize {
    let offset = offset.min(max_scroll(rows));
    if notches >= 0 {
        offset.saturating_sub(notches as usize)
    } else {
        offset.saturating_add(notches.unsigned_abs() as usize).min(max_scroll(rows))
    }
}

/// What a click at `p` (relative to the expanded island's top-left) hit, if anything.
pub fn hit_expanded(tab: Tab, media_present: bool, rows: usize, scroll: usize, seekable: bool, p: Point) -> Option<ExpandedHit> {
    use layout::*;
    for (tab, r) in tabs(media_present) {
        if inside(r, p) {
            return Some(ExpandedHit::Tab(tab));
        }
    }
    match tab {
        Tab::Agents => {
            if !inside(LIST, p) {
                return None;
            }
            let scroll = scroll.min(max_scroll(rows));
            (scroll..rows.min(scroll.saturating_add(MAX_ROWS)))
                .find(|&i| inside(row(i - scroll), p))
                .map(ExpandedHit::Row)
        }
        Tab::Media => {
            let seek_hit = Rect {
                x: SEEK.x,
                y: SEEK.y + SEEK.h / 2.0 - SEEK_HIT_H / 2.0,
                w: SEEK.w,
                h: SEEK_HIT_H,
            };
            if seekable && inside(seek_hit, p) {
                return Some(ExpandedHit::Seek(((p.x - SEEK.x) / SEEK.w).clamp(0.0, 1.0)));
            }
            [(PREV, ExpandedHit::Prev), (PLAY, ExpandedHit::PlayPause), (NEXT, ExpandedHit::Next)]
                .into_iter()
                .find(|(c, _)| in_circle(*c, p))
                .map(|(_, hit)| hit)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::test_info;
    use crate::herdr::AgentStatus::*;
    use std::time::{Duration, Instant};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn priority_ignores_media() {
        let t0 = Instant::now();
        for media in [false, true] {
            let mut t = AgentTable::default();
            assert_eq!(choose_collapsed(&t, media), Collapsed { dot: Dot::Idle, media });
            t.upsert(test_info("w1:p1", Working), t0);
            assert_eq!(choose_collapsed(&t, media), Collapsed { dot: Dot::Working, media });
            t.upsert(test_info("w1:p2", Done), t0);
            assert_eq!(choose_collapsed(&t, media), Collapsed { dot: Dot::Done, media });
            t.upsert(test_info("w1:p3", Blocked), t0);
            assert_eq!(choose_collapsed(&t, media), Collapsed { dot: Dot::Blocked, media });
            t.upsert(test_info("w1:p1", Idle), t0);
            t.remove("w1:p2");
            t.remove("w1:p3");
            assert_eq!(choose_collapsed(&t, media), Collapsed { dot: Dot::Idle, media });
        }
    }

    #[test]
    fn tab_layouts_and_hits() {
        assert_eq!(layout::tabs(false).collect::<Vec<_>>(), vec![(Tab::Agents, rect(290.0, 10.0, 60.0, 24.0))]);
        assert_eq!(layout::tabs(true).collect::<Vec<_>>(), vec![
            (Tab::Agents, rect(232.0, 10.0, 60.0, 24.0)),
            (Tab::Media, rect(296.0, 10.0, 54.0, 24.0)),
        ]);
        let hit = |media, x| hit_expanded(Tab::Agents, media, 0, 0, false, Point { x, y: 20.0 });
        assert_eq!(hit(false, 291.0), Some(ExpandedHit::Tab(Tab::Agents)));
        assert_eq!(hit(false, 345.0), Some(ExpandedHit::Tab(Tab::Agents)));
        assert_eq!(hit(false, 250.0), None);
        assert_eq!(hit(true, 250.0), Some(ExpandedHit::Tab(Tab::Agents)));
        assert_eq!(hit(true, 300.0), Some(ExpandedHit::Tab(Tab::Media)));
        assert_eq!(hit(true, 294.0), None);
        for x in 0..=360 {
            assert_ne!(hit(false, x as f32), Some(ExpandedHit::Tab(Tab::Media)));
        }
    }

    #[test]
    fn sizes() {
        assert_eq!(COLLAPSED, Size { w: 32.0, h: 32.0 });
    }

    const TOP: Spot = Spot::TOP_CENTRE;

    const PET_SIZES: [Size; 4] = [
        Size { w: 40.0, h: 40.0 },
        Size { w: 160.0, h: 40.0 },
        Size { w: 260.0, h: 40.0 },
        EXPANDED,
    ];
    const PET_SPOTS: [Spot; 5] = [
        Spot { x: 0.0, y: 0.0 },
        Spot { x: 1.0, y: 0.0 },
        Spot { x: 0.0, y: 1.0 },
        Spot { x: 1.0, y: 1.0 },
        Spot { x: 0.5, y: 0.5 },
    ];

    #[test]
    fn the_pet_sits_left_of_the_island() {
        for mode in [DisplayMode::Pill, DisplayMode::Attached] {
            for size in PET_SIZES {
                for spot in PET_SPOTS {
                    if mode == DisplayMode::Attached && spot.y != 0.0 {
                        continue;
                    }
                    let c = pet_cell(mode, spot, size, 1);
                    let r = island_rect(mode, spot, size, 1);
                    assert_eq!(c.x + PET.w + PET_GAP, r.x);
                    if mode == DisplayMode::Attached {
                        assert_eq!(c.y, r.y);
                    } else if spot.y <= 0.5 {
                        assert_eq!(c.y + PET_BOX.h / 2.0, r.y + r.h / 2.0);
                    } else if spot.y == 1.0 {
                        assert_eq!(c.y + PET_BOX.h, r.y + r.h);
                    }
                }
            }
        }
    }

    #[test]
    fn the_second_pet_cell_matches_the_embedded_micomet_atlas() {
        let sheet = crate::pet::Sheet::parse(include_str!("../../../apps/island/assets/pet/micomet.json")).unwrap();
        assert_eq!(SECOND_PET, Size { w: sheet.solo.cell_w as f32, h: sheet.solo.cell_h as f32 });
    }

    #[test]
    fn micomet_sits_one_step_left_of_the_pet() {
        for mode in [DisplayMode::Pill, DisplayMode::Attached] {
            for size in PET_SIZES {
                for spot in PET_SPOTS {
                    if mode == DisplayMode::Attached && spot.y != 0.0 {
                        continue;
                    }
                    let (one, two) = (pet_cell(mode, spot, size, 1), pet_cell(mode, spot, size, 2));
                    let s = second_pet_cell(mode, spot, size);
                    assert_eq!(s.x + 112.0, two.x);
                    // miComet's cell reaches 16 DIPs into the first pet's.
                    assert_eq!(s.x + 128.0, two.x + 16.0);
                    assert_eq!(s.y, two.y);
                    // the first pet keeps its place relative to the island
                    assert_eq!(two.x + PET.w + PET_GAP, island_rect(mode, spot, size, 2).x);
                    assert_eq!(one.x + PET.w + PET_GAP, island_rect(mode, spot, size, 1).x);
                    assert_eq!(one.y, two.y);
                    let b = block_rect(spot, size, 2);
                    assert_eq!(s.x, b.x);
                    assert!(s.x + SECOND_PET.w <= b.x + b.w);
                    assert!(s.y >= b.y && s.y + SECOND_PET.h - 4.0 <= b.y + b.h);
                    assert_eq!(b.w, block_size(size, 1).w + 112.0);
                }
            }
        }
    }

    #[test]
    fn at_the_top_the_pet_is_centred_on_the_pill() {
        let area = rect(100.0, 80.0, 1920.0, 1040.0);
        let size = Size { w: 160.0, h: 40.0 };
        let o = window_origin(DisplayMode::Pill, area, area, 1.0, TOP, 1);
        let c = pet_cell(DisplayMode::Pill, TOP, size, 1);
        let p = island_on_screen(DisplayMode::Pill, area, area, 1.0, size, TOP, 1);
        assert_eq!(o.y + c.y, area.y + 4.0);
        assert_eq!(p.y, area.y + 14.0);
        assert_eq!(p.y + size.h / 2.0, o.y + c.y + 30.0);
    }

    #[test]
    fn attached_keeps_the_notch_on_the_edge() {
        let bounds = rect(100.0, 80.0, 1920.0, 1080.0);
        for size in [Size { w: 160.0, h: 40.0 }, EXPANDED] {
            let o = window_origin(DisplayMode::Attached, bounds, bounds, 1.0, TOP, 1);
            let c = pet_cell(DisplayMode::Attached, TOP, size, 1);
            let p = island_on_screen(DisplayMode::Attached, bounds, bounds, 1.0, size, TOP, 1);
            assert_eq!(p.y, bounds.y);
            assert_eq!(o.y + c.y, p.y);
        }
    }

    #[test]
    fn the_pet_and_island_stay_in_the_block() {
        for mode in [DisplayMode::Pill, DisplayMode::Attached] {
            for size in PET_SIZES {
                for spot in PET_SPOTS {
                    if mode == DisplayMode::Attached && spot.y != 0.0 {
                        continue;
                    }
                    let b = block_rect(spot, size, 1);
                    let r = island_rect(mode, spot, size, 1);
                    let c = pet_cell(mode, spot, size, 1);
                    for r in [r, Rect { x: c.x, y: c.y, w: PET_BOX.w, h: PET_BOX.h }] {
                        assert!(r.x >= b.x && r.y >= b.y, "{mode:?}, {spot:?}: {r:?} outside {b:?}");
                        assert!(r.x + r.w <= b.x + b.w && r.y + r.h <= b.y + b.h);
                    }
                }
            }
        }
    }

    #[test]
    fn the_block_fits_the_shadow_room() {
        assert_eq!(WINDOW, Size { w: 640.0, h: 300.0 });
        for size in PET_SIZES {
            for spot in PET_SPOTS {
                for pet in [0, 1, 2] {
                    let r = block_rect(spot, size, pet);
                    assert!(r.x >= 50.0 && r.y >= 50.0);
                    assert!(r.x + r.w <= WINDOW.w - 50.0 && r.y + r.h <= WINDOW.h - 50.0);
                }
            }
        }
    }

    #[test]
    fn a_centred_island_stays_centred() {
        for scale in [1.0, 1.5] {
            let area = rect(100.0, 80.0, 1920.0 * scale, 1040.0 * scale);
            for mode in [DisplayMode::Pill, DisplayMode::Attached] {
                for size in [Size { w: 40.0, h: 40.0 }, Size { w: 160.0, h: 40.0 }, EXPANDED] {
                    let p = island_on_screen(mode, area, area, scale, size, TOP, 1);
                    let want = area.x + (area.w - size.w * scale) / 2.0;
                    assert!((p.x - want).abs() <= 1.0, "{mode:?}, {size:?}, {scale}: {} != {want}", p.x);
                }
            }
        }
    }

    #[test]
    fn the_right_edge_ignores_the_pet() {
        for scale in [1.0, 1.5] {
            let area = rect(100.0, 80.0, 1920.0 * scale, 1040.0 * scale);
            let size = Size { w: 160.0, h: 40.0 };
            let p = island_on_screen(DisplayMode::Pill, area, area, scale, size, Spot { x: 1.0, y: 0.0 }, 1);
            assert_eq!(p.x + size.w * scale, area.x + area.w - 4.0 * scale);
        }
    }

    #[test]
    fn the_island_moves_steadily_with_the_spot() {
        let area = rect(0.0, 0.0, 1920.0, 1040.0);
        let size = Size { w: 160.0, h: 40.0 };
        let mut previous = f32::NEG_INFINITY;
        for i in 0..=20 {
            let spot = Spot { x: i as f32 * 0.05, y: 0.0 };
            let p = island_on_screen(DisplayMode::Pill, area, area, 1.0, size, spot, 1);
            assert!(p.x >= previous, "{spot:?}: {} < {previous}", p.x);
            previous = p.x;
        }
    }

    #[test]
    fn bottom_left_puts_the_pet_in_the_corner() {
        let wa = rect(100.0, 80.0, 1920.0, 1040.0);
        let size = Size { w: 160.0, h: 40.0 };
        let spot = Spot { x: 0.0, y: 1.0 };
        let o = window_origin(DisplayMode::Pill, wa, wa, 1.0, spot, 1);
        let c = pet_cell(DisplayMode::Pill, spot, size, 1);
        let p = island_on_screen(DisplayMode::Pill, wa, wa, 1.0, size, spot, 1);
        assert_eq!(o.x + c.x, wa.x + 4.0);
        assert_eq!(p.x, wa.x + 72.0);
        assert_eq!(p.y + size.h, wa.y + wa.h - 4.0);
        assert_eq!(o.y + c.y, p.y + size.h - 60.0);
    }

    #[test]
    fn without_the_pet_the_island_is_alone() {
        for size in PET_SIZES {
            for spot in PET_SPOTS {
                assert_eq!(block_rect(spot, size, 0), island_rect(DisplayMode::Pill, spot, size, 0));
            }
        }
    }

    #[test]
    fn placement() {
        let pill = Size { w: 160.0, h: 40.0 };
        let wa = rect(0.0, 0.0, 1920.0, 1040.0);
        let b = rect(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(island_on_screen(DisplayMode::Pill, wa, b, 1.0, pill, TOP, 0), Point { x: 880.0, y: 4.0 });
        // attached hangs from the very top of the monitor
        assert_eq!(island_on_screen(DisplayMode::Attached, wa, b, 1.0, EXPANDED, TOP, 0), Point { x: 780.0, y: 0.0 });
        assert_eq!(corner_radii(DisplayMode::Attached, 20.0), [0.0, 0.0, 20.0, 20.0]);
        assert_eq!(corner_radii(DisplayMode::Pill, 20.0), [20.0; 4]);
        let wa15 = rect(0.0, 0.0, 2880.0, 1680.0);
        assert_eq!(island_on_screen(DisplayMode::Pill, wa15, wa15, 1.5, pill, TOP, 0), Point { x: 1320.0, y: 6.0 });
        let top_taskbar = rect(0.0, 48.0, 1920.0, 1032.0);
        assert_eq!(island_on_screen(DisplayMode::Pill, top_taskbar, b, 1.0, pill, TOP, 0).y, 52.0);
        assert_eq!(island_on_screen(DisplayMode::Attached, top_taskbar, b, 1.0, pill, TOP, 0).y, 0.0);
        // second monitor to the left of the primary
        let left = rect(-1920.0, 0.0, 1920.0, 1080.0);
        assert_eq!(island_on_screen(DisplayMode::Pill, left, left, 1.0, pill, TOP, 0).x, -1040.0);
    }

    #[test]
    fn the_island_grows_away_from_the_edges_it_sits_at() {
        let wa = rect(0.0, 0.0, 1920.0, 1040.0);
        let on_screen = |spot, size| island_on_screen(DisplayMode::Pill, wa, wa, 1.0, size, spot, 0);
        let pill = Size { w: 40.0, h: 40.0 };
        // bottom-left: 4 in from the left, 4 above the bottom; expanded keeps that corner
        let bottom_left = Spot { x: 0.0, y: 1.0 };
        assert_eq!(on_screen(bottom_left, pill), Point { x: 4.0, y: 996.0 });
        assert_eq!(on_screen(bottom_left, EXPANDED), Point { x: 4.0, y: 836.0 });
        let top_right = Spot { x: 1.0, y: 0.0 };
        assert_eq!(on_screen(top_right, EXPANDED), Point { x: 1556.0, y: 4.0 });
        // in the middle it grows both ways from the same centre
        let middle = Spot { x: 0.5, y: 0.5 };
        assert_eq!(on_screen(middle, pill), Point { x: 940.0, y: 500.0 });
        assert_eq!(on_screen(middle, EXPANDED), Point { x: 780.0, y: 420.0 });
    }

    #[test]
    fn the_shadow_has_room_on_every_side() {
        let ok = |r: Rect| r.x >= 50.0 && r.y >= 50.0 && r.x + r.w <= WINDOW.w - 50.0 && r.y + r.h <= WINDOW.h - 50.0;
        let pill = Size { w: 40.0, h: 40.0 };
        for spot in [TOP, Spot { x: 0.0, y: 1.0 }, Spot { x: 1.0, y: 0.0 }, Spot { x: 0.5, y: 0.5 }] {
            assert!(ok(island_rect(DisplayMode::Pill, spot, pill, 0)), "{spot:?}");
            assert!(ok(island_rect(DisplayMode::Pill, spot, EXPANDED, 0)), "{spot:?}");
        }
    }

    #[test]
    fn spots_are_clamped_and_attached_stays_on_top() {
        assert_eq!(Spot::new(DisplayMode::Pill, 7.0, -1.0), Spot { x: 1.0, y: 0.0 });
        assert_eq!(Spot::new(DisplayMode::Pill, f32::NAN, f32::NAN), TOP);
        assert_eq!(Spot::new(DisplayMode::Pill, 0.0, 1.0), Spot { x: 0.0, y: 1.0 });
        assert_eq!(Spot::new(DisplayMode::Attached, 0.0, 1.0), Spot { x: 0.0, y: 0.0 });
    }

    #[test]
    fn rounded_hit() {
        let r = rect(0.0, 0.0, 160.0, 40.0);
        assert!(hit_rounded(r, [20.0; 4], Point { x: 80.0, y: 20.0 }));
        assert!(!hit_rounded(r, [20.0; 4], Point { x: 1.0, y: 1.0 }));
        assert!(!hit_rounded(r, [20.0; 4], Point { x: 159.0, y: 39.0 }));
        assert!(hit_rounded(r, [0.0, 0.0, 20.0, 20.0], Point { x: 1.0, y: 1.0 }));
        assert!(!hit_rounded(r, [0.0, 0.0, 20.0, 20.0], Point { x: 1.0, y: 39.0 }));
        assert!(!hit_rounded(r, [20.0; 4], Point { x: 170.0, y: 20.0 }));
    }

    #[test]
    fn rows_span_the_panel() {
        for i in 0..layout::MAX_ROWS {
            let r = layout::row(i);
            assert_eq!(r.x, 8.0);
            assert_eq!(r.x + r.w, EXPANDED.w - 8.0);
            let p = Point { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 };
            assert_eq!(hit_expanded(Tab::Agents, false, 5, 1, false, p), Some(ExpandedHit::Row(i + 1)));
        }
    }

    #[test]
    fn scrolling_clamps_to_the_available_rows() {
        for rows in 0..=4 {
            assert_eq!(max_scroll(rows), 0);
            assert_eq!(scroll_by(10, -100, rows), 0);
            assert_eq!(scroll_by(10, 100, rows), 0);
        }
        assert_eq!(max_scroll(5), 1);
        assert_eq!(scroll_by(0, -1, 5), 1);
        assert_eq!(scroll_by(1, 1, 5), 0);
        assert_eq!(scroll_by(2, -2, 10), 4);
        assert_eq!(scroll_by(2, 1, 10), 1);
        assert_eq!(scroll_by(2, i32::MIN, 10), 6);
        assert_eq!(scroll_by(2, i32::MAX, 10), 0);
        assert_eq!(scroll_by(20, 0, 5), 1);
    }

    #[test]
    fn scrolled_hits_use_absolute_rows_inside_the_list() {
        let hit = |rows, scroll, x, y| hit_expanded(Tab::Agents, false, rows, scroll, false, Point { x, y });
        assert_eq!(hit(5, 1, 100.0, 50.0), Some(ExpandedHit::Row(1)));
        assert_eq!(hit(5, 1, 100.0, 175.0), Some(ExpandedHit::Row(4)));
        assert_eq!(hit(2, 0, 100.0, 120.0), None);
        assert_eq!(hit(5, 1, 100.0, 43.0), None);
        assert_eq!(hit(6, 1, 100.0, 181.0), None);
        assert_eq!(hit(6, 1, 100.0, 178.5), None);
        assert_eq!(hit(5, 1, 100.0, 77.0), None);
        assert_eq!(hit(5, 1, 7.0, 50.0), None);
        assert_eq!(hit(5, 1, 353.0, 50.0), None);
    }

    #[test]
    fn expanded_hits() {
        let p = |x, y| Point { x, y };
        assert_eq!(hit_expanded(Tab::Media, true, 0, 0, true, p(180.0, 160.0)), Some(ExpandedHit::PlayPause));
        assert_eq!(hit_expanded(Tab::Media, true, 0, 0, true, p(130.0, 160.0)), Some(ExpandedHit::Prev));
        assert_eq!(hit_expanded(Tab::Media, true, 0, 0, true, p(230.0, 160.0)), Some(ExpandedHit::Next));
        assert_eq!(hit_expanded(Tab::Media, true, 0, 0, true, p(180.0, 110.0)), Some(ExpandedHit::Seek(0.5)));
        assert_eq!(hit_expanded(Tab::Media, true, 0, 0, false, p(180.0, 110.0)), None);
        assert_eq!(hit_expanded(Tab::Agents, true, 2, 0, false, p(100.0, 80.0)), Some(ExpandedHit::Row(1)));
        assert_eq!(hit_expanded(Tab::Agents, true, 1, 0, false, p(100.0, 80.0)), None);
        assert_eq!(hit_expanded(Tab::Agents, true, 0, 0, false, p(300.0, 20.0)), Some(ExpandedHit::Tab(Tab::Media)));
        assert_eq!(hit_expanded(Tab::Media, true, 0, 0, false, p(250.0, 20.0)), Some(ExpandedHit::Tab(Tab::Agents)));
        assert_eq!(hit_expanded(Tab::Agents, false, 0, 0, false, p(300.0, 20.0)), Some(ExpandedHit::Tab(Tab::Agents)));
        // buttons only exist on the media tab
        assert_eq!(hit_expanded(Tab::Agents, true, 0, 0, false, p(180.0, 160.0)), None);
    }

    #[test]
    fn time_in_state_labels() {
        let s = Duration::from_secs;
        assert_eq!(time_in_state(s(0)), "0s");
        assert_eq!(time_in_state(s(12)), "12s");
        assert_eq!(time_in_state(s(59)), "59s");
        assert_eq!(time_in_state(s(60)), "1m");
        assert_eq!(time_in_state(s(4 * 60 + 59)), "4m");
        assert_eq!(time_in_state(s(3599)), "59m");
        assert_eq!(time_in_state(s(3600)), "1h");
        assert_eq!(time_in_state(s(30 * 3600)), "30h");
    }

    #[test]
    fn clock_labels() {
        assert_eq!(clock_label(0.0), "0:00");
        assert_eq!(clock_label(42.7), "0:42");
        assert_eq!(clock_label(243.0), "4:03");
        assert_eq!(clock_label(3723.0), "1:02:03");
        assert_eq!(clock_label(-5.0), "0:00");
    }

    #[test]
    fn jump_only_when_exactly_one_agent_needs_attention() {
        let t0 = Instant::now();
        let mut t = AgentTable::default();
        t.upsert(test_info("w1:p1", Working), t0);
        assert_eq!(jump_target(&t), None);
        t.upsert(test_info("w1:p2", Done), t0);
        assert_eq!(jump_target(&t), Some("w1:p2".to_string()));
        t.upsert(test_info("w1:p3", Blocked), t0);
        assert_eq!(jump_target(&t), None);
        t.remove("w1:p2");
        assert_eq!(jump_target(&t), Some("w1:p3".to_string()));
    }

    #[test]
    fn over_fullscreen_a_click_jumps_to_the_oldest_blocked_agent() {
        let t0 = Instant::now();
        let mut t = AgentTable::default();
        t.upsert(test_info("w1:p1", Done), t0);
        t.upsert(test_info("w1:p2", Blocked), t0 + Duration::from_secs(1));
        t.upsert(test_info("w1:p3", Blocked), t0 + Duration::from_secs(2));
        assert_eq!(jump_target(&t), None);
        assert_eq!(fullscreen_jump_target(&t), Some("w1:p2".to_string()));
        t.remove("w1:p2");
        t.remove("w1:p3");
        assert_eq!(fullscreen_jump_target(&t), Some("w1:p1".to_string()));
    }

    #[test]
    fn fullscreen_shows_only_the_blocked_dot() {
        for media in [false, true] {
            for dot in [Dot::Idle, Dot::Working, Dot::Done, Dot::Blocked] {
                let c = Collapsed { dot, media };
                assert_eq!(shown(false, false, &c), Shown::All);
                assert_eq!(shown(false, true, &c), if dot == Dot::Blocked { Shown::CollapsedOnly } else { Shown::Hidden });
                assert_eq!(shown(true, false, &c), Shown::Hidden);
                assert_eq!(shown(true, true, &c), Shown::Hidden);
            }
        }
    }

    /// The island's top-left on screen (physical pixels) at `spot`.
    fn island_on_screen(mode: DisplayMode, wa: Rect, b: Rect, scale: f32, size: Size, spot: Spot, pet: u8) -> Point {
        let o = window_origin(mode, wa, b, scale, spot, pet);
        let r = island_rect(mode, spot, size, pet);
        Point { x: o.x + r.x * scale, y: o.y + r.y * scale }
    }

    #[test]
    fn spot_at_inverts_the_placement() {
        let wa = rect(0.0, 0.0, 1920.0, 1040.0);
        let b = rect(0.0, 0.0, 1920.0, 1080.0);
        for size in PET_SIZES {
            for (mode, spot) in [
                (DisplayMode::Pill, Spot { x: 0.2, y: 0.75 }),
                (DisplayMode::Pill, Spot { x: 0.25, y: 0.5 }),
                (DisplayMode::Pill, Spot { x: 0.75, y: 0.5 }),
                (DisplayMode::Pill, Spot { x: 0.0, y: 0.0 }),
                (DisplayMode::Pill, Spot { x: 0.0, y: 1.0 }),
                (DisplayMode::Pill, Spot { x: 1.0, y: 0.0 }),
                (DisplayMode::Pill, Spot { x: 1.0, y: 1.0 }),
                (DisplayMode::Pill, Spot { x: 0.5, y: 0.5 }),
                (DisplayMode::Attached, Spot { x: 0.5, y: 0.0 }),
            ] {
                for pet in [0, 1, 2] {
                    for scale in [1.0, 1.5] {
                        let o = window_origin(mode, wa, b, scale, spot, pet);
                        let r = block_rect(spot, size, pet);
                        let (x, y) = (o.x + r.x * scale, o.y + r.y * scale);
                        assert_near(spot_at(mode, wa, b, scale, block_size(size, pet), pet, Point { x, y }), spot);
                    }
                }
            }
        }
        // at 150 % on a second monitor to the left
        let left = rect(-2880.0, 0.0, 2880.0, 1620.0);
        let spot = Spot { x: 0.25, y: 0.5 };
        let size = Size { w: 160.0, h: 40.0 };
        for pet in [0, 1, 2] {
            let o = window_origin(DisplayMode::Pill, left, left, 1.5, spot, pet);
            let r = block_rect(spot, size, pet);
            let size = block_size(size, pet);
            assert_near(spot_at(DisplayMode::Pill, left, left, 1.5, size, pet, Point { x: o.x + r.x * 1.5, y: o.y + r.y * 1.5 }), spot);
        }
    }

    /// Equal up to the window snapping to whole pixels (under 1 px of ~1800).
    fn assert_near(got: Spot, want: Spot) {
        assert!((got.x - want.x).abs() < 1e-3 && (got.y - want.y).abs() < 1e-3, "{got:?} != {want:?}");
    }

    #[test]
    fn dragged_past_an_edge_it_stays_at_that_edge() {
        let wa = rect(0.0, 0.0, 1920.0, 1040.0);
        let size = Size { w: 160.0, h: 40.0 };
        assert_eq!(spot_at(DisplayMode::Pill, wa, wa, 1.0, size, 0, Point { x: -500.0, y: 5000.0 }), Spot { x: 0.0, y: 1.0 });
        assert_eq!(spot_at(DisplayMode::Pill, wa, wa, 1.0, size, 0, Point { x: 5000.0, y: -500.0 }), Spot { x: 1.0, y: 0.0 });
        // attached ignores the vertical move
        assert_eq!(spot_at(DisplayMode::Attached, wa, wa, 1.0, size, 0, Point { x: 880.0, y: 700.0 }), Spot { x: 0.5, y: 0.0 });
        // a monitor narrower than the island: centred
        let tiny = rect(0.0, 0.0, 100.0, 600.0);
        assert_eq!(spot_at(DisplayMode::Pill, tiny, tiny, 1.0, size, 0, Point { x: 0.0, y: 0.0 }).x, 0.5);
    }

    #[test]
    fn a_drag_starts_past_four_dips() {
        assert!(!is_drag(4, 0, 1.0));
        assert!(is_drag(5, 0, 1.0));
        assert!(is_drag(-5, 0, 1.0));
        assert!(is_drag(0, -5, 1.0));
        assert!(!is_drag(2, 3, 1.0));
        assert!(is_drag(3, 3, 1.0));
        assert!(!is_drag(6, 0, 1.5));
        assert!(is_drag(7, 0, 1.5));
    }
}
