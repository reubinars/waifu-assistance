//! GitHub PR / CI status: the model, the bridge wire format, ordering, and change detection.

use serde::{Deserialize, Serialize};

use crate::hooks::{cap_to, sanitize};

mod diff;
pub mod pop;
mod state;
pub mod text;
pub use diff::{GhEvent, GhEventKind, diff};
pub use pop::{POP_SECS, PopSlot, PopWorld};
pub use state::{GhState, MAX_POPS};
pub use text::GhRow;

const MAX_TEXT: usize = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Mine,
    ReviewRequested,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ci {
    Passing,
    Failing,
    Running,
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Review {
    Approved,
    ChangesRequested,
    Needed,
    #[default]
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub repo: String,
    pub number: u32,
    pub title: String,
    pub url: String,
    pub draft: bool,
    pub updated_at: String,
    pub role: Role,
    #[serde(default)]
    pub ci: Ci,
    #[serde(default)]
    pub review: Review,
}

pub fn ci_from_rollup(state: Option<&str>) -> Ci {
    match state {
        Some("SUCCESS") => Ci::Passing,
        Some("FAILURE" | "ERROR") => Ci::Failing,
        Some("PENDING" | "EXPECTED") => Ci::Running,
        _ => Ci::None,
    }
}

pub fn review_from_decision(d: Option<&str>) -> Review {
    match d {
        Some("APPROVED") => Review::Approved,
        Some("CHANGES_REQUESTED") => Review::ChangesRequested,
        Some("REVIEW_REQUIRED") => Review::Needed,
        _ => Review::None,
    }
}

/// The bridge's `{"github":{...}}` line, without a trailing newline.
pub fn github_line(r: &Result<Vec<PullRequest>, String>) -> String {
    match r {
        Ok(prs) => serde_json::json!({ "github": { "prs": prs } }),
        Err(e) => serde_json::json!({ "github": { "error": e } }),
    }
    .to_string()
}

/// Parses the object under the `"github"` key. PRs that don't parse are dropped; `None` if neither key is present.
pub fn parse_github(v: &serde_json::Value) -> Option<Result<Vec<PullRequest>, String>> {
    if let Some(items) = v.get("prs").and_then(|p| p.as_array()) {
        let prs = items
            .iter()
            .filter_map(|i| serde_json::from_value::<PullRequest>(i.clone()).ok())
            .map(|mut p| {
                p.title = cap_to(&sanitize(&p.title, false), MAX_TEXT);
                p.repo = cap_to(&sanitize(&p.repo, false), MAX_TEXT);
                p
            })
            .collect();
        return Some(Ok(prs));
    }
    v.get("error").and_then(|e| e.as_str()).map(|e| Err(e.to_owned()))
}

pub fn needs_you(pr: &PullRequest) -> bool {
    pr.role == Role::ReviewRequested
        || (pr.role == Role::Mine && (pr.ci == Ci::Failing || pr.review == Review::ChangesRequested))
}

/// Needs-you first, then newest first, then by repo and number.
pub fn sort(prs: &mut [PullRequest]) {
    prs.sort_by(|a, b| {
        needs_you(b)
            .cmp(&needs_you(a))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.repo.cmp(&b.repo))
            .then_with(|| a.number.cmp(&b.number))
    });
}

#[cfg(test)]
pub(crate) fn test_pr(repo: &str, n: u32, role: Role, ci: Ci, review: Review) -> PullRequest {
    PullRequest {
        repo: repo.to_owned(),
        number: n,
        title: "t".to_owned(),
        url: format!("https://github.com/{repo}/pull/{n}"),
        draft: false,
        updated_at: "2026-11-15T00:00:00Z".to_owned(),
        role,
        ci,
        review,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(repo: &str, n: u32, role: Role, ci: Ci, review: Review) -> PullRequest {
        test_pr(repo, n, role, ci, review)
    }

    #[test]
    fn sort_puts_needs_you_first_then_newest() {
        let mut ok = pr("a", 1, Role::Mine, Ci::Passing, Review::None);
        ok.updated_at = "2026-11-15T03:00:00Z".into();
        let mut req = pr("a", 2, Role::ReviewRequested, Ci::None, Review::None);
        req.updated_at = "2026-11-15T02:00:00Z".into();
        let mut fail = pr("a", 3, Role::Mine, Ci::Failing, Review::None);
        fail.updated_at = "2026-11-15T01:00:00Z".into();
        let mut v = vec![ok.clone(), req.clone(), fail.clone()];
        sort(&mut v);
        assert_eq!(v, vec![req, fail, ok]);

        let mut v = vec![
            pr("b", 1, Role::Mine, Ci::None, Review::None),
            pr("a", 2, Role::Mine, Ci::None, Review::None),
            pr("a", 1, Role::Mine, Ci::None, Review::None),
        ];
        sort(&mut v);
        let ids: Vec<_> = v.iter().map(|p| (p.repo.as_str(), p.number)).collect();
        assert_eq!(ids, vec![("a", 1), ("a", 2), ("b", 1)]);
    }

    #[test]
    fn rollup_and_decision_mapping() {
        assert_eq!(ci_from_rollup(Some("SUCCESS")), Ci::Passing);
        assert_eq!(ci_from_rollup(Some("FAILURE")), Ci::Failing);
        assert_eq!(ci_from_rollup(Some("ERROR")), Ci::Failing);
        assert_eq!(ci_from_rollup(Some("PENDING")), Ci::Running);
        assert_eq!(ci_from_rollup(Some("EXPECTED")), Ci::Running);
        assert_eq!(ci_from_rollup(Some("WHAT")), Ci::None);
        assert_eq!(ci_from_rollup(None), Ci::None);
        assert_eq!(review_from_decision(Some("APPROVED")), Review::Approved);
        assert_eq!(review_from_decision(Some("CHANGES_REQUESTED")), Review::ChangesRequested);
        assert_eq!(review_from_decision(Some("REVIEW_REQUIRED")), Review::Needed);
        assert_eq!(review_from_decision(Some("x")), Review::None);
        assert_eq!(review_from_decision(None), Review::None);
    }

    #[test]
    fn wire_round_trip() {
        let r = Ok(vec![
            pr("a/b", 1, Role::Mine, Ci::Failing, Review::Needed),
            pr("c/d", 2, Role::ReviewRequested, Ci::None, Review::None),
        ]);
        let v: serde_json::Value = serde_json::from_str(&github_line(&r)).unwrap();
        assert_eq!(parse_github(v.get("github").unwrap()), Some(r));
        let r = Err("gh not logged in".to_owned());
        let v: serde_json::Value = serde_json::from_str(&github_line(&r)).unwrap();
        assert_eq!(parse_github(v.get("github").unwrap()), Some(r));
        assert_eq!(parse_github(&serde_json::json!({})), None);
    }

    #[test]
    fn parse_sanitizes_and_caps_title() {
        let long = "y".repeat(200);
        let mut a = pr("a", 1, Role::Mine, Ci::None, Review::None);
        a.title = "x\u{202E}y".into();
        let mut b = pr("a", 2, Role::Mine, Ci::None, Review::None);
        b.title = long.clone();
        let mut v = serde_json::json!({ "prs": [a, b, { "repo": "a", "title": "no number" }] });
        let parsed = parse_github(&v).unwrap().unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].title, "x?y");
        assert_eq!(parsed[1].title, cap_to(&long, 120));
        assert!(parsed[1].title.ends_with('…'));
        v["prs"] = serde_json::json!([]);
        assert_eq!(parse_github(&v), Some(Ok(vec![])));
    }
}
