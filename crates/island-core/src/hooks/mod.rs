// Checked 2026-11-15 with Claude Code 2.1.325: Claude shows its own permission dialog while a PermissionRequest hook runs (it does not wait for it) and applies the hook's decision whenever it arrives, closing the open dialog; PreToolUse hooks block the tool until they return.

mod claude;

use crate::approvals::{ApprovalRequest, Decision};
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    OpenCode,
}

impl Agent {
    pub fn from_arg(s: &str) -> Option<Agent> {
        match s {
            "claude" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            "opencode" => Some(Agent::OpenCode),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HookInput {
    Approval(ApprovalRequest),
    Question(String),
}

/// Reads one hook's stdin JSON. Codex and OpenCode adapters come later.
pub fn parse(agent: Agent, input: &Value) -> Option<HookInput> {
    match agent {
        Agent::Claude => claude::parse(input),
        Agent::Codex | Agent::OpenCode => None,
    }
}

/// The text a hook prints for the user's decision. Codex and OpenCode adapters come later.
pub fn output(agent: Agent, decision: Decision, input: &Value) -> Option<String> {
    match agent {
        Agent::Claude => claude::output(decision, input),
        Agent::Codex | Agent::OpenCode => None,
    }
}

/// `path` relative to `cwd` when it lies inside it, otherwise unchanged.
fn rel(path: &str, cwd: Option<&str>) -> String {
    cwd.and_then(|c| Path::new(path).strip_prefix(c).ok())
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

/// Newline-terminated lines: a trailing partial line counts too, empty text is 0.
fn lines(s: &str) -> u32 {
    let n = s.matches('\n').count();
    (if s.is_empty() || s.ends_with('\n') { n } else { n + 1 }) as u32
}

/// The first two lines, with an ellipsis after them if the command was cut.
fn trim_command(s: &str) -> String {
    let mut it = s.lines();
    let first: Vec<&str> = it.by_ref().take(2).collect();
    let cut = it.next().is_some();
    let mut out = first.join("\n");
    if cut {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_write_counts_lines_and_trims_command() {
        let big = "x\n".repeat(20_000);
        assert_eq!(lines(&big), 20_000);
        let cmd = (0..300).map(|i| format!("echo {i}")).collect::<Vec<_>>().join("\n");
        assert_eq!(trim_command(&cmd).lines().count(), 2);
        assert!(trim_command(&cmd).ends_with('…'));
    }

    #[test]
    fn paths_outside_cwd_stay_absolute() {
        assert_eq!(rel("/home/u/p/src/a.rs", Some("/home/u/p")), "src/a.rs");
        assert_eq!(rel("/etc/hosts", Some("/home/u/p")), "/etc/hosts");
        assert_eq!(rel("/home/u/p/a.rs", None), "/home/u/p/a.rs");
        assert_eq!(rel("/home/u/pq/a.rs", Some("/home/u/p")), "/home/u/pq/a.rs"); // prefix is not a parent
    }
}
