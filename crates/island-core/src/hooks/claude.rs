//! Claude Code hooks: PermissionRequest for approvals, PreToolUse for AskUserQuestion.

use super::{cap, cap_to, lines, MAX_TOOL_CHARS, rel, sanitize, trim_command, HookInput};
use crate::approvals::{ApprovalRequest, Decision, Detail, FileChange};
use serde_json::{json, Value};

/// Inputs `parse` drops on purpose: Claude's permission event for AskUserQuestion (ruling R12).
pub fn is_ignored(input: &Value) -> bool {
    input.get("hook_event_name").and_then(Value::as_str) == Some("PermissionRequest")
        && input.get("tool_name").and_then(Value::as_str) == Some("AskUserQuestion")
}

pub fn parse(input: &Value) -> Option<HookInput> {
    let event = input.get("hook_event_name")?.as_str()?;
    let tool = input.get("tool_name")?.as_str()?;
    match (event, tool) {
        ("PreToolUse", "AskUserQuestion") => question(input).map(HookInput::Question),
        // The same question also arrives through the blocking PreToolUse event, so this one is ignored.
        ("PermissionRequest", "AskUserQuestion") => None,
        ("PermissionRequest", _) => approval(input, tool).map(HookInput::Approval),
        _ => None,
    }
}

