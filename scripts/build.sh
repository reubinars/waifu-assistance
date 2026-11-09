#!/usr/bin/env bash
# Builds the Linux bridge and the Windows island in release mode.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -p island-bridge
cargo xwin build --release -p island --target x86_64-pc-windows-msvc
