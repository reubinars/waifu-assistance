use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const CONNECTED: &str = r#"{"bridge":"connected"}"#;
const DISCONNECTED: &str = r#"{"bridge":"disconnected"}"#;

struct Bridge {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    sock: PathBuf,
}

impl Bridge {
    fn sock_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ib-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("h.sock")
    }

    /// Bridge started with herdr already listening.
    fn spawn_listening(name: &str) -> (Bridge, UnixListener) {
        let sock = Bridge::sock_path(name);
        let listener = UnixListener::bind(&sock).unwrap();
        (Bridge::spawn_at(sock), listener)
    }

    /// Bridge started with no socket present.
    fn spawn(name: &str) -> Bridge {
        Bridge::spawn_at(Bridge::sock_path(name))
    }

    fn spawn_at(sock: PathBuf) -> Bridge {
        let mut child = Command::new(env!("CARGO_BIN_EXE_island-bridge"))
            .env("HERDR_SOCKET_PATH", &sock)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        Bridge { child, stdin, lines, sock }
    }

    fn next(&self, secs: u64) -> String {
        self.lines
            .recv_timeout(Duration::from_secs(secs))
            .expect("no line from bridge in time")
    }

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.flush().unwrap();
    }

    fn listen(&self) -> UnixListener {
        let _ = std::fs::remove_file(&self.sock);
        UnixListener::bind(&self.sock).unwrap()
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn accept(listener: &UnixListener) -> (UnixStream, BufReader<UnixStream>) {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let stream = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "bridge never connected");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("{e}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let reader = BufReader::new(stream.try_clone().unwrap());
    (stream, reader)
}

#[test]
fn relays_both_ways() {
    let (mut b, listener) = Bridge::spawn_listening("relay");
    let (mut stream, mut reader) = accept(&listener);
    assert_eq!(b.next(2), CONNECTED);

    let req = "{\"id\":\"island:1\",\"method\":\"agent.list\",\"params\":{}}\n";
    b.send(req);
    let mut got = String::new();
    reader.read_line(&mut got).unwrap();
    assert_eq!(got, req);

    stream.write_all(b"{\"x\":1}\n").unwrap();
    assert_eq!(b.next(2), r#"{"x":1}"#);
}

#[test]
fn waits_for_missing_socket() {
    let b = Bridge::spawn("missing");
    assert_eq!(b.next(2), DISCONNECTED);
    let listener = b.listen();
    let _conn = accept(&listener);
    assert_eq!(b.next(3), CONNECTED);
}

#[test]
fn reconnects_after_drop() {
    let (b, listener) = Bridge::spawn_listening("drop");
    let conn = accept(&listener);
    assert_eq!(b.next(2), CONNECTED);

    drop(conn);
    drop(listener);
    std::fs::remove_file(&b.sock).unwrap();
    assert_eq!(b.next(2), DISCONNECTED);

    let listener = b.listen();
    let _conn = accept(&listener);
    assert_eq!(b.next(3), CONNECTED);
}

#[test]
fn exits_on_stdin_eof() {
    let (mut b, listener) = Bridge::spawn_listening("eof");
    let _conn = accept(&listener);
    assert_eq!(b.next(2), CONNECTED);

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
