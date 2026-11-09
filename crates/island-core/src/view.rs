//! What the island shows, where it sits, and what a click hit.

use crate::agents::{Agent, AgentTable};
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

#[derive(Clone, Debug, PartialEq)]
pub enum Collapsed {
    Idle,
    Media,
    Working { count: usize },
    Done { label: String, pane_id: String },
    Blocked { label: String, pane_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Agents,
    Media,
}

/// The fixed transparent window the island is drawn in (DIPs).
pub const WINDOW: Size = Size { w: 420.0, h: 260.0 };
pub const EXPANDED: Size = Size { w: 360.0, h: 200.0 };
const COLLAPSED_H: f32 = 40.0;
const PILL_TOP: f32 = 4.0;

fn label(a: &Agent, verb: &str, others: usize) -> String {
    let mut s = format!("{} {verb}", a.display_name());
    if !a.folder().is_empty() {
        s.push_str(" · ");
        s.push_str(a.folder());
    }
    if others > 0 {
        s.push_str(&format!(" +{others}"));
    }
    s
}

/// Priority: blocked > done > media > working > idle.
pub fn choose_collapsed(agents: &AgentTable, media_present: bool) -> Collapsed {
    let others = (agents.count(AgentStatus::Blocked) + agents.count(AgentStatus::Done)).saturating_sub(1);
    if let Some(a) = agents.first(AgentStatus::Blocked) {
        return Collapsed::Blocked { label: label(a, "needs you", others), pane_id: a.info.pane_id.clone() };
    }
    if let Some(a) = agents.first(AgentStatus::Done) {
        return Collapsed::Done { label: label(a, "done", others), pane_id: a.info.pane_id.clone() };
    }
    if media_present {
        return Collapsed::Media;
    }
    match agents.count(AgentStatus::Working) {
        0 => Collapsed::Idle,
        count => Collapsed::Working { count },
    }
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
    match (user_hidden, fullscreen, c) {
        (true, _, _) => Shown::Hidden,
        (false, false, _) => Shown::All,
        (false, true, Collapsed::Blocked { .. }) => Shown::CollapsedOnly,
        (false, true, _) => Shown::Hidden,
    }
}

pub fn collapsed_size(c: &Collapsed) -> Size {
    let w = match c {
        Collapsed::Idle => 40.0,
        Collapsed::Media | Collapsed::Working { .. } => 160.0,
        Collapsed::Done { .. } => 240.0,
        Collapsed::Blocked { .. } => 260.0,
    };
    Size { w, h: COLLAPSED_H }
}

pub fn default_tab(c: &Collapsed) -> Tab {
    match c {
        Collapsed::Working { .. } | Collapsed::Done { .. } | Collapsed::Blocked { .. } => Tab::Agents,
        Collapsed::Idle | Collapsed::Media => Tab::Media,
    }
}

/// Tabs appear only when both agents and media have something to show.
pub fn tabs_visible(agents: &AgentTable, media_present: bool) -> bool {
    !agents.is_empty() && media_present
}

/// The island's rect inside the window, in DIPs.
pub fn island_rect(mode: DisplayMode, size: Size) -> Rect {
    let y = match mode {
        DisplayMode::Pill => PILL_TOP,
        DisplayMode::Attached => 0.0,
    };
    Rect { x: (WINDOW.w - size.w) / 2.0, y, w: size.w, h: size.h }
}

/// [top-left, top-right, bottom-right, bottom-left].
pub fn corner_radii(mode: DisplayMode, radius: f32) -> [f32; 4] {
    match mode {
        DisplayMode::Pill => [radius; 4],
        DisplayMode::Attached => [0.0, 0.0, radius, radius],
    }
}

/// The span the window slides along: the work area (pill) or the whole monitor (attached).
fn top_area(mode: DisplayMode, work_area: Rect, bounds: Rect) -> Rect {
    match mode {
        DisplayMode::Pill => work_area,
        DisplayMode::Attached => bounds,
    }
}

/// The window's top-left in physical pixels, at the top of the work area (pill) or of the whole
/// monitor (attached). `position` runs from 0 (left edge) through 0.5 (centred) to 1 (right edge);
/// out of range it is clamped, and NaN, or a monitor narrower than the window, centres it.
pub fn window_origin(mode: DisplayMode, work_area: Rect, bounds: Rect, scale: f32, position: f32) -> Point {
    let area = top_area(mode, work_area, bounds);
    let room = area.w - WINDOW.w * scale;
    let p = if room > 0.0 && !position.is_nan() { position.clamp(0.0, 1.0) } else { 0.5 };
    Point { x: area.x + (room * p).round(), y: area.y }
}

/// The `position` that puts the window's left edge at `x` (physical pixels), clamped to 0..=1.
pub fn window_position(mode: DisplayMode, work_area: Rect, bounds: Rect, scale: f32, x: f32) -> f32 {
    let area = top_area(mode, work_area, bounds);
    let room = area.w - WINDOW.w * scale;
    if room > 0.0 { ((x - area.x) / room).clamp(0.0, 1.0) } else { 0.5 }
}

/// A press on the collapsed island becomes a drag once the pointer has moved this far sideways.
const DRAG_START: f32 = 4.0;

/// Whether a sideways move of `dx` physical pixels is a drag rather than a click.
pub fn is_drag(dx: i32, scale: f32) -> bool {
    dx.abs() as f32 / scale > DRAG_START
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
/// The collapsed agent status: an icon `icon_w` wide, `gap`, then a label `text_w` wide, centred as one
/// group in `pill`, at least `pad` in from each side. Returns (icon x, label x, label width); a label wider
/// than the room left is cut to it (the caller trims the text to that width).
pub fn centred_status(pill: Rect, pad: f32, icon_w: f32, gap: f32, text_w: f32) -> (f32, f32, f32) {
    let label_w = text_w.min((pill.w - 2.0 * pad - icon_w - gap).max(0.0));
    let icon_x = (pill.x + (pill.w - icon_w - gap - label_w) / 2.0).max(pill.x + pad);
    (icon_x, icon_x + icon_w + gap, label_w)
}

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

    pub const PAD_Y: f32 = 14.0;
    pub const PAD_X: f32 = 16.0;
    pub const TAB_AGENTS: Rect = Rect { x: 232.0, y: 10.0, w: 60.0, h: 24.0 };
    pub const TAB_MEDIA: Rect = Rect { x: 296.0, y: 10.0, w: 54.0, h: 24.0 };
    pub const MAX_ROWS: usize = 4;
    pub const ROW_X: f32 = 8.0;
    pub const ROW_Y: f32 = 44.0;
    pub const ROW_STEP: f32 = 34.0;
    pub const ROW_W: f32 = 344.0;
    pub const ROW_H: f32 = 32.0;
    pub const ROW_RADIUS: f32 = 10.0;
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

/// What a click at `p` (relative to the expanded island's top-left) hit, if anything.
pub fn hit_expanded(tab: Tab, tabs_shown: bool, rows: usize, seekable: bool, p: Point) -> Option<ExpandedHit> {
    use layout::*;
    if tabs_shown {
        if inside(TAB_AGENTS, p) {
            return Some(ExpandedHit::Tab(Tab::Agents));
        }
        if inside(TAB_MEDIA, p) {
            return Some(ExpandedHit::Tab(Tab::Media));
        }
    }
    match tab {
        Tab::Agents => (0..rows.min(MAX_ROWS)).find(|&i| inside(row(i), p)).map(ExpandedHit::Row),
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
    fn priority_order() {
        let t0 = Instant::now();
        let mut t = AgentTable::default();
        assert_eq!(choose_collapsed(&t, false), Collapsed::Idle);
        t.upsert(test_info("w1:p1", Working), t0);
        assert_eq!(choose_collapsed(&t, false), Collapsed::Working { count: 1 });
        assert_eq!(choose_collapsed(&t, true), Collapsed::Media);
        t.upsert(test_info("w1:p2", Done), t0);
        assert!(matches!(choose_collapsed(&t, true), Collapsed::Done { pane_id, .. } if pane_id == "w1:p2"));
        t.upsert(test_info("w1:p3", Blocked), t0);
        assert!(matches!(choose_collapsed(&t, true), Collapsed::Blocked { pane_id, .. } if pane_id == "w1:p3"));
        t.upsert(test_info("w1:p1", Idle), t0);
        t.remove("w1:p2");
        t.remove("w1:p3");
        assert_eq!(choose_collapsed(&t, false), Collapsed::Idle);
        assert_eq!(default_tab(&Collapsed::Working { count: 1 }), Tab::Agents);
        assert_eq!(default_tab(&Collapsed::Idle), Tab::Media);
        assert_eq!(default_tab(&Collapsed::Media), Tab::Media);
    }

    #[test]
    fn labels() {
        let t0 = Instant::now();
        let mut t = AgentTable::default();
        t.upsert(test_info("w1:p1", Blocked), t0);
        let mut codex = test_info("w1:p2", Done);
        codex.agent = Some("codex".into());
        codex.cwd = None;
        t.upsert(codex.clone(), t0);
        assert_eq!(
            choose_collapsed(&t, false),
            Collapsed::Blocked { label: "claude needs you · learn-anything +1".into(), pane_id: "w1:p1".into() }
        );
        let mut only = AgentTable::default();
        only.upsert(codex, t0);
        assert_eq!(choose_collapsed(&only, false), Collapsed::Done { label: "codex done".into(), pane_id: "w1:p2".into() });
        // the oldest blocked agent is the one shown
        t.upsert(test_info("w1:p0", Blocked), t0 + Duration::from_secs(1));
        assert!(matches!(choose_collapsed(&t, false), Collapsed::Blocked { label, pane_id }
            if pane_id == "w1:p1" && label.ends_with(" +2")));
    }

    #[test]
    fn sizes() {
        assert_eq!(collapsed_size(&Collapsed::Idle), Size { w: 40.0, h: 40.0 });
        assert_eq!(collapsed_size(&Collapsed::Media).w, 160.0);
        assert_eq!(collapsed_size(&Collapsed::Working { count: 3 }).w, 160.0);
        assert_eq!(collapsed_size(&Collapsed::Done { label: String::new(), pane_id: String::new() }).w, 240.0);
        assert_eq!(collapsed_size(&Collapsed::Blocked { label: String::new(), pane_id: String::new() }).w, 260.0);
        let mut t = AgentTable::default();
        assert!(!tabs_visible(&t, true));
        t.upsert(test_info("w1:p1", Idle), Instant::now());
        assert!(!tabs_visible(&t, false));
        assert!(tabs_visible(&t, true));
    }

    #[test]
    fn placement() {
        assert_eq!(island_rect(DisplayMode::Pill, Size { w: 160.0, h: 40.0 }), rect(130.0, 4.0, 160.0, 40.0));
        assert_eq!(island_rect(DisplayMode::Attached, EXPANDED).y, 0.0);
        assert_eq!(corner_radii(DisplayMode::Attached, 20.0), [0.0, 0.0, 20.0, 20.0]);
        assert_eq!(corner_radii(DisplayMode::Pill, 20.0), [20.0; 4]);
        let wa = rect(0.0, 0.0, 1920.0, 1040.0);
        let b = rect(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(window_origin(DisplayMode::Pill, wa, b, 1.0, 0.5), Point { x: 750.0, y: 0.0 });
        let wa15 = rect(0.0, 0.0, 2880.0, 1680.0);
        assert_eq!(window_origin(DisplayMode::Pill, wa15, wa15, 1.5, 0.5), Point { x: 1125.0, y: 0.0 });
        let top_taskbar = rect(0.0, 48.0, 1920.0, 1032.0);
        assert_eq!(window_origin(DisplayMode::Pill, top_taskbar, b, 1.0, 0.5).y, 48.0);
        assert_eq!(window_origin(DisplayMode::Attached, top_taskbar, b, 1.0, 0.5).y, 0.0);
        // second monitor to the left of the primary
        let left = rect(-1920.0, 0.0, 1920.0, 1080.0);
        assert_eq!(window_origin(DisplayMode::Pill, left, left, 1.0, 0.5).x, -1170.0);
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
    fn expanded_hits() {
        let p = |x, y| Point { x, y };
        assert_eq!(hit_expanded(Tab::Media, true, 0, true, p(180.0, 160.0)), Some(ExpandedHit::PlayPause));
        assert_eq!(hit_expanded(Tab::Media, true, 0, true, p(130.0, 160.0)), Some(ExpandedHit::Prev));
        assert_eq!(hit_expanded(Tab::Media, true, 0, true, p(230.0, 160.0)), Some(ExpandedHit::Next));
        assert_eq!(hit_expanded(Tab::Media, true, 0, true, p(180.0, 110.0)), Some(ExpandedHit::Seek(0.5)));
        assert_eq!(hit_expanded(Tab::Media, true, 0, false, p(180.0, 110.0)), None);
        assert_eq!(hit_expanded(Tab::Agents, true, 2, false, p(100.0, 80.0)), Some(ExpandedHit::Row(1)));
        assert_eq!(hit_expanded(Tab::Agents, true, 1, false, p(100.0, 80.0)), None);
        assert_eq!(hit_expanded(Tab::Agents, true, 0, false, p(300.0, 20.0)), Some(ExpandedHit::Tab(Tab::Media)));
        assert_eq!(hit_expanded(Tab::Media, true, 0, false, p(250.0, 20.0)), Some(ExpandedHit::Tab(Tab::Agents)));
        assert_eq!(hit_expanded(Tab::Agents, false, 0, false, p(300.0, 20.0)), None);
        // buttons only exist on the media tab
        assert_eq!(hit_expanded(Tab::Agents, true, 0, false, p(180.0, 160.0)), None);
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
    fn fullscreen_shows_only_the_blocked_pill() {
        let blocked = Collapsed::Blocked { label: "a needs you".into(), pane_id: "w1:p1".into() };
        let done = Collapsed::Done { label: "a done".into(), pane_id: "w1:p1".into() };
        assert_eq!(shown(false, false, &Collapsed::Idle), Shown::All);
        assert_eq!(shown(false, false, &blocked), Shown::All);
        assert_eq!(shown(false, true, &blocked), Shown::CollapsedOnly);
        for c in [Collapsed::Idle, Collapsed::Media, Collapsed::Working { count: 2 }, done] {
            assert_eq!(shown(false, true, &c), Shown::Hidden, "{c:?}");
        }
        // hidden from the tray stays hidden, even for a blocked agent
        assert_eq!(shown(true, false, &blocked), Shown::Hidden);
        assert_eq!(shown(true, true, &blocked), Shown::Hidden);
    }

    #[test]
    fn status_group_is_centred_in_the_pill() {
        let pill = rect(100.0, 0.0, 160.0, 37.0);
        // spinner 14 + gap 8 + label 50 = 72 wide, so it starts (160 - 72) / 2 = 44 in
        assert_eq!(centred_status(pill, 10.0, 14.0, 8.0, 50.0), (144.0, 166.0, 50.0));
    }

    #[test]
    fn status_label_too_wide_keeps_the_padding() {
        let pill = rect(100.0, 0.0, 160.0, 37.0);
        // room for the label is 160 - 2 * 10 - 14 - 8 = 118; the caller trims it to that
        assert_eq!(centred_status(pill, 10.0, 14.0, 8.0, 300.0), (110.0, 132.0, 118.0));
    }

    #[test]
    fn position_slides_the_window_along_the_top() {
        let wa = rect(0.0, 0.0, 1920.0, 1040.0);
        let b = rect(0.0, 0.0, 1920.0, 1080.0);
        // the window is 420 wide, so its left edge goes from 0 to 1500
        assert_eq!(window_origin(DisplayMode::Pill, wa, b, 1.0, 0.0), Point { x: 0.0, y: 0.0 });
        assert_eq!(window_origin(DisplayMode::Pill, wa, b, 1.0, 1.0), Point { x: 1500.0, y: 0.0 });
        assert_eq!(window_origin(DisplayMode::Pill, wa, b, 1.0, 0.2), Point { x: 300.0, y: 0.0 });
        // a hand-edited config can't push it off screen
        assert_eq!(window_origin(DisplayMode::Pill, wa, b, 1.0, 7.0).x, 1500.0);
        assert_eq!(window_origin(DisplayMode::Pill, wa, b, 1.0, -1.0).x, 0.0);
        assert_eq!(window_origin(DisplayMode::Pill, wa, b, 1.0, f32::NAN).x, 750.0);
    }

    #[test]
    fn window_position_inverts_window_origin() {
        let left = rect(-1920.0, 0.0, 1920.0, 1080.0);
        assert_eq!(window_position(DisplayMode::Pill, left, left, 1.0, -1620.0), 0.2);
        // dragged past an edge
        assert_eq!(window_position(DisplayMode::Pill, left, left, 1.0, 50.0), 1.0);
        assert_eq!(window_position(DisplayMode::Pill, left, left, 1.0, -5000.0), 0.0);
        // attached measures against the whole monitor, like window_origin
        let wa = rect(100.0, 0.0, 1820.0, 1080.0);
        let b = rect(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(window_position(DisplayMode::Attached, wa, b, 1.0, 750.0), 0.5);
        // a monitor narrower than the window: the window stays centred
        let tiny = rect(0.0, 0.0, 300.0, 600.0);
        assert_eq!(window_position(DisplayMode::Pill, tiny, tiny, 1.0, 0.0), 0.5);
    }

    #[test]
    fn a_drag_starts_past_four_dips() {
        assert!(!is_drag(4, 1.0));
        assert!(is_drag(5, 1.0));
        assert!(is_drag(-5, 1.0));
        assert!(!is_drag(6, 1.5));
        assert!(is_drag(7, 1.5));
    }
}
