//! Runs the real bridge and hook binaries: the bridge is the island's end of the socket.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_island-bridge");
const WAIT: Duration = Duration::from_secs(5);

fn fixture(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/island-core/tests/fixtures/hooks").join(name);
    std::fs::read(p).unwrap()
}

struct Bridge {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    dir: PathBuf,
}

impl Bridge {
    fn spawn(name: &str) -> Bridge {
        let dir = std::env::temp_dir().join(format!("iba-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Bridge::spawn_in(dir)
    }

    fn spawn_in(dir: PathBuf) -> Bridge {
        let mut child = Command::new(BIN)
            .env("XDG_RUNTIME_DIR", &dir)
            .env("HOME", &dir)
            .env("HERDR_SOCKET_PATH", dir.join("none.sock"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        let b = Bridge { child, stdin, lines, dir };
        let deadline = Instant::now() + WAIT;
        while !b.sock().exists() {
            assert!(Instant::now() < deadline, "socket never appeared");
            std::thread::sleep(Duration::from_millis(10));
        }
        b
    }

    fn sock(&self) -> PathBuf {
        self.dir.join("island.sock")
    }

    /// The next stdout line containing `key` (a top-level JSON key), as JSON.
    fn next_with(&self, key: &str) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self.lines.recv_timeout(left).unwrap_or_else(|_| panic!("no `{key}` line within 5 s"));
            let v: Value = serde_json::from_str(&line).unwrap();
            if v.get(key).is_some() {
                return v;
            }
        }
    }

    fn send(&mut self, line: &str) {
        let s = self.stdin.as_mut().unwrap();
        writeln!(s, "{line}").unwrap();
        s.flush().unwrap();
    }

    fn hook(&self, agent: &str, fixture_name: &str, pane: Option<&str>) -> Child {
        let mut cmd = Command::new(BIN);
        cmd.args(["hook", agent])
            .env("XDG_RUNTIME_DIR", &self.dir)
            .env("HOME", &self.dir)
            .env_remove("HERDR_PANE_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        if let Some(p) = pane {
            cmd.env("HERDR_PANE_ID", p);
        }
        spawn_hook(cmd, fixture_name)
    }
}

fn spawn_hook(mut cmd: Command, fixture_name: &str) -> Child {
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(&fixture(fixture_name)).unwrap();
    child
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Waits for the child to exit within the deadline and returns its stdout.
fn finish(mut child: Child) -> (i32, String) {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(st) = child.try_wait().unwrap() {
            let mut out = String::new();
            child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
            return (st.code().unwrap_or(-1), out);
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("child did not exit within 5 s");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn round_trip(decision: &str, expect: &str) {
    let mut b = Bridge::spawn(decision);
    let hook = b.hook("claude", "claude_edit.json", Some("p1"));
    let a = b.next_with("approval")["approval"].clone();
    let id = a["id"].as_str().unwrap().to_string();
    assert_eq!(a["pane_id"], "p1");
    assert_eq!(a["can_always"], true);
    assert_eq!(a["timeout_ms"], 45000);
    b.send(&json!({"decide": {"id": id, "decision": decision}}).to_string());
    let (code, out) = finish(hook);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["hookSpecificOutput"]["decision"]["behavior"], expect, "{out}");
    let closed = b.next_with("approval_closed");
    assert_eq!(closed["approval_closed"]["id"], id.as_str());
}

#[test]
fn allow_round_trip() {
    round_trip("allow", "allow");
}

#[test]
fn deny_and_always_round_trip() {
    round_trip("deny", "deny");
    round_trip("always", "allow");
}

#[test]
fn hook_without_pane_or_socket_is_silent() {
    let b = Bridge::spawn("silent");
    let t = Instant::now();
    let (code, out) = finish(b.hook("claude", "claude_edit.json", None));
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(t.elapsed() < Duration::from_secs(1));

    let dir = std::env::temp_dir().join(format!("iba-{}-nosock", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut cmd = Command::new(BIN);
    cmd.args(["hook", "claude"])
        .env("XDG_RUNTIME_DIR", &dir)
        .env("HOME", &dir)
        .env("HERDR_PANE_ID", "p1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let t = Instant::now();
    let (code, out) = finish(spawn_hook(cmd, "claude_edit.json"));
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(t.elapsed() < Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn island_exit_releases_the_hook() {
    let mut b = Bridge::spawn("exit");
    let hook = b.hook("claude", "claude_edit.json", Some("p1"));
    b.next_with("approval");
    b.stdin = None; // closes the bridge's stdin
    let (code, out) = finish(hook);
    assert_eq!((code, out.as_str()), (0, ""));
}

#[test]
fn hook_killed_closes_the_card() {
    let b = Bridge::spawn("kill");
    let mut hook = b.hook("claude", "claude_edit.json", Some("p1"));
    let id = b.next_with("approval")["approval"]["id"].clone();
    hook.kill().unwrap();
    hook.wait().unwrap();
    assert_eq!(b.next_with("approval_closed")["approval_closed"]["id"], id);
}

#[test]
fn stale_decision_is_ignored() {
    let mut b = Bridge::spawn("stale");
    b.send(r#"{"decide":{"id":"a999","decision":"allow"}}"#);
    // The bridge is still up and still serving: an approval after the stale decide works.
    let hook = b.hook("claude", "claude_edit.json", Some("p1"));
    let id = b.next_with("approval")["approval"]["id"].clone();
    assert_ne!(id, "a999");
    b.send(&json!({"decide": {"id": id, "decision": "deny"}}).to_string());
    let (code, out) = finish(hook);
    assert_eq!(code, 0);
    assert!(out.contains("deny"));
}

#[test]
fn socket_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let b = Bridge::spawn("mode");
    assert_eq!(std::fs::metadata(b.sock()).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn stale_socket_file_is_replaced() {
    use std::os::unix::net::UnixListener;
    let dir = std::env::temp_dir().join(format!("iba-{}-stale-file", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    drop(UnixListener::bind(dir.join("island.sock")).unwrap()); // leaves a dead socket file
    let mut child = Command::new(BIN)
        .env("XDG_RUNTIME_DIR", &dir)
        .env("HOME", &dir)
        .env("HERDR_SOCKET_PATH", dir.join("none.sock"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + WAIT;
    while std::os::unix::net::UnixStream::connect(dir.join("island.sock")).is_err() {
        assert!(Instant::now() < deadline, "bridge never served the replaced socket");
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn question_is_fire_and_forget() {
    let b = Bridge::spawn("question");
    let (code, out) = finish(b.hook("claude", "claude_question.json", Some("p1")));
    assert_eq!((code, out.as_str()), (0, ""));
    let q = b.next_with("question")["question"].clone();
    assert_eq!(q["pane_id"], "p1");
    assert_eq!(q["agent"], "claude");
    assert!(q["text"].as_str().unwrap().contains("Which color"));
}

#[test]
fn herdr_lines_still_pass_through() {
    use std::os::unix::net::UnixListener;
    let dir = std::env::temp_dir().join(format!("iba-{}-pass", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let herdr = dir.join("herdr.sock");
    let listener = UnixListener::bind(&herdr).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut l = String::new();
                if BufReader::new(stream).read_line(&mut l).unwrap_or(0) > 0 {
                    let _ = tx.send(l.trim_end().to_string());
                }
            });
        }
    });
    let mut child = Command::new(BIN)
        .env("XDG_RUNTIME_DIR", &dir)
        .env("HOME", &dir)
        .env("HERDR_SOCKET_PATH", &herdr)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let list = r#"{"id":"x","method":"agent.list","params":{}}"#;
    writeln!(stdin, "{list}").unwrap();
    writeln!(stdin, r#"{{"decide":{{"id":"a1","decision":"allow"}}}}"#).unwrap();
    stdin.flush().unwrap();
    assert_eq!(rx.recv_timeout(WAIT).unwrap(), list);
    assert!(rx.recv_timeout(Duration::from_millis(500)).is_err(), "decide line reached herdr");
    assert!(child.try_wait().unwrap().is_none());
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn newest_bridge_takes_the_socket() {
    use std::os::unix::fs::MetadataExt;
    let a = Bridge::spawn("newest");
    let ino = std::fs::metadata(a.sock()).unwrap().ino();
    let b = Bridge::spawn_in(a.dir.clone());
    let deadline = Instant::now() + WAIT;
    while std::fs::metadata(b.sock()).unwrap().ino() == ino {
        assert!(Instant::now() < deadline, "second bridge never took the path");
        std::thread::sleep(Duration::from_millis(10));
    }
    let hook = b.hook("claude", "claude_edit.json", Some("p1"));
    b.next_with("approval");
    drop(hook);
}

#[test]
fn socket_file_without_listener_is_silent() {
    let dir = std::env::temp_dir().join(format!("iba-{}-dead", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    drop(std::os::unix::net::UnixListener::bind(dir.join("island.sock")).unwrap());
    let mut cmd = Command::new(BIN);
    cmd.args(["hook", "claude"])
        .env("XDG_RUNTIME_DIR", &dir)
        .env("HOME", &dir)
        .env("HERDR_PANE_ID", "p1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let t = Instant::now();
    let (code, out) = finish(spawn_hook(cmd, "claude_edit.json"));
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(t.elapsed() < Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unparsed_input_logs_once_and_exits_quietly() {
    let b = Bridge::spawn("log");
    // the intentional ignore (AskUserQuestion's permission event) is not logged
    let (code, out) = finish(b.hook("claude", "claude_question_permission.json", Some("p1")));
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(!b.dir.join(".local/state/island/hook.log").exists());
    for _ in 0..2 {
        let (code, out) = finish(b.hook("claude", "claude_other_event.json", Some("p1")));
        assert_eq!((code, out.as_str()), (0, ""));
    }
    let log = std::fs::read_to_string(b.dir.join(".local/state/island/hook.log")).unwrap_or_default();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 1, "{log}");
    assert!(lines[0].starts_with("claude - "), "{log}");
}

#[test]
fn hook_timeout_exits_silently_and_closes_the_card() {
    let b = Bridge::spawn("timeout");
    let mut cmd = Command::new(BIN);
    cmd.args(["hook", "claude"])
        .env("XDG_RUNTIME_DIR", &b.dir)
        .env("HOME", &b.dir)
        .env("HERDR_PANE_ID", "p1")
        .env("ISLAND_HOOK_TIMEOUT_MS", "300")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let hook = spawn_hook(cmd, "claude_edit.json");
    let id = b.next_with("approval")["approval"]["id"].clone();
    let (code, out) = finish(hook);
    assert_eq!((code, out.as_str()), (0, ""));
    assert_eq!(b.next_with("approval_closed")["approval_closed"]["id"], id);
}
