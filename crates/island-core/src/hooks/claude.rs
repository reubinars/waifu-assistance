//! Claude Code hooks: PermissionRequest for approvals, PreToolUse for AskUserQuestion.

use super::{lines, rel, trim_command, HookInput};
use crate::approvals::{ApprovalRequest, Decision, Detail, FileChange};
use serde_json::{json, Value};

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
        Decision::Always => {
            let mut d = json!({"behavior": "allow"});
            if let Some(s) = input.get("permission_suggestions") {
                d["updatedPermissions"] = s.clone();
            }
            d
        }
        Decision::None => return None,
    };
    Some(json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": decision}}).to_string())
}

fn question(input: &Value) -> Option<String> {
    let questions = input.pointer("/tool_input/questions")?.as_array()?;
    let first = questions.first()?.get("question")?.as_str()?;
    let more = questions.len() - 1;
    Some(if more > 0 { format!("{first} (+{more} more)") } else { first.to_owned() })
}

fn approval(input: &Value, tool: &str) -> Option<ApprovalRequest> {
    let ti = input.get("tool_input")?;
    let cwd = input.get("cwd").and_then(Value::as_str);
    let can_always = input.get("permission_suggestions").and_then(Value::as_array).is_some_and(|a| !a.is_empty());
    let (summary, detail) = match tool {
        "Edit" => {
            let path = ti.get("file_path")?.as_str()?;
            let old = ti.get("old_string").and_then(Value::as_str).unwrap_or("");
            let new = ti.get("new_string").and_then(Value::as_str).unwrap_or("");
            (
                "Wants to edit 1 file".to_owned(),
                Some(Detail::Files(vec![FileChange {
                    path: rel(path, cwd),
                    added: lines(new),
                    removed: lines(old),
                }])),
            )
        }
        "Write" => {
            let path = ti.get("file_path")?.as_str()?;
            let content = ti.get("content").and_then(Value::as_str).unwrap_or("");
            (
                "Wants to write 1 file".to_owned(),
                Some(Detail::Files(vec![FileChange { path: rel(path, cwd), added: lines(content), removed: 0 }])),
            )
        }
        "Bash" => {
            let command = ti.get("command")?.as_str()?;
            ("Wants to run a command".to_owned(), Some(Detail::Command(trim_command(command))))
        }
        _ => (format!("Wants to use {tool}"), None),
    };
    Some(ApprovalRequest { agent: "claude".to_owned(), tool: tool.to_owned(), summary, detail, can_always })
}

#[cfg(test)]
mod tests {
    use super::super::{output, parse, trim_command, Agent, HookInput};
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
            "claude_read" => include_str!("../../tests/fixtures/hooks/claude_read.json"),
            _ => panic!("no fixture named {name}"),
        };
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn edit_counts_lines() {
        // claude_edit.json; FileChange path is relative to the input's cwd
        let Some(HookInput::Approval(r)) = parse(Agent::Claude, &fx("claude_edit")) else { panic!() };
        assert_eq!(r.agent, "claude");
        assert_eq!(r.tool, "Edit");
        assert_eq!(r.summary, "Wants to edit 1 file");
        assert!(matches!(r.detail, Some(Detail::Files(ref f)) if f.len() == 1 && !f[0].path.starts_with('/')));
        let Some(Detail::Files(f)) = r.detail else { panic!() };
        assert_eq!(f[0], FileChange { path: "notes.txt".into(), added: 1, removed: 1 });
    }

    #[test]
    fn write_counts_content_as_added() {
        let Some(HookInput::Approval(r)) = parse(Agent::Claude, &fx("claude_write")) else { panic!() };
        assert_eq!(r.tool, "Write");
        assert_eq!(r.summary, "Wants to write 1 file");
        assert_eq!(
            r.detail,
            Some(Detail::Files(vec![FileChange { path: "hello.txt".into(), added: 2, removed: 0 }]))
        );
    }

    #[test]
    fn bash_is_a_command() {
        let i = fx("claude_bash");
        let Some(HookInput::Approval(r)) = parse(Agent::Claude, &i) else { panic!() };
        assert_eq!(r.tool, "Bash");
        assert_eq!(r.summary, "Wants to run a command");
        let cmd = i["tool_input"]["command"].as_str().unwrap();
        assert_eq!(r.detail, Some(Detail::Command(trim_command(cmd))));
    }

    #[test]
    fn other_tools_name_the_tool() {
        let Some(HookInput::Approval(r)) = parse(Agent::Claude, &fx("claude_webfetch")) else { panic!() };
        assert_eq!(r.summary, "Wants to use WebFetch");
        assert_eq!(r.detail, None);
        assert!(r.can_always);
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
    fn question_permission_is_ignored() {
        assert_eq!(parse(Agent::Claude, &fx("claude_question_permission")), None);
    }

    #[test]
    fn can_always_follows_suggestions() {
        assert!(can_always_of(&fx("claude_edit")));
        let mut i = fx("claude_edit");
        i.as_object_mut().unwrap().remove("permission_suggestions");
        assert!(!can_always_of(&i));
        i["permission_suggestions"] = json!([]);
        assert!(!can_always_of(&i));
    }

    fn can_always_of(i: &Value) -> bool {
        let Some(HookInput::Approval(r)) = parse(Agent::Claude, i) else { panic!() };
        r.can_always
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
    fn garbage_is_none() {
        assert_eq!(parse(Agent::Claude, &json!({"x":1})), None);
    }
}
