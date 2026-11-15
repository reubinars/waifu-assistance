Made assistance to accompany me vibe code

![Both pets beside the status dot](.github/demo.gif)

## Agent approvals

Approvals currently work for Claude Code; Codex and OpenCode are coming. When Claude Code asks permission for a tool, the island shows the request: a card with Allow, Deny, Always allow (when the agent offers it) and Open terminal buttons in the Agents tab, plus a badge on the collapsed island. If the island is collapsed and no fullscreen window is up, it also pops open without taking focus. It shows the first pending request ("1 of N") with a countdown bar that empties after 45 s, when the agent falls back to its own prompt. Nothing is ever allowed or denied except by a click, and the island never takes keyboard focus.

Hooks are installed into the agent CLIs from WSL:

```
island-bridge hooks install   # add the island's hooks (backs up each file first, idempotent)
island-bridge hooks remove    # remove exactly those entries
```

`scripts/install.sh` runs `hooks install` for you. Once Codex support lands, Codex will ask you to trust its newly added hook with `/hooks`.

## GitHub status

The island's GitHub tab lists your open pull requests and the ones where your review is requested, with each PR's CI state and review state. A review chip (Approved, Changes requested, Review needed) shows on your own PRs only when the repo requires reviews; drafts are tagged Draft. Clicking a row opens the PR in your browser.

It uses the `gh` login in WSL, so run `gh auth login` there once. The bridge polls GitHub every 60 s. When CI on one of your PRs fails or passes, a review is requested from you, or a PR of yours is approved or gets changes requested, the collapsed island pops open for 4 s with a one-line note (clicking it opens the PR), and a purple dot stays top-left until you open the GitHub tab. Pops are skipped while a fullscreen window is up or the island is hidden, and wait while an approval is showing.

The feature is read-only: it only queries GitHub and never changes anything there.
