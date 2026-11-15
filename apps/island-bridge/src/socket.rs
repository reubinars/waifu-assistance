//! The private Unix socket agent hooks connect to, and the pending approvals waiting on it.
//!
//! A hook sends one line and then, for an approval, keeps the connection open until it
//! gets the island's decision. The bridge never decides on its own: when the hook goes away
//! (answered in the terminal, killed, timed out) the card is closed.

use island_core::approvals::{ApprovalRequest, Decision};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// How long a connection has to send its request line.
const FIRST_LINE_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest request line accepted.
const MAX_LINE: u64 = 1 << 20;

static PENDING: Mutex<Option<HashMap<String, UnixStream>>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn pending<T>(f: impl FnOnce(&mut HashMap<String, UnixStream>) -> T) -> T {
    let mut guard = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// `$XDG_RUNTIME_DIR/island.sock`, else `$HOME/.local/state/island/island.sock`.
pub fn socket_path() -> PathBuf {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    match var("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("island.sock"),
        None => PathBuf::from(var("HOME").unwrap_or_default()).join(".local/state/island/island.sock"),
    }
}

/// Binds the socket (mode 0600, replacing a stale file) and serves it on a thread.
pub fn spawn() -> io::Result<()> {
    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // The newest bridge wins: a socket already there is replaced by the rename below.
    if std::fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_socket()) {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a non-socket file is in the way"));
    }
    // Bind under a temporary name and rename, so the socket never exists with wider permissions.
    let tmp = path.with_extension(format!("sock.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let listener = UnixListener::bind(&tmp)?;
    let published = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
        .and_then(|_| std::fs::rename(&tmp, &path));
    if let Err(e) = published {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    std::thread::spawn(move || serve(stream));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    });
    Ok(())
}

fn serve(stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(FIRST_LINE_TIMEOUT));
    let Ok(reader) = stream.try_clone() else { return };
    let mut reader = BufReader::new(reader.take(MAX_LINE));
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(&line) else { return };
    let Some(pane_id) = v.get("pane_id").and_then(Value::as_str).filter(|s| !s.is_empty()) else { return };
    match v.get("kind").and_then(Value::as_str) {
        Some("question") => {
            let text = v.get("text").and_then(Value::as_str).unwrap_or_default();
            let agent = v.get("agent").and_then(Value::as_str).unwrap_or_default();
            crate::emit(&json!({"question": {"pane_id": pane_id, "agent": agent, "text": text}}).to_string());
        }
        Some("approval") => {
            let Ok(req) = v.get("request").cloned().map(serde_json::from_value::<ApprovalRequest>).transpose() else {
                return;
            };
            let Some(req) = req else { return };
            let timeout_ms = v.get("timeout_ms").and_then(Value::as_u64).unwrap_or(45_000);
            let Ok(held) = stream.try_clone() else { return };
            let id = format!("a{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
            let mut body = serde_json::to_value(&req).unwrap_or(Value::Null);
            let Some(obj) = body.as_object_mut() else { return };
            obj.insert("id".into(), id.clone().into());
            obj.insert("pane_id".into(), pane_id.into());
            obj.insert("timeout_ms".into(), timeout_ms.into());
            pending(|p| p.insert(id.clone(), held));
            crate::emit(&json!({"approval": body}).to_string());
            // Block until the hook goes away; anything it still sends is ignored.
            let _ = stream.set_read_timeout(None);
            let mut sink = [0u8; 256];
            while matches!((&stream).read(&mut sink), Ok(n) if n > 0) {}
            pending(|p| p.remove(&id));
            crate::emit(&json!({"approval_closed": {"id": id}}).to_string());
        }
        _ => {}
    }
}

/// Answers a `{"decide":{"id","decision"}}` line from the island. Unknown ids are ignored.
pub fn decide(line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let Some(d) = v.get("decide") else { return };
    let Some(id) = d.get("id").and_then(Value::as_str) else { return };
    let Some(decision) = d.get("decision").and_then(|x| serde_json::from_value::<Decision>(x.clone()).ok()) else {
        return;
    };
    let Some(mut stream) = pending(|p| p.remove(id)) else { return };
    let reply = json!({"decision": decision}).to_string();
    let _ = stream.write_all(format!("{reply}\n").as_bytes());
    let _ = stream.flush();
    // Wakes the reader thread, which then reports the card closed.
    let _ = stream.shutdown(std::net::Shutdown::Both);
}
