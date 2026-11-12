//! Claude's plan limits, as written by the Claude statusline script to `~/.cache/island/claude-limits.json`.

use std::path::Path;

use island_core::usage::{CliUsage, UsageWindow};
use serde_json::Value;

/// `five_hour` and `seven_day` in the file, with their window lengths in minutes.
const WINDOWS: [(&str, u32); 2] = [("five_hour", 300), ("seven_day", 10080)];

pub fn read(home: &Path) -> Option<CliUsage> {
    let text = std::fs::read_to_string(home.join(".cache/island/claude-limits.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let windows: Vec<UsageWindow> = WINDOWS
        .iter()
        .filter_map(|&(key, minutes)| {
            let w = v.get(key)?;
            let used_percent = w.get("used_percentage")?.as_f64()? as f32;
            let resets_at = w.get("resets_at").and_then(Value::as_i64);
            Some(UsageWindow { minutes, used_percent, resets_at })
        })
        .collect();
    (!windows.is_empty()).then(|| CliUsage { cli: "claude".into(), windows })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::test_support::{temp_home, write};

    #[test]
    fn reads_both_windows() {
        let home = temp_home("claude-ok");
        write(
            &home,
            ".cache/island/claude-limits.json",
            r#"{"five_hour":{"used_percentage":42,"resets_at":1794477894},"seven_day":{"used_percentage":21.5,"resets_at":null},"other":{}}"#,
        );
        assert_eq!(
            read(&home),
            Some(CliUsage {
                cli: "claude".into(),
                windows: vec![
                    UsageWindow { minutes: 300, used_percent: 42.0, resets_at: Some(1794477894) },
                    UsageWindow { minutes: 10080, used_percent: 21.5, resets_at: None },
                ],
            })
        );
    }

    #[test]
    fn skips_window_without_percent() {
        let home = temp_home("claude-partial");
        write(
            &home,
            ".cache/island/claude-limits.json",
            r#"{"five_hour":{"resets_at":1794477894},"seven_day":{"used_percentage":21.5,"resets_at":null}}"#,
        );
        assert_eq!(
            read(&home),
            Some(CliUsage {
                cli: "claude".into(),
                windows: vec![UsageWindow { minutes: 10080, used_percent: 21.5, resets_at: None }],
            })
        );
    }

    #[test]
    fn iso_resets_at_keeps_window_without_reset() {
        let home = temp_home("claude-iso");
        write(
            &home,
            ".cache/island/claude-limits.json",
            r#"{"five_hour":{"used_percentage":42,"resets_at":"2026-11-12T10:00:00Z"}}"#,
        );
        assert_eq!(
            read(&home),
            Some(CliUsage {
                cli: "claude".into(),
                windows: vec![UsageWindow { minutes: 300, used_percent: 42.0, resets_at: None }],
            })
        );
    }

    #[test]
    fn missing_or_junk_is_none() {
        let home = temp_home("claude-none");
        assert_eq!(read(&home), None);
        write(&home, ".cache/island/claude-limits.json", "{\"five_hour\":");
        assert_eq!(read(&home), None);
        write(&home, ".cache/island/claude-limits.json", r#"{"five_hour":{}}"#);
        assert_eq!(read(&home), None);
    }
}
