//! The table of herdr agents the island shows, keyed by pane id.

use crate::herdr::{AgentInfo, AgentStatus};
use std::collections::BTreeMap;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct Agent {
    pub info: AgentInfo,
    /// When this agent entered its current status (herdr sends no timestamps).
    pub since: Instant,
}

impl Agent {
    pub fn display_name(&self) -> &str {
        self.info.name.as_deref().or(self.info.agent.as_deref()).unwrap_or("agent")
    }

    pub fn folder(&self) -> &str {
        let cwd = self.info.cwd.as_deref().unwrap_or("").trim_end_matches('/');
        cwd.rsplit('/').next().unwrap_or("")
    }
}

fn rank(s: AgentStatus) -> u8 {
    match s {
        AgentStatus::Blocked => 0,
        AgentStatus::Done => 1,
        AgentStatus::Working => 2,
        AgentStatus::Idle => 3,
        AgentStatus::Unknown => 4,
    }
}

#[derive(Default)]
pub struct AgentTable {
    agents: BTreeMap<String, Agent>,
}

impl AgentTable {
    pub fn replace_all(&mut self, list: Vec<AgentInfo>, now: Instant) {
        self.agents.clear();
        for info in list {
            self.upsert(info, now);
        }
    }

    /// Adds or replaces an agent; `since` is kept when its status is unchanged.
    pub fn upsert(&mut self, info: AgentInfo, now: Instant) {
        let since = match self.agents.get(&info.pane_id) {
            Some(old) if old.info.status == info.status => old.since,
            _ => now,
        };
        self.agents.insert(info.pane_id.clone(), Agent { info, since });
    }

    /// Updates a known pane's status (and title, when given). False if the pane is unknown.
    pub fn set_status(&mut self, pane_id: &str, status: AgentStatus, title: Option<String>, now: Instant) -> bool {
        let Some(a) = self.agents.get_mut(pane_id) else { return false };
        if a.info.status != status {
            a.info.status = status;
            a.since = now;
        }
        if title.is_some() {
            a.info.title = title;
        }
        true
    }

    pub fn remove(&mut self, pane_id: &str) {
        self.agents.remove(pane_id);
    }

    pub fn clear(&mut self) {
        self.agents.clear();
    }

    /// Up to `limit` agents: Blocked, Done, Working, Idle, Unknown; oldest first within each.
    pub fn rows(&self, limit: usize) -> Vec<&Agent> {
        let mut rows: Vec<&Agent> = self.agents.values().collect();
        rows.sort_by_key(|a| (rank(a.info.status), a.since));
        rows.truncate(limit);
        rows
    }

    pub fn count(&self, s: AgentStatus) -> usize {
        self.agents.values().filter(|a| a.info.status == s).count()
    }

    /// The agent that has been in status `s` the longest.
    pub fn first(&self, s: AgentStatus) -> Option<&Agent> {
        self.agents.values().filter(|a| a.info.status == s).min_by_key(|a| a.since)
    }

    pub fn pane_ids(&self) -> Vec<String> {
        self.agents.keys().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }
}

#[cfg(test)]
pub(crate) fn test_info(pane: &str, status: crate::herdr::AgentStatus) -> crate::herdr::AgentInfo {
    crate::herdr::AgentInfo {
        pane_id: pane.into(),
        workspace_id: pane.split(':').next().unwrap_or_default().into(),
        agent: Some("claude".into()),
        name: None,
        title: None,
        cwd: Some("/home/u/projects/learn-anything".into()),
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::AgentStatus::*;
    use crate::herdr::AgentInfo;
    use std::time::{Duration, Instant};

    fn info(pane: &str, s: crate::herdr::AgentStatus) -> AgentInfo {
        test_info(pane, s)
    }

    #[test]
    fn replace_all_drops_ghosts() {
        let t0 = Instant::now();
        let mut t = AgentTable::default();
        t.replace_all(vec![info("w1:p1", Idle), info("w1:p2", Blocked)], t0);
        t.replace_all(vec![info("w1:p2", Blocked)], t0);
        assert_eq!(t.pane_ids(), vec!["w1:p2".to_string()]);
    }

    #[test]
    fn status_change_stamps_since_only_on_change() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(5);
        let mut t = AgentTable::default();
        t.replace_all(vec![info("w1:p1", Working)], t0);
        assert!(t.set_status("w1:p1", Working, None, t1));
        assert_eq!(t.first(Working).unwrap().since, t0);
        assert!(t.set_status("w1:p1", Blocked, Some("fix bug".into()), t1));
        let a = t.first(Blocked).unwrap();
        assert_eq!(a.since, t1);
        assert_eq!(a.info.title.as_deref(), Some("fix bug"));
    }

    #[test]
    fn unknown_pane_status_is_ignored() {
        let mut t = AgentTable::default();
        assert!(!t.set_status("w9:p9", Blocked, None, Instant::now()));
        assert!(t.is_empty());
    }

    #[test]
    fn rows_order_and_limit() {
        let t0 = Instant::now();
        let mut t = AgentTable::default();
        t.upsert(info("a", Idle), t0);
        t.upsert(info("b", Working), t0);
        t.upsert(info("c", Done), t0);
        t.upsert(info("d", Blocked), t0 + Duration::from_secs(2));
        t.upsert(info("e", Blocked), t0 + Duration::from_secs(1));
        let order: Vec<&str> = t.rows(4).iter().map(|a| a.info.pane_id.as_str()).collect();
        assert_eq!(order, vec!["e", "d", "c", "b"]);
        assert_eq!(t.count(Blocked), 2);
        assert_eq!(t.first(Blocked).unwrap().info.pane_id, "e");
    }

    #[test]
    fn missing_fields_fall_back() {
        let a = Agent {
            info: AgentInfo {
                pane_id: "w1:p1".into(),
                workspace_id: "w1".into(),
                agent: None,
                name: None,
                title: None,
                cwd: None,
                status: Blocked,
            },
            since: Instant::now(),
        };
        assert_eq!(a.display_name(), "agent");
        assert_eq!(a.folder(), "");
    }

    #[test]
    fn name_wins_over_agent() {
        let mut i = info("w1:p3", Idle);
        i.name = Some("researcher".into());
        i.agent = Some("codex".into());
        assert_eq!(Agent { info: i, since: Instant::now() }.display_name(), "researcher");
        assert_eq!(Agent { info: info("w1:p1", Idle), since: Instant::now() }.display_name(), "claude");
    }

    #[test]
    fn folder_is_last_segment() {
        let mut i = info("w1:p1", Idle);
        i.cwd = Some("/home/u/projects/dynamic-island/docs/rea/".into());
        assert_eq!(Agent { info: i, since: Instant::now() }.folder(), "rea");
    }
}
