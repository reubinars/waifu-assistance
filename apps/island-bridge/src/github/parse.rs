//! Turns the `gh api graphql` response into the sorted pull-request list.

use std::collections::HashSet;

use island_core::github::{PullRequest, Role, ci_from_rollup, review_from_decision, sort};
use serde_json::Value;

fn node(v: &Value, role: Role) -> Option<PullRequest> {
    let s = |x: Option<&Value>| x.and_then(Value::as_str).map(str::to_string);
    let rollup = v.pointer("/commits/nodes/0/commit/statusCheckRollup/state").and_then(Value::as_str);
    Some(PullRequest {
        repo: s(v.pointer("/repository/nameWithOwner"))?,
        number: u32::try_from(v.get("number")?.as_u64()?).ok()?,
        title: s(v.get("title")).unwrap_or_default(),
        url: s(v.get("url"))?,
        draft: v.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        updated_at: s(v.get("updatedAt")).unwrap_or_default(),
        role,
        ci: ci_from_rollup(rollup),
        review: review_from_decision(v.get("reviewDecision").and_then(Value::as_str)),
    })
}

pub fn parse_response(json: &str) -> Result<Vec<PullRequest>, String> {
    let unreachable = || "GitHub unreachable".to_string();
    let v: Value = serde_json::from_str(json).map_err(|_| unreachable())?;
    let limited = v
        .get("errors")
        .and_then(Value::as_array)
        .is_some_and(|e| e.iter().any(|x| x.to_string().to_lowercase().contains("rate limit")));
    if limited {
        return Err("GitHub rate limited".to_string());
    }
    let data = v.get("data").filter(|d| d.is_object()).ok_or_else(unreachable)?;
    let mut seen = HashSet::new();
    let mut prs = Vec::new();
    for (key, role) in [("mine", Role::Mine), ("review", Role::ReviewRequested)] {
        let nodes = data.pointer(&format!("/{key}/nodes")).and_then(Value::as_array);
        for pr in nodes.into_iter().flatten().filter_map(|n| node(n, role)) {
            if seen.insert((pr.repo.clone(), pr.number)) {
                prs.push(pr);
            }
        }
    }
    sort(&mut prs);
    Ok(prs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use island_core::github::{Ci, Review, Role};

    const FIXTURE: &str = include_str!("../../tests/fixtures/gh-graphql.json");

    #[test]
    fn parse_fixture() {
        let prs = parse_response(FIXTURE).unwrap();
        assert_eq!(prs.len(), 2);
        let first = &prs[0];
        assert_eq!((first.repo.as_str(), first.number), ("org/repo", 45));
        assert_eq!(first.role, Role::ReviewRequested);
        assert!(first.draft);
        assert_eq!(first.ci, Ci::Failing);
        assert_eq!(first.review, Review::None);
        let second = &prs[1];
        assert_eq!((second.repo.as_str(), second.number), ("reubinars/waifu-assistance", 7));
        assert_eq!(second.role, Role::Mine);
        assert_eq!(second.review, Review::Needed);
        assert_eq!(second.ci, Ci::None);
    }

    #[test]
    fn parse_dedupes_into_mine() {
        let node =
            r#"{"number":1,"title":"t","url":"https://github.com/a/b/pull/1","repository":{"nameWithOwner":"a/b"}}"#;
        let json = format!(r#"{{"data":{{"mine":{{"nodes":[{node}]}},"review":{{"nodes":[{node}]}}}}}}"#);
        let prs = parse_response(&json).unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].role, Role::Mine);
    }

    #[test]
    fn parse_garbage_and_rate_limit() {
        assert_eq!(parse_response("nope"), Err("GitHub unreachable".to_string()));
        assert_eq!(
            parse_response(r#"{"errors":[{"message":"API rate limit exceeded"}]}"#),
            Err("GitHub rate limited".to_string())
        );
    }
}
