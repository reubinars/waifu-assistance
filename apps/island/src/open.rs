//! Opens a URI or program through the shell, off the UI thread.

use crate::log;
use island_core::github::text::is_safe_url;
use std::thread;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

/// Opens `target` the way the Run dialog would. Off the UI thread: the shell wants an STA and may
/// take a moment. `what` names the action in the log; the target is not logged.
pub fn shell_open(target: String, what: &'static str) {
    thread::spawn(move || {
        let file: Vec<u16> = target.encode_utf16().chain([0]).collect();
        let r = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            ShellExecuteW(None, w!("open"), PCWSTR(file.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
        };
        // above 32 is success
        let code = r.0 as isize;
        if code > 32 {
            log::info!("{what} opened");
        } else {
            log::warning!("{what} open failed ({code})");
        }
    });
}

/// Opens a PR page in the browser. Anything that is not a plain github.com URL is ignored: the
/// string came off the network and must never reach the shell otherwise.
pub fn open_url(url: &str) {
    if is_safe_url(url) {
        shell_open(url.to_owned(), "url");
    }
}
