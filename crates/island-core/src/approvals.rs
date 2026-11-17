//! Agent approval wire types: what an agent asks the island to approve, and the user's answer.

use crate::agents::{Agent, AgentTable};
use crate::herdr::AgentStatus;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

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
    /// What "always" would grant; absent from older bridges, which read as a saved rule.
    #[serde(default)]
    pub scope: AlwaysScope,
}

/// What the agent's offered "always allow" actually grants, so the button can say so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlwaysScope {
    /// A rule saved to the agent's settings: it outlives this session.
    #[default]
    Saved,
    /// A rule that lasts until the agent session ends.
    Session,
    /// Switches the session to accepting file edits without asking.
    AcceptEdits,
}

/// An approval the island is showing, and when the agent stops waiting for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: String,
    pub pane_id: String,
    pub req: ApprovalRequest,
    pub expires: Instant,
    /// The wire timeout the countdown is measured against.
    pub timeout_ms: u64,
}

/// Whether each agent CLI's hook is installed, by herdr agent name. A missing key means the CLI isn't installed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct HookStatus(pub BTreeMap<String, bool>);

#[derive(Clone, Debug, PartialEq)]
pub enum PendingKind {
    Approval(Request),
    Question(String),
    NeedsYou { hook_missing: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub pane_id: String,
    pub kind: PendingKind,
}

/// How long before the advertised timeout the island stops offering an approval.
pub const EXPIRY_MARGIN_MS: u64 = 1000;

struct Asked {
    pane_id: String,
    text: String,
}

/// What the island is waiting on the user for, across panes.
#[derive(Default)]
pub struct ApprovalQueue {
    /// In arrival order.
    approvals: Vec<Request>,
    /// At most one per pane, in arrival order.
    questions: Vec<Asked>,
    /// Panes whose question the user chose to answer later.
    later: BTreeSet<String>,
    /// Panes that are Blocked, as the queue has seen their status changes.
    blocked: BTreeSet<String>,
}

impl ApprovalQueue {
    pub fn insert(&mut self, id: String, pane_id: String, req: ApprovalRequest, timeout_ms: u64, now: Instant) {
        let expires = now + Duration::from_millis(timeout_ms);
        self.approvals.push(Request { id, pane_id, req, expires, timeout_ms });
    }

    /// Replaces the pane's earlier question, if any.
    pub fn question(&mut self, pane_id: String, text: String) {
        self.questions.retain(|q| q.pane_id != pane_id);
        self.questions.push(Asked { pane_id, text });
    }

    pub fn close(&mut self, id: &str) {
        self.approvals.retain(|r| r.id != id);
    }

    /// Removes and returns the approval with this id, to answer it.
    pub fn take(&mut self, id: &str) -> Option<Request> {
        let i = self.approvals.iter().position(|r| r.id == id)?;
        Some(self.approvals.remove(i))
    }

    /// Whether any approval is waiting.
    pub fn has_approval(&self) -> bool {
        !self.approvals.is_empty()
    }

    pub fn later(&mut self, pane_id: &str) {
        self.later.insert(pane_id.to_owned());
    }

    /// Leaving Blocked drops the pane's question; entering Blocked shows questions again.
    pub fn on_status(&mut self, pane_id: &str, status: AgentStatus) {
        if status == AgentStatus::Blocked {
            if self.blocked.insert(pane_id.to_owned()) {
                self.later.remove(pane_id);
            }
        } else if self.blocked.remove(pane_id) {
            self.questions.retain(|q| q.pane_id != pane_id);
        }
    }

    /// Keeps questions, Later and blocked state only for the listed panes: an agent list is the
    /// authoritative snapshot, and closes missed across a herdr restart would leave ghosts.
    /// Approvals stay; their timeout and the hook closing already bound them.
    pub fn retain_panes(&mut self, listed: &BTreeSet<String>) {
        self.questions.retain(|q| listed.contains(&q.pane_id));
        self.later.retain(|p| listed.contains(p));
        self.blocked.retain(|p| listed.contains(p));
    }

    pub fn remove_pane(&mut self, pane_id: &str) {
        self.approvals.retain(|r| r.pane_id != pane_id);
        self.questions.retain(|q| q.pane_id != pane_id);
        self.later.remove(pane_id);
        self.blocked.remove(pane_id);
    }

