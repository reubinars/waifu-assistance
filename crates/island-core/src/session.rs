//! Turns bridge lines into agent-table changes and the requests to send back (spec §4.3).

use crate::agents::AgentTable;
use crate::approvals::{ApprovalQueue, HookStatus};
use crate::herdr::{self, Incoming};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

#[derive(Default)]
pub struct HerdrSession {
    pub agents: AgentTable,
    pub approvals: ApprovalQueue,
    /// The bridge's hook status snapshot. Kept across reconnects: the bridge re-sends it.
    pub hooks: HookStatus,
    /// GitHub PR state; `None` until the bridge first sends a `github` line.
    pub github: Option<crate::github::GhState>,
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
        // approvals stay: the bridge holds their hook connections across herdr link changes
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
                self.reset();
                vec![]
            }
            Incoming::AgentList { agents, .. } => {
                let listed: BTreeSet<String> = agents.iter().map(|a| a.pane_id.clone()).collect();
                self.approvals.retain_panes(&listed);
                for a in &agents {
                    self.approvals.on_status(&a.pane_id, a.status);
                }
                self.agents.replace_all(agents, now);
                if self.agents.is_empty() { vec![] } else { vec![self.subscribe_all()] }
            }
            Incoming::Detected { pane_id } => {
                let id = self.next_id();
                vec![herdr::req_agent_get(&id, &pane_id)]
            }
            Incoming::AgentGet { agent, .. } => {
                let new = !self.sent.contains(&agent.pane_id);
                self.approvals.on_status(&agent.pane_id, agent.status);
                self.agents.upsert(agent, now);
                if new { vec![self.subscribe_all()] } else { vec![] }
            }
            Incoming::StatusChanged { pane_id, status, title, .. } => {
                self.agents.set_status(&pane_id, status, title, now);
                self.approvals.on_status(&pane_id, status);
                vec![]
            }
            // A stale status subscription for a closed or released pane is harmless; no resubscribe.
            // A released pane keeps its subscription, which still fits if an agent starts there again.
            Incoming::Closed { pane_id } | Incoming::Released { pane_id } => {
                self.agents.remove(&pane_id);
                self.approvals.remove_pane(&pane_id);
                vec![]
            }
            Incoming::Approval { id, pane_id, req, timeout_ms } => {
                self.approvals.insert(id, pane_id, req, timeout_ms, now);
                vec![]
            }
            Incoming::ApprovalClosed { id } => {
                self.approvals.close(&id);
                vec![]
            }
            Incoming::Question { pane_id, text } => {
                self.approvals.question(pane_id, text);
                vec![]
            }
            Incoming::Hooks(hooks) => {
                self.hooks = hooks;
                vec![]
            }
            Incoming::Ok { id } => self.subscription_started(&id),
            Incoming::GitHub(r) => {
                self.github.get_or_insert_with(Default::default).apply(r);
                vec![]
            }
            Incoming::Error { .. } | Incoming::Usage(_) | Incoming::Other => vec![],
        }
    }

    /// The bridge process exited: its hook connections are gone, so the approvals are too.
    pub fn bridge_down(&mut self) {
        self.reset();
        self.approvals.clear();
        if let Some(g) = &mut self.github {
            g.rebaseline();
        }
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
    fn bridge_down_clears_requests() {
        let mut s = HerdrSession::default();
        s.handle(list(&["w1:p1"]), now());
        let req = crate::approvals::ApprovalRequest {
            agent: "claude".into(),
            tool: "Edit".into(),
            summary: "Wants to edit 1 file".into(),
            detail: None,
            can_always: true,
        };
        s.handle(Incoming::Approval { id: "a1".into(), pane_id: "w1:p1".into(), req, timeout_ms: 45000 }, now());
        let shown = s.approvals.pending(&s.agents, &s.hooks);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].pane_id, "w1:p1");
        s.bridge_down();
        assert!(s.approvals.pending(&s.agents, &s.hooks).is_empty());
        assert!(s.approvals.take("a1").is_none());
    }

    #[test]
    fn approval_survives_herdr_reconnect() {
        let mut s = HerdrSession::default();
        let req = crate::approvals::ApprovalRequest {
            agent: "claude".into(),
            tool: "Edit".into(),
            summary: "s".into(),
            detail: None,
            can_always: true,
        };
        s.handle(Incoming::Approval { id: "a1".into(), pane_id: "w1:p1".into(), req, timeout_ms: 45000 }, now());
        s.handle(Incoming::Bridge { connected: false }, now());
        s.handle(Incoming::Bridge { connected: true }, now());
        assert!(s.approvals.has_approval());
        assert!(s.approvals.take("a1").is_some());
    }

    #[test]
    fn agent_list_drops_ghost_questions_but_keeps_approvals() {
        let mut s = HerdrSession::default();
        let req = crate::approvals::ApprovalRequest {
            agent: "claude".into(),
            tool: "Edit".into(),
            summary: "s".into(),
            detail: None,
            can_always: true,
        };
        s.handle(Incoming::AgentList { id: "island:1".into(), agents: vec![info("dead", Blocked)] }, now());
        s.approvals.question("dead".into(), "Which?".into());
        s.handle(Incoming::Approval { id: "a1".into(), pane_id: "dead".into(), req, timeout_ms: 45000 }, now());
        s.handle(Incoming::Bridge { connected: false }, now());
        s.handle(Incoming::Bridge { connected: true }, now());
        s.handle(list(&["other"]), now());
        // 1 approval only: the question for the unlisted pane is gone
        assert_eq!(s.approvals.badge(&s.agents), 1);
        assert!(s.approvals.has_approval());
        assert!(s.approvals.take("a1").is_some());
    }

    #[test]
    fn agent_list_feeds_blocked_so_a_repeat_keeps_later() {
        let mut s = HerdrSession::default();
        s.handle(
            Incoming::AgentList { id: "island:1".into(), agents: vec![info("p", Blocked)] },
            now(),
        );
        s.approvals.question("p".into(), "Which?".into());
        s.approvals.later("p");
        s.handle(
            Incoming::StatusChanged { pane_id: "p".into(), status: Blocked, agent: None, title: None },
            now(),
        );
        // still hidden by Later: the repeat was not a fresh entry into Blocked
        assert!(s.approvals.pending(&s.agents, &s.hooks).is_empty());
        assert_eq!(s.approvals.badge(&s.agents), 1);
    }

    #[test]
    fn ids_increase() {
        let mut s = HerdrSession::default();
        let out = s.handle(Incoming::Bridge { connected: true }, now());
        assert!(out[0].contains("\"id\":\"island:1\""));
        assert!(out[1].contains("\"id\":\"island:2\""));
        assert!(s.focus("w1:p1").contains("\"id\":\"island:3\""));
    }

    fn gh(n: u32, ci: crate::github::Ci) -> crate::github::PullRequest {
        crate::github::test_pr("o/a", n, crate::github::Role::Mine, ci, crate::github::Review::None)
    }

    #[test]
    fn github_message_creates_state() {
        let mut s = HerdrSession::default();
        assert!(s.github.is_none());
        s.handle(crate::herdr::parse_line(r#"{"github":{"prs":[]}}"#).unwrap(), now());
        assert!(s.github.is_some());
    }

    #[test]
    fn bridge_down_rebaselines() {
        let mut s = HerdrSession::default();
        s.handle(Incoming::GitHub(Ok(vec![gh(1, crate::github::Ci::Running)])), now());
        s.bridge_down();
        s.handle(Incoming::GitHub(Ok(vec![gh(1, crate::github::Ci::Failing)])), now());
        let g = s.github.as_mut().unwrap();
        assert_eq!(g.unseen, 0);
        assert_eq!(g.next_pop(), None);
    }

    #[test]
    fn herdr_reconnect_keeps_github() {
        let mut s = HerdrSession::default();
        s.handle(Incoming::GitHub(Ok(vec![gh(1, crate::github::Ci::Running)])), now());
        s.handle(Incoming::Bridge { connected: true }, now());
        assert_eq!(s.github.as_ref().unwrap().prs.len(), 1);
    }
}
