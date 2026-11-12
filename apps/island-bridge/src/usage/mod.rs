//! Plan-limit usage for the Claude and Codex CLIs, polled every `POLL` and printed as `{"usage":[...]}` lines.

mod claude;
mod codex;

use std::path::{Path, PathBuf};
use std::time::Duration;

use island_core::usage::{CliUsage, usage_line};

/// How often the limits are read.
pub const POLL: Duration = Duration::from_secs(30);

/// Both CLIs' limits that could be read, Claude first. Empty when there is no home directory.
pub fn snapshot(home: Option<&Path>) -> Vec<CliUsage> {
    let Some(home) = home else { return Vec::new() };
    [claude::read(home), codex::read(home)].into_iter().flatten().collect()
}

/// Remembers the last snapshot so that only the first one and the changes get printed.
#[derive(Default)]
pub struct Poller {
    last: Option<Vec<CliUsage>>,
}

impl Poller {
    pub fn tick(&mut self, snap: Vec<CliUsage>) -> Option<String> {
        if self.last.as_ref() == Some(&snap) {
            return None;
        }
        let line = usage_line(&snap);
        self.last = Some(snap);
        Some(line)
    }
}

/// Polls on its own thread for the life of the process.
pub fn spawn(home: Option<PathBuf>) {
    std::thread::spawn(move || {
        let mut poller = Poller::default();
        loop {
            if let Some(line) = poller.tick(snapshot(home.as_deref())) {
                crate::emit(&line);
            }
            std::thread::sleep(POLL);
        }
    });
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, UNIX_EPOCH};

    /// A fresh, empty directory unique to this process and `name`.
    pub fn temp_home(name: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("ib-usage-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    /// Writes `content` to `home/rel`, creating parent directories.
    pub fn write(home: &Path, rel: &str, content: &str) -> PathBuf {
        let path = home.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    /// Sets the file's modification time to `secs` after the Unix epoch.
    pub fn set_mtime(path: &Path, secs: u64) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::test_support::{temp_home, write};
    use island_core::usage::CliUsage;

    #[test]
    fn poller_prints_first_and_on_change_only() {
        let mut p = Poller::default();
        assert_eq!(p.tick(vec![]).as_deref(), Some(r#"{"usage":[]}"#));
        assert_eq!(p.tick(vec![]), None);
        let snap = vec![CliUsage { cli: "codex".into(), windows: vec![] }];
        assert!(p.tick(snap.clone()).is_some());
        assert_eq!(p.tick(snap), None);
    }

    #[test]
    fn no_home_is_empty() {
        assert!(snapshot(None).is_empty());
    }

    #[test]
    fn order_is_claude_then_codex() {
        let home = temp_home("order");
        write(&home, ".cache/island/claude-limits.json", r#"{"five_hour":{"used_percentage":1,"resets_at":null}}"#);
        write(
            &home,
            ".codex/sessions/2026/11/11/rollout-a.jsonl",
            r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":2,"window_minutes":300,"resets_at":null}}}}"#,
        );
        let clis: Vec<String> = snapshot(Some(&home)).into_iter().map(|c| c.cli).collect();
        assert_eq!(clis, vec!["claude", "codex"]);
    }
}
