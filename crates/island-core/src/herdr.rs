//! herdr socket protocol (herdr 0.9.5): parsing the lines the bridge relays, and building requests.
//!
//! Every field herdr sends is treated as optional except `pane_id`; unknown statuses,
//! events and result types never fail a parse.

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentInfo {
    pub pane_id: String,
    pub workspace_id: String,
    pub agent: Option<String>,
    pub name: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub status: AgentStatus,
}

#[derive(Debug, PartialEq)]
pub enum Incoming {
    Bridge { connected: bool },
    AgentList { id: String, agents: Vec<AgentInfo> },
    AgentGet { id: String, agent: AgentInfo },
    /// Any other success response.
    Ok { id: String },
    Error { id: String, message: String },
    Detected { pane_id: String },
    StatusChanged { pane_id: String, status: AgentStatus, agent: Option<String>, title: Option<String> },
    Closed { pane_id: String },
    /// Valid JSON we don't use.
    Other,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn status(v: &Value) -> AgentStatus {
    v.get("agent_status")
        .and_then(|s| AgentStatus::deserialize(s).ok())
        .unwrap_or(AgentStatus::Unknown)
}

fn agent_info(v: &Value) -> Option<AgentInfo> {
    Some(AgentInfo {
        pane_id: text(v, "pane_id")?,
        workspace_id: text(v, "workspace_id").unwrap_or_default(),
        agent: text(v, "agent"),
        name: text(v, "name"),
        title: text(v, "terminal_title_stripped"),
        cwd: text(v, "cwd"),
        status: status(v),
    })
}

fn event(kind: &str, data: &Value) -> Incoming {
    // herdr names some events `pane.x` and others `pane_x` on the wire.
    let kind = kind.replace('.', "_");
    let Some(pane_id) = text(data, "pane_id") else { return Incoming::Other };
    match kind.as_str() {
        "pane_agent_detected" => Incoming::Detected { pane_id },
        "pane_closed" => Incoming::Closed { pane_id },
        "pane_agent_status_changed" => Incoming::StatusChanged {
            pane_id,
            status: status(data),
            agent: text(data, "display_agent").or_else(|| text(data, "agent")),
            title: text(data, "terminal_title_stripped"),
        },
        _ => Incoming::Other,
    }
}

/// Parses one line from the bridge. Only invalid JSON is an error.
pub fn parse_line(line: &str) -> Result<Incoming, serde_json::Error> {
    let v: Value = serde_json::from_str(line)?;
    if let Some(b) = v.get("bridge").and_then(Value::as_str) {
        return Ok(Incoming::Bridge { connected: b == "connected" });
    }
    if let Some(kind) = v.get("event").and_then(Value::as_str) {
        return Ok(event(kind, v.get("data").unwrap_or(&Value::Null)));
    }
    let id = text(&v, "id").unwrap_or_default();
    if let Some(err) = v.get("error") {
        let message = text(err, "message").or_else(|| text(err, "code")).unwrap_or_default();
        return Ok(Incoming::Error { id, message });
    }
    let Some(result) = v.get("result") else { return Ok(Incoming::Other) };
    Ok(match result.get("type").and_then(Value::as_str) {
        Some("agent_list") => Incoming::AgentList {
            id,
            agents: result
                .get("agents")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(agent_info).collect())
                .unwrap_or_default(),
        },
        Some("agent_info") => match result.get("agent").and_then(agent_info) {
            Some(agent) => Incoming::AgentGet { id, agent },
            None => Incoming::Ok { id },
        },
        _ => Incoming::Ok { id },
    })
}

fn request(id: &str, method: &str, params: Value) -> String {
    let mut s = json!({ "id": id, "method": method, "params": params }).to_string();
    s.push('\n');
    s
}

pub fn req_agent_list(id: &str) -> String {
    request(id, "agent.list", json!({}))
}

pub fn req_agent_get(id: &str, pane_id: &str) -> String {
    request(id, "agent.get", json!({ "target": pane_id }))
}

pub fn req_agent_focus(id: &str, pane_id: &str) -> String {
    request(id, "agent.focus", json!({ "target": pane_id }))
}

/// The island's one subscription: new agents, closed panes, and the status of each known pane.
/// Each call replaces the previous subscription (the bridge closes the old connection).
pub fn req_subscribe(id: &str, pane_ids: &[&str]) -> String {
    let mut subs = vec![json!({ "type": "pane.agent_detected" }), json!({ "type": "pane.closed" })];
    subs.extend(pane_ids.iter().map(|p| json!({ "type": "pane.agent_status_changed", "pane_id": p })));
    request(id, "events.subscribe", json!({ "subscriptions": subs }))
}
