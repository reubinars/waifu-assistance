//! §7 fullscreen: is the foreground window covering the main monitor? Checked when the
//! foreground window changes or moves (WinEvent hooks), never polled.

use crate::log;
use crate::window;
use island_core::Rect;
use island_core::platform::is_fullscreen;
use std::cell::Cell;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_FOREGROUND, GetClassNameW, GetForegroundWindow, GetWindowRect,
    GWL_STYLE, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, IsZoomed, OBJID_WINDOW, PostMessageW,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP, WS_CAPTION,
};

/// Posted to the visual window when the answer changes; wParam is 1 for fullscreen.
pub const WM_APP_FULLSCREEN: u32 = WM_APP + 3;

thread_local! {
    static LAST: Cell<Option<bool>> = const { Cell::new(None) };
    static TARGET: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
}

/// Unhooks on drop.
pub struct Hooks(Vec<HWINEVENTHOOK>);

impl Drop for Hooks {
    fn drop(&mut self) {
        for h in &self.0 {
            unsafe {
                let _ = UnhookWinEvent(*h);
            }
        }
    }
}

/// Hooks foreground changes and moves and reports the current state to `visual`. Out of
/// context, the callback runs inside this thread's message calls (even `WaitMessage`), so it
/// posts rather than queues: a queued event would sit there until some other message came.
pub fn install(visual: HWND) -> Hooks {
    TARGET.with(|t| t.set(visual));
    let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
    let hooks = [EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_LOCATIONCHANGE]
        .into_iter()
        .filter_map(|e| {
            let h = unsafe { SetWinEventHook(e, e, None, Some(on_event), 0, 0, flags) };
            if h.is_invalid() {
                log::warning!("fullscreen hook {e:#x} not installed");
                None
            } else {
                Some(h)
            }
        })
        .collect();
    check(visual);
    Hooks(hooks)
}

unsafe extern "system" fn on_event(_: HWINEVENTHOOK, event: u32, hwnd: HWND, object: i32, child: i32, _: u32, _: u32) {
    // location changes fire for every window, caret and cursor: only the foreground window counts
    if event == EVENT_OBJECT_LOCATIONCHANGE
        && (object != OBJID_WINDOW.0 || child != CHILDID_SELF as i32 || hwnd != unsafe { GetForegroundWindow() })
    {
        return;
    }
    check(TARGET.with(Cell::get));
}

/// Re-evaluates the foreground window; posts `WM_APP_FULLSCREEN` to `visual` when the answer changes.
pub fn check(visual: HWND) {
    let fg = foreground();
    let (_, monitor, _) = window::primary_monitor();
    let now = fg.as_ref().is_some_and(|f| is_fullscreen(f.rect, monitor, &f.class, f.own, f.app_maximized));
    if LAST.with(|l| l.replace(Some(now))) != Some(now) {
        log::debug!("foreground {fg:?} on {monitor:?}: fullscreen {now}");
        unsafe {
            let _ = PostMessageW(Some(visual), WM_APP_FULLSCREEN, WPARAM(now as usize), LPARAM(0));
        }
    }
}

/// What `is_fullscreen` needs to know about the foreground window.
#[derive(Debug)]
struct Foreground {
    rect: Rect,
    class: String,
    own: bool,
    /// Maximized with a caption: an ordinary app window.
    app_maximized: bool,
}

/// `None` when there is no foreground window that could cover anything.
fn foreground() -> Option<Foreground> {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || IsIconic(fg).as_bool() || !IsWindowVisible(fg).as_bool() {
            return None;
        }
        let mut r = RECT::default();
        GetWindowRect(fg, &mut r).ok()?;
        let mut buf = [0u16; 256];
        let n = GetClassNameW(fg, &mut buf).max(0) as usize;
        let class = String::from_utf16_lossy(&buf[..n]);
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        let rect = Rect { x: r.left as f32, y: r.top as f32, w: (r.right - r.left) as f32, h: (r.bottom - r.top) as f32 };
        let captioned = GetWindowLongPtrW(fg, GWL_STYLE) as u32 & WS_CAPTION.0 == WS_CAPTION.0;
        let app_maximized = IsZoomed(fg).as_bool() && captioned;
        Some(Foreground { rect, class, own: pid == std::process::id(), app_maximized })
    }
}
