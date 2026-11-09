//! Pure helpers for the Windows side: the WSL distro list, the bridge command line, fullscreen detection.

use crate::Rect;
use crate::config::Config;
use std::time::Duration;

/// Windows image paths use backslashes even when this policy is tested on Linux.
pub fn terminal_process_matches(configured: &str, image_path: &str) -> bool {
    !configured.is_empty()
        && image_path.rsplit(['\\', '/']).next().is_some_and(|name| name.eq_ignore_ascii_case(configured))
}

/// Child failures back off, while a stable bridge earns a fresh retry budget.
pub struct BridgeBackoff {
    next_seconds: u64,
}

impl Default for BridgeBackoff {
    fn default() -> Self { Self { next_seconds: 1 } }
}

impl BridgeBackoff {
    pub fn failed(&mut self) -> Duration {
        let delay = self.next_seconds;
        self.next_seconds = (delay * 2).min(30);
        Duration::from_secs(delay)
    }

    pub fn ran_for(&mut self, uptime: Duration) {
        if uptime >= Duration::from_secs(30) { *self = Self::default(); }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Distro {
    pub name: String,
    pub default: bool,
    pub running: bool,
}

/// Decodes `wsl.exe -l -v` output: UTF-16LE (BOM optional), or UTF-8 when `WSL_UTF8=1`.
fn decode(raw: &[u8]) -> String {
    let raw = raw.strip_prefix(&[0xFF, 0xFE]).unwrap_or(raw);
    if raw.contains(&0) {
        let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
        String::from_utf16_lossy(&units)
    } else {
        let raw = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(raw);
        String::from_utf8_lossy(raw).into_owned()
    }
}

/// Parses `wsl.exe -l -v`: a header line, then `[*] NAME STATE VERSION` rows.
pub fn parse_wsl_list_verbose(raw: &[u8]) -> Vec<Distro> {
    decode(raw)
        .lines()
        .skip(1)
        .filter_map(|line| {
            let line = line.trim_matches(|c: char| c.is_whitespace() || c == '\0');
            let (default, rest) = match line.strip_prefix('*') {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            let mut cols = rest.split_whitespace();
            let name = cols.next()?.to_owned();
            let state = cols.next()?;
            Some(Distro { name, default, running: state == "Running" })
        })
        .collect()
}

/// Whether the configured distro ("" = the default one) is running.
pub fn should_run_bridge(distros: &[Distro], configured: &str) -> bool {
    distros
        .iter()
        .find(|d| if configured.is_empty() { d.default } else { d.name == configured })
        .is_some_and(|d| d.running)
}

/// Arguments for `wsl.exe` that start the bridge; `$HOME` is left for the shell to expand.
pub fn bridge_command(cfg: &Config) -> Vec<String> {
    let path = cfg.bridge_path.replace('\\', "\\\\").replace('"', "\\\"").replace('`', "\\`");
    let mut args = Vec::new();
    if !cfg.wsl_distro.is_empty() {
        args.extend(["-d".to_owned(), cfg.wsl_distro.clone()]);
    }
    args.extend(["--".to_owned(), "sh".to_owned(), "-c".to_owned(), format!("exec \"{path}\"")]);
    args
}

/// A foreground window that covers its whole monitor and isn't the desktop, the taskbar or us.
pub fn is_fullscreen(fg: Rect, monitor: Rect, class: &str, is_own: bool) -> bool {
    if is_own || matches!(class, "Progman" | "WorkerW" | "Shell_TrayWnd") {
        return false;
    }
    fg.x <= monitor.x && fg.y <= monitor.y && fg.x + fg.w >= monitor.x + monitor.w && fg.y + fg.h >= monitor.y + monitor.h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_process_matches_only_the_image_filename_case_insensitively() {
        assert!(terminal_process_matches("warp.exe", r"C:\Users\Victus\AppData\Local\Warp\WaRp.ExE"));
        assert!(terminal_process_matches("WARP.EXE", "warp.exe"));
        assert!(!terminal_process_matches("warp.exe", r"C:\warp.exe\other.exe"));
        assert!(!terminal_process_matches("warp.exe", r"C:\app\notwarp.exe"));
        assert!(!terminal_process_matches("", ""));
    }

    fn utf16le(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn wsl_fixture() {
        let raw = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wsl_list_verbose.txt")).unwrap();
        let d = parse_wsl_list_verbose(&raw);
        assert_eq!(d.len(), 1);
        let u = d.iter().find(|d| d.name == "Ubuntu").unwrap();
        assert!(u.default && u.running);
        assert!(should_run_bridge(&d, ""));
    }

    #[test]
    fn wsl_states() {
        let raw = utf16le("  NAME      STATE           VERSION\r\n* Ubuntu    Stopped         2\r\n  Debian    Running         2\r\n");
        let d = parse_wsl_list_verbose(&raw);
        assert!(!should_run_bridge(&d, ""));
        assert!(should_run_bridge(&d, "Debian"));
        assert!(!should_run_bridge(&d, "Arch"));
        // WSL_UTF8=1 makes wsl.exe print UTF-8 instead
        let d = parse_wsl_list_verbose(b"  NAME      STATE           VERSION\n* Ubuntu    Running         2\n");
        assert!(should_run_bridge(&d, ""));
        assert!(parse_wsl_list_verbose(&[]).is_empty());
        assert!(!should_run_bridge(&[], ""));
    }

    #[test]
    fn bridge_cmd() {
        assert_eq!(bridge_command(&Config::default()), vec!["--", "sh", "-c", "exec \"$HOME/.local/bin/island-bridge\""]);
        assert_eq!(bridge_command(&Config { wsl_distro: "Ubuntu".into(), ..Config::default() })[..2], ["-d", "Ubuntu"]);
        let odd = Config { bridge_path: "/opt/a \"b\"/bridge".into(), ..Config::default() };
        assert_eq!(bridge_command(&odd)[3], "exec \"/opt/a \\\"b\\\"/bridge\"");
    }

    #[test]
    fn fullscreen() {
        let m = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 };
        assert!(is_fullscreen(m, m, "Chrome_WidgetWin_1", false));
        assert!(!is_fullscreen(m, m, "WorkerW", false));
        assert!(!is_fullscreen(m, m, "Progman", false));
        assert!(!is_fullscreen(m, m, "Shell_TrayWnd", false));
        assert!(!is_fullscreen(Rect { h: 1040.0, ..m }, m, "Chrome_WidgetWin_1", false));
        assert!(!is_fullscreen(m, m, "X", true));
        // larger than the monitor (borderless games often overhang) still counts
        assert!(is_fullscreen(Rect { x: -8.0, y: -8.0, w: 1936.0, h: 1096.0 }, m, "UnityWndClass", false));
    }

    #[test]
    fn bridge_restarts_back_off_and_reset_only_after_a_stable_run() {
        use std::time::Duration;
        let mut retry = BridgeBackoff::default();
        let seconds: Vec<_> = (0..8).map(|_| retry.failed().as_secs()).collect();
        assert_eq!(seconds, [1, 2, 4, 8, 16, 30, 30, 30]);
        retry.ran_for(Duration::from_secs(2));
        assert_eq!(retry.failed().as_secs(), 30);
        retry.ran_for(Duration::from_secs(30));
        assert_eq!(retry.failed().as_secs(), 1);
    }
}
