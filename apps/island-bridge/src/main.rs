//! island-bridge: relays newline-delimited JSON between stdio and herdr's Unix socket.
//!
//! island.exe runs this inside WSL. It prints `{"bridge":"connected"}` /
//! `{"bridge":"disconnected"}` on each transition, reconnects with backoff
//! (1 s doubling to 10 s) and exits when stdin closes.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(10);

enum Msg {
    Line(String),
    Eof,
    SocketClosed(u64),
}

fn socket_path() -> PathBuf {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    if let Some(p) = var("HERDR_SOCKET_PATH") {
        return p.into();
    }
    let config = match var("XDG_CONFIG_HOME") {
        Some(x) => PathBuf::from(x),
        None => PathBuf::from(var("HOME").unwrap_or_default()).join(".config"),
    };
    config.join("herdr").join("herdr.sock")
}

/// Writes one line to stdout; if the island is gone there is nothing left to do.
fn emit(line: &str) {
    let mut out = io::stdout().lock();
    if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
        std::process::exit(0);
    }
}

struct Status(Option<bool>);

impl Status {
    fn set(&mut self, connected: bool) {
        if self.0 != Some(connected) {
            self.0 = Some(connected);
            emit(if connected { r#"{"bridge":"connected"}"# } else { r#"{"bridge":"disconnected"}"# });
        }
    }
}

fn spawn_stdin_reader(tx: Sender<Msg>) {
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(Msg::Line(line)).is_err() {
                return;
            }
        }
        let _ = tx.send(Msg::Eof);
    });
}

fn spawn_socket_reader(stream: UnixStream, generation: u64, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            emit(&line);
        }
        let _ = tx.send(Msg::SocketClosed(generation));
    });
}

/// Forwards stdin lines to herdr until the connection ends.
fn relay(stream: &mut UnixStream, generation: u64, rx: &Receiver<Msg>) {
    loop {
        match rx.recv() {
            Ok(Msg::Line(line)) => {
                let sent = stream
                    .write_all(line.as_bytes())
                    .and_then(|_| stream.write_all(b"\n"))
                    .and_then(|_| stream.flush());
                if sent.is_err() {
                    return;
                }
            }
            Ok(Msg::SocketClosed(g)) if g == generation => return,
            Ok(Msg::SocketClosed(_)) => {}
            Ok(Msg::Eof) | Err(_) => std::process::exit(0),
        }
    }
}

/// Waits out the backoff, dropping stdin lines (herdr is not there to take them).
fn wait(backoff: Duration, rx: &Receiver<Msg>) {
    let deadline = Instant::now() + backoff;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Msg::Eof) | Err(RecvTimeoutError::Disconnected) => std::process::exit(0),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => return,
        }
    }
}

fn main() {
    let (tx, rx) = mpsc::channel();
    spawn_stdin_reader(tx.clone());

    let path = socket_path();
    let mut status = Status(None);
    let mut backoff = MIN_BACKOFF;
    let mut generation = 0u64;

    loop {
        match UnixStream::connect(&path) {
            Ok(mut stream) => {
                generation += 1;
                backoff = MIN_BACKOFF;
                status.set(true);
                if let Ok(reader) = stream.try_clone() {
                    spawn_socket_reader(reader, generation, tx.clone());
                    relay(&mut stream, generation, &rx);
                }
                let _ = stream.shutdown(std::net::Shutdown::Both);
                status.set(false);
            }
            Err(_) => status.set(false),
        }
        wait(backoff, &rx);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}
