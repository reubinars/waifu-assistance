//! `%LOCALAPPDATA%\island\island.log`: `[{unix_ms}] {level} {msg}` lines, rotated at 1 MB, two files kept.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 1024 * 1024;

static PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
static DEBUG: AtomicBool = AtomicBool::new(false);

/// `%LOCALAPPDATA%\island`.
pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("island")
}

pub fn init(dir: &Path, debug: bool) {
    let _ = fs::create_dir_all(dir);
    DEBUG.store(debug, Ordering::Relaxed);
    if let Ok(mut p) = PATH.lock() {
        *p = Some(dir.join("island.log"));
    }
}

pub fn debug_enabled() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

pub fn write(level: &str, msg: &str) {
    let Ok(guard) = PATH.lock() else { return };
    let Some(path) = guard.as_ref() else { return };
    if fs::metadata(path).is_ok_and(|m| m.len() >= MAX_BYTES) {
        let _ = fs::rename(path, path.with_extension("log.1"));
    }
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{ms}] {level} {msg}");
    }
}

macro_rules! info { ($($t:tt)*) => { $crate::log::write("INFO", &format!($($t)*)) } }
macro_rules! warning { ($($t:tt)*) => { $crate::log::write("WARN", &format!($($t)*)) } }
macro_rules! error { ($($t:tt)*) => { $crate::log::write("ERROR", &format!($($t)*)) } }
macro_rules! debug {
    ($($t:tt)*) => { if $crate::log::debug_enabled() { $crate::log::write("DEBUG", &format!($($t)*)) } }
}
#[allow(unused_imports)]
pub(crate) use {debug, error, info, warning};
