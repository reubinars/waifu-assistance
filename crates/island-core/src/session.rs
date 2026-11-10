//! Turns bridge lines into agent-table changes and the requests to send back (spec §4.3).

use crate::agents::AgentTable;
use crate::herdr::{self, Incoming};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

#[derive(Default)]
pub struct HerdrSession {
    pub agents: AgentTable,
    last_id: u64,
    /// Panes in the most recently sent subscription.
    sent: BTreeSet<String>,
    /// Panes whose status subscription herdr has acknowledged.
    acked: BTreeSet<String>,
    /// Subscribe requests awaiting herdr's ack, by id.
    pending: BTreeMap<String, BTreeSet<String>>,
}

impl HerdrSession {
    fn next_id(&mut self) -> String {
        self.last_id += 1;
        format!("island:{}", self.last_id)
    }

    /// Subscription covering every known pane; replaces the previous one.
    fn subscribe_all(&mut self) -> String {
        let id = self.next_id();
        let panes: BTreeSet<String> = self.agents.pane_ids().into_iter().collect();
        let refs: Vec<&str> = panes.iter().map(String::as_str).collect();
        let req = herdr::req_subscribe(&id, &refs);
        self.sent = panes.clone();
        self.pending.insert(id, panes);
        req
    }

    /// Once a subscription is live, re-fetch the panes it newly covers: their status may
    /// have changed between the snapshot and the subscription taking effect.
    fn subscription_started(&mut self, id: &str) -> Vec<String> {
        let Some(panes) = self.pending.remove(id) else { return vec![] };
        let fresh: Vec<String> = panes.difference(&self.acked).cloned().collect();
        self.acked = panes;
        fresh
            .iter()
            .map(|pane| {
                let id = self.next_id();
                herdr::req_agent_get(&id, pane)
            })
            .collect()
    }

    fn reset(&mut self) {
        self.agents.clear();
        self.sent.clear();
        self.acked.clear();
        self.pending.clear();
    }

    /// Applies one bridge line; returns the requests to write to the bridge, in order.
    pub fn handle(&mut self, msg: Incoming, now: Instant) -> Vec<String> {
        match msg {
            Incoming::Bridge { connected: true } => {
                self.reset();
                let sub = self.subscribe_all();
                let id = self.next_id();
                vec![sub, herdr::req_agent_list(&id)]
            }
            Incoming::Bridge { connected: false } => {
                self.bridge_down();
                vec![]
            }
            Incoming::AgentList { agents, .. } => {
                self.agents.replace_all(agents, now);
                if self.agents.is_empty() { vec![] } else { vec![self.subscribe_all()] }
            }
            Incoming::Detected { pane_id } => {
                let id = self.next_id();
                vec![herdr::req_agent_get(&id, &pane_id)]
            }
            Incoming::AgentGet { agent, .. } => {
                let new = !self.sent.contains(&agent.pane_id);
                self.agents.upsert(agent, now);
                if new { vec![self.subscribe_all()] } else { vec![] }
            }
            Incoming::StatusChanged { pane_id, status, title, .. } => {
                self.agents.set_status(&pane_id, status, title, now);
                vec![]
            }
            // A stale status subscription for a closed or released pane is harmless; no resubscribe.
            // A released pane keeps its subscription, which still fits if an agent starts there again.
            Incoming::Closed { pane_id } | Incoming::Released { pane_id } => {
                self.agents.remove(&pane_id);
                vec![]
            }
            Incoming::Ok { id } => self.subscription_started(&id),
            Incoming::Error { .. } | Incoming::Other => vec![],
        }
    }

    pub fn bridge_down(&mut self) {
        self.reset();
    }

