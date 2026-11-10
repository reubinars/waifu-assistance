//! Pet state selection and atlas animation timing, independent of rendering.

use crate::{Point, Rect, agents::AgentTable, herdr::AgentStatus};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetState {
    Idle,
    Working,
    Blocked,
    Done,
    Sleep,
}

pub fn pet_state(agents: &AgentTable) -> PetState {
    if agents.is_empty() {
        return PetState::Sleep;
    }
    for (status, state) in [
        (AgentStatus::Blocked, PetState::Blocked),
        (AgentStatus::Done, PetState::Done),
        (AgentStatus::Working, PetState::Working),
    ] {
        if agents.count(status) > 0 {
            return state;
        }
    }
    PetState::Idle
}

pub struct Sheet {
    pub solo: Atlas,
    pub emblem: Atlas,
}

pub struct Atlas {
    pub cell_w: u32,
    pub cell_h: u32,
    pub columns: u32,
    pub rows: u32,
    pub anchor: (u32, u32),
    clips: [Clip; 5],
}

#[derive(Deserialize)]
struct RawSheet {
    schema_version: u32,
    atlases: RawAtlases,
}

#[derive(Deserialize)]
struct RawAtlases {
    solo: RawAtlas,
    emblem: RawAtlas,
}

#[derive(Deserialize)]
struct RawAtlas {
    cell_width: u32,
    cell_height: u32,
    columns: u32,
    rows: u32,
    anchor: (u32, u32),
    states: BTreeMap<String, Clip>,
}

#[derive(Deserialize)]
struct Clip {
    column: u32,
    frames: Vec<u32>,
    durations_ms: Vec<u32>,
    #[serde(rename = "loop")]
    looping: bool,
    resting_frame: u32,
    #[serde(skip)]
    total_ms: u32,
}

impl Sheet {
    pub fn parse(json: &str) -> Result<Sheet, String> {
        let raw: RawSheet = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if raw.schema_version != 3 {
            return Err("unsupported pet schema version".into());
        }
        Ok(Sheet {
            solo: Atlas::from_raw(raw.atlases.solo)?,
            emblem: Atlas::from_raw(raw.atlases.emblem)?,
        })
    }
}

impl Atlas {
    fn from_raw(mut raw: RawAtlas) -> Result<Self, String> {
        let mut clips = Vec::with_capacity(5);
        for name in ["idle", "working", "blocked", "done", "sleep"] {
            let mut clip = raw
                .states
                .remove(name)
                .ok_or_else(|| format!("missing pet state: {name}"))?;
            if clip.column >= raw.columns
                || clip.resting_frame >= raw.rows
                || clip.frames.iter().any(|&f| f >= raw.rows)
                || clip.frames.is_empty()
                || clip.frames.len() != clip.durations_ms.len()
                || clip.durations_ms.contains(&0)
            {
                return Err(format!("invalid pet clip: {name}"));
            }
            clip.total_ms = clip
                .durations_ms
                .iter()
                .try_fold(0u32, |sum, &ms| sum.checked_add(ms))
                .ok_or_else(|| format!("pet clip duration overflows: {name}"))?;
            clips.push(clip);
        }
        Ok(Self {
            cell_w: raw.cell_width,
            cell_h: raw.cell_height,
            columns: raw.columns,
            rows: raw.rows,
            anchor: raw.anchor,
            // Exactly the five named states above, in PetState order.
            clips: clips.try_into().ok().unwrap(),
        })
    }

    pub fn cell(&self, s: PetState, elapsed_ms: u32) -> (u32, u32) {
        let clip = &self.clips[s as usize];
        let row = clip
            .frame(elapsed_ms)
            .map_or(clip.resting_frame, |(i, _)| clip.frames[i]);
        (clip.column, row)
    }

    /// Milliseconds until the current frame ends; None once a one-shot rests.
    pub fn next_change(&self, s: PetState, elapsed_ms: u32) -> Option<u32> {
        self.clips[s as usize]
            .frame(elapsed_ms)
            .map(|(_, remaining)| remaining)
    }

