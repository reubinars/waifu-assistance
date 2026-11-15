// Checked 2026-11-15 with Claude Code 2.1.325: Claude shows its own permission dialog while a PermissionRequest hook runs (it does not wait for it) and applies the hook's decision whenever it arrives, closing the open dialog; PreToolUse hooks block the tool until they return.

mod claude;

use crate::approvals::{ApprovalRequest, Decision};
use serde_json::Value;
use std::path::Path;

/// Longest text shown from an agent's input, ellipsis included.
const MAX_CHARS: usize = 300;
/// Longest tool name and file path shown, ellipsis included.
const MAX_TOOL_CHARS: usize = 40;
const MAX_PATH_CHARS: usize = 200;

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

/// Whether `parse` returns None on purpose for this input, so it isn't worth logging as unparsed.
pub fn is_ignored(agent: Agent, input: &Value) -> bool {
    match agent {
        Agent::Claude => claude::is_ignored(input),
        Agent::Codex | Agent::OpenCode => false,
    }
}

/// The text a hook prints for the user's decision. Codex and OpenCode adapters come later.
pub fn output(agent: Agent, decision: Decision, input: &Value) -> Option<String> {
    match agent {
        Agent::Claude => claude::output(decision, input),
        Agent::Codex | Agent::OpenCode => None,
    }
}

/// `path` relative to `cwd` when it lies inside it, otherwise unchanged. Sanitized for display.
fn rel(path: &str, cwd: Option<&str>) -> String {
    let shown = cwd
        .and_then(|c| Path::new(path).strip_prefix(c).ok())
        .map(|r| if r.as_os_str().is_empty() { ".".to_owned() } else { r.to_string_lossy().into_owned() })
        .unwrap_or_else(|| path.to_owned());
    cap_to(&sanitize(&shown, false), MAX_PATH_CHARS)
}

/// Replaces control characters, and bidi and format characters that can disguise a name, with '?'.
/// With `command` set (text shown as a command) newlines survive, a tab becomes a space and a CR is dropped.
fn sanitize(s: &str, command: bool) -> String {
    s.chars()
        .filter(|&c| !(command && c == '\r'))
        .map(|c| {
            if command && c == '\t' {
                return ' ';
            }
            let control = c.is_control() && !(command && c == '\n');
            let format = matches!(
                c,
                '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
            );
            if control || format { '?' } else { c }
        })
        .collect()
}

/// Newline-terminated lines: a trailing partial line counts too, empty text is 0.
fn lines(s: &str) -> u32 {
    let n = s.matches('\n').count();
    (if s.is_empty() || s.ends_with('\n') { n } else { n + 1 }) as u32
}

/// Keeps the first `MAX_CHARS` characters, ending with an ellipsis when cut. Cuts on a char boundary.
fn cap(s: &str) -> String {
    cap_to(s, MAX_CHARS)
}

/// Keeps the first `max` characters, ending with an ellipsis when cut.
fn cap_to(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// The first two lines, capped at `MAX_CHARS`, with an ellipsis whenever anything was cut.
fn trim_command(s: &str) -> String {
    let s = sanitize(s, true);
    let mut it = s.lines();
    let first: Vec<&str> = it.by_ref().take(2).collect();
    let cut_lines = it.next().is_some();
    let mut out = first.join("\n");
    if cut_lines {
        out.push('…');
    }
    cap(&out)
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

    #[test]
    fn path_equal_to_cwd_is_dot() {
        assert_eq!(rel("/home/u/p", Some("/home/u/p")), ".");
    }

    #[test]
    fn sanitize_replaces_controls_and_format_chars() {
        assert_eq!(sanitize("evil\u{202E}hs.sh", false), "evil?hs.sh");
        assert_eq!(sanitize("a\rb", false), "a?b");
        assert_eq!(sanitize("a\nb", false), "a?b");
        assert_eq!(sanitize("a\nb", true), "a\nb");
        // as a command, a tab is a space and a CR vanishes
        assert_eq!(sanitize("a\tb\r\nc", true), "a b\nc");
        assert_eq!(sanitize("a\tb", false), "a?b");
        assert_eq!(sanitize("x\u{200B}\u{2066}\u{FEFF}é", false), "x???é");
    }

    #[test]
    fn long_path_is_capped() {
        let p = rel(&format!("/{}", "p".repeat(500)), None);
        assert_eq!(p.chars().count(), MAX_PATH_CHARS);
        assert!(p.ends_with('…'));
    }

    #[test]
    fn huge_single_line_is_capped() {
        let cmd = "y".repeat(100_000);
        let t = trim_command(&cmd);
        assert_eq!(t.chars().count(), MAX_CHARS);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn multibyte_cut_lands_on_a_char_boundary() {
        let t = trim_command(&"é".repeat(400));
        assert_eq!(t.chars().count(), MAX_CHARS);
        assert_eq!(t, format!("{}…", "é".repeat(MAX_CHARS - 1)));
    }

    #[test]
    fn short_text_is_untouched() {
        assert_eq!(trim_command("ls -la\necho hi"), "ls -la\necho hi");
        assert_eq!(trim_command("ls\n"), "ls");
    }
}
