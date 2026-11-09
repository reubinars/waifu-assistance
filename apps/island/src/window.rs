//! Two windows (ledger Task 10 ruling): a click-through visual window the island is composed
//! into, and an invisible owned input window shaped like the island that takes the clicks.
//! Window procedures only queue events; the main loop hands them to `App`.

use crate::log;
use island_core::view::{DisplayMode, WINDOW, window_origin};
use island_core::{Point, Rect};
use std::cell::RefCell;
use std::collections::VecDeque;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY,
    MONITORINFO, MonitorFromPoint, RGN_OR, SetWindowRgn,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, IsWindow, WM_CAPTURECHANGED, WM_CLOSE, HTTRANSPARENT, IDC_ARROW, LWA_ALPHA, LoadCursorW,
    MA_NOACTIVATE, PostQuitMessage, RegisterClassExW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW,
    SetForegroundWindow, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, WM_ACTIVATE,
    WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
    WM_MOUSEMOVE, WM_NCHITTEST, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, Result, w};

/// Virtual-key code of Esc (Win32_UI_Input_KeyboardAndMouse, not worth a feature).
pub const VK_ESCAPE: u32 = 0x1b;

#[derive(Clone, Copy, Debug)]
pub enum Event {
    /// Cursor position in screen pixels.
    MouseMove(i32, i32),
    MouseLeave,
    MouseDown(i32, i32),
    MouseUp(i32, i32),
    Key(u32),
    /// The input window gained (true) or lost (false) activation.
    Activate(bool),
    Timer(usize),
    /// DPI, display or work area changed.
    Reposition,
    /// The input window lost mouse capture (mid-drag).
    CaptureLost,
}

/// Win32_UI_Controls, not worth a feature.
const WM_MOUSELEAVE: u32 = 0x02A3;

thread_local! {
    static EVENTS: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
}

fn push(e: Event) {
    EVENTS.with(|q| q.borrow_mut().push_back(e));
}

pub fn take_events() -> Vec<Event> {
    EVENTS.with(|q| q.borrow_mut().drain(..).collect())
}

fn xy(l: LPARAM) -> (i32, i32) {
    ((l.0 & 0xffff) as i16 as i32, ((l.0 >> 16) & 0xffff) as i16 as i32)
}

fn to_screen(hwnd: HWND, l: LPARAM) -> (i32, i32) {
    let (x, y) = xy(l);
    let mut p = POINT { x, y };
    unsafe {
        let _ = ClientToScreen(hwnd, &mut p);
    }
    (p.x, p.y)
}

extern "system" fn visual_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_TIMER => {
            push(Event::Timer(wp.0));
            LRESULT(0)
        }
        WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            push(Event::Reposition);
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

extern "system" fn input_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let mut t = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut t);
            }
            let (x, y) = to_screen(hwnd, lp);
            push(Event::MouseMove(x, y));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            push(Event::MouseLeave);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = to_screen(hwnd, lp);
            push(Event::MouseDown(x, y));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = to_screen(hwnd, lp);
            push(Event::MouseUp(x, y));
            LRESULT(0)
        }
        WM_KEYDOWN => {
            push(Event::Key(wp.0 as u32));
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            push(Event::CaptureLost);
            LRESULT(0)
        }
        // Alt+F4 on the focused (expanded) island: collapse like Esc. Letting DefWindowProc
        // destroy the input window would leave an island that can never be clicked again.
        WM_CLOSE => {
            push(Event::Key(VK_ESCAPE));
            LRESULT(0)
        }
        WM_ACTIVATE => {
            push(Event::Activate((wp.0 & 0xffff) != 0));
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => {
            let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
            if ex & WS_EX_NOACTIVATE.0 != 0 { LRESULT(MA_NOACTIVATE as isize) } else { unsafe { DefWindowProcW(hwnd, msg, wp, lp) } }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// The primary monitor's work area and bounds (physical pixels) and its scale (DPI / 96).
pub fn primary_monitor() -> (Rect, Rect, f32) {
    let r = |r: RECT| Rect { x: r.left as f32, y: r.top as f32, w: (r.right - r.left) as f32, h: (r.bottom - r.top) as f32 };
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        (r(info.rcWork), r(info.rcMonitor), dx as f32 / 96.0)
    }
}

pub struct Windows {
    pub visual: HWND,
    pub input: HWND,
    /// Visual window top-left, physical pixels.
    pub origin: (i32, i32),
    pub scale: f32,
    /// Last input-window placement: x, y, w, h, corner diameter, attached.
    placed: Option<(i32, i32, i32, i32, i32, bool)>,
    /// The foreground window before the island took focus.
    prev_foreground: Option<HWND>,
}

pub fn create(mode: DisplayMode) -> Result<Windows> {
    unsafe {
        let instance = GetModuleHandleW(None)?.into();
        let cursor = LoadCursorW(None, IDC_ARROW)?;
        for (name, proc_) in [
            (w!("IslandVisual"), visual_proc as unsafe extern "system" fn(_, _, _, _) -> _),
            (w!("IslandInput"), input_proc),
        ] {
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc_),
                hInstance: instance,
                hCursor: cursor,
                lpszClassName: name,
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 {
                return Err(windows::core::Error::from_thread());
            }
        }
        let visual = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            w!("IslandVisual"),
            w!("Island"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )?;
        SetLayeredWindowAttributes(visual, COLORREF(0), 255, LWA_ALPHA)?;
        let input = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("IslandInput"),
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            Some(visual),
            None,
            Some(instance),
            None,
        )?;
        let mut win = Windows { visual, input, origin: (0, 0), scale: 1.0, placed: None, prev_foreground: None };
        win.place(mode);
        let _ = ShowWindow(visual, SW_SHOWNOACTIVATE);
        Ok(win)
    }
}