/// The JSON Claude reads from the hook's stdout. None prints nothing, so Claude shows its own dialog.
pub fn output(decision: Decision, input: &Value) -> Option<String> {
    let decision = match decision {
        Decision::Allow => json!({"behavior": "allow"}),
        Decision::Deny => json!({"behavior": "deny"}),
        Decision::Always => match suggestions(input) {
            Some(s) => json!({"behavior": "allow", "updatedPermissions": s}),
            None => json!({"behavior": "allow"}),
        },
        Decision::None => return None,
    };
    Some(json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": decision}}).to_string())
}

/// Claude's offered "always allow" rules, when there are any.
fn suggestions(input: &Value) -> Option<&Value> {
    input.get("permission_suggestions").filter(|s| s.as_array().is_some_and(|a| !a.is_empty()))
}

fn question(input: &Value) -> Option<String> {
    let questions = input.pointer("/tool_input/questions")?.as_array()?;
    let first = cap(&sanitize(questions.first()?.get("question")?.as_str()?, false));
    let more = questions.len() - 1;
    Some(if more > 0 { format!("{first} (+{more} more)") } else { first })
}

fn approval(input: &Value, tool: &str) -> Option<ApprovalRequest> {
    let ti = input.get("tool_input").unwrap_or(&Value::Null);
    let cwd = input.get("cwd").and_then(Value::as_str);
    let can_always = suggestions(input).is_some();
    let tool = cap_to(&sanitize(tool, false), MAX_TOOL_CHARS);
    let (summary, detail) = detail(&tool, ti, cwd).unwrap_or_else(|| (format!("Wants to use {tool}"), None));
    Some(ApprovalRequest { agent: "claude".to_owned(), tool, summary, detail, can_always })
}

/// The summary and detail for a tool whose required input fields are all present. None falls back to the generic line.
fn detail(tool: &str, ti: &Value, cwd: Option<&str>) -> Option<(String, Option<Detail>)> {
    let text = |k: &str| ti.get(k).and_then(Value::as_str);
    match tool {
        "Edit" => {
            let (path, old, new) = (text("file_path")?, text("old_string")?, text("new_string")?);
            let change = FileChange { path: rel(path, cwd), added: lines(new), removed: lines(old) };
            Some(("Wants to edit 1 file".to_owned(), Some(Detail::Files(vec![change]))))
        }
        "Write" => {
            let (path, content) = (text("file_path")?, text("content")?);
            let change = FileChange { path: rel(path, cwd), added: lines(content), removed: 0 };
            Some(("Wants to edit 1 file".to_owned(), Some(Detail::Files(vec![change]))))
        }
        "NotebookEdit" => {
            let path = text("notebook_path")?;
            let added = if text("edit_mode") == Some("delete") { 0 } else { lines(text("new_source")?) };
            let change = FileChange { path: rel(path, cwd), added, removed: 0 };
            Some(("Wants to edit 1 file".to_owned(), Some(Detail::Files(vec![change]))))
        }
        "Bash" => {
            let command = text("command")?;
            Some(("Wants to run a command".to_owned(), Some(Detail::Command(trim_command(command)))))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{is_ignored, output, parse, trim_command, Agent, HookInput};
    use crate::approvals::{Decision, Detail, FileChange};
    use serde_json::{json, Value};

    fn fx(name: &str) -> Value {
        let text = match name {
            "claude_edit" => include_str!("../../tests/fixtures/hooks/claude_edit.json"),
            "claude_write" => include_str!("../../tests/fixtures/hooks/claude_write.json"),
            "claude_bash" => include_str!("../../tests/fixtures/hooks/claude_bash.json"),
            "claude_webfetch" => include_str!("../../tests/fixtures/hooks/claude_webfetch.json"),
            "claude_question" => include_str!("../../tests/fixtures/hooks/claude_question.json"),
            "claude_question_permission" => {
                include_str!("../../tests/fixtures/hooks/claude_question_permission.json")
            }
            "claude_other_event" => include_str!("../../tests/fixtures/hooks/claude_other_event.json"),
            "claude_read" => include_str!("../../tests/fixtures/hooks/claude_read.json"),
            _ => panic!("no fixture named {name}"),
        };
        serde_json::from_str(text).unwrap()
    }

    fn approval_of(i: &Value) -> crate::approvals::ApprovalRequest {
        let Some(HookInput::Approval(r)) = parse(Agent::Claude, i) else { panic!() };
        r
    }

    #[test]
    fn edit_counts_lines() {
        // claude_edit.json; FileChange path is relative to the input's cwd
        let r = approval_of(&fx("claude_edit"));
        assert_eq!(r.agent, "claude");
        assert_eq!(r.tool, "Edit");
        assert_eq!(r.summary, "Wants to edit 1 file");
        assert!(matches!(r.detail, Some(Detail::Files(ref f)) if f.len() == 1 && !f[0].path.starts_with('/')));
        let Some(Detail::Files(f)) = r.detail else { panic!() };
        assert_eq!(f[0], FileChange { path: "notes.txt".into(), added: 1, removed: 1 });
    }

    #[test]
    fn write_counts_content_as_added() {
        let r = approval_of(&fx("claude_write"));
        assert_eq!(r.tool, "Write");
        assert_eq!(r.summary, "Wants to edit 1 file");
        assert_eq!(
            r.detail,
            Some(Detail::Files(vec![FileChange { path: "hello.txt".into(), added: 2, removed: 0 }]))
        );
    }

    #[test]
    fn notebook_edits_count_cells() {
        let mut i = json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "NotebookEdit",
            "cwd": "/home/u/p",
            "tool_input": {"notebook_path": "/home/u/p/nb.ipynb", "new_source": "a\nb\n", "edit_mode": "replace"}
        });
        let r = approval_of(&i);
        assert_eq!(r.summary, "Wants to edit 1 file");
        assert_eq!(
            r.detail,
            Some(Detail::Files(vec![FileChange { path: "nb.ipynb".into(), added: 2, removed: 0 }]))
        );
        assert!(!r.can_always);
        i["tool_input"]["edit_mode"] = json!("delete");
        assert_eq!(
            approval_of(&i).detail,
            Some(Detail::Files(vec![FileChange { path: "nb.ipynb".into(), added: 0, removed: 0 }]))
        );
    }

    #[test]
    fn missing_required_field_falls_back_to_generic() {
        let mut i = fx("claude_edit");
        i["tool_input"].as_object_mut().unwrap().remove("new_string");
        let r = approval_of(&i);
        assert_eq!(r.summary, "Wants to use Edit");
        assert_eq!(r.detail, None);
        assert!(r.can_always);

        let mut b = fx("claude_bash");
        b["tool_input"].as_object_mut().unwrap().remove("command");
        assert_eq!(approval_of(&b).summary, "Wants to use Bash");
        assert_eq!(approval_of(&b).detail, None);
    }

    #[test]
    fn bash_is_a_command() {
        let i = fx("claude_bash");
        let r = approval_of(&i);
        assert_eq!(r.tool, "Bash");
        assert_eq!(r.summary, "Wants to run a command");
        let cmd = i["tool_input"]["command"].as_str().unwrap();
        assert_eq!(r.detail, Some(Detail::Command(trim_command(cmd))));
    }

    #[test]
    fn other_tools_name_the_tool() {
        let r = approval_of(&fx("claude_webfetch"));
        assert_eq!(r.summary, "Wants to use WebFetch");
        assert_eq!(r.detail, None);
        assert!(r.can_always);
    }

    #[test]
    fn read_is_generic_and_always_allowable() {
        let r = approval_of(&fx("claude_read"));
        assert_eq!(r.tool, "Read");
        assert_eq!(r.summary, "Wants to use Read");
        assert_eq!(r.detail, None);
        assert!(r.can_always);
    }

    #[test]
    fn hostile_path_is_sanitized() {
        let mut i = fx("claude_edit");
        i["tool_input"]["file_path"] = json!("/home/reubinars/projects/demo/evil\u{202E}hs.sh");
        assert_eq!(
            approval_of(&i).detail,
            Some(Detail::Files(vec![FileChange { path: "evil?hs.sh".into(), added: 1, removed: 1 }]))
        );
    }

    #[test]
    fn question_text_and_more() {
        // claude_question: Question(text) == questions[0].question, plus " (+N more)" when questions.len() > 1
        let Some(HookInput::Question(q)) = parse(Agent::Claude, &fx("claude_question")) else { panic!() };
        assert_eq!(q, "Which color do you prefer?");

        let mut i = fx("claude_question");
        let second = json!({"question": "Size?", "header": "Size", "options": [], "multiSelect": false});
        i["tool_input"]["questions"].as_array_mut().unwrap().push(second.clone());
        i["tool_input"]["questions"].as_array_mut().unwrap().push(second);
        let Some(HookInput::Question(q)) = parse(Agent::Claude, &i) else { panic!() };
        assert_eq!(q, "Which color do you prefer? (+2 more)");
    }

    #[test]
    fn long_question_is_capped_before_the_suffix() {
        let mut i = fx("claude_question");
        i["tool_input"]["questions"][0]["question"] = json!("z".repeat(100_000));
        let second = json!({"question": "Size?", "header": "Size", "options": [], "multiSelect": false});
        i["tool_input"]["questions"].as_array_mut().unwrap().push(second.clone());
        i["tool_input"]["questions"].as_array_mut().unwrap().push(second);
        let Some(HookInput::Question(q)) = parse(Agent::Claude, &i) else { panic!() };
        assert_eq!(q, format!("{}… (+2 more)", "z".repeat(299)));
    }

    #[test]
    fn long_tool_name_is_capped() {
        let mut i = fx("claude_webfetch");
        i["tool_name"] = json!("T".repeat(100));
        let r = approval_of(&i);
        assert_eq!(r.tool.chars().count(), 40);
        assert!(r.tool.ends_with('…'));
    }

    #[test]
    fn question_permission_is_ignored() {
        let i = fx("claude_question_permission");
        assert_eq!(parse(Agent::Claude, &i), None);
        assert!(is_ignored(Agent::Claude, &i));
        // unknown input is unparseable, not ignored
        assert!(!is_ignored(Agent::Claude, &fx("claude_other_event")));
        assert_eq!(parse(Agent::Claude, &fx("claude_other_event")), None);
    }

    #[test]
    fn can_always_follows_suggestions() {
        assert!(approval_of(&fx("claude_edit")).can_always);
        let mut i = fx("claude_edit");
        i.as_object_mut().unwrap().remove("permission_suggestions");
        assert!(!approval_of(&i).can_always);
        i["permission_suggestions"] = json!([]);
        assert!(!approval_of(&i).can_always);
    }

    #[test]
    fn outputs() {
        let i = fx("claude_edit");
        let v = |d| serde_json::from_str::<Value>(&output(Agent::Claude, d, &i).unwrap()).unwrap();
        assert_eq!(v(Decision::Allow)["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert_eq!(v(Decision::Allow)["hookSpecificOutput"]["hookEventName"], "PermissionRequest");
        assert_eq!(v(Decision::Deny)["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert_eq!(
            v(Decision::Always)["hookSpecificOutput"]["decision"]["updatedPermissions"],
            i["permission_suggestions"]
        );
        assert!(v(Decision::Allow)["hookSpecificOutput"]["decision"].get("updatedPermissions").is_none());
        assert_eq!(output(Agent::Claude, Decision::None, &i), None);
    }

    #[test]
    fn always_without_suggestions_is_a_plain_allow() {
        let mut i = fx("claude_edit");
        i["permission_suggestions"] = json!([]);
        let out: Value = serde_json::from_str(&output(Agent::Claude, Decision::Always, &i).unwrap()).unwrap();
        assert_eq!(out["hookSpecificOutput"]["decision"], json!({"behavior": "allow"}));
    }

    #[test]
    fn garbage_is_none() {
        assert_eq!(parse(Agent::Claude, &json!({"x":1})), None);
    }
}
