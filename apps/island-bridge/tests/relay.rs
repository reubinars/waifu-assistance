//! Runs the real bridge binary against a fake herdr that behaves like herdr 0.9.5:
//! one request per connection; a subscribed connection only streams events.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const CONNECTED: &str = r#"{"bridge":"connected"}"#;
const DISCONNECTED: &str = r#"{"bridge":"disconnected"}"#;
const LIST: &str = r#"{"id":"island:1","method":"agent.list","params":{}}"#;
const SUB: &str =
    r#"{"id":"island:2","method":"events.subscribe","params":{"subscriptions":[{"type":"pane.closed"}]}}"#;
const SUB2: &str =
    r#"{"id":"island:3","method":"events.subscribe","params":{"subscriptions":[{"type":"pane.agent_detected"}]}}"#;

fn sock_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ib-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("h.sock")
}

/// A connection the bridge opened that carried a line (empty probe connections are skipped).
struct Conn {
    line: String,
    stream: UnixStream,
}

impl Conn {
    fn reply(&mut self, line: &str) {
        self.stream.write_all(format!("{line}\n").as_bytes()).unwrap();
    }

    /// True once the bridge has closed its end.
    fn closed_within(&mut self, secs: u64) -> bool {
        self.stream.set_read_timeout(Some(Duration::from_secs(secs))).unwrap();
        let mut buf = [0u8; 64];
        matches!(self.stream.read(&mut buf), Ok(0))
    }
}

