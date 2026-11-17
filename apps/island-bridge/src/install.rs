//! `island-bridge hooks install|remove`: wires the island's permission hooks into the Claude
//! Code, Codex and OpenCode config, and reports which of them are wired.
//!
//! Only entries whose command is `<...>/island-bridge hook <agent>` are ever touched. Other
//! entries keep their bytes and order. Each change copies the file's previous bytes to
//! `<file>.bak`, then writes the new bytes through `<file>.tmp`. Modes are kept (new files are 0600).

use island_core::approvals::HookStatus;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const PLUGIN: &str = include_str!("opencode-plugin.js");
const HOOK_TIMEOUT_SECS: u64 = 60;
const AGENTS: [&str; 3] = ["claude", "codex", "opencode"];
/// Mode for files we create (a user's config is private).
const PRIVATE: u32 = 0o600;

/// Whether the Codex and OpenCode adapters exist. Until they do, install leaves those CLIs'
/// config alone and status doesn't report them. `remove` still cleans up after an older install.
const ADAPTERS_READY: bool = false;

/// Adds or refreshes the island's hooks. Returns one line per file changed or in error.
pub fn install(home: &Path, exe: &Path) -> io::Result<Vec<String>> {
    install_with(home, exe, ADAPTERS_READY)
}

fn install_with(home: &Path, exe: &Path, adapters: bool) -> io::Result<Vec<String>> {
    let mut lines = Vec::new();
    if home.join(".claude").is_dir() {
        let path = home.join(".claude/settings.json");
        let result = edit_json(&path, true, |root| {
            let hooks = hooks_of(root)?;
            upsert(entry(hooks, "PermissionRequest"), group(exe, "claude", None))?;
            upsert(entry(hooks, "PreToolUse"), group(exe, "claude", Some("AskUserQuestion")))
        });
        outcome("claude", &path, "hooks installed in", result, &mut lines);
    }
    if adapters && home.join(".codex").is_dir() {
        let path = home.join(".codex/hooks.json");
        let result = edit_json(&path, true, |root| {
            upsert(entry(hooks_of(root)?, "PermissionRequest"), group(exe, "codex", None))
        });
        outcome("codex", &path, "hooks installed in", result, &mut lines);
    }
    if adapters && home.join(".config/opencode").is_dir() {
        let path = plugin_path(home);
        let result = install_plugin(&path, exe);
        outcome("opencode", &path, "plugin written to", result, &mut lines);
    }
    Ok(lines)
}

/// Deletes the island's entries and plugin. Returns one line per file changed or in error.
pub fn remove(home: &Path) -> io::Result<Vec<String>> {
    let mut lines = Vec::new();
    for (label, rel) in [("claude", ".claude/settings.json"), ("codex", ".codex/hooks.json")] {
        let path = home.join(rel);
        let result = edit_json(&path, false, |root| {
            remove_ours(root);
            Ok(())
        });
        outcome(label, &path, "hooks removed from", result, &mut lines);
    }
    let path = plugin_path(home);
    let result = replace_file(&path, None);
    outcome("opencode", &path, "plugin removed from", result, &mut lines);
    Ok(lines)
}

/// Which CLIs have a config dir here, and whether the island's entry is in place for each.
pub fn status(home: &Path) -> HookStatus {
    status_with(home, ADAPTERS_READY)
}

fn status_with(home: &Path, adapters: bool) -> HookStatus {
    let mut map = BTreeMap::new();
    if home.join(".claude").is_dir() {
        let ours = read_json(&home.join(".claude/settings.json")).is_some_and(|v| {
            has_ours(&v, "PermissionRequest", None, "claude")
                && has_ours(&v, "PreToolUse", Some("AskUserQuestion"), "claude")
        });
        map.insert("claude".to_string(), ours);
    }
    if adapters && home.join(".codex").is_dir() {
        let ours = read_json(&home.join(".codex/hooks.json"))
            .is_some_and(|v| has_ours(&v, "PermissionRequest", None, "codex"));
        map.insert("codex".to_string(), ours);
    }
    if adapters && home.join(".config/opencode").is_dir() {
        map.insert("opencode".to_string(), plugin_path(home).is_file());
    }
    HookStatus(map)
}

fn plugin_path(home: &Path) -> PathBuf {
    home.join(".config/opencode/plugins/island-approval.js")
}

