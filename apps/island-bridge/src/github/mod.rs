//! Open PRs and review requests, polled through `gh` and printed as `{"github":...}` lines on change.

mod fetch;
mod parse;

use std::ffi::{OsStr, OsString};
use std::time::Duration;

use island_core::github::{PullRequest, github_line};

pub const POLL: Duration = Duration::from_secs(60);
pub const GH_TIMEOUT: Duration = Duration::from_secs(20);
pub const BACKOFF: Duration = Duration::from_secs(300);

pub const QUERY: &str = r#"query { mine: search(query: "is:pr is:open author:@me archived:false sort:updated-desc", type: ISSUE, first: 30) { nodes { ...pr } } review: search(query: "is:pr is:open review-requested:@me archived:false sort:updated-desc", type: ISSUE, first: 30) { nodes { ...pr } } } fragment pr on PullRequest { number title url isDraft updatedAt reviewDecision repository { nameWithOwner } commits(last: 1) { nodes { commit { statusCheckRollup { state } } } } }"#;

/// The `gh` to run: `ISLAND_GH` when set (a test hook), else `gh`.
pub fn program() -> OsString {
    std::env::var_os("ISLAND_GH").filter(|v| !v.is_empty()).unwrap_or_else(|| "gh".into())
}

/// One poll. A failed `gh` that still printed usable data (a partial GraphQL result) counts as a
/// list, unless it was rate limited or either search came back null: a missing half would read as
/// every PR in it closing, and their events would fire again when it returns.
pub fn snapshot(program: &OsStr, timeout: Duration) -> Result<Vec<PullRequest>, String> {
    match fetch::fetch(program, timeout) {
        Ok(json) => parse::parse_response(&json),
        Err(f) if f.reason == fetch::RATE_LIMITED => Err(f.reason),
        Err(f) if !both_searches(&f.stdout) => Err(f.reason),
        Err(f) => parse::parse_response(&f.stdout).map_err(|_| f.reason),
    }
}

/// Whether `json` holds a non-null `data.mine` and `data.review`.
fn both_searches(json: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return false };
    ["mine", "review"].iter().all(|k| v.pointer(&format!("/data/{k}")).is_some_and(|s| s.is_object()))
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
            let limited = matches!(&r, Err(e) if e == fetch::RATE_LIMITED);
            std::thread::sleep(if limited { BACKOFF } else { POLL });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::fetch::tests::script;
    use super::*;

    const NODE: &str =
        r#"{"number":1,"title":"t","url":"https://github.com/a/b/pull/1","repository":{"nameWithOwner":"a/b"}}"#;

    fn snap(name: &str, body: &str) -> Result<Vec<PullRequest>, String> {
        snapshot(script(name, body).as_os_str(), Duration::from_secs(5))
    }

    #[test]
    fn partial_data_on_failure_is_used() {
        let json = format!(
            r#"{{"data":{{"mine":{{"nodes":[{NODE},null]}},"review":{{"nodes":[]}}}},"errors":[{{"message":"Something went wrong"}}]}}"#
        );
        let prs = snap("gh-partial", &format!("printf '%s' '{json}'; exit 1")).unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].number, 1);
    }

    #[test]
    fn failure_with_a_null_search_is_an_error() {
        let json = format!(
            r#"{{"data":{{"mine":{{"nodes":[{NODE}]}},"review":null}},"errors":[{{"message":"Something went wrong"}}]}}"#
        );
        assert_eq!(snap("gh-half", &format!("printf '%s' '{json}'; exit 1")), Err("GitHub unreachable".to_string()));
        let json = r#"{"data":{"mine":null,"review":null},"errors":[{"message":"Something went wrong"}]}"#;
        assert_eq!(snap("gh-none", &format!("printf '%s' '{json}'; exit 1")), Err("GitHub unreachable".to_string()));
    }

    #[test]
    fn rate_limit_on_failure_wins_over_data() {
        let json = format!(
            r#"{{"data":{{"mine":{{"nodes":[{NODE}]}},"review":{{"nodes":[]}}}},"errors":[{{"type":"RATE_LIMITED","message":"API rate limit exceeded"}}]}}"#
        );
        assert_eq!(
            snap("gh-partial-rate", &format!("printf '%s' '{json}'; exit 1")),
            Err("GitHub rate limited".to_string())
        );
    }

    #[test]
    fn garbage_on_success_is_unreachable() {
        assert_eq!(snap("gh-garbage", "echo garbage"), Err("GitHub unreachable".to_string()));
    }

    #[test]
    fn poller_prints_first_and_changes_only() {
        let mut p = Poller::default();
        assert!(p.tick(&Ok(vec![])).is_some());
        assert!(p.tick(&Ok(vec![])).is_none());
        assert!(p.tick(&Err("x".to_string())).is_some());
        assert!(p.tick(&Err("x".to_string())).is_none());
    }
}
