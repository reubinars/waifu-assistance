#!/usr/bin/env bash
# Records a permission hook's stdin for test fixtures, then waits 10 s so we can
# see whether the agent holds its own prompt until the hook returns.
# Usage (as a hook command): capture-hook.sh <name>
set -u
dir="$HOME/.local/state/island/capture"
mkdir -p "$dir"
cat > "$dir/${1:-hook}-$(date +%s%3N).json"
sleep 10
exit 0