fn outcome(label: &str, path: &Path, done: &str, result: Result<bool, String>, lines: &mut Vec<String>) {
    match result {
        Ok(true) => lines.push(format!("{label}: {done} {}", path.display())),
        Ok(false) => {}
        Err(e) => lines.push(format!("{label}: {e}")),
    }
}

fn install_plugin(path: &Path, exe: &Path) -> Result<bool, String> {
    let exe_js = json!(exe.display().to_string()).to_string();
    let source = PLUGIN.replace("__EXE__", &exe_js);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| at(dir, e))?;
    }
    replace_file(path, Some(source.as_bytes()))
}

/// `"<exe>" hook <agent>`, the command every island entry runs. The path is double-quoted for
/// the shell, with `"`, `\`, `$` and backticks escaped.
fn command(exe: &Path, agent: &str) -> String {
    let mut cmd = String::from("\"");
    for c in exe.display().to_string().chars() {
        if matches!(c, '"' | '\\' | '$' | '`') {
            cmd.push('\\');
        }
        cmd.push(c);
    }
    cmd.push_str(&format!("\" hook {agent}"));
    cmd
}

/// Whether `cmd` is one of ours: `<exe ending in island-bridge> hook <agent>`.
fn is_ours(cmd: &str, agent: Option<&str>) -> bool {
    let Some((exe, cmd_agent)) = cmd.rsplit_once(" hook ") else { return false };
    // quoted (current form) or bare (form from before quoting)
    let exe = exe.strip_prefix('"').and_then(|e| e.strip_suffix('"')).unwrap_or(exe);
    let agent_ok = match agent {
        Some(a) => cmd_agent == a,
        None => AGENTS.contains(&cmd_agent),
    };
    agent_ok && exe.ends_with("island-bridge")
}

fn is_ours_hook(hook: &Value) -> bool {
    hook.get("command").and_then(Value::as_str).is_some_and(|c| is_ours(c, None))
}

fn group(exe: &Path, agent: &str, matcher: Option<&str>) -> Value {
    let mut g = Map::new();
    if let Some(m) = matcher {
        g.insert("matcher".into(), json!(m));
    }
    g.insert(
        "hooks".into(),
        json!([{"type": "command", "command": command(exe, agent), "timeout": HOOK_TIMEOUT_SECS}]),
    );
    Value::Object(g)
}

/// Whether `event` has a group with this `matcher` that runs our command for `agent`.
fn has_ours(root: &Value, event: &str, matcher: Option<&str>, agent: &str) -> bool {
    let groups = root.get("hooks").and_then(|h| h.get(event)).and_then(Value::as_array);
    groups.is_some_and(|groups| {
        groups.iter().any(|g| {
            g.get("matcher").and_then(Value::as_str) == matcher
                && g.get("hooks").and_then(Value::as_array).is_some_and(|hs| {
                    hs.iter().any(|h| h.get("command").and_then(Value::as_str).is_some_and(|c| is_ours(c, Some(agent))))
                })
        })
    })
}

/// Removes our hook entries from `list`. Returns the index where the first one sat, if any.
fn strip(list: &mut Vec<Value>) -> Option<usize> {
    let mut first = None;
    let mut out = Vec::with_capacity(list.len());
    for mut group in std::mem::take(list) {
        let ours = group.get("hooks").and_then(Value::as_array).is_some_and(|hs| hs.iter().any(is_ours_hook));
        if !ours {
            out.push(group);
            continue;
        }
        first.get_or_insert(out.len());
        if let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) {
            hooks.retain(|h| !is_ours_hook(h));
        }
        if group["hooks"].as_array().is_some_and(Vec::is_empty) {
            continue;
        }
        out.push(group);
    }
    *list = out;
    first
}

/// Puts `group` where our old entry was, or at the end. Other entries keep their order.
fn upsert(list: &mut Value, group: Value) -> Result<(), String> {
    let list = list.as_array_mut().ok_or("hook list is not an array")?;
    match strip(list) {
        Some(at) => list.insert(at, group),
        None => list.push(group),
    }
    Ok(())
}

fn hooks_of(root: &mut Value) -> Result<&mut Map<String, Value>, String> {
    let obj = root.as_object_mut().ok_or("is not a JSON object")?;
    obj.entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "\"hooks\" is not an object".to_string())
}

