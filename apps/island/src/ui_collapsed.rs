//! The collapsed island: the morphing three-dot centre piece, the record behind it when media
//! is present, and the GitHub dot and approval badge.

use crate::draw::{Align, Color, Painter};
use island_core::Rect;
use island_core::collapsed::{Pose, tick_points};
use island_core::github::GhEvent;
use windows::core::Result;

const BLOCKED: Color = Color::rgba(0xff, 0x95, 0x00, 1.0);
const RECORD: Color = Color::rgba(0x14, 0x14, 0x14, 1.0);
const GROOVE: Color = Color::rgba(0x22, 0x22, 0x22, 1.0);
const GROOVE_W: f32 = 0.5;
const GROOVE_STEP: f32 = 1.5;
/// The sheen's wedges, widest first, stacked so the middle peaks near 0.16 white.
const SHEEN_WEDGES: [f32; 3] = [35.0, 23.0, 12.0];
const SHEEN_ALPHA: f32 = 0.055;
const TICK_W: f32 = 1.0;
const RIPPLE_W: f32 = 1.0;

pub struct Scene {
    /// Requests waiting on the user; above 0 a badge shows top-right.
    pub badge: u8,
    /// GitHub events not yet seen on the GitHub tab: a purple dot top-left.
    pub gh_dot: bool,
    /// The GitHub event the pop face shows (the last one, while it fades out).
    pub pop: Option<GhEvent>,
    /// The centre piece at `t`.
    pub pose: Pose,
}

const BADGE_R: f32 = 9.0;
const GH_DOT_R: f32 = 6.0;
/// The GitHub dot's centre, in from the top-left: far enough that the whole dot sits inside the
/// 32x32 round pill (centre (16, 16), radius 16).
const GH_DOT_INSET: f32 = 9.0;
const _: () = {
    let d = 16.0 - GH_DOT_INSET;
    // distance between the centres plus the dot's radius stays within the pill's radius
    assert!(2.0 * d * d <= (16.0 - GH_DOT_R) * (16.0 - GH_DOT_R));
};
/// GitHub purple.
const GH_DOT: Color = Color::rgba(0x89, 0x57, 0xe5, 1.0);

/// Draws the collapsed island inside `r` at opacity `k` (the caller clips to the island shape).
pub fn draw(p: &mut Painter, r: Rect, k: f32, s: &Scene) -> Result<()> {
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let pose = &s.pose;
    if pose.record_d > 0.0 {
        record(p, cx, cy, pose, k)?;
    }
    let [cr, cg, cb, ca] = pose.color;
    let rgb = |a: f32| Color { r: cr, g: cg, b: cb, a };
    if let Some(ring) = pose.ring {
        p.circle_outline(cx, cy, ring.d, rgb(ca * ring.alpha * k), RIPPLE_W);
    }
    // one layer for the shared colour's opacity, so stacked dots read as one dot
    p.push_layer(None, ca * k);
    for d in &pose.dots {
        p.circle(cx + d.x, cy + d.y, d.d, rgb(d.alpha));
    }
    p.pop_layer();
    let tick = tick_points(pose.tick);
    if tick.len() >= 2 {
        let pts: Vec<(f32, f32)> = tick.iter().map(|q| (cx + q.x, cy + q.y)).collect();
        p.polyline(&pts, Color::BLACK.fade(k), TICK_W)?;
    }
    if s.gh_dot {
        p.circle(r.x + GH_DOT_INSET, r.y + GH_DOT_INSET, 2.0 * GH_DOT_R, GH_DOT.fade(k));
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

/// The grey record: disc, grooves, turning sheen, black label.
fn record(p: &Painter, cx: f32, cy: f32, pose: &Pose, k: f32) -> Result<()> {
    let rim = pose.record_d / 2.0;
    p.circle(cx, cy, pose.record_d, RECORD.fade(k));
    let mut gr = pose.label_d / 2.0 + 1.0;
    while gr <= rim - 0.5 {
        p.circle_outline(cx, cy, 2.0 * gr, GROOVE.fade(k), GROOVE_W);
        gr += GROOVE_STEP;
    }
    for base in [pose.sheen, pose.sheen + std::f32::consts::PI] {
        for width in SHEEN_WEDGES {
            let half = width.to_radians() / 2.0;
            let steps = 6;
            let mut pts = vec![(cx, cy)];
            for i in 0..=steps {
                let a = base - half + 2.0 * half * i as f32 / steps as f32;
                pts.push((cx + rim * a.cos(), cy + rim * a.sin()));
            }
            p.polygon(&pts, Color::white(SHEEN_ALPHA * k))?;
        }
    }
    p.circle(cx, cy, pose.label_d, Color::BLACK.fade(k));
    Ok(())
}
