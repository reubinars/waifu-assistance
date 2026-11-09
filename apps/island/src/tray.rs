//! The notification-area icon and its menu.

use crate::log;
use island_core::config::Config;
use island_core::platform::dot_icon_bgra;
use island_core::view::DisplayMode;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CheckMenuRadioItem, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetSystemMetrics, HICON,
    ICONINFO, MF_BYCOMMAND, MF_CHECKED, MF_SEPARATOR, MF_STRING, PostMessageW, SM_CXSMICON, SetForegroundWindow,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_APP, WM_NULL,
};
use windows::core::{PCWSTR, Result, w};

/// The icon's callback message, sent to the tray window.
pub const WM_APP_TRAY: u32 = WM_APP + 2;

const ID_SHOW: u32 = 1;
const ID_PILL: u32 = 2;
const ID_ATTACHED: u32 = 3;
const ID_STARTUP: u32 = 4;
const ID_QUIT: u32 = 5;
const ID_RESET: u32 = 6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    ToggleShown,
    Mode(DisplayMode),
    ResetPosition,
    ToggleStartup,
    Quit,
}

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
}

fn dot_icon() -> Result<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64);
    let px = dot_icon_bgra(size as u32);
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits.cast::<u8>(), px.len());
        let mask = CreateBitmap(size, size, 1, 1, None);
        let icon = CreateIconIndirect(&ICONINFO { fIcon: true.into(), hbmMask: mask, hbmColor: color, ..Default::default() });
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

impl Tray {
    fn data(&self) -> NOTIFYICONDATAW {
        let mut d = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: self.icon,
            ..Default::default()
        };
        for (dst, src) in d.szTip.iter_mut().zip("Island".encode_utf16()) {
            *dst = src;
        }
        d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        d
    }

    /// Adds the icon; `hwnd` receives `WM_APP_TRAY`.
    pub fn add(hwnd: HWND) -> Result<Tray> {
        let tray = Tray { hwnd, icon: dot_icon()? };
        tray.readd();
        Ok(tray)
    }

    /// (Re-)adds the icon, e.g. after Explorer restarted.
    pub fn readd(&self) {
        let d = self.data();
        unsafe {
            if !Shell_NotifyIconW(NIM_ADD, &d).as_bool() {
                log::warning!("tray icon not added");
                return;
            }
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &d);
        }
    }

    /// Shows the menu at screen point `(x, y)`; returns what was picked.
    pub fn show_menu(&self, x: i32, y: i32, cfg: &Config, visible: bool) -> Option<Command> {
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            let check = |on: bool| if on { MF_STRING | MF_CHECKED } else { MF_STRING };
            let items: [(u32, _, PCWSTR); 6] = [
                (ID_SHOW, MF_STRING, if visible { w!("Hide") } else { w!("Show") }),
                (ID_PILL, MF_STRING, w!("Pill")),
                (ID_ATTACHED, MF_STRING, w!("Attached")),
                (ID_RESET, MF_STRING, w!("Reset position")),
                (ID_STARTUP, check(cfg.start_with_windows), w!("Start with Windows")),
                (ID_QUIT, MF_STRING, w!("Quit")),
            ];
            for (id, flags, text) in items {
                if id == ID_QUIT {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
                let _ = AppendMenuW(menu, flags, id as usize, text);
            }
            let mode = if cfg.display_mode == DisplayMode::Attached { ID_ATTACHED } else { ID_PILL };
            let _ = CheckMenuRadioItem(menu, ID_PILL, ID_ATTACHED, mode, MF_BYCOMMAND.0);
            // the menu only dismisses on an outside click if its owner is foreground
            let _ = SetForegroundWindow(self.hwnd);
            let picked = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY).0, x, y, self.hwnd, None).0 as u32;
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            match picked {
                ID_SHOW => Some(Command::ToggleShown),
                ID_PILL => Some(Command::Mode(DisplayMode::Pill)),
                ID_ATTACHED => Some(Command::Mode(DisplayMode::Attached)),
                ID_RESET => Some(Command::ResetPosition),
                ID_STARTUP => Some(Command::ToggleStartup),
                ID_QUIT => Some(Command::Quit),
                _ => None,
            }
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data());
            let _ = DestroyIcon(self.icon);
        }
    }
}