    /// Drops approvals that are within `EXPIRY_MARGIN_MS` of their timeout, so a click can't
    /// arrive after the hook gave up. True if any were removed.
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.approvals.len();
        let margin = Duration::from_millis(EXPIRY_MARGIN_MS);
        self.approvals.retain(|r| r.expires > now + margin);
        self.approvals.len() != before
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn has_prompt(&self, pane_id: &str) -> bool {
        self.approvals.iter().any(|r| r.pane_id == pane_id) || self.questions.iter().any(|q| q.pane_id == pane_id)
    }

    /// Blocked panes with neither an approval nor a question, in table order.
    fn needs_you<'a>(&'a self, agents: &'a AgentTable) -> impl Iterator<Item = &'a Agent> + 'a {
        agents
            .rows(usize::MAX)
            .into_iter()
            .filter(move |a| a.info.status == AgentStatus::Blocked && !self.has_prompt(&a.info.pane_id))
    }

    /// What to show, in order: approvals, questions (not hidden by Later), then Blocked panes with neither.
    pub fn pending(&self, agents: &AgentTable, hooks: &HookStatus) -> Vec<Pending> {
        let approvals = self
            .approvals
            .iter()
            .map(|r| Pending { pane_id: r.pane_id.clone(), kind: PendingKind::Approval(r.clone()) });
        let questions = self
            .questions
            .iter()
            .filter(|q| !self.later.contains(&q.pane_id))
            .map(|q| Pending { pane_id: q.pane_id.clone(), kind: PendingKind::Question(q.text.clone()) });
        let needs_you = self.needs_you(agents).map(|a| {
            let hook_missing =
                a.info.agent.as_ref().is_some_and(|name| hooks.0.get(&name.to_lowercase()) == Some(&false));
            Pending { pane_id: a.info.pane_id.clone(), kind: PendingKind::NeedsYou { hook_missing } }
        });
        approvals.chain(questions).chain(needs_you).collect()
    }

    /// Counts what `pending` would show, plus questions hidden by Later; capped at 255.
    pub fn badge(&self, agents: &AgentTable) -> u8 {
        (self.approvals.len() + self.questions.len() + self.needs_you(agents).count()).min(255) as u8
    }
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

    use crate::agents::test_info;
    use crate::herdr::AgentStatus::*;

    fn req(agent: &str) -> ApprovalRequest {
        ApprovalRequest {
            agent: agent.into(),
            tool: "Edit".into(),
            summary: "s".into(),
            detail: None,
            can_always: true,
            scope: Default::default(),
        }
    }

    fn table(panes: &[(&str, AgentStatus)], now: Instant) -> AgentTable {
        let mut t = AgentTable::default();
        for (p, s) in panes {
            t.upsert(test_info(p, *s), now);
        }
        t
    }

    fn pending(q: &ApprovalQueue, agents: &AgentTable) -> Vec<(String, PendingKind)> {
        q.pending(agents, &HookStatus::default()).into_iter().map(|p| (p.pane_id, p.kind)).collect()
    }

    #[test]
    fn order_is_approval_question_needs_you() {
        let t0 = Instant::now();
        let agents = table(&[("A", Blocked), ("B", Blocked), ("C", Blocked)], t0);
        let mut q = ApprovalQueue::default();
        q.question("B".into(), "Which?".into());
        q.insert("a1".into(), "A".into(), req("claude"), 45000, t0);
        assert_eq!(
            pending(&q, &agents),
            vec![
                ("A".to_string(), PendingKind::Approval(q.approvals[0].clone())),
                ("B".to_string(), PendingKind::Question("Which?".into())),
                ("C".to_string(), PendingKind::NeedsYou { hook_missing: false }),
            ]
        );
    }

    #[test]
    fn two_panes_answer_their_own_ids() {
        let t0 = Instant::now();
        let agents = table(&[("p1", Blocked), ("p2", Blocked)], t0);
        let mut q = ApprovalQueue::default();
        q.insert("a1".into(), "p1".into(), req("claude"), 45000, t0);
        q.insert("a2".into(), "p2".into(), req("claude"), 45000, t0);

        let taken = q.take("a1").expect("a1 is queued");
        assert_eq!((taken.id.as_str(), taken.pane_id.as_str()), ("a1", "p1"));
        // p1 has no prompt left, so it falls back to Needs you; p2 keeps its own approval.
        assert_eq!(
            pending(&q, &agents),
            vec![
                ("p2".to_string(), PendingKind::Approval(q.approvals[0].clone())),
                ("p1".to_string(), PendingKind::NeedsYou { hook_missing: false }),
            ]
        );
        assert!(q.take("a1").is_none());
        assert_eq!(q.take("a2").map(|r| r.pane_id), Some("p2".to_string()));
        // both panes are still Blocked, so with their approvals gone they are Needs you
        assert_eq!(
            pending(&q, &agents),
            vec![
                ("p1".to_string(), PendingKind::NeedsYou { hook_missing: false }),
                ("p2".to_string(), PendingKind::NeedsYou { hook_missing: false }),
            ]
        );
    }