struct Herdr {
    conns: Receiver<Conn>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Herdr {
    fn listen(sock: &PathBuf) -> Herdr {
        let _ = std::fs::remove_file(sock);
        let listener = UnixListener::bind(sock).unwrap();
        listener.set_nonblocking(true).unwrap();
        let (tx, conns) = mpsc::channel();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        std::thread::spawn(move || {
            while !stop2.load(std::sync::atomic::Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let tx = tx.clone();
                        std::thread::spawn(move || {
                            stream.set_nonblocking(false).unwrap();
                            let mut line = String::new();
                            let mut reader = BufReader::new(stream.try_clone().unwrap());
                            if reader.read_line(&mut line).unwrap_or(0) > 0 {
                                let line = line.trim_end().to_string();
                                let _ = tx.send(Conn { line, stream });
                            }
                        });
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Herdr { conns, stop }
    }

    fn next(&self) -> Conn {
        self.conns.recv_timeout(Duration::from_secs(3)).expect("bridge opened no connection with a line")
    }

    /// Stops accepting and removes the socket, like `herdr server stop`.
    fn stop(self, sock: &PathBuf) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(20));
        let _ = std::fs::remove_file(sock);
    }
}

struct Bridge {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    usage: Receiver<String>,
    hooks: Receiver<String>,
}

impl Bridge {
    fn spawn(sock: &PathBuf, home: &PathBuf) -> Bridge {
        let mut child = Command::new(env!("CARGO_BIN_EXE_island-bridge"))
            .env("HERDR_SOCKET_PATH", sock)
            .env("HOME", home)
            .env("XDG_RUNTIME_DIR", home)
            .env("ISLAND_GH", "/nonexistent/gh")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        let (utx, usage) = mpsc::channel();
        let (htx, hooks) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let sent = if line.starts_with(r#"{"usage""#) {
                    utx.send(line).is_ok()
                } else if line.starts_with(r#"{"hooks""#) {
                    htx.send(line).is_ok()
                } else if line.starts_with(r#"{"github""#) {
                    true // the GitHub poller's lines are not what these tests are about
                } else {
                    tx.send(line).is_ok()
                };
                if !sent {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        Bridge { child, stdin, lines, usage, hooks }
    }

    fn next(&self, secs: u64) -> String {
        self.lines.recv_timeout(Duration::from_secs(secs)).expect("no line from bridge in time")
    }

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(format!("{line}\n").as_bytes()).unwrap();
        stdin.flush().unwrap();
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// An empty HOME next to the socket, so the usage poller finds no CLI limits.
fn home_for(sock: &PathBuf) -> PathBuf {
    let home = sock.parent().unwrap().join("home");
    std::fs::create_dir_all(&home).unwrap();
    home
}

fn connected(name: &str) -> (PathBuf, Herdr, Bridge) {
    let sock = sock_path(name);
    let herdr = Herdr::listen(&sock);
    let bridge = Bridge::spawn(&sock, &home_for(&sock));
    assert_eq!(bridge.next(2), CONNECTED);
    (sock, herdr, bridge)
}

#[test]
fn each_request_gets_its_own_connection() {
    let (_sock, herdr, mut b) = connected("requests");
    b.send(LIST);
    b.send(r#"{"id":"island:9","method":"agent.get","params":{"target":"w1:p1"}}"#);
    let mut first = herdr.next();
    let mut second = herdr.next();
    let mut got = [first.line.clone(), second.line.clone()];
    got.sort();
    assert_eq!(got[0], LIST);
    assert!(got[1].contains("agent.get"));
    first.reply(r#"{"id":"a","result":{}}"#);
    second.reply(r#"{"id":"b","result":{}}"#);
    let mut out = vec![b.next(2), b.next(2)];
    out.sort();
    assert_eq!(out, vec![r#"{"id":"a","result":{}}"#, r#"{"id":"b","result":{}}"#]);
    assert!(first.closed_within(2), "bridge kept a request connection open");
}

#[test]
fn subscription_streams_events() {
    let (_sock, herdr, mut b) = connected("stream");
    b.send(SUB);
    let mut sub = herdr.next();
    assert_eq!(sub.line, SUB);
    sub.reply(r#"{"id":"island:2","result":{"type":"subscription_started"}}"#);
    sub.reply(r#"{"data":{"pane_id":"w1:p1"},"event":"pane_closed"}"#);
    assert_eq!(b.next(2), r#"{"id":"island:2","result":{"type":"subscription_started"}}"#);
    assert_eq!(b.next(2), r#"{"data":{"pane_id":"w1:p1"},"event":"pane_closed"}"#);
}

#[test]
fn new_subscription_replaces_old() {
    let (_sock, herdr, mut b) = connected("replace");
    b.send(SUB);
    let mut old = herdr.next();
    b.send(SUB2);
    let mut new = herdr.next();
    assert_eq!(new.line, SUB2);
    // the old one keeps streaming until herdr has started the new one, so no event is lost
    assert!(!old.closed_within(1), "old subscription closed before the new one started");
    new.reply(r#"{"id":"island:3","result":{"type":"subscription_started"}}"#);
    assert_eq!(b.next(2), r#"{"id":"island:3","result":{"type":"subscription_started"}}"#);
    assert!(old.closed_within(2), "old subscription not closed");
    new.reply(r#"{"event":"x"}"#);
    assert_eq!(b.next(2), r#"{"event":"x"}"#);
    // replacing must not look like herdr going away
    assert!(b.lines.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn waits_for_missing_socket() {
    let sock = sock_path("missing");
    let b = Bridge::spawn(&sock, &home_for(&sock));
    assert_eq!(b.next(2), DISCONNECTED);
    // an empty HOME still gets one usage line, so the island can clear a stale strip
    let usage = b.usage.recv_timeout(Duration::from_secs(2)).expect("no usage line");
    assert_eq!(usage, r#"{"usage":[]}"#);
    let _herdr = Herdr::listen(&sock);
    assert_eq!(b.next(3), CONNECTED);
}

#[test]
fn reports_herdr_stopping_and_returning() {
    let (sock, herdr, mut b) = connected("restart");
    b.send(SUB);
    let sub = herdr.next();
    herdr.stop(&sock);
    drop(sub);
    assert_eq!(b.next(2), DISCONNECTED);
    let _herdr = Herdr::listen(&sock);
    assert_eq!(b.next(3), CONNECTED);
}

#[test]
fn request_to_dead_herdr_reports_disconnected() {
    let (sock, herdr, mut b) = connected("deadreq");
    herdr.stop(&sock);
    b.send(LIST);
    assert_eq!(b.next(2), DISCONNECTED);
}

#[test]
fn usage_is_printed_without_herdr() {
    let sock = sock_path("usage"); // no listener: herdr is down
    let home = sock.parent().unwrap().join("home");
    std::fs::create_dir_all(home.join(".cache/island")).unwrap();
    std::fs::write(
        home.join(".cache/island/claude-limits.json"),
        r#"{"five_hour":{"used_percentage":42,"resets_at":null}}"#,
    )
    .unwrap();
    let b = Bridge::spawn(&sock, &home);
    let line = b.usage.recv_timeout(Duration::from_secs(2)).expect("no usage line");
    assert_eq!(
        line,
        r#"{"usage":[{"cli":"claude","windows":[{"minutes":300,"used_percent":42.0,"resets_at":null}]}]}"#
    );
}

#[test]
fn hook_status_is_reported_on_start() {
    // no herdr and an empty HOME: still one status line, with no CLI keys
    let sock = sock_path("hooks");
    let b = Bridge::spawn(&sock, &home_for(&sock));
    let line = b.hooks.recv_timeout(Duration::from_secs(2)).expect("no hooks line");
    assert_eq!(line, r#"{"hooks":{}}"#);
}

#[test]
fn hook_status_is_re_emitted_when_it_changes() {
    let sock = sock_path("hooks-change");
    let home = home_for(&sock);
    let mut child = Command::new(env!("CARGO_BIN_EXE_island-bridge"))
        .env("HERDR_SOCKET_PATH", &sock)
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &home)
        .env("ISLAND_GH", "/nonexistent/gh")
        .env("ISLAND_HOOK_STATUS_MS", "50")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if l.starts_with(r#"{"hooks""#) && tx.send(l).is_err() {
                break;
            }
        }
    });
    let next_hooks = || rx.recv_timeout(Duration::from_secs(3)).expect("no hooks line");
    assert_eq!(next_hooks(), r#"{"hooks":{}}"#);
    // a CLI appears after start: the island hears about it
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    assert_eq!(next_hooks(), r#"{"hooks":{"claude":false}}"#);
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn github_reports_gh_not_installed() {
    let sock = sock_path("gh-missing");
    let home = home_for(&sock);
    let mut child = Command::new(env!("CARGO_BIN_EXE_island-bridge"))
        .env("HERDR_SOCKET_PATH", &sock)
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &home)
        .env("ISLAND_GH", "/nonexistent/gh")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if l.starts_with(r#"{"github""#) && tx.send(l).is_err() {
                break;
            }
        }
    });
    let line = rx.recv_timeout(Duration::from_secs(3)).expect("no github line");
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(line, r#"{"github":{"error":"gh not installed"}}"#);
}

#[test]
fn exits_on_stdin_eof() {
    let (_sock, _herdr, mut b) = connected("eof");
    drop(b.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if let Some(status) = b.child.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        assert!(Instant::now() < deadline, "bridge did not exit on stdin EOF");
        std::thread::sleep(Duration::from_millis(20));
    }
}