    pub fn src(&self, (col, row): (u32, u32)) -> Rect {
        Rect {
            x: col as f32 * self.cell_w as f32,
            y: row as f32 * self.cell_h as f32,
            w: self.cell_w as f32,
            h: self.cell_h as f32,
        }
    }
}

/// Keep the cell's 1× anchor position while snapping integer-scaled pixels to the physical grid.
pub fn sprite_dest(at: Point, atlas: &Atlas, scale: f32) -> Rect {
    let n = scale.floor().max(1.0);
    let (ax, ay) = (atlas.anchor.0 as f32, atlas.anchor.1 as f32);
    Rect {
        x: ((at.x + ax) * scale - ax * n).round() / scale,
        y: ((at.y + ay) * scale - ay * n).round() / scale,
        w: atlas.cell_w as f32 * n / scale,
        h: atlas.cell_h as f32 * n / scale,
    }
}

impl Clip {
    fn frame(&self, elapsed_ms: u32) -> Option<(usize, u32)> {
        let mut elapsed = if self.looping {
            elapsed_ms % self.total_ms
        } else {
            elapsed_ms
        };
        for (i, &duration) in self.durations_ms.iter().enumerate() {
            if elapsed < duration {
                return Some((i, duration - elapsed));
            }
            elapsed -= duration;
        }
        None
    }
}

pub struct PetClock {
    state: PetState,
    changed_at: f64,
}

impl PetClock {
    pub fn new(s: PetState, t: f64) -> Self {
        Self { state: s, changed_at: t }
    }

    pub fn state(&self) -> PetState {
        self.state
    }

    pub fn set(&mut self, s: PetState, t: f64) {
        if self.state != s {
            self.state = s;
            self.changed_at = t;
        }
    }

    pub fn ms(&self, t: f64) -> u32 {
        elapsed_ms(t, self.changed_at)
    }
}

