//! Row and pop copy for the GitHub tab, and the URL gate for opening a PR.

use super::{Ci, GhEvent, GhEventKind, GhState, PullRequest, Review, Role};

#[derive(Clone, Debug, PartialEq)]
pub enum GhRow {
    Message(String),
    Pr { pr: PullRequest, dim: bool },
}

pub fn rows(s: &GhState) -> Vec<GhRow> {
    let mut out = Vec::new();
    if let Some(e) = &s.error {
        out.push(GhRow::Message(error_text(e)));
        out.extend(s.prs.iter().map(|p| GhRow::Pr { pr: p.clone(), dim: true }));
    } else if s.prs.is_empty() {
        out.push(GhRow::Message("No open PRs or review requests".to_owned()));
    } else {
        out.extend(s.prs.iter().map(|p| GhRow::Pr { pr: p.clone(), dim: p.draft }));
    }
    out
}

/// `rows(s).len()`, without cloning the list.
pub fn row_count(s: &GhState) -> usize {
    match (&s.error, s.prs.len()) {
        (Some(_), n) => n + 1,
        (None, 0) => 1,
        (None, n) => n,
    }
}

pub fn error_text(e: &str) -> String {
    if e == "gh not logged in" { "gh not logged in — run gh auth login in WSL".to_owned() } else { e.to_owned() }
}

pub fn short_repo(repo: &str) -> &str {
    repo.rsplit('/').next().unwrap_or(repo)
}

pub fn ci_glyph(c: Ci) -> &'static str {
    match c {
        Ci::Passing => "✓",
        Ci::Failing => "✗",
        Ci::Running => "⟳",
        Ci::None => "·",
    }
}

pub fn head(pr: &PullRequest) -> String {
    format!("{}#{}  {}", short_repo(&pr.repo), pr.number, pr.title)
}

/// The review chip and draft tag; empty for a non-draft PR of yours in a repo that needs no review.
pub fn chip(pr: &PullRequest) -> String {
    let review = match (pr.role, pr.review) {
        (Role::ReviewRequested, _) => Some("Your review"),
        (Role::Mine, Review::Approved) => Some("Approved"),
        (Role::Mine, Review::ChangesRequested) => Some("Changes requested"),
        (Role::Mine, Review::Needed) => Some("Review needed"),
        (Role::Mine, Review::None) => None,
    };
    match (review, pr.draft) {
        (Some(r), true) => format!("{r} · Draft"),
        (Some(r), false) => r.to_owned(),
        (None, true) => "Draft".to_owned(),
        (None, false) => String::new(),
    }
}

pub fn pop_text(e: &GhEvent) -> String {
    let lead = match e.kind {
        GhEventKind::CiFailed => "✗ CI failed",
        GhEventKind::CiPassed => "✓ CI passed",
        GhEventKind::ReviewRequested => "👀 Review requested",
        GhEventKind::Approved => "✓ Approved",
        GhEventKind::ChangesRequested => "✎ Changes requested",
    };
    format!("{lead} · {} #{}", short_repo(&e.repo), e.number)
}

/// Only these URLs may reach the shell: GitHub over https, ASCII only, no whitespace, control
/// characters, or characters a URL never carries unescaped.
pub fn is_safe_url(url: &str) -> bool {
    url.starts_with("https://github.com/")
        && url.is_ascii()
        && !url.chars().any(|c| c.is_whitespace() || c.is_control() || "\"<>\\^`{|}".contains(c))
}

#[cfg(test)]
mod tests {
    use super::super::test_pr as pr;
    use super::*;

