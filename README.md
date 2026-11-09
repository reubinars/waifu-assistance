# island

A Dynamic Island for Windows, for working with coding agents in WSL. A small pill at the top of the main monitor tells you when an agent in herdr needs you, and one click takes you to its pane. It also shows what's playing (art, title, seekbar, controls). Native Rust (Win32, Direct2D, DirectComposition). It uses no CPU while idle and stays under 20 MB.

## Install

From WSL, with Rust and [`cargo-xwin`](https://github.com/rust-cross/cargo-xwin):

```sh
scripts/install.sh
```

This builds both programs, installs the bridge to `~/.local/bin/island-bridge` and `island.exe` to `%LOCALAPPDATA%\island\`, then starts the island (stopping any copy already running). Run it again to update. The tray icon (a white dot) has Hide/Show, Pill/Attached, Start with Windows and Quit.

## Config

`%LOCALAPPDATA%\island\config.toml` is read at start. Missing keys take their defaults. The tray menu writes it back.

| Key | Default | |
|---|---|---|
| `display_mode` | `"pill"` | `"pill"` floats below the top edge; `"attached"` hangs from it |
| `start_with_windows` | `true` | kept in sync with `HKCU\…\Run\Island` at every start |
| `wsl_distro` | `""` | distro running herdr; `""` = the default distro |
| `bridge_path` | `"$HOME/.local/bin/island-bridge"` | bridge path inside WSL |
| `terminal_window_class` | `"CASCADIA_HOSTING_WINDOW_CLASS"` | window brought forward after a jump (Windows Terminal) |
| `terminal_process` | `""` | terminal executable, e.g. `"warp.exe"`; wins over the class when set |
| `terminal_launch` | `""` | run when a jump finds no terminal window: a URI or a program, e.g. `"warp://tab_config/herdr"` |

Logs go to `%LOCALAPPDATA%\island\island.log`. Pass `--debug` for more detail, or `--demo` for a scripted tour that saves nothing.

## How it finds things

**Agents.** The island never starts WSL. Every 15 s it checks `wsl.exe -l -v`. Once the configured distro is running, it starts `island-bridge` through `wsl.exe`. The bridge relays JSON lines between the island and herdr's socket (`$HERDR_SOCKET_PATH`, else `~/.config/herdr/herdr.sock`). The island subscribes to herdr's agent events, so a blocked agent shows within a moment. Clicking a blocked or done agent focuses its pane in herdr and brings the terminal forward. With no terminal window open, it runs `terminal_launch` instead: for Warp, a tab config whose pane runs `herdr` reattaches to the session on that pane. If WSL, the bridge or herdr is down, the island shows nothing about agents.

**Media.** The island follows Windows' current media session (SMTC), so any player that publishes there works: Spotify, browsers and the like. Controls and seeking go back through the same session.

**Fullscreen.** When the foreground window covers the whole main monitor (a video, a game), the island hides. The one exception is a blocked agent: its pill still shows.

## Develop

`cargo test --workspace --exclude island` runs the pure logic in WSL. `scripts/build.sh` builds release binaries. `target/x86_64-pc-windows-msvc/debug/island.exe` runs straight from WSL.
