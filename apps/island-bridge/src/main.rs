//! island-bridge: relays newline-delimited JSON between stdio and herdr's Unix socket.
//!
//! herdr answers one request per connection, and a subscribed connection only streams
//! events. So each request line gets its own short connection, and `events.subscribe`
//! lines get one long-lived connection that each new subscribe replaces. The old one is
//! closed only once herdr answers on the new one, so no event falls between them.
//!
//! Prints `{"bridge":"connected"}` / `{"bridge":"disconnected"}` on each transition,
//! retries with backoff (1 s doubling to 10 s) and exits when stdin closes.

use std::io::{self, BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use serde_json::json;

mod github;
mod hook;
mod install;
mod socket;
mod usage;

const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(10);
/// How often the hook status is re-read from the agents' config.
const HOOK_STATUS_INTERVAL: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

enum Msg {
    Line(String),
    Eof,
    /// The subscription connection with this generation sent its first line.
    SubscriptionStarted(u64),
    /// The subscription connection with this generation ended.
    SubscriptionClosed(u64),
    /// A request could not reach herdr.
    Unreachable,
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
pub(crate) fn emit(line: &str) {
    let mut out = io::stdout().lock();
    if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
        std::process::exit(0);
    }
}

fn is_subscribe(line: &str) -> bool {
    line.contains(r#""method":"events.subscribe""#)
}

fn send_line(stream: &mut UnixStream, line: &str) -> io::Result<()> {
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn spawn_stdin_reader(tx: Sender<Msg>) {
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line.starts_with(r#"{"decide""#) {
                socket::decide(&line);
                continue;
            }
            if tx.send(Msg::Line(line)).is_err() {
                return;
            }
        }
        let _ = tx.send(Msg::Eof);
    });
}

/// One request, one connection, one response line.
fn spawn_request(path: PathBuf, line: String, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let Ok(mut stream) = UnixStream::connect(&path) else {
            let _ = tx.send(Msg::Unreachable);
            return;
        };
        let _ = stream.set_read_timeout(Some(REQUEST_TIMEOUT));
        if send_line(&mut stream, &line).is_err() {
            let _ = tx.send(Msg::Unreachable);
            return;
        }
        let mut response = String::new();
        if BufReader::new(&stream).read_line(&mut response).unwrap_or(0) > 0 {
            emit(response.trim_end_matches(['\r', '\n']));
        }
        let _ = stream.shutdown(Shutdown::Both);
    });
}

