//! Codex's plan limits, from the `rate_limits` of the newest rollout that has them.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use island_core::usage::{CliUsage, UsageWindow};
use serde_json::Value;

/// How many of the newest rollouts to look at before giving up.
const MAX_FILES: usize = 5;

pub fn read(home: &Path) -> Option<CliUsage> {
    let mut files = Vec::new();
    collect(&home.join(".codex/sessions"), &mut files);
    files.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
    files.iter().take(MAX_FILES).find_map(|(_, path)| from_rollout(&std::fs::read_to_string(path).ok()?))
}

/// Gathers `rollout-*.jsonl` files under `dir`, recursively, with their modification times.
fn collect(dir: &Path, out: &mut Vec<(SystemTime, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("rollout-") && name.ends_with(".jsonl") {
            if let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) {
                out.push((mtime, path));
            }
        }
    }
}

/// The last line of the rollout that carries usable `rate_limits`. Lines that don't parse (a
/// line still being written, say) are skipped.
fn from_rollout(text: &str) -> Option<CliUsage> {
    text.lines().rev().find_map(|line| {
        if !line.contains(r#""rate_limits""#) {
            return None;
        }
        let v: Value = serde_json::from_str(line).ok()?;
        let limits = find_key(&v, "rate_limits")?;
        let windows: Vec<UsageWindow> =
            ["primary", "secondary"].iter().filter_map(|k| window(limits.get(k)?)).collect();
        (!windows.is_empty()).then(|| CliUsage { cli: "codex".into(), windows })
    })
}

/// The first value under `key` at any depth.
fn find_key<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(map) => map.get(key).or_else(|| map.values().find_map(|c| find_key(c, key))),
        Value::Array(items) => items.iter().find_map(|c| find_key(c, key)),
        _ => None,
    }
}

fn window(w: &Value) -> Option<UsageWindow> {
    Some(UsageWindow {
        minutes: u32::try_from(w.get("window_minutes")?.as_u64()?).ok()?,
        used_percent: w.get("used_percent")?.as_f64()? as f32,
        resets_at: w.get("resets_at").and_then(Value::as_i64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::test_support::{set_mtime, temp_home, write};

    const BASE: u64 = 1_700_000_000;

    /// A `token_count` event line carrying both windows.
    fn rl(p: u32, s: u32) -> String {
        format!(
            r#"{{"type":"event_msg","payload":{{"type":"token_count","rate_limits":{{"primary":{{"used_percent":{p},"window_minutes":300,"resets_at":111}},"secondary":{{"used_percent":{s},"window_minutes":10080,"resets_at":222}}}}}}}}"#
        )
    }

    /// Writes a rollout under `~/.codex/sessions/<rel>` with the given modification time.
    fn rollout(home: &std::path::Path, rel: &str, body: &str, mtime: u64) {
        let path = write(home, &format!(".codex/sessions/{rel}"), body);
        set_mtime(&path, mtime);
    }

    #[test]
    fn newest_file_last_line_wins() {
        let home = temp_home("codex-last");
        rollout(&home, "2026/11/10/rollout-a.jsonl", &rl(1, 2), BASE);
        let body = format!("{}\n{}\n{}", rl(10, 20), r#"{"type":"x"}"#, rl(30, 40));
        rollout(&home, "2026/11/11/rollout-b.jsonl", &body, BASE + 100);
        assert_eq!(
            read(&home).unwrap().windows,
            vec![
                UsageWindow { minutes: 300, used_percent: 30.0, resets_at: Some(111) },
                UsageWindow { minutes: 10080, used_percent: 40.0, resets_at: Some(222) },
            ]
        );
    }

    #[test]
    fn truncated_last_line_is_skipped() {
        let home = temp_home("codex-trunc");
        let cut = rl(50, 60);
        let body = format!("{}\n{}", rl(30, 40), &cut[..cut.len() / 2]);
        rollout(&home, "2026/11/11/rollout-b.jsonl", &body, BASE + 100);
        let windows = read(&home).unwrap().windows;
        assert_eq!(windows[0].used_percent, 30.0);
        assert_eq!(windows[1].used_percent, 40.0);
    }

    #[test]
    fn falls_back_when_newest_has_none() {
        let home = temp_home("codex-fallback");
        rollout(&home, "2026/11/10/rollout-a.jsonl", &rl(5, 6), BASE);
        rollout(&home, "2026/11/11/rollout-b.jsonl", r#"{"type":"session_meta"}"#, BASE + 100);
        let windows = read(&home).unwrap().windows;
        assert_eq!(windows[0].used_percent, 5.0);
        assert_eq!(windows[1].used_percent, 6.0);
    }

    #[test]
    fn gives_up_after_five_files() {
        let home = temp_home("codex-five");
        rollout(&home, "2026/11/01/rollout-0.jsonl", &rl(7, 8), BASE);
        for i in 1..=5u64 {
            rollout(&home, &format!("2026/11/0{}/rollout-{i}.jsonl", i + 1), r#"{"type":"x"}"#, BASE + 100 * i);
        }
        assert_eq!(read(&home), None);
    }

    #[test]
    fn ignores_non_rollout_files() {
        let home = temp_home("codex-names");
        rollout(&home, "2026/11/10/rollout-a.jsonl", &rl(1, 2), BASE);
        rollout(&home, "2026/11/11/notes.jsonl", &rl(9, 9), BASE + 100);
        assert_eq!(read(&home).unwrap().windows[0].used_percent, 1.0);
    }

    #[test]
    fn no_sessions_dir_is_none() {
        assert_eq!(read(&temp_home("codex-empty")), None);
    }
}
