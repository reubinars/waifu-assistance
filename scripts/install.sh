#!/usr/bin/env bash
# Builds both programs, installs the bridge into WSL and island.exe into %LOCALAPPDATA%\island,
# then (re)starts the island. Run from WSL.
set -euo pipefail
cd "$(dirname "$0")/.."

scripts/build.sh

# cmd.exe warns about (and refuses) a UNC working directory, so call it from a Windows drive
win() { (cd /mnt/c && cmd.exe /c "$@" 2>/dev/null | tr -d '\r'); }
appdata=$(wslpath "$(win 'echo %LOCALAPPDATA%')")
dest="$appdata/island"

# stopping the island also ends its bridge (stdin closes), freeing both binaries
taskkill.exe /IM island.exe /F >/dev/null 2>&1 || true
sleep 0.5

install -D -m 755 target/release/island-bridge "$HOME/.local/bin/island-bridge"
echo "bridge  -> $HOME/.local/bin/island-bridge"

mkdir -p "$dest"
cp target/x86_64-pc-windows-msvc/release/island.exe "$dest/island.exe"
echo "island  -> $dest/island.exe"

# with start_with_windows = true (the default) the island points the Run key at itself on launch
# Start-Process shares no handles with WSL; a `cmd.exe /c start` child keeps the interop pipes
# open and the script never returns
(cd /mnt/c && powershell.exe -NoProfile -Command 'Start-Process "$env:LOCALAPPDATA\island\island.exe"' </dev/null)
echo "started"
