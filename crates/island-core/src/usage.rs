//! Plan-limit usage (Claude / Codex windows): the model, the bridge wire format, and the strip text.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    pub minutes: u32,
    pub used_percent: f32,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CliUsage {
    pub cli: String,
    pub windows: Vec<UsageWindow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Low,
    Mid,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Name,
    Dim,
    Percent(Level),
    /// A small bar filled to this percent (0..=100), coloured by its level; the run's text is empty.
    Bar(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub tone: Tone,
}

/// The bridge's `{"usage":[...]}` line, without a trailing newline.
pub fn usage_line(snapshot: &[CliUsage]) -> String {
    serde_json::json!({ "usage": snapshot }).to_string()
}

/// Parses the array under the `"usage"` key. Entries that don't parse, and entries with no windows, are dropped.
pub fn parse_usage(v: &serde_json::Value) -> Vec<CliUsage> {
    let Some(items) = v.as_array() else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| serde_json::from_value::<CliUsage>(item.clone()).ok())
        .filter(|c| !c.windows.is_empty())
        .collect()
}

pub fn level(pct: f32) -> Level {
    if pct < 50.0 {
        Level::Low
    } else if pct < 80.0 {
        Level::Mid
    } else {
        Level::High
    }
}

/// Under a day: whole hours (`5h`). A day or more: whole days (`7d`).
pub fn window_label(minutes: u32) -> String {
    if minutes < 1440 { format!("{}h", minutes / 60) } else { format!("{}d", minutes / 1440) }
}