impl Windows {
    /// Puts the visual window at the top centre of the primary monitor; updates `scale`.
    pub fn place(&mut self, mode: DisplayMode) {
        let (work, bounds, scale) = primary_monitor();
        let o = window_origin(mode, work, bounds, scale);
        let (w, h) = self.size_px_for(scale);
        self.origin = (o.x as i32, o.y as i32);
        self.scale = scale;
        self.placed = None;
        unsafe {
            if let Err(e) = SetWindowPos(self.visual, None, o.x as i32, o.y as i32, w as i32, h as i32, SWP_NOACTIVATE | SWP_NOZORDER) {
                log::warning!("place window: {e}");
            }
        }
        log::info!("window at {:?} scale {scale}", self.origin);
    }

    fn size_px_for(&self, scale: f32) -> (u32, u32) {
        ((WINDOW.w * scale).round() as u32, (WINDOW.h * scale).round() as u32)
    }

    pub fn size_px(&self) -> (u32, u32) {
        self.size_px_for(self.scale)
    }

    /// Screen pixels → window DIPs.
    pub fn to_dip(&self, x: i32, y: i32) -> Point {
        Point { x: (x - self.origin.0) as f32 / self.scale, y: (y - self.origin.1) as f32 / self.scale }
    }

    /// Moves and reshapes the input window to the island's current rect (DIPs) and radius.
    pub fn shape_input(&mut self, rect: Rect, radius: f32, mode: DisplayMode) {
        let s = self.scale;
        let (x0, y0) = ((rect.x * s).floor() as i32, (rect.y * s).floor() as i32);
        let (x1, y1) = (((rect.x + rect.w) * s).ceil() as i32, ((rect.y + rect.h) * s).ceil() as i32);
        let (w, h) = ((x1 - x0).max(1), (y1 - y0).max(1));
        let d = ((radius * 2.0 * s).round() as i32).min(w).min(h);
        let attached = mode == DisplayMode::Attached;
        let key = (self.origin.0 + x0, self.origin.1 + y0, w, h, d, attached);
        if self.placed == Some(key) {
            return;
        }
        self.placed = Some(key);
        unsafe {
            let _ = SetWindowPos(self.input, None, key.0, key.1, w, h, SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW);
            let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, d, d);
            if attached {
                let top = CreateRectRgn(0, 0, w, h / 2);
                CombineRgn(Some(rgn), Some(rgn), Some(top), RGN_OR);
                let _ = DeleteObject(top.into());
            }
            SetWindowRgn(self.input, Some(rgn), true);
        }
    }

    /// Expanded: the input window may take focus, and takes it.
    pub fn activate(&mut self) {
        unsafe {
            let fg = GetForegroundWindow();
            if fg != self.input && !fg.is_invalid() {
                self.prev_foreground = Some(fg);
            }
            self.set_noactivate(false);
            if !SetForegroundWindow(self.input).as_bool() {
                log::debug!("SetForegroundWindow refused");
            }
        }
    }

    /// Collapsed: no focus again. If the island still has it, it goes back to the window that
    /// had it before.
    pub fn deactivate(&mut self) {
        self.set_noactivate(true);
        let prev = self.prev_foreground.take();
        unsafe {
            if GetForegroundWindow() == self.input
                && let Some(prev) = prev
                && IsWindow(Some(prev)).as_bool()
            {
                let _ = SetForegroundWindow(prev);
            }
        }
    }

    fn set_noactivate(&self, on: bool) {
        unsafe {
            let ex = GetWindowLongPtrW(self.input, GWL_EXSTYLE);
            let flag = WS_EX_NOACTIVATE.0 as isize;
            SetWindowLongPtrW(self.input, GWL_EXSTYLE, if on { ex | flag } else { ex & !flag });
        }
    }

    /// Keeps mouse input coming while the seekbar is dragged outside the island.
    pub fn capture(&self, on: bool) {
        unsafe {
            if on {
                SetCapture(self.input);
            } else {
                let _ = ReleaseCapture();
            }
        }
    }
}