fn open_subscription(path: &Path, line: &str, generation: u64, tx: Sender<Msg>) -> Option<UnixStream> {
    let mut stream = UnixStream::connect(path).ok()?;
    send_line(&mut stream, line).ok()?;
    let reader = stream.try_clone().ok()?;
    std::thread::spawn(move || {
        let mut started = false;
        for line in BufReader::new(reader).lines() {
            let Ok(line) = line else { break };
            emit(&line);
            if !started {
                started = true;
                let _ = tx.send(Msg::SubscriptionStarted(generation));
            }
        }
        let _ = tx.send(Msg::SubscriptionClosed(generation));
    });
    Some(stream)
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

/// Serves lines while herdr is reachable; returns when it stops being reachable.
fn serve(path: &Path, rx: &Receiver<Msg>, tx: &Sender<Msg>, generation: &mut u64) {
    let mut subscription: Option<UnixStream> = None;
    // Replaced subscriptions, kept streaming until the current one has started.
    let mut retiring: Vec<UnixStream> = Vec::new();
    let close_all = |current: &mut Option<UnixStream>, retiring: &mut Vec<UnixStream>| {
        for s in current.take().into_iter().chain(retiring.drain(..)) {
            let _ = s.shutdown(Shutdown::Both);
        }
    };
    loop {
        match rx.recv() {
            Ok(Msg::Line(line)) if is_subscribe(&line) => {
                *generation += 1;
                match open_subscription(path, &line, *generation, tx.clone()) {
                    Some(new) => retiring.extend(subscription.replace(new)),
                    None => {
                        close_all(&mut subscription, &mut retiring);
                        return;
                    }
                }
            }
            Ok(Msg::Line(line)) => spawn_request(path.to_path_buf(), line, tx.clone()),
            Ok(Msg::SubscriptionStarted(g)) if g == *generation => {
                for s in retiring.drain(..) {
                    let _ = s.shutdown(Shutdown::Both);
                }
            }
            Ok(Msg::SubscriptionStarted(_)) => {}
            Ok(Msg::SubscriptionClosed(g)) if g == *generation && subscription.is_some() => {
                close_all(&mut subscription, &mut retiring);
                return;
            }
            Ok(Msg::SubscriptionClosed(_)) => {}
            Ok(Msg::Unreachable) => {
                close_all(&mut subscription, &mut retiring);
                *generation += 1; // its reader's close notice is now stale
                return;
            }
            Ok(Msg::Eof) | Err(_) => std::process::exit(0),
        }
    }
}

/// Waits out the backoff, dropping lines (herdr is not there to take them).
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

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// `island-bridge hooks install|remove`: prints one line per changed file and returns the exit code.
fn hooks_command(action: Option<&str>) -> i32 {
    let Some(home) = home_dir() else {
        eprintln!("island-bridge: HOME is not set");
        return 1;
    };
    let lines = match action {
        Some("install") => match std::env::current_exe() {
            Ok(exe) => install::install(&home, &exe),
            Err(e) => {
                eprintln!("island-bridge: cannot find own path: {e}");
                return 1;
            }
        },
        Some("remove") => install::remove(&home),
        _ => {
            eprintln!("usage: island-bridge hooks install|remove");
            return 2;
        }
    };
    match lines {
        Ok(lines) => {
            lines.iter().for_each(|l| emit(l));
            0
        }
        Err(e) => {
            eprintln!("island-bridge: hooks: {e}");
            1
        }
    }
}

fn emit_hooks(status: &island_core::approvals::HookStatus) {
    emit(&json!({"hooks": status.0}).to_string());
}

fn current_hook_status() -> island_core::approvals::HookStatus {
    home_dir().map(|h| install::status(&h)).unwrap_or_default()
}

#[cfg(debug_assertions)]
fn hook_status_interval() -> Duration {
    std::env::var("ISLAND_HOOK_STATUS_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(HOOK_STATUS_INTERVAL, |ms: u64| Duration::from_millis(ms.max(1)))
}

#[cfg(not(debug_assertions))]
fn hook_status_interval() -> Duration {
    HOOK_STATUS_INTERVAL
}

/// Tells the island which agent CLIs have our hook in place, now and again whenever that changes.
fn spawn_hook_status() {
    let mut last = current_hook_status();
    emit_hooks(&last);
    std::thread::spawn(move || loop {
        std::thread::sleep(hook_status_interval());
        let now = current_hook_status();
        if now != last {
            emit_hooks(&now);
            last = now;
        }
    });
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("hook") {
        std::process::exit(hook::run(args.get(2).map_or("", String::as_str)));
    }
    if args.get(1).map(String::as_str) == Some("hooks") {
        std::process::exit(hooks_command(args.get(2).map(String::as_str)));
    }
    if let Err(e) = socket::spawn() {
        eprintln!("island-bridge: approvals disabled, cannot serve {}: {e}", socket::socket_path().display());
    }
    spawn_hook_status();
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    usage::spawn(home);
    github::spawn();

    let (tx, rx) = mpsc::channel();
    spawn_stdin_reader(tx.clone());

    let path = socket_path();
    let mut status = Status(None);
    let mut backoff = MIN_BACKOFF;
    let mut generation = 0u64;

    loop {
        // An empty connection tells us whether herdr is listening.
        match UnixStream::connect(&path) {
            Ok(probe) => {
                let _ = probe.shutdown(Shutdown::Both);
                backoff = MIN_BACKOFF;
                status.set(true);
                serve(&path, &rx, &tx, &mut generation);
                status.set(false);
            }
            Err(_) => status.set(false),
        }
        wait(backoff, &rx);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}
