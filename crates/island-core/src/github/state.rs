//! The GitHub tab's state: the latest list, bridge errors, and the unseen / pop queues.

use std::collections::VecDeque;

use super::{GhEvent, PullRequest, diff, sort};

pub const MAX_POPS: usize = 3;

#[derive(Default)]
pub struct GhState {
    pub prs: Vec<PullRequest>,
    pub error: Option<String>,
    pub unseen: u32,
    baseline: bool,
    pops: VecDeque<GhEvent>,
}

impl GhState {
    /// The first list after a (re)baseline raises no events; an error keeps the last list and raises none.
    pub fn apply(&mut self, r: Result<Vec<PullRequest>, String>) {
        match r {
            Ok(mut list) => {
                sort(&mut list);
                if self.baseline {
                    for e in diff(&self.prs, &list) {
                        self.unseen += 1;
                        if self.pops.len() < MAX_POPS {
                            self.pops.push_back(e);
                        }
                    }
                }
                self.prs = list;
                self.error = None;
                self.baseline = true;
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn rebaseline(&mut self) {
        self.baseline = false;
        self.pops.clear();
    }

    /// The tab is in view: nothing is unseen, and nothing queued needs to pop.
    pub fn seen(&mut self) {
        self.unseen = 0;
        self.pops.clear();
    }

    pub fn next_pop(&mut self) -> Option<GhEvent> {
        self.pops.pop_front()
    }

    pub fn drop_pops(&mut self) {
        self.pops.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_pr as pr;
    use super::super::{Ci, GhEventKind, Review, Role};
    use super::*;

    fn mine(n: u32, ci: Ci) -> PullRequest {
        pr("o/a", n, Role::Mine, ci, Review::None)
    }

    #[test]
    fn first_list_is_a_baseline() {
        let mut s = GhState::default();
        s.apply(Ok(vec![mine(1, Ci::Failing)]));
        assert_eq!(s.unseen, 0);
        assert_eq!(s.next_pop(), None);
    }

    #[test]
    fn events_count_and_queue() {
        let mut s = GhState::default();
        s.apply(Ok(vec![mine(1, Ci::Running)]));
        s.apply(Ok(vec![mine(1, Ci::Failing)]));
        assert_eq!(s.unseen, 1);
        assert_eq!(s.next_pop().map(|e| e.kind), Some(GhEventKind::CiFailed));
        assert_eq!(s.next_pop(), None);
    }

    #[test]
    fn queue_caps_at_three() {
        let mut s = GhState::default();
        s.apply(Ok(vec![]));
        s.apply(Ok((1..=5).map(|n| pr("o/a", n, Role::ReviewRequested, Ci::None, Review::None)).collect()));
        assert_eq!(s.unseen, 5);
        let mut n = 0;
        while s.next_pop().is_some() {
            n += 1;
        }
        assert_eq!(n, 3);
    }

    #[test]
    fn no_events_across_an_error() {
        let mut s = GhState::default();
        s.apply(Ok(vec![mine(1, Ci::Running)]));
        s.apply(Err("GitHub unreachable".into()));
        assert_eq!(s.prs.len(), 1);
        assert!(s.error.is_some());
        s.apply(Ok(vec![mine(1, Ci::Running)]));
        assert_eq!(s.unseen, 0);
        assert_eq!(s.next_pop(), None);
        assert!(s.error.is_none());
    }

    #[test]
    fn seen_resets_unseen() {
        let mut s = GhState::default();
        s.apply(Ok(vec![mine(1, Ci::Running)]));
        s.apply(Ok(vec![mine(1, Ci::Failing)]));
        s.seen();
        assert_eq!(s.unseen, 0);
        // what the tab shows needs no pop either
        assert_eq!(s.next_pop(), None);
    }

    #[test]
    fn drop_pops_keeps_unseen() {
        let mut s = GhState::default();
        s.apply(Ok(vec![mine(1, Ci::Running)]));
        s.apply(Ok(vec![mine(1, Ci::Failing)]));
        s.drop_pops();
        assert_eq!(s.next_pop(), None);
        assert_eq!(s.unseen, 1);
    }
}
