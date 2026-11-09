//! Start with Windows: the `Island` value under HKCU `Software\Microsoft\Windows\CurrentVersion\Run`.

use crate::log;
use island_core::platform::{run_value, startup_change};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::{HSTRING, PCWSTR, Result, w};

const RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const NAME: PCWSTR = w!("Island");

fn exe() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()
}

fn check(e: WIN32_ERROR) -> Result<()> {
    if e == ERROR_SUCCESS { Ok(()) } else { Err(e.into()) }
}

/// Writes (`true`) or deletes (`false`) the Run value for the current exe.
pub fn set(enabled: bool) -> Result<()> {
    unsafe {
        if enabled {
            let data: Vec<u16> = run_value(&exe()).encode_utf16().chain([0]).collect();
            check(RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN,
                NAME,
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                (data.len() * 2) as u32,
            ))
        } else {
            match RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, NAME) {
                ERROR_FILE_NOT_FOUND => Ok(()),
                e => check(e),
            }
        }
    }
}

/// The current Run value, if there is one.
fn current() -> Option<String> {
    unsafe {
        let mut bytes = 0u32;
        if RegGetValueW(HKEY_CURRENT_USER, RUN, NAME, RRF_RT_REG_SZ, None, None, Some(&mut bytes)) != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
        if RegGetValueW(HKEY_CURRENT_USER, RUN, NAME, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut bytes))
            != ERROR_SUCCESS
        {
            return None;
        }
        let s = HSTRING::from_wide(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len())]);
        Some(s.to_string_lossy())
    }
}

/// Makes the Run value match `want` (and point at this exe).
pub fn sync(want: bool) {
    let exe = exe();
    if let Some(write) = startup_change(want, current().as_deref(), &exe) {
        match set(write) {
            Ok(()) => log::info!("start with Windows {}", if write { "set" } else { "removed" }),
            Err(e) => log::warning!("start with Windows: {e}"),
        }
    }
}