    #[test]
    fn rows_for_error_empty_and_drafts() {
        let mut s = GhState::default();
        s.apply(Ok(vec![]));
        assert!(matches!(&rows(&s)[..], [GhRow::Message(m)] if m == "No open PRs or review requests"));

        let mut d = pr("o/a", 1, Role::Mine, Ci::None, Review::None);
        d.draft = true;
        s.apply(Ok(vec![d, pr("o/a", 2, Role::Mine, Ci::None, Review::None)]));
        let r = rows(&s);
        assert_eq!(r.len(), 2);
        let dims: Vec<bool> = r.iter().map(|x| matches!(x, GhRow::Pr { dim: true, .. })).collect();
        assert_eq!(dims.iter().filter(|d| **d).count(), 1);

        s.apply(Err("gh not logged in".into()));
        let r = rows(&s);
        assert_eq!(r.len(), 3);
        assert!(matches!(&r[0], GhRow::Message(m) if m == "gh not logged in — run gh auth login in WSL"));
        assert!(r[1..].iter().all(|x| matches!(x, GhRow::Pr { dim: true, .. })));
        assert_eq!(error_text("GitHub unreachable"), "GitHub unreachable");
    }

    #[test]
    fn row_count_matches_rows() {
        let mut s = GhState::default();
        assert_eq!(row_count(&s), rows(&s).len());
        s.apply(Ok(vec![]));
        assert_eq!(row_count(&s), rows(&s).len());
        let list =
            vec![pr("o/a", 1, Role::Mine, Ci::None, Review::None), pr("o/a", 2, Role::Mine, Ci::None, Review::None)];
        s.apply(Ok(list));
        assert_eq!(row_count(&s), 2);
        assert_eq!(row_count(&s), rows(&s).len());
        s.apply(Err("GitHub unreachable".into()));
        assert_eq!(row_count(&s), 3);
        assert_eq!(row_count(&s), rows(&s).len());
    }

    #[test]
    fn chip_copy() {
        let c = |role, review, draft| {
            let mut p = pr("o/a", 1, role, Ci::None, review);
            p.draft = draft;
            chip(&p)
        };
        assert_eq!(c(Role::ReviewRequested, Review::None, false), "Your review");
        assert_eq!(c(Role::Mine, Review::Approved, false), "Approved");
        assert_eq!(c(Role::Mine, Review::ChangesRequested, false), "Changes requested");
        assert_eq!(c(Role::Mine, Review::Needed, false), "Review needed");
        assert_eq!(c(Role::Mine, Review::None, false), "");
        assert_eq!(c(Role::Mine, Review::None, true), "Draft");
        assert_eq!(c(Role::Mine, Review::Approved, true), "Approved · Draft");
    }

    #[test]
    fn pop_copy() {
        let e = |kind| GhEvent { kind, repo: "reubinars/waifu-assistance".into(), number: 12, url: String::new() };
        assert_eq!(pop_text(&e(GhEventKind::CiFailed)), "✗ CI failed · waifu-assistance #12");
        assert_eq!(pop_text(&e(GhEventKind::CiPassed)), "✓ CI passed · waifu-assistance #12");
        assert_eq!(pop_text(&e(GhEventKind::ReviewRequested)), "👀 Review requested · waifu-assistance #12");
        assert_eq!(pop_text(&e(GhEventKind::Approved)), "✓ Approved · waifu-assistance #12");
        assert_eq!(pop_text(&e(GhEventKind::ChangesRequested)), "✎ Changes requested · waifu-assistance #12");
    }

    #[test]
    fn glyphs_and_head() {
        assert_eq!(ci_glyph(Ci::Passing), "✓");
        assert_eq!(ci_glyph(Ci::Failing), "✗");
        assert_eq!(ci_glyph(Ci::Running), "⟳");
        assert_eq!(ci_glyph(Ci::None), "·");
        assert_eq!(head(&pr("o/a", 7, Role::Mine, Ci::None, Review::None)), "a#7  t");
    }

    #[test]
    fn unsafe_urls_are_refused() {
        assert!(is_safe_url("https://github.com/a/b/pull/1"));
        for u in [
            "http://github.com/a",
            "https://github.com.evil.io/x",
            "https://evil.io/https://github.com/",
            "C:\\Windows\\calc.exe",
            "https://github.com/a b",
            "https://github.com/a\n",
            "https://github.com/a\"b",
            "https://github.com/\u{202E}x",
            "https://github.com/a<b",
            "https://github.com/a|b",
        ] {
            assert!(!is_safe_url(u), "{u}");
        }
    }
}
