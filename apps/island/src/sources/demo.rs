//! `--demo`: scripted fake media and agents that cycle through every island state.

use super::{AgentSource, ArtImage, MediaCommand, MediaSource, SourceEvent, UiSender};
use crate::log;
use island_core::herdr::{AgentInfo, AgentStatus, Incoming};
use island_core::media::{MediaSnapshot, Playback};
use std::thread;
use std::time::Duration;

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
        tx.send(SourceEvent::Media(None));
        tx.send(SourceEvent::Art(None));
        agents(&tx, vec![]);
        thread::sleep(STEP);
        // media playing
        tx.send(SourceEvent::Art(Some(gradient_art())));
        tx.send(SourceEvent::Media(Some(song(Playback::Playing))));
        thread::sleep(STEP);
        // working ×2 (media off: media outranks working)
        tx.send(SourceEvent::Media(None));
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
        // media paused: lingers DEMO_LINGER, then the island shrinks to idle by itself
        agents(&tx, vec![]);
        tx.send(SourceEvent::Art(Some(gradient_art())));
        tx.send(SourceEvent::Media(Some(song(Playback::Paused))));
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

pub struct DemoMedia;

impl MediaSource for DemoMedia {
    fn start(&mut self, _tx: UiSender) {}

    fn command(&self, c: MediaCommand) {
        log::info!("demo media command {c:?}");
    }
}
