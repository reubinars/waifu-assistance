//! The collapsed island: the morphing three-dot centre piece, the record behind it when media
//! is present, and the GitHub dot and approval badge.

use crate::draw::{Align, Color, Painter};
use island_core::Rect;
use island_core::collapsed::{Pose, tick_points};
use island_core::github::GhEvent;
use windows::core::Result;
use windows_numerics::Matrix3x2;

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

/// The approval badge and the GitHub dot are a matched pair of corner marks: same size, mirrored
/// insets, each far enough in that the whole mark sits inside the 32x32 round pill
/// (centre (16, 16), radius 16) instead of being cut by its edge.
const BADGE_R: f32 = 6.0;
const GH_DOT_R: f32 = 6.0;
const MARK_INSET: f32 = 9.0;
/// A black ring around each mark, cut out of whatever it sits on (record, ripple), so the marks
/// read as separate from the centre piece. Only the part outside the pill is clipped away.
const MARK_CUTOUT: f32 = 1.5;
const _: () = {
    let d = 16.0 - MARK_INSET;
    // distance between the centres plus the mark's radius stays within the pill's radius
    assert!(2.0 * d * d <= (16.0 - BADGE_R) * (16.0 - BADGE_R));
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
    // one layer for the shared colour's opacity, so stacked dots read as one dot; inside it the
    // dots take the max, not the sum, so dots still fading while they overlap don't flash brighter
    p.push_layer(None, ca * k);
    p.blend_max(true);
    for d in &pose.dots {
        p.circle(cx + d.x, cy + d.y, d.d, rgb(d.alpha));
    }
    p.blend_max(false);
    p.pop_layer();
    let tick = tick_points(pose.tick);
    if tick.len() >= 2 {
        let pts: Vec<(f32, f32)> = tick.iter().map(|q| (cx + q.x, cy + q.y)).collect();
        p.polyline(&pts, Color::BLACK.fade(k), TICK_W)?;
    }
    if s.gh_dot {
        let (gx, gy) = (r.x + MARK_INSET, r.y + MARK_INSET);
        p.circle(gx, gy, 2.0 * (GH_DOT_R + MARK_CUTOUT), Color::BLACK.fade(k));
        p.circle(gx, gy, 2.0 * GH_DOT_R, GH_DOT.fade(k));
    }
    if s.badge > 0 {
        let (bx, by) = (r.x + r.w - MARK_INSET, r.y + MARK_INSET);
        p.circle(bx, by, 2.0 * (BADGE_R + MARK_CUTOUT), Color::BLACK.fade(k));
        p.circle(bx, by, 2.0 * BADGE_R, BLOCKED.fade(k));
        // one digit fits the 12 px badge; ten or more waiting just says "more"
        let label = if s.badge > 9 { "+".to_string() } else { s.badge.to_string() };
        let box_ = Rect { x: bx - BADGE_R, y: by - BADGE_R, w: 2.0 * BADGE_R, h: 2.0 * BADGE_R };
        let l = p.layout_tabular(&label, box_, 9.0, 700, Align::Center)?;
        let (dx, dy) = Painter::ink_centring(&l);
        p.draw_layout(&l, box_.x + dx, box_.y + dy, Color::BLACK.fade(k));
    }
    Ok(())
}

/// A wedge `width` degrees wide from the origin out to radius 1, centred on +x.
fn unit_wedge(width: f32) -> Vec<(f32, f32)> {
    const STEPS: usize = 6;
    let half = width.to_radians() / 2.0;
    let arc = (0..=STEPS).map(|i| {
        let a = -half + 2.0 * half * i as f32 / STEPS as f32;
        (a.cos(), a.sin())
    });
    std::iter::once((0.0, 0.0)).chain(arc).collect()
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
    // the wedges are built once at unit size, pointing along +x, and turned and scaled into place
    const KEYS: [&str; 3] = ["sheen-wide", "sheen-mid", "sheen-narrow"];
    for base in [pose.sheen, pose.sheen + std::f32::consts::PI] {
        let (sin, cos) = base.sin_cos();
        let m = Matrix3x2 { M11: rim * cos, M12: rim * sin, M21: -rim * sin, M22: rim * cos, M31: cx, M32: cy };
        for (key, width) in KEYS.into_iter().zip(SHEEN_WEDGES) {
            p.polygon_cached(key, || unit_wedge(width), m, Color::white(SHEEN_ALPHA * k))?;
        }
    }
    p.circle(cx, cy, pose.label_d, Color::BLACK.fade(k));
    Ok(())
}