fn entry<'a>(hooks: &'a mut Map<String, Value>, event: &str) -> &'a mut Value {
    hooks.entry(event).or_insert_with(|| json!([]))
}

/// Deletes our entries. Containers this emptied go too, so the file returns to its prior shape.
fn remove_ours(root: &mut Value) {
    let Some(obj) = root.as_object_mut() else { return };
    let Some(hooks) = obj.get_mut("hooks").and_then(Value::as_object_mut) else { return };
    let mut touched = false;
    for key in hooks.keys().cloned().collect::<Vec<_>>() {
        let Some(list) = hooks.get_mut(&key).and_then(Value::as_array_mut) else { continue };
        if strip(list).is_some() {
            touched = true;
            if list.is_empty() {
                hooks.remove(&key);
            }
        }
    }
    if touched && hooks.is_empty() {
        obj.remove("hooks");
    }
}

/// Applies `edit` to the JSON object at `path` and writes it back if it changed. A missing file
/// is created only when `create` is set. Emptying the file deletes it.
fn edit_json(path: &Path, create: bool, edit: impl FnOnce(&mut Value) -> Result<(), String>) -> Result<bool, String> {
    let existing = read_opt(path)?;
    let mut root = match existing.as_deref() {
        Some(bytes) => match serde_json::from_slice::<Value>(bytes) {
            Ok(v) if v.is_object() => v,
            _ => return Err(format!("{} is not a JSON object; left unchanged", path.display())),
        },
        None if create => json!({}),
        None => return Ok(false),
    };
    let before = root.clone();
    edit(&mut root).map_err(|e| format!("{e}; {} left unchanged", path.display()))?;
    if root == before {
        return Ok(false);
    }
    let is_link = fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    let bytes = if root.as_object().is_some_and(Map::is_empty) {
        // a symlinked config keeps its target, emptied, rather than losing the file behind the link
        is_link.then(|| b"{}\n".to_vec())
    } else {
        let mut b = serde_json::to_vec_pretty(&root).map_err(|e| format!("{}: {e}", path.display()))?;
        b.push(b'\n');
        Some(b)
    };
    replace_file(path, bytes.as_deref())
}

/// Replaces `path` with `bytes` (or deletes it on `None`), backing up the old bytes first.
/// Returns false when the file already holds `bytes`.
fn replace_file(path: &Path, bytes: Option<&[u8]>) -> Result<bool, String> {
    // a symlinked config (a dotfiles repo) is written through, so the link survives
    let resolved = if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        fs::canonicalize(path).map_err(|e| at(path, e))?
    } else {
        path.to_path_buf()
    };
    let path = resolved.as_path();
    let existing = read_opt(path)?;
    if existing.as_deref() == bytes {
        return Ok(false);
    }
    // the backup and the new file keep the original's mode; a new file is private
    let mode = match fs::metadata(path) {
        Ok(m) if existing.is_some() => m.permissions().mode() & 0o777,
        _ => PRIVATE,
    };
    if let Some(old) = &existing {
        write_with_mode(&with_suffix(path, ".bak"), old, mode).map_err(|e| at(path, e))?;
    }
    match bytes {
        Some(b) => {
            let tmp = with_suffix(path, ".tmp");
            write_with_mode(&tmp, b, mode).and_then(|()| fs::rename(&tmp, path)).map_err(|e| at(path, e))?;
        }
        None => fs::remove_file(path).map_err(|e| at(path, e))?,
    }
    Ok(true)
}

/// Writes `bytes` to `path` with `mode`, also when the file already existed. The mode is set
/// before any content is written, so the bytes are never readable under an older, wider mode.
fn write_with_mode(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().write(true).create(true).truncate(false).mode(mode).open(path)?;
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.set_len(0)?;
    file.write_all(bytes)
}

