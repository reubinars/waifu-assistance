//! Background sources (media, agents) and how they reach the UI thread.

pub mod demo;

use island_core::herdr::Incoming;
use island_core::media::MediaSnapshot;
use std::sync::mpsc::Sender;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted to the visual window after each `SourceEvent` is queued.
pub const WM_APP_SOURCE: u32 = WM_APP + 1;

pub enum SourceEvent {
    Media(Option<MediaSnapshot>),
    Art(Option<ArtImage>),
    /// One line from the bridge, parsed.
    Herdr(Incoming),
    /// The bridge process exited or could not start.
    #[allow(dead_code, reason = "sent by the herdr bridge source (Chunk C)")]
    BridgeDown,
    /// `--demo` only: drive the island as a user would.
    Demo(DemoAction),
}

#[derive(Clone, Copy, Debug)]
pub enum DemoAction {
    Expand(island_core::view::Tab),
    Collapse,
    Mode(island_core::view::DisplayMode),
}

/// Premultiplied BGRA album art.
pub struct ArtImage {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub accent: Option<[u8; 3]>,
}

#[derive(Clone)]
pub struct UiSender {
    tx: Sender<SourceEvent>,
    hwnd: isize,
}

impl UiSender {
    pub fn new(tx: Sender<SourceEvent>, hwnd: HWND) -> UiSender {
        UiSender { tx, hwnd: hwnd.0 as isize }
    }

    /// Queues the event, then wakes the UI thread.
    pub fn send(&self, e: SourceEvent) {
        if self.tx.send(e).is_ok() {
            unsafe {
                let _ = PostMessageW(Some(HWND(self.hwnd as _)), WM_APP_SOURCE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
    /// Seconds.
    Seek(f64),
}

pub trait MediaSource {
    fn start(&mut self, tx: UiSender);
    fn command(&self, c: MediaCommand);
}

pub trait AgentSource {
    fn start(&mut self, tx: UiSender);
    /// Writes one request line to the bridge.
    fn send(&self, line: &str);
    /// Brings the terminal window forward (after `agent.focus`).
    fn raise_terminal(&self);
}

/// Used when a source isn't available.
pub struct NoSource;

impl MediaSource for NoSource {
    fn start(&mut self, _tx: UiSender) {}
    fn command(&self, _c: MediaCommand) {}
}

impl AgentSource for NoSource {
    fn start(&mut self, _tx: UiSender) {}
    fn send(&self, _line: &str) {}
    fn raise_terminal(&self) {}
}
