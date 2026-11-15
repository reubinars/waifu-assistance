//! Change detection between two PR snapshots, for notifications.

use std::collections::HashMap;

use super::{Ci, PullRequest, Review, Role};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhEventKind {
    CiFailed,
    CiPassed,
    ReviewRequested,
    Approved,
    ChangesRequested,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhEvent {
    pub kind: GhEventKind,
    pub repo: String,
    pub number: u32,
    pub url: String,
}

/// Events worth a notification going from `old` to `new`, in `new`'s order. Vanished PRs are silent.
pub fn diff(old: &[PullRequest], new: &[PullRequest]) -> Vec<GhEvent> {
    let old: HashMap<(&str, u32), &PullRequest> = old.iter().map(|p| ((p.repo.as_str(), p.number), p)).collect();
    let mut out = Vec::new();
    for p in new {
        let prev = old.get(&(p.repo.as_str(), p.number)).copied();
        let mut push = |kind| {
            out.push(GhEvent { kind, repo: p.repo.clone(), number: p.number, url: p.url.clone() });
        };
        match p.role {
            Role::ReviewRequested => {
                if prev.is_none_or(|o| o.role != Role::ReviewRequested) {
                    push(GhEventKind::ReviewRequested);
                }
            }
            Role::Mine => {
                let prev_ci = prev.map(|o| o.ci);
                if p.ci == Ci::Failing && prev_ci != Some(Ci::Failing) {
                    push(GhEventKind::CiFailed);
                } else if p.ci == Ci::Passing && matches!(prev_ci, Some(Ci::Running | Ci::Failing)) {
                    push(GhEventKind::CiPassed);
                }
                let prev_review = prev.map(|o| o.review);
                if p.review != prev_review.unwrap_or_default() {
                    match p.review {
                        Review::Approved => push(GhEventKind::Approved),
                        Review::ChangesRequested => push(GhEventKind::ChangesRequested),
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::test_pr as pr;
    use super::*;

    fn ev(kind: GhEventKind, repo: &str, n: u32) -> GhEvent {
        GhEvent { kind, repo: repo.into(), number: n, url: format!("https://github.com/{repo}/pull/{n}") }
    }

    #[test]
    fn ci_failing_fires_once() {
        let old = [pr("a", 1, Role::Mine, Ci::Running, Review::None)];
        let new = [pr("a", 1, Role::Mine, Ci::Failing, Review::None)];
        assert_eq!(diff(&old, &new), vec![ev(GhEventKind::CiFailed, "a", 1)]);
        assert_eq!(diff(&new, &new), vec![]);
    }

    #[test]
    fn new_pr_already_failing_fires() {
        let new = [pr("a", 1, Role::Mine, Ci::Failing, Review::None)];
        assert_eq!(diff(&[], &new), vec![ev(GhEventKind::CiFailed, "a", 1)]);
    }

    #[test]
    fn ci_passed_only_after_running_or_failing() {
        let passing = [pr("a", 1, Role::Mine, Ci::Passing, Review::None)];
        for from in [Ci::Running, Ci::Failing] {
            let old = [pr("a", 1, Role::Mine, from, Review::None)];
            assert_eq!(diff(&old, &passing), vec![ev(GhEventKind::CiPassed, "a", 1)]);
        }
        let old = [pr("a", 1, Role::Mine, Ci::None, Review::None)];
        assert_eq!(diff(&old, &passing), vec![]);
        assert_eq!(diff(&[], &passing), vec![]);
    }

    #[test]
    fn new_review_request_fires() {
        let new = [pr("b", 2, Role::ReviewRequested, Ci::None, Review::None)];
        assert_eq!(diff(&[], &new), vec![ev(GhEventKind::ReviewRequested, "b", 2)]);
        assert_eq!(diff(&new, &new), vec![]);
    }

    #[test]
    fn review_decisions_fire_for_mine_only() {
        let needed = [pr("a", 1, Role::Mine, Ci::None, Review::Needed)];
        let approved = [pr("a", 1, Role::Mine, Ci::None, Review::Approved)];
        let changes = [pr("a", 1, Role::Mine, Ci::None, Review::ChangesRequested)];
        assert_eq!(diff(&needed, &approved), vec![ev(GhEventKind::Approved, "a", 1)]);
        assert_eq!(diff(&needed, &changes), vec![ev(GhEventKind::ChangesRequested, "a", 1)]);
        let r_old = [pr("a", 1, Role::ReviewRequested, Ci::None, Review::Needed)];
        let r_new = [pr("a", 1, Role::ReviewRequested, Ci::None, Review::Approved)];
        assert_eq!(diff(&r_old, &r_new), vec![]);
    }

    #[test]
    fn disappearing_pr_is_silent() {
        let old = [pr("a", 1, Role::Mine, Ci::Failing, Review::None)];
        assert_eq!(diff(&old, &[]), vec![]);
    }

    #[test]
    fn new_pr_already_approved_fires() {
        let new = [pr("a", 1, Role::Mine, Ci::None, Review::Approved)];
        assert_eq!(diff(&[], &new), vec![ev(GhEventKind::Approved, "a", 1)]);
    }
}