fn read_opt(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(at(path, e)),
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn at(path: &Path, e: io::Error) -> String {
    format!("{}: {e}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/opt/test/bin/island-bridge";

    const CLAUDE: &str = r#"{
  "model": "opus",
  "hooks": {
    "SessionStart": [
      { "hooks": [ { "type": "command", "command": "herdr session-start claude", "timeout": 10 } ] }
    ]
  }
}
"#;

    const CODEX: &str = r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"herdr session-start codex","timeout":10}]}]}}"#;

    fn home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("iba-install-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn parse(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn install_is_idempotent_and_keeps_herdr() {
        let home = home("idem");
        write(&home.join(".claude/settings.json"), CLAUDE);
        write(&home.join(".codex/hooks.json"), CODEX);
        fs::create_dir_all(home.join(".config/opencode")).unwrap();
        let exe = Path::new(EXE);
        let claude = home.join(".claude/settings.json");
        let codex = home.join(".codex/hooks.json");
        let plugin = plugin_path(&home);

        assert_eq!(install_with(&home, exe, true).unwrap().len(), 3);
        let (c1, x1, p1) = (fs::read(&claude).unwrap(), fs::read(&codex).unwrap(), fs::read(&plugin).unwrap());
        assert_eq!(fs::read_to_string(claude.with_extension("json.bak")).unwrap(), CLAUDE);

        assert!(install_with(&home, exe, true).unwrap().is_empty());
        assert_eq!(fs::read(&claude).unwrap(), c1);
        assert_eq!(fs::read(&codex).unwrap(), x1);
        assert_eq!(fs::read(&plugin).unwrap(), p1);

        let v = parse(&claude);
        let original: Value = serde_json::from_str(CLAUDE).unwrap();
        assert_eq!(v["hooks"]["SessionStart"], original["hooks"]["SessionStart"]);
        assert_eq!(v["model"], "opus");
        assert_eq!(v["hooks"]["PermissionRequest"][0]["hooks"][0]["command"], format!("\"{EXE}\" hook claude"));
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "AskUserQuestion");
        let codex_original: Value = serde_json::from_str(CODEX).unwrap();
        assert_eq!(parse(&codex)["hooks"]["SessionStart"], codex_original["hooks"]["SessionStart"]);
        assert_eq!(
            parse(&codex)["hooks"]["PermissionRequest"][0]["hooks"][0]["command"],
            format!("\"{EXE}\" hook codex")
        );
        assert!(String::from_utf8_lossy(&p1).contains(&format!("const EXE = \"{EXE}\";")));

        // a different exe replaces our entry rather than adding a second one
        install_with(&home, Path::new("/other/island-bridge"), true).unwrap();
        let v = parse(&claude);
        let groups = v["hooks"]["PermissionRequest"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["hooks"][0]["command"], "\"/other/island-bridge\" hook claude");
    }

    #[test]
    fn remove_restores() {
        let cases = [("herdr", CLAUDE), ("bare", "{\n  \"model\": \"opus\"\n}\n")];
        for (name, claude_text) in cases {
            let home = home(&format!("remove-{name}"));
            write(&home.join(".claude/settings.json"), claude_text);
            write(&home.join(".codex/hooks.json"), CODEX);
            fs::create_dir_all(home.join(".config/opencode")).unwrap();
            install_with(&home, Path::new(EXE), true).unwrap();

            let lines = remove(&home).unwrap();
            assert_eq!(lines.len(), 3, "{lines:?}");
            let original: Value = serde_json::from_str(claude_text).unwrap();
            assert_eq!(parse(&home.join(".claude/settings.json")), original, "{name}");
            assert_eq!(parse(&home.join(".codex/hooks.json")), serde_json::from_str::<Value>(CODEX).unwrap());
            assert!(!plugin_path(&home).exists());
        }
    }

    #[test]
    fn remove_deletes_files_install_created() {
        let home = home("created");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        install_with(&home, Path::new(EXE), true).unwrap();
        assert!(home.join(".claude/settings.json").is_file());
        assert!(home.join(".codex/hooks.json").is_file());

        remove(&home).unwrap();
        assert!(!home.join(".claude/settings.json").exists());
        assert!(!home.join(".codex/hooks.json").exists());
        assert!(home.join(".claude").is_dir() && home.join(".codex").is_dir());
    }

    #[test]
    fn skips_missing_clis() {
        let home = home("missing");
        write(&home.join(".claude/settings.json"), CLAUDE);

        let lines = install_with(&home, Path::new(EXE), true).unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(!home.join(".codex").exists());
        assert!(!home.join(".config").exists());
        let s = status_with(&home, true);
        let keys: Vec<&str> = s.0.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["claude"]);
    }

    #[test]
    fn adapters_off_leaves_codex_and_opencode_alone_but_remove_cleans_up() {
        let home = home("gate");
        write(&home.join(".claude/settings.json"), CLAUDE);
        write(&home.join(".codex/hooks.json"), CODEX);
        fs::create_dir_all(home.join(".config/opencode")).unwrap();

        let lines = install_with(&home, Path::new(EXE), false).unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(fs::read_to_string(home.join(".codex/hooks.json")).unwrap(), CODEX);
        assert!(!home.join(".codex/hooks.json.bak").exists());
        assert!(!plugin_path(&home).exists());
        let keys: Vec<String> = status_with(&home, false).0.into_keys().collect();
        assert_eq!(keys, vec!["claude".to_string()]);

        // an earlier install that did write them is still removed
        install_with(&home, Path::new(EXE), true).unwrap();
        assert!(plugin_path(&home).exists());
        remove(&home).unwrap();
        assert_eq!(parse(&home.join(".codex/hooks.json")), serde_json::from_str::<Value>(CODEX).unwrap());
        assert!(!plugin_path(&home).exists());
    }

    #[test]
    fn status_reports() {
        let home = home("status");
        write(&home.join(".claude/settings.json"), CLAUDE);
        fs::create_dir_all(home.join(".codex")).unwrap();

        let before = status_with(&home, true).0;
        assert_eq!(before.get("claude"), Some(&false));
        assert_eq!(before.get("codex"), Some(&false));
        assert!(!before.contains_key("opencode"));

        install_with(&home, Path::new(EXE), true).unwrap();
        let after = status_with(&home, true).0;
        assert_eq!(after.get("claude"), Some(&true));
        assert_eq!(after.get("codex"), Some(&true));
    }

    #[test]
    fn invalid_json_is_left_alone() {
        let home = home("invalid");
        write(&home.join(".claude/settings.json"), "{ not json");
        write(&home.join(".codex/hooks.json"), CODEX);

        let lines = install_with(&home, Path::new(EXE), true).unwrap();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("claude:") && l.contains("settings.json")), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("codex: hooks installed in")), "{lines:?}");
        assert_eq!(fs::read_to_string(home.join(".claude/settings.json")).unwrap(), "{ not json");
        assert!(!home.join(".claude/settings.json.bak").exists());
        assert_eq!(status_with(&home, true).0.get("codex"), Some(&true));

        let lines = remove(&home).unwrap();
        assert!(lines.iter().any(|l| l.starts_with("claude:") && l.contains("settings.json")), "{lines:?}");
        assert_eq!(fs::read_to_string(home.join(".claude/settings.json")).unwrap(), "{ not json");
    }
    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn install_keeps_file_modes() {
        let home = home("modes");
        let claude = home.join(".claude/settings.json");
        write(&claude, CLAUDE);
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o600)).unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();

        install_with(&home, Path::new(EXE), true).unwrap();
        assert_eq!(mode(&claude), 0o600);
        assert_eq!(mode(&claude.with_extension("json.bak")), 0o600);
        assert_eq!(mode(&home.join(".codex/hooks.json")), 0o600);
    }

    #[test]
    fn a_wider_old_backup_is_narrowed_before_new_content() {
        let dir = home("wide-bak");
        let f = dir.join("f.bak");
        write(&f, "old secret");
        fs::set_permissions(&f, fs::Permissions::from_mode(0o644)).unwrap();
        write_with_mode(&f, b"new", 0o600).unwrap();
        assert_eq!((mode(&f), fs::read_to_string(&f).unwrap().as_str()), (0o600, "new"));

        // through install: a stale world-readable .bak ends up with the original's mode
        let home = home("wide-bak-install");
        let claude = home.join(".claude/settings.json");
        write(&claude, CLAUDE);
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o600)).unwrap();
        let bak = home.join(".claude/settings.json.bak");
        write(&bak, "stale");
        fs::set_permissions(&bak, fs::Permissions::from_mode(0o644)).unwrap();
        install_with(&home, Path::new(EXE), false).unwrap();
        assert_eq!(mode(&bak), 0o600);
        assert_eq!(fs::read_to_string(&bak).unwrap(), CLAUDE);
    }

    #[test]
    fn symlinked_settings_survive_install_and_remove() {
        let home = home("symlink");
        let real = home.join("dotfiles/settings.json");
        write(&real, CLAUDE);
        fs::create_dir_all(home.join(".claude")).unwrap();
        let link = home.join(".claude/settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        install_with(&home, Path::new(EXE), false).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(has_ours(&parse(&real), "PermissionRequest", None, "claude"));
        assert_eq!(fs::read_to_string(home.join("dotfiles/settings.json.bak")).unwrap(), CLAUDE);
        assert!(!home.join(".claude/settings.json.bak").exists());
        assert!(!home.join(".claude/settings.json.tmp").exists());

        remove(&home).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(parse(&real), serde_json::from_str::<Value>(CLAUDE).unwrap());

        // a config that only held our entries is emptied, not deleted from behind the link
        write(&real, "{}\n");
        install_with(&home, Path::new(EXE), false).unwrap();
        remove(&home).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "{}\n");
    }

    #[test]
    fn upgrade_replaces_bare_entry() {
        let home = home("upgrade");
        let claude = home.join(".claude/settings.json");
        let bare = json!({"hooks": {
            "PermissionRequest": [{"hooks": [{"type": "command", "command": format!("{EXE} hook claude"), "timeout": 60}]}]
        }});
        write(&claude, &bare.to_string());

        install_with(&home, Path::new(EXE), true).unwrap();
        let v = parse(&claude);
        let groups = v["hooks"]["PermissionRequest"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["hooks"][0]["command"], format!("\"{EXE}\" hook claude"));
        assert_eq!(status_with(&home, true).0.get("claude"), Some(&true));
    }

    #[test]
    fn claude_needs_both_entries() {
        let home = home("both");
        let bare = json!({"hooks": {
            "PermissionRequest": [{"hooks": [{"type": "command", "command": format!("\"{EXE}\" hook claude")}]}]
        }});
        write(&home.join(".claude/settings.json"), &bare.to_string());
        assert_eq!(status_with(&home, true).0.get("claude"), Some(&false));
    }

    #[test]
    fn command_quotes_the_exe() {
        assert_eq!(command(Path::new("/a\"b$c/island-bridge"), "claude"), r#""/a\"b\$c/island-bridge" hook claude"#);
    }

    const THIRD_PARTY: &str = r#"{
  "hooks": {
    "PermissionRequest": [
      { "hooks": [ { "type": "command", "command": "/usr/local/bin/guard check", "timeout": 30 } ] },
      { "hooks": [
          { "type": "command", "command": "/opt/test/bin/island-bridge hook claude", "timeout": 60 },
          { "type": "command", "command": "/usr/local/bin/audit log", "timeout": 5 }
        ] },
      { "hooks": [ { "type": "command", "command": "/usr/local/bin/notify ask", "timeout": 15 } ] }
    ]
  }
}
"#;

    #[test]
    fn third_party_entries_survive_install_and_remove() {
        let home = home("third-party");
        let claude = home.join(".claude/settings.json");
        write(&claude, THIRD_PARTY);
        let exe = Path::new(EXE);

        install_with(&home, exe, true).unwrap();
        let once = fs::read(&claude).unwrap();
        install_with(&home, exe, true).unwrap();
        assert_eq!(fs::read(&claude).unwrap(), once, "install is byte-idempotent");

        let v = parse(&claude);
        let list = v["hooks"]["PermissionRequest"].as_array().unwrap();
        let commands: Vec<&str> = list
            .iter()
            .flat_map(|g| g["hooks"].as_array().unwrap().iter())
            .filter_map(|h| h["command"].as_str())
            .filter(|c| !c.contains("island-bridge"))
            .collect();
        assert_eq!(commands, ["/usr/local/bin/guard check", "/usr/local/bin/audit log", "/usr/local/bin/notify ask"]);
        // the mixed group keeps its third-party hook and loses ours
        assert_eq!(list[2]["hooks"], json!([{"type": "command", "command": "/usr/local/bin/audit log", "timeout": 5}]));

        remove(&home).unwrap();
        let mut expected: Value = serde_json::from_str(THIRD_PARTY).unwrap();
        expected["hooks"]["PermissionRequest"][1]["hooks"] =
            json!([{"type": "command", "command": "/usr/local/bin/audit log", "timeout": 5}]);
        assert_eq!(parse(&claude), expected);
    }
}
