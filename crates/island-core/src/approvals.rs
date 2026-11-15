//! Agent approval wire types: what an agent asks the island to approve, and the user's answer.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
    Always,
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub added: u32,
    pub removed: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Detail {
    Files(Vec<FileChange>),
    Command(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub agent: String,
    pub tool: String,
    pub summary: String,
    pub detail: Option<Detail>,
    pub can_always: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wire_shapes() {
        assert_eq!(serde_json::to_string(&Decision::Always).unwrap(), "\"always\"");
        assert_eq!(serde_json::to_value(Detail::Command("ls".into())).unwrap(), json!({"command":"ls"}));
        assert_eq!(
            serde_json::to_value(Detail::Files(vec![FileChange { path: "a".into(), added: 1, removed: 0 }])).unwrap(),
            json!({"files":[{"path":"a","added":1,"removed":0}]})
        );
        let r: ApprovalRequest = serde_json::from_value(
            json!({"agent":"codex","tool":"shell","summary":"s","detail":null,"can_always":false}),
        )
        .unwrap();
        assert_eq!(r.detail, None);
    }
}
