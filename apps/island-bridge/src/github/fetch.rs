//! Runs `gh api graphql` with a deadline.

use std::ffi::OsStr;
use std::io::{ErrorKind, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::QUERY;

fn drain(mut r: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
    });
    rx
}

pub const RATE_LIMITED: &str = "GitHub rate limited";

/// Why `gh` gave no output to use, and what it printed to stdout anyway (empty if it never ran
/// to the end).
#[derive(Debug, PartialEq)]
pub struct Failure {
    pub reason: String,
    pub stdout: String,
}

impl Failure {
    fn bare(reason: &str) -> Failure {
        Failure { reason: reason.to_string(), stdout: String::new() }
    }
}

pub fn fetch(program: &OsStr, timeout: Duration) -> Result<String, Failure> {
    let unreachable = || Failure::bare("GitHub unreachable");
    let mut child = Command::new(program)
        .args(["api", "graphql", "-f"])
        .arg(format!("query={QUERY}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| if e.kind() == ErrorKind::NotFound { Failure::bare("gh not installed") } else { unreachable() })?;
    let out = drain(child.stdout.take().ok_or_else(unreachable)?);
    let err = drain(child.stderr.take().ok_or_else(unreachable)?);

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            // Timed out or lost track of it: the readers end on their own once the pipes close.
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unreachable());
            }
        }
    };
    let grace = Duration::from_secs(1);
    let stdout = out.recv_timeout(grace).unwrap_or_default();
    let stderr = err.recv_timeout(grace).unwrap_or_default();
    if status.success() {
        return Ok(stdout);
    }
    let all = format!("{stdout}{stderr}").to_lowercase();
    // stdout carries PR titles and URLs, so only stderr can say we're logged out
    let stderr = stderr.to_lowercase();
    let reason = if all.contains("rate limit") {
        RATE_LIMITED
    } else if ["auth login", "bad credentials", "401", "not logged"].iter().any(|k| stderr.contains(k)) {
        "gh not logged in"
    } else {
        "GitHub unreachable"
    };
    Err(Failure { reason: reason.to_string(), stdout })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::github::QUERY;
    use crate::usage::test_support::{temp_home, write};
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    pub(in crate::github) fn script(name: &str, body: &str) -> std::path::PathBuf {
        let dir = temp_home(name);
        let path = write(&dir, "gh", &format!("#!/bin/sh\n{body}\n"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn reason(r: Result<String, Failure>) -> String {
        r.unwrap_err().reason
    }

    #[test]
    fn missing_program_is_not_installed() {
        let r = fetch(std::ffi::OsStr::new("/nonexistent/gh"), Duration::from_secs(1));
        assert_eq!(reason(r), "gh not installed");
    }

    #[test]
    fn logged_out_maps() {
        let p = script("gh-out", "echo 'gh: Bad credentials (HTTP 401)' >&2; exit 1");
        assert_eq!(reason(fetch(p.as_os_str(), Duration::from_secs(5))), "gh not logged in");
    }

    #[test]
    fn rate_limit_maps() {
        let p = script("gh-rate", "echo 'API rate limit exceeded' >&2; exit 1");
        assert_eq!(reason(fetch(p.as_os_str(), Duration::from_secs(5))), "GitHub rate limited");
    }

    #[test]
    fn hang_times_out() {
        let p = script("gh-hang", "exec sleep 5");
        let start = Instant::now();
        let r = fetch(p.as_os_str(), Duration::from_millis(300));
        assert_eq!(reason(r), "GitHub unreachable");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn success_passes_args() {
        let p = script("gh-args", r#"printf '%s\n' "$@""#);
        let out = fetch(p.as_os_str(), Duration::from_secs(5)).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines, ["api", "graphql", "-f", &format!("query={QUERY}")]);
        assert_eq!(QUERY.matches("archived:false sort:updated-desc\"").count(), 2);
    }

    #[test]
    fn auth_words_on_stdout_are_not_logged_out() {
        let json = r#"{"data":{"mine":{"nodes":[{"url":"https://github.com/a/b/pull/1401"}]}}}"#;
        let p = script("gh-401-out", &format!("printf '%s' '{json}'; echo 'something else' >&2; exit 1"));
        let f = fetch(p.as_os_str(), Duration::from_secs(5)).unwrap_err();
        assert_eq!(f.reason, "GitHub unreachable");
        assert_eq!(f.stdout, json);
    }
}
