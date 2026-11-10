//! herdr protocol parsing against real herdr 0.9.5 captures, and request building.

use island_core::herdr::*;

fn fx(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn parses_bridge_status() {
    assert_eq!(parse_line(r#"{"bridge":"connected"}"#).unwrap(), Incoming::Bridge { connected: true });
    assert_eq!(parse_line(r#"{"bridge":"disconnected"}"#).unwrap(), Incoming::Bridge { connected: false });
}

#[test]
fn parses_agent_list_fixture() {
    match parse_line(&fx("agent_list.json")).unwrap() {
        Incoming::AgentList { id, agents } => {
            assert_eq!(id, "x:3");
            assert_eq!(agents.len(), 6);
            let a = &agents[0];
            assert_eq!(a.pane_id, "w1:p1");
            assert_eq!(a.workspace_id, "w1");
            assert_eq!(a.agent.as_deref(), Some("claude"));
            assert_eq!(a.title.as_deref(), Some("Docker and Kubernetes learning guide"));
            assert_eq!(a.cwd.as_deref(), Some("/home/reubinars/projects/learn-anything"));
            assert_eq!(a.status, AgentStatus::Idle);
            assert_eq!(agents[1].name.as_deref(), Some("researcher"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn parses_agent_get_fixture() {
    match parse_line(&fx("agent_get.json")).unwrap() {
        Incoming::AgentGet { id, agent } => {
            assert_eq!(id, "x:4");
            assert_eq!(agent.pane_id, "w3:p7");
            assert_eq!(agent.status, AgentStatus::Idle);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn parses_events() {
    assert_eq!(
        parse_line(&fx("event_status_changed.json")).unwrap(),
        Incoming::StatusChanged {
            pane_id: "w3:p8".into(),
            status: AgentStatus::Working,
            agent: Some("claude".into()),
            title: None
        }
    );
    assert_eq!(parse_line(&fx("event_agent_detected.json")).unwrap(), Incoming::Detected { pane_id: "w3:p7".into() });
    // An agent exiting while its pane stays open: herdr reuses the detected event with `released`.
    assert_eq!(parse_line(&fx("event_agent_released.json")).unwrap(), Incoming::Released { pane_id: "w3:pC".into() });
    assert_eq!(parse_line(&fx("event_pane_closed.json")).unwrap(), Incoming::Closed { pane_id: "w3:p8".into() });
    assert_eq!(parse_line(&fx("subscribe_ack.json")).unwrap(), Incoming::Ok { id: "p:1".into() });
    match parse_line(&fx("error_response.json")).unwrap() {
        Incoming::Error { id, message } => {
            assert_eq!(id, "x:5");
            assert!(message.contains("missing field"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn event_names_accept_dot_or_underscore() {
    let closed = r#"{"data":{"pane_id":"w1:p2"},"event":"pane.closed"}"#;
    assert_eq!(parse_line(closed).unwrap(), Incoming::Closed { pane_id: "w1:p2".into() });
    let status = r#"{"data":{"pane_id":"w1:p2","agent_status":"blocked","agent":"claude","display_agent":"Claude Code","terminal_title_stripped":"fix bug"},"event":"pane_agent_status_changed"}"#;
    assert_eq!(
        parse_line(status).unwrap(),
        Incoming::StatusChanged {
            pane_id: "w1:p2".into(),
            status: AgentStatus::Blocked,
            agent: Some("Claude Code".into()),
            title: Some("fix bug".into())
        }
    );
    assert_eq!(parse_line(r#"{"data":{},"event":"tab_created"}"#).unwrap(), Incoming::Other);
}

#[test]
fn unknown_status_and_missing_fields() {
    let l = r#"{"id":"i:1","result":{"type":"agent_list","agents":[{"pane_id":"w1:p9","workspace_id":"w1","agent_status":"sleeping"}]}}"#;
    let Incoming::AgentList { agents, .. } = parse_line(l).unwrap() else { panic!() };
    assert_eq!(agents[0].status, AgentStatus::Unknown);
    assert_eq!(agents[0].agent, None);
    assert_eq!(agents[0].cwd, None);
    // an entry with no pane_id is skipped rather than failing the whole list
    let l = r#"{"id":"i:2","result":{"type":"agent_list","agents":[{"workspace_id":"w1"},{"pane_id":"w1:p1"}]}}"#;
    let Incoming::AgentList { agents, .. } = parse_line(l).unwrap() else { panic!() };
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].status, AgentStatus::Unknown);
}

#[test]
fn garbage_is_an_error() {
    assert!(parse_line("not json").is_err());
}

#[test]
fn builds_requests() {
    assert_eq!(req_agent_list("island:1"), "{\"id\":\"island:1\",\"method\":\"agent.list\",\"params\":{}}\n");
    assert_eq!(
        req_agent_get("island:2", "w1:p1"),
        "{\"id\":\"island:2\",\"method\":\"agent.get\",\"params\":{\"target\":\"w1:p1\"}}\n"
    );
    assert_eq!(
        req_agent_focus("island:3", "w1:p1"),
        "{\"id\":\"island:3\",\"method\":\"agent.focus\",\"params\":{\"target\":\"w1:p1\"}}\n"
    );
    assert_eq!(
        req_subscribe("island:4", &["w1:p1"]),
        "{\"id\":\"island:4\",\"method\":\"events.subscribe\",\"params\":{\"subscriptions\":[{\"type\":\"pane.agent_detected\"},{\"type\":\"pane.closed\"},{\"type\":\"pane.agent_status_changed\",\"pane_id\":\"w1:p1\"}]}}\n"
    );
    assert_eq!(
        req_subscribe("island:5", &[]),
        "{\"id\":\"island:5\",\"method\":\"events.subscribe\",\"params\":{\"subscriptions\":[{\"type\":\"pane.agent_detected\"},{\"type\":\"pane.closed\"}]}}\n"
    );
}
