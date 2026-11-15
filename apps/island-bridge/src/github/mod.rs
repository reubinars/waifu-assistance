//! Open PRs and review requests, polled through `gh` and printed as `{"github":...}` lines on change.

mod fetch;
mod parse;

use std::ffi::{OsStr, OsString};
use std::time::Duration;

use island_core::github::{PullRequest, github_line};

pub const POLL: Duration = Duration::from_secs(60);
pub const GH_TIMEOUT: Duration = Duration::from_secs(20);
pub const BACKOFF: Duration = Duration::from_secs(300);

pub const QUERY: &str = r#"query { mine: search(query: "is:pr is:open author:@me archived:false", type: ISSUE, first: 30) { nodes { ...pr } } review: search(query: "is:pr is:open review-requested:@me archived:false", type: ISSUE, first: 30) { nodes { ...pr } } } fragment pr on PullRequest { number title url isDraft updatedAt reviewDecision repository { nameWithOwner } commits(last: 1) { nodes { commit { statusCheckRollup { state } } } } }"#;

/// The `gh` to run: `ISLAND_GH` when set (a test hook), else `gh`.
pub fn program() -> OsString {
    std::env::var_os("ISLAND_GH").filter(|v| !v.is_empty()).unwrap_or_else(|| "gh".into())
}

pub fn snapshot(program: &OsStr, timeout: Duration) -> Result<Vec<PullRequest>, String> {
    fetch::fetch(program, timeout).and_then(|json| parse::parse_response(&json))
}

/// Remembers the last printed line so that only the first one and the changes get printed.
#[derive(Default)]
pub struct Poller {
    last: Option<String>,
}

impl Poller {
    pub fn tick(&mut self, r: &Result<Vec<PullRequest>, String>) -> Option<String> {
        let line = github_line(r);
        if self.last.as_ref() == Some(&line) {
            return None;
        }
        self.last = Some(line.clone());
        Some(line)
    }
}

/// Polls on its own thread for the life of the process.
pub fn spawn() {
    std::thread::spawn(|| {
        let mut poller = Poller::default();
        loop {
            let r = snapshot(&program(), GH_TIMEOUT);
            if let Some(line) = poller.tick(&r) {
                crate::emit(&line);
            }
            let limited = matches!(&r, Err(e) if e == "GitHub rate limited");
            std::thread::sleep(if limited { BACKOFF } else { POLL });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poller_prints_first_and_changes_only() {
        let mut p = Poller::default();
        assert!(p.tick(&Ok(vec![])).is_some());
        assert!(p.tick(&Ok(vec![])).is_none());
        assert!(p.tick(&Err("x".to_string())).is_some());
        assert!(p.tick(&Err("x".to_string())).is_none());
    }
}