fn elapsed_ms(t: f64, start: f64) -> u32 {
    // Round away sub-millisecond floating-point error (e.g. 1.2 - 1.0).
    // Float-to-integer casts saturate at zero and u32::MAX.
    ((t - start) * 1000.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{agents::test_info, herdr::AgentStatus};
    use PetState::*;
    use std::time::Instant;

    const JSON: &str = include_str!("../../../apps/island/assets/pet/pet.json");

    fn sheet() -> Sheet {
        Sheet::parse(JSON).unwrap()
    }

    fn assert_near(got: f32, want: f32) {
        assert!((got - want).abs() < 1e-4, "{got} != {want}");
    }

    #[test]
    fn sprites_land_on_their_anchor_at_any_scale() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let r = sprite_dest(Point { x: 10.3, y: 4.0 }, &sheet().emblem, scale);
            let n = scale.floor().max(1.0);
            assert_near(r.w * scale, 32.0 * n);
            assert_near(r.h * scale, 32.0 * n);
            assert_near((r.x * scale).fract().min(1.0 - (r.x * scale).fract()), 0.0);
            assert_near((r.y * scale).fract().min(1.0 - (r.y * scale).fract()), 0.0);
            assert!(((r.x + 16.0 * n / scale) - (10.3 + 16.0)).abs() * scale <= 0.5);
            assert!(((r.y + 28.0 * n / scale) - (4.0 + 28.0)).abs() * scale <= 0.5);
        }
    }

    #[test]
    fn no_agents_sleeps() {
        assert_eq!(pet_state(&AgentTable::default()), Sleep);
    }

    #[test]
    fn the_highest_priority_status_wins() {
        let mut agents = AgentTable::default();
        let now = Instant::now();
        for (pane, status, want) in [
            ("a", AgentStatus::Idle, Idle),
            ("b", AgentStatus::Working, Working),
            ("c", AgentStatus::Done, Done),
            ("d", AgentStatus::Blocked, Blocked),
        ] {
            agents.upsert(test_info(pane, status), now);
            assert_eq!(pet_state(&agents), want);
        }
        agents.replace_all(vec![test_info("e", AgentStatus::Unknown)], now);
        assert_eq!(pet_state(&agents), Idle);
    }

    #[test]
    fn the_shipped_sheet_matches_its_atlases() {
        let s = sheet();
        assert_eq!(
            (s.solo.cell_w * s.solo.columns, s.solo.cell_h * s.solo.rows),
            (320, 256)
        );
        assert_eq!(
            (
                s.emblem.cell_w * s.emblem.columns,
                s.emblem.cell_h * s.emblem.rows
            ),
            (160, 64)
        );
        assert_eq!((s.solo.anchor, s.emblem.anchor), ((32, 60), (16, 28)));
        assert_eq!(
            s.solo.src((3, 2)),
            crate::Rect {
                x: 192.0,
                y: 128.0,
                w: 64.0,
                h: 64.0
            }
        );
    }

    #[test]
    fn bad_sheets_are_rejected() {
        let original: serde_json::Value = serde_json::from_str(JSON).unwrap();
        let mut bad = original.clone();
        bad["schema_version"] = 2.into();
        assert!(Sheet::parse(&bad.to_string()).is_err());
        let mut bad = original.clone();
        bad["atlases"]["solo"]["states"]["idle"]["frames"][0] = 9.into();
        assert!(Sheet::parse(&bad.to_string()).is_err());
        let mut bad = original.clone();
        bad["atlases"]["solo"]["states"]["idle"]["durations_ms"] = serde_json::json!([550]);
        assert!(Sheet::parse(&bad.to_string()).is_err());
        let mut bad = original.clone();
        bad["atlases"]["emblem"]["states"]
            .as_object_mut()
            .unwrap()
            .remove("sleep");
        assert!(Sheet::parse(&bad.to_string()).is_err());
        let mut bad = original.clone();
        bad["atlases"]["emblem"]["states"]["sleep"]["column"] = 5.into();
        assert!(Sheet::parse(&bad.to_string()).is_err());
        for (field, value) in [
            ("frames", serde_json::json!([])),
            ("durations_ms", serde_json::json!([0, 90, 160])),
            ("resting_frame", serde_json::json!(2)),
            ("durations_ms", serde_json::json!([u32::MAX, 90, 160])),
        ] {
            let mut bad = original.clone();
            bad["atlases"]["emblem"]["states"]["sleep"][field] = value;
            assert!(Sheet::parse(&bad.to_string()).is_err(), "{field}");
        }
    }

    #[test]
    fn loops_wrap() {
        let s = sheet();
        assert_eq!(s.solo.cell(Working, 0), (1, 0));
        assert_eq!(s.solo.cell(Working, 260), (1, 1));
        assert_eq!(s.solo.cell(Working, 520), (1, 0));
        assert_eq!(s.solo.next_change(Working, 100), Some(160));
    }

    #[test]
    fn one_shots_rest() {
        let s = sheet();
        assert_eq!(s.solo.cell(Done, 199), (3, 0));
        assert_eq!(s.solo.cell(Done, 700), (3, 3));
        assert_eq!(s.solo.cell(Done, 60_000), (3, 3));
        assert_eq!(s.solo.next_change(Done, 1_600), None);
    }

    #[test]
    fn the_emblem_blinks_once() {
        let s = sheet();
        assert_eq!(s.emblem.cell(Sleep, 200), (4, 1));
        assert_eq!(s.emblem.cell(Sleep, 300), (4, 0));
        assert_eq!(s.emblem.next_change(Sleep, 300), Some(110));
        assert_eq!(s.emblem.next_change(Sleep, 410), None);
    }

    #[test]
    fn the_clock_restarts_only_on_a_change() {
        let mut clock = PetClock::new(Working, 0.0);
        clock.set(Working, 1.0);
        assert_eq!(clock.state(), Working);
        assert_eq!(clock.ms(1.0), 1000);
        clock.set(Idle, 1.0);
        assert_eq!(clock.state(), Idle);
        assert_eq!(clock.ms(1.2), 200);
        assert_eq!(clock.ms(0.5), 0);
        assert_eq!(clock.ms(f64::MAX), u32::MAX);
        clock.set(Working, 2.0);
        assert_eq!(clock.ms(2.0), 0);
    }
}