    #[test]
    fn expiry_and_close() {
        let t0 = Instant::now();
        let mut q = ApprovalQueue::default();
        q.insert("a1".into(), "p1".into(), req("claude"), 45000, t0);
        q.insert("a2".into(), "p2".into(), req("claude"), 45000, t0);
        assert!(!q.expire(t0 + Duration::from_millis(43999)));
        // gone a second early, so a late click can't reach a hook that has given up
        assert!(q.expire(t0 + Duration::from_millis(44001)));
        assert!(q.take("a1").is_none() && q.take("a2").is_none());
        q.insert("a3".into(), "p3".into(), req("claude"), 45000, t0);
        q.close("a3");
        assert!(q.take("a3").is_none());
        q.close("nope");
        assert!(!q.expire(t0));
    }

    #[test]
    fn question_clears_when_pane_leaves_blocked() {
        let t0 = Instant::now();
        let mut q = ApprovalQueue::default();
        q.on_status("p", Blocked);
        q.question("p".into(), "Which?".into());
        assert_eq!(q.badge(&table(&[("p", Blocked)], t0)), 1);
        q.on_status("p", Working);
        assert!(pending(&q, &table(&[("p", Working)], t0)).is_empty());
        assert_eq!(q.badge(&table(&[("p", Working)], t0)), 0);
    }

    #[test]
    fn repeated_non_blocked_status_keeps_a_question() {
        let t0 = Instant::now();
        let mut q = ApprovalQueue::default();
        q.on_status("p", Working);
        q.question("p".into(), "Which?".into());
        q.on_status("p", Working);
        assert_eq!(q.badge(&table(&[("p", Working)], t0)), 1);
        q.on_status("p", Blocked);
        q.on_status("p", Idle);
        assert_eq!(q.badge(&table(&[("p", Idle)], t0)), 0);
    }

    #[test]
    fn later_hides_until_next_blocked() {
        let t0 = Instant::now();
        let agents = table(&[("p", Blocked)], t0);
        let mut q = ApprovalQueue::default();
        q.on_status("p", Blocked);
        q.question("p".into(), "Which?".into());
        q.later("p");
        // hidden from the list (and not Needs you either, since the pane still has a question)
        assert!(pending(&q, &agents).is_empty());
        assert_eq!(q.badge(&agents), 1);
        q.on_status("p", Working);
        q.on_status("p", Blocked);
        q.question("p".into(), "Again?".into());
        assert_eq!(pending(&q, &agents), vec![("p".to_string(), PendingKind::Question("Again?".into()))]);
    }

    #[test]
    fn needs_you_flags_missing_hook() {
        let t0 = Instant::now();
        let mut agents = table(&[("k", Blocked)], t0);
        let mut codex = test_info("c", Blocked);
        codex.agent = Some("Codex".into());
        agents.upsert(codex, t0);
        let hooks = HookStatus([("codex".to_string(), false)].into_iter().collect());
        assert_eq!(
            ApprovalQueue::default()
                .pending(&agents, &hooks)
                .into_iter()
                .map(|p| (p.pane_id, p.kind))
                .collect::<Vec<_>>(),
            vec![
                ("c".to_string(), PendingKind::NeedsYou { hook_missing: true }),
                ("k".to_string(), PendingKind::NeedsYou { hook_missing: false }),
            ]
        );
    }

    #[test]
    fn pane_close_removes_everything_for_it() {
        let t0 = Instant::now();
        let mut q = ApprovalQueue::default();
        q.insert("a1".into(), "p1".into(), req("claude"), 45000, t0);
        q.question("p1".into(), "Which?".into());
        q.later("p1");
        q.insert("a2".into(), "p2".into(), req("claude"), 45000, t0);
        q.remove_pane("p1");
        assert!(q.take("a1").is_none());
        assert!(!q.has_prompt("p1"));
        assert!(!q.later.contains("p1"));
        // the agents table is what herdr closes for p1 too; without it, nothing is left for p1
        let agents = table(&[("p2", Blocked)], t0);
        assert_eq!(pending(&q, &agents), vec![("p2".to_string(), PendingKind::Approval(q.approvals[0].clone()))]);
        q.clear();
        assert_eq!(q.badge(&agents), 1);
    }
}
