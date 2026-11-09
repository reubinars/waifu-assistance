//! `--demo`: scripted fake media and agents that cycle through every island state.

use super::{AgentSource, ArtImage, DemoAction, MediaCommand, MediaSource, SourceEvent, UiSender};
use crate::log;
use island_core::herdr::{AgentInfo, AgentStatus, Incoming};
use island_core::media::{MediaSnapshot, Playback};
use island_core::view::{DisplayMode, Tab};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// The song the script last sent and when, so `DemoMedia` can answer commands against it.
static SONG: Mutex<Option<(MediaSnapshot, Instant)>> = Mutex::new(None);

fn media(tx: &UiSender, snap: Option<MediaSnapshot>) {
    *SONG.lock().unwrap() = snap.clone().map(|s| (s, Instant::now()));
    tx.send(SourceEvent::Media(snap));
}

fn demo(tx: &UiSender, a: DemoAction) {
    tx.send(SourceEvent::Demo(a));
}

/// How long paused media stays shown in the demo.
pub const DEMO_LINGER: Duration = Duration::from_secs(3);

const STEP: Duration = Duration::from_secs(3);
const ACCENT: [u8; 3] = [0x9b, 0x5c, 0xff];

fn agent(pane: &str, agent: &str, status: AgentStatus, title: &str, cwd: &str) -> AgentInfo {
    AgentInfo {
        pane_id: pane.into(),
        workspace_id: "w1".into(),
        agent: Some(agent.into()),
        name: None,
        title: Some(title.into()),
        cwd: Some(cwd.into()),
        status,
    }
}

fn song(playback: Playback) -> MediaSnapshot {
    MediaSnapshot {
        title: "Midnight City".into(),
        artist: "M83".into(),
        album: "Hurry Up, We're Dreaming".into(),
        playback,
        position_s: 42.0,
        duration_s: 243.0,
        source_app: "demo".into(),
    }
}

/// 140×140 opaque purple→pink diagonal gradient.
fn gradient_art() -> ArtImage {
    let (w, h) = (140u32, 140u32);
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let t = (x + y) as f32 / (w + h - 2) as f32;
            let r = 0x6a as f32 + (0xff - 0x6a) as f32 * t;
            let g = 0x3b as f32 + (0x5e - 0x3b) as f32 * t;
            let b = 0xff as f32 + (0xb0 - 0xff) as f32 * t;
            bgra.extend_from_slice(&[b as u8, g as u8, r as u8, 255]);
        }
    }
    ArtImage { bgra, width: w, height: h, accent: Some(ACCENT) }
}

fn agents(tx: &UiSender, list: Vec<AgentInfo>) {
    tx.send(SourceEvent::Herdr(Incoming::AgentList { id: "demo".into(), agents: list }));
}

