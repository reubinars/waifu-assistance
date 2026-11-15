Made assistance to accompany me vibe code

![Both pets beside the status dot](.github/demo.gif)

## Agent approvals

When Claude Code, Codex or OpenCode asks permission for a tool, the island shows the request: a card with Allow, Deny, Always allow (when the agent offers it) and Open terminal buttons in the Agents tab, plus a badge on the collapsed island. If the island is collapsed and no fullscreen window is up, it also pops open without taking focus. It shows the first pending request ("1 of N") with a countdown bar that empties after 45 s, when the agent falls back to its own prompt. Nothing is ever allowed or denied except by a click, and the island never takes keyboard focus.

Hooks are installed into the agent CLIs from WSL:

```
island-bridge hooks install   # add the island's hooks (backs up each file first, idempotent)
island-bridge hooks remove    # remove exactly those entries
```

`scripts/install.sh` runs `hooks install` for you. Codex asks you to trust a newly added hook the first time it runs, so approve that prompt once in Codex.

<!-- wait-result: filled in after hook capture -->