pub fn cli_label(cli: &str) -> String {
    match cli {
        "claude" => "Claude".to_owned(),
        "codex" => "Codex".to_owned(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    }
}

/// Time left as `4d3h`, `2h10m`, `9m`. Never `0m`.
pub fn countdown(secs_left: i64) -> String {
    let days = secs_left / 86400;
    let hours = secs_left / 3600;
    if days > 0 {
        format!("{days}d{}h", (secs_left % 86400) / 3600)
    } else if hours > 0 {
        format!("{hours}h{:02}m", (secs_left % 3600) / 60)
    } else {
        format!("{}m", (secs_left / 60).max(1))
    }
}

/// The displayed percent (clamped to 0..=100) and the seconds left. A window whose reset time has
/// passed shows 0% with no countdown; one with no reset time shows no countdown.
pub fn effective(w: &UsageWindow, now_unix: i64) -> (f32, Option<i64>) {
    let pct = w.used_percent.clamp(0.0, 100.0);
    match w.resets_at {
        Some(reset) if now_unix >= reset => (0.0, None),
        Some(reset) => (pct, Some(reset - now_unix)),
        None => (pct, None),
    }
}

/// The strip text: one name run per CLI, then its windows, each with a bar, a percent and a countdown.
pub fn strip_runs(snapshot: &[CliUsage], now_unix: i64) -> Vec<Run> {
    let mut runs = Vec::new();
    for (i, cli) in snapshot.iter().enumerate() {
        let lead = if i == 0 { "" } else { "   " };
        runs.push(Run { text: format!("{lead}{}", cli_label(&cli.cli)), tone: Tone::Name });
        for (j, w) in cli.windows.iter().enumerate() {
            let sep = if j == 0 { " " } else { "  " };
            runs.push(Run { text: format!("{sep}{} ", window_label(w.minutes)), tone: Tone::Dim });
            let (pct, secs) = effective(w, now_unix);
            // Colour and text both use the rounded percent, so the colour matches the number shown.
            let shown = pct.round();
            runs.push(Run { text: String::new(), tone: Tone::Bar(shown as u8) });
            runs.push(Run { text: format!("{shown:.0}%"), tone: Tone::Percent(level(shown)) });
            if let Some(secs) = secs {
                runs.push(Run { text: format!(" · {}", countdown(secs)), tone: Tone::Dim });
            }
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(window_label(300), "5h");
        assert_eq!(window_label(60), "1h");
        assert_eq!(window_label(1440), "1d");
        assert_eq!(window_label(10080), "7d");
        assert_eq!(window_label(90), "1h");
        assert_eq!(window_label(1500), "1d");
        assert_eq!(window_label(2000), "1d");
        assert_eq!(cli_label("claude"), "Claude");
        assert_eq!(cli_label("codex"), "Codex");
    }

    #[test]
    fn level_follows_shown_percent() {
        let w = |p| UsageWindow { minutes: 300, used_percent: p, resets_at: None };
        assert_eq!(
            strip_runs(&[CliUsage { cli: "claude".into(), windows: vec![w(79.6)] }], 0)[3],
            Run { text: "80%".into(), tone: Tone::Percent(Level::High) }
        );
        assert_eq!(
            strip_runs(&[CliUsage { cli: "claude".into(), windows: vec![w(49.6)] }], 0)[3],
            Run { text: "50%".into(), tone: Tone::Percent(Level::Mid) }
        );
    }

    #[test]
    fn countdowns() {
        assert_eq!(countdown(4 * 86400 + 3 * 3600 + 59), "4d3h");
        assert_eq!(countdown(2 * 3600 + 10 * 60), "2h10m");
        assert_eq!(countdown(3600 + 5 * 60), "1h05m");
        assert_eq!(countdown(9 * 60 + 30), "9m");
        assert_eq!(countdown(20), "1m"); // never "0m"
    }

    #[test]
    fn levels() {
        assert_eq!(level(49.9), Level::Low);
        assert_eq!(level(50.0), Level::Mid);
        assert_eq!(level(79.9), Level::Mid);
        assert_eq!(level(80.0), Level::High);
    }

    #[test]
    fn reset_rule_and_clamp() {
        let w = |p, r| UsageWindow { minutes: 300, used_percent: p, resets_at: r };
        assert_eq!(effective(&w(42.0, Some(1000)), 400), (42.0, Some(600)));
        assert_eq!(effective(&w(42.0, Some(1000)), 1000), (0.0, None));
        assert_eq!(effective(&w(42.0, Some(1000)), 2000), (0.0, None));
        assert_eq!(effective(&w(42.0, None), 2000), (42.0, None));
        assert_eq!(effective(&w(130.0, None), 0), (100.0, None));
        assert_eq!(effective(&w(-5.0, None), 0), (0.0, None));
    }

    #[test]
    fn runs() {
        let now = 1_000_000;
        let snap = vec![
            CliUsage {
                cli: "claude".into(),
                windows: vec![
                    UsageWindow { minutes: 300, used_percent: 42.4, resets_at: Some(now + 2 * 3600 + 600) },
                    UsageWindow { minutes: 10080, used_percent: 85.0, resets_at: None },
                ],
            },
            CliUsage {
                cli: "codex".into(),
                windows: vec![UsageWindow { minutes: 300, used_percent: 55.0, resets_at: Some(now - 1) }],
            },
        ];
        let r = |t: &str, tone| Run { text: t.into(), tone };
        assert_eq!(
            strip_runs(&snap, now),
            vec![
                r("Claude", Tone::Name),
                r(" 5h ", Tone::Dim),
                r("", Tone::Bar(42)),
                r("42%", Tone::Percent(Level::Low)),
                r(" · 2h10m", Tone::Dim),
                r("  7d ", Tone::Dim),
                r("", Tone::Bar(85)),
                r("85%", Tone::Percent(Level::High)),
                r("   Codex", Tone::Name),
                r(" 5h ", Tone::Dim),
                r("", Tone::Bar(0)),
                r("0%", Tone::Percent(Level::Low)),
            ]
        );
        assert!(strip_runs(&[], now).is_empty());
    }

    #[test]
    fn wire_round_trip_and_junk() {
        let snap = vec![CliUsage {
            cli: "codex".into(),
            windows: vec![UsageWindow { minutes: 300, used_percent: 20.0, resets_at: Some(5) }],
        }];
        let line = usage_line(&snap);
        assert!(line.starts_with(r#"{"usage":["#) && !line.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parse_usage(&v["usage"]), snap);
        let junk = serde_json::json!([
            {"cli":"x"},
            7,
            {"cli":"y","windows":[]},
            {"cli":"z","windows":[{"minutes":60,"used_percent":1,"resets_at":null}]}
        ]);
        assert_eq!(parse_usage(&junk).iter().map(|c| c.cli.as_str()).collect::<Vec<_>>(), vec!["z"]);
        assert!(parse_usage(&serde_json::json!("nope")).is_empty());
    }
}