fn script(tx: UiSender) {
    tx.send(SourceEvent::Herdr(Incoming::Bridge { connected: true }));
    let proj = "/home/demo/projects/dynamic-island";
    let working = || {
        vec![
            agent("w1:p1", "claude", AgentStatus::Working, "Refactoring the herdr parser", proj),
            agent("w1:p2", "opencode", AgentStatus::Working, "Writing tests", "/home/demo/projects/nil"),
        ]
    };
    let long_title = format!("東京 🚀 {}", "x".repeat(200));
    let long_dir = "/home/demo/ディレクトリ-with-a-very-long-name";
    loop {
        // idle
        media(&tx, None);
        tx.send(SourceEvent::Art(None));
        agents(&tx, vec![]);
        thread::sleep(STEP);
        // media playing
        tx.send(SourceEvent::Art(Some(gradient_art())));
        media(&tx, Some(song(Playback::Playing)));
        thread::sleep(STEP);
        // working ×2 (media off: media outranks working)
        media(&tx, None);
        agents(&tx, working());
        thread::sleep(STEP);
        // done
        let mut list = working();
        list.push(agent("w1:p3", "codex", AgentStatus::Done, "Implemented the tray", "/home/demo/rea"));
        agents(&tx, list.clone());
        thread::sleep(STEP);
        // blocked ×2, long CJK/emoji title and folder
        list.retain(|a| a.status != AgentStatus::Done);
        list.push(agent("w1:p4", "claude", AgentStatus::Blocked, &long_title, long_dir));
        list.push(agent("w1:p5", "codex", AgentStatus::Blocked, "Allow edit to Cargo.toml?", proj));
        agents(&tx, list);
        thread::sleep(STEP);
        // expanded: agents tab (5 agents, 4 rows), then media tab; then attached mode
        let rows = vec![
            agent("w1:p6", "gemini", AgentStatus::Idle, "Waiting for input", "/home/demo/projects/nil"),
            agent("w1:p1", "claude", AgentStatus::Working, "Refactoring the herdr parser", proj),
            agent("w1:p4", "claude", AgentStatus::Blocked, &long_title, long_dir),
            agent("w1:p3", "codex", AgentStatus::Done, "Implemented the tray", "/home/demo/rea"),
            agent("w1:p2", "opencode", AgentStatus::Working, "Writing tests", "/home/demo/projects/nil"),
        ];
        agents(&tx, rows);
        tx.send(SourceEvent::Art(Some(gradient_art())));
        media(&tx, Some(song(Playback::Playing)));
        thread::sleep(Duration::from_secs(1));
        demo(&tx, DemoAction::Expand(Tab::Agents));
        thread::sleep(STEP);
        demo(&tx, DemoAction::Expand(Tab::Media));
        thread::sleep(STEP);
        demo(&tx, DemoAction::Collapse);
        agents(&tx, vec![]);
        demo(&tx, DemoAction::Mode(DisplayMode::Attached));
        thread::sleep(STEP);
        demo(&tx, DemoAction::Expand(Tab::Media));
        thread::sleep(STEP);
        demo(&tx, DemoAction::Collapse);
        thread::sleep(Duration::from_secs(1));
        demo(&tx, DemoAction::Mode(DisplayMode::Pill));
        // media paused: lingers DEMO_LINGER, then the island shrinks to idle by itself
        agents(&tx, vec![]);
        tx.send(SourceEvent::Art(Some(gradient_art())));
        media(&tx, Some(song(Playback::Paused)));
        thread::sleep(STEP + DEMO_LINGER);
    }
}

pub struct DemoAgents;

impl AgentSource for DemoAgents {
    fn start(&mut self, tx: UiSender) {
        thread::spawn(move || script(tx));
    }

    fn send(&self, line: &str) {
        log::debug!("demo bridge <- {line}");
    }

    fn raise_terminal(&self) {
        log::info!("demo: raise terminal");
    }
}

/// Answers play/pause, seek and skip by changing the current demo song.
#[derive(Default)]
pub struct DemoMedia {
    tx: Option<UiSender>,
}

impl MediaSource for DemoMedia {
    fn start(&mut self, tx: UiSender) {
        self.tx = Some(tx);
    }

    fn command(&self, c: MediaCommand) {
        log::info!("demo media command {c:?}");
        let Some(tx) = &self.tx else { return };
        let mut song = SONG.lock().unwrap();
        let Some((s, at)) = song.as_mut() else { return };
        if s.playback == Playback::Playing {
            s.position_s = (s.position_s + at.elapsed().as_secs_f64()).min(s.duration_s);
        }
        *at = Instant::now();
        match c {
            MediaCommand::PlayPause => {
                s.playback = if s.playback == Playback::Playing { Playback::Paused } else { Playback::Playing };
            }
            MediaCommand::Seek(pos) => s.position_s = pos,
            MediaCommand::Next | MediaCommand::Previous => {
                s.title = if s.title == "Midnight City" { "Wait".into() } else { "Midnight City".into() };
                s.position_s = 0.0;
            }
        }
        tx.send(SourceEvent::Media(Some(s.clone())));
    }
}
