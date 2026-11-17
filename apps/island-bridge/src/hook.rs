//! `island-bridge hook <agent>`: reads an agent's permission-hook JSON on stdin, asks the
//! island through the bridge socket and prints the user's decision. Any failure prints
//! nothing and exits 0, which leaves the decision to the agent's own prompt.

use island_core::approvals::Decision;
use island_core::hooks::{self, Agent, HookInput};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

const HOOK_TIMEOUT: Duration = Duration::from_millis(45_000);
const CONNECT_BUDGET: Duration = Duration::from_millis(500);

#[cfg(debug_assertions)]
fn hook_timeout() -> Duration {
    std::env::var("ISLAND_HOOK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(HOOK_TIMEOUT, |ms: u64| Duration::from_millis(ms.max(1)))
}

#[cfg(not(debug_assertions))]
fn hook_timeout() -> Duration {
    HOOK_TIMEOUT
}

fn connect(path: PathBuf) -> Option<UnixStream> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(UnixStream::connect(path));
    });
    rx.recv_timeout(CONNECT_BUDGET).ok()?.ok()
}

/// Appends one line to hook.log unless this (agent, event, tool) already has one.
fn log_unparsed(agent: &str, input: &Value) {
    let field = |k: &str| {
        input.get(k).and_then(Value::as_str).map(|s| s.chars().filter(|c| !c.is_control()).take(60).collect::<String>())
    };
    let (event, tool) =
        (field("hook_event_name").unwrap_or_else(|| "?".into()), field("tool_name").unwrap_or_else(|| "?".into()));
    let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) else { return };
    let dir = PathBuf::from(home).join(".local/state/island");
    let log = dir.join("hook.log");
    let key = format!("{agent} - {event} {tool}: ");
    if std::fs::read_to_string(&log).is_ok_and(|s| s.lines().any(|l| l.starts_with(&key))) {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
        let _ = writeln!(f, "{key}could not parse hook input");
    }
}

pub fn run(agent_arg: &str) -> i32 {
    let _ = run_inner(agent_arg);
    0
}

fn run_inner(agent_arg: &str) -> Option<()> {
    let agent = Agent::from_arg(agent_arg)?;
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).ok()?;
    let input: Value = serde_json::from_str(&raw).ok()?;
    let Some(parsed) = hooks::parse(agent, &input) else {
        if !hooks::is_ignored(agent, &input) {
            log_unparsed(agent_arg, &input);
        }
        return None;
    };
    let pane_id = std::env::var("HERDR_PANE_ID").ok().filter(|p| !p.is_empty())?;
    let timeout = hook_timeout();
    let (msg, wants_answer) = match &parsed {
        HookInput::Approval(req) => (
            json!({"pane_id": pane_id, "kind": "approval", "request": req, "timeout_ms": HOOK_TIMEOUT.as_millis() as u64}),
            true,
        ),
        HookInput::Question(text) => {
            (json!({"pane_id": pane_id, "kind": "question", "agent": agent_arg, "text": text}), false)
        }
    };
    let mut stream = connect(crate::socket::socket_path())?;
    stream.write_all(format!("{msg}\n").as_bytes()).ok()?;
    stream.flush().ok()?;
    if !wants_answer {
        return None;
    }
    stream.set_read_timeout(Some(timeout)).ok()?;
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).ok()? == 0 {
        return None;
    }
    let v: Value = serde_json::from_str(&line).ok()?;
    let decision: Decision = serde_json::from_value(v.get("decision")?.clone()).ok()?;
    let out = hooks::output(agent, decision, &input)?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{out}").ok()?;
    stdout.flush().ok()
}