    pub fn focus(&mut self, pane_id: &str) -> String {
        let id = self.next_id();
        herdr::req_agent_focus(&id, pane_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::test_info as info;
    use crate::herdr::{AgentStatus::*, Incoming};
    use std::time::Instant;

    fn now() -> Instant {
        Instant::now()
    }

    fn list(panes: &[&str]) -> Incoming {
        Incoming::AgentList { id: "island:1".into(), agents: panes.iter().map(|p| info(p, Idle)).collect() }
    }

    #[test]
    fn connect_resyncs() {
        let mut s = HerdrSession::default();
        s.agents.upsert(info("w1:p7", Blocked), now());
        let out = s.handle(Incoming::Bridge { connected: true }, now());
        assert!(s.agents.is_empty());
        assert_eq!(out.len(), 2);
        // subscribe before listing, so nothing detected in between is missed
        assert!(out[0].contains("\"events.subscribe\"") && out[0].contains("pane.agent_detected"));
        assert!(!out[0].contains("pane.agent_status_changed"));
        assert!(out[1].contains("\"agent.list\""));
    }

    #[test]
    fn list_subscribes_every_pane() {
        let mut s = HerdrSession::default();
        let out = s.handle(list(&["w1:p1", "w1:p2"]), now());
        assert_eq!(out.len(), 1);
        assert!(out[0].contains("\"events.subscribe\""));
        assert!(out[0].contains("pane.closed"));
        assert!(out[0].contains("\"pane_id\":\"w1:p1\"") && out[0].contains("\"pane_id\":\"w1:p2\""));
        assert!(s.handle(list(&[]), now()).is_empty());
    }

    #[test]
    fn detected_then_get() {
        let mut s = HerdrSession::default();
        s.handle(list(&["w1:p1"]), now());
        let out = s.handle(Incoming::Detected { pane_id: "w1:p3".into() }, now());
        assert_eq!(out.len(), 1);
        assert!(out[0].contains("\"agent.get\"") && out[0].contains("w1:p3"));
        let out = s.handle(Incoming::AgentGet { id: "island:3".into(), agent: info("w1:p3", Working) }, now());
        assert_eq!(s.agents.pane_ids(), vec!["w1:p1".to_string(), "w1:p3".to_string()]);
        // the replacement subscription covers every known pane, not just the new one
        assert_eq!(out.len(), 1);
        assert!(out[0].contains("\"pane_id\":\"w1:p1\"") && out[0].contains("\"pane_id\":\"w1:p3\""));
    }

    #[test]
    fn status_and_close() {
        let mut s = HerdrSession::default();
        s.handle(list(&["w1:p1"]), now());
        let out = s.handle(
            Incoming::StatusChanged { pane_id: "w1:p1".into(), status: Blocked, agent: None, title: None },
            now(),
        );
        assert!(out.is_empty());
        assert_eq!(s.agents.first(Blocked).unwrap().info.pane_id, "w1:p1");
        assert!(s.handle(Incoming::Closed { pane_id: "w1:p1".into() }, now()).is_empty());
        assert!(s.agents.is_empty());
        assert!(s.handle(Incoming::Error { id: "island:9".into(), message: "x".into() }, now()).is_empty());
    }

    #[test]
    fn released_agent_is_dropped_while_its_pane_stays() {
        let mut s = HerdrSession::default();
        s.handle(list(&["w1:p1", "w1:p2"]), now());
        // no agent.get: herdr answers agent_not_found for a released pane
        assert!(s.handle(Incoming::Released { pane_id: "w1:p1".into() }, now()).is_empty());
        assert_eq!(s.agents.pane_ids(), vec!["w1:p2".to_string()]);
    }

    #[test]
    fn herdr_restart_leaves_no_ghosts() {
        let mut s = HerdrSession::default();
        s.handle(list(&["w1:p1", "w1:p2"]), now());
        assert!(s.handle(Incoming::Bridge { connected: false }, now()).is_empty());
        assert!(s.agents.is_empty());
        s.handle(Incoming::Bridge { connected: true }, now());
        s.handle(list(&["w1:p2"]), now());
        assert_eq!(s.agents.pane_ids(), vec!["w1:p2".to_string()]);
    }

    fn id_of(req: &str) -> String {
        serde_json::from_str::<serde_json::Value>(req).unwrap()["id"].as_str().unwrap().to_owned()
    }

    #[test]
    fn subscribe_ack_refetches_newly_covered_panes() {
        // a status change between the snapshot and the subscription taking effect is caught up
        let mut s = HerdrSession::default();
        let sub = s.handle(list(&["w1:p1", "w1:p2"]), now()).remove(0);
        let out = s.handle(Incoming::Ok { id: id_of(&sub) }, now());
        assert_eq!(out.len(), 2);
        assert!(out[0].contains("\"agent.get\"") && out[0].contains("w1:p1"));
        assert!(out[1].contains("\"agent.get\"") && out[1].contains("w1:p2"));
        assert!(s.handle(Incoming::Ok { id: id_of(&sub) }, now()).is_empty());
        assert!(s.handle(Incoming::Ok { id: "island:99".into() }, now()).is_empty());
    }

    #[test]
    fn agent_get_resubscribes_only_for_new_panes() {
        let mut s = HerdrSession::default();
        let sub = s.handle(list(&["w1:p1"]), now()).remove(0);
        s.handle(Incoming::Ok { id: id_of(&sub) }, now());
        // the refetch of a covered pane does not resubscribe
        assert!(s.handle(Incoming::AgentGet { id: "x".into(), agent: info("w1:p1", Working) }, now()).is_empty());
        let sub = s.handle(Incoming::AgentGet { id: "y".into(), agent: info("w1:p3", Working) }, now()).remove(0);
        let out = s.handle(Incoming::Ok { id: id_of(&sub) }, now());
        assert_eq!(out.len(), 1);
        assert!(out[0].contains("w1:p3"));
    }

    #[test]
    fn ids_increase() {
        let mut s = HerdrSession::default();
        let out = s.handle(Incoming::Bridge { connected: true }, now());
        assert!(out[0].contains("\"id\":\"island:1\""));
        assert!(out[1].contains("\"id\":\"island:2\""));
        assert!(s.focus("w1:p1").contains("\"id\":\"island:3\""));
    }
}
